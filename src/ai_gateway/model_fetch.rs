//! 从上游 provider 拉取模型列表的共享实现：候选 URL 推导、响应解析、
//! DeepSeek 型号过滤与响应预览截断。
//!
//! 本模块是版本化管理 API
//! （`POST /api/v1/manage/gateway/provider/models/fetch`）使用的异步实现。

use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use super::config::{ProviderType, provider_api_root};
use super::templates;

/// 每个候选 URL 的拉取超时。管理 API 语义固定为 15 秒/次。
pub const MODEL_LIST_FETCH_TIMEOUT: Duration = Duration::from_secs(15);

/// 响应体预览的最大字符数。
const PREVIEW_MAX_CHARS: usize = 240;

/// 单个候选 URL 的一次拉取尝试结果。不携带任何鉴权信息。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FetchAttempt {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

/// 上游声明的单个模型：只有上游明确给出的字段才会带上。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FetchedModel {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_image_input: Option<bool>,
}

/// 拉取结果：`models` 为 `Some` 表示某个候选成功并解析出非空模型列表；
/// `attempts` 按尝试顺序记录每个候选（含成功那一次）。
#[derive(Debug)]
pub struct FetchOutcome {
    pub models: Option<Vec<FetchedModel>>,
    pub attempts: Vec<FetchAttempt>,
}

/// 构造候选 models URL 列表：显式 `models_url`（及其展开变体）→
/// `{base}/models` → `{root}/v1/models` →
/// 未显式配置 `models_url` 时追加已知服务的兜底地址。
pub fn model_list_candidates(
    base_url: &str,
    models_url: Option<&str>,
    fallback_models_url: Option<&str>,
) -> Vec<String> {
    let raw = base_url.trim().trim_end_matches('/');
    if raw.is_empty() {
        return Vec::new();
    }

    let mut candidates = Vec::new();
    if let Some(models_url) = models_url.map(str::trim).filter(|value| !value.is_empty()) {
        push_configured_candidates(&mut candidates, models_url);
    }

    let root = provider_api_root(raw);
    push_candidate(&mut candidates, format!("{raw}/models"));
    push_candidate(&mut candidates, format!("{root}/v1/models"));
    if models_url
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_none()
        && let Some(fallback_models_url) = fallback_models_url
            .map(str::trim)
            .filter(|value| !value.is_empty())
    {
        push_configured_candidates(&mut candidates, fallback_models_url);
    }
    candidates
}

fn push_configured_candidates(candidates: &mut Vec<String>, models_url: &str) {
    let configured = models_url.trim().trim_end_matches('/');
    if configured.is_empty() {
        return;
    }

    if configured.to_ascii_lowercase().ends_with("/models") {
        push_candidate(candidates, configured.to_string());
        return;
    }

    push_candidate(candidates, format!("{configured}/models"));
    let root = provider_api_root(configured);
    push_candidate(candidates, format!("{root}/v1/models"));
}

fn push_candidate(candidates: &mut Vec<String>, url: String) {
    if !candidates.iter().any(|candidate| candidate == &url) {
        candidates.push(url);
    }
}

/// 按 provider 元数据推导「已知服务」的官方 models 地址。
///
/// GLM/智谱（名称或 Anthropic 兼容 profile 命中，且 Base URL 指向 open.bigmodel.cn）返回 GLM
/// 模板中的官方 models 地址。管理 API 的请求体不携带 compatibility，因此
/// 额外接受 `provider_type == AnthropicMessages` 作为等价信号——Base URL
/// 指向 open.bigmodel.cn 的模板即 GLM，其类型同为 Anthropic Messages。
pub fn known_models_url(
    provider_name: Option<&str>,
    provider_type: &ProviderType,
    compatibility: Option<&str>,
    base_url: &str,
) -> Option<String> {
    let provider_name = provider_name.unwrap_or_default().trim();
    let base_url = base_url.trim().to_ascii_lowercase();
    let glm_signal = matches!(compatibility, Some("glm_anthropic" | "zhipu_anthropic"))
        || provider_name.eq_ignore_ascii_case("glm")
        || provider_name.eq_ignore_ascii_case("zhipu")
        || *provider_type == ProviderType::AnthropicMessages;
    if glm_signal && base_url.contains("open.bigmodel.cn") {
        return templates::glm_template().models_url.map(str::to_string);
    }
    None
}

/// 从 JSON 响应中提取模型 id：支持根数组 / `data` 数组 / `models` 数组三种形态，
/// 元素取 `id`、`slug` 字段或字符串本身；去重保序。
#[cfg(test)]
pub fn extract_model_ids(value: &Value) -> Vec<String> {
    extract_models(value)
        .into_iter()
        .map(|model| model.id)
        .collect()
}

