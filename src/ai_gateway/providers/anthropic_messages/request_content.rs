use serde_json::{Value, json};

use crate::ai_gateway::error::GatewayError;
use crate::ai_gateway::model::{
    ContentPart, FunctionCallOutput, FunctionCallOutputContentItem, ItemContent, ItemType,
    JsonString, ResponseItem,
};
use crate::ai_gateway::tool_names::ToolNameMap;

pub(super) fn build_anthropic_messages(
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
                    push_anthropic_message(&mut messages, role, content, MergeMode::None);
                }
            }
            ItemType::FunctionCallOutput
            | ItemType::ToolSearchOutput
            | ItemType::CustomToolCallOutput => {
                let call_id = item.call_id.as_deref().unwrap_or("");
                if call_id.is_empty() {
                    return Err(GatewayError::bad_request(
                        "anthropic_messages requires tool output call_id",
                    ));
                }
                let content = match item.item_type {
                    ItemType::ToolSearchOutput => {
                        Value::String(tool_search_output_to_anthropic_content(item))
                    }
                    _ => item
                        .output
                        .as_ref()
                        .map(function_call_output_to_anthropic_content)
                        .unwrap_or_else(|| Value::String(String::new())),
                };
                push_anthropic_message(
                    &mut messages,
                    "user",
                    vec![tool_result_block(call_id, content)],
                    MergeMode::ToolResult,
                );
            }
            ItemType::FunctionCall | ItemType::ToolSearchCall | ItemType::CustomToolCall => {
                if let Some(block) = response_tool_call_to_anthropic(item, tool_name_map) {
                    push_anthropic_message(
                        &mut messages,
                        "assistant",
                        vec![block],
                        MergeMode::ToolUse,
                    );
                }
            }
            ItemType::WebSearchCall => {
                if let Some(block) = web_search_call_to_anthropic(item) {
                    push_anthropic_message(
                        &mut messages,
                        "assistant",
                        vec![block],
                        MergeMode::ToolUse,
                    );
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

#[derive(Clone, Copy)]
enum MergeMode {
    None,
    ToolUse,
    ToolResult,
}

fn push_anthropic_message(
    messages: &mut Vec<Value>,
    role: &str,
    content: Vec<Value>,
    merge_mode: MergeMode,
) {
    if content.is_empty() {
        return;
    }
    if should_merge_with_last_message(messages, role, merge_mode)
        && let Some(last_content) = messages
            .last_mut()
            .and_then(|message| message.get_mut("content"))
            .and_then(Value::as_array_mut)
    {
        append_content_blocks(last_content, content, merge_mode);
        return;
    }
    messages.push(json!({
        "role": role,
        "content": content,
    }));
}

fn should_merge_with_last_message(messages: &[Value], role: &str, merge_mode: MergeMode) -> bool {
    let Some(last) = messages.last() else {
        return false;
    };
    if last.get("role").and_then(Value::as_str) != Some(role) {
        return false;
    }
    let Some(content) = last.get("content").and_then(Value::as_array) else {
        return false;
    };
    match merge_mode {
        MergeMode::None => false,
        MergeMode::ToolUse => content
            .iter()
            .all(|block| block.get("type").and_then(Value::as_str) == Some("tool_use")),
        MergeMode::ToolResult => content
            .iter()
            .all(|block| block.get("type").and_then(Value::as_str) == Some("tool_result")),
    }
}

fn append_content_blocks(target: &mut Vec<Value>, content: Vec<Value>, merge_mode: MergeMode) {
    match merge_mode {
        MergeMode::ToolResult => {
            for block in content {
                append_tool_result_block(target, block);
            }
        }
        _ => target.extend(content),
    }
}

fn append_tool_result_block(target: &mut Vec<Value>, mut block: Value) {
    let Some(tool_use_id) = block
        .get("tool_use_id")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        target.push(block);
        return;
    };
    let Some(existing) = target.iter_mut().find(|existing| {
        existing.get("type").and_then(Value::as_str) == Some("tool_result")
            && existing.get("tool_use_id").and_then(Value::as_str) == Some(tool_use_id.as_str())
    }) else {
        target.push(block);
        return;
    };
    let next_content = block.get_mut("content").map(Value::take);
    if let Some(next_content) = next_content {
        merge_tool_result_content(existing, next_content);
    }
}

fn tool_result_block(tool_use_id: &str, content: Value) -> Value {
    json!({
        "type": "tool_result",
        "tool_use_id": tool_use_id,
        "content": content,
    })
}

fn merge_tool_result_content(existing: &mut Value, next: Value) {
    let current = existing.get_mut("content").map(Value::take);
    existing["content"] = match current {
        Some(current) => combine_tool_result_content(current, next),
        None => next,
    };
}

fn combine_tool_result_content(current: Value, next: Value) -> Value {
    match (current, next) {
        (Value::String(current), Value::String(next)) => {
            if current.is_empty() {
                Value::String(next)
            } else if next.is_empty() {
                Value::String(current)
            } else {
                Value::String(format!("{current}\n\n{next}"))
            }
        }
        (Value::Array(mut current), Value::Array(next)) => {
            current.extend(next);
            Value::Array(current)
        }
        (Value::Array(mut current), Value::String(next)) => {
            if !next.is_empty() {
                current.push(json!({"type": "text", "text": next}));
            }
            Value::Array(current)
        }
        (Value::String(current), Value::Array(mut next)) => {
            if current.is_empty() {
                Value::Array(next)
            } else {
                let mut content = vec![json!({"type": "text", "text": current})];
                content.append(&mut next);
                Value::Array(content)
            }
        }
        (current, next) => Value::String(format!("{current}\n\n{next}")),
    }
}

fn web_search_call_to_anthropic(item: &ResponseItem) -> Option<Value> {
    let query = item
        .action
        .as_ref()
        .and_then(|action| action.get("query").or_else(|| action.get("search_query")))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if query.trim().is_empty() {
        return None;
    }
    Some(json!({
        "type": "tool_use",
        "id": item.call_id.as_deref().or(item.id.as_deref()).unwrap_or(""),
        "name": "WebSearch",
        "input": {"query": query},
    }))
}

fn tool_search_output_to_anthropic_content(item: &ResponseItem) -> String {
    serde_json::to_string(&json!({
        "status": item.status.as_deref().unwrap_or("completed"),
        "execution": item.execution.as_deref().unwrap_or("client"),
        "tools": item.tools.clone().unwrap_or_default(),
    }))
    .unwrap_or_else(|_| "{\"tools\":[]}".to_string())
}

fn function_call_output_to_anthropic_content(output: &FunctionCallOutput) -> Value {
    match output {
        FunctionCallOutput::Text(text) => Value::String(text.clone()),
        FunctionCallOutput::ContentItems(items) if tool_output_items_are_text_only(items) => {
            Value::String(output.to_chat_tool_content())
        }
        FunctionCallOutput::ContentItems(items) => {
            let blocks = items
                .iter()
                .filter_map(tool_output_content_item_to_anthropic)
                .collect::<Vec<_>>();
            if blocks.is_empty() {
                Value::String(output.to_chat_tool_content())
            } else {
                Value::Array(blocks)
            }
        }
    }
}

fn tool_output_items_are_text_only(items: &[FunctionCallOutputContentItem]) -> bool {
    items.iter().all(|item| {
        matches!(
            item.item_type.as_str(),
            "input_text" | "output_text" | "text"
        )
    })
}

fn tool_output_content_item_to_anthropic(item: &FunctionCallOutputContentItem) -> Option<Value> {
    match item.item_type.as_str() {
        "input_text" | "output_text" | "text" => text_block(item.text.as_deref().unwrap_or("")),
        "input_image" | "image_url" => image_block(item.image_url.as_deref().unwrap_or(""), None),
        _ => None,
    }
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
