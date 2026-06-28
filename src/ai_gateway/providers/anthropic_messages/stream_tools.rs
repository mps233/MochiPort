use std::collections::VecDeque;

use axum::body::Bytes;
use serde_json::{Value, json};

use super::stream_events::emit_sse;
use super::stream_items::{
    completed_tool_item, in_progress_tool_item, tool_delta_event, web_search_item,
};
use super::stream_state::AnthropicStreamState;
use crate::ai_gateway::model::generate_item_id;
use crate::ai_gateway::tool_names::{ToolCallKind, ToolCallTarget};

pub(super) struct AnthropicContentBlockState {
    pub(super) item_id: String,
    pub(super) output_index: usize,
    pub(super) target: ToolCallTarget,
    call_id: String,
    pub(super) arguments: String,
    pub(super) custom_emitted_input: String,
}

pub(super) struct AnthropicWebSearchBlockState {
    item_id: String,
    output_index: Option<usize>,
    call_id: String,
    input: Value,
    arguments: String,
    result: Option<Value>,
    close_on_block_stop: bool,
}

impl AnthropicStreamState {
    pub(super) fn start_tool_block(
        &mut self,
        index: usize,
        block: &Value,
        queue: &mut VecDeque<Bytes>,
    ) {
        self.close_reasoning_item(queue);
        self.close_message_item(queue);
        if self.content_blocks.contains_key(&index) {
            return;
        }

        let call_id = block
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let raw_name = block.get("name").and_then(Value::as_str).unwrap_or("");
        let target = self.tool_name_map.decode(raw_name);
        let item_id = generate_item_id();
        let output_index = self.output_index;
        self.output_index += 1;
        let added_item = in_progress_tool_item(&item_id, &call_id, &target);
        emit_sse(
            queue,
            "response.output_item.added",
            json!({
                "type": "response.output_item.added",
                "sequence_number": self.next_seq(),
                "output_index": output_index,
                "item": added_item,
            }),
        );

        let mut state = AnthropicContentBlockState {
            item_id,
            output_index,
            target,
            call_id,
            arguments: String::new(),
            custom_emitted_input: String::new(),
        };
        if let Some(input) = block.get("input") {
            if !input.is_null() && input != &json!({}) {
                state.arguments = serde_json::to_string(input).unwrap_or_default();
            }
        }
        self.content_blocks.insert(index, state);
    }

    pub(super) fn is_unmapped_web_search_tool_use(&self, block: &Value) -> bool {
        let name = block.get("name").and_then(Value::as_str).unwrap_or("");
        name == "WebSearch"
            || self.profile.is_web_search_server_tool(name) && !self.tool_name_map.has_encoded(name)
    }

    pub(super) fn handle_tool_delta(
        &mut self,
        index: usize,
        partial_json: &str,
        queue: &mut VecDeque<Bytes>,
    ) {
        if partial_json.is_empty() {
            return;
        }
        let pending = {
            let Some(state) = self.content_blocks.get_mut(&index) else {
                return;
            };
            state.arguments.push_str(partial_json);
            tool_delta_event(state, partial_json)
        };
        if let Some(pending) = pending {
            emit_sse(
                queue,
                pending.event_type,
                json!({
                    "type": pending.event_type,
                    "sequence_number": self.next_seq(),
                    "item_id": pending.item_id,
                    "output_index": pending.output_index,
                    "delta": pending.delta,
                }),
            );
        }
    }

    pub(super) fn close_tool_block(&mut self, index: usize, queue: &mut VecDeque<Bytes>) {
        let Some(state) = self.content_blocks.remove(&index) else {
            return;
        };
        let item = completed_tool_item(
            &state.item_id,
            &state.call_id,
            &state.target,
            &state.arguments,
        );
        match state.target.kind {
            ToolCallKind::Custom => emit_sse(
                queue,
                "response.custom_tool_call_input.done",
                json!({
                    "type": "response.custom_tool_call_input.done",
                    "sequence_number": self.next_seq(),
                    "item_id": item["id"],
                    "output_index": state.output_index,
                    "input": item["input"],
                }),
            ),
            ToolCallKind::Function => emit_sse(
                queue,
                "response.function_call_arguments.done",
                json!({
                    "type": "response.function_call_arguments.done",
                    "sequence_number": self.next_seq(),
                    "item_id": item["id"],
                    "output_index": state.output_index,
                    "name": item["name"],
                    "arguments": item["arguments"],
                }),
            ),
            ToolCallKind::ToolSearch => {}
        }
        emit_sse(
            queue,
            "response.output_item.done",
            json!({
                "type": "response.output_item.done",
                "sequence_number": self.next_seq(),
                "output_index": state.output_index,
                "item": item.clone(),
            }),
        );
        self.completed_output.push(item);
    }

    pub(super) fn start_server_tool_block(
        &mut self,
        index: usize,
        block: &Value,
        queue: &mut VecDeque<Bytes>,
    ) {
        self.close_reasoning_item(queue);
        self.close_message_item(queue);
        if self.web_search_blocks.contains_key(&index) {
            return;
        }
        let name = block.get("name").and_then(Value::as_str).unwrap_or("");
        if name != "WebSearch" && !self.profile.is_web_search_server_tool(name) {
            return;
        }
        let is_tool_use_web_search = block.get("type").and_then(Value::as_str) == Some("tool_use");
        let input = block.get("input").cloned().unwrap_or_else(|| json!({}));

        let call_id = block
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let item_id = if call_id.is_empty() {
            generate_item_id()
        } else {
            call_id.clone()
        };
        let arguments = if input.is_null() || input == json!({}) {
            String::new()
        } else {
            serde_json::to_string(&input).unwrap_or_default()
        };
        let output_index =
            self.emit_web_search_added_if_ready(&item_id, &call_id, input.clone(), queue);
        self.web_search_blocks.insert(
            index,
            AnthropicWebSearchBlockState {
                item_id,
                output_index,
                call_id,
                input,
                arguments,
                result: None,
                close_on_block_stop: is_tool_use_web_search,
            },
        );
    }

