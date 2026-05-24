use std::{
    net::SocketAddr,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use anyhow::{Context, Result, anyhow};
use axum::{
    Json, Router,
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::IntoResponse,
    routing::any,
};
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{
        Message as TungsteniteMessage, client::IntoClientRequest, protocol::WebSocketConfig,
    },
};

use crate::{app_state::SharedState, codex::CodexNotification, types::InboundAttachment};

static BRIDGE_REQUEST_ID: AtomicU64 = AtomicU64::new(100_000);
const REMOTE_APP_SERVER_MAX_WEBSOCKET_MESSAGE_SIZE: usize = 128 << 20;
const UPSTREAM_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayStatusResponse {
    pub running: bool,
    pub tui_connected: bool,
    pub upstream_connected: bool,
    pub public_ws_url: String,
    pub upstream_ws_url: String,
    pub current_thread_id: Option<String>,
    pub current_turn_id: Option<String>,
    pub last_error: Option<String>,
}

pub async fn status(State(state): State<SharedState>) -> Json<RelayStatusResponse> {
    Json(status_snapshot(&state).await)
}

pub async fn start(State(state): State<SharedState>) -> impl IntoResponse {
    match start_relay(state.clone()).await {
        Ok(()) => Json(json!({ "ok": true })),
        Err(err) => {
            state
                .push_event("error", "relay_start_failed", err.to_string())
                .await;
            Json(json!({ "ok": false, "error": err.to_string() }))
        }
    }
}

pub async fn websocket(
    State(state): State<SharedState>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| async move {
        if let Err(err) = proxy_tui_connection(state.clone(), socket).await {
            let message = err.to_string();
            {
                let mut relay = state.relay.inner.lock().await;
                relay.tui_connected = false;
                relay.upstream_connected = false;
                relay.last_error = Some(message.clone());
            }
            state.push_event("error", "relay_ws_failed", message).await;
        }
    })
}

pub fn router(state: SharedState) -> Router {
    Router::new().fallback(any(websocket)).with_state(state)
}

pub fn subscribe(state: &SharedState) -> tokio::sync::broadcast::Receiver<CodexNotification> {
    state.relay.notifications.subscribe()
}

pub async fn status_snapshot(state: &SharedState) -> RelayStatusResponse {
    let relay = state.relay.inner.lock().await;
    RelayStatusResponse {
        running: relay.running,
        tui_connected: relay.tui_connected,
        upstream_connected: relay.upstream_connected,
        public_ws_url: relay.public_ws_url.clone(),
        upstream_ws_url: relay.upstream_ws_url.clone(),
        current_thread_id: relay.current_thread_id.clone(),
        current_turn_id: relay.current_turn_id.clone(),
        last_error: relay.last_error.clone(),
    }
}

pub async fn start_relay(state: SharedState) -> Result<()> {
    {
        let relay = state.relay.inner.lock().await;
        if relay.running {
            return Ok(());
        }
    }

    let config = state.config.lock().await.clone();
    let public_addr: SocketAddr = config.relay.public_ws.parse().with_context(|| {
        format!(
            "invalid relay public ws address `{}`",
            config.relay.public_ws
        )
    })?;
    let listener = TcpListener::bind(public_addr).await?;
    let local_addr = listener.local_addr()?;
    {
        let mut relay = state.relay.inner.lock().await;
        relay.public_ws_url = format!("ws://{local_addr}");
        relay.running = true;
        relay.last_error = None;
    }
    state
        .push_event(
            "info",
            "relay_started",
            format!(
                "public=ws://{local_addr} upstream=ws://{}",
                config.relay.upstream_ws
            ),
        )
        .await;
    let app = router(state.clone());
    tokio::spawn(async move {
        if let Err(err) = axum::serve(listener, app).await {
            state
                .push_event("error", "relay_server_failed", err.to_string())
                .await;
        }
    });
    Ok(())
}

