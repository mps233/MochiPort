use serde_json::{Value, json};

use super::apply_patch_tool::{
    APPLY_PATCH_DESCRIPTION, APPLY_PATCH_INPUT_DESCRIPTION, APPLY_PATCH_TOOL_NAME,
};
use super::config::ProviderType;
use super::tool_names::ToolNameMap;

#[derive(Debug, Default)]
pub struct ResponsesLiteToolPreparation {
    pub carriers_removed: usize,
    pub tools_added: usize,
    pub duplicates_removed: usize,
    pub grok_tools_converted: usize,
    pub grok_tool_names: Option<ToolNameMap>,
}

impl ResponsesLiteToolPreparation {
    pub fn changed(&self) -> bool {
        self.carriers_removed > 0
            || self.tools_added > 0
            || self.duplicates_removed > 0
            || self.grok_tools_converted > 0
    }
}

/// Responses Lite stores client-executed tools in an `additional_tools` input
/// item. Only OpenAI Responses understands that carrier natively. Other
/// providers receive the same declarations through the ordinary top-level
/// `tools` field, and the carrier is removed from conversation history.
pub fn prepare_for_provider(
    raw_body: &mut Value,
    provider_type: &ProviderType,
) -> Result<ResponsesLiteToolPreparation, String> {
    if provider_type == &ProviderType::OpenAiResponses {
        return Ok(ResponsesLiteToolPreparation::default());
    }

    let mut preparation = ResponsesLiteToolPreparation::default();
    let additional_tools = extract_additional_tools(raw_body, &mut preparation)?;
    merge_top_level_tools(raw_body, additional_tools, &mut preparation)?;

    if provider_type == &ProviderType::GrokResponses {
        let mut tool_names = ToolNameMap::default();
        preparation.grok_tools_converted = convert_grok_tools(raw_body, &mut tool_names)?;
        normalize_grok_tool_choice(raw_body, &mut tool_names);
        if !tool_names.is_empty() {
            preparation.grok_tool_names = Some(tool_names);
        }
    }

    Ok(preparation)
}

fn extract_additional_tools(
    raw_body: &mut Value,
    preparation: &mut ResponsesLiteToolPreparation,
) -> Result<Vec<Value>, String> {
    let Some(input) = raw_body.get_mut("input") else {
        return Ok(Vec::new());
    };
    let Some(items) = input.as_array_mut() else {
        return Ok(Vec::new());
    };

    let mut retained = Vec::with_capacity(items.len());
    let mut tools = Vec::new();
    for item in std::mem::take(items) {
        if item.get("type").and_then(Value::as_str) != Some("additional_tools") {
            retained.push(item);
            continue;
        }

        preparation.carriers_removed += 1;
        match item.get("tools") {
            None | Some(Value::Null) => {}
            Some(Value::Array(additional)) => tools.extend(additional.iter().cloned()),
            Some(_) => return Err("additional_tools.tools must be an array".to_string()),
        }
    }
    *items = retained;
    Ok(tools)
}

fn merge_top_level_tools(
    raw_body: &mut Value,
    additional_tools: Vec<Value>,
    preparation: &mut ResponsesLiteToolPreparation,
) -> Result<(), String> {
    let object = raw_body
        .as_object_mut()
        .ok_or_else(|| "Responses request must be a JSON object".to_string())?;
    let existing_tools_value = object.remove("tools");
    let had_explicit_tools = existing_tools_value.is_some();
    let existing = match existing_tools_value {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(tools)) => tools,
        Some(other) => {
            object.insert("tools".to_string(), other);
            return Err("Responses tools must be an array".to_string());
        }
    };

    if existing.is_empty() && additional_tools.is_empty() {
        if had_explicit_tools {
            object.insert("tools".to_string(), Value::Array(Vec::new()));
        }
        return Ok(());
    }

    let existing_count = existing.len();
    let mut merged = Vec::with_capacity(existing_count + additional_tools.len());
    for tool in existing {
        preparation.duplicates_removed += merge_tool(&mut merged, tool).duplicates;
    }
    for tool in additional_tools {
        let outcome = merge_tool(&mut merged, tool);
        preparation.duplicates_removed += outcome.duplicates;
        if outcome.added {
            preparation.tools_added += 1;
        }
    }
    object.insert("tools".to_string(), Value::Array(merged));
    Ok(())
}

#[derive(Default)]
struct MergeOutcome {
    added: bool,
    duplicates: usize,
}

fn merge_tool(target: &mut Vec<Value>, candidate: Value) -> MergeOutcome {
    let identity = tool_identity(&candidate);
    let Some(existing_index) = target
        .iter()
        .position(|existing| tool_identity(existing) == identity)
    else {
        target.push(candidate);
        return MergeOutcome {
            added: true,
            duplicates: 0,
        };
    };

    let mut duplicates = 1;
    if candidate.get("type").and_then(Value::as_str) == Some("namespace") {
        duplicates += merge_namespace_children(&mut target[existing_index], candidate);
    }
    MergeOutcome {
        added: false,
        duplicates,
    }
}

