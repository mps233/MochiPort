use serde_json::{Value, json};

use crate::ai_gateway::ir::{
    GatewayBuiltinCall, GatewayBuiltinTool, GatewayContentBlock, GatewayCustomTool,
    GatewayCustomToolCall, GatewayFunctionCall, GatewayFunctionTool, GatewayItem, GatewayMessage,
    GatewayNamespaceTool, GatewayReasoning, GatewayReasoningItem, GatewayTextOptions, GatewayTool,
    GatewayToolOutput, GatewayToolSearch, GatewayToolSearchCall, GatewayToolSearchOutput,
    GatewayTurn, GatewayUnknownItem, GatewayUnknownTool, TextKind, ToolKey, ToolKind,
};
use crate::ai_gateway::model::{
    FunctionCallOutput, GatewayRequest, ItemContent, ItemType, ResponseItem,
};
use crate::ai_gateway::tool_names::TOOL_SEARCH_NAME;

pub fn decode_gateway_turn(request: &GatewayRequest, raw: Value) -> GatewayTurn {
    GatewayTurn {
        model: request.model.clone(),
        instructions: request.instructions.clone(),
        input: request
            .input
            .iter()
            .map(|item| decode_item(item, raw_item(&raw, item)))
            .collect(),
        tools: request.tools.iter().map(decode_tool).collect(),
        tool_choice: request.tool_choice.clone(),
        reasoning: request
            .reasoning
            .as_ref()
            .map(|reasoning| GatewayReasoning {
                effort: reasoning.effort.clone(),
                budget_tokens: reasoning.budget_tokens,
                generate_summary: reasoning.generate_summary.clone(),
            }),
        text: request.text.as_ref().map(|text| GatewayTextOptions {
            format: text
                .format
                .as_ref()
                .map(|format| serde_json::to_value(format).unwrap_or_else(|_| Value::Null)),
        }),
        stream: request.stream,
        max_output_tokens: request.max_output_tokens,
        temperature: request.temperature,
        top_p: request.top_p,
        prompt_cache_key: request.prompt_cache_key.clone(),
        prompt_cache_retention: request.prompt_cache_retention.clone(),
        previous_response_id: request.previous_response_id.clone(),
        raw,
    }
}

fn decode_item(item: &ResponseItem, raw: Value) -> GatewayItem {
    match item.item_type {
        ItemType::Message | ItemType::InputText | ItemType::InputImage | ItemType::OutputText => {
            GatewayItem::Message(GatewayMessage {
                role: item.role.clone().unwrap_or_else(|| "user".to_string()),
                content: decode_content(item),
                status: item.status.clone(),
                raw,
            })
        }
        ItemType::Reasoning => GatewayItem::Reasoning(GatewayReasoningItem {
            id: item.id.clone(),
            status: item.status.clone(),
            summary: item
                .summary
                .as_ref()
                .map(|parts| parts.iter().map(|part| part.text.clone()).collect())
                .unwrap_or_default(),
            encrypted_content: item.encrypted_content.clone(),
            raw,
        }),
        ItemType::FunctionCall => GatewayItem::FunctionCall(GatewayFunctionCall {
            id: item.id.clone(),
            call_id: item.call_id.clone(),
            namespace: item.namespace.clone(),
            name: item.name.clone().unwrap_or_default(),
            arguments: item
                .arguments
                .as_ref()
                .map(|arguments| arguments.to_value())
                .unwrap_or_else(|| json!({})),
            status: item.status.clone(),
            raw,
        }),
        ItemType::FunctionCallOutput => GatewayItem::FunctionCallOutput(GatewayToolOutput {
            call_id: item.call_id.clone(),
            name: item.name.clone(),
            output: output_to_value(item.output.as_ref()),
            raw,
        }),
        ItemType::ToolSearchCall => GatewayItem::ToolSearchCall(GatewayToolSearchCall {
            id: item.id.clone(),
            call_id: item.call_id.clone(),
            execution: item
                .execution
                .clone()
                .unwrap_or_else(|| "client".to_string()),
            arguments: item
                .arguments
                .as_ref()
                .map(|arguments| arguments.to_value())
                .unwrap_or_else(|| json!({})),
            status: item.status.clone(),
            raw,
        }),
        ItemType::ToolSearchOutput => {
            let raw_tools = item.tools.clone().unwrap_or_default();
            GatewayItem::ToolSearchOutput(GatewayToolSearchOutput {
                call_id: item.call_id.clone(),
                status: item
                    .status
                    .clone()
                    .unwrap_or_else(|| "completed".to_string()),
                execution: item
                    .execution
                    .clone()
                    .unwrap_or_else(|| "client".to_string()),
                tools: raw_tools.iter().map(decode_tool).collect(),
                raw_tools,
                raw,
            })
        }
        ItemType::CustomToolCall => GatewayItem::CustomToolCall(GatewayCustomToolCall {
            id: item.id.clone(),
            call_id: item.call_id.clone(),
            name: item.name.clone().unwrap_or_default(),
            input: item.input.clone().unwrap_or_default(),
            status: item.status.clone(),
            raw,
        }),
        ItemType::CustomToolCallOutput => GatewayItem::CustomToolCallOutput(GatewayToolOutput {
            call_id: item.call_id.clone(),
            name: item.name.clone(),
            output: output_to_value(item.output.as_ref()),
            raw,
        }),
        ItemType::WebSearchCall | ItemType::ImageGenerationCall => {
            GatewayItem::BuiltinCall(GatewayBuiltinCall {
                id: item.id.clone(),
                item_type: raw_item_type(&raw, item).to_string(),
                status: item.status.clone(),
                action: item.action.clone(),
                raw,
            })
        }
        ItemType::Unknown => GatewayItem::Unknown(GatewayUnknownItem {
            item_type: raw_item_type(&raw, item).to_string(),
            raw,
        }),
    }
}