async fn proxy_tui_connection(state: SharedState, socket: WebSocket) -> Result<()> {
    let (upstream_url, replaced_previous_connection, connection_epoch) = {
        let mut relay = state.relay.inner.lock().await;
        let replaced_previous_connection =
            relay.disconnect_tx.take().map(|tx| tx.send(())).is_some();
        relay.tui_connected = true;
        relay.last_error = None;
        relay.connection_epoch = relay.connection_epoch.saturating_add(1);
        (
            relay.upstream_ws_url.clone(),
            replaced_previous_connection,
            relay.connection_epoch,
        )
    };
    if replaced_previous_connection {
        state
            .push_event(
                "warn",
                "relay_previous_tui_disconnected",
                "replaced by a new Codex TUI connection",
            )
            .await;
    }
    state
        .push_event("info", "relay_tui_connected", "codex TUI connected")
        .await;

    let upstream = match connect_upstream_app_server(&upstream_url).await {
        Ok(value) => value,
        Err(err) => {
            return Err(anyhow!(
                "failed to connect upstream app-server `{upstream_url}`: {err}"
            ));
        }
    };
    {
        let mut relay = state.relay.inner.lock().await;
        relay.upstream_connected = true;
    }
    state
        .push_event("info", "relay_upstream_connected", upstream_url)
        .await;

    let (mut tui_write, mut tui_read) = socket.split();
    let (mut upstream_write, mut upstream_read) = upstream.split();

    let (inject_tx, mut inject_rx) = tokio::sync::mpsc::channel::<Value>(128);
    let (disconnect_tx, mut disconnect_rx) = tokio::sync::oneshot::channel::<()>();
    let (upstream_out_tx, mut upstream_out_rx) =
        tokio::sync::mpsc::unbounded_channel::<TungsteniteMessage>();
    let (tui_out_tx, mut tui_out_rx) = tokio::sync::mpsc::unbounded_channel::<Message>();
    {
        let mut relay = state.relay.inner.lock().await;
        relay.last_error = None;
        relay.inject_tx = Some(inject_tx);
        relay.disconnect_tx = Some(disconnect_tx);
    }

    let tui_reader_state = state.clone();
    let tui_reader_upstream_tx = upstream_out_tx.clone();
    let tui_reader_tui_tx = tui_out_tx.clone();
    let mut tui_reader = tokio::spawn(async move {
        while let Some(tui_msg) = tui_read.next().await {
            let tui_msg = tui_msg?;
            match tui_msg {
                Message::Text(text) => {
                    if let Ok(value) = serde_json::from_str::<Value>(&text) {
                        observe_client_message(&tui_reader_state, &value).await;
                    }
                    tui_reader_upstream_tx
                        .send(TungsteniteMessage::Text(text.to_string().into()))
                        .map_err(|_| anyhow!("upstream writer channel closed"))?;
                }
                Message::Binary(data) => {
                    tui_reader_upstream_tx
                        .send(TungsteniteMessage::Binary(data.to_vec()))
                        .map_err(|_| anyhow!("upstream writer channel closed"))?;
                }
                Message::Ping(data) => {
                    let _ = tui_reader_tui_tx.send(Message::Pong(data));
                }
                Message::Pong(_) => {}
                Message::Close(frame) => {
                    tui_reader_upstream_tx
                        .send(TungsteniteMessage::Close(frame.map(|f| {
                            tokio_tungstenite::tungstenite::protocol::CloseFrame {
                                code: f.code.into(),
                                reason: f.reason.to_string().into(),
                            }
                        })))
                        .map_err(|_| anyhow!("upstream writer channel closed"))?;
                    break;
                }
            }
        }
        Ok::<(), anyhow::Error>(())
    });

    let upstream_reader_state = state.clone();
    let upstream_reader_upstream_tx = upstream_out_tx.clone();
    let upstream_reader_tui_tx = tui_out_tx.clone();
    let mut upstream_reader = tokio::spawn(async move {
        while let Some(upstream_msg) = upstream_read.next().await {
            let upstream_msg = upstream_msg?;
            match upstream_msg {
                TungsteniteMessage::Text(text) => {
                    let parsed = serde_json::from_str::<Value>(&text).ok();
                    let bridge_owned_response =
                        parsed.as_ref().is_some_and(is_bridge_owned_response);
                    if !bridge_owned_response {
                        upstream_reader_tui_tx
                            .send(Message::Text(text.to_string().into()))
                            .map_err(|_| anyhow!("tui writer channel closed"))?;
                    }
                    if let Some(value) = parsed {
                        let _ = observe_server_message(
                            &upstream_reader_state,
                            connection_epoch,
                            &value,
                        )
                        .await;
                    }
                }
                TungsteniteMessage::Binary(data) => {
                    upstream_reader_tui_tx
                        .send(Message::Binary(data.into()))
                        .map_err(|_| anyhow!("tui writer channel closed"))?;
                }
                TungsteniteMessage::Close(frame) => {
                    let _ = upstream_reader_tui_tx.send(Message::Close(frame.map(|f| {
                        axum::extract::ws::CloseFrame {
                            code: f.code.into(),
                            reason: f.reason.to_string().into(),
                        }
                    })));
                    break;
                }
                TungsteniteMessage::Ping(data) => {
                    upstream_reader_upstream_tx
                        .send(TungsteniteMessage::Pong(data))
                        .map_err(|_| anyhow!("upstream writer channel closed"))?;
                }
                TungsteniteMessage::Pong(_) | TungsteniteMessage::Frame(_) => {}
            }
        }
        Ok::<(), anyhow::Error>(())
    });

    let injector_upstream_tx = upstream_out_tx.clone();
    let mut injector = tokio::spawn(async move {
        while let Some(injected) = inject_rx.recv().await {
            injector_upstream_tx
                .send(TungsteniteMessage::Text(injected.to_string().into()))
                .map_err(|_| anyhow!("upstream writer channel closed"))?;
        }
        Ok::<(), anyhow::Error>(())
    });

    let mut upstream_writer = tokio::spawn(async move {
        while let Some(message) = upstream_out_rx.recv().await {
            upstream_write.send(message).await?;
        }
        Ok::<(), anyhow::Error>(())
    });

    let mut tui_writer = tokio::spawn(async move {
        while let Some(message) = tui_out_rx.recv().await {
            tui_write.send(message).await?;
        }
        Ok::<(), anyhow::Error>(())
    });

    tokio::select! {
        _ = &mut disconnect_rx => state.push_event("warn", "relay_tui_replaced", "closing superseded Codex TUI connection").await,
        result = &mut tui_reader => log_proxy_task_result(&state, "relay_tui_reader_finished", result).await,
        result = &mut upstream_reader => log_proxy_task_result(&state, "relay_upstream_reader_finished", result).await,
        result = &mut injector => log_proxy_task_result(&state, "relay_injector_finished", result).await,
        result = &mut upstream_writer => log_proxy_task_result(&state, "relay_upstream_writer_finished", result).await,
        result = &mut tui_writer => log_proxy_task_result(&state, "relay_tui_writer_finished", result).await,
    }

    tui_reader.abort();
    upstream_reader.abort();
    injector.abort();
    upstream_writer.abort();
    tui_writer.abort();

    {
        let mut relay = state.relay.inner.lock().await;
        if relay.connection_epoch == connection_epoch {
            relay.tui_connected = false;
            relay.upstream_connected = false;
            relay.current_turn_id = None;
            relay.inject_tx = None;
            relay.disconnect_tx = None;
            relay.client_request_methods.clear();
            relay.client_request_thread_ids.clear();
        }
    }
    if is_active_connection_epoch(&state, connection_epoch).await {
        state
            .push_event("warn", "relay_tui_disconnected", "codex TUI disconnected")
            .await;
    }
    Ok(())
}

