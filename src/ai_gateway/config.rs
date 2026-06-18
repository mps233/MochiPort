use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// AI Gateway 顶层配置，对应 config.toml 中 `[aiGateway]` 段。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AiGatewayConfig {
    pub enabled: bool,
    /// 兼容旧配置的遗留字段；路由不使用默认/兜底 provider。
    #[serde(skip_serializing)]
    pub default_provider: String,
    /// 全局 prompt_cache_retention 值（如 "1h"），可选。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_cache_retention: Option<String>,
    /// provider 列表。
    pub providers: Vec<ProviderConfig>,
    /// Codex App 前端模型选择器可见的模型列表。
    ///
    /// 该列表只控制 `/models` 暴露给 Codex App 的 catalog 模型，不参与上游 provider 路由。
    pub codex_visible_models: Vec<String>,
}

impl Default for AiGatewayConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            default_provider: String::new(),
            prompt_cache_retention: None,
            providers: Vec::new(),
            codex_visible_models: Vec::new(),
        }
    }
}

impl AiGatewayConfig {
    /// 按 model 名选择已启用的 provider：只匹配显式配置的 model。
    pub fn select_provider(&self, model: &str) -> Option<&ProviderConfig> {
        self.select_provider_for_session(model, None)
    }

    /// 按 model 名和 session_id 选择已启用的 provider。
    ///
    /// 同一个 model 可以配置到多个 provider；有 session_id 时使用 Rendezvous/HRW
    /// Hash 在候选 provider 中稳定选择一个。没有 session_id 时保持旧行为：
    /// 返回配置顺序中的第一个匹配 provider。
    pub fn select_provider_for_session(
        &self,
        model: &str,
        session_id: Option<&str>,
    ) -> Option<&ProviderConfig> {
        let candidates = self
            .providers
            .iter()
            .filter(|provider| provider.enabled && provider.models.iter().any(|m| m == model));
        let Some(session_id) = session_id.map(str::trim).filter(|value| !value.is_empty()) else {
            return candidates.into_iter().next();
        };

        candidates
            .into_iter()
            .max_by(|left, right| compare_hrw_provider(session_id, left, right))
    }
}

fn compare_hrw_provider(
    session_id: &str,
    left: &ProviderConfig,
    right: &ProviderConfig,
) -> std::cmp::Ordering {
    let left_score = hrw_score(session_id, left);
    let right_score = hrw_score(session_id, right);
    left_score.cmp(&right_score).then_with(|| {
        provider_route_id(left)
            .as_str()
            .cmp(provider_route_id(right).as_str())
    })
}

fn hrw_score(session_id: &str, provider: &ProviderConfig) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"codex-remote-ai-gateway-hrw-v1\0");
    hasher.update(session_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(provider_route_id(provider).as_bytes());
    let digest = hasher.finalize();
    let mut score = [0_u8; 32];
    score.copy_from_slice(&digest);
    score
}

fn provider_route_id(provider: &ProviderConfig) -> String {
    let name = provider.name.trim();
    let base_url = provider.base_url.trim();
    format!(
        "{}\0{}\0{}",
        name,
        provider.provider_type.route_key(),
        base_url
    )
}

impl ProviderType {
    fn route_key(&self) -> &'static str {
        match self {
            Self::OpenAiResponses => "openai_responses",
            Self::ChatCompletions => "chat_completions",
        }
    }
}

/// 将 provider Base URL 规范成 API 根地址。
///
/// UI 允许用户填 `https://api.example.com` 或 `https://api.example.com/v1`；
/// 出站请求统一在这里去掉末尾 `/v1`，再拼具体路径。
pub fn provider_api_root(base_url: &str) -> String {
    let base = base_url.trim().trim_end_matches('/');
    if base.to_ascii_lowercase().ends_with("/v1") {
        base[..base.len() - 3].trim_end_matches('/').to_string()
    } else {
        base.to_string()
    }
}

/// 将 provider Base URL 规范成 UI/配置展示用地址。
///
/// 目前支持的上游协议都使用 `/v1`，所以保存时统一展示为带 `/v1`。
pub fn provider_display_base_url(base_url: &str) -> String {
    let root = provider_api_root(base_url);
    if root.is_empty() {
        String::new()
    } else {
        format!("{root}/v1")
    }
}

/// 单个 provider 配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ProviderConfig {
    /// provider 名称标识（如 "openai"、"deepseek"）。
    pub name: String,
    /// 是否启用该 provider。
    pub enabled: bool,
    /// provider 类型：`"openai_responses"` 或 `"chat_completions"`。
    pub provider_type: ProviderType,
    /// 上游 API base URL。
    pub base_url: String,
    /// API key。
    pub api_key: String,
    /// 该 provider 支持的 model 列表（精确匹配用）。
    pub models: Vec<String>,
    /// 可选的 prompt_cache_retention 覆盖。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_cache_retention: Option<String>,
    /// 上游请求超时（秒）。
    pub timeout_secs: u64,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            enabled: true,
            provider_type: ProviderType::OpenAiResponses,
            base_url: String::new(),
            api_key: String::new(),
            models: Vec::new(),
            prompt_cache_retention: None,
            timeout_secs: 300,
        }
    }
}

