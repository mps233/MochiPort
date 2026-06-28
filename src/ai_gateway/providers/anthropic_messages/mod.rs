//! Anthropic Messages 出站 provider。

use std::io;

use axum::{
    body::{Body, Bytes},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::Response,
};
use futures_util::{StreamExt, stream as futures_stream};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tracing::{debug, error};

use crate::ai_gateway::config::ProviderConfig;
use crate::ai_gateway::context::GatewayContext;
use crate::ai_gateway::error::GatewayError;
use crate::ai_gateway::model::{GatewayRequest, ResponseObject, generate_response_id};
use crate::ai_gateway::request_log::{
    self, RequestLogContext, RequestLogUpdate, ResponsesSseLogStream, UpstreamSseCaptureStream,
};
use crate::ai_gateway::tool_names::ToolNameMap;

use super::{
    apply_total_request_timeout, ensure_success_response, execute_stream_start,
    execute_upstream_request,
};

mod citations;
mod custom_tools;
mod glm_compat;
mod options;
mod request;
mod request_content;
mod request_reasoning;
mod request_tools;
mod response;
mod stream;
mod stream_events;
mod stream_items;
mod stream_message;
mod stream_reasoning;
mod stream_response;
mod stream_state;
mod stream_tools;
mod types;

#[cfg(test)]
mod tests;

use options::{
    AnthropicAuthStyle, AnthropicHeaderStyle, AnthropicProviderOptions, AnthropicProviderProfile,
    AnthropicVersionHeader,
};
use request::build_anthropic_request;
use response::convert_anthropic_response;
use stream::AnthropicSseToResponsesSse;
use types::{
    ANTHROPIC_CLAUDE_CODE_BETA, ANTHROPIC_WEB_SEARCH_TYPE, CLAUDE_CODE_STAINLESS_PACKAGE_VERSION,
    CLAUDE_CODE_STAINLESS_RUNTIME_VERSION, CLAUDE_CODE_USER_AGENT,
};

const WEB_SEARCH_TOOL_NAME: &str = "WebSearch";