async fn connect_upstream_app_server(
    upstream_url: &str,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
> {
    let request = upstream_url
        .into_client_request()
        .with_context(|| format!("invalid upstream websocket URL `{upstream_url}`"))?;
    let mut websocket_config = WebSocketConfig::default();
    websocket_config.max_frame_size = Some(REMOTE_APP_SERVER_MAX_WEBSOCKET_MESSAGE_SIZE);
    websocket_config.max_message_size = Some(REMOTE_APP_SERVER_MAX_WEBSOCKET_MESSAGE_SIZE);
    tokio::time::timeout(
        UPSTREAM_CONNECT_TIMEOUT,
        connect_async_with_config(
            request,
            Some(websocket_config),
            /*disable_nagle*/ false,
        ),
    )
    .await
    .map_err(|_| anyhow!("timed out connecting upstream app-server `{upstream_url}`"))?
    .map(|(stream, _response)| stream)
    .with_context(|| format!("failed to connect upstream app-server `{upstream_url}`"))
}

async fn log_proxy_task_result(
    state: &SharedState,
    kind: &str,
    result: std::result::Result<Result<()>, tokio::task::JoinError>,
) {
    match result {
        Ok(Ok(())) => {
            state.push_event("warn", kind, "closed").await;
        }
        Ok(Err(err)) => {
            state.push_event("error", kind, err.to_string()).await;
        }
        Err(err) => {
            state.push_event("error", kind, err.to_string()).await;
        }
    }
}

async fn observe_client_message(state: &SharedState, message: &Value) {
    let method = message
        .get("method")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    if !method.is_empty()
        && let Some(id) = message.get("id")
    {
        let mut relay = state.relay.inner.lock().await;
        relay
            .client_request_methods
            .insert(request_id_key(id), method.to_string());
        if let Some(thread_id) = message
            .get("params")
            .and_then(|v| v.get("threadId"))
            .and_then(|v| v.as_str())
        {
            relay
                .client_request_thread_ids
                .insert(request_id_key(id), thread_id.to_string());
        }
    }
    match method {
        "initialize"
        | "model/list"
        | "config/read"
        | "account/read"
        | "skills/list"
        | "mcpServerStatus/list"
        | "plugin/list"
        | "app/list"
        | "thread/loaded/list"
        | "thread/read"
        | "modelProvider/capabilities/read"
        | "configRequirements/read" => {
            state
                .push_event("info", "relay_client_request", format!("method={method}"))
                .await;
        }
        "thread/start" => {
            state
                .push_event("info", "relay_thread_start_requested", "")
                .await;
        }
        "turn/start" | "turn/steer" => {
            let params = message.get("params");
            if let Some(thread_id) = message
                .get("params")
                .and_then(|v| v.get("threadId"))
                .and_then(|v| v.as_str())
            {
                let mut relay = state.relay.inner.lock().await;
                relay.current_thread_id = Some(thread_id.to_string());
            }
            let text_len = params
                .and_then(|v| v.get("input"))
                .and_then(extract_turn_input_text)
                .map(|text| text.chars().count())
                .unwrap_or(0);
            state
                .push_event(
                    "info",
                    "relay_turn_start_requested",
                    format!("method={method} text_len={text_len}"),
                )
                .await;
        }
        _ => {}
    }
}

fn request_id_key(id: &Value) -> String {
    id.as_str()
        .map(str::to_string)
        .unwrap_or_else(|| id.to_string())
}

fn extract_turn_input_text(input: &Value) -> Option<String> {
    let items = input.as_array()?;
    let mut lines = Vec::new();
    for item in items {
        match item.get("type").and_then(|v| v.as_str()) {
            Some("text") => {
                if let Some(text) = item
                    .get("text")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    lines.push(text.to_string());
                }
            }
            Some("localImage") => {
                if let Some(path) = item
                    .get("path")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    lines.push(format!("[image] {path}"));
                }
            }
            _ => {}
        }
    }
    let text = lines.join("\n").trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn is_bridge_owned_response(message: &Value) -> bool {
    message
        .get("id")
        .and_then(Value::as_u64)
        .is_some_and(|request_id| request_id >= 100_000)
        && message.get("method").is_none()
}

