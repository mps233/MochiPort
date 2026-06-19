use std::{
    collections::{HashMap, VecDeque},
    pin::Pin,
    task::{Context, Poll},
};

use axum::body::Bytes;
use futures_util::Stream;
use serde_json::{Value, json};

use crate::ai_gateway::model::{generate_item_id, generate_response_id};
use crate::ai_gateway::tool_names::{ToolCallKind, ToolCallTarget, ToolNameMap};

pub(super) struct AnthropicSseToResponsesSse<S> {
    inner: S,
    state: AnthropicStreamState,
    line_buf: String,
    event_name: Option<String>,
    data_lines: Vec<String>,
    output_queue: VecDeque<Bytes>,
}

impl<S> AnthropicSseToResponsesSse<S> {
    pub(super) fn new(inner: S, model: String, tool_name_map: ToolNameMap) -> Self {
        Self {
            inner,
            state: AnthropicStreamState::new(model, tool_name_map),
            line_buf: String::new(),
            event_name: None,
            data_lines: Vec::new(),
            output_queue: VecDeque::new(),
        }
    }

    fn process_sse_line(&mut self, line: &str) {
        if line.is_empty() {
            self.flush_sse_event();
            return;
        }
        if line.starts_with(':') {
            return;
        }
        if let Some(event) = line.strip_prefix("event:") {
            self.event_name = Some(event.strip_prefix(' ').unwrap_or(event).to_string());
            return;
        }
        if let Some(data) = line.strip_prefix("data:") {
            self.data_lines
                .push(data.strip_prefix(' ').unwrap_or(data).to_string());
        }
    }

    fn flush_sse_event(&mut self) {
        if self.data_lines.is_empty() {
            self.event_name = None;
            return;
        }
        let data = self.data_lines.join("\n");
        self.data_lines.clear();
        self.event_name = None;
        if data.trim() == "[DONE]" {
            self.state.handle_done(&mut self.output_queue);
            return;
        }
        if let Ok(value) = serde_json::from_str::<Value>(&data) {
            self.state.process_event(&value, &mut self.output_queue);
        }
    }
}