pub async fn handle(
    client: &reqwest::Client,
    ctx: &GatewayContext,
    request: &GatewayRequest,
    response_model: &str,
    provider: &ProviderConfig,
    log_context: Option<RequestLogContext>,
) -> Result<Response<Body>, GatewayError> {
    let options = AnthropicProviderOptions::from_provider(provider)?;
    let (mut anthropic_body, tool_name_map) = build_anthropic_request(request, options.profile)?;
    let has_internal_web_search = has_web_search_client_tool(&anthropic_body);
    let url = options.messages_url(provider);
    debug!(
        url = %url,
        stream = request.stream,
        internal_web_search = has_internal_web_search,
        "proxying to anthropic messages"
    );

    if has_internal_web_search {
        if let Some(response) = handle_with_internal_web_search(
            client,
            ctx,
            request,
            response_model,
            provider,
            &options,
            log_context.clone(),
            anthropic_body.clone(),
            tool_name_map.clone(),
        )
        .await?
        {
            return Ok(response);
        }
        anthropic_body = force_anthropic_stream(anthropic_body, request.stream);
    }

    let upstream_req = build_anthropic_upstream_request(
        client,
        ctx,
        request,
        &anthropic_body,
        provider,
        &options,
    )?;

    if let Some(log_context) = &log_context {
        let update = RequestLogUpdate {
            upstream_request_headers_json: request_log::headers_to_json(upstream_req.headers()),
            upstream_request_body_bytes: request_log::json_body_size_bytes(&anthropic_body),
            upstream_request_json: serde_json::to_string(&anthropic_body).ok(),
            ..RequestLogUpdate::default()
        };
        if let Err(err) = log_context.store.update_record(log_context.log_id, &update) {
            request_log::log_update_error(err);
        }
    }

    let upstream_resp = if request.stream {
        execute_stream_start(
            client,
            upstream_req,
            provider.timeout_secs,
            "anthropic upstream request failed",
        )
        .await?
    } else {
        execute_upstream_request(
            client,
            upstream_req,
            provider.timeout_secs,
            "anthropic upstream request failed",
        )
        .await?
    };
    let upstream_resp = ensure_success_response(&provider.name, upstream_resp).await?;

    if request.stream {
        return handle_stream(
            upstream_resp,
            response_model,
            tool_name_map,
            options.profile,
            log_context,
        )
        .await;
    }

    let anthropic_resp: Value = upstream_resp.json().await.map_err(|e| {
        GatewayError::upstream(StatusCode::BAD_GATEWAY, format!("parse upstream json: {e}"))
    })?;
    let response_obj = convert_anthropic_response(
        &anthropic_resp,
        response_model,
        &tool_name_map,
        options.profile,
    );
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
        if let Err(err) = log_context.store.update_record(log_context.log_id, &update) {
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

#[allow(clippy::too_many_arguments)]
async fn handle_with_internal_web_search(
    client: &reqwest::Client,
    ctx: &GatewayContext,
    request: &GatewayRequest,
    response_model: &str,
    provider: &ProviderConfig,
    options: &AnthropicProviderOptions,
    log_context: Option<RequestLogContext>,
    mut anthropic_body: Value,
    tool_name_map: ToolNameMap,
) -> Result<Option<Response<Body>>, GatewayError> {
    if request.stream {
        return Ok(Some(stream_internal_web_search_response(
            client,
            ctx,
            request,
            response_model,
            provider,
            options,
            log_context,
            anthropic_body,
            tool_name_map,
        )));
    }

    let mut internal_web_search_items = Vec::new();
    loop {
        let step_body = force_anthropic_stream(anthropic_body.clone(), true);
        let step_resp = execute_anthropic_stream_message(
            client,
            ctx,
            request,
            provider,
            options,
            &step_body,
            &log_context,
        )
        .await?;
        let tool_uses = find_web_search_tool_uses(&step_resp);
        if tool_uses.is_empty() {
            let response_obj = convert_anthropic_response(
                &step_resp,
                response_model,
                &tool_name_map,
                options.profile,
            );
            return Ok(Some(response_from_response_object(
                response_obj,
                request.stream,
                &log_context,
                &internal_web_search_items,
            )?));
        }

        let mut tool_results = Vec::new();
        for tool_use in tool_uses {
            internal_web_search_items.push(web_search_response_item(&tool_use));
            let search_text = execute_internal_web_search(
                client,
                ctx,
                request,
                provider,
                options,
                tool_use.query.as_str(),
                &log_context,
            )
            .await?;
            tool_results.push((tool_use.id, search_text));
        }
        append_tool_results(&mut anthropic_body, &step_resp, tool_results);
    }
}

#[allow(clippy::too_many_arguments)]
fn stream_internal_web_search_response(
    client: &reqwest::Client,
    ctx: &GatewayContext,
    request: &GatewayRequest,
    response_model: &str,
    provider: &ProviderConfig,
    options: &AnthropicProviderOptions,
    log_context: Option<RequestLogContext>,
    anthropic_body: Value,
    tool_name_map: ToolNameMap,
) -> Response<Body> {
    let (tx, rx) = mpsc::channel::<Result<Bytes, io::Error>>(32);
    let task_log_context = log_context.clone();
    let client = client.clone();
    let ctx = ctx.clone();
    let request = request.clone();
    let response_model = response_model.to_string();
    let provider = provider.clone();
    let options = options.clone();

    tokio::spawn(async move {
        if let Err(err) = run_internal_web_search_stream(
            client,
            ctx,
            request,
            response_model,
            provider,
            options,
            task_log_context,
            anthropic_body,
            tool_name_map,
            tx.clone(),
        )
        .await
        {
            let _ = tx
                .send(Err(io::Error::new(io::ErrorKind::Other, err.message)))
                .await;
        }
    });

    let sse_stream = futures_stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    });
    let body = if let Some(log_context) = log_context {
        Body::from_stream(ResponsesSseLogStream::new(
            Box::pin(sse_stream),
            log_context,
        ))
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
    response
}

#[allow(clippy::too_many_arguments)]
async fn run_internal_web_search_stream(
    client: reqwest::Client,
    ctx: GatewayContext,
    request: GatewayRequest,
    response_model: String,
    provider: ProviderConfig,
    options: AnthropicProviderOptions,
    log_context: Option<RequestLogContext>,
    mut anthropic_body: Value,
    tool_name_map: ToolNameMap,
    tx: mpsc::Sender<Result<Bytes, io::Error>>,
) -> Result<(), GatewayError> {
    let mut state = InternalWebSearchSseState::new(response_model.clone());
    state.emit_started(&tx).await?;

    loop {
        let step_body = force_anthropic_stream(anthropic_body.clone(), true);
        let step_resp = execute_anthropic_stream_message(
            &client,
            &ctx,
            &request,
            &provider,
            &options,
            &step_body,
            &log_context,
        )
        .await?;
        let tool_uses = find_web_search_tool_uses(&step_resp);
        if tool_uses.is_empty() {
            let response_obj = convert_anthropic_response(
                &step_resp,
                &response_model,
                &tool_name_map,
                options.profile,
            );
            state.emit_final_response(&tx, response_obj).await?;
            return Ok(());
        }

        let mut tool_results = Vec::new();
        for tool_use in tool_uses {
            let search_item = state.emit_web_search_started(&tx, &tool_use).await?;
            let search_text = execute_internal_web_search(
                &client,
                &ctx,
                &request,
                &provider,
                &options,
                tool_use.query.as_str(),
                &log_context,
            )
            .await?;
            state
                .emit_web_search_completed(&tx, &search_item, &tool_use)
                .await?;
            tool_results.push((tool_use.id, search_text));
        }
        append_tool_results(&mut anthropic_body, &step_resp, tool_results);
    }
}

struct InternalWebSearchSseState {
    response_id: String,
    model: String,
    created_at: i64,
    sequence_number: usize,
    output_index: usize,
}

struct InternalWebSearchSseItem {
    item_id: String,
    output_index: usize,
}

impl InternalWebSearchSseState {
    fn new(model: String) -> Self {
        Self {
            response_id: generate_response_id(),
            model,
            created_at: stream_events::unix_timestamp(),
            sequence_number: 0,
            output_index: 0,
        }
    }

    async fn emit_started(
        &mut self,
        tx: &mpsc::Sender<Result<Bytes, io::Error>>,
    ) -> Result<(), GatewayError> {
        let created_seq = self.next_seq();
        let created_response = self.response_value("in_progress", Vec::new());
        self.emit_event(
            tx,
            "response.created",
            json!({
                "type": "response.created",
                "sequence_number": created_seq,
                "response": created_response,
            }),
        )
        .await?;
        let in_progress_seq = self.next_seq();
        let in_progress_response = self.response_value("in_progress", Vec::new());
        self.emit_event(
            tx,
            "response.in_progress",
            json!({
                "type": "response.in_progress",
                "sequence_number": in_progress_seq,
                "response": in_progress_response,
            }),
        )
        .await
    }

    async fn emit_web_search_started(
        &mut self,
        tx: &mpsc::Sender<Result<Bytes, io::Error>>,
        _tool_use: &WebSearchToolUse,
    ) -> Result<InternalWebSearchSseItem, GatewayError> {
        let item_id = generate_web_search_item_id();
        let output_index = self.output_index;
        self.output_index += 1;
        let added_seq = self.next_seq();
        self.emit_event(
            tx,
            "response.output_item.added",
            json!({
                "type": "response.output_item.added",
                "sequence_number": added_seq,
                "output_index": output_index,
                "item": {
                    "type": "web_search_call",
                    "id": item_id.clone(),
                    "status": "in_progress",
                },
            }),
        )
        .await?;
        let in_progress_seq = self.next_seq();
        self.emit_event(
            tx,
            "response.web_search_call.in_progress",
            json!({
                "type": "response.web_search_call.in_progress",
                "sequence_number": in_progress_seq,
                "item_id": item_id.clone(),
                "output_index": output_index,
            }),
        )
        .await?;
        let searching_seq = self.next_seq();
        self.emit_event(
            tx,
            "response.web_search_call.searching",
            json!({
                "type": "response.web_search_call.searching",
                "sequence_number": searching_seq,
                "item_id": item_id.clone(),
                "output_index": output_index,
            }),
        )
        .await?;
        Ok(InternalWebSearchSseItem {
            item_id,
            output_index,
        })
    }

    async fn emit_web_search_completed(
        &mut self,
        tx: &mpsc::Sender<Result<Bytes, io::Error>>,
        search_item: &InternalWebSearchSseItem,
        tool_use: &WebSearchToolUse,
    ) -> Result<(), GatewayError> {
        let completed_seq = self.next_seq();
        self.emit_event(
            tx,
            "response.web_search_call.completed",
            json!({
                "type": "response.web_search_call.completed",
                "sequence_number": completed_seq,
                "item_id": search_item.item_id.clone(),
                "output_index": search_item.output_index,
            }),
        )
        .await?;
        let done_seq = self.next_seq();
        self.emit_event(
            tx,
            "response.output_item.done",
            json!({
                "type": "response.output_item.done",
                "sequence_number": done_seq,
                "output_index": search_item.output_index,
                "item": {
                    "type": "web_search_call",
                    "id": search_item.item_id.clone(),
                    "status": "completed",
                    "action": {
                        "type": "search",
                        "query": tool_use.query.clone(),
                        "queries": [tool_use.query.clone()],
                    },
                },
            }),
        )
        .await
    }

    async fn emit_final_response(
        &mut self,
        tx: &mpsc::Sender<Result<Bytes, io::Error>>,
        mut response_obj: ResponseObject,
    ) -> Result<(), GatewayError> {
        response_obj.id = self.response_id.clone();
        response_obj.model = self.model.clone();
        response_obj.created_at = self.created_at;
        let mut output = Vec::new();
        for item in &response_obj.output {
            let item = serde_json::to_value(item).unwrap_or_default();
            let output_index = self.output_index;
            self.output_index += 1;
            let added_seq = self.next_seq();
            self.emit_event(
                tx,
                "response.output_item.added",
                json!({
                    "type": "response.output_item.added",
                    "sequence_number": added_seq,
                    "output_index": output_index,
                    "item": item,
                }),
            )
            .await?;
            let done_seq = self.next_seq();
            self.emit_event(
                tx,
                "response.output_item.done",
                json!({
                    "type": "response.output_item.done",
                    "sequence_number": done_seq,
                    "output_index": output_index,
                    "item": item,
                }),
            )
            .await?;
            output.push(item);
        }
        let status = response_obj.status.clone();
        let event_type = if status == "incomplete" {
            "response.incomplete"
        } else {
            "response.completed"
        };
        let mut response_value = serde_json::to_value(response_obj).unwrap_or_default();
        response_value["output"] = Value::Array(output);
        let final_seq = self.next_seq();
        self.emit_event(
            tx,
            event_type,
            json!({
                "type": event_type,
                "sequence_number": final_seq,
                "response": response_value,
            }),
        )
        .await
    }

    fn response_value(&self, status: &str, output: Vec<Value>) -> Value {
        json!({
            "id": self.response_id.clone(),
            "object": "response",
            "model": self.model.clone(),
            "created_at": self.created_at,
            "status": status,
            "output": output,
        })
    }

    async fn emit_event(
        &mut self,
        tx: &mpsc::Sender<Result<Bytes, io::Error>>,
        event_type: &str,
        data: Value,
    ) -> Result<(), GatewayError> {
        tx.send(Ok(Bytes::from(format!(
            "event: {event_type}\ndata: {data}\n\n"
        ))))
        .await
        .map_err(|_| {
            GatewayError::upstream(
                StatusCode::BAD_GATEWAY,
                "client disconnected during anthropic internal web search stream",
            )
        })
    }

    fn next_seq(&mut self) -> usize {
        let seq = self.sequence_number;
        self.sequence_number += 1;
        seq
    }
}

fn generate_web_search_item_id() -> String {
    format!("ws_{}", uuid::Uuid::new_v4().as_simple())
}

async fn execute_anthropic_stream_message(
    client: &reqwest::Client,
    ctx: &GatewayContext,
    request: &GatewayRequest,
    provider: &ProviderConfig,
    options: &AnthropicProviderOptions,
    anthropic_body: &Value,
    log_context: &Option<RequestLogContext>,
) -> Result<Value, GatewayError> {
    let mut step_request = request.clone();
    step_request.stream = true;
    let upstream_req = build_anthropic_upstream_request(
        client,
        ctx,
        &step_request,
        anthropic_body,
        provider,
        options,
    )?;
    update_upstream_log(log_context, upstream_req.headers(), anthropic_body);
    let upstream_resp = execute_stream_start(
        client,
        upstream_req,
        provider.timeout_secs,
        "anthropic upstream request failed",
    )
    .await?;
    let upstream_resp = ensure_success_response(&provider.name, upstream_resp).await?;
    let raw_sse = read_sse_to_string(upstream_resp, log_context).await?;
    anthropic_message_from_sse(&raw_sse)
}

async fn execute_internal_web_search(
    client: &reqwest::Client,
    ctx: &GatewayContext,
    request: &GatewayRequest,
    provider: &ProviderConfig,
    options: &AnthropicProviderOptions,
    query: &str,
    log_context: &Option<RequestLogContext>,
) -> Result<String, GatewayError> {
    let body = internal_web_search_body(ctx, &request.model, query);
    let mut search_request = request.clone();
    search_request.stream = true;
    let upstream_req =
        build_anthropic_upstream_request(client, ctx, &search_request, &body, provider, options)?;
    let upstream_resp = execute_stream_start(
        client,
        upstream_req,
        provider.timeout_secs,
        "anthropic internal web search request failed",
    )
    .await?;
    let upstream_resp = ensure_success_response(&provider.name, upstream_resp).await?;
    let raw = read_sse_to_string(upstream_resp, log_context).await?;
    Ok(search_results_to_tool_text(query, &raw))
}

fn internal_web_search_body(ctx: &GatewayContext, model: &str, query: &str) -> Value {
    json!({
        "model": model,
        "tools": [{
            "name": "web_search",
            "type": ANTHROPIC_WEB_SEARCH_TYPE,
            "max_uses": 8
        }],
        "stream": true,
        "system": [
            {
                "text": "You are Claude Code, Anthropic's official CLI for Claude.",
                "type": "text"
            },
            {
                "text": "You are an assistant for performing a web search tool use",
                "type": "text"
            }
        ],
        "messages": [{
            "role": "user",
            "content": [{
                "text": format!("Perform a web search for the query: {query}"),
                "type": "text"
            }]
        }],
        "metadata": {
            "user_id": claude_code_metadata_user_id(ctx)
        },
        "thinking": {"type": "disabled"},
        "max_tokens": 64000,
        "tool_choice": {
            "name": "web_search",
            "type": "tool"
        },
        "output_config": {"effort": "high"}
    })
}

fn claude_code_metadata_user_id(ctx: &GatewayContext) -> String {
    let session_id = ctx.session_id.as_deref().unwrap_or(&ctx.prompt_cache_key);
    json!({
        "device_id": claude_code_device_id(session_id),
        "account_uuid": "",
        "session_id": session_id
    })
    .to_string()
}

fn claude_code_device_id(session_id: &str) -> String {
    let digest = Sha256::digest(format!("codexhub-claude-code-device:{session_id}").as_bytes());
    hex::encode(digest)
}

async fn read_sse_to_string(
    response: reqwest::Response,
    log_context: &Option<RequestLogContext>,
) -> Result<String, GatewayError> {
    let mut stream = response.bytes_stream();
    let mut raw = String::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| {
            GatewayError::upstream(
                StatusCode::BAD_GATEWAY,
                format!("read upstream stream: {e}"),
            )
        })?;
        raw.push_str(&String::from_utf8_lossy(&chunk));
    }
    save_internal_upstream_sse(log_context, &raw);
    Ok(raw)
}

