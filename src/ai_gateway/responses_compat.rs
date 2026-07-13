use axum::body::Bytes;
use futures_util::Stream;
use serde_json::Value;
use std::{
    collections::VecDeque,
    pin::Pin,
    task::{Context, Poll},
};

use super::encrypted_content::{EncryptedContentScope, encode_response_object};
use super::tool_names::{ToolCallKind, ToolNameMap};

/// Applies narrow, idempotent compatibility rules to a Responses payload while
/// preserving fields that CodexHub does not know about yet.
#[cfg(test)]
pub(crate) fn normalize_response_value(value: &mut Value) -> bool {
    normalize_response_value_with_scope(value, None)
}

#[cfg(test)]
pub(crate) fn normalize_response_value_with_scope(
    value: &mut Value,
    encrypted_content_scope: Option<&EncryptedContentScope>,
) -> bool {
    normalize_response_value_with_scope_and_tool_names(value, encrypted_content_scope, None)
}

pub(crate) fn normalize_response_value_with_scope_and_tool_names(
    value: &mut Value,
    encrypted_content_scope: Option<&EncryptedContentScope>,
    tool_names: Option<&ToolNameMap>,
) -> bool {
    match value {
        Value::Object(object) => {
            let mut changed =
                encrypted_content_scope.is_some_and(|scope| encode_response_object(object, scope));
            changed |=
                tool_names.is_some_and(|tool_names| restore_provider_tool_call(object, tool_names));
            let duplicate_exec_namespace = object
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|item_type| item_type == "custom_tool_call")
                && object
                    .get("name")
                    .and_then(Value::as_str)
                    .zip(object.get("namespace").and_then(Value::as_str))
                    .is_some_and(|(name, namespace)| {
                        name.trim() == "exec" && namespace.trim() == "exec"
                    });

            if duplicate_exec_namespace {
                object.remove("namespace");
                changed = true;
            }

            for child in object.values_mut() {
                changed |= normalize_response_value_with_scope_and_tool_names(
                    child,
                    encrypted_content_scope,
                    tool_names,
                );
            }
            changed
        }
        Value::Array(items) => {
            let mut changed = false;
            for item in items {
                changed |= normalize_response_value_with_scope_and_tool_names(
                    item,
                    encrypted_content_scope,
                    tool_names,
                );
            }
            changed
        }
        _ => false,
    }
}

/// Normalizes a complete Responses JSON payload. Unchanged and invalid payloads
/// retain their original bytes so whitespace and unknown wire details survive.
#[cfg(test)]
pub(crate) fn normalize_json_body(body_bytes: Bytes) -> (Bytes, Option<Value>) {
    normalize_json_body_with_scope(body_bytes, None)
}

#[cfg(test)]
pub(crate) fn normalize_json_body_with_scope(
    body_bytes: Bytes,
    encrypted_content_scope: Option<&EncryptedContentScope>,
) -> (Bytes, Option<Value>) {
    normalize_json_body_with_scope_and_tool_names(body_bytes, encrypted_content_scope, None)
}

pub(crate) fn normalize_json_body_with_scope_and_tool_names(
    body_bytes: Bytes,
    encrypted_content_scope: Option<&EncryptedContentScope>,
    tool_names: Option<&ToolNameMap>,
) -> (Bytes, Option<Value>) {
    let mut response_json = serde_json::from_slice::<Value>(&body_bytes).ok();
    let Some(value) = response_json.as_mut() else {
        return (body_bytes, response_json);
    };
    if normalize_response_value_with_scope_and_tool_names(
        value,
        encrypted_content_scope,
        tool_names,
    ) {
        let rewritten = serde_json::to_vec(value)
            .map(Bytes::from)
            .unwrap_or_else(|_| body_bytes.clone());
        (rewritten, response_json)
    } else {
        (body_bytes, response_json)
    }
}

