//! Anthropic Messages 出站 provider。

use axum::{
    body::{Body, Bytes},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::Response,
};
use futures_util::Stream;
use serde_json::{Map, Value, json};
use std::{
    collections::{HashMap, VecDeque},
    pin::Pin,
    task::{Context, Poll},
};
use tracing::{debug, error};

use crate::ai_gateway::config::{ProviderConfig, provider_api_root};
use crate::ai_gateway::context::{GatewayContext, apply_upstream_headers};
use crate::ai_gateway::error::GatewayError;
use crate::ai_gateway::model::{
    ContentPart, GatewayRequest, InputTokensDetails, ItemContent, ItemType, JsonString,
    OutputTokensDetails, ResponseItem, ResponseObject, Usage, generate_item_id,
    generate_response_id,
};
use crate::ai_gateway::request_log::{
    self, RequestLogContext, RequestLogUpdate, ResponsesSseLogStream,
};
use crate::ai_gateway::tool_names::{ToolCallKind, ToolCallTarget, ToolNameMap};

use super::{apply_total_request_timeout, execute_stream_start, map_upstream_response};

const ANTHROPIC_VERSION: &str = "2023-06-01";
const ANTHROPIC_WEB_SEARCH_TYPE: &str = "web_search_20260318";
const DEFAULT_MAX_TOKENS: i64 = 4096;

pub async fn handle(
    ctx: &GatewayContext,
    request: &GatewayRequest,
    provider: &ProviderConfig,
    log_context: Option<RequestLogContext>,
) -> Result<Response<Body>, GatewayError> {
    let (anthropic_body, tool_name_map) = build_anthropic_request(request)?;
    let url = format!("{}/v1/messages", provider_api_root(&provider.base_url));
    debug!(url = %url, stream = false, "proxying to anthropic messages");

    let client = reqwest::Client::new();
    let upstream_req = client
        .post(&url)
        .header("content-type", "application/json")
        .header("x-api-key", provider.api_key.clone())
        .header("anthropic-version", ANTHROPIC_VERSION)
        .json(&anthropic_body);
    let upstream_req = apply_upstream_headers(
        apply_total_request_timeout(upstream_req, provider.timeout_secs, request.stream),
        &ctx.upstream_headers,
    )
    .build()
    .map_err(|e| {
        error!(error = %e, "build anthropic upstream request failed");
        GatewayError::upstream(
            StatusCode::BAD_GATEWAY,
            format!("build upstream request: {e}"),
        )
    })?;

    if let Some(log_context) = &log_context {
        let update = RequestLogUpdate {
            upstream_request_headers_json: request_log::headers_to_json(upstream_req.headers()),
            upstream_request_json: serde_json::to_string(&anthropic_body).ok(),
            ..RequestLogUpdate::default()
        };
        if let Err(err) =
            request_log::update_record(&log_context.db_path, log_context.log_id, &update)
        {
            request_log::log_update_error(err);
        }
    }

    let upstream_resp = if request.stream {
        execute_stream_start(
            &client,
            upstream_req,
            provider.timeout_secs,
            "anthropic upstream request failed",
        )
        .await?
    } else {
        map_upstream_response(
            client.execute(upstream_req).await,
            "anthropic upstream request failed",
        )?
    };
    let upstream_status = upstream_resp.status();
    if !upstream_status.is_success() {
        let status =
            StatusCode::from_u16(upstream_status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
        let body_text = upstream_resp.text().await.unwrap_or_default();
        return Err(GatewayError::upstream(status, body_text));
    }

    if request.stream {
        return handle_stream(upstream_resp, &request.model, tool_name_map, log_context).await;
    }

    let anthropic_resp: Value = upstream_resp.json().await.map_err(|e| {
        GatewayError::upstream(StatusCode::BAD_GATEWAY, format!("parse upstream json: {e}"))
    })?;
    let response_obj = convert_anthropic_response(&anthropic_resp, &request.model, &tool_name_map);
    let body_bytes = serde_json::to_vec(&response_obj).unwrap_or_default();

    if let Some(log_context) = &log_context {
        let response_value = serde_json::to_value(&response_obj).unwrap_or_default();
        let update = RequestLogUpdate {
            status: Some(response_obj.status.clone()),
            usage: Some(request_log::usage_from_response_value(&response_value)),
            latency_ms: Some(request_log::elapsed_ms(log_context.started_at)),
            response_json: serde_json::to_string(&response_value).ok(),
            ..RequestLogUpdate::default()
        };
        if let Err(err) =
            request_log::update_record(&log_context.db_path, log_context.log_id, &update)
        {
            request_log::log_update_error(err);
        }
    }

    let mut response = Response::new(Body::from(body_bytes));
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        HeaderName::from_static("content-type"),
        HeaderValue::from_static("application/json"),
    );
    Ok(response)
}

