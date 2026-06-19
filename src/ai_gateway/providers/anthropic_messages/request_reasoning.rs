use serde_json::{Value, json};

use crate::ai_gateway::model::Reasoning;

pub(super) fn anthropic_thinking(reasoning: &Reasoning) -> Option<Value> {
    let budget_tokens = reasoning
        .budget_tokens
        .or_else(|| effort_budget_tokens(reasoning.effort.as_deref()))?;

    if budget_tokens <= 0 {
        return None;
    }

    Some(json!({
        "type": "enabled",
        "budget_tokens": budget_tokens,
    }))
}

fn effort_budget_tokens(effort: Option<&str>) -> Option<i64> {
    match effort {
        Some("none") | Some("minimal") => None,
        Some("low") => Some(1_024),
        Some("medium") | None => Some(4_096),
        Some("high") => Some(8_192),
        Some("xhigh") | Some("max") => Some(16_384),
        _ => Some(4_096),
    }
}