/// 从 JSON 响应中提取模型及其上游声明过的元数据。
///
/// id 兼容 `id` / `slug` / `model` / 字符串本身；展示名兼容 `display_name` /
/// `displayName` / `name`；上下文窗口兼容 `context_window` / `contextWindow` /
/// `context_length` / `max_context_window`；图片能力只在 `input_modalities`、
/// `modalities`、`supports_image_input` 明确包含/声明图片时才认为是真。
/// 上游没声明的一律留空，不做推测。
pub fn extract_models(value: &Value) -> Vec<FetchedModel> {
    let items = value
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| value.get("models").and_then(Value::as_array))
        .or_else(|| value.as_array());
    let Some(items) = items else {
        return Vec::new();
    };

    let mut models: Vec<FetchedModel> = Vec::new();
    for item in items {
        let Some(id) = model_id(item) else {
            continue;
        };
        if models.iter().any(|existing| existing.id == id) {
            continue;
        }
        models.push(FetchedModel {
            display_name: model_display_name(item, &id),
            context_window: model_context_window(item),
            supports_image_input: model_supports_image_input(item),
            id,
        });
    }
    models
}

fn model_id(item: &Value) -> Option<String> {
    item.as_str()
        .or_else(|| item.get("id").and_then(Value::as_str))
        .or_else(|| item.get("slug").and_then(Value::as_str))
        .or_else(|| item.get("model").and_then(Value::as_str))
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(ToOwned::to_owned)
}

fn model_display_name(item: &Value, id: &str) -> Option<String> {
    let name = ["display_name", "displayName", "name"]
        .iter()
        .find_map(|key| item.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|name| !name.is_empty() && *name != id)?;
    Some(name.to_string())
}

fn model_context_window(item: &Value) -> Option<u64> {
    for key in [
        "context_window",
        "contextWindow",
        "context_length",
        "contextLength",
        "max_context_window",
        "maxContextWindow",
    ] {
        if let Some(value) = item.get(key).and_then(Value::as_u64)
            && value > 0
        {
            return Some(value);
        }
    }
    None
}

fn model_supports_image_input(item: &Value) -> Option<bool> {
    if let Some(value) = item
        .get("supports_image_input")
        .or_else(|| item.get("supportsImageInput"))
        .and_then(Value::as_bool)
    {
        return Some(value);
    }
    for key in ["input_modalities", "inputModalities", "modalities"] {
        if let Some(values) = item.get(key).and_then(Value::as_array) {
            return Some(values.iter().any(|value| {
                value
                    .as_str()
                    .is_some_and(|text| text.eq_ignore_ascii_case("image"))
            }));
        }
    }
    None
}

/// DeepSeek Responses 专用过滤：只保留 Pro 和 Flash（大小写不敏感，
/// 允许 `vendor/deepseek-v4-pro` 之类带命名空间前缀的 id）。其他协议
/// 不假定具体厂商，因此不过滤。
pub fn filter_fetched_models_for_provider(
    provider_type: &ProviderType,
    models: Vec<FetchedModel>,
) -> Vec<FetchedModel> {
    if provider_type != &ProviderType::DeepSeekResponses {
        return models;
    }

    models
        .into_iter()
        .filter(|model| {
            model.id.trim().rsplit('/').next().is_some_and(|slug| {
                slug.eq_ignore_ascii_case("deepseek-v4-pro")
                    || slug.eq_ignore_ascii_case("deepseek-v4-flash")
            })
        })
        .collect()
}

/// 响应体预览：截断到 240 字符并压平换行/制表符，避免把长响应回显给客户端。
pub fn response_preview(body: &str) -> String {
    let preview: String = body.chars().take(PREVIEW_MAX_CHARS).collect();
    preview.replace(['\r', '\n', '\t'], " ")
}

