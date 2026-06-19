use serde_json::{Map, Value, json};

use crate::ai_gateway::error::GatewayError;
use crate::ai_gateway::model::{
    ContentPart, GatewayRequest, ItemContent, ItemType, JsonString, ResponseItem,
};
use crate::ai_gateway::tool_names::ToolNameMap;

use super::types::{ANTHROPIC_WEB_SEARCH_TYPE, DEFAULT_MAX_TOKENS};
pub(super) fn build_anthropic_request(
    request: &GatewayRequest,
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
    Ok((Value::Object(body), tool_name_map))
}

fn build_anthropic_messages(
    input: &[ResponseItem],
    tool_name_map: &mut ToolNameMap,
) -> Result<Vec<Value>, GatewayError> {
    let mut messages = Vec::new();
    for item in input {
        match item.item_type {
            ItemType::Message
            | ItemType::InputText
            | ItemType::InputImage
            | ItemType::OutputText => {
                let role = anthropic_role(item.role.as_deref(), &item.item_type);
                let content = anthropic_content_blocks(item);
                if !content.is_empty() {
                    messages.push(json!({
                        "role": role,
                        "content": content,
                    }));
                }
            }
            ItemType::FunctionCallOutput | ItemType::CustomToolCallOutput => {
                let call_id = item.call_id.as_deref().unwrap_or("");
                if call_id.is_empty() {
                    return Err(GatewayError::bad_request(
                        "anthropic_messages requires tool output call_id",
                    ));
                }
                messages.push(json!({
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": call_id,
                        "content": item.output.as_ref().map(|output| output.to_chat_tool_content()).unwrap_or_default(),
                    }],
                }));
            }
            ItemType::FunctionCall | ItemType::ToolSearchCall | ItemType::CustomToolCall => {
                if let Some(block) = response_tool_call_to_anthropic(item, tool_name_map) {
                    messages.push(json!({
                        "role": "assistant",
                        "content": [block],
                    }));
                }
            }
            _ => {}
        }
    }

    if messages.is_empty() {
        return Err(GatewayError::bad_request(
            "anthropic_messages requires at least one user or assistant message",
        ));
    }
    Ok(messages)
}

fn response_tool_call_to_anthropic(
    item: &ResponseItem,
    tool_name_map: &mut ToolNameMap,
) -> Option<Value> {
    let (name, input) = match item.item_type {
        ItemType::FunctionCall => {
            let name = tool_name_map.encode_function(
                item.namespace.as_deref(),
                item.name.as_deref().unwrap_or(""),
            );
            let input = item
                .arguments
                .as_ref()
                .map(JsonString::to_value)
                .unwrap_or_else(|| json!({}));
            (name, input)
        }
        ItemType::ToolSearchCall => {
            let name = tool_name_map.encode_tool_search();
            let input = item
                .arguments
                .as_ref()
                .map(JsonString::to_value)
                .unwrap_or_else(|| json!({}));
            (name, input)
        }
        ItemType::CustomToolCall => {
            let name = tool_name_map.encode_custom(item.name.as_deref().unwrap_or(""));
            let input = json!({ "input": item.input.clone().unwrap_or_default() });
            (name, input)
        }
        _ => return None,
    };
    if name.trim().is_empty() {
        return None;
    }
    Some(json!({
        "type": "tool_use",
        "id": item.call_id.as_deref().or(item.id.as_deref()).unwrap_or(""),
        "name": name,
        "input": input,
    }))
}

