use serde_json::{Map, Value, json};

use crate::ai_gateway::error::GatewayError;
use crate::ai_gateway::model::GatewayRequest;
use crate::ai_gateway::tool_names::ToolNameMap;

use super::options::AnthropicProviderProfile;
use super::request_content::build_anthropic_messages;
use super::request_reasoning::insert_reasoning_options;
use super::request_tools::{build_anthropic_tools, convert_tool_choice_to_anthropic};
use super::types::DEFAULT_MAX_TOKENS;

pub(super) fn build_anthropic_request(
    request: &GatewayRequest,
    profile: AnthropicProviderProfile,
) -> Result<(Value, ToolNameMap), GatewayError> {
    let mut tool_name_map = ToolNameMap::default();
    let mut body = Map::new();
    body.insert("model".to_string(), json!(request.model));
    body.insert(
        "max_tokens".to_string(),
        json!(request.max_output_tokens.unwrap_or(DEFAULT_MAX_TOKENS)),
    );

    if let Some(instructions) = request
        .instructions
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        body.insert("system".to_string(), json!(instructions));
    }
    if let Some(temperature) = request.temperature {
        body.insert("temperature".to_string(), json!(temperature));
    }
    if let Some(top_p) = request.top_p {
        body.insert("top_p".to_string(), json!(top_p));
    }
    if request.stream {
        body.insert("stream".to_string(), json!(true));
    }
    insert_reasoning_options(&mut body, profile, request.reasoning.as_ref());

    let messages = build_anthropic_messages(&request.input, &mut tool_name_map)?;
    body.insert("messages".to_string(), Value::Array(messages));

    let tools = build_anthropic_tools(request, &mut tool_name_map);
    if !tools.is_empty() {
        body.insert("tools".to_string(), Value::Array(tools));
    }
    if let Some(tool_choice) = &request.tool_choice {
        body.insert(
            "tool_choice".to_string(),
            convert_tool_choice_to_anthropic(tool_choice, &mut tool_name_map),
        );
    }
    insert_prompt_cache_control(&mut body);
    Ok((Value::Object(body), tool_name_map))
}

fn insert_prompt_cache_control(body: &mut Map<String, Value>) {
    let cache_control = anthropic_cache_control();

    if let Some(system) = body.get_mut("system") {
        insert_system_cache_control(system, &cache_control);
    }

    if let Some(Value::Array(messages)) = body.get_mut("messages") {
        insert_message_cache_control(messages, &cache_control);
    }
}

fn insert_system_cache_control(system: &mut Value, cache_control: &Map<String, Value>) {
    match system {
        Value::String(text) if !text.is_empty() => {
            let text = text.clone();
            *system = json!([{
                "type": "text",
                "text": text,
                "cache_control": cache_control,
            }]);
        }
        Value::Array(parts) => {
            for part in parts {
                if let Value::Object(part) = part
                    && is_cacheable_anthropic_text_block(part)
                {
                    part.entry("cache_control".to_string())
                        .or_insert_with(|| Value::Object(cache_control.clone()));
                }
            }
        }
        _ => {}
    }
}

fn insert_message_cache_control(messages: &mut [Value], cache_control: &Map<String, Value>) {
    for message in messages.iter_mut().rev() {
        if message.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(content) = message.get_mut("content") else {
            continue;
        };
        match content {
            Value::String(text) if !text.is_empty() => {
                let text = text.clone();
                *content = json!([{
                    "type": "text",
                    "text": text,
                    "cache_control": cache_control,
                }]);
                return;
            }
            Value::Array(parts) => {
                if let Some(Value::Object(part)) = parts.iter_mut().rev().find(|part| {
                    part.as_object()
                        .map(is_cacheable_anthropic_text_block)
                        .unwrap_or(false)
                }) {
                    part.entry("cache_control".to_string())
                        .or_insert_with(|| Value::Object(cache_control.clone()));
                    return;
                }
            }
            _ => {}
        }
    }
}

fn is_cacheable_anthropic_text_block(block: &Map<String, Value>) -> bool {
    block.get("type").and_then(Value::as_str) == Some("text")
        && block
            .get("text")
            .and_then(Value::as_str)
            .map(|text| !text.is_empty())
            .unwrap_or(false)
}

fn anthropic_cache_control() -> Map<String, Value> {
    let mut cache_control = Map::new();
    cache_control.insert("type".to_string(), json!("ephemeral"));
    cache_control
}