/// Rewrites Responses SSE data lines with the same rules used for complete JSON
/// responses. The stream buffers raw bytes so UTF-8 characters may safely span
/// upstream network chunks.
pub(crate) struct ResponsesCompatSseStream<S> {
    inner: S,
    encrypted_content_scope: Option<EncryptedContentScope>,
    tool_names: Option<ToolNameMap>,
    line_buf: Vec<u8>,
    output_queue: VecDeque<Result<Bytes, std::io::Error>>,
    ended: bool,
}

impl<S> ResponsesCompatSseStream<S> {
    #[cfg(test)]
    pub(crate) fn new(inner: S) -> Self {
        Self::with_optional_encrypted_content_scope(inner, None)
    }

    pub(crate) fn with_encrypted_content_scope(
        inner: S,
        encrypted_content_scope: EncryptedContentScope,
    ) -> Self {
        Self::with_optional_compatibility(inner, Some(encrypted_content_scope), None)
    }

    pub(crate) fn with_compatibility(
        inner: S,
        encrypted_content_scope: EncryptedContentScope,
        tool_names: Option<ToolNameMap>,
    ) -> Self {
        Self::with_optional_compatibility(inner, Some(encrypted_content_scope), tool_names)
    }

    #[cfg(test)]
    fn with_optional_encrypted_content_scope(
        inner: S,
        encrypted_content_scope: Option<EncryptedContentScope>,
    ) -> Self {
        Self::with_optional_compatibility(inner, encrypted_content_scope, None)
    }

    fn with_optional_compatibility(
        inner: S,
        encrypted_content_scope: Option<EncryptedContentScope>,
        tool_names: Option<ToolNameMap>,
    ) -> Self {
        Self {
            inner,
            encrypted_content_scope,
            tool_names,
            line_buf: Vec::new(),
            output_queue: VecDeque::new(),
            ended: false,
        }
    }
}

