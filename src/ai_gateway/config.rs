use serde::{Deserialize, Serialize};

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
}

impl Default for AiGatewayConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            default_provider: String::new(),
            prompt_cache_retention: None,
            providers: Vec::new(),
        }
    }
}

impl AiGatewayConfig {
    /// 按 model 名选择已启用的 provider：只匹配显式配置的 model。
    pub fn select_provider(&self, model: &str) -> Option<&ProviderConfig> {
        for provider in self.providers.iter().filter(|provider| provider.enabled) {
            if provider.models.iter().any(|m| m == model) {
                return Some(provider);
            }
        }
        None
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
                vec!["deepseek-chat", "deepseek-reasoner"],
            ),
        ]);
        let p = config.select_provider("deepseek-chat").unwrap();
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
        assert!(config.select_provider("deepseek-chat").is_none());
    }

    #[test]
    fn test_no_fallback_even_with_legacy_default_provider() {
        let config = make_config(vec![
            make_provider("openai", ProviderType::OpenAiResponses, vec!["gpt-4o"]),
            make_provider(
                "deepseek",
                ProviderType::ChatCompletions,
                vec!["deepseek-chat"],
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
                vec!["deepseek-chat"],
            ),
        ]);
        let p = config.select_provider("deepseek-chat").unwrap();
        assert_eq!(p.name, "other");
    }

    #[test]
    fn test_disabled_provider_is_not_selected() {
        let mut provider = make_provider(
            "deepseek",
            ProviderType::ChatCompletions,
            vec!["deepseek-chat"],
        );
        provider.enabled = false;
        let config = make_config(vec![provider]);

        assert!(config.select_provider("deepseek-chat").is_none());
        assert!(config.select_provider("deepseek-v3").is_none());
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
            models = ["deepseek-chat"]
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
}