async fn handle_stream(
    resp: reqwest::Response,
    model: &str,
    tool_name_map: ToolNameMap,
    log_context: Option<RequestLogContext>,
) -> Result<Response<Body>, GatewayError> {
    let sse_stream =
        AnthropicSseToResponsesSse::new(resp.bytes_stream(), model.to_string(), tool_name_map);
    let body = if let Some(log_context) = log_context {
        Body::from_stream(ResponsesSseLogStream::new(sse_stream, log_context))
    } else {
        Body::from_stream(sse_stream)
    };

    let mut headers = HeaderMap::new();
    headers.insert(
        HeaderName::from_static("content-type"),
        HeaderValue::from_static("text/event-stream"),
    );
    headers.insert(
        HeaderName::from_static("cache-control"),
        HeaderValue::from_static("no-cache"),
    );
    headers.insert(
        HeaderName::from_static("connection"),
        HeaderValue::from_static("keep-alive"),
    );

    let mut response = Response::new(body);
    *response.status_mut() = StatusCode::OK;
    *response.headers_mut() = headers;
    Ok(response)
}

fn build_anthropic_request(request: &GatewayRequest) -> Result<(Value, ToolNameMap), GatewayError> {
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

fn convert_anthropic_response(
    response: &Value,
    request_model: &str,
    tool_name_map: &ToolNameMap,
) -> ResponseObject {
    let output = response
        .get("content")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| anthropic_content_to_response_item(item, tool_name_map))
                .collect()
        })
        .unwrap_or_default();
    let usage = response.get("usage").map(convert_usage_value);
    let status = match response.get("stop_reason").and_then(Value::as_str) {
        Some("max_tokens") => "incomplete",
        _ => "completed",
    };

    ResponseObject {
        id: response
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(generate_response_id),
        object_type: "response".to_string(),
        model: response
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or(request_model)
            .to_string(),
        created_at: chrono_timestamp(),
        status: status.to_string(),
        output,
        usage,
        error: None,
    }
}