impl<S> Stream for ResponsesCompatSseStream<S>
where
    S: Stream<Item = Result<Bytes, std::io::Error>> + Unpin,
{
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            if let Some(item) = this.output_queue.pop_front() {
                return Poll::Ready(Some(item));
            }

            if this.ended {
                return Poll::Ready(None);
            }

            match Pin::new(&mut this.inner).poll_next(cx) {
                Poll::Ready(Some(Ok(chunk))) => this.push_rewritten_chunk(&chunk),
                Poll::Ready(Some(Err(err))) => return Poll::Ready(Some(Err(err))),
                Poll::Ready(None) => {
                    this.ended = true;
                    if !this.line_buf.is_empty() {
                        let mut line = std::mem::take(&mut this.line_buf);
                        if line.last() == Some(&b'\r') {
                            line.pop();
                        }
                        this.push_rewritten_line(&line);
                    }
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl<S> ResponsesCompatSseStream<S> {
    fn push_rewritten_chunk(&mut self, chunk: &Bytes) {
        self.line_buf.extend_from_slice(chunk);
        while let Some(pos) = self.line_buf.iter().position(|byte| *byte == b'\n') {
            let mut line = self.line_buf.drain(..=pos).collect::<Vec<_>>();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            self.push_rewritten_line(&line);
        }
    }

    fn push_rewritten_line(&mut self, line: &[u8]) {
        let rewritten = rewrite_sse_line(
            line,
            self.encrypted_content_scope.as_ref(),
            self.tool_names.as_ref(),
        );
        let mut output = Vec::with_capacity(rewritten.len() + 1);
        output.extend_from_slice(&rewritten);
        output.push(b'\n');
        self.output_queue.push_back(Ok(Bytes::from(output)));
    }
}

fn rewrite_sse_line(
    line: &[u8],
    encrypted_content_scope: Option<&EncryptedContentScope>,
    tool_names: Option<&ToolNameMap>,
) -> Bytes {
    let Ok(line_text) = std::str::from_utf8(line) else {
        return Bytes::copy_from_slice(line);
    };
    let Some(data) = sse_data_value(line_text) else {
        return Bytes::copy_from_slice(line);
    };
    if data.trim() == "[DONE]" {
        return Bytes::copy_from_slice(line);
    }
    let Ok(mut event) = serde_json::from_str::<Value>(data) else {
        return Bytes::copy_from_slice(line);
    };
    if !normalize_response_value_with_scope_and_tool_names(
        &mut event,
        encrypted_content_scope,
        tool_names,
    ) {
        return Bytes::copy_from_slice(line);
    }
    serde_json::to_vec(&event)
        .map(|json| {
            let mut rewritten = Vec::with_capacity(json.len() + 6);
            rewritten.extend_from_slice(b"data: ");
            rewritten.extend_from_slice(&json);
            Bytes::from(rewritten)
        })
        .unwrap_or_else(|_| Bytes::copy_from_slice(line))
}

fn restore_provider_tool_call(
    object: &mut serde_json::Map<String, Value>,
    tool_names: &ToolNameMap,
) -> bool {
    if object.get("type").and_then(Value::as_str) != Some("function_call") {
        return false;
    }
    let Some(encoded_name) = object
        .get("name")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return false;
    };
    if !tool_names.has_encoded(&encoded_name) {
        return false;
    }

    let target = tool_names.decode(&encoded_name);
    let mut changed = set_string_field(object, "name", &target.name);
    match target.namespace.as_deref() {
        Some(namespace) => changed |= set_string_field(object, "namespace", namespace),
        None => changed |= object.remove("namespace").is_some(),
    }

    if target.kind == ToolCallKind::Custom {
        changed |= set_string_field(object, "type", "custom_tool_call");
        if let Some(arguments) = object.remove("arguments") {
            object.insert(
                "input".to_string(),
                Value::String(custom_input_from_arguments(arguments)),
            );
            changed = true;
        } else {
            object
                .entry("input".to_string())
                .or_insert_with(|| Value::String(String::new()));
        }
    }
    changed
}

fn set_string_field(object: &mut serde_json::Map<String, Value>, key: &str, value: &str) -> bool {
    if object.get(key).and_then(Value::as_str) == Some(value) {
        return false;
    }
    object.insert(key.to_string(), Value::String(value.to_string()));
    true
}

fn custom_input_from_arguments(arguments: Value) -> String {
    match arguments {
        Value::String(arguments) => serde_json::from_str::<Value>(&arguments)
            .ok()
            .and_then(custom_input_value)
            .unwrap_or(arguments),
        arguments => custom_input_value(arguments.clone())
            .unwrap_or_else(|| serde_json::to_string(&arguments).unwrap_or_default()),
    }
}

fn custom_input_value(arguments: Value) -> Option<String> {
    let input = arguments.as_object()?.get("input")?;
    Some(match input {
        Value::String(input) => input.clone(),
        input => serde_json::to_string(input).unwrap_or_default(),
    })
}

fn sse_data_value(line: &str) -> Option<&str> {
    let data = line.strip_prefix("data:")?;
    Some(data.strip_prefix(' ').unwrap_or(data))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_gateway::config::{ProviderConfig, ProviderType};
    use futures_util::{StreamExt, stream};
    use serde_json::json;

    fn grok_scope() -> EncryptedContentScope {
        EncryptedContentScope::for_provider(&ProviderConfig {
            name: "grok".to_string(),
            provider_type: ProviderType::GrokResponses,
            base_url: "https://api.x.ai/v1".to_string(),
            ..ProviderConfig::default()
        })
    }

    #[test]
    fn removes_namespace_only_from_exec_custom_tool_calls() {
        let mut event = json!({
            "type": "response.output_item.done",
            "item": {
                "type": "custom_tool_call",
                "name": "exec",
                "namespace": "exec",
                "call_id": "call_1",
                "input": "text('ok')",
                "future_field": {"kept": true}
            },
            "response": {
                "output": [
                    {
                        "type": "custom_tool_call",
                        "name": "lookup",
                        "namespace": "lookup",
                        "call_id": "call_2",
                        "input": "payload"
                    },
                    {
                        "type": "function_call",
                        "name": "read_file",
                        "namespace": "fs"
                    }
                ],
                "future_response_field": [1, 2, 3]
            }
        });

        assert!(normalize_response_value(&mut event));
        assert!(event["item"].get("namespace").is_none());
        assert_eq!(event["item"]["future_field"]["kept"], true);
        assert_eq!(event["response"]["output"][0]["namespace"], "lookup");
        assert_eq!(event["response"]["output"][1]["namespace"], "fs");
        assert_eq!(event["response"]["future_response_field"], json!([1, 2, 3]));

        let normalized_once = event.clone();
        assert!(!normalize_response_value(&mut event));
        assert_eq!(event, normalized_once);
    }

    #[test]
    fn json_body_preserves_original_bytes_when_no_rule_matches() {
        let body = Bytes::from_static(
            br#"{ "output": [{"type":"custom_tool_call","name":"lookup","namespace":"lookup","opaque":7}] }"#,
        );

        let (normalized, parsed) = normalize_json_body(body.clone());

        assert_eq!(normalized, body);
        assert_eq!(parsed.expect("parsed response")["output"][0]["opaque"], 7);
    }

    #[test]
    fn invalid_json_body_is_unchanged() {
        let body = Bytes::from_static(b"{not-json");
        let (normalized, parsed) = normalize_json_body(body.clone());

        assert_eq!(normalized, body);
        assert!(parsed.is_none());
    }

    #[test]
    fn json_body_marks_provider_encrypted_content() {
        let body = Bytes::from_static(
            br#"{"output":[{"type":"reasoning","encrypted_content":"opaque-grok"}]}"#,
        );
        let scope = grok_scope();

        let (normalized, parsed) = normalize_json_body_with_scope(body, Some(&scope));

        let parsed = parsed.expect("parsed response");
        let encrypted = parsed["output"][0]["encrypted_content"]
            .as_str()
            .expect("encrypted content");
        assert!(encrypted.starts_with("codexhub:enc:v1:grok:"));
        assert!(encrypted.ends_with(":opaque-grok"));
        assert_eq!(
            serde_json::from_slice::<Value>(&normalized).unwrap(),
            parsed
        );
    }

    #[test]
    fn json_body_restores_grok_custom_and_namespace_tool_calls() {
        let mut tool_names = ToolNameMap::default();
        let exec_name = tool_names.encode_custom("exec");
        let browser_name = tool_names.encode_function(Some("browser"), "open");
        let body = Bytes::from(
            json!({
                "output": [
                    {
                        "type": "function_call",
                        "call_id": "call_exec",
                        "name": exec_name,
                        "arguments": "{\"input\":\"Get-ChildItem\"}"
                    },
                    {
                        "type": "function_call",
                        "call_id": "call_open",
                        "name": browser_name,
                        "arguments": "{\"url\":\"https://example.com\"}"
                    }
                ]
            })
            .to_string(),
        );

        let (_, parsed) = normalize_json_body_with_scope_and_tool_names(
            body,
            Some(&grok_scope()),
            Some(&tool_names),
        );
        let parsed = parsed.unwrap();

        assert_eq!(parsed["output"][0]["type"], "custom_tool_call");
        assert_eq!(parsed["output"][0]["name"], "exec");
        assert_eq!(parsed["output"][0]["input"], "Get-ChildItem");
        assert!(parsed["output"][0].get("arguments").is_none());
        assert_eq!(parsed["output"][1]["type"], "function_call");
        assert_eq!(parsed["output"][1]["name"], "open");
        assert_eq!(parsed["output"][1]["namespace"], "browser");
    }

    #[tokio::test]
    async fn stream_normalizes_exec_namespace_and_keeps_other_namespaces() {
        let chunks = stream::iter(vec![
            Ok::<_, std::io::Error>(Bytes::from(
                "event: response.output_item.done\n\
                 data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"custom_tool_call\",\"name\":\"exec\",\"namespace\":\"exec\",\"call_id\":\"call_1\",\"future_field\":true}}\n\n",
            )),
            Ok(Bytes::from(
                "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"name\":\"read_file\",\"namespace\":\"fs\"}}\n\n",
            )),
        ]);
        let output = ResponsesCompatSseStream::new(Box::pin(chunks))
            .collect::<Vec<Result<Bytes, std::io::Error>>>()
            .await;
        let text = output
            .into_iter()
            .map(|item| String::from_utf8(item.expect("chunk").to_vec()).expect("utf8"))
            .collect::<String>();

        assert!(text.contains("\"type\":\"custom_tool_call\""));
        assert!(text.contains("\"name\":\"exec\""));
        assert!(!text.contains("\"namespace\":\"exec\""));
        assert!(text.contains("\"future_field\":true"));
        assert!(text.contains("\"namespace\":\"fs\""));
    }

    #[tokio::test]
    async fn stream_preserves_utf8_split_across_chunks() {
        let payload = format!(
            "data: {}\n\n",
            json!({
                "type": "response.output_text.delta",
                "delta": "你好"
            })
        );
        let split_at = payload.find('你').expect("unicode payload") + 1;
        let bytes = payload.as_bytes();
        let chunks = stream::iter(vec![
            Ok::<_, std::io::Error>(Bytes::copy_from_slice(&bytes[..split_at])),
            Ok(Bytes::copy_from_slice(&bytes[split_at..])),
        ]);

        let output = ResponsesCompatSseStream::new(Box::pin(chunks))
            .collect::<Vec<Result<Bytes, std::io::Error>>>()
            .await
            .into_iter()
            .map(|item| item.expect("chunk"))
            .fold(Vec::new(), |mut output, chunk| {
                output.extend_from_slice(&chunk);
                output
            });

        assert_eq!(String::from_utf8(output).expect("utf8"), payload);
    }

    #[tokio::test]
    async fn stream_marks_reasoning_encrypted_content() {
        let chunks = stream::iter(vec![Ok::<_, std::io::Error>(Bytes::from(
            "event: response.output_item.done\n\
             data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"reasoning\",\"encrypted_content\":\"opaque-grok\"}}\n\n",
        ))]);

        let output =
            ResponsesCompatSseStream::with_encrypted_content_scope(Box::pin(chunks), grok_scope())
                .collect::<Vec<Result<Bytes, std::io::Error>>>()
                .await
                .into_iter()
                .map(|item| String::from_utf8(item.expect("chunk").to_vec()).expect("utf8"))
                .collect::<String>();

        assert!(output.contains("codexhub:enc:v1:grok:"));
        assert!(output.contains(":opaque-grok"));
    }

    #[tokio::test]
    async fn stream_restores_grok_custom_tool_items() {
        let mut tool_names = ToolNameMap::default();
        let exec_name = tool_names.encode_custom("exec");
        let chunks = stream::iter(vec![Ok::<_, std::io::Error>(Bytes::from(format!(
            "data: {}\n\ndata: {}\n\n",
            json!({
                "type": "response.output_item.added",
                "item": {
                    "type": "function_call",
                    "id": "fc_1",
                    "call_id": "call_1",
                    "name": exec_name,
                    "arguments": ""
                }
            }),
            json!({
                "type": "response.output_item.done",
                "item": {
                    "type": "function_call",
                    "id": "fc_1",
                    "call_id": "call_1",
                    "name": exec_name,
                    "arguments": "{\"input\":\"pwd\"}"
                }
            })
        )))]);

        let output = ResponsesCompatSseStream::with_compatibility(
            Box::pin(chunks),
            grok_scope(),
            Some(tool_names),
        )
        .collect::<Vec<Result<Bytes, std::io::Error>>>()
        .await
        .into_iter()
        .map(|item| String::from_utf8(item.unwrap().to_vec()).unwrap())
        .collect::<String>();

        assert!(output.contains("\"type\":\"custom_tool_call\""));
        assert!(output.contains("\"name\":\"exec\""));
        assert!(output.contains("\"input\":\"pwd\""));
    }
}