fn decode_content(item: &ResponseItem) -> Vec<GatewayContentBlock> {
    match &item.content {
        Some(ItemContent::Text(text)) => vec![GatewayContentBlock::Text {
            text: text.clone(),
            kind: content_text_kind(item),
            raw: None,
        }],
        Some(ItemContent::Parts(parts)) => parts
            .iter()
            .map(|part| match part.part_type.as_str() {
                "input_text" | "output_text" | "text" => GatewayContentBlock::Text {
                    text: part.text.clone().unwrap_or_default(),
                    kind: match part.part_type.as_str() {
                        "input_text" => TextKind::Input,
                        "output_text" => TextKind::Output,
                        _ => TextKind::Plain,
                    },
                    raw: serde_json::to_value(part).ok(),
                },
                "input_image" | "image_url" => GatewayContentBlock::Image {
                    image_url: part.image_url.clone().unwrap_or_default(),
                    detail: part.detail.clone(),
                    raw: serde_json::to_value(part).ok(),
                },
                other => GatewayContentBlock::Unknown {
                    block_type: other.to_string(),
                    raw: serde_json::to_value(part).unwrap_or_else(|_| Value::Null),
                },
            })
            .collect(),
        None => {
            if let Some(text) = &item.text {
                vec![GatewayContentBlock::Text {
                    text: text.clone(),
                    kind: content_text_kind(item),
                    raw: None,
                }]
            } else if let Some(image_url) = &item.image_url {
                vec![GatewayContentBlock::Image {
                    image_url: image_url.clone(),
                    detail: item.detail.clone(),
                    raw: None,
                }]
            } else {
                Vec::new()
            }
        }
    }
}

fn content_text_kind(item: &ResponseItem) -> TextKind {
    match item.item_type {
        ItemType::InputText => TextKind::Input,
        ItemType::OutputText => TextKind::Output,
        _ => TextKind::Plain,
    }
}

fn output_to_value(output: Option<&FunctionCallOutput>) -> Value {
    match output {
        Some(FunctionCallOutput::Text(text)) => Value::String(text.clone()),
        Some(FunctionCallOutput::ContentItems(items)) => {
            serde_json::to_value(items).unwrap_or_else(|_| Value::Array(Vec::new()))
        }
        None => Value::Null,
    }
}

