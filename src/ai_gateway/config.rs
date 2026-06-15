use serde::{Deserialize, Serialize};

/// AI Gateway 顶层配置，对应 config.toml 中 `[aiGateway]` 段。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AiGatewayConfig {
    pub enabled: bool,
    /// 默认 provider 名，当 model 无法匹配任何 provider 时使用。
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
    /// 按名称查找 provider 配置。
    pub fn get_provider(&self, name: &str) -> Option<&ProviderConfig> {
        self.providers.iter().find(|p| p.name == name)
    }

    /// 按 model 名选择 provider：精确匹配 → 前缀匹配 → default fallback。
    pub fn select_provider(&self, model: &str) -> Option<&ProviderConfig> {
        // 1. 精确匹配：models 列表包含该 model
        for provider in &self.providers {
            if provider.models.iter().any(|m| m == model) {
                return Some(provider);
            }
        }
        // 2. 前缀匹配：model 以 provider name 开头
        for provider in &self.providers {
            if model.starts_with(&provider.name) {
                return Some(provider);
            }
        }
        // 3. fallback：default_provider
        if !self.default_provider.is_empty() {
            return self.get_provider(&self.default_provider);
        }
        None
    }
}

/// 单个 provider 配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ProviderConfig {
    /// provider 名称标识（如 "openai"、"deepseek"）。
    pub name: String,
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

    fn make_config(providers: Vec<ProviderConfig>, default: &str) -> AiGatewayConfig {
        AiGatewayConfig {
            enabled: true,
            default_provider: default.into(),
            providers,
            ..Default::default()
        }
    }

    #[test]
    fn test_exact_match() {
        let config = make_config(
            vec![
                make_provider("openai", ProviderType::OpenAiResponses, vec!["gpt-4o", "gpt-4.1"]),
                make_provider("deepseek", ProviderType::ChatCompletions, vec!["deepseek-chat", "deepseek-reasoner"]),
            ],
            "openai",
        );
        let p = config.select_provider("deepseek-chat").unwrap();
        assert_eq!(p.name, "deepseek");

        let p = config.select_provider("gpt-4o").unwrap();
        assert_eq!(p.name, "openai");
    }

    #[test]
    fn test_prefix_match() {
        let config = make_config(
            vec![
                make_provider("openai", ProviderType::OpenAiResponses, vec![]),
                make_provider("deepseek", ProviderType::ChatCompletions, vec![]),
            ],
            "openai",
        );
        // "deepseek-v3" 前缀匹配 "deepseek"
        let p = config.select_provider("deepseek-v3").unwrap();
        assert_eq!(p.name, "deepseek");
    }

    #[test]
    fn test_default_fallback() {
        let config = make_config(
            vec![
                make_provider("openai", ProviderType::OpenAiResponses, vec!["gpt-4o"]),
                make_provider("deepseek", ProviderType::ChatCompletions, vec!["deepseek-chat"]),
            ],
            "openai",
        );
        // "claude-xxx" 既不精确也不前缀匹配，fallback 到 default
        let p = config.select_provider("claude-sonnet-4").unwrap();
        assert_eq!(p.name, "openai");
    }

    #[test]
    fn test_no_match_no_default() {
        let config = make_config(
            vec![make_provider("openai", ProviderType::OpenAiResponses, vec!["gpt-4o"])],
            "",
        );
        assert!(config.select_provider("unknown-model").is_none());
    }

    #[test]
    fn test_exact_takes_priority_over_prefix() {
        let config = make_config(
            vec![
                make_provider("deepseek", ProviderType::ChatCompletions, vec![]),
                make_provider("other", ProviderType::OpenAiResponses, vec!["deepseek-chat"]),
            ],
            "",
        );
        // "deepseek-chat" 精确匹配 "other" 的 models 列表
        let p = config.select_provider("deepseek-chat").unwrap();
        assert_eq!(p.name, "other");
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
        assert_eq!(config.providers[0].provider_type, ProviderType::OpenAiResponses);
        assert_eq!(config.providers[1].provider_type, ProviderType::ChatCompletions);
        assert_eq!(config.providers[0].timeout_secs, 120);
        assert_eq!(config.providers[1].timeout_secs, 300); // default
    }
}