async fn observe_server_message(
    state: &SharedState,
    connection_epoch: u64,
    message: &Value,
) -> bool {
    if !is_active_connection_epoch(state, connection_epoch).await {
        return false;
    }
    if let Some(id) = message.get("id") {
        if let Some(method) = message.get("method").and_then(|v| v.as_str()) {
            let params = message.get("params").cloned();
            let details = server_request_details(method, id, params.as_ref());
            state
                .push_event(
                    "info",
                    "relay_server_request",
                    format!("{details} epoch={connection_epoch}"),
                )
                .await;
            let _ = state.relay.notifications.send(CodexNotification {
                method: method.to_string(),
                params,
                request_id: Some(id.clone()),
            });
            return false;
        }

        let bridge_owned_response = id.as_u64().is_some_and(|request_id| request_id >= 100_000);
        let client_method = {
            let mut relay = state.relay.inner.lock().await;
            relay.client_request_methods.remove(&request_id_key(id))
        };
        let client_thread_id = {
            let mut relay = state.relay.inner.lock().await;
            relay.client_request_thread_ids.remove(&request_id_key(id))
        };
        if let Some(method) = client_method.as_deref() {
            state
                .push_event("info", "relay_response", format!("method={method} id={id}"))
                .await;
        }
        if let Some(result) = message.get("result") {
            if let Some(thread_id) = result
                .get("thread")
                .and_then(|v| v.get("id"))
                .and_then(|v| v.as_str())
            {
                let mut relay = state.relay.inner.lock().await;
                relay.current_thread_id = Some(thread_id.to_string());
                state
                    .push_event("info", "relay_thread_active", format!("thread={thread_id}"))
                    .await;
            }
            if !bridge_owned_response
                && client_method
                    .as_deref()
                    .is_some_and(|method| matches!(method, "turn/start" | "turn/steer"))
                && let Some(turn_id) = result
                    .get("turn")
                    .and_then(|v| v.get("id"))
                    .and_then(|v| v.as_str())
            {
                let thread_id = client_thread_id.or_else(|| {
                    result
                        .get("turn")
                        .and_then(|turn| turn.get("threadId"))
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                });
                let thread_id = match thread_id {
                    Some(thread_id) => Some(thread_id),
                    None => {
                        let relay = state.relay.inner.lock().await;
                        relay.current_thread_id.clone()
                    }
                };
                {
                    let mut relay = state.relay.inner.lock().await;
                    relay.current_turn_id = Some(turn_id.to_string());
                }
                if let Some(thread_id) = thread_id {
                    bind_default_route_for_thread(state, &thread_id).await;
                }
            }
        }
        if message.get("error").is_some() {
            state
                .push_event(
                    "error",
                    "relay_upstream_error",
                    format!("id={id} error={}", message["error"]),
                )
                .await;
        }
        if let Some(request_id) = id.as_u64()
            && bridge_owned_response
        {
            if let Some(tx) = state.relay.inner.lock().await.pending.remove(&request_id) {
                let result = if let Some(error) = message.get("error") {
                    Err(anyhow!("upstream request failed: {error}"))
                } else {
                    Ok(message.get("result").cloned().unwrap_or(Value::Null))
                };
                let _ = tx.send(result);
            }
            return true;
        }
        return false;
    }

    if let Some(method) = message.get("method").and_then(|v| v.as_str()) {
        let params = message.get("params").cloned();
        if method == "turn/started" {
            let thread_id = params
                .as_ref()
                .and_then(|p| p.get("threadId").and_then(|v| v.as_str()))
                .map(str::to_string);
            let turn_id = params
                .as_ref()
                .and_then(|p| {
                    p.get("turn")
                        .and_then(|turn| turn.get("id"))
                        .and_then(|v| v.as_str())
                        .or_else(|| p.get("turnId").and_then(|v| v.as_str()))
                })
                .map(str::to_string);
            if let Some(thread_id) = thread_id.as_deref() {
                bind_default_route_for_thread(state, thread_id).await;
            }
            if let Some(turn_id) = turn_id.as_deref() {
                let mut relay = state.relay.inner.lock().await;
                relay.current_turn_id = Some(turn_id.to_string());
                if let Some(thread_id) = thread_id {
                    relay.current_thread_id = Some(thread_id);
                }
            }
            state
                .push_event(
                    "info",
                    "relay_turn_started",
                    format!("turn={}", turn_id.unwrap_or_default()),
                )
                .await;
        } else if method == "turn/completed" {
            let turn_id = params
                .as_ref()
                .and_then(|p| {
                    p.get("turn")
                        .and_then(|turn| turn.get("id"))
                        .and_then(|v| v.as_str())
                        .or_else(|| p.get("turnId").and_then(|v| v.as_str()))
                })
                .unwrap_or_default()
                .to_string();
            let mut relay = state.relay.inner.lock().await;
            relay.current_turn_id = None;
            drop(relay);
            state
                .push_event("info", "relay_turn_completed", format!("turn={turn_id}"))
                .await;
        }
        state
            .push_event(
                "info",
                "relay_server_notification",
                format!("method={method}"),
            )
            .await;
        let request_id = message.get("id").cloned();
        let _ = state.relay.notifications.send(CodexNotification {
            method: method.to_string(),
            params,
            request_id,
        });
    }
    false
}

