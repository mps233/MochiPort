//! 从上游 provider 拉取模型列表的共享实现：候选 URL 推导、响应解析、
//! DeepSeek 型号过滤与响应预览截断。
//!
//! 逻辑对齐 `src/gui.rs` 的 `fetch_remote_models`（及其辅助函数
//! `model_list_candidates`、`extract_model_ids`、`filter_fetched_models_for_provider`、
//! `push_model_items`、`response_preview`、`known_models_url_for_provider`、
//! `is_default_models_url_for_base` 的推导语义），修改时需同步两处。
//! 旧 wxDragon GUI 处于维护模式，仍保留其阻塞式副本；本模块是版本化管理 API
//! （`POST /api/v1/manage/gateway/provider/models/fetch`）使用的异步版本。

use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use super::config::{ProviderType, provider_api_root};
use super::templates;

/// 每个候选 URL 的拉取超时。管理 API 语义固定为 15 秒/次（旧 GUI 为 30 秒）。
pub const MODEL_LIST_FETCH_TIMEOUT: Duration = Duration::from_secs(15);

/// 响应体预览的最大字符数，与旧 GUI 的 `response_preview` 一致。
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

/// 拉取结果：`models` 为 `Some` 表示某个候选成功并解析出非空模型列表；
/// `attempts` 按尝试顺序记录每个候选（含成功那一次）。
#[derive(Debug)]
pub struct FetchOutcome {
    pub models: Option<Vec<String>>,
    pub attempts: Vec<FetchAttempt>,
}

/// 构造候选 models URL 列表，顺序与去重对齐旧 GUI 的 `model_list_candidates`：
/// 显式 `models_url`（及其展开变体）→ `{base}/models` → `{root}/v1/models` →
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
/// 对齐 `src/gui.rs` 的 `known_models_url_for_provider`：GLM/智谱（名称或
/// Anthropic 兼容 profile 命中，且 Base URL 指向 open.bigmodel.cn）返回 GLM
/// 模板中的官方 models 地址。管理 API 的请求体不携带 compatibility，因此
/// 额外接受 `provider_type == AnthropicMessages` 作为等价信号——旧 GUI 中
/// Base URL 指向 open.bigmodel.cn 的模板即 GLM，其类型同为 Anthropic Messages。
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
pub fn extract_model_ids(value: &Value) -> Vec<String> {
    let mut models = Vec::new();
    if let Some(items) = value.get("data").and_then(|value| value.as_array()) {
        push_model_items(&mut models, items);
    } else if let Some(items) = value.get("models").and_then(|value| value.as_array()) {
        push_model_items(&mut models, items);
    } else if let Some(items) = value.as_array() {
        push_model_items(&mut models, items);
    }
    models
}

/// DeepSeek Responses 专用过滤：只保留 Pro 和 Flash（大小写不敏感，
/// 允许 `vendor/deepseek-v4-pro` 之类带命名空间前缀的 id）。其他协议
/// 不假定具体厂商，因此不过滤。
pub fn filter_fetched_models_for_provider(
    provider_type: &ProviderType,
    models: Vec<String>,
) -> Vec<String> {
    if provider_type != &ProviderType::DeepSeekResponses {
        return models;
    }

    models
        .into_iter()
        .filter(|model| {
            model.trim().rsplit('/').next().is_some_and(|slug| {
                slug.eq_ignore_ascii_case("deepseek-v4-pro")
                    || slug.eq_ignore_ascii_case("deepseek-v4-flash")
            })
        })
        .collect()
}

fn push_model_items(models: &mut Vec<String>, items: &[Value]) {
    for item in items {
        let id = item
            .as_str()
            .or_else(|| item.get("id").and_then(|value| value.as_str()))
            .or_else(|| item.get("slug").and_then(|value| value.as_str()))
            .map(str::trim)
            .filter(|id| !id.is_empty());
        if let Some(id) = id {
            let id = id.to_string();
            if !models.iter().any(|existing| existing == &id) {
                models.push(id);
            }
        }
    }
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
                let models = extract_model_ids(&json);
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

        // 不带 /models 结尾的显式配置会展开成两个变体（对齐旧 GUI）。
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

        // data 优先于 models（对齐旧 GUI 的 else-if 顺序）。
        let both = json!({ "data": [{ "id": "d" }], "models": [{ "id": "m" }] });
        assert_eq!(extract_model_ids(&both), vec!["d"]);

        assert!(extract_model_ids(&json!({ "object": "list" })).is_empty());
    }

    #[test]
    fn deepseek_responses_accepts_pro_and_flash_without_filtering_other_protocols() {
        let models = vec![
            "deepseek-v4-pro".to_string(),
            "DeepSeek-V4-Pro".to_string(),
            "vendor/deepseek-v4-pro".to_string(),
            "deepseek-v4-flash".to_string(),
            "ns/deepseek-v4-flash".to_string(),
            "other-model".to_string(),
        ];

        assert_eq!(
            filter_fetched_models_for_provider(&ProviderType::ChatCompletions, models.clone()),
            models.clone()
        );
        assert_eq!(
            filter_fetched_models_for_provider(&ProviderType::DeepSeekResponses, models.clone()),
            vec![
                "deepseek-v4-pro".to_string(),
                "DeepSeek-V4-Pro".to_string(),
                "vendor/deepseek-v4-pro".to_string(),
                "deepseek-v4-flash".to_string(),
                "ns/deepseek-v4-flash".to_string(),
            ]
        );
        assert_eq!(
            filter_fetched_models_for_provider(&ProviderType::OpenAiResponses, models.clone()),
            models
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