/// 逐个候选 URL 发 GET 拉取模型列表，第一个成功且解析出非空列表的候选即停止。
///
/// `client` 必须来自 `crate::outbound_http`，以复用 daemon 的出站代理设置；
/// 有 `api_key` 时携带 `Authorization: Bearer` 头，但 attempts 里绝不回显它。
pub async fn fetch_models(
    client: &reqwest::Client,
    candidates: &[String],
    api_key: &str,
    timeout: Duration,
) -> FetchOutcome {
    let api_key = api_key.trim();
    let mut attempts = Vec::new();
    for url in candidates {
        let mut request = client.get(url).timeout(timeout);
        if !api_key.is_empty() {
            request = request.header(reqwest::header::AUTHORIZATION, format!("Bearer {api_key}"));
        }

        let response = match request.send().await {
            Ok(response) => response,
            Err(err) => {
                attempts.push(FetchAttempt {
                    url: url.clone(),
                    status: None,
                    error: Some(err.to_string()),
                    preview: None,
                });
                continue;
            }
        };
        let status = response.status();
        let body = match response.text().await {
            Ok(body) => body,
            Err(err) => {
                attempts.push(FetchAttempt {
                    url: url.clone(),
                    status: Some(status.as_u16()),
                    error: Some(err.to_string()),
                    preview: None,
                });
                continue;
            }
        };
        if !status.is_success() {
            attempts.push(FetchAttempt {
                url: url.clone(),
                status: Some(status.as_u16()),
                error: None,
                preview: Some(response_preview(&body)),
            });
            continue;
        }

        match serde_json::from_str::<Value>(&body) {
            Ok(json) => {
                let models = extract_models(&json);
                if models.is_empty() {
                    attempts.push(FetchAttempt {
                        url: url.clone(),
                        status: Some(status.as_u16()),
                        error: Some("response contained no model ids".to_string()),
                        preview: Some(response_preview(&body)),
                    });
                    continue;
                }
                attempts.push(FetchAttempt {
                    url: url.clone(),
                    status: Some(status.as_u16()),
                    error: None,
                    preview: None,
                });
                return FetchOutcome {
                    models: Some(models),
                    attempts,
                };
            }
            Err(err) => {
                attempts.push(FetchAttempt {
                    url: url.clone(),
                    status: Some(status.as_u16()),
                    error: Some(format!("response is not JSON ({err})")),
                    preview: Some(response_preview(&body)),
                });
            }
        }
    }
    FetchOutcome {
        models: None,
        attempts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn model_list_candidates_follow_legacy_order_and_dedup() {
        // 显式 models_url 优先；base 的标准变体去重；显式配置时忽略兜底地址。
        assert_eq!(
            model_list_candidates(
                "https://api.example.com/v1/",
                Some("https://models.example.com/custom/models"),
                Some("https://fallback.example.com/v4/models"),
            ),
            vec![
                "https://models.example.com/custom/models".to_string(),
                "https://api.example.com/v1/models".to_string(),
            ]
        );

        // 不带 /models 结尾的显式配置会展开成两个变体。
        assert_eq!(
            model_list_candidates(
                "https://api.example.com/v1",
                Some("https://gateway.example.com/v1"),
                None,
            ),
            vec![
                "https://gateway.example.com/v1/models".to_string(),
                "https://api.example.com/v1/models".to_string(),
            ]
        );

        // 无显式 models_url 时，已知服务兜底地址排在标准变体之后。
        assert_eq!(
            model_list_candidates(
                "https://open.bigmodel.cn/api/anthropic",
                None,
                Some("https://open.bigmodel.cn/api/paas/v4/models"),
            ),
            vec![
                "https://open.bigmodel.cn/api/anthropic/models".to_string(),
                "https://open.bigmodel.cn/api/anthropic/v1/models".to_string(),
                "https://open.bigmodel.cn/api/paas/v4/models".to_string(),
            ]
        );

        // 空 base URL 没有候选。
        assert!(model_list_candidates("  ", Some("https://x.example/models"), None).is_empty());
    }

    #[test]
    fn extract_model_ids_supports_three_shapes_and_dedups() {
        let data = json!({
            "data": [
                { "id": "model-a" },
                { "id": " model-b " },
                { "id": "model-a" },
                { "slug": "model-c" },
                { "name": "ignored" },
                ""
            ]
        });
        assert_eq!(
            extract_model_ids(&data),
            vec!["model-a", "model-b", "model-c"]
        );

        let models = json!({ "models": [{ "id": "m1" }, "m2"] });
        assert_eq!(extract_model_ids(&models), vec!["m1", "m2"]);

        let root = json!(["x", { "id": "y" }, "x"]);
        assert_eq!(extract_model_ids(&root), vec!["x", "y"]);

        // data 优先于 models。
        let both = json!({ "data": [{ "id": "d" }], "models": [{ "id": "m" }] });
        assert_eq!(extract_model_ids(&both), vec!["d"]);

        assert!(extract_model_ids(&json!({ "object": "list" })).is_empty());
    }

    fn fetched(id: &str) -> FetchedModel {
        FetchedModel {
            id: id.to_string(),
            display_name: None,
            context_window: None,
            supports_image_input: None,
        }
    }

    fn fetched_ids(models: &[FetchedModel]) -> Vec<&str> {
        models.iter().map(|model| model.id.as_str()).collect()
    }

    #[test]
    fn deepseek_responses_accepts_pro_and_flash_without_filtering_other_protocols() {
        let models = vec![
            fetched("deepseek-v4-pro"),
            fetched("DeepSeek-V4-Pro"),
            fetched("vendor/deepseek-v4-pro"),
            fetched("deepseek-v4-flash"),
            fetched("ns/deepseek-v4-flash"),
            fetched("other-model"),
        ];

        let all = fetched_ids(&models)
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        assert_eq!(
            fetched_ids(&filter_fetched_models_for_provider(
                &ProviderType::ChatCompletions,
                models.clone()
            ))
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>(),
            all.clone()
        );
        assert_eq!(
            fetched_ids(&filter_fetched_models_for_provider(
                &ProviderType::DeepSeekResponses,
                models.clone()
            )),
            vec![
                "deepseek-v4-pro",
                "DeepSeek-V4-Pro",
                "vendor/deepseek-v4-pro",
                "deepseek-v4-flash",
                "ns/deepseek-v4-flash",
            ]
        );
        assert_eq!(
            fetched_ids(&filter_fetched_models_for_provider(
                &ProviderType::OpenAiResponses,
                models
            ))
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>(),
            all
        );
    }

    #[test]
    fn extract_models_reads_only_upstream_declared_metadata() {
        let value = json!({
            "data": [
                {
                    "id": "deepseek-v4.1-flash",
                    "display_name": "Deepseek-V4.1-Flash",
                    "context_window": 1048576,
                    "input_modalities": ["text", "image"]
                },
                { "id": "plain-model" },
                { "id": "name-only", "name": "Name Only" },
                { "slug": "slug-model", "context_length": 128000 }
            ]
        });

        let models = extract_models(&value);
        assert_eq!(
            models
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            vec![
                "deepseek-v4.1-flash",
                "plain-model",
                "name-only",
                "slug-model"
            ]
        );
        assert_eq!(
            models[0].display_name.as_deref(),
            Some("Deepseek-V4.1-Flash")
        );
        assert_eq!(models[0].context_window, Some(1_048_576));
        assert_eq!(models[0].supports_image_input, Some(true));
        // 上游没声明的一律留空，不做推测。
        assert_eq!(models[1].display_name, None);
        assert_eq!(models[1].context_window, None);
        assert_eq!(models[1].supports_image_input, None);
        assert_eq!(models[2].display_name.as_deref(), Some("Name Only"));
        assert_eq!(models[3].context_window, Some(128_000));
        // 展示名与 id 相同时不重复记录。
        let same = json!({ "data": [{ "id": "same", "display_name": "same" }] });
        assert_eq!(extract_models(&same)[0].display_name, None);
        // 明确声明不支持图片时按上游口径记录。
        let text_only = json!({ "data": [{ "id": "t", "input_modalities": ["text"] }] });
        assert_eq!(
            extract_models(&text_only)[0].supports_image_input,
            Some(false)
        );
    }

    #[test]
    fn response_preview_truncates_and_flattens_whitespace() {
        let body = format!("line1\r\nline2\t{}", "x".repeat(300));
        let preview = response_preview(&body);
        assert_eq!(preview.chars().count(), 240);
        assert!(!preview.contains('\n'));
        assert!(!preview.contains('\r'));
        assert!(!preview.contains('\t'));
        assert!(preview.starts_with("line1  line2 x"));

        assert_eq!(response_preview("short"), "short");
    }

    #[test]
    fn known_models_url_detects_glm_endpoints() {
        let glm_url = Some("https://open.bigmodel.cn/api/paas/v4/models".to_string());
        assert_eq!(
            known_models_url(
                Some("glm"),
                &ProviderType::AnthropicMessages,
                None,
                "https://open.bigmodel.cn/api/anthropic",
            ),
            glm_url
        );
        assert_eq!(
            known_models_url(
                Some("my-provider"),
                &ProviderType::AnthropicMessages,
                Some("glm_anthropic"),
                "https://OPEN.BIGMODEL.CN/api/anthropic",
            ),
            glm_url
        );
        assert_eq!(
            known_models_url(
                Some("zhipu"),
                &ProviderType::ChatCompletions,
                None,
                "https://open.bigmodel.cn/api/paas/v4",
            ),
            glm_url
        );
        // 请求体不带 compatibility 时，Anthropic Messages 类型即视为 GLM 信号。
        assert_eq!(
            known_models_url(
                None,
                &ProviderType::AnthropicMessages,
                None,
                "https://open.bigmodel.cn/api/anthropic",
            ),
            glm_url
        );
        // Base URL 不指向 open.bigmodel.cn 时不返回兜底地址。
        assert_eq!(
            known_models_url(
                Some("glm"),
                &ProviderType::AnthropicMessages,
                Some("glm_anthropic"),
                "https://api.anthropic.com/v1",
            ),
            None
        );
        // 没有任何 GLM 信号时不返回兜底地址。
        assert_eq!(
            known_models_url(
                Some("custom"),
                &ProviderType::OpenAiResponses,
                None,
                "https://open.bigmodel.cn/api/anthropic",
            ),
            None
        );
    }
}