fn decode_tool(tool: &Value) -> GatewayTool {
    let Some(obj) = tool.as_object() else {
        return GatewayTool::Unknown(GatewayUnknownTool {
            tool_type: "unknown".to_string(),
            raw: tool.clone(),
        });
    };
    match obj.get("type").and_then(Value::as_str).unwrap_or("unknown") {
        "function" => decode_function_tool(tool, None),
        "namespace" => {
            let namespace = obj
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let tools = obj
                .get("tools")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .map(|tool| decode_function_tool(tool, Some(namespace.as_str())))
                        .collect()
                })
                .unwrap_or_default();
            GatewayTool::Namespace(GatewayNamespaceTool {
                name: namespace,
                description: obj
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                tools,
                raw: tool.clone(),
            })
        }
        "tool_search" => GatewayTool::ToolSearch(GatewayToolSearch {
            key: ToolKey {
                namespace: None,
                name: TOOL_SEARCH_NAME.to_string(),
                kind: ToolKind::ToolSearch,
            },
            execution: obj
                .get("execution")
                .and_then(Value::as_str)
                .map(str::to_string),
            description: obj
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_string),
            parameters: obj.get("parameters").cloned(),
            raw: tool.clone(),
        }),
        "custom" => {
            let name = obj
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            GatewayTool::Custom(GatewayCustomTool {
                key: ToolKey {
                    namespace: None,
                    name,
                    kind: ToolKind::Custom,
                },
                description: obj
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                format: obj.get("format").cloned(),
                raw: tool.clone(),
            })
        }
        "web_search" | "web_search_preview" => GatewayTool::Builtin(GatewayBuiltinTool {
            key: ToolKey {
                namespace: None,
                name: "web_search".to_string(),
                kind: ToolKind::WebSearch,
            },
            raw: tool.clone(),
        }),
        "image_generation" => GatewayTool::Builtin(GatewayBuiltinTool {
            key: ToolKey {
                namespace: None,
                name: "image_generation".to_string(),
                kind: ToolKind::ImageGeneration,
            },
            raw: tool.clone(),
        }),
        other => GatewayTool::Unknown(GatewayUnknownTool {
            tool_type: other.to_string(),
            raw: tool.clone(),
        }),
    }
}