fn response_from_response_object(
    response_obj: crate::ai_gateway::model::ResponseObject,
    stream: bool,
    log_context: &Option<RequestLogContext>,
    prefix_items: &[Value],
) -> Result<Response<Body>, GatewayError> {
    if let Some(log_context) = log_context {
        let response_value = serde_json::to_value(&response_obj).unwrap_or_default();
        let update = RequestLogUpdate {
            status: Some(response_obj.status.clone()),
            usage: Some(request_log::usage_from_response_value(&response_value)),
            latency_ms: Some(request_log::elapsed_ms(log_context.started_at)),
            response_json: serde_json::to_string(&response_value).ok(),
            ..RequestLogUpdate::default()
        };
        if let Err(err) = log_context.store.update_record(log_context.log_id, &update) {
            request_log::log_update_error(err);
        }
    }
    if stream {
        let body_bytes = response_object_to_sse(&response_obj, prefix_items);
        let mut response = Response::new(Body::from(body_bytes));
        *response.status_mut() = StatusCode::OK;
        response.headers_mut().insert(
            HeaderName::from_static("content-type"),
            HeaderValue::from_static("text/event-stream"),
        );
        return Ok(response);
    }
    let body_bytes = serde_json::to_vec(&response_obj).unwrap_or_default();
    let mut response = Response::new(Body::from(body_bytes));
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        HeaderName::from_static("content-type"),
        HeaderValue::from_static("application/json"),
    );
    Ok(response)
}

