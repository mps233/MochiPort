use std::collections::HashSet;

use once_cell::sync::Lazy;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::config::AiGatewayConfig;

static BASE_MODEL_CATALOG: Lazy<Value> = Lazy::new(|| {
    serde_json::from_str(include_str!("models.json")).expect("embedded AI Gateway model catalog")
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogModelOption {
    pub slug: String,
    pub display_name: String,
    pub description: String,
}

pub fn visible_catalog_model_options() -> Vec<CatalogModelOption> {
    catalog_models()
        .iter()
        .filter(|model| is_catalog_model_visible(model))
        .filter_map(|model| {
            let slug = model_slug(model)?.to_string();
            let display_name = model
                .get("display_name")
                .and_then(Value::as_str)
                .unwrap_or(&slug)
                .to_string();
            let description = model
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            Some(CatalogModelOption {
                slug,
                display_name,
                description,
            })
        })
        .collect()
}

#[cfg(test)]
pub fn configured_models_response(config: &AiGatewayConfig) -> Value {
    build_configured_models_response(config)
}

pub fn configured_models_etag(config: &AiGatewayConfig) -> String {
    let response = build_configured_models_response(config);
    configured_models_etag_from_response(&response)
}

pub fn configured_models_response_with_etag(config: &AiGatewayConfig) -> (Value, String) {
    let response = build_configured_models_response(config);
    let etag = configured_models_etag_from_response(&response);
    (response, etag)
}

fn build_configured_models_response(config: &AiGatewayConfig) -> Value {
    let catalog_models = catalog_models();

    let mut emitted = HashSet::new();
    let mut models = Vec::new();
    let mut priority = 0;

    for model_id in selected_codex_model_ids(config) {
        if !emitted.insert(model_id.clone()) {
            continue;
        }

        let model = catalog_models
            .iter()
            .find(|model| {
                model_slug(model) == Some(model_id.as_str()) && is_catalog_model_visible(model)
            })
            .cloned();
        let Some(mut model) = model else {
            continue;
        };
        normalize_deepseek_model(&mut model);
        if let Some(object) = model.as_object_mut() {
            object.insert("priority".to_string(), json!(priority));
        }
        priority += 1;
        models.push(model);
    }

    json!({ "models": models })
}

fn catalog_models() -> &'static Vec<Value> {
    BASE_MODEL_CATALOG
        .get("models")
        .and_then(Value::as_array)
        .expect("embedded AI Gateway model catalog must contain models array")
}

fn configured_models_etag_from_response(response: &Value) -> String {
    let serialized = serde_json::to_vec(response)
        .expect("configured models response should always serialize for etag");
    let digest = Sha256::digest(serialized);
    format!("\"sha256:{}\"", hex::encode(digest))
}

fn selected_codex_model_ids(config: &AiGatewayConfig) -> Vec<String> {
    config
        .codex_visible_models
        .iter()
        .map(|model| model.trim())
        .filter(|model| !model.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn model_slug(model: &Value) -> Option<&str> {
    model.get("slug").and_then(Value::as_str)
}

fn is_catalog_model_visible(model: &Value) -> bool {
    model
        .get("supported_in_api")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        && model.get("visibility").and_then(Value::as_str) == Some("list")
}

fn normalize_deepseek_model(model: &mut Value) {
    let Some(slug) = model_slug(model) else {
        return;
    };
    if !slug.starts_with("deepseek-") {
        return;
    }

    if let Some(object) = model.as_object_mut() {
        object.insert("apply_patch_tool_type".to_string(), Value::Null);
        object.insert("web_search_tool_type".to_string(), json!("text"));
        object.insert("supports_search_tool".to_string(), Value::Bool(false));
        object.insert(
            "supports_image_detail_original".to_string(),
            Value::Bool(false),
        );
        object.insert("input_modalities".to_string(), json!(["text"]));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config(models: &[&str]) -> AiGatewayConfig {
        AiGatewayConfig {
            codex_visible_models: models.iter().map(|model| model.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn configured_models_response_uses_codex_visible_models() {
        let config = config(&[
            "gpt-5.5",
            "deepseek-v4-pro",
            "deepseek-v4-flash",
            "custom-model",
            "codex-auto-review",
        ]);

        let response = configured_models_response(&config);
        let slugs: Vec<&str> = response["models"]
            .as_array()
            .unwrap()
            .iter()
            .map(|model| model["slug"].as_str().unwrap())
            .collect();

        assert_eq!(
            slugs,
            vec!["gpt-5.5", "deepseek-v4-pro", "deepseek-v4-flash"]
        );
        assert_eq!(response["models"][1]["display_name"], "deepseek-v4-pro");
        assert_eq!(response["models"][1]["apply_patch_tool_type"], Value::Null);
        assert_eq!(response["models"][1]["supports_search_tool"], false);
        assert_eq!(
            response["models"][1]["supports_image_detail_original"],
            false
        );
        assert_eq!(response["models"][1]["input_modalities"], json!(["text"]));
    }

    #[test]
    fn configured_models_etag_is_stable_for_same_response() {
        let config = config(&["deepseek-v4-pro", "deepseek-v4-flash"]);

        let (response, etag) = configured_models_response_with_etag(&config);

        assert_eq!(response, configured_models_response(&config));
        assert_eq!(etag, configured_models_etag(&config));
        assert!(etag.starts_with("\"sha256:"));
        assert!(etag.ends_with('"'));
    }

    #[test]
    fn configured_models_etag_changes_when_visible_models_change() {
        let base_config = config(&["deepseek-v4-pro"]);
        let changed_config = config(&["deepseek-v4-pro", "deepseek-v4-flash"]);

        assert_ne!(
            configured_models_etag(&base_config),
            configured_models_etag(&changed_config)
        );
    }

    #[test]
    fn configured_models_response_skips_unknown_configured_model() {
        let config = config(&["custom-model"]);

        let response = configured_models_response(&config);
        assert!(response["models"].as_array().unwrap().is_empty());
    }

    #[test]
    fn configured_models_response_skips_hidden_catalog_model() {
        let config = config(&["codex-auto-review"]);

        let response = configured_models_response(&config);
        assert!(response["models"].as_array().unwrap().is_empty());
    }

    #[test]
    fn deepseek_models_disable_apply_patch_tool() {
        let response = configured_models_response(&config(&["deepseek-v4-pro"]));
        let model = &response["models"][0];
        assert_eq!(model["apply_patch_tool_type"], Value::Null);
        assert_eq!(model["supports_image_detail_original"], false);
        assert_eq!(model["input_modalities"], json!(["text"]));
        assert_eq!(model["web_search_tool_type"], "text");
        assert_eq!(model["supports_search_tool"], false);
    }

    #[test]
    fn configured_models_response_returns_empty_when_no_models_configured() {
        let config = config(&[]);

        let response = configured_models_response(&config);
        assert!(response["models"].as_array().unwrap().is_empty());
    }

    #[test]
    fn catalog_model_visibility_requires_api_support_and_list_visibility() {
        assert!(is_catalog_model_visible(&json!({
            "supported_in_api": true,
            "visibility": "list"
        })));
        assert!(!is_catalog_model_visible(&json!({
            "supported_in_api": false,
            "visibility": "list"
        })));
        assert!(!is_catalog_model_visible(&json!({
            "supported_in_api": true,
            "visibility": "hide"
        })));
        assert!(!is_catalog_model_visible(&json!({
            "visibility": "list"
        })));
    }

    #[test]
    fn visible_catalog_model_options_returns_listable_api_models() {
        let options = visible_catalog_model_options();
        assert!(options.iter().any(|model| model.slug == "gpt-5.5"));
        assert!(
            options
                .iter()
                .all(|model| !model.slug.trim().is_empty() && !model.display_name.trim().is_empty())
        );
    }
}
