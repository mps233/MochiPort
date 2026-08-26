use serde_json::{Value, json};

use crate::ai_gateway::custom_tool_input::{
    completed_custom_tool_input, partial_custom_tool_input,
};
use crate::ai_gateway::tool_names::{ToolCallKind, ToolCallTarget};

use super::stream_tools::AnthropicContentBlockState;
pub(super) struct ToolDeltaEvent {
    pub(super) event_type: &'static str,
    pub(super) item_id: String,
    pub(super) output_index: usize,
    pub(super) delta: String,
}

pub(super) fn tool_delta_event(
    state: &mut AnthropicContentBlockState,
    raw_delta: &str,
) -> Option<ToolDeltaEvent> {
    match state.target.kind {
        ToolCallKind::Custom => {
            let full_input = match partial_custom_tool_input(&state.arguments) {
                Some(input) => input,
                None if !state.arguments.trim_start().starts_with('{') => state.arguments.clone(),
                None => return None,
            };
            let delta = full_input
                .strip_prefix(&state.custom_emitted_input)
                .unwrap_or(&full_input)
                .to_string();
            if delta.is_empty() {
                return None;
            }
            state.custom_emitted_input = full_input;
            Some(ToolDeltaEvent {
                event_type: "response.custom_tool_call_input.delta",
                item_id: state.item_id.clone(),
                output_index: state.output_index,
                delta,
            })
        }
        ToolCallKind::Function => Some(ToolDeltaEvent {
            event_type: "response.function_call_arguments.delta",
            item_id: state.item_id.clone(),
            output_index: state.output_index,
            delta: raw_delta.to_string(),
        }),
        ToolCallKind::ToolSearch => None,
    }
}

pub(super) fn in_progress_tool_item(
    item_id: &str,
    call_id: &str,
    target: &ToolCallTarget,
) -> Value {
    match target.kind {
        ToolCallKind::ToolSearch => json!({
            "type": "tool_search_call",
            "id": item_id,
            "call_id": call_id,
            "execution": "client",
            "arguments": {},
            "status": "in_progress",
        }),
        ToolCallKind::Custom => json!({
            "type": "custom_tool_call",
            "id": item_id,
            "call_id": call_id,
            "name": target.name,
            "input": "",
        }),
        ToolCallKind::Function => {
            let mut item = json!({
                "type": "function_call",
                "id": item_id,
                "call_id": call_id,
                "name": target.name,
                "arguments": "",
                "status": "in_progress",
            });
            if let Some(namespace) = &target.namespace {
                item["namespace"] = json!(namespace);
            }
            item
        }
    }
}

pub(super) fn completed_tool_item(
    item_id: &str,
    call_id: &str,
    target: &ToolCallTarget,
    arguments: &str,
) -> Value {
    match target.kind {
        ToolCallKind::ToolSearch => json!({
            "type": "tool_search_call",
            "id": item_id,
            "call_id": call_id,
            "execution": "client",
            "arguments": serde_json::from_str::<Value>(arguments).unwrap_or_else(|_| json!({})),
            "status": "completed",
        }),
        ToolCallKind::Custom => json!({
            "type": "custom_tool_call",
            "id": item_id,
            "call_id": call_id,
            "name": target.name,
            "input": completed_custom_tool_input(arguments),
        }),
        ToolCallKind::Function => {
            let mut item = json!({
                "type": "function_call",
                "id": item_id,
                "call_id": call_id,
                "name": target.name,
                "arguments": arguments,
                "status": "completed",
            });
            if let Some(namespace) = &target.namespace {
                item["namespace"] = json!(namespace);
            }
            item
        }
    }
}

pub(super) fn web_search_item(item_id: &str, call_id: &str, status: &str, input: Value) -> Value {
    let query = input
        .get("query")
        .or_else(|| input.get("search_query"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut item = json!({
        "type": "web_search_call",
        "id": item_id,
        "call_id": call_id,
        "status": status,
    });
    if status != "in_progress" {
        item["action"] = json!({
            "type": "search",
            "query": query,
            "queries": [query],
        });
    }
    item
}