fn response_object_to_sse(
    response_obj: &crate::ai_gateway::model::ResponseObject,
    prefix_items: &[Value],
) -> Vec<u8> {
    let mut created = serde_json::to_value(response_obj).unwrap_or_default();
    created["status"] = json!("in_progress");
    let mut completed = serde_json::to_value(response_obj).unwrap_or_default();
    let mut output_items = prefix_items.to_vec();
    output_items.extend(
        response_obj
            .output
            .iter()
            .filter_map(|item| serde_json::to_value(item).ok()),
    );
    completed["output"] = Value::Array(output_items.clone());
    let mut events = Vec::new();
    events.push((
        "response.created",
        json!({
            "type": "response.created",
            "sequence_number": 0,
            "response": created,
        }),
    ));
    events.push((
        "response.in_progress",
        json!({
            "type": "response.in_progress",
            "sequence_number": 1,
            "response": created,
        }),
    ));
    for (index, item) in output_items.iter().enumerate() {
        events.push((
            "response.output_item.added",
            json!({
                "type": "response.output_item.added",
                "sequence_number": index * 2 + 2,
                "output_index": index,
                "item": item,
            }),
        ));
        events.push((
            "response.output_item.done",
            json!({
                "type": "response.output_item.done",
                "sequence_number": index * 2 + 3,
                "output_index": index,
                "item": item,
            }),
        ));
    }
    events.push((
        if response_obj.status == "incomplete" {
            "response.incomplete"
        } else {
            "response.completed"
        },
        json!({
            "type": if response_obj.status == "incomplete" {
                "response.incomplete"
            } else {
                "response.completed"
            },
            "sequence_number": (prefix_items.len() + response_obj.output.len()) * 2 + 2,
            "response": completed,
        }),
    ));
    events
        .into_iter()
        .map(|(event, data)| format!("event: {event}\ndata: {data}\n\n"))
        .collect::<String>()
        .into_bytes()
}