fn build_anthropic_tools(request: &GatewayRequest, tool_name_map: &mut ToolNameMap) -> Vec<Value> {
    let mut tools = request.tools.clone();
    tools.extend(tool_search_output_tools(&request.input));

    tools
        .iter()
        .flat_map(|tool| {
            let Some(obj) = tool.as_object() else {
                return Vec::new();
            };
            match obj.get("type").and_then(Value::as_str) {
                Some("namespace") => {
                    let namespace = obj.get("name").and_then(Value::as_str).unwrap_or("");
                    obj.get("tools")
                        .and_then(Value::as_array)
                        .map(|items| {
                            items
                                .iter()
                                .filter_map(|item| {
                                    let item_obj = item.as_object()?;
                                    if item_obj.get("type").and_then(Value::as_str)
                                        != Some("function")
                                    {
                                        return None;
                                    }
                                    build_anthropic_function_tool(
                                        item_obj,
                                        Some(namespace),
                                        tool_name_map,
                                    )
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default()
                }
                Some("function") => build_anthropic_function_tool(obj, None, tool_name_map)
                    .map(|tool| vec![tool])
                    .unwrap_or_default(),
                Some("tool_search") => build_anthropic_tool_search_tool(obj, tool_name_map)
                    .map(|tool| vec![tool])
                    .unwrap_or_default(),
                Some("custom") => build_anthropic_custom_tool(obj, tool_name_map)
                    .map(|tool| vec![tool])
                    .unwrap_or_default(),
                Some("web_search") | Some("web_search_preview") => {
                    build_anthropic_web_search_tool(obj)
                        .map(|tool| vec![tool])
                        .unwrap_or_default()
                }
                _ => Vec::new(),
            }
        })
        .collect()
}

fn build_anthropic_function_tool(
    tool: &Map<String, Value>,
    namespace: Option<&str>,
    tool_name_map: &mut ToolNameMap,
) -> Option<Value> {
    let function = tool.get("function").and_then(Value::as_object);
    let name = function
        .and_then(|function| function.get("name"))
        .or_else(|| tool.get("name"))
        .and_then(Value::as_str)?;
    let encoded_name = tool_name_map.encode_function(namespace, name);
    let description = function
        .and_then(|function| function.get("description"))
        .or_else(|| tool.get("description"))
        .cloned()
        .unwrap_or_else(|| json!(""));
    let input_schema = function
        .and_then(|function| function.get("parameters"))
        .or_else(|| tool.get("parameters"))
        .cloned()
        .unwrap_or_else(default_tool_schema);

    Some(json!({
        "name": encoded_name,
        "description": description,
        "input_schema": input_schema,
    }))
}

fn build_anthropic_tool_search_tool(
    tool: &Map<String, Value>,
    tool_name_map: &mut ToolNameMap,
) -> Option<Value> {
    let mut result = Map::new();
    result.insert(
        "name".to_string(),
        json!(tool_name_map.encode_tool_search()),
    );
    result.insert(
        "description".to_string(),
        tool.get("description")
            .cloned()
            .unwrap_or_else(|| json!("Search available tools.")),
    );
    result.insert(
        "input_schema".to_string(),
        tool.get("parameters")
            .cloned()
            .unwrap_or_else(default_tool_schema),
    );
    Some(Value::Object(result))
}

fn build_anthropic_custom_tool(
    tool: &Map<String, Value>,
    tool_name_map: &mut ToolNameMap,
) -> Option<Value> {
    let name = tool.get("name").and_then(Value::as_str)?;
    Some(json!({
        "name": tool_name_map.encode_custom(name),
        "description": tool.get("description").cloned().unwrap_or_else(|| json!("")),
        "input_schema": {
            "type": "object",
            "properties": {
                "input": {
                    "type": "string",
                    "description": "Freeform input for the custom tool."
                }
            },
            "required": ["input"],
            "additionalProperties": false
        }
    }))
}

fn build_anthropic_web_search_tool(tool: &Map<String, Value>) -> Option<Value> {
    let mut result = Map::new();
    result.insert("type".to_string(), json!(ANTHROPIC_WEB_SEARCH_TYPE));
    result.insert("name".to_string(), json!("web_search"));

    let config = tool
        .get("web_search")
        .or_else(|| tool.get("web_search_preview"))
        .and_then(Value::as_object)
        .unwrap_or(tool);
    copy_optional_fields(
        config,
        &mut result,
        &[
            "max_uses",
            "allowed_domains",
            "blocked_domains",
            "user_location",
        ],
    );
    Some(Value::Object(result))
}

fn copy_optional_fields(
    source: &Map<String, Value>,
    target: &mut Map<String, Value>,
    keys: &[&str],
) {
    for key in keys {
        if let Some(value) = source.get(*key) {
            target.insert((*key).to_string(), value.clone());
        }
    }
}

fn tool_search_output_tools(items: &[ResponseItem]) -> Vec<Value> {
    items
        .iter()
        .filter(|item| item.item_type == ItemType::ToolSearchOutput)
        .flat_map(|item| item.tools.clone().unwrap_or_default())
        .collect()
}

fn default_tool_schema() -> Value {
    json!({
        "type": "object",
        "properties": {},
        "additionalProperties": true
    })
}

fn convert_tool_choice_to_anthropic(tool_choice: &Value, tool_name_map: &mut ToolNameMap) -> Value {
    if let Some(mode) = tool_choice.as_str() {
        return anthropic_tool_choice_mode(mode);
    }

    let Some(obj) = tool_choice.as_object() else {
        return json!({"type": "auto"});
    };

    if let Some(mode) = obj.get("mode").and_then(Value::as_str) {
        return anthropic_tool_choice_mode(mode);
    }

    match obj.get("type").and_then(Value::as_str) {
        Some("function") => {
            let namespace = obj
                .get("namespace")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty());
            obj.get("name")
                .and_then(Value::as_str)
                .map(|name| {
                    json!({
                        "type": "tool",
                        "name": tool_name_map.encode_function(namespace, name),
                    })
                })
                .unwrap_or_else(|| json!({"type": "auto"}))
        }
        Some("tool_search") => json!({
            "type": "tool",
            "name": tool_name_map.encode_tool_search(),
        }),
        Some("custom") => obj
            .get("name")
            .and_then(Value::as_str)
            .map(|name| {
                json!({
                    "type": "tool",
                    "name": tool_name_map.encode_custom(name),
                })
            })
            .unwrap_or_else(|| json!({"type": "auto"})),
        Some("auto") | Some("none") | Some("any") | Some("required") => {
            anthropic_tool_choice_mode(obj.get("type").and_then(Value::as_str).unwrap_or("auto"))
        }
        _ => json!({"type": "auto"}),
    }
}

fn anthropic_tool_choice_mode(mode: &str) -> Value {
    match mode {
        "none" => json!({"type": "none"}),
        "required" | "any" => json!({"type": "any"}),
        _ => json!({"type": "auto"}),
    }
}

fn anthropic_role(role: Option<&str>, item_type: &ItemType) -> &'static str {
    match (role, item_type) {
        (Some("assistant"), _) | (None, ItemType::OutputText) => "assistant",
        _ => "user",
    }
}

fn anthropic_content_blocks(item: &ResponseItem) -> Vec<Value> {
    match &item.content {
        Some(ItemContent::Text(text)) => text_block(text).into_iter().collect(),
        Some(ItemContent::Parts(parts)) => {
            parts.iter().filter_map(content_part_to_anthropic).collect()
        }
        None => {
            if let Some(text) = &item.text {
                text_block(text).into_iter().collect()
            } else if let Some(image_url) = &item.image_url {
                image_block(image_url, item.detail.as_deref())
                    .into_iter()
                    .collect()
            } else {
                Vec::new()
            }
        }
    }
}

fn content_part_to_anthropic(part: &ContentPart) -> Option<Value> {
    match part.part_type.as_str() {
        "input_text" | "output_text" | "text" => text_block(part.text.as_deref().unwrap_or("")),
        "input_image" | "image_url" => image_block(
            part.image_url.as_deref().unwrap_or(""),
            part.detail.as_deref(),
        ),
        _ => None,
    }
}

fn text_block(text: &str) -> Option<Value> {
    if text.is_empty() {
        None
    } else {
        Some(json!({"type": "text", "text": text}))
    }
}

fn image_block(image_url: &str, _detail: Option<&str>) -> Option<Value> {
    let data_url = image_url.strip_prefix("data:")?;
    let (media_type, data) = data_url.split_once(";base64,")?;
    Some(json!({
        "type": "image",
        "source": {
            "type": "base64",
            "media_type": media_type,
            "data": data,
        }
    }))
}