fn anthropic_content_to_response_item(
    item: &Value,
    tool_name_map: &ToolNameMap,
) -> Option<ResponseItem> {
    match item.get("type").and_then(Value::as_str)? {
        "text" => {
            let text = item.get("text").and_then(Value::as_str).unwrap_or("");
            if text.is_empty() {
                return None;
            }
            Some(ResponseItem {
                item_type: ItemType::Message,
                id: Some(generate_item_id()),
                role: Some("assistant".to_string()),
                content: Some(ItemContent::Parts(vec![ContentPart::output_text(text)])),
                text: None,
                name: None,
                namespace: None,
                call_id: None,
                arguments: None,
                input: None,
                output: None,
                status: Some("completed".to_string()),
                execution: None,
                tools: None,
                image_url: None,
                detail: None,
                action: None,
                summary: None,
                encrypted_content: None,
            })
        }
        "tool_use" => {
            let raw_name = item.get("name").and_then(Value::as_str).unwrap_or("");
            let target = tool_name_map.decode(raw_name);
            let input = item.get("input").cloned().unwrap_or_else(|| json!({}));
            let (item_type, name, namespace, arguments, custom_input, execution) = match target.kind
            {
                ToolCallKind::ToolSearch => (
                    ItemType::ToolSearchCall,
                    None,
                    None,
                    Some(JsonString::Value(input)),
                    None,
                    Some("client".to_string()),
                ),
                ToolCallKind::Custom => (
                    ItemType::CustomToolCall,
                    Some(target.name),
                    None,
                    None,
                    Some(extract_custom_tool_input(&input)),
                    None,
                ),
                ToolCallKind::Function => (
                    ItemType::FunctionCall,
                    Some(target.name),
                    target.namespace,
                    Some(JsonString::String(
                        serde_json::to_string(&input).unwrap_or_else(|_| "{}".to_string()),
                    )),
                    None,
                    None,
                ),
            };
            Some(ResponseItem {
                item_type,
                id: Some(generate_item_id()),
                role: None,
                content: None,
                text: None,
                name,
                namespace,
                call_id: item.get("id").and_then(Value::as_str).map(str::to_string),
                arguments,
                input: custom_input,
                output: None,
                status: Some("completed".to_string()),
                execution,
                tools: None,
                image_url: None,
                detail: None,
                action: None,
                summary: None,
                encrypted_content: None,
            })
        }
        "server_tool_use" => {
            let name = item.get("name").and_then(Value::as_str).unwrap_or("");
            if name != "web_search" {
                return None;
            }
            Some(ResponseItem {
                item_type: ItemType::WebSearchCall,
                id: item
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| Some(generate_item_id())),
                role: None,
                content: None,
                text: None,
                name: None,
                namespace: None,
                call_id: item.get("id").and_then(Value::as_str).map(str::to_string),
                arguments: None,
                input: None,
                output: None,
                status: Some("completed".to_string()),
                execution: None,
                tools: None,
                image_url: None,
                detail: None,
                action: Some(server_tool_action(item)),
                summary: None,
                encrypted_content: None,
            })
        }
        "web_search_tool_result" => Some(ResponseItem {
            item_type: ItemType::WebSearchCall,
            id: item
                .get("tool_use_id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| Some(generate_item_id())),
            role: None,
            content: None,
            text: None,
            name: None,
            namespace: None,
            call_id: item
                .get("tool_use_id")
                .and_then(Value::as_str)
                .map(str::to_string),
            arguments: None,
            input: None,
            output: None,
            status: Some(
                if item
                    .get("is_error")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    "failed"
                } else {
                    "completed"
                }
                .to_string(),
            ),
            execution: None,
            tools: None,
            image_url: None,
            detail: None,
            action: Some(json!({
                "type": "web_search_tool_result",
                "tool_use_id": item.get("tool_use_id").cloned().unwrap_or(Value::Null),
                "content": item.get("content").cloned().unwrap_or(Value::Null),
                "is_error": item.get("is_error").cloned().unwrap_or(Value::Bool(false)),
            })),
            summary: None,
            encrypted_content: None,
        }),
        _ => None,
    }
}

fn server_tool_action(item: &Value) -> Value {
    json!({
        "type": "web_search",
        "input": item.get("input").cloned().unwrap_or_else(|| json!({})),
    })
}

fn extract_custom_tool_input(input: &Value) -> String {
    input
        .get("input")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| {
            if input.is_string() {
                input.as_str().unwrap_or_default().to_string()
            } else {
                serde_json::to_string(input).unwrap_or_default()
            }
        })
}

struct AnthropicSseToResponsesSse<S> {
    inner: S,
    state: AnthropicStreamState,
    line_buf: String,
    event_name: Option<String>,
    data_lines: Vec<String>,
    output_queue: VecDeque<Bytes>,
}