fn web_search_response_item(tool_use: &WebSearchToolUse) -> Value {
    json!({
        "type": "web_search_call",
        "id": tool_use.id,
        "call_id": tool_use.id,
        "status": "completed",
        "action": {
            "type": "search",
            "query": tool_use.query.clone(),
            "queries": [tool_use.query.clone()],
        },
    })
}

#[derive(Debug)]
struct WebSearchToolUse {
    id: String,
    query: String,
}

fn find_web_search_tool_uses(response: &Value) -> Vec<WebSearchToolUse> {
    response
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|block| {
            if block.get("type").and_then(Value::as_str) != Some("tool_use")
                || block.get("name").and_then(Value::as_str) != Some(WEB_SEARCH_TOOL_NAME)
            {
                return None;
            }
            let id = block.get("id").and_then(Value::as_str)?.to_string();
            let query = block
                .get("input")
                .and_then(|input| input.get("query").or_else(|| input.get("search_query")))
                .and_then(Value::as_str)?
                .trim()
                .to_string();
            (!query.is_empty()).then_some(WebSearchToolUse { id, query })
        })
        .collect()
}

fn append_tool_results(
    anthropic_body: &mut Value,
    assistant_response: &Value,
    tool_results: Vec<(String, String)>,
) {
    if tool_results.is_empty() {
        return;
    }
    let Some(messages) = anthropic_body
        .get_mut("messages")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    if let Some(content) = assistant_response.get("content").cloned() {
        messages.push(json!({
            "role": "assistant",
            "content": content
        }));
    }
    let content = tool_results
        .into_iter()
        .map(|(tool_use_id, search_text)| {
            json!({
                "type": "tool_result",
                "tool_use_id": tool_use_id,
                "content": search_text
            })
        })
        .collect::<Vec<_>>();
    messages.push(json!({
        "role": "user",
        "content": content
    }));
}