/// Provider 类型枚举。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderType {
    /// OpenAI Responses API 透传。
    OpenAiResponses,
    /// Chat Completions API（DeepSeek 等）。
    ChatCompletions,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_provider(name: &str, ptype: ProviderType, models: Vec<&str>) -> ProviderConfig {
        ProviderConfig {
            name: name.into(),
            provider_type: ptype,
            models: models.into_iter().map(|s| s.into()).collect(),
            ..Default::default()
        }
    }

    fn make_config(providers: Vec<ProviderConfig>) -> AiGatewayConfig {
        AiGatewayConfig {
            enabled: true,
            providers,
            ..Default::default()
        }
    }

    #[test]
    fn test_exact_match() {
        let config = make_config(vec![
            make_provider(
                "openai",
                ProviderType::OpenAiResponses,
                vec!["gpt-4o", "gpt-4.1"],
            ),
            make_provider(
                "deepseek",
                ProviderType::ChatCompletions,
                vec!["deepseek-v4-flash", "deepseek-v4-pro"],
            ),
        ]);
        let p = config.select_provider("deepseek-v4-flash").unwrap();
        assert_eq!(p.name, "deepseek");

        let p = config.select_provider("gpt-4o").unwrap();
        assert_eq!(p.name, "openai");
    }

    #[test]
    fn test_empty_model_list_does_not_match_provider_name_prefix() {
        let config = make_config(vec![
            make_provider("openai", ProviderType::OpenAiResponses, vec![]),
            make_provider("deepseek", ProviderType::ChatCompletions, vec![]),
        ]);
        assert!(config.select_provider("deepseek-v3").is_none());
        assert!(config.select_provider("deepseek-v4-flash").is_none());
    }

    #[test]
    fn test_no_fallback_even_with_legacy_default_provider() {
        let config = make_config(vec![
            make_provider("openai", ProviderType::OpenAiResponses, vec!["gpt-4o"]),
            make_provider(
                "deepseek",
                ProviderType::ChatCompletions,
                vec!["deepseek-v4-flash"],
            ),
        ]);
        let config = AiGatewayConfig {
            default_provider: "openai".into(),
            ..config
        };
        assert!(config.select_provider("claude-sonnet-4").is_none());
    }

    #[test]
    fn test_no_match_no_default() {
        let config = make_config(vec![make_provider(
            "openai",
            ProviderType::OpenAiResponses,
            vec!["gpt-4o"],
        )]);
        assert!(config.select_provider("unknown-model").is_none());
    }

    #[test]
    fn test_exact_match_uses_configured_model_not_provider_name() {
        let config = make_config(vec![
            make_provider("deepseek", ProviderType::ChatCompletions, vec![]),
            make_provider(
                "other",
                ProviderType::OpenAiResponses,
                vec!["deepseek-v4-flash"],
            ),
        ]);
        let p = config.select_provider("deepseek-v4-flash").unwrap();
        assert_eq!(p.name, "other");
    }

    #[test]
    fn test_disabled_provider_is_not_selected() {
        let mut provider = make_provider(
            "deepseek",
            ProviderType::ChatCompletions,
            vec!["deepseek-v4-flash"],
        );
        provider.enabled = false;
        let config = make_config(vec![provider]);

        assert!(config.select_provider("deepseek-v4-flash").is_none());
        assert!(config.select_provider("deepseek-v3").is_none());
    }

    #[test]
    fn test_session_provider_selection_is_stable() {
        let config = make_config(vec![
            make_provider("openai-a", ProviderType::OpenAiResponses, vec!["gpt-5.5"]),
            make_provider("openai-b", ProviderType::OpenAiResponses, vec!["gpt-5.5"]),
            make_provider("openai-c", ProviderType::OpenAiResponses, vec!["gpt-5.5"]),
        ]);

        let first = config
            .select_provider_for_session("gpt-5.5", Some("session-abc"))
            .unwrap()
            .name
            .clone();
        let second = config
            .select_provider_for_session("gpt-5.5", Some("session-abc"))
            .unwrap()
            .name
            .clone();

        assert_eq!(first, second);
    }

    #[test]
    fn test_session_provider_selection_is_independent_of_config_order() {
        let providers = vec![
            make_provider("openai-a", ProviderType::OpenAiResponses, vec!["gpt-5.5"]),
            make_provider("openai-b", ProviderType::OpenAiResponses, vec!["gpt-5.5"]),
            make_provider("openai-c", ProviderType::OpenAiResponses, vec!["gpt-5.5"]),
            make_provider("openai-d", ProviderType::OpenAiResponses, vec!["gpt-5.5"]),
        ];
        let config = make_config(providers.clone());
        let reversed = make_config(providers.into_iter().rev().collect());

        let selected = config
            .select_provider_for_session("gpt-5.5", Some("session-abc"))
            .unwrap()
            .name
            .clone();
        let selected_reversed = reversed
            .select_provider_for_session("gpt-5.5", Some("session-abc"))
            .unwrap()
            .name
            .clone();

        assert_eq!(selected, selected_reversed);
    }

    #[test]
    fn test_session_provider_selection_ignores_unmatched_models() {
        let config = make_config(vec![
            make_provider("openai-a", ProviderType::OpenAiResponses, vec!["gpt-5.5"]),
            make_provider("openai-b", ProviderType::OpenAiResponses, vec!["gpt-5.5"]),
            make_provider("other", ProviderType::OpenAiResponses, vec!["gpt-4o"]),
        ]);

        let selected = config
            .select_provider_for_session("gpt-5.5", Some("session-abc"))
            .unwrap();

        assert_ne!(selected.name, "other");
    }

    #[test]
    fn test_session_provider_selection_stays_when_non_selected_provider_removed() {
        let providers = vec![
            make_provider("openai-a", ProviderType::OpenAiResponses, vec!["gpt-5.5"]),
            make_provider("openai-b", ProviderType::OpenAiResponses, vec!["gpt-5.5"]),
            make_provider("openai-c", ProviderType::OpenAiResponses, vec!["gpt-5.5"]),
            make_provider("openai-d", ProviderType::OpenAiResponses, vec!["gpt-5.5"]),
        ];
        let config = make_config(providers.clone());
        let selected = config
            .select_provider_for_session("gpt-5.5", Some("session-abc"))
            .unwrap()
            .name
            .clone();
        let removed_name = providers
            .iter()
            .find(|provider| provider.name != selected)
            .unwrap()
            .name
            .clone();
        let pruned = make_config(
            providers
                .into_iter()
                .filter(|provider| provider.name != removed_name)
                .collect(),
        );

        let selected_after_removal = pruned
            .select_provider_for_session("gpt-5.5", Some("session-abc"))
            .unwrap()
            .name
            .clone();

        assert_eq!(selected, selected_after_removal);
    }

    #[test]
    fn test_missing_session_id_keeps_first_match_behavior() {
        let config = make_config(vec![
            make_provider("openai-a", ProviderType::OpenAiResponses, vec!["gpt-5.5"]),
            make_provider("openai-b", ProviderType::OpenAiResponses, vec!["gpt-5.5"]),
        ]);

        assert_eq!(
            config
                .select_provider_for_session("gpt-5.5", None)
                .unwrap()
                .name,
            "openai-a"
        );
        assert_eq!(
            config
                .select_provider_for_session("gpt-5.5", Some("   "))
                .unwrap()
                .name,
            "openai-a"
        );
    }

    #[test]
    fn test_toml_deserialization() {
        let toml_str = r#"
            enabled = true
            defaultProvider = "openai"
            [[providers]]
            name = "openai"
            providerType = "open_ai_responses"
            baseUrl = "https://api.openai.com"
            apiKey = "sk-xxx"
            models = ["gpt-4o"]
            timeoutSecs = 120

            [[providers]]
            name = "deepseek"
            providerType = "chat_completions"
            baseUrl = "https://api.deepseek.com"
            apiKey = "sk-yyy"
            models = ["deepseek-v4-flash"]
        "#;
        let config: AiGatewayConfig = toml::from_str(toml_str).unwrap();
        assert!(config.enabled);
        assert_eq!(config.providers.len(), 2);
        assert_eq!(
            config.providers[0].provider_type,
            ProviderType::OpenAiResponses
        );
        assert_eq!(
            config.providers[1].provider_type,
            ProviderType::ChatCompletions
        );
        assert_eq!(config.providers[0].timeout_secs, 120);
        assert_eq!(config.providers[1].timeout_secs, 300); // default
    }

    #[test]
    fn test_provider_api_root_accepts_versioned_or_root_base_url() {
        assert_eq!(
            provider_api_root("https://api.deepseek.com"),
            "https://api.deepseek.com"
        );
        assert_eq!(
            provider_api_root("https://api.deepseek.com/v1"),
            "https://api.deepseek.com"
        );
        assert_eq!(
            provider_api_root("https://proxy.example.com/openai/v1/"),
            "https://proxy.example.com/openai"
        );
    }

    #[test]
    fn test_provider_display_base_url_uses_v1() {
        assert_eq!(
            provider_display_base_url("https://api.deepseek.com"),
            "https://api.deepseek.com/v1"
        );
        assert_eq!(
            provider_display_base_url("https://api.deepseek.com/v1"),
            "https://api.deepseek.com/v1"
        );
        assert_eq!(provider_display_base_url(""), "");
    }
}