async fn is_active_connection_epoch(state: &SharedState, connection_epoch: u64) -> bool {
    state.relay.inner.lock().await.connection_epoch == connection_epoch
}

fn server_request_details(method: &str, id: &Value, params: Option<&Value>) -> String {
    let thread = params
        .and_then(|params| params.get("threadId").and_then(|v| v.as_str()))
        .unwrap_or_default();
    let turn = params
        .and_then(|params| params.get("turnId").and_then(|v| v.as_str()))
        .unwrap_or_default();
    let item = params
        .and_then(|params| params.get("itemId").and_then(|v| v.as_str()))
        .unwrap_or_default();
    let cwd = params
        .and_then(|params| params.get("cwd").and_then(|v| v.as_str()))
        .unwrap_or_default();
    let command = params.and_then(concise_request_command).unwrap_or_default();
    format!(
        "method={method} id={id} thread={thread} turn={turn} item={item} cwd={cwd} command={command}"
    )
}

fn concise_request_command(params: &Value) -> Option<String> {
    let command = params.get("command")?;
    if let Some(text) = command.as_str() {
        return Some(text.to_string());
    }
    command.as_array().map(|items| {
        items
            .iter()
            .filter_map(|item| item.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    })
}

async fn bind_default_route_for_thread(state: &SharedState, thread_id: &str) {
    let route = {
        let runtime = state.runtime.lock().await;
        runtime
            .route_for_thread(thread_id)
            .or_else(|| runtime.last_route.clone())
    };
    if let Some(route) = route {
        state.runtime.lock().await.bind_route(thread_id, route);
    }
}

pub async fn send_response(state: &SharedState, request_id: Value, result: Value) -> Result<()> {
    let inject_tx = {
        let relay = state.relay.inner.lock().await;
        relay
            .inject_tx
            .clone()
            .ok_or_else(|| anyhow!("relay injection channel is not connected"))?
    };
    inject_tx
        .send(json!({ "id": request_id, "result": result }))
        .await
        .map_err(|_| anyhow!("relay injection channel closed"))?;
    Ok(())
}

pub async fn interrupt_turn(state: &SharedState, thread_id: &str, turn_id: &str) -> Result<()> {
    request(
        state,
        "turn/interrupt",
        json!({
            "threadId": thread_id,
            "turnId": turn_id,
        }),
    )
    .await
    .map(|_| ())
}

pub async fn start_turn(
    state: &SharedState,
    thread_id: &str,
    text: &str,
    attachments: &[InboundAttachment],
) -> Result<String> {
    let mut params = json!({
        "threadId": thread_id,
        "input": turn_input_items(text, attachments),
    });
    prune_nulls(&mut params);
    let response = request(state, "turn/start", params).await?;
    let turn_id = response
        .get("turn")
        .and_then(|v| v.get("id"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("turn/start response missing turn.id: {response}"))?;
    {
        let mut relay = state.relay.inner.lock().await;
        relay.current_turn_id = Some(turn_id.clone());
    }
    Ok(turn_id)
}

fn turn_input_items(text: &str, attachments: &[InboundAttachment]) -> Vec<Value> {
    let mut items = Vec::new();
    let trimmed = text.trim();
    if !trimmed.is_empty() {
        items.push(json!({
            "type": "text",
            "text": trimmed,
            "text_elements": [],
        }));
    }
    for attachment in attachments {
        let Some(local_path) = attachment
            .local_path
            .as_ref()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        match attachment.kind.as_str() {
            "image" => items.push(json!({
                "type": "localImage",
                "path": local_path,
            })),
            "file" | "text" | "video" => items.push(json!({
                "type": "text",
                "text": format!("File: {local_path}"),
                "text_elements": [],
            })),
            _ => {}
        }
    }
    if items.is_empty() {
        items.push(json!({
            "type": "text",
            "text": "",
            "text_elements": [],
        }));
    }
    items
}

async fn request(state: &SharedState, method: &str, params: Value) -> Result<Value> {
    let id = next_request_id();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let inject_tx = {
        let mut relay = state.relay.inner.lock().await;
        let inject_tx = relay.inject_tx.clone().ok_or_else(|| {
            anyhow!(
                "Codex TUI 还没有连接。请先运行：codex --remote {}",
                relay.public_ws_url
            )
        })?;
        relay.pending.insert(id, tx);
        relay
            .client_request_methods
            .insert(id.to_string(), method.to_string());
        if let Some(thread_id) = params.get("threadId").and_then(|v| v.as_str()) {
            relay
                .client_request_thread_ids
                .insert(id.to_string(), thread_id.to_string());
        }
        inject_tx
    };
    inject_tx
        .send(json!({ "id": id, "method": method, "params": params }))
        .await
        .map_err(|_| anyhow!("relay injection channel closed"))?;
    rx.await
        .map_err(|_| anyhow!("relay response channel closed"))?
}

fn next_request_id() -> u64 {
    BRIDGE_REQUEST_ID.fetch_add(1, Ordering::Relaxed)
}

fn prune_nulls(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.retain(|_, v| !v.is_null());
            for v in map.values_mut() {
                prune_nulls(v);
            }
        }
        Value::Array(items) => {
            for item in items {
                prune_nulls(item);
            }
        }
        _ => {}
    }
}