fn search_results_to_tool_text(query: &str, raw_sse: &str) -> String {
    let mut results = Vec::new();
    for event in parse_sse_json_events(raw_sse) {
        let Some(block) = event.get("content_block").filter(|block| {
            block.get("type").and_then(Value::as_str) == Some("web_search_tool_result")
        }) else {
            continue;
        };
        if let Some(content) = block.get("content").and_then(Value::as_array) {
            for result in content {
                if result.get("type").and_then(Value::as_str) != Some("web_search_result") {
                    continue;
                }
                let title = result.get("title").and_then(Value::as_str).unwrap_or("");
                let url = result.get("url").and_then(Value::as_str).unwrap_or("");
                let snippet = result
                    .get("encrypted_content")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                results.push(format!("{title}\nURL: {url}\nSnippet: {snippet}"));
            }
        }
    }
    if results.is_empty() {
        return format!("No web search results were returned for query: {query}");
    }
    format!(
        "Web search results for query: {query}\n\n{}",
        results.join("\n\n")
    )
}

fn anthropic_message_from_sse(raw_sse: &str) -> Result<Value, GatewayError> {
    let mut message = Map::new();
    let mut content = Vec::new();
    let mut stop_reason: Option<String> = None;
    let mut usage = Map::new();
    for event in parse_sse_json_events(raw_sse) {
        match event.get("type").and_then(Value::as_str) {
            Some("message_start") => {
                if let Some(start_message) = event.get("message").and_then(Value::as_object) {
                    for (key, value) in start_message {
                        if key == "content" {
                            continue;
                        }
                        if key == "usage"
                            && let Some(start_usage) = value.as_object()
                        {
                            for (usage_key, usage_value) in start_usage {
                                usage.insert(usage_key.clone(), usage_value.clone());
                            }
                            continue;
                        }
                        message.insert(key.clone(), value.clone());
                    }
                }
            }
            Some("content_block_start") => {
                if let Some(block) = event.get("content_block").cloned() {
                    let index = event.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                    ensure_content_index(&mut content, index);
                    content[index] = block;
                }
            }
            Some("content_block_delta") => {
                let index = event.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                ensure_content_index(&mut content, index);
                if let Some(delta) = event.get("delta") {
                    apply_content_delta(&mut content[index], delta);
                }
            }
            Some("message_delta") => {
                if let Some(reason) = event
                    .get("delta")
                    .and_then(|delta| delta.get("stop_reason"))
                    .and_then(Value::as_str)
                {
                    stop_reason = Some(reason.to_string());
                }
                if let Some(delta_usage) = event.get("usage").and_then(Value::as_object) {
                    for (usage_key, usage_value) in delta_usage {
                        usage.insert(usage_key.clone(), usage_value.clone());
                    }
                }
            }
            _ => {}
        }
    }
    if message.is_empty() && content.is_empty() {
        return Err(GatewayError::upstream(
            StatusCode::BAD_GATEWAY,
            "anthropic stream did not contain a message",
        ));
    }
    message
        .entry("type".to_string())
        .or_insert_with(|| json!("message"));
    message
        .entry("role".to_string())
        .or_insert_with(|| json!("assistant"));
    message
        .entry("content".to_string())
        .or_insert_with(|| Value::Array(content));
    message.insert(
        "stop_reason".to_string(),
        stop_reason.map(Value::String).unwrap_or(Value::Null),
    );
    if !usage.is_empty() {
        message.insert("usage".to_string(), Value::Object(usage));
    }
    Ok(Value::Object(message))
}

fn ensure_content_index(content: &mut Vec<Value>, index: usize) {
    while content.len() <= index {
        content.push(Value::Null);
    }
}

fn apply_content_delta(block: &mut Value, delta: &Value) {
    match delta.get("type").and_then(Value::as_str) {
        Some("text_delta") => {
            let text = delta.get("text").and_then(Value::as_str).unwrap_or("");
            append_string_field(block, "text", text);
        }
        Some("input_json_delta") => {
            let partial = delta
                .get("partial_json")
                .and_then(Value::as_str)
                .unwrap_or("");
            append_string_field(block, "__partial_input_json", partial);
            if let Some(input) = block
                .get("__partial_input_json")
                .and_then(Value::as_str)
                .and_then(|value| serde_json::from_str::<Value>(value).ok())
                && let Some(object) = block.as_object_mut()
            {
                object.insert("input".to_string(), input);
                object.remove("__partial_input_json");
            }
        }
        Some("thinking_delta") => {
            let thinking = delta.get("thinking").and_then(Value::as_str).unwrap_or("");
            append_string_field(block, "thinking", thinking);
        }
        _ => {}
    }
}

fn append_string_field(block: &mut Value, field: &str, value: &str) {
    if value.is_empty() {
        return;
    }
    if let Some(object) = block.as_object_mut() {
        let entry = object
            .entry(field.to_string())
            .or_insert_with(|| Value::String(String::new()));
        if let Some(text) = entry.as_str() {
            *entry = Value::String(format!("{text}{value}"));
        }
    }
}

fn parse_sse_json_events(raw_sse: &str) -> Vec<Value> {
    let mut events = Vec::new();
    let mut data_lines = Vec::new();
    for line in raw_sse.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            if !data_lines.is_empty() {
                let data = data_lines.join("\n");
                if data.trim() != "[DONE]"
                    && let Ok(value) = serde_json::from_str::<Value>(&data)
                {
                    events.push(value);
                }
                data_lines.clear();
            }
            continue;
        }
        if let Some(data) = line.strip_prefix("data:") {
            data_lines.push(data.strip_prefix(' ').unwrap_or(data).to_string());
        }
    }
    if !data_lines.is_empty() {
        let data = data_lines.join("\n");
        if data.trim() != "[DONE]"
            && let Ok(value) = serde_json::from_str::<Value>(&data)
        {
            events.push(value);
        }
    }
    events
}

