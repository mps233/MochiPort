//! 内置 AI Gateway 服务商模板。
//!
//! 这里是「按服务商一键填充 provider 编辑器」数据的唯一来源，供管理 API
//! `GET /api/v1/manage/gateway/provider-templates` 使用。
//! 模板是纯静态数据，不包含任何用户配置或密钥；「自定义」不在此列，
//! 由客户端本地兜底。

use super::config::ProviderType;

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

/// 面向新建流程展示的内置服务商模板。
static PROVIDER_TEMPLATES: [ProviderTemplate; 5] = [
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
        id: "deepseek-responses",
        display_name: "DeepSeek Responses",
        provider_type: ProviderType::DeepSeekResponses,
        compatibility: None,
        base_url: "https://api.deepseek.com/v1",
        models_url: None,
        models: &["deepseek-v4-pro"],
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

/// 智谱 GLM 模板（Anthropic Messages 协议 + `glm_anthropic` 兼容 profile）。
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
            assert!(template.base_url.is_empty() || template.base_url.starts_with("https://"));
        }
    }

    #[test]
    fn chat_completions_type_is_not_recommended_in_builtin_templates() {
        // 自定义 Chat Completions 由客户端本地兜底，不进入内置模板推荐列表。
        assert!(
            provider_templates()
                .iter()
                .all(|candidate| candidate.provider_type != ProviderType::ChatCompletions)
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
}
