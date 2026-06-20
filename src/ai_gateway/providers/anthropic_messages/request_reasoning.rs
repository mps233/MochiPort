use serde_json::{Map, Value, json};

use crate::ai_gateway::model::Reasoning;

use super::options::AnthropicProviderProfile;

pub(super) fn insert_reasoning_options(
    body: &mut Map<String, Value>,
    profile: AnthropicProviderProfile,
    reasoning: Option<&Reasoning>,
) {
    let Some(reasoning) = reasoning else {
        return;
    };

    match profile {
        AnthropicProviderProfile::Anthropic => insert_anthropic_reasoning(body, reasoning),
        AnthropicProviderProfile::GlmAnthropic => insert_glm_reasoning(body, reasoning),
    }
}

fn insert_anthropic_reasoning(body: &mut Map<String, Value>, reasoning: &Reasoning) {
    if let Some(budget_tokens) = reasoning.budget_tokens {
        if budget_tokens > 0 {
            body.insert(
                "thinking".to_string(),
                json!({
                    "type": "enabled",
                    "budget_tokens": budget_tokens,
                }),
            );
        }
        return;
    }

    let Some(effort) = normalized_effort(reasoning.effort.as_deref()) else {
        return;
    };

    body.insert("thinking".to_string(), json!({ "type": "adaptive" }));
    body.insert(
        "output_config".to_string(),
        json!({
            "effort": effort,
        }),
    );
}

fn insert_glm_reasoning(body: &mut Map<String, Value>, reasoning: &Reasoning) {
    let Some(effort) = normalized_glm_effort(reasoning.effort.as_deref()) else {
        body.insert("thinking".to_string(), json!({ "type": "disabled" }));
        return;
    };

    body.insert("thinking".to_string(), json!({ "type": "enabled" }));
    body.insert("reasoning_effort".to_string(), json!(effort));
}

fn normalized_effort(effort: Option<&str>) -> Option<&str> {
    match effort.map(str::trim).filter(|value| !value.is_empty()) {
        Some("none") | Some("minimal") => None,
        Some("low") => Some("low"),
        Some("medium") | None => Some("medium"),
        Some("high") => Some("high"),
        Some("xhigh") => Some("xhigh"),
        Some("max") => Some("max"),
        Some(other) => Some(other),
    }
}

fn normalized_glm_effort(effort: Option<&str>) -> Option<&str> {
    match normalized_effort(effort) {
        Some("low") | Some("medium") | Some("high") => Some("high"),
        Some("xhigh") | Some("max") => Some("max"),
        other => other,
    }
}