fn has_web_search_client_tool(body: &Value) -> bool {
    body.get("tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools.iter().any(|tool| {
                tool.get("name").and_then(Value::as_str) == Some(WEB_SEARCH_TOOL_NAME)
                    && tool.get("type").is_none()
            })
        })
        .unwrap_or(false)
}

fn force_anthropic_stream(mut body: Value, stream: bool) -> Value {
    if stream {
        body["stream"] = json!(true);
    } else if let Some(object) = body.as_object_mut() {
        object.remove("stream");
    }
    body
}

fn update_upstream_log(
    log_context: &Option<RequestLogContext>,
    headers: &HeaderMap,
    anthropic_body: &Value,
) {
    if let Some(log_context) = log_context {
        let update = RequestLogUpdate {
            upstream_request_headers_json: request_log::headers_to_json(headers),
            upstream_request_body_bytes: request_log::json_body_size_bytes(anthropic_body),
            upstream_request_json: serde_json::to_string(anthropic_body).ok(),
            ..RequestLogUpdate::default()
        };
        if let Err(err) = log_context.store.update_record(log_context.log_id, &update) {
            request_log::log_update_error(err);
        }
    }
}

fn save_internal_upstream_sse(log_context: &Option<RequestLogContext>, raw_sse: &str) {
    let Some(log_context) = log_context else {
        return;
    };
    if raw_sse.is_empty() {
        return;
    }
    let existing = log_context
        .store
        .get_detail(log_context.log_id)
        .ok()
        .flatten()
        .and_then(|detail| detail.upstream_response_sse)
        .unwrap_or_default();
    let mut text = if existing.is_empty() {
        raw_sse.to_string()
    } else {
        format!("{existing}\n\n: [codexhub] internal anthropic SSE segment\n\n{raw_sse}")
    };
    const INTERNAL_UPSTREAM_SSE_LOG_LIMIT_BYTES: usize = 512 * 1024;
    if text.len() > INTERNAL_UPSTREAM_SSE_LOG_LIMIT_BYTES {
        text.truncate(INTERNAL_UPSTREAM_SSE_LOG_LIMIT_BYTES);
        text.push_str("\n\n: [codexhub] upstream SSE log truncated\n");
    }
    let update = RequestLogUpdate {
        upstream_response_sse: Some(text),
        ..RequestLogUpdate::default()
    };
    if let Err(err) = log_context.store.update_record(log_context.log_id, &update) {
        request_log::log_update_error(err);
    }
}

fn build_anthropic_upstream_request(
    client: &reqwest::Client,
    ctx: &GatewayContext,
    request: &GatewayRequest,
    anthropic_body: &Value,
    provider: &ProviderConfig,
    options: &AnthropicProviderOptions,
) -> Result<reqwest::Request, GatewayError> {
    let req_builder = client
        .post(options.messages_url(provider))
        .json(anthropic_body);
    let req_builder =
        apply_total_request_timeout(req_builder, provider.timeout_secs, request.stream);
    let mut upstream_req = req_builder.build().map_err(|e| {
        error!(error = %e, "build anthropic upstream request failed");
        GatewayError::upstream(
            StatusCode::BAD_GATEWAY,
            format!("build upstream request: {e}"),
        )
    })?;

    *upstream_req.version_mut() = reqwest::Version::HTTP_11;
    apply_anthropic_managed_headers(upstream_req.headers_mut(), ctx, provider, options)?;
    Ok(upstream_req)
}

fn apply_anthropic_managed_headers(
    headers: &mut HeaderMap,
    ctx: &GatewayContext,
    provider: &ProviderConfig,
    options: &AnthropicProviderOptions,
) -> Result<(), GatewayError> {
    headers.clear();
    insert_static_header(headers, "content-type", "application/json");
    apply_anthropic_auth(headers, provider, options)?;
    apply_anthropic_version_header(headers, options);
    apply_anthropic_client_headers(headers, ctx, provider, options)?;
    Ok(())
}

fn apply_anthropic_auth(
    headers: &mut HeaderMap,
    provider: &ProviderConfig,
    options: &AnthropicProviderOptions,
) -> Result<(), GatewayError> {
    match options.auth {
        AnthropicAuthStyle::XApiKey => {
            insert_dynamic_header(headers, "x-api-key", provider.api_key.trim())?;
        }
        AnthropicAuthStyle::Bearer => {
            insert_dynamic_header(
                headers,
                "authorization",
                bearer_authorization(&provider.api_key),
            )?;
        }
    }
    Ok(())
}

fn apply_anthropic_version_header(headers: &mut HeaderMap, options: &AnthropicProviderOptions) {
    match options.version_header {
        AnthropicVersionHeader::Required(version) => {
            insert_static_header(headers, "anthropic-version", version);
        }
    }
}

