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
    prompt_cache_retention: Option<&str>,
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
    insert_prompt_cache_control(&mut body, prompt_cache_retention);
    Ok((Value::Object(body), tool_name_map))
}

fn insert_prompt_cache_control(
    body: &mut Map<String, Value>,
    prompt_cache_retention: Option<&str>,
) {
    if body.contains_key("cache_control") {
        return;
    }

    let mut cache_control = Map::new();
    cache_control.insert("type".to_string(), json!("ephemeral"));
    if matches!(
        normalized_anthropic_cache_ttl(prompt_cache_retention),
        Some("1h")
    ) {
        cache_control.insert("ttl".to_string(), json!("1h"));
    }
    body.insert("cache_control".to_string(), Value::Object(cache_control));
}

fn normalized_anthropic_cache_ttl(prompt_cache_retention: Option<&str>) -> Option<&'static str> {
    let value = prompt_cache_retention?.trim().to_ascii_lowercase();
    match value.as_str() {
        "1h" => Some("1h"),
        _ => None,
    }
}
