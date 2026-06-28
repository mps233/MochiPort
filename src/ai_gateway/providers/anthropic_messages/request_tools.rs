use serde_json::{Map, Value, json};

use crate::ai_gateway::model::{GatewayRequest, ItemType, ResponseItem};
use crate::ai_gateway::tool_names::{ToolCallTarget, ToolNameMap};

use super::custom_tools::{custom_tool_description, custom_tool_input_description};
use super::types::ANTHROPIC_WEB_SEARCH_TYPE;

const WEB_SEARCH_DESCRIPTION: &str = "Search the web. Returns result blocks with titles and URLs. US-only.\n\n- The current month is June 2026 -- use this when searching for recent information.\n- `allowed_domains` / `blocked_domains` filter results.\n- After answering from results, end with a \"Sources:\" list of the URLs you used as markdown links.";

const JSON_SCHEMA_DRAFT_2020_12: &str = "https://json-schema.org/draft/2020-12/schema";

pub(super) fn build_anthropic_tools(
    request: &GatewayRequest,
    tool_name_map: &mut ToolNameMap,
) -> Vec<Value> {
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
                    build_anthropic_web_search_client_tool(obj)
                        .map(|tool| vec![tool])
                        .unwrap_or_default()
                }
                Some(tool_type) if tool_type == ANTHROPIC_WEB_SEARCH_TYPE => {
                    build_anthropic_web_search_tool(obj)
                        .map(|tool| vec![tool])
                        .unwrap_or_default()
                }
                _ => build_native_anthropic_client_tool(obj, tool_name_map)
                    .map(|tool| vec![tool])
                    .unwrap_or_default(),
            }
        })
        .collect()
}

fn build_anthropic_web_search_client_tool(tool: &Map<String, Value>) -> Option<Value> {
    let mut properties = Map::new();
    properties.insert(
        "query".to_string(),
        json!({
            "type": "string",
            "minLength": 2,
            "description": "The search query to use"
        }),
    );
    properties.insert(
        "allowed_domains".to_string(),
        json!({
            "type": "array",
            "items": {"type": "string"},
            "description": "Only include search results from these domains"
        }),
    );
    properties.insert(
        "blocked_domains".to_string(),
        json!({
            "type": "array",
            "items": {"type": "string"},
            "description": "Never include search results from these domains"
        }),
    );

    let mut result = Map::new();
    result.insert("name".to_string(), json!("WebSearch"));
    result.insert(
        "description".to_string(),
        tool.get("description")
            .cloned()
            .unwrap_or_else(|| json!(WEB_SEARCH_DESCRIPTION)),
    );
    result.insert(
        "input_schema".to_string(),
        json!({
            "type": "object",
            "$schema": JSON_SCHEMA_DRAFT_2020_12,
            "required": ["query"],
            "properties": properties,
            "additionalProperties": false
        }),
    );
    Some(Value::Object(result))
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
        .or_else(|| function.and_then(|function| function.get("input_schema")))
        .or_else(|| function.and_then(|function| function.get("inputSchema")))
        .or_else(|| tool.get("parameters"))
        .or_else(|| tool.get("input_schema"))
        .or_else(|| tool.get("inputSchema"))
        .cloned()
        .map(normalize_anthropic_input_schema)
        .unwrap_or_else(default_tool_schema);

    let mut result = Map::new();
    result.insert("name".to_string(), json!(encoded_name));
    result.insert("description".to_string(), description);
    result.insert("input_schema".to_string(), input_schema);
    Some(Value::Object(result))
}

fn build_native_anthropic_client_tool(
    tool: &Map<String, Value>,
    tool_name_map: &mut ToolNameMap,
) -> Option<Value> {
    let name = tool.get("name").and_then(Value::as_str)?;
    let input_schema = tool
        .get("input_schema")
        .or_else(|| tool.get("inputSchema"))
        .cloned()
        .map(normalize_anthropic_input_schema)?;
    tool_name_map.insert(name.to_string(), ToolCallTarget::function(None, name));

    let mut result = Map::new();
    result.insert("name".to_string(), json!(name));
    if let Some(description) = tool.get("description") {
        result.insert("description".to_string(), description.clone());
    }
    result.insert("input_schema".to_string(), input_schema);
    Some(Value::Object(result))
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
            .map(normalize_anthropic_input_schema)
            .unwrap_or_else(default_tool_schema),
    );
    Some(Value::Object(result))
}

fn build_anthropic_custom_tool(
    tool: &Map<String, Value>,
    tool_name_map: &mut ToolNameMap,
) -> Option<Value> {
    let name = tool.get("name").and_then(Value::as_str)?;
    let description = custom_tool_description(tool);
    Some(json!({
        "name": tool_name_map.encode_custom(name),
        "description": description,
        "input_schema": {
            "type": "object",
            "$schema": JSON_SCHEMA_DRAFT_2020_12,
            "properties": {
                "input": {
                    "type": "string",
                    "description": custom_tool_input_description(name)
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
    result
        .entry("max_uses".to_string())
        .or_insert_with(|| json!(8));
    if !result.contains_key("allowed_domains")
        && let Some(allowed_domains) = config
            .get("filters")
            .and_then(Value::as_object)
            .and_then(|filters| filters.get("allowed_domains"))
    {
        result.insert("allowed_domains".to_string(), allowed_domains.clone());
    }
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

fn normalize_anthropic_input_schema(schema: Value) -> Value {
    let Value::Object(mut schema) = schema else {
        return default_tool_schema();
    };

    schema
        .entry("type".to_string())
        .or_insert_with(|| json!("object"));
    schema
        .entry("$schema".to_string())
        .or_insert_with(|| json!(JSON_SCHEMA_DRAFT_2020_12));
    schema
        .entry("properties".to_string())
        .or_insert_with(|| json!({}));
    schema
        .entry("additionalProperties".to_string())
        .or_insert_with(|| json!(false));
    Value::Object(schema)
}

fn default_tool_schema() -> Value {
    json!({
        "type": "object",
        "$schema": JSON_SCHEMA_DRAFT_2020_12,
        "properties": {},
        "additionalProperties": false
    })
}

pub(super) fn convert_tool_choice_to_anthropic(
    tool_choice: &Value,
    tool_name_map: &mut ToolNameMap,
) -> Value {
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
        Some("tool") => obj
            .get("name")
            .and_then(Value::as_str)
            .map(|name| {
                json!({
                    "type": "tool",
                    "name": name,
                })
            })
            .unwrap_or_else(|| json!({"type": "auto"})),
        Some("web_search") | Some("web_search_preview") => {
            json!({
                "type": "tool",
                "name": "WebSearch",
            })
        }
        Some(tool_type) if tool_type == ANTHROPIC_WEB_SEARCH_TYPE => {
            json!({
                "type": "tool",
                "name": "web_search",
            })
        }
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