fn apply_anthropic_client_headers(
    headers: &mut HeaderMap,
    ctx: &GatewayContext,
    provider: &ProviderConfig,
    options: &AnthropicProviderOptions,
) -> Result<(), GatewayError> {
    match options.headers {
        AnthropicHeaderStyle::Plain => {}
        AnthropicHeaderStyle::ClaudeCode => {
            let existing_beta = headers
                .get("anthropic-beta")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default();
            let merged_beta = merge_anthropic_betas(ANTHROPIC_CLAUDE_CODE_BETA, existing_beta);
            insert_dynamic_header(headers, "anthropic-beta", &merged_beta)?;
            insert_static_header(headers, "anthropic-dangerous-direct-browser-access", "true");
            insert_static_header(headers, "x-app", "cli");
            insert_static_header(headers, "accept", "application/json");
            insert_static_header(headers, "accept-encoding", "gzip, deflate, br, zstd");
            insert_static_header(headers, "connection", "keep-alive");
            insert_static_header(headers, "user-agent", CLAUDE_CODE_USER_AGENT);
            insert_dynamic_header(
                headers,
                "x-stainless-timeout",
                provider.timeout_secs.to_string(),
            )?;
            insert_static_header(headers, "x-stainless-retry-count", "0");
            insert_static_header(headers, "x-stainless-runtime", "node");
            insert_static_header(headers, "x-stainless-lang", "js");
            insert_static_header(headers, "x-stainless-arch", stainless_arch());
            insert_static_header(headers, "x-stainless-os", stainless_os());
            insert_static_header(
                headers,
                "x-stainless-package-version",
                CLAUDE_CODE_STAINLESS_PACKAGE_VERSION,
            );
            insert_static_header(
                headers,
                "x-stainless-runtime-version",
                CLAUDE_CODE_STAINLESS_RUNTIME_VERSION,
            );
            let session_id = ctx.session_id.as_deref().unwrap_or(&ctx.prompt_cache_key);
            insert_dynamic_header(headers, "x-claude-code-session-id", session_id)?;
        }
    }
    Ok(())
}

fn insert_static_header(headers: &mut HeaderMap, name: &'static str, value: &'static str) {
    headers.insert(
        HeaderName::from_static(name),
        HeaderValue::from_static(value),
    );
}

fn insert_dynamic_header(
    headers: &mut HeaderMap,
    name: &'static str,
    value: impl AsRef<str>,
) -> Result<(), GatewayError> {
    let value = HeaderValue::from_str(value.as_ref()).map_err(|e| {
        GatewayError::upstream(
            StatusCode::BAD_GATEWAY,
            format!("invalid Anthropic header '{name}': {e}"),
        )
    })?;
    headers.insert(HeaderName::from_static(name), value);
    Ok(())
}

fn bearer_authorization(api_key: &str) -> String {
    let api_key = api_key.trim();
    if let Some(token) = strip_bearer_prefix(api_key) {
        format!("Bearer {}", token.trim())
    } else {
        format!("Bearer {api_key}")
    }
}

fn strip_bearer_prefix(value: &str) -> Option<&str> {
    let prefix_len = "bearer ".len();
    if value
        .get(..prefix_len)
        .map(|prefix| prefix.eq_ignore_ascii_case("bearer "))
        .unwrap_or(false)
    {
        Some(&value[prefix_len..])
    } else {
        None
    }
}

fn merge_anthropic_betas(required: &str, inbound: &str) -> String {
    let mut merged = Vec::new();
    for beta in required.split(',').chain(inbound.split(',')) {
        let beta = beta.trim();
        if !beta.is_empty() && !merged.contains(&beta) {
            merged.push(beta);
        }
    }
    merged.join(",")
}

fn stainless_arch() -> &'static str {
    #[cfg(target_arch = "x86_64")]
    {
        "x64"
    }
    #[cfg(target_arch = "aarch64")]
    {
        "arm64"
    }
    #[cfg(all(not(target_arch = "x86_64"), not(target_arch = "aarch64")))]
    {
        std::env::consts::ARCH
    }
}

fn stainless_os() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "Windows"
    }
    #[cfg(target_os = "macos")]
    {
        "MacOS"
    }
    #[cfg(target_os = "linux")]
    {
        "Linux"
    }
    #[cfg(all(
        not(target_os = "windows"),
        not(target_os = "macos"),
        not(target_os = "linux")
    ))]
    {
        std::env::consts::OS
    }
}

async fn handle_stream(
    resp: reqwest::Response,
    model: &str,
    tool_name_map: ToolNameMap,
    profile: AnthropicProviderProfile,
    log_context: Option<RequestLogContext>,
) -> Result<Response<Body>, GatewayError> {
    let upstream_bytes = resp.bytes_stream();
    let body = if let Some(log_context) = log_context {
        let captured_upstream = UpstreamSseCaptureStream::new(upstream_bytes, log_context.clone());
        let sse_stream = AnthropicSseToResponsesSse::new(
            captured_upstream,
            model.to_string(),
            tool_name_map,
            profile,
        );
        Body::from_stream(ResponsesSseLogStream::new(sse_stream, log_context))
    } else {
        let sse_stream = AnthropicSseToResponsesSse::new(
            upstream_bytes,
            model.to_string(),
            tool_name_map,
            profile,
        );
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