impl<S> AnthropicSseToResponsesSse<S> {
    fn new(inner: S, model: String, tool_name_map: ToolNameMap) -> Self {
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

fn convert_usage_value(usage: &Value) -> Usage {
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
    Usage {
        input_tokens: input,
        output_tokens: output,
        total_tokens: input + output,
        input_tokens_details: Some(InputTokensDetails {
            cached_tokens: cached,
        }),
        output_tokens_details: Some(OutputTokensDetails {
            reasoning_tokens: 0,
        }),
    }
}

fn chrono_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_gateway::model::{FunctionCallOutput, FunctionCallOutputContentItem};
    use futures_util::{StreamExt, stream};

    fn request(input: Vec<ResponseItem>) -> GatewayRequest {
        GatewayRequest {
            model: "claude-sonnet-4-6".to_string(),
            instructions: Some("Be precise.".to_string()),
            input,
            tools: Vec::new(),
            tool_choice: None,
            reasoning: None,
            text: None,
            stream: false,
            max_output_tokens: Some(1234),
            temperature: Some(0.2),
            top_p: None,
            prompt_cache_key: None,
            prompt_cache_retention: None,
            previous_response_id: None,
        }
    }

    fn message(role: &str, text: &str) -> ResponseItem {
        ResponseItem {
            item_type: ItemType::Message,
            id: None,
            role: Some(role.to_string()),
            content: Some(ItemContent::Parts(vec![ContentPart {
                part_type: if role == "assistant" {
                    "output_text".to_string()
                } else {
                    "input_text".to_string()
                },
                text: Some(text.to_string()),
                image_url: None,
                detail: None,
                annotations: None,
            }])),
            text: None,
            name: None,
            namespace: None,
            call_id: None,
            arguments: None,
            input: None,
            output: None,
            status: None,
            execution: None,
            tools: None,
            image_url: None,
            detail: None,
            action: None,
            summary: None,
            encrypted_content: None,
        }
    }

    fn parse_events_from_bytes(chunks: &[Bytes]) -> Vec<(String, Value)> {
        let mut events = Vec::new();
        for chunk in chunks {
            let text = String::from_utf8_lossy(chunk);
            let mut event_type = String::new();
            let mut data = String::new();
            for line in text.lines() {
                if let Some(event) = line.strip_prefix("event: ") {
                    event_type = event.to_string();
                } else if let Some(value) = line.strip_prefix("data: ") {
                    data = value.to_string();
                }
            }
            if !event_type.is_empty() && !data.is_empty() {
                events.push((event_type, serde_json::from_str(&data).unwrap()));
            }
        }
        events
    }

    #[test]
    fn builds_anthropic_text_request() {
        let (body, _) = build_anthropic_request(&request(vec![
            message("user", "hello"),
            message("assistant", "hi"),
            message("user", "continue"),
        ]))
        .unwrap();

        assert_eq!(body["model"], "claude-sonnet-4-6");
        assert_eq!(body["max_tokens"], 1234);
        assert_eq!(body["system"], "Be precise.");
        assert_eq!(body["temperature"], 0.2);
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][0]["content"][0]["text"], "hello");
        assert_eq!(body["messages"][1]["role"], "assistant");
        assert_eq!(body["messages"][1]["content"][0]["text"], "hi");
    }

    #[test]
    fn builds_anthropic_tool_result_message() {
        let mut output = message("user", "ignored");
        output.item_type = ItemType::FunctionCallOutput;
        output.content = None;
        output.call_id = Some("toolu_123".to_string());
        output.output = Some(FunctionCallOutput::ContentItems(vec![
            FunctionCallOutputContentItem {
                item_type: "output_text".to_string(),
                text: Some("done".to_string()),
                image_url: None,
                encrypted_content: None,
                detail: None,
            },
        ]));

        let (body, _) =
            build_anthropic_request(&request(vec![message("user", "run"), output])).unwrap();
        assert_eq!(body["messages"][1]["role"], "user");
        assert_eq!(body["messages"][1]["content"][0]["type"], "tool_result");
        assert_eq!(
            body["messages"][1]["content"][0]["tool_use_id"],
            "toolu_123"
        );
        assert_eq!(body["messages"][1]["content"][0]["content"], "done");
    }

    #[test]
    fn builds_anthropic_tools_and_tool_choice() {
        let mut req = request(vec![message("user", "search docs")]);
        req.tools = vec![json!({
            "type": "namespace",
            "name": "browser",
            "tools": [{
                "type": "function",
                "name": "open page",
                "description": "Open a URL",
                "parameters": {
                    "type": "object",
                    "properties": {"url": {"type": "string"}},
                    "required": ["url"]
                }
            }]
        })];
        req.tool_choice = Some(json!({
            "type": "function",
            "namespace": "browser",
            "name": "open page"
        }));

        let (body, map) = build_anthropic_request(&req).unwrap();
        assert_eq!(body["tools"][0]["name"], "browser__codexns__open_page");
        assert_eq!(body["tools"][0]["description"], "Open a URL");
        assert_eq!(body["tools"][0]["input_schema"]["required"][0], "url");
        assert_eq!(body["tool_choice"]["type"], "tool");
        assert_eq!(body["tool_choice"]["name"], "browser__codexns__open_page");

        let target = map.decode("browser__codexns__open_page");
        assert_eq!(target.namespace.as_deref(), Some("browser"));
        assert_eq!(target.name, "open page");
    }

    #[test]
    fn builds_anthropic_web_search_server_tool() {
        let mut req = request(vec![message("user", "latest rust news")]);
        req.tools = vec![json!({
            "type": "web_search_preview",
            "web_search": {
                "max_uses": 3,
                "allowed_domains": ["www.rust-lang.org"]
            }
        })];

        let (body, _) = build_anthropic_request(&req).unwrap();
        assert_eq!(body["tools"][0]["type"], ANTHROPIC_WEB_SEARCH_TYPE);
        assert_eq!(body["tools"][0]["name"], "web_search");
        assert_eq!(body["tools"][0]["max_uses"], 3);
        assert_eq!(body["tools"][0]["allowed_domains"][0], "www.rust-lang.org");
    }

    #[test]
    fn converts_anthropic_text_response() {
        let response = json!({
            "id": "msg_123",
            "type": "message",
            "role": "assistant",
            "model": "claude-sonnet-4-6",
            "content": [{"type": "text", "text": "hello back"}],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 10,
                "output_tokens": 3,
                "cache_read_input_tokens": 4
            }
        });

        let converted =
            convert_anthropic_response(&response, "fallback-model", &ToolNameMap::default());
        assert_eq!(converted.id, "msg_123");
        assert_eq!(converted.model, "claude-sonnet-4-6");
        assert_eq!(converted.status, "completed");
        assert_eq!(converted.output.len(), 1);
        let Some(ItemContent::Parts(parts)) = converted.output[0].content.as_ref() else {
            panic!("expected output_text content part");
        };
        assert_eq!(parts[0].part_type, "output_text");
        assert_eq!(parts[0].text.as_deref(), Some("hello back"));
        let usage = converted.usage.unwrap();
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 3);
        assert_eq!(usage.total_tokens, 13);
        assert_eq!(usage.input_tokens_details.unwrap().cached_tokens, 4);
    }

    #[test]
    fn converts_anthropic_tool_use_response() {
        let mut map = ToolNameMap::default();
        let encoded = map.encode_function(Some("browser"), "open page");
        let response = json!({
            "id": "msg_123",
            "type": "message",
            "role": "assistant",
            "model": "claude-sonnet-4-6",
            "content": [{
                "type": "tool_use",
                "id": "toolu_123",
                "name": encoded,
                "input": {"url": "https://example.com"}
            }],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 10, "output_tokens": 3}
        });

        let converted = convert_anthropic_response(&response, "fallback-model", &map);
        assert_eq!(converted.status, "completed");
        assert_eq!(converted.output.len(), 1);
        let item = &converted.output[0];
        assert_eq!(item.item_type, ItemType::FunctionCall);
        assert_eq!(item.namespace.as_deref(), Some("browser"));
        assert_eq!(item.name.as_deref(), Some("open page"));
        assert_eq!(item.call_id.as_deref(), Some("toolu_123"));
        assert_eq!(
            item.arguments.as_ref().unwrap().to_value()["url"],
            "https://example.com"
        );
    }

    #[test]
    fn converts_anthropic_server_web_search_response() {
        let response = json!({
            "id": "msg_123",
            "type": "message",
            "role": "assistant",
            "model": "claude-sonnet-4-6",
            "content": [
                {
                    "type": "server_tool_use",
                    "id": "srvtoolu_123",
                    "name": "web_search",
                    "input": {"query": "rust 2026"}
                },
                {
                    "type": "web_search_tool_result",
                    "tool_use_id": "srvtoolu_123",
                    "content": [{"type": "web_search_result", "title": "Rust", "url": "https://www.rust-lang.org"}]
                }
            ],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 10, "output_tokens": 3}
        });

        let converted =
            convert_anthropic_response(&response, "fallback-model", &ToolNameMap::default());
        assert_eq!(converted.output.len(), 2);
        assert_eq!(converted.output[0].item_type, ItemType::WebSearchCall);
        assert_eq!(converted.output[0].call_id.as_deref(), Some("srvtoolu_123"));
        assert_eq!(
            converted.output[0].action.as_ref().unwrap()["input"]["query"],
            "rust 2026"
        );
        assert_eq!(converted.output[1].item_type, ItemType::WebSearchCall);
        assert_eq!(
            converted.output[1].action.as_ref().unwrap()["content"][0]["url"],
            "https://www.rust-lang.org"
        );
    }

    #[tokio::test]
    async fn streams_anthropic_text_as_responses_sse() {
        let input = stream::iter(vec![
            Ok::<_, std::io::Error>(Bytes::from_static(
                b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-sonnet-4-6\",\"content\":[],\"usage\":{\"input_tokens\":2,\"output_tokens\":0}}}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hello\"}}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
            )),
        ]);

        let chunks = AnthropicSseToResponsesSse::new(
            input,
            "fallback-model".to_string(),
            ToolNameMap::default(),
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .map(Result::unwrap)
        .collect::<Vec<_>>();
        let events = parse_events_from_bytes(&chunks);

        assert!(events.iter().any(|(event, _)| event == "response.created"));
        assert!(
            events
                .iter()
                .any(|(event, data)| event == "response.output_text.delta"
                    && data["delta"] == "hello")
        );
        let completed = events
            .iter()
            .find(|(event, _)| event == "response.completed")
            .unwrap();
        assert_eq!(completed.1["response"]["id"], "msg_1");
        assert_eq!(completed.1["response"]["usage"]["input_tokens"], 2);
        assert_eq!(completed.1["response"]["usage"]["output_tokens"], 1);
    }

    #[tokio::test]
    async fn streams_anthropic_tool_use_as_responses_sse() {
        let mut map = ToolNameMap::default();
        let encoded = map.encode_function(Some("browser"), "open page");
        let start = format!(
            "event: content_block_start\ndata: {{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"{encoded}\",\"input\":{{}}}}}}\n\n"
        );
        let input = stream::iter(vec![
            Ok::<_, std::io::Error>(Bytes::from_static(
                b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-sonnet-4-6\",\"content\":[],\"usage\":{\"input_tokens\":2,\"output_tokens\":0}}}\n\n",
            )),
            Ok(Bytes::from(start)),
            Ok(Bytes::from_static(
                b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"url\\\":\"}}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"\\\"https://example.com\\\"}\"}}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
            )),
        ]);

        let chunks = AnthropicSseToResponsesSse::new(input, "fallback-model".to_string(), map)
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(Result::unwrap)
            .collect::<Vec<_>>();
        let events = parse_events_from_bytes(&chunks);

        assert!(events.iter().any(|(event, data)| event
            == "response.function_call_arguments.delta"
            && data["delta"] == "{\"url\":"));
        let done = events
            .iter()
            .find(|(event, data)| {
                event == "response.output_item.done" && data["item"]["type"] == "function_call"
            })
            .unwrap();
        assert_eq!(done.1["item"]["namespace"], "browser");
        assert_eq!(done.1["item"]["name"], "open page");
        assert_eq!(
            done.1["item"]["arguments"],
            "{\"url\":\"https://example.com\"}"
        );
    }

    #[tokio::test]
    async fn streams_anthropic_web_search_as_responses_sse() {
        let input = stream::iter(vec![
            Ok::<_, std::io::Error>(Bytes::from_static(
                b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-sonnet-4-6\",\"content\":[],\"usage\":{\"input_tokens\":2,\"output_tokens\":0}}}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"server_tool_use\",\"id\":\"srvtoolu_1\",\"name\":\"web_search\",\"input\":{\"query\":\"rust 2026\"}}}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"web_search_tool_result\",\"tool_use_id\":\"srvtoolu_1\",\"content\":[{\"type\":\"web_search_result\",\"title\":\"Rust\",\"url\":\"https://www.rust-lang.org\"}]}}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
            )),
        ]);

        let chunks = AnthropicSseToResponsesSse::new(
            input,
            "fallback-model".to_string(),
            ToolNameMap::default(),
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .map(Result::unwrap)
        .collect::<Vec<_>>();
        let events = parse_events_from_bytes(&chunks);

        assert!(
            events
                .iter()
                .any(|(event, data)| event == "response.output_item.added"
                    && data["item"]["type"] == "web_search_call"
                    && data["item"]["status"] == "in_progress")
        );
        let done = events
            .iter()
            .find(|(event, data)| {
                event == "response.output_item.done" && data["item"]["type"] == "web_search_call"
            })
            .unwrap();
        assert_eq!(done.1["item"]["call_id"], "srvtoolu_1");
        assert_eq!(done.1["item"]["action"]["input"]["query"], "rust 2026");
        assert_eq!(
            done.1["item"]["action"]["result"]["content"][0]["url"],
            "https://www.rust-lang.org"
        );
    }
}