fn merge_namespace_children(existing: &mut Value, mut candidate: Value) -> usize {
    let Some(candidate_tools) = candidate
        .as_object_mut()
        .and_then(|object| object.remove("tools"))
        .and_then(|tools| tools.as_array().cloned())
    else {
        return 0;
    };
    let Some(existing_object) = existing.as_object_mut() else {
        return 0;
    };
    let existing_tools = existing_object
        .entry("tools".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    let Some(existing_tools) = existing_tools.as_array_mut() else {
        return 0;
    };

    candidate_tools
        .into_iter()
        .map(|tool| merge_tool(existing_tools, tool).duplicates)
        .sum()
}

fn tool_identity(tool: &Value) -> String {
    let Some(object) = tool.as_object() else {
        return serde_json::to_string(tool).unwrap_or_default();
    };
    let tool_type = object.get("type").and_then(Value::as_str).unwrap_or("");
    let function = object.get("function").and_then(Value::as_object);
    let name = function
        .and_then(|function| function.get("name"))
        .or_else(|| object.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let namespace = object
        .get("namespace")
        .and_then(Value::as_str)
        .unwrap_or("");

    if !tool_type.is_empty() && (!name.is_empty() || tool_type == "tool_search") {
        format!("{tool_type}\0{namespace}\0{name}")
    } else if !tool_type.is_empty() {
        format!(
            "{tool_type}\0{}",
            serde_json::to_string(tool).unwrap_or_default()
        )
    } else {
        serde_json::to_string(tool).unwrap_or_default()
    }
}

fn convert_grok_tools(raw_body: &mut Value, tool_names: &mut ToolNameMap) -> Result<usize, String> {
    let Some(tools) = raw_body.get_mut("tools").and_then(Value::as_array_mut) else {
        return Ok(0);
    };
    let original = std::mem::take(tools);
    let mut converted = Vec::with_capacity(original.len());
    let mut converted_count = 0;

    for tool in original {
        match tool.get("type").and_then(Value::as_str) {
            Some("custom") => {
                if let Some(tool) = grok_custom_tool(&tool, tool_names)? {
                    converted.push(tool);
                    converted_count += 1;
                }
            }
            Some("namespace") => {
                let namespace = tool.get("name").and_then(Value::as_str).unwrap_or("");
                if let Some(children) = tool.get("tools").and_then(Value::as_array) {
                    for child in children {
                        if child.get("type").and_then(Value::as_str) != Some("function") {
                            continue;
                        }
                        if let Some(tool) = grok_function_tool(child, Some(namespace), tool_names) {
                            converted.push(tool);
                            converted_count += 1;
                        }
                    }
                }
            }
            Some("function") => {
                if let Some(tool) = grok_function_tool(&tool, None, tool_names) {
                    converted.push(tool);
                }
            }
            _ => converted.push(tool),
        }
    }

    let mut deduplicated = Vec::with_capacity(converted.len());
    for tool in converted {
        merge_tool(&mut deduplicated, tool);
    }
    *tools = deduplicated;
    Ok(converted_count)
}

fn grok_function_tool(
    tool: &Value,
    namespace: Option<&str>,
    tool_names: &mut ToolNameMap,
) -> Option<Value> {
    let object = tool.as_object()?;
    let function = object.get("function").and_then(Value::as_object);
    let name = function
        .and_then(|function| function.get("name"))
        .or_else(|| object.get("name"))
        .and_then(Value::as_str)?;
    let encoded_name = tool_names.encode_function(namespace, name);

    let mut result = object.clone();
    result.remove("function");
    result.remove("namespace");
    result.insert("type".to_string(), json!("function"));
    result.insert("name".to_string(), json!(encoded_name));
    if let Some(function) = function {
        for (key, value) in function {
            if key != "name" {
                result.insert(key.clone(), value.clone());
            }
        }
    }
    if !result.contains_key("parameters")
        && let Some(schema) = result
            .remove("input_schema")
            .or_else(|| result.remove("inputSchema"))
    {
        result.insert("parameters".to_string(), schema);
    }
    Some(Value::Object(result))
}

fn grok_custom_tool(tool: &Value, tool_names: &mut ToolNameMap) -> Result<Option<Value>, String> {
    let Some(object) = tool.as_object() else {
        return Ok(None);
    };
    let Some(name) = object.get("name").and_then(Value::as_str) else {
        return Err("custom tool name is required".to_string());
    };
    let description = if name == APPLY_PATCH_TOOL_NAME {
        Value::String(APPLY_PATCH_DESCRIPTION.to_string())
    } else {
        object
            .get("description")
            .cloned()
            .unwrap_or_else(|| Value::String(String::new()))
    };
    let input_description = if name == APPLY_PATCH_TOOL_NAME {
        APPLY_PATCH_INPUT_DESCRIPTION
    } else {
        "Freeform input for the custom tool."
    };

    Ok(Some(json!({
        "type": "function",
        "name": tool_names.encode_custom(name),
        "description": description,
        "parameters": {
            "type": "object",
            "properties": {
                "input": {
                    "type": "string",
                    "description": input_description
                }
            },
            "required": ["input"],
            "additionalProperties": false
        },
        "strict": false
    })))
}

fn normalize_grok_tool_choice(raw_body: &mut Value, tool_names: &mut ToolNameMap) {
    let Some(choice) = raw_body
        .get_mut("tool_choice")
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    let choice_type = choice
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let function = choice.get("function").and_then(Value::as_object);
    let name = function
        .and_then(|function| function.get("name"))
        .or_else(|| choice.get("name"))
        .and_then(Value::as_str)
        .map(str::to_string);

    match (choice_type.as_str(), name) {
        ("custom", Some(name)) => {
            choice.insert("type".to_string(), json!("function"));
            choice.insert("name".to_string(), json!(tool_names.encode_custom(&name)));
            choice.remove("function");
            choice.remove("namespace");
        }
        ("function", Some(name)) => {
            let namespace = choice
                .get("namespace")
                .and_then(Value::as_str)
                .map(str::to_string);
            choice.insert(
                "name".to_string(),
                json!(tool_names.encode_function(namespace.as_deref(), &name)),
            );
            choice.remove("function");
            choice.remove("namespace");
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_gateway::tool_names::{ToolCallKind, ToolCallTarget};

    #[test]
    fn openai_keeps_additional_tools_native() {
        let mut body = json!({
            "input": [{"type":"additional_tools","tools":[{"type":"custom","name":"exec"}]}]
        });
        let original = body.clone();

        let preparation = prepare_for_provider(&mut body, &ProviderType::OpenAiResponses).unwrap();

        assert!(!preparation.changed());
        assert_eq!(body, original);
    }

    #[test]
    fn chat_provider_merges_deduplicates_and_removes_carriers() {
        let mut body = json!({
            "tools": [{"type":"function","name":"wait","parameters":{"type":"object"}}],
            "input": [
                {"type":"additional_tools","role":"developer","tools":[
                    {"type":"function","name":"wait","parameters":{"type":"object"}},
                    {"type":"custom","name":"exec"}
                ]},
                {"type":"message","role":"user","content":"run it"}
            ]
        });

        let preparation = prepare_for_provider(&mut body, &ProviderType::ChatCompletions).unwrap();

        assert_eq!(preparation.carriers_removed, 1);
        assert_eq!(preparation.tools_added, 1);
        assert_eq!(preparation.duplicates_removed, 1);
        assert_eq!(body["tools"].as_array().unwrap().len(), 2);
        assert_eq!(body["tools"][0]["name"], "wait");
        assert_eq!(body["tools"][1]["name"], "exec");
        assert_eq!(body["input"].as_array().unwrap().len(), 1);
        assert_eq!(body["input"][0]["type"], "message");
    }

    #[test]
    fn duplicate_namespaces_merge_children_in_order() {
        let mut body = json!({
            "tools": [{"type":"namespace","name":"browser","tools":[
                {"type":"function","name":"open"}
            ]}],
            "input": [{"type":"additional_tools","tools":[
                {"type":"namespace","name":"browser","tools":[
                    {"type":"function","name":"open"},
                    {"type":"function","name":"click"}
                ]}
            ]}]
        });

        prepare_for_provider(&mut body, &ProviderType::AnthropicMessages).unwrap();

        let children = body["tools"][0]["tools"].as_array().unwrap();
        assert_eq!(children.len(), 2);
        assert_eq!(children[0]["name"], "open");
        assert_eq!(children[1]["name"], "click");
    }

    #[test]
    fn grok_converts_custom_and_namespace_tools_to_functions() {
        let mut body = json!({
            "input": [{"type":"additional_tools","tools":[
                {"type":"custom","name":"exec","description":"Run code"},
                {"type":"namespace","name":"browser","tools":[
                    {"type":"function","name":"open","parameters":{"type":"object"}}
                ]}
            ]}]
        });

        let preparation = prepare_for_provider(&mut body, &ProviderType::GrokResponses).unwrap();

        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 2);
        assert!(tools.iter().all(|tool| tool["type"] == "function"));
        assert_eq!(tools[0]["name"], "exec");
        let browser_name = tools[1]["name"].as_str().unwrap();
        let map = preparation.grok_tool_names.unwrap();
        assert_eq!(
            map.decode("exec"),
            ToolCallTarget {
                kind: ToolCallKind::Custom,
                namespace: None,
                name: "exec".to_string(),
            }
        );
        assert_eq!(
            map.decode(browser_name),
            ToolCallTarget::function(Some("browser"), "open")
        );
        assert!(body["input"].as_array().unwrap().is_empty());
    }
}