    pub(super) fn handle_web_search_delta(
        &mut self,
        index: usize,
        partial_json: &str,
        queue: &mut VecDeque<Bytes>,
    ) {
        if partial_json.is_empty() {
            return;
        }
        let Some((item_id, call_id, input, should_emit)) =
            self.web_search_blocks.get_mut(&index).map(|state| {
                state.arguments.push_str(partial_json);
                if let Ok(input) = serde_json::from_str::<Value>(&state.arguments) {
                    state.input = input;
                }
                (
                    state.item_id.clone(),
                    state.call_id.clone(),
                    state.input.clone(),
                    state.output_index.is_none() && !web_search_query(&state.input).is_empty(),
                )
            })
        else {
            return;
        };
        if should_emit {
            let output_index =
                self.emit_web_search_added_if_ready(&item_id, &call_id, input, queue);
            if let Some(output_index) = output_index
                && let Some(state) = self.web_search_blocks.get_mut(&index)
            {
                state.output_index = Some(output_index);
            }
        }
    }

    pub(super) fn attach_web_search_result(
        &mut self,
        index: usize,
        block: &Value,
        queue: &mut VecDeque<Bytes>,
    ) {
        let tool_use_id = block.get("tool_use_id").and_then(Value::as_str);
        let result = json!({
            "type": "web_search_tool_result",
            "tool_use_id": block.get("tool_use_id").cloned().unwrap_or(Value::Null),
            "content": block.get("content").cloned().unwrap_or(Value::Null),
            "is_error": block.get("is_error").cloned().unwrap_or(Value::Bool(false)),
        });

        let state_index = if self.web_search_blocks.contains_key(&index) {
            Some(index)
        } else {
            tool_use_id.and_then(|tool_use_id| {
                self.web_search_blocks
                    .iter()
                    .find(|(_, state)| state.call_id == tool_use_id)
                    .map(|(index, _)| *index)
            })
        };

        if let Some(index) = state_index {
            let Some(state) = self.web_search_blocks.get_mut(&index) else {
                return;
            };
            state.result = Some(result);
            self.close_web_search_block(index, queue);
            return;
        }
    }

    pub(super) fn close_web_search_block(&mut self, index: usize, queue: &mut VecDeque<Bytes>) {
        let Some(state) = self.web_search_blocks.remove(&index) else {
            return;
        };
        let failed = state
            .result
            .as_ref()
            .and_then(|result| result.get("is_error"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let input = if state.input == json!({}) && !state.arguments.is_empty() {
            serde_json::from_str::<Value>(&state.arguments).unwrap_or(state.input)
        } else {
            state.input
        };
        let output_index = if let Some(output_index) = state.output_index {
            output_index
        } else {
            if web_search_query(&input).is_empty() {
                return;
            }
            let Some(output_index) = self.emit_web_search_added_if_ready(
                &state.item_id,
                &state.call_id,
                input.clone(),
                queue,
            ) else {
                return;
            };
            output_index
        };
        let item = web_search_item(
            &state.item_id,
            &state.call_id,
            if failed { "failed" } else { "completed" },
            input,
        );
        if !failed {
            emit_sse(
                queue,
                "response.web_search_call.completed",
                json!({
                    "type": "response.web_search_call.completed",
                    "sequence_number": self.next_seq(),
                    "item_id": state.item_id,
                    "output_index": output_index,
                }),
            );
        }
        emit_sse(
            queue,
            "response.output_item.done",
            json!({
                "type": "response.output_item.done",
                "sequence_number": self.next_seq(),
                "output_index": output_index,
                "item": item.clone(),
            }),
        );
        self.completed_output.push(item);
    }

    pub(super) fn close_web_search_tool_use_block(
        &mut self,
        index: usize,
        queue: &mut VecDeque<Bytes>,
    ) {
        if self
            .web_search_blocks
            .get(&index)
            .map(|state| state.close_on_block_stop)
            .unwrap_or(false)
        {
            self.close_web_search_block(index, queue);
        }
    }

    fn emit_web_search_added_if_ready(
        &mut self,
        item_id: &str,
        call_id: &str,
        input: Value,
        queue: &mut VecDeque<Bytes>,
    ) -> Option<usize> {
        if web_search_query(&input).is_empty() {
            return None;
        }
        let output_index = self.output_index;
        self.output_index += 1;
        let added_item = web_search_item(item_id, call_id, "in_progress", input);
        emit_sse(
            queue,
            "response.output_item.added",
            json!({
                "type": "response.output_item.added",
                "sequence_number": self.next_seq(),
                "output_index": output_index,
                "item": added_item,
            }),
        );
        emit_sse(
            queue,
            "response.web_search_call.in_progress",
            json!({
                "type": "response.web_search_call.in_progress",
                "sequence_number": self.next_seq(),
                "item_id": item_id,
                "output_index": output_index,
            }),
        );
        emit_sse(
            queue,
            "response.web_search_call.searching",
            json!({
                "type": "response.web_search_call.searching",
                "sequence_number": self.next_seq(),
                "item_id": item_id,
                "output_index": output_index,
            }),
        );
        Some(output_index)
    }
}

fn web_search_query(input: &Value) -> &str {
    input
        .get("query")
        .or_else(|| input.get("search_query"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}
