use std::collections::VecDeque;

use axum::body::Bytes;
use serde_json::{Value, json};

use super::citations;
use super::glm_compat;
use super::options::AnthropicProviderProfile;
use super::stream_events::emit_sse;
use super::stream_state::AnthropicStreamState;
use crate::ai_gateway::model::generate_item_id;

pub(super) struct StreamMessageItem {
    pub(super) item_id: String,
    pub(super) output_index: usize,
    pub(super) text: String,
    pub(super) content_part_started: bool,
    pub(super) annotations: Vec<StreamAnnotation>,
}

pub(super) struct StreamAnnotation {
    pub(super) value: Value,
    pending_end_index: bool,
}

impl AnthropicStreamState {
    pub(super) fn ensure_message_item(&mut self, queue: &mut VecDeque<Bytes>) {
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
            annotations: Vec::new(),
        });
    }

    pub(super) fn handle_text_delta(&mut self, text: &str, queue: &mut VecDeque<Bytes>) {
        if text.is_empty() {
            return;
        }
        if matches!(self.profile, AnthropicProviderProfile::GlmAnthropic) {
            self.glm_pending_text
                .get_or_insert_with(String::new)
                .push_str(text);
            return;
        }
        self.emit_message_text(text, queue);
    }

    pub(super) fn flush_glm_pending_text(&mut self, queue: &mut VecDeque<Bytes>) {
        let Some(text) = self.glm_pending_text.take() else {
            return;
        };
        let Some(cleaned) = glm_compat::clean_private_web_search_text(&text) else {
            return;
        };
        self.emit_message_text(&cleaned, queue);
        self.close_message_item(queue);
    }

    fn emit_message_text(&mut self, text: &str, queue: &mut VecDeque<Bytes>) {
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
            let current_end_index = item.text.chars().count();
            for annotation in &mut item.annotations {
                if annotation.pending_end_index {
                    annotation.value["end_index"] = json!(current_end_index);
                }
            }
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

    pub(super) fn handle_citation_delta(&mut self, citation: &Value, queue: &mut VecDeque<Bytes>) {
        let start_index = self
            .message_item
            .as_ref()
            .map(|item| item.text.chars().count())
            .unwrap_or(0);
        let Some(annotation) =
            citations::convert_anthropic_citation_at(citation, start_index, start_index)
        else {
            return;
        };
        self.ensure_message_item(queue);

        let mut added_part = None;
        let mut event_payload = None;
        let seq_for_part = self.next_seq();
        let seq_for_annotation = self.next_seq();
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
            let annotation_index = item.annotations.len();
            item.annotations.push(StreamAnnotation {
                value: annotation.clone(),
                pending_end_index: true,
            });
            event_payload = Some(json!({
                "type": "response.output_text.annotation.added",
                "sequence_number": seq_for_annotation,
                "item_id": item.item_id,
                "output_index": item.output_index,
                "content_index": 0,
                "annotation_index": annotation_index,
                "annotation": annotation,
            }));
        }
        if let Some(data) = added_part {
            emit_sse(queue, "response.content_part.added", data);
        }
        if let Some(data) = event_payload {
            emit_sse(queue, "response.output_text.annotation.added", data);
        }
    }

    pub(super) fn close_message_item(&mut self, queue: &mut VecDeque<Bytes>) {
        let Some(item) = self.message_item.take() else {
            return;
        };
        let annotations = item
            .annotations
            .iter()
            .map(|annotation| annotation.value.clone())
            .collect::<Vec<_>>();
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
                    "part": {"type": "output_text", "text": item.text, "annotations": annotations.clone()},
                }),
            );
        }
        let output_item = json!({
            "type": "message",
            "id": item.item_id,
            "role": "assistant",
            "status": "completed",
            "content": [{"type": "output_text", "text": item.text, "annotations": annotations}],
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
}