impl<S, E> Stream for AnthropicSseToResponsesSse<S>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
    E: std::fmt::Display,
{
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        if let Some(bytes) = this.output_queue.pop_front() {
            return Poll::Ready(Some(Ok(bytes)));
        }

        loop {
            match Pin::new(&mut this.inner).poll_next(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    let text = String::from_utf8_lossy(&chunk);
                    this.line_buf.push_str(&text);
                    while let Some(pos) = this.line_buf.find('\n') {
                        let line = this.line_buf[..pos].trim_end_matches('\r').to_string();
                        this.line_buf = this.line_buf[pos + 1..].to_string();
                        this.process_sse_line(&line);
                    }
                    if let Some(bytes) = this.output_queue.pop_front() {
                        return Poll::Ready(Some(Ok(bytes)));
                    }
                }
                Poll::Ready(Some(Err(e))) => {
                    return Poll::Ready(Some(Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        e.to_string(),
                    ))));
                }
                Poll::Ready(None) => {
                    if !this.line_buf.is_empty() {
                        let line = std::mem::take(&mut this.line_buf);
                        this.process_sse_line(line.trim_end_matches('\r'));
                    }
                    this.flush_sse_event();
                    this.state.handle_done(&mut this.output_queue);
                    if let Some(bytes) = this.output_queue.pop_front() {
                        return Poll::Ready(Some(Ok(bytes)));
                    }
                    return Poll::Ready(None);
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

struct AnthropicStreamState {
    has_started: bool,
    response_completed: bool,
    response_id: String,
    model: String,
    created_at: i64,
    sequence_number: usize,
    output_index: usize,
    message_item: Option<StreamMessageItem>,
    content_blocks: HashMap<usize, AnthropicContentBlockState>,
    web_search_blocks: HashMap<usize, AnthropicWebSearchBlockState>,
    completed_output: Vec<Value>,
    usage: Option<Value>,
    stop_reason: Option<String>,
    tool_name_map: ToolNameMap,
}

struct StreamMessageItem {
    item_id: String,
    output_index: usize,
    text: String,
    content_part_started: bool,
}

struct AnthropicContentBlockState {
    item_id: String,
    output_index: usize,
    target: ToolCallTarget,
    call_id: String,
    arguments: String,
    custom_emitted_input: String,
}

struct AnthropicWebSearchBlockState {
    item_id: String,
    output_index: usize,
    call_id: String,
    input: Value,
    result: Option<Value>,
}

impl AnthropicStreamState {
    fn new(model: String, tool_name_map: ToolNameMap) -> Self {
        Self {
            has_started: false,
            response_completed: false,
            response_id: generate_response_id(),
            model,
            created_at: unix_timestamp(),
            sequence_number: 0,
            output_index: 0,
            message_item: None,
            content_blocks: HashMap::new(),
            web_search_blocks: HashMap::new(),
            completed_output: Vec::new(),
            usage: None,
            stop_reason: None,
            tool_name_map,
        }
    }

    fn process_event(&mut self, event: &Value, queue: &mut VecDeque<Bytes>) {
        match event.get("type").and_then(Value::as_str) {
            Some("message_start") => self.handle_message_start(event, queue),
            Some("content_block_start") => self.handle_content_block_start(event, queue),
            Some("content_block_delta") => self.handle_content_block_delta(event, queue),
            Some("content_block_stop") => self.handle_content_block_stop(event, queue),
            Some("message_delta") => self.handle_message_delta(event),
            Some("message_stop") => self.handle_done(queue),
            _ => {}
        }
    }

    fn handle_message_start(&mut self, event: &Value, queue: &mut VecDeque<Bytes>) {
        if self.has_started {
            return;
        }
        self.has_started = true;
        let message = event.get("message").unwrap_or(event);
        if let Some(id) = message.get("id").and_then(Value::as_str) {
            self.response_id = id.to_string();
        }
        if let Some(model) = message.get("model").and_then(Value::as_str) {
            self.model = model.to_string();
        }
        if let Some(usage) = message
            .get("usage")
            .and_then(convert_anthropic_stream_usage)
        {
            self.merge_usage(usage);
        }
        self.emit_response_created(queue);
        self.emit_response_in_progress(queue);
    }

    fn handle_content_block_start(&mut self, event: &Value, queue: &mut VecDeque<Bytes>) {
        self.ensure_started(queue);
        let index = event.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
        let block = event.get("content_block").unwrap_or(&Value::Null);
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                self.ensure_message_item(queue);
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    if !text.is_empty() {
                        self.handle_text_delta(text, queue);
                    }
                }
            }
            Some("tool_use") => self.start_tool_block(index, block, queue),
            Some("server_tool_use") => self.start_server_tool_block(index, block, queue),
            Some("web_search_tool_result") => self.attach_web_search_result(index, block),
            _ => {}
        }
    }

    fn handle_content_block_delta(&mut self, event: &Value, queue: &mut VecDeque<Bytes>) {
        self.ensure_started(queue);
        let index = event.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
        let delta = event.get("delta").unwrap_or(&Value::Null);
        match delta.get("type").and_then(Value::as_str) {
            Some("text_delta") => {
                if let Some(text) = delta.get("text").and_then(Value::as_str) {
                    self.handle_text_delta(text, queue);
                }
            }
            Some("input_json_delta") => {
                if let Some(partial_json) = delta.get("partial_json").and_then(Value::as_str) {
                    self.handle_tool_delta(index, partial_json, queue);
                }
            }
            _ => {}
        }
    }

    fn handle_content_block_stop(&mut self, event: &Value, queue: &mut VecDeque<Bytes>) {
        let index = event.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
        if self.content_blocks.contains_key(&index) {
            self.close_tool_block(index, queue);
        }
        if self.web_search_blocks.contains_key(&index) {
            self.close_web_search_block(index, queue);
        }
    }

    fn handle_message_delta(&mut self, event: &Value) {
        if let Some(delta) = event.get("delta") {
            if let Some(stop_reason) = delta.get("stop_reason").and_then(Value::as_str) {
                self.stop_reason = Some(stop_reason.to_string());
            }
        }
        if let Some(usage) = event.get("usage").and_then(convert_anthropic_stream_usage) {
            self.merge_usage(usage);
        }
    }

    fn handle_done(&mut self, queue: &mut VecDeque<Bytes>) {
        if !self.has_started {
            return;
        }
        self.close_message_item(queue);
        let mut indices: Vec<usize> = self.content_blocks.keys().cloned().collect();
        indices.sort_unstable();
        for index in indices {
            self.close_tool_block(index, queue);
        }
        let mut web_indices: Vec<usize> = self.web_search_blocks.keys().cloned().collect();
        web_indices.sort_unstable();
        for index in web_indices {
            self.close_web_search_block(index, queue);
        }
        if !self.response_completed {
            self.emit_response_completed(queue);
        }
    }

    fn ensure_started(&mut self, queue: &mut VecDeque<Bytes>) {
        if self.has_started {
            return;
        }
        self.has_started = true;
        self.emit_response_created(queue);
        self.emit_response_in_progress(queue);
    }

    fn ensure_message_item(&mut self, queue: &mut VecDeque<Bytes>) {
        if self.message_item.is_some() {
            return;
        }
        let item_id = generate_item_id();
        let output_index = self.output_index;
        self.output_index += 1;
        emit_sse(
            queue,
            "response.output_item.added",
            json!({
                "type": "response.output_item.added",
                "sequence_number": self.next_seq(),
                "output_index": output_index,
                "item": {
                    "type": "message",
                    "id": item_id,
                    "role": "assistant",
                    "status": "in_progress",
                    "content": [],
                }
            }),
        );
        self.message_item = Some(StreamMessageItem {
            item_id,
            output_index,
            text: String::new(),
            content_part_started: false,
        });
    }

    fn handle_text_delta(&mut self, text: &str, queue: &mut VecDeque<Bytes>) {
        if text.is_empty() {
            return;
        }
        self.ensure_message_item(queue);

        let mut added_part = None;
        let mut delta_event = None;
        let seq_for_part = self.next_seq();
        let seq_for_delta = self.next_seq();
        if let Some(item) = self.message_item.as_mut() {
            if !item.content_part_started {
                item.content_part_started = true;
                added_part = Some(json!({
                    "type": "response.content_part.added",
                    "sequence_number": seq_for_part,
                    "item_id": item.item_id,
                    "output_index": item.output_index,
                    "content_index": 0,
                    "part": {"type": "output_text", "text": "", "annotations": []},
                }));
            }
            item.text.push_str(text);
            delta_event = Some(json!({
                "type": "response.output_text.delta",
                "sequence_number": seq_for_delta,
                "item_id": item.item_id,
                "output_index": item.output_index,
                "content_index": 0,
                "delta": text,
                "logprobs": [],
            }));
        }
        if let Some(data) = added_part {
            emit_sse(queue, "response.content_part.added", data);
        }
        if let Some(data) = delta_event {
            emit_sse(queue, "response.output_text.delta", data);
        }
    }

    fn close_message_item(&mut self, queue: &mut VecDeque<Bytes>) {
        let Some(item) = self.message_item.take() else {
            return;
        };
        if item.content_part_started {
            emit_sse(
                queue,
                "response.output_text.done",
                json!({
                    "type": "response.output_text.done",
                    "sequence_number": self.next_seq(),
                    "item_id": item.item_id,
                    "output_index": item.output_index,
                    "content_index": 0,
                    "text": item.text,
                    "logprobs": [],
                }),
            );
            emit_sse(
                queue,
                "response.content_part.done",
                json!({
                    "type": "response.content_part.done",
                    "sequence_number": self.next_seq(),
                    "item_id": item.item_id,
                    "output_index": item.output_index,
                    "content_index": 0,
                    "part": {"type": "output_text", "text": item.text, "annotations": []},
                }),
            );
        }
        let output_item = json!({
            "type": "message",
            "id": item.item_id,
            "role": "assistant",
            "status": "completed",
            "content": [{"type": "output_text", "text": item.text, "annotations": []}],
        });
        emit_sse(
            queue,
            "response.output_item.done",
            json!({
                "type": "response.output_item.done",
                "sequence_number": self.next_seq(),
                "output_index": item.output_index,
                "item": output_item.clone(),
            }),
        );
        self.completed_output.push(output_item);
    }

    fn start_tool_block(&mut self, index: usize, block: &Value, queue: &mut VecDeque<Bytes>) {
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

    fn handle_tool_delta(&mut self, index: usize, partial_json: &str, queue: &mut VecDeque<Bytes>) {
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

    fn close_tool_block(&mut self, index: usize, queue: &mut VecDeque<Bytes>) {
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

    fn start_server_tool_block(
        &mut self,
        index: usize,
        block: &Value,
        queue: &mut VecDeque<Bytes>,
    ) {
        self.close_message_item(queue);
        if self.web_search_blocks.contains_key(&index) {
            return;
        }
        if block.get("name").and_then(Value::as_str) != Some("web_search") {
            return;
        }

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
        let output_index = self.output_index;
        self.output_index += 1;
        let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
        let added_item = web_search_item(&item_id, &call_id, "in_progress", input.clone(), None);
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
        self.web_search_blocks.insert(
            index,
            AnthropicWebSearchBlockState {
                item_id,
                output_index,
                call_id,
                input,
                result: None,
            },
        );
    }

    fn attach_web_search_result(&mut self, index: usize, block: &Value) {
        let tool_use_id = block.get("tool_use_id").and_then(Value::as_str);
        let result = json!({
            "type": "web_search_tool_result",
            "tool_use_id": block.get("tool_use_id").cloned().unwrap_or(Value::Null),
            "content": block.get("content").cloned().unwrap_or(Value::Null),
            "is_error": block.get("is_error").cloned().unwrap_or(Value::Bool(false)),
        });

        if let Some(state) = self.web_search_blocks.get_mut(&index) {
            state.result = Some(result);
            return;
        }
        if let Some(tool_use_id) = tool_use_id {
            if let Some(state) = self
                .web_search_blocks
                .values_mut()
                .find(|state| state.call_id == tool_use_id)
            {
                state.result = Some(result);
            }
        }
    }

    fn close_web_search_block(&mut self, index: usize, queue: &mut VecDeque<Bytes>) {
        let Some(state) = self.web_search_blocks.remove(&index) else {
            return;
        };
        let failed = state
            .result
            .as_ref()
            .and_then(|result| result.get("is_error"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let item = web_search_item(
            &state.item_id,
            &state.call_id,
            if failed { "failed" } else { "completed" },
            state.input,
            state.result,
        );
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

    fn response_object(&self, status: &str) -> Value {
        let mut response = json!({
            "id": self.response_id,
            "object": "response",
            "model": self.model,
            "created_at": self.created_at,
            "status": status,
            "output": self.completed_output,
        });
        if let Some(usage) = &self.usage {
            response["usage"] = usage.clone();
        }
        response
    }

    fn emit_response_created(&mut self, queue: &mut VecDeque<Bytes>) {
        emit_sse(
            queue,
            "response.created",
            json!({
                "type": "response.created",
                "sequence_number": self.next_seq(),
                "response": self.response_object("in_progress"),
            }),
        );
    }

    fn emit_response_in_progress(&mut self, queue: &mut VecDeque<Bytes>) {
        emit_sse(
            queue,
            "response.in_progress",
            json!({
                "type": "response.in_progress",
                "sequence_number": self.next_seq(),
                "response": self.response_object("in_progress"),
            }),
        );
    }

    fn emit_response_completed(&mut self, queue: &mut VecDeque<Bytes>) {
        let status = match self.stop_reason.as_deref() {
            Some("max_tokens") => "incomplete",
            _ => "completed",
        };
        let event_type = if status == "incomplete" {
            "response.incomplete"
        } else {
            "response.completed"
        };
        emit_sse(
            queue,
            event_type,
            json!({
                "type": event_type,
                "sequence_number": self.next_seq(),
                "response": self.response_object(status),
            }),
        );
        self.response_completed = true;
    }

    fn next_seq(&mut self) -> usize {
        let seq = self.sequence_number;
        self.sequence_number += 1;
        seq
    }

    fn merge_usage(&mut self, usage: Value) {
        let Some(existing) = self.usage.as_mut() else {
            self.usage = Some(usage);
            return;
        };
        merge_i64_field(existing, &usage, "input_tokens");
        merge_i64_field(existing, &usage, "output_tokens");
        let input = existing
            .get("input_tokens")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let output = existing
            .get("output_tokens")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        existing["total_tokens"] = json!(input + output);

        if let Some(cached) = usage
            .get("input_tokens_details")
            .and_then(|details| details.get("cached_tokens"))
            .and_then(Value::as_i64)
        {
            existing["input_tokens_details"]["cached_tokens"] = json!(cached);
        }
    }
}

struct ToolDeltaEvent {
    event_type: &'static str,
    item_id: String,
    output_index: usize,
    delta: String,
}

fn tool_delta_event(
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

fn in_progress_tool_item(item_id: &str, call_id: &str, target: &ToolCallTarget) -> Value {
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

fn completed_tool_item(
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
            "input": parse_custom_tool_input(arguments).unwrap_or_else(|| arguments.to_string()),
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

fn web_search_item(
    item_id: &str,
    call_id: &str,
    status: &str,
    input: Value,
    result: Option<Value>,
) -> Value {
    let mut action = json!({
        "type": "web_search",
        "input": input,
    });
    if let Some(result) = result {
        action["result"] = result;
    }
    json!({
        "type": "web_search_call",
        "id": item_id,
        "call_id": call_id,
        "status": status,
        "action": action,
    })
}

fn partial_custom_tool_input(arguments: &str) -> Option<String> {
    parse_custom_tool_input(arguments).or_else(|| partial_wrapped_input_prefix(arguments))
}

fn parse_custom_tool_input(arguments: &str) -> Option<String> {
    serde_json::from_str::<Value>(arguments)
        .ok()?
        .get("input")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn partial_wrapped_input_prefix(arguments: &str) -> Option<String> {
    let mut rest = arguments.trim_start();
    rest = rest.strip_prefix('{')?.trim_start();
    let (key, after_key) = parse_json_string_prefix(rest)?;
    if key != "input" {
        return None;
    }
    rest = after_key.trim_start();
    rest = rest.strip_prefix(':')?.trim_start();
    parse_json_string_prefix(rest).map(|(value, _)| value)
}

fn parse_json_string_prefix(input: &str) -> Option<(String, &str)> {
    if !input.starts_with('"') {
        return None;
    }

    let mut output = String::new();
    let mut pos = 1;
    while pos < input.len() {
        let ch = input[pos..].chars().next()?;
        match ch {
            '"' => {
                let next = pos + ch.len_utf8();
                return Some((output, &input[next..]));
            }
            '\\' => {
                pos += ch.len_utf8();
                let escaped = input[pos..].chars().next()?;
                match escaped {
                    '"' => output.push('"'),
                    '\\' => output.push('\\'),
                    '/' => output.push('/'),
                    'b' => output.push('\u{0008}'),
                    'f' => output.push('\u{000c}'),
                    'n' => output.push('\n'),
                    'r' => output.push('\r'),
                    't' => output.push('\t'),
                    'u' => {
                        let after_u = pos + escaped.len_utf8();
                        let decoded = decode_json_unicode_escape(input, after_u)?;
                        output.push(decoded.0);
                        pos = decoded.1;
                        continue;
                    }
                    _ => output.push(escaped),
                }
                pos += escaped.len_utf8();
            }
            _ => {
                output.push(ch);
                pos += ch.len_utf8();
            }
        }
    }

    Some((output, ""))
}

fn decode_json_unicode_escape(input: &str, offset: usize) -> Option<(char, usize)> {
    let first = read_hex_u16(input, offset)?;
    let first_end = offset + 4;
    if (0xD800..=0xDBFF).contains(&first) {
        let low_offset = first_end + 2;
        if input.get(first_end..low_offset) != Some("\\u") {
            return None;
        }
        let second = read_hex_u16(input, low_offset)?;
        if !(0xDC00..=0xDFFF).contains(&second) {
            return None;
        }
        let codepoint = 0x10000 + (((first as u32 - 0xD800) << 10) | (second as u32 - 0xDC00));
        char::from_u32(codepoint).map(|ch| (ch, low_offset + 4))
    } else {
        char::from_u32(first as u32).map(|ch| (ch, first_end))
    }
}

fn read_hex_u16(input: &str, offset: usize) -> Option<u16> {
    let hex = input.get(offset..offset + 4)?;
    u16::from_str_radix(hex, 16).ok()
}

fn emit_sse(queue: &mut VecDeque<Bytes>, event_type: &str, data: Value) {
    queue.push_back(Bytes::from(format!(
        "event: {}\ndata: {}\n\n",
        event_type, data
    )));
}

fn convert_anthropic_stream_usage(usage: &Value) -> Option<Value> {
    usage.as_object()?;
    let input = usage
        .get("input_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let output = usage
        .get("output_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let cached = usage
        .get("cache_read_input_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    Some(json!({
        "input_tokens": input,
        "output_tokens": output,
        "total_tokens": input + output,
        "input_tokens_details": {"cached_tokens": cached},
        "output_tokens_details": {"reasoning_tokens": 0},
    }))
}

fn merge_i64_field(target: &mut Value, source: &Value, field: &str) {
    if let Some(value) = source.get(field).and_then(Value::as_i64) {
        if value != 0 || target.get(field).and_then(Value::as_i64).is_none() {
            target[field] = json!(value);
        }
    }
}

fn unix_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
