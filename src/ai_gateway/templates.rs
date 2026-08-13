//! 内置 AI Gateway 服务商模板。
//!
//! 这里是「按服务商一键填充 provider 编辑器」数据的唯一来源：
//! 旧 wxDragon GUI（`gui` feature）与管理 API
//! `GET /api/v1/manage/gateway/provider-templates` 共用同一份数据。
//! 模板是纯静态数据，不包含任何用户配置或密钥；「自定义」不在此列，
//! 由客户端本地兜底。

use super::config::{ProviderConfig, ProviderType};

/// 单个内置服务商模板。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderTemplate {
    /// 模板标识，同时是建议的默认 provider 名称（如 "openai"）。
    pub id: &'static str,
    /// 人类可读名称（英文基准，客户端可按需本地化）。
    pub display_name: &'static str,
    /// 上游协议类型。
    pub provider_type: ProviderType,
    /// Anthropic Messages 兼容 profile（如 "glm_anthropic"）。
    pub compatibility: Option<&'static str>,
    /// 默认上游 Base URL。
    pub base_url: &'static str,
    /// 默认模型列表接口。仅当无法按 `{base_url}/models` 推导时提供。
    pub models_url: Option<&'static str>,
    /// 默认模型列表。
    pub models: &'static [&'static str],
}

impl ProviderTemplate {
    /// 生成可直接进入编辑器/保存流程的 [`ProviderConfig`] 草稿：
    /// 无 API key，weight/timeout 等沿用默认值。
    #[cfg_attr(not(feature = "gui"), allow(dead_code))]
    pub fn to_provider_config(&self) -> ProviderConfig {
        ProviderConfig {
            name: self.id.to_string(),
            provider_type: self.provider_type.clone(),
            compatibility: self.compatibility.map(str::to_string),
            base_url: self.base_url.to_string(),
            models_url: self.models_url.map(str::to_string),
            models: self.models.iter().map(|model| model.to_string()).collect(),
            ..ProviderConfig::default()
        }
    }
}

/// 内置服务商模板，顺序与旧 GUI 服务商单选按钮一致。
static PROVIDER_TEMPLATES: [ProviderTemplate; 6] = [
    ProviderTemplate {
        id: "openai",
        display_name: "OpenAI",
        provider_type: ProviderType::OpenAiResponses,
        compatibility: None,
        base_url: "https://api.openai.com/v1",
        models_url: None,
        models: &[],
    },
    ProviderTemplate {
        id: "grok",
        display_name: "Grok",
        provider_type: ProviderType::GrokResponses,
        compatibility: None,
        base_url: "https://api.x.ai/v1",
        models_url: None,
        models: &[],
    },
    ProviderTemplate {
        id: "deepseek",
        display_name: "DeepSeek Chat",
        provider_type: ProviderType::ChatCompletions,
        compatibility: None,
        base_url: "https://api.deepseek.com/v1",
        models_url: None,
        models: &[],
    },
    ProviderTemplate {
        id: "deepseek-responses",
        display_name: "DeepSeek Responses",
        provider_type: ProviderType::DeepSeekResponses,
        compatibility: None,
        base_url: "https://api.deepseek.com/v1",
        models_url: None,
        models: &["deepseek-v4-flash"],
    },
    ProviderTemplate {
        id: "anthropic",
        display_name: "Anthropic",
        provider_type: ProviderType::AnthropicMessages,
        compatibility: Some("anthropic"),
        base_url: "https://api.anthropic.com/v1",
        models_url: None,
        models: &[],
    },
    ProviderTemplate {
        id: "glm",
        display_name: "GLM",
        provider_type: ProviderType::AnthropicMessages,
        compatibility: Some("glm_anthropic"),
        base_url: "https://open.bigmodel.cn/api/anthropic",
        models_url: Some("https://open.bigmodel.cn/api/paas/v4/models"),
        models: &[],
    },
];

/// 全部内置服务商模板。
pub fn provider_templates() -> &'static [ProviderTemplate] {
    &PROVIDER_TEMPLATES
}

/// 按协议类型取官方默认模板；`AnthropicMessages` 返回 Anthropic 官方模板，
/// GLM 等兼容厂商使用 [`glm_template`] 之类的专属模板。
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub fn default_template_for(provider_type: &ProviderType) -> &'static ProviderTemplate {
    let id = match provider_type {
        ProviderType::OpenAiResponses => "openai",
        ProviderType::GrokResponses => "grok",
        ProviderType::ChatCompletions => "deepseek",
        ProviderType::DeepSeekResponses => "deepseek-responses",
        ProviderType::AnthropicMessages => "anthropic",
    };
    template_by_id(id)
}

/// 智谱 GLM 模板（Anthropic Messages 协议 + `glm_anthropic` 兼容 profile）。
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub fn glm_template() -> &'static ProviderTemplate {
    template_by_id("glm")
}

fn template_by_id(id: &str) -> &'static ProviderTemplate {
    PROVIDER_TEMPLATES
        .iter()
        .find(|template| template.id == id)
        .expect("built-in provider template ids are static")
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn template_ids_are_unique_and_fields_are_non_empty() {
        let mut seen = HashSet::new();
        for template in provider_templates() {
            assert!(
                seen.insert(template.id),
                "duplicate template {}",
                template.id
            );
            assert!(!template.id.trim().is_empty());
            assert!(!template.display_name.trim().is_empty());
            assert!(template.base_url.starts_with("https://"));
        }
    }

    #[test]
    fn every_provider_type_maps_to_its_legacy_gui_default_template() {
        assert_eq!(
            default_template_for(&ProviderType::OpenAiResponses).id,
            "openai"
        );
        assert_eq!(
            default_template_for(&ProviderType::GrokResponses).id,
            "grok"
        );
        assert_eq!(
            default_template_for(&ProviderType::ChatCompletions).id,
            "deepseek"
        );
        assert_eq!(
            default_template_for(&ProviderType::DeepSeekResponses).id,
            "deepseek-responses"
        );
        assert_eq!(
            default_template_for(&ProviderType::AnthropicMessages).id,
            "anthropic"
        );
        assert_eq!(
            default_template_for(&ProviderType::AnthropicMessages).compatibility,
            Some("anthropic")
        );
    }

    #[test]
    fn glm_template_keeps_legacy_gui_defaults() {
        let template = glm_template();
        assert_eq!(template.provider_type, ProviderType::AnthropicMessages);
        assert_eq!(template.compatibility, Some("glm_anthropic"));
        assert_eq!(template.base_url, "https://open.bigmodel.cn/api/anthropic");
        assert_eq!(
            template.models_url,
            Some("https://open.bigmodel.cn/api/paas/v4/models")
        );
    }

    #[test]
    fn to_provider_config_produces_secretless_draft_with_defaults() {
        let provider = default_template_for(&ProviderType::DeepSeekResponses).to_provider_config();
        assert_eq!(provider.name, "deepseek-responses");
        assert_eq!(provider.provider_type, ProviderType::DeepSeekResponses);
        assert_eq!(provider.base_url, "https://api.deepseek.com/v1");
        assert_eq!(provider.models, vec!["deepseek-v4-flash".to_string()]);
        assert!(provider.api_key.is_empty());
        assert!(provider.model_aliases.is_empty());

        let defaults = ProviderConfig::default();
        assert_eq!(provider.enabled, defaults.enabled);
        assert_eq!(provider.weight, defaults.weight);
        assert_eq!(provider.timeout_secs, defaults.timeout_secs);
    }
}