fn decode_function_tool(tool: &Value, namespace: Option<&str>) -> GatewayTool {
    let Some(obj) = tool.as_object() else {
        return GatewayTool::Unknown(GatewayUnknownTool {
            tool_type: "unknown".to_string(),
            raw: tool.clone(),
        });
    };
    let function = obj
        .get("function")
        .and_then(Value::as_object)
        .unwrap_or(obj);
    let name = function
        .get("name")
        .or_else(|| obj.get("name"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    GatewayTool::Function(GatewayFunctionTool {
        key: ToolKey {
            namespace: namespace.map(str::to_string),
            name,
            kind: ToolKind::Function,
        },
        description: function
            .get("description")
            .or_else(|| obj.get("description"))
            .and_then(Value::as_str)
            .map(str::to_string),
        parameters: function
            .get("parameters")
            .or_else(|| obj.get("parameters"))
            .cloned(),
        strict: function
            .get("strict")
            .or_else(|| obj.get("strict"))
            .and_then(Value::as_bool),
        raw: tool.clone(),
    })
}

fn raw_item(raw: &Value, item: &ResponseItem) -> Value {
    raw.get("input")
        .and_then(Value::as_array)
        .and_then(|items| {
            items.iter().find(|candidate| {
                let candidate_type = candidate.get("type").and_then(Value::as_str);
                (item.item_type == ItemType::Unknown
                    || candidate_type == Some(item_type_name(item)))
                    && same_optional_string(candidate, "id", item.id.as_deref())
                    && same_optional_string(candidate, "call_id", item.call_id.as_deref())
            })
        })
        .cloned()
        .unwrap_or_else(|| serde_json::to_value(item).unwrap_or_else(|_| Value::Null))
}

fn same_optional_string(candidate: &Value, key: &str, expected: Option<&str>) -> bool {
    match expected {
        Some(expected) => candidate.get(key).and_then(Value::as_str) == Some(expected),
        None => true,
    }
}

fn item_type_name(item: &ResponseItem) -> &'static str {
    match item.item_type {
        ItemType::Message => "message",
        ItemType::InputText => "input_text",
        ItemType::InputImage => "input_image",
        ItemType::FunctionCall => "function_call",
        ItemType::FunctionCallOutput => "function_call_output",
        ItemType::ToolSearchCall => "tool_search_call",
        ItemType::ToolSearchOutput => "tool_search_output",
        ItemType::CustomToolCall => "custom_tool_call",
        ItemType::CustomToolCallOutput => "custom_tool_call_output",
        ItemType::WebSearchCall => "web_search_call",
        ItemType::ImageGenerationCall => "image_generation_call",
        ItemType::Reasoning => "reasoning",
        ItemType::OutputText => "output_text",
        ItemType::Unknown => "unknown",
    }
}

fn raw_item_type<'a>(raw: &'a Value, item: &ResponseItem) -> &'a str {
    raw.get("type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| item_type_name(item))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn decode(raw: Value) -> GatewayTurn {
        let request: GatewayRequest = serde_json::from_value(raw.clone()).unwrap();
        decode_gateway_turn(&request, raw)
    }

    #[test]
    fn decodes_string_input_as_user_text_message() {
        let turn = decode(json!({
            "model": "deepseek-v4-flash",
            "input": "hello",
            "stream": false
        }));

        assert_eq!(turn.model, "deepseek-v4-flash");
        assert_eq!(turn.input.len(), 1);
        match &turn.input[0] {
            GatewayItem::Message(message) => {
                assert_eq!(message.role, "user");
                assert!(matches!(
                    message.content[0],
                    GatewayContentBlock::Text {
                        ref text,
                        kind: TextKind::Input,
                        ..
                    } if text == "hello"
                ));
            }
            other => panic!("expected message, got {other:?}"),
        }
    }

    #[test]
    fn decodes_namespaced_tool_and_tool_search_output_tools() {
        let turn = decode(json!({
            "model": "deepseek-v4-flash",
            "input": [{
                "type": "tool_search_output",
                "call_id": "search_1",
                "status": "completed",
                "execution": "client",
                "tools": [{
                    "type": "namespace",
                    "name": "codex_app",
                    "tools": [{
                        "type": "function",
                        "name": "read_thread_terminal",
                        "parameters": {"type": "object"}
                    }]
                }]
            }]
        }));

        match &turn.input[0] {
            GatewayItem::ToolSearchOutput(output) => {
                assert_eq!(output.call_id.as_deref(), Some("search_1"));
                match &output.tools[0] {
                    GatewayTool::Namespace(namespace) => {
                        assert_eq!(namespace.name, "codex_app");
                        match &namespace.tools[0] {
                            GatewayTool::Function(function) => {
                                assert_eq!(function.key.namespace.as_deref(), Some("codex_app"));
                                assert_eq!(function.key.name, "read_thread_terminal");
                            }
                            other => panic!("expected function tool, got {other:?}"),
                        }
                    }
                    other => panic!("expected namespace tool, got {other:?}"),
                }
            }
            other => panic!("expected tool search output, got {other:?}"),
        }
    }

    #[test]
    fn decodes_tool_search_and_custom_call_semantics() {
        let turn = decode(json!({
            "model": "deepseek-v4-flash",
            "tools": [
                {"type": "tool_search", "execution": "client"},
                {"type": "custom", "name": "apply_patch"}
            ],
            "input": [
                {
                    "type": "tool_search_call",
                    "call_id": "search_1",
                    "execution": "client",
                    "arguments": {"query": "browser", "limit": 2}
                },
                {
                    "type": "custom_tool_call",
                    "call_id": "call_patch",
                    "name": "apply_patch",
                    "input": "*** Begin Patch\n*** End Patch\n"
                }
            ]
        }));

        assert!(matches!(turn.tools[0], GatewayTool::ToolSearch(_)));
        assert!(matches!(turn.tools[1], GatewayTool::Custom(_)));
        match &turn.input[0] {
            GatewayItem::ToolSearchCall(call) => {
                assert_eq!(call.arguments, json!({"query": "browser", "limit": 2}));
            }
            other => panic!("expected tool search call, got {other:?}"),
        }
        match &turn.input[1] {
            GatewayItem::CustomToolCall(call) => {
                assert_eq!(call.name, "apply_patch");
                assert_eq!(call.input, "*** Begin Patch\n*** End Patch\n");
            }
            other => panic!("expected custom tool call, got {other:?}"),
        }
    }

    #[test]
    fn preserves_unknown_item_raw_payload() {
        let turn = decode(json!({
            "model": "deepseek-v4-flash",
            "input": [{
                "type": "future_tool_call",
                "id": "future_1",
                "payload": {"x": 1}
            }]
        }));

        match &turn.input[0] {
            GatewayItem::Unknown(unknown) => {
                assert_eq!(unknown.raw["type"], "future_tool_call");
                assert_eq!(unknown.raw["payload"], json!({"x": 1}));
            }
            other => panic!("expected unknown item, got {other:?}"),
        }
    }
}
