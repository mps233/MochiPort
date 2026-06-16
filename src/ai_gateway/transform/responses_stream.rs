//! Chat Completions SSE → Responses API SSE 流状态机。
//! 参考 AxonHub `responses/inbound_stream.go` 的 `responsesInboundStream`。

use std::collections::{HashMap, VecDeque};
use std::pin::Pin;
use std::task::{Context, Poll};

use axum::body::Bytes;
use futures_util::Stream;
use serde_json::{Value, json};

use crate::ai_gateway::model::{generate_item_id, generate_response_id};

// ─── Stream adapter ────────────────────────────────────────────

/// 将上游 Chat SSE byte stream 转换为 Responses SSE byte stream。
pub struct ChatSseToResponsesSse<S> {
    inner: S,
    state: ResponsesStreamState,
    /// 上游 SSE 未完成的行缓冲。
    line_buf: String,
    /// 已生成的输出事件队列。
    output_queue: VecDeque<Bytes>,
}

impl<S> ChatSseToResponsesSse<S> {
    pub fn new(inner: S, model: String) -> Self {
        Self {
            inner,
            state: ResponsesStreamState::new(model),
            line_buf: String::new(),
            output_queue: VecDeque::new(),
        }
    }
}

impl<S, E> Stream for ChatSseToResponsesSse<S>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
    E: std::fmt::Display,
{
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        // 先输出队列里的事件
        if let Some(bytes) = this.output_queue.pop_front() {
            return Poll::Ready(Some(Ok(bytes)));
        }

        // 从上游拉取数据
        loop {
            match Pin::new(&mut this.inner).poll_next(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    let text = String::from_utf8_lossy(&chunk);
                    this.line_buf.push_str(&text);

                    // 按行处理 SSE
                    while let Some(pos) = this.line_buf.find('\n') {
                        let line = this.line_buf[..pos].trim_end_matches('\r').to_string();
                        this.line_buf = this.line_buf[pos + 1..].to_string();

                        if let Some(data) = line.strip_prefix("data: ") {
                            if data.trim() == "[DONE]" {
                                // 生成结束事件
                                this.state.handle_done(&mut this.output_queue);
                                // drain output_queue
                                if let Some(bytes) = this.output_queue.pop_front() {
                                    return Poll::Ready(Some(Ok(bytes)));
                                }
                                return Poll::Ready(None);
                            }

                            if let Ok(chunk_json) = serde_json::from_str::<Value>(data) {
                                this.state
                                    .process_chunk(&chunk_json, &mut this.output_queue);
                            }
                        }
                    }

                    // 输出队列中的事件
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
                    // 流结束，确保所有事件都已生成
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

// ─── 状态机 ────────────────────────────────────────────────────

struct ResponsesStreamState {
    // 阶段标记
    has_started: bool,
    response_created: bool,
    message_item_started: bool,
    reasoning_item_started: bool,
    reasoning_summary_part: bool,
    content_part_started: bool,
    finished: bool,
    response_completed: bool,

    // 元数据
    response_id: String,
    model: String,
    created_at: i64,

    // 索引
    output_index: usize,
    content_index: usize,
    sequence_number: usize,
    current_item_id: String,

    // 积累器
    accumulated_text: String,
    accumulated_reasoning: String,

    // 工具调用追踪
    tool_calls: HashMap<usize, ToolCallState>,

    // usage
    usage: Option<Value>,

    // finish_reason
    finish_reason: Option<String>,
}

struct ToolCallState {
    id: String,
    name: String,
    arguments: String,
    item_id: String,
    output_index: usize,
}

impl ResponsesStreamState {
    fn new(model: String) -> Self {
        Self {
            has_started: false,
            response_created: false,
            message_item_started: false,
            reasoning_item_started: false,
            reasoning_summary_part: false,
            content_part_started: false,
            finished: false,
            response_completed: false,
            response_id: generate_response_id(),
            model,
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64,
            output_index: 0,
            content_index: 0,
            sequence_number: 0,
            current_item_id: String::new(),
            accumulated_text: String::new(),
            accumulated_reasoning: String::new(),
            tool_calls: HashMap::new(),
            usage: None,
            finish_reason: None,
        }
    }

    fn next_seq(&mut self) -> usize {
        let seq = self.sequence_number;
        self.sequence_number += 1;
        seq
    }

    fn response_object(&self, status: &str) -> Value {
        let mut resp = json!({
            "id": self.response_id,
            "object": "response",
            "model": self.model,
            "created_at": self.created_at,
            "status": status,
            "output": [],
        });
        if let Some(usage) = &self.usage {
            resp["usage"] = usage.clone();
        }
        resp
    }

    /// 处理一个 Chat SSE chunk。
    fn process_chunk(&mut self, chunk: &Value, queue: &mut VecDeque<Bytes>) {
        // 第一个 chunk：生成 response.created + response.in_progress
        if !self.has_started {
            self.has_started = true;
            if let Some(model) = chunk.get("model").and_then(|v| v.as_str()) {
                self.model = model.to_string();
            }
            if let Some(created) = chunk.get("created").and_then(|v| v.as_i64()) {
                self.created_at = created;
            }
            self.emit_response_created(queue);
        }

        let choice = match chunk
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|a| a.first())
        {
            Some(c) => c,
            None => {
                // 可能是纯 usage chunk
                if let Some(usage) = chunk.get("usage") {
                    self.usage = Some(convert_usage_value(usage));
                    if self.finished && !self.response_completed {
                        self.emit_response_completed(queue);
                    }
                }
                return;
            }
        };

        let delta = choice.get("delta").unwrap_or(&Value::Null);

        // reasoning_content
        if let Some(reasoning) = delta.get("reasoning_content").and_then(|v| v.as_str()) {
            if !reasoning.is_empty() {
                self.handle_reasoning_delta(reasoning, queue);
            }
        }

        // content
        if let Some(content) = delta.get("content").and_then(|v| v.as_str()) {
            if !content.is_empty() {
                self.handle_content_delta(content, queue);
            }
        }

        // tool_calls
        if let Some(tool_calls) = delta.get("tool_calls").and_then(|v| v.as_array()) {
            for tc in tool_calls {
                self.handle_tool_call_delta(tc, queue);
            }
        }

        // finish_reason
        if let Some(reason) = choice.get("finish_reason").and_then(|v| v.as_str()) {
            self.finish_reason = Some(reason.to_string());
            self.handle_finish(queue);
        }

        // usage（有些 provider 在最后一个 chunk 里带 usage）
        if let Some(usage) = chunk.get("usage") {
            self.usage = Some(convert_usage_value(usage));
            if self.finished && !self.response_completed {
                self.emit_response_completed(queue);
            }
        }
    }

    fn handle_reasoning_delta(&mut self, text: &str, queue: &mut VecDeque<Bytes>) {
        if !self.reasoning_item_started {
            self.current_item_id = generate_item_id();
            let seq = self.next_seq();
            emit_sse(
                queue,
                "response.output_item.added",
                json!({
                    "type": "response.output_item.added",
                    "sequence_number": seq,
                    "output_index": self.output_index,
                    "item": {
                        "type": "reasoning",
                        "id": self.current_item_id,
                        "status": "in_progress",
                        "summary": [],
                    }
                }),
            );
            self.reasoning_item_started = true;
        }

        if !self.reasoning_summary_part {
            let seq = self.next_seq();
            emit_sse(
                queue,
                "response.reasoning_summary_part.added",
                json!({
                    "type": "response.reasoning_summary_part.added",
                    "sequence_number": seq,
                    "item_id": self.current_item_id,
                    "output_index": self.output_index,
                    "summary_index": 0,
                    "part": {"type": "summary_text", "text": ""},
                }),
            );
            self.reasoning_summary_part = true;
        }

        self.accumulated_reasoning.push_str(text);
        let seq = self.next_seq();
        emit_sse(
            queue,
            "response.reasoning_summary_text.delta",
            json!({
                "type": "response.reasoning_summary_text.delta",
                "sequence_number": seq,
                "item_id": self.current_item_id,
                "output_index": self.output_index,
                "summary_index": 0,
                "delta": text,
            }),
        );
    }

    fn close_reasoning_item(&mut self, queue: &mut VecDeque<Bytes>) {
        if !self.reasoning_item_started {
            return;
        }

        // summary_text.done
        let seq = self.next_seq();
        emit_sse(
            queue,
            "response.reasoning_summary_text.done",
            json!({
                "type": "response.reasoning_summary_text.done",
                "sequence_number": seq,
                "item_id": self.current_item_id,
                "output_index": self.output_index,
                "summary_index": 0,
                "text": self.accumulated_reasoning,
            }),
        );

        // summary_part.done
        let seq = self.next_seq();
        emit_sse(
            queue,
            "response.reasoning_summary_part.done",
            json!({
                "type": "response.reasoning_summary_part.done",
                "sequence_number": seq,
                "item_id": self.current_item_id,
                "output_index": self.output_index,
                "summary_index": 0,
                "part": {"type": "summary_text", "text": self.accumulated_reasoning},
            }),
        );

        // output_item.done
        let seq = self.next_seq();
        emit_sse(
            queue,
            "response.output_item.done",
            json!({
                "type": "response.output_item.done",
                "sequence_number": seq,
                "output_index": self.output_index,
                "item": {
                    "type": "reasoning",
                    "id": self.current_item_id,
                    "status": "completed",
                    "summary": [{"type": "summary_text", "text": self.accumulated_reasoning}],
                }
            }),
        );

        self.output_index += 1;
        self.reasoning_item_started = false;
        self.reasoning_summary_part = false;
        self.accumulated_reasoning.clear();
    }

    fn handle_content_delta(&mut self, text: &str, queue: &mut VecDeque<Bytes>) {
        // 如果 reasoning 还在，先关闭
        if self.reasoning_item_started {
            self.close_reasoning_item(queue);
        }

        if !self.message_item_started {
            self.current_item_id = generate_item_id();
            let seq = self.next_seq();
            emit_sse(
                queue,
                "response.output_item.added",
                json!({
                    "type": "response.output_item.added",
                    "sequence_number": seq,
                    "output_index": self.output_index,
                    "item": {
                        "type": "message",
                        "id": self.current_item_id,
                        "role": "assistant",
                        "status": "in_progress",
                        "content": [],
                    }
                }),
            );
            self.message_item_started = true;
        }

        if !self.content_part_started {
            let seq = self.next_seq();
            emit_sse(
                queue,
                "response.content_part.added",
                json!({
                    "type": "response.content_part.added",
                    "sequence_number": seq,
                    "item_id": self.current_item_id,
                    "output_index": self.output_index,
                    "content_index": self.content_index,
                    "part": {"type": "output_text", "text": "", "annotations": []},
                }),
            );
            self.content_part_started = true;
        }

        self.accumulated_text.push_str(text);
        let seq = self.next_seq();
        emit_sse(
            queue,
            "response.output_text.delta",
            json!({
                "type": "response.output_text.delta",
                "sequence_number": seq,
                "item_id": self.current_item_id,
                "output_index": self.output_index,
                "content_index": self.content_index,
                "delta": text,
            }),
        );
    }

    fn close_content_part(&mut self, queue: &mut VecDeque<Bytes>) {
        if !self.content_part_started {
            return;
        }

        // output_text.done
        let seq = self.next_seq();
        emit_sse(
            queue,
            "response.output_text.done",
            json!({
                "type": "response.output_text.done",
                "sequence_number": seq,
                "item_id": self.current_item_id,
                "output_index": self.output_index,
                "content_index": self.content_index,
                "text": self.accumulated_text,
            }),
        );

        // content_part.done
        let seq = self.next_seq();
        emit_sse(
            queue,
            "response.content_part.done",
            json!({
                "type": "response.content_part.done",
                "sequence_number": seq,
                "item_id": self.current_item_id,
                "output_index": self.output_index,
                "content_index": self.content_index,
                "part": {"type": "output_text", "text": self.accumulated_text, "annotations": []},
            }),
        );

        self.content_part_started = false;
    }

    fn close_message_item(&mut self, queue: &mut VecDeque<Bytes>) {
        if !self.message_item_started {
            return;
        }

        self.close_content_part(queue);

        // output_item.done
        let seq = self.next_seq();
        emit_sse(
            queue,
            "response.output_item.done",
            json!({
                "type": "response.output_item.done",
                "sequence_number": seq,
                "output_index": self.output_index,
                "item": {
                    "type": "message",
                    "id": self.current_item_id,
                    "role": "assistant",
                    "status": "completed",
                    "content": [{"type": "output_text", "text": self.accumulated_text, "annotations": []}],
                }
            }),
        );

        self.output_index += 1;
        self.message_item_started = false;
        self.accumulated_text.clear();
    }

    fn handle_tool_call_delta(&mut self, tc: &Value, queue: &mut VecDeque<Bytes>) {
        let index = tc.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let func = tc.get("function").unwrap_or(&Value::Null);

        if !self.tool_calls.contains_key(&index) {
            // 先关闭 content/reasoning
            if self.message_item_started {
                self.close_message_item(queue);
            }
            if self.reasoning_item_started {
                self.close_reasoning_item(queue);
            }

            let item_id = generate_item_id();
            let call_id = tc
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let name = func
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let tc_state = ToolCallState {
                id: call_id.clone(),
                name: name.clone(),
                arguments: String::new(),
                item_id: item_id.clone(),
                output_index: self.output_index,
            };

            // output_item.added for function_call
            let seq = self.next_seq();
            emit_sse(
                queue,
                "response.output_item.added",
                json!({
                    "type": "response.output_item.added",
                    "sequence_number": seq,
                    "output_index": self.output_index,
                    "item": {
                        "type": "function_call",
                        "id": item_id,
                        "call_id": call_id,
                        "name": name,
                        "arguments": "",
                        "status": "in_progress",
                    }
                }),
            );

            self.tool_calls.insert(index, tc_state);
            self.output_index += 1;
        }

        // 积累 arguments
        if let Some(args) = func.get("arguments").and_then(|v| v.as_str()) {
            if !args.is_empty() {
                let seq = self.next_seq();
                let tc_state = self.tool_calls.get_mut(&index).unwrap();
                tc_state.arguments.push_str(args);
                let item_id = tc_state.item_id.clone();
                let output_index = tc_state.output_index;

                emit_sse(
                    queue,
                    "response.function_call_arguments.delta",
                    json!({
                        "type": "response.function_call_arguments.delta",
                        "sequence_number": seq,
                        "item_id": item_id,
                        "output_index": output_index,
                        "delta": args,
                    }),
                );
            }
        }
    }

    fn close_tool_calls(&mut self, queue: &mut VecDeque<Bytes>) {
        let indices: Vec<usize> = self.tool_calls.keys().cloned().collect();
        for index in indices {
            let tc = self.tool_calls.remove(&index).unwrap();

            // function_call_arguments.done
            let seq = self.next_seq();
            emit_sse(
                queue,
                "response.function_call_arguments.done",
                json!({
                    "type": "response.function_call_arguments.done",
                    "sequence_number": seq,
                    "item_id": tc.item_id,
                    "output_index": tc.output_index,
                    "arguments": tc.arguments,
                }),
            );

            // output_item.done
            let seq = self.next_seq();
            emit_sse(
                queue,
                "response.output_item.done",
                json!({
                    "type": "response.output_item.done",
                    "sequence_number": seq,
                    "output_index": tc.output_index,
                    "item": {
                        "type": "function_call",
                        "id": tc.item_id,
                        "call_id": tc.id,
                        "name": tc.name,
                        "arguments": tc.arguments,
                        "status": "completed",
                    }
                }),
            );
        }
    }

    fn handle_finish(&mut self, queue: &mut VecDeque<Bytes>) {
        // 关闭所有打开的 item
        if self.reasoning_item_started {
            self.close_reasoning_item(queue);
        }
        if self.message_item_started {
            self.close_message_item(queue);
        }
        if !self.tool_calls.is_empty() {
            self.close_tool_calls(queue);
        }
        self.finished = true;

        // 如果已有 usage，立即完成
        if self.usage.is_some() {
            self.emit_response_completed(queue);
        }
    }

    fn handle_done(&mut self, queue: &mut VecDeque<Bytes>) {
        if !self.has_started {
            return;
        }
        // 确保完成
        if !self.finished {
            self.handle_finish(queue);
        }
        if !self.response_completed {
            self.emit_response_completed(queue);
        }
    }

    fn emit_response_created(&mut self, queue: &mut VecDeque<Bytes>) {
        let seq = self.next_seq();
        emit_sse(
            queue,
            "response.created",
            json!({
                "type": "response.created",
                "sequence_number": seq,
                "response": self.response_object("in_progress"),
            }),
        );

        self.response_created = true;
    }

    fn emit_response_completed(&mut self, queue: &mut VecDeque<Bytes>) {
        let status = match self.finish_reason.as_deref() {
            Some("length") => "incomplete",
            _ => "completed",
        };

        let event_type = match status {
            "incomplete" => "response.incomplete",
            _ => "response.completed",
        };

        let seq = self.next_seq();
        emit_sse(
            queue,
            event_type,
            json!({
                "type": event_type,
                "sequence_number": seq,
                "response": self.response_object(status),
            }),
        );

        self.response_completed = true;
    }
}

// ─── 工具函数 ──────────────────────────────────────────────────

fn emit_sse(queue: &mut VecDeque<Bytes>, event_type: &str, data: Value) {
    let line = format!("event: {}\ndata: {}\n\n", event_type, data.to_string());
    queue.push_back(Bytes::from(line));
}

fn convert_usage_value(usage: &Value) -> Value {
    let input = usage
        .get("prompt_tokens")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let output = usage
        .get("completion_tokens")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let total = usage
        .get("total_tokens")
        .and_then(|v| v.as_i64())
        .unwrap_or(input + output);

    let cached = usage
        .get("prompt_tokens_details")
        .and_then(|d| d.get("cached_tokens"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let reasoning = usage
        .get("completion_tokens_details")
        .and_then(|d| d.get("reasoning_tokens"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    json!({
        "input_tokens": input,
        "output_tokens": output,
        "total_tokens": total,
        "input_tokens_details": {"cached_tokens": cached},
        "output_tokens_details": {"reasoning_tokens": reasoning},
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 辅助：直接对状态机喂 chunk，收集输出事件。
    fn feed_chunks(chunks: &[Value]) -> Vec<(String, Value)> {
        let mut state = ResponsesStreamState::new("test-model".into());
        let mut queue = VecDeque::new();
        for chunk in chunks {
            state.process_chunk(chunk, &mut queue);
        }
        state.handle_done(&mut queue);
        parse_events(&queue)
    }

    /// 解析 SSE 事件队列为 (event_type, data_json) 列表。
    fn parse_events(queue: &VecDeque<Bytes>) -> Vec<(String, Value)> {
        let mut events = Vec::new();
        for bytes in queue {
            let text = String::from_utf8_lossy(bytes);
            let mut event_type = String::new();
            let mut data = String::new();
            for line in text.lines() {
                if let Some(et) = line.strip_prefix("event: ") {
                    event_type = et.to_string();
                } else if let Some(d) = line.strip_prefix("data: ") {
                    data = d.to_string();
                }
            }
            if !event_type.is_empty() && !data.is_empty() {
                let val: Value = serde_json::from_str(&data).unwrap();
                events.push((event_type, val));
            }
        }
        events
    }

    fn event_types(events: &[(String, Value)]) -> Vec<&str> {
        events.iter().map(|(t, _)| t.as_str()).collect()
    }

    // ─── 基本文本流 ────────────────────────────────────────────

    #[test]
    fn test_simple_text_stream() {
        let chunks = vec![
            json!({
                "model": "deepseek-chat",
                "created": 1700000000,
                "choices": [{"index": 0, "delta": {"content": "Hello"}}]
            }),
            json!({
                "choices": [{"index": 0, "delta": {"content": " world"}}]
            }),
            json!({
                "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7}
            }),
        ];

        let events = feed_chunks(&chunks);
        let types = event_types(&events);

        assert!(types.contains(&"response.created"));
        assert!(types.contains(&"response.output_item.added"));
        assert!(types.contains(&"response.content_part.added"));

        // 应有两个 output_text.delta
        let deltas: Vec<&Value> = events
            .iter()
            .filter(|(t, _)| t == "response.output_text.delta")
            .map(|(_, v)| v)
            .collect();
        assert_eq!(deltas.len(), 2);
        assert_eq!(deltas[0]["delta"], "Hello");
        assert_eq!(deltas[1]["delta"], " world");

        assert!(types.contains(&"response.output_text.done"));
        assert!(types.contains(&"response.content_part.done"));
        assert!(types.contains(&"response.output_item.done"));
        assert!(types.contains(&"response.completed"));

        // sequence_number 递增
        for (i, (_, ev)) in events.iter().enumerate() {
            assert_eq!(ev["sequence_number"].as_u64().unwrap(), i as u64);
        }
    }

    // ─── reasoning → text 流 ───────────────────────────────────

    #[test]
    fn test_reasoning_then_text_stream() {
        let chunks = vec![
            json!({
                "model": "deepseek-reasoner",
                "created": 1700000000,
                "choices": [{"index": 0, "delta": {"reasoning_content": "Let me think"}}]
            }),
            json!({
                "choices": [{"index": 0, "delta": {"reasoning_content": "..."}}]
            }),
            json!({
                "choices": [{"index": 0, "delta": {"content": "The answer"}}]
            }),
            json!({
                "choices": [{"index": 0, "delta": {"content": " is 42."}}]
            }),
            json!({
                "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 10, "completion_tokens": 20, "total_tokens": 30}
            }),
        ];

        let events = feed_chunks(&chunks);
        let types = event_types(&events);

        // 应有 reasoning 相关事件
        assert!(types.contains(&"response.reasoning_summary_part.added"));
        assert!(types.contains(&"response.reasoning_summary_text.delta"));
        assert!(types.contains(&"response.reasoning_summary_text.done"));
        assert!(types.contains(&"response.reasoning_summary_part.done"));

        // reasoning item 应在 message item 之前关闭
        let reasoning_done_idx = types
            .iter()
            .position(|&t| t == "response.reasoning_summary_text.done")
            .unwrap();
        let message_added_idx = types
            .iter()
            .rposition(|&t| t == "response.output_item.added")
            .unwrap();
        assert!(reasoning_done_idx < message_added_idx);

        // text content 事件
        let text_deltas: Vec<&str> = events
            .iter()
            .filter(|(t, _)| t == "response.output_text.delta")
            .map(|(_, v)| v["delta"].as_str().unwrap())
            .collect();
        assert_eq!(text_deltas, vec!["The answer", " is 42."]);

        assert!(types.contains(&"response.completed"));
    }

    // ─── tool call 流 ──────────────────────────────────────────

    #[test]
    fn test_tool_call_stream() {
        let chunks = vec![
            json!({
                "model": "deepseek-chat",
                "created": 1700000000,
                "choices": [{"index": 0, "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_abc",
                        "type": "function",
                        "function": {"name": "get_weather", "arguments": ""}
                    }]
                }}]
            }),
            json!({
                "choices": [{"index": 0, "delta": {
                    "tool_calls": [{"index": 0, "function": {"arguments": "{\"city\""}}]
                }}]
            }),
            json!({
                "choices": [{"index": 0, "delta": {
                    "tool_calls": [{"index": 0, "function": {"arguments": ":\"NYC\"}"}}]
                }}]
            }),
            json!({
                "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}],
                "usage": {"prompt_tokens": 10, "completion_tokens": 15, "total_tokens": 25}
            }),
        ];

        let events = feed_chunks(&chunks);
        let types = event_types(&events);

        // function_call item added
        let fc_added: Vec<&Value> = events
            .iter()
            .filter(|(t, _)| t == "response.output_item.added")
            .map(|(_, v)| v)
            .collect();
        assert_eq!(fc_added.len(), 1);
        assert_eq!(fc_added[0]["item"]["type"], "function_call");
        assert_eq!(fc_added[0]["item"]["name"], "get_weather");

        // arguments deltas
        let arg_deltas: Vec<&str> = events
            .iter()
            .filter(|(t, _)| t == "response.function_call_arguments.delta")
            .map(|(_, v)| v["delta"].as_str().unwrap())
            .collect();
        assert_eq!(arg_deltas, vec!["{\"city\"", ":\"NYC\"}"]);

        // arguments done
        let arg_done: Vec<&Value> = events
            .iter()
            .filter(|(t, _)| t == "response.function_call_arguments.done")
            .map(|(_, v)| v)
            .collect();
        assert_eq!(arg_done.len(), 1);
        assert_eq!(arg_done[0]["arguments"], "{\"city\":\"NYC\"}");

        assert!(types.contains(&"response.completed"));
    }

    // ─── usage 分离的 chunk ────────────────────────────────────

    #[test]
    fn test_usage_in_separate_chunk() {
        let chunks = vec![
            json!({
                "model": "deepseek-chat",
                "created": 1700000000,
                "choices": [{"index": 0, "delta": {"content": "hi"}}]
            }),
            json!({
                "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]
            }),
            // usage 在单独的 chunk 里
            json!({
                "usage": {"prompt_tokens": 10, "completion_tokens": 1, "total_tokens": 11}
            }),
        ];

        let events = feed_chunks(&chunks);
        let types = event_types(&events);
        assert!(types.contains(&"response.completed"));
    }

    // ─── finish_reason=length → incomplete ─────────────────────

    #[test]
    fn test_length_finish_reason_incomplete() {
        let chunks = vec![
            json!({
                "model": "deepseek-chat",
                "created": 1700000000,
                "choices": [{"index": 0, "delta": {"content": "truncated"}}]
            }),
            json!({
                "choices": [{"index": 0, "delta": {}, "finish_reason": "length"}],
                "usage": {"prompt_tokens": 10, "completion_tokens": 4096, "total_tokens": 4106}
            }),
        ];

        let events = feed_chunks(&chunks);
        let incomplete: Vec<&Value> = events
            .iter()
            .filter(|(t, _)| t == "response.incomplete")
            .map(|(_, v)| v)
            .collect();
        assert_eq!(incomplete.len(), 1);
        assert_eq!(incomplete[0]["response"]["status"], "incomplete");
    }

    // ─── 空流 ──────────────────────────────────────────────────

    #[test]
    fn test_empty_stream() {
        let events = feed_chunks(&[]);
        // handle_done 在 has_started=false 时不输出任何事件
        assert!(events.is_empty());
    }

    // ─── 多工具并行流式 ────────────────────────────────────────

    #[test]
    fn test_parallel_tool_calls_stream() {
        let chunks = vec![
            json!({
                "model": "deepseek-chat",
                "created": 1700000000,
                "choices": [{"index": 0, "delta": {
                    "tool_calls": [
                        {"index": 0, "id": "call_1", "type": "function", "function": {"name": "get_weather", "arguments": ""}},
                        {"index": 1, "id": "call_2", "type": "function", "function": {"name": "get_time", "arguments": ""}}
                    ]
                }}]
            }),
            json!({
                "choices": [{"index": 0, "delta": {
                    "tool_calls": [
                        {"index": 0, "function": {"arguments": "{\"city\""}},
                        {"index": 1, "function": {"arguments": "{\"tz\""}}
                    ]
                }}]
            }),
            json!({
                "choices": [{"index": 0, "delta": {
                    "tool_calls": [
                        {"index": 0, "function": {"arguments": ":\"NYC\"}"}},
                        {"index": 1, "function": {"arguments": ":\"EST\"}"}}
                    ]
                }}]
            }),
            json!({
                "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}],
                "usage": {"prompt_tokens": 20, "completion_tokens": 15, "total_tokens": 35}
            }),
        ];

        let events = feed_chunks(&chunks);

        // 应有 2 个 output_item.added（两个 function_call）
        let items_added: Vec<&Value> = events
            .iter()
            .filter(|(t, _)| t == "response.output_item.added")
            .map(|(_, v)| v)
            .collect();
        assert_eq!(items_added.len(), 2);
        assert_eq!(items_added[0]["item"]["name"], "get_weather");
        assert_eq!(items_added[1]["item"]["name"], "get_time");

        // 应有 4 个 arguments delta（每个工具 2 次）
        let arg_deltas: Vec<&Value> = events
            .iter()
            .filter(|(t, _)| t == "response.function_call_arguments.delta")
            .map(|(_, v)| v)
            .collect();
        assert_eq!(arg_deltas.len(), 4);

        // 应有 2 个 arguments done，参数完整
        let arg_dones: Vec<&Value> = events
            .iter()
            .filter(|(t, _)| t == "response.function_call_arguments.done")
            .map(|(_, v)| v)
            .collect();
        assert_eq!(arg_dones.len(), 2);
        // 验证拼接完整（顺序可能因 HashMap 迭代而不确定，所以收集后排序检查）
        let mut final_args: Vec<&str> = arg_dones
            .iter()
            .map(|v| v["arguments"].as_str().unwrap())
            .collect();
        final_args.sort();
        assert_eq!(final_args, vec![r#"{"city":"NYC"}"#, r#"{"tz":"EST"}"#]);

        assert!(event_types(&events).contains(&"response.completed"));
    }

    // ─── reasoning → tool call 流式 ────────────────────────────

    #[test]
    fn test_reasoning_then_tool_call_stream() {
        let chunks = vec![
            json!({
                "model": "deepseek-reasoner",
                "created": 1700000000,
                "choices": [{"index": 0, "delta": {"reasoning_content": "I should call a tool"}}]
            }),
            json!({
                "choices": [{"index": 0, "delta": {
                    "tool_calls": [{"index": 0, "id": "call_1", "type": "function",
                        "function": {"name": "search", "arguments": ""}}]
                }}]
            }),
            json!({
                "choices": [{"index": 0, "delta": {
                    "tool_calls": [{"index": 0, "function": {"arguments": "{\"q\":\"rust\"}"}}]
                }}]
            }),
            json!({
                "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}],
                "usage": {"prompt_tokens": 10, "completion_tokens": 20, "total_tokens": 30}
            }),
        ];

        let events = feed_chunks(&chunks);
        let types = event_types(&events);

        // reasoning 事件应该在 tool call 之前关闭
        assert!(types.contains(&"response.reasoning_summary_text.delta"));
        assert!(types.contains(&"response.reasoning_summary_text.done"));

        let reasoning_done_pos = types
            .iter()
            .position(|&t| t == "response.reasoning_summary_text.done")
            .unwrap();
        let fc_added_pos = types
            .iter()
            .rposition(|&t| t == "response.output_item.added")
            .unwrap();
        assert!(reasoning_done_pos < fc_added_pos);

        // tool call 事件
        assert!(types.contains(&"response.function_call_arguments.delta"));
        assert!(types.contains(&"response.function_call_arguments.done"));
        assert!(types.contains(&"response.completed"));
    }

    // ─── text → tool call 流式（先输出文本再调用工具）──────────

    #[test]
    fn test_text_then_tool_call_stream() {
        let chunks = vec![
            json!({
                "model": "deepseek-chat",
                "created": 1700000000,
                "choices": [{"index": 0, "delta": {"content": "Let me check"}}]
            }),
            json!({
                "choices": [{"index": 0, "delta": {
                    "tool_calls": [{"index": 0, "id": "call_1", "type": "function",
                        "function": {"name": "lookup", "arguments": "{}"}}]
                }}]
            }),
            json!({
                "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}],
                "usage": {"prompt_tokens": 5, "completion_tokens": 10, "total_tokens": 15}
            }),
        ];

        let events = feed_chunks(&chunks);
        let types = event_types(&events);

        // 应先关闭 message item，再开 function_call item
        assert!(types.contains(&"response.output_text.delta"));
        assert!(types.contains(&"response.output_text.done"));
        assert!(types.contains(&"response.output_item.done"));

        // message 的 output_item.done 应在 function_call 的 output_item.added 之前
        let msg_done_pos = types
            .iter()
            .position(|&t| t == "response.output_item.done")
            .unwrap();
        let fc_added_pos = types
            .iter()
            .rposition(|&t| t == "response.output_item.added")
            .unwrap();
        assert!(msg_done_pos < fc_added_pos);

        assert!(types.contains(&"response.function_call_arguments.done"));
        assert!(types.contains(&"response.completed"));
    }

    // ─── sequence_number 全局递增 ──────────────────────────────

    #[test]
    fn test_sequence_numbers_monotonic() {
        let chunks = vec![
            json!({
                "model": "test",
                "created": 1700000000,
                "choices": [{"index": 0, "delta": {"reasoning_content": "think"}}]
            }),
            json!({
                "choices": [{"index": 0, "delta": {"content": "answer"}}]
            }),
            json!({
                "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
            }),
        ];

        let events = feed_chunks(&chunks);
        let seq_nums: Vec<u64> = events
            .iter()
            .map(|(_, v)| v["sequence_number"].as_u64().unwrap())
            .collect();
        // 严格递增
        for i in 1..seq_nums.len() {
            assert!(
                seq_nums[i] == seq_nums[i - 1] + 1,
                "sequence_number not monotonic at index {}: {} vs {}",
                i,
                seq_nums[i],
                seq_nums[i - 1]
            );
        }
    }

    // ─── 纯 usage chunk（无 choices）────────────────────────────

    #[test]
    fn test_standalone_usage_chunk() {
        let chunks = vec![
            json!({
                "model": "deepseek-chat",
                "created": 1700000000,
                "choices": [{"index": 0, "delta": {"content": "hi"}}]
            }),
            json!({
                "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]
            }),
            // 独立 usage chunk，无 choices 字段
            json!({
                "usage": {"prompt_tokens": 100, "completion_tokens": 50, "total_tokens": 150}
            }),
        ];

        let events = feed_chunks(&chunks);
        let types = event_types(&events);
        assert!(types.contains(&"response.completed"));

        // completed 事件里应包含 usage
        let completed = events
            .iter()
            .find(|(t, _)| t == "response.completed")
            .unwrap();
        assert_eq!(completed.1["response"]["usage"]["input_tokens"], 100);
        assert_eq!(completed.1["response"]["usage"]["output_tokens"], 50);
    }
}
