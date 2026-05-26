use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Result;
use tokio::{sync::mpsc, task::JoinSet};
use tracing::info;

use crate::{
    app_state::SharedState,
    codex::{
        approval_decision_by_input, approval_request_view, approval_response,
        extract_agent_message_text, extract_turn_reply_text, notification_thread_id,
    },
    im::feishu::{
        FeishuApi, FeishuSettings, renderer,
        runtime::{
            complete_existing_item_card, ensure_started_streaming_card_state,
            upsert_streaming_card_state,
        },
        ws::listen_ws,
    },
    im_runtime::{
        PendingApproval, RouteTarget, ThreadRoutingRequestState, TurnOrigin,
        route_from_conversation_key,
    },
    remote_control_backend,
    types::{InboundAction, InboundMessage, ThreadRouteDirection},
};

static THREAD_ROUTING_REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);
const THREAD_HISTORY_PAGE_SIZE: u32 = 8;
const THREAD_LOADED_LIMIT: u32 = 64;

pub async fn start_bridge(state: SharedState) {
    let config = state.config.lock().await.clone();
    if config.feishu.app_id.trim().is_empty() || config.feishu.app_secret.trim().is_empty() {
        state
            .push_event(
                "error",
                "bridge_config_invalid",
                "missing feishu app_id/app_secret",
            )
            .await;
        return;
    }

    let remote_status = remote_control_backend::status_snapshot(&state).await;
    if !remote_status.connected {
        state
            .push_event(
                "warn",
                "remote_control_not_connected",
                "Codex remote-control is not connected yet; Feishu listener will still start",
            )
            .await;
    }

    let (tx, mut rx) = mpsc::channel::<InboundMessage>(128);
    let api = FeishuApi::new(FeishuSettings::from_app_config(&config.feishu));
    let generation = state.runtime.lock().await.start_bridge_generation();
    install_default_feishu_route(
        &state,
        &config.bridge.account_id,
        &config.feishu.allowed_open_ids,
    )
    .await;
    let feishu_api_for_ws = api.clone();
    let account_id = config.bridge.account_id.clone();
    let attachment_root = attachment_root(&config.state_path);
    let ws_state = state.clone();
    let mut tasks = JoinSet::new();
    tasks.spawn(codex_event_router(state.clone(), api.clone(), generation));
    tasks.spawn(async move {
        loop {
            if !is_current_generation(&ws_state, generation).await {
                break;
            }
            {
                let mut ws = ws_state.feishu_ws.lock().await;
                ws.connecting = true;
                ws.connected = false;
                ws.last_error = None;
            }
            ws_state
                .push_event("info", "feishu_ws_connecting", "connecting")
                .await;
            match listen_ws(
                ws_state.clone(),
                feishu_api_for_ws.clone(),
                account_id.clone(),
                attachment_root.clone(),
                tx.clone(),
            )
            .await
            {
                Ok(()) => {
                    {
                        let mut ws = ws_state.feishu_ws.lock().await;
                        ws.connecting = false;
                        ws.connected = false;
                    }
                    ws_state
                        .push_event("warn", "feishu_ws_stopped", "websocket stopped")
                        .await;
                }
                Err(err) => {
                    let message = err.to_string();
                    {
                        let mut ws = ws_state.feishu_ws.lock().await;
                        ws.connecting = false;
                        ws.connected = false;
                        ws.last_error = Some(message.clone());
                    }
                    ws_state
                        .push_event("error", "feishu_ws_failed", message)
                        .await;
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    });

    state
        .push_event("info", "bridge_started", "bridge running")
        .await;
    loop {
        tokio::select! {
            message = rx.recv() => {
                let Some(message) = message else { break; };
                if !is_current_generation(&state, generation).await {
                    break;
                }
                let state = state.clone();
                let api = api.clone();
                tasks.spawn(async move {
                    if let Err(err) = handle_inbound(state.clone(), api, message).await {
                        state
                            .push_event("error", "inbound_failed", err.to_string())
                            .await;
                    }
                });
            }
            task = tasks.join_next() => {
                if let Some(Err(err)) = task {
                    state
                        .push_event("error", "bridge_task_failed", err.to_string())
                        .await;
                }
            }
        }
    }
    tasks.abort_all();
    state.runtime.lock().await.invalidate_bridge_generation();
}

async fn is_current_generation(state: &SharedState, generation: u64) -> bool {
    state.runtime.lock().await.is_bridge_generation(generation)
}

async fn install_default_feishu_route(
    state: &SharedState,
    account_id: &str,
    allowed_open_ids: &[String],
) {
    let Some(open_id) = allowed_open_ids
        .first()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    else {
        return;
    };
    let route = RouteTarget {
        conversation_key: format!("feishu:{account_id}:open_id:{open_id}"),
        account_id: account_id.to_string(),
        chat_id: format!("open_id:{open_id}"),
    };
    state.runtime.lock().await.last_route = Some(route);
    state
        .push_event("info", "feishu_default_route", format!("open_id={open_id}"))
        .await;
}

async fn handle_inbound(state: SharedState, api: FeishuApi, message: InboundMessage) -> Result<()> {
    info!(
        "inbound feishu message chat={} sender={}",
        message.chat_id, message.sender_id
    );
    state
        .push_event(
            "info",
            "feishu_message",
            format!(
                "chat={} sender={} text_len={} attachments={}",
                message.chat_id,
                message.sender_id,
                message.text.chars().count(),
                message.attachments.len()
            ),
        )
        .await;

    let trimmed = message.text.trim();
    let route = route_for_message(&message);
    {
        let mut runtime = state.runtime.lock().await;
        runtime.last_route = Some(route.clone());
    }
    if let Some(action) = message.action.clone() {
        return handle_inbound_action(state, api, message, action).await;
    }
    if handle_control_message(&state, &api, &message, trimmed).await? {
        return Ok(());
    }
    let remote_status = remote_control_backend::status_snapshot(&state).await;
    if !remote_status.connected {
        api.send_text_message(
            &message.chat_id,
            "Codex remote-control 还没有连接。请在项目目录运行 codex，确认它已经通过 remote-control 连接到 codex-remote。",
        )
        .await?;
        return Ok(());
    }

    let Some(thread_id) = resolve_thread_for_route(&state, &api, &message, &route).await? else {
        send_thread_routing_list(&state, &api, &message, None, None, 1).await?;
        return Ok(());
    };
    {
        let mut persisted = state.persisted.lock().await;
        persisted
            .sessions
            .insert(route.conversation_key.clone(), thread_id.clone());
        let config = state.config.lock().await.clone();
        persisted.save(&config.state_path)?;
    }
    {
        let mut runtime = state.runtime.lock().await;
        runtime.bind_route(&thread_id, route.clone());
    }

    let turn_id = match remote_control_backend::start_turn(
        &state,
        &thread_id,
        trimmed,
        &message.attachments,
    )
    .await
    {
        Ok(turn_id) => turn_id,
        Err(err) if is_stale_thread_error(&err) => {
            clear_thread_binding(&state, &route.conversation_key).await?;
            state
                .push_event(
                    "warn",
                    "thread_route_stale",
                    format!(
                        "conversation={} thread={} during=turn/start err={err}",
                        route.conversation_key, thread_id
                    ),
                )
                .await;
            send_thread_routing_list(&state, &api, &message, None, None, 1).await?;
            return Ok(());
        }
        Err(err) => {
            api.send_text_message(
                    &message.chat_id,
                    &format!(
                        "Codex App 没有接收这条消息：{err}\n\n请确认 Codex App 还打开着 remote-control，或发送 /threads 重新选择会话。"
                    ),
                )
                .await?;
            return Err(err);
        }
    };
    state
        .runtime
        .lock()
        .await
        .mark_turn_started(&thread_id, &turn_id);
    state
        .runtime
        .lock()
        .await
        .remember_turn_origin(&turn_id, TurnOrigin::Feishu);
    state
        .push_event(
            "info",
            "feishu_turn_started",
            format!(
                "chat={} thread={} turn={turn_id}",
                message.chat_id, thread_id
            ),
        )
        .await;
    Ok(())
}

fn attachment_root(state_path: &PathBuf) -> PathBuf {
    state_path
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".im")
        .join("attachments")
}

async fn handle_control_message(
    state: &SharedState,
    api: &FeishuApi,
    message: &InboundMessage,
    command: &str,
) -> Result<bool> {
    let normalized = command.to_ascii_lowercase();
    if is_approval_reply(&normalized) {
        return handle_approval_text_reply(state, api, message, command).await;
    }
    match normalized.as_str() {
        "/new" => {
            if let Some((_, turn_id)) = active_turn_for_message(state, message).await {
                api.send_text_message(
                    &message.chat_id,
                    &format!(
                        "当前任务仍在执行中（turn: {turn_id}）。请先发送 /s 中断，或等待完成。"
                    ),
                )
                .await?;
                return Ok(true);
            }
            {
                let mut runtime = state.runtime.lock().await;
                runtime.unbind_routes_for_conversation(&message.conversation_key());
            }
            let mut persisted = state.persisted.lock().await;
            persisted.sessions.remove(&message.conversation_key());
            let config = state.config.lock().await.clone();
            persisted.save(&config.state_path)?;
            api.send_text_message(
                &message.chat_id,
                "已解除当前绑定。下一条消息会先让你选择要接入的 thread。",
            )
            .await?;
            return Ok(true);
        }
        "/status" => {
            let text =
                if let Some((thread_id, turn_id)) = active_turn_for_message(state, message).await {
                    format!("thread: {thread_id}\n执行: 执行中\nturn: {turn_id}")
                } else if let Some(thread_id) =
                    live_thread_for_route(state, &route_for_message(message)).await
                {
                    format!("thread: {thread_id}\n执行: 空闲")
                } else {
                    "当前飞书会话还没有绑定任何 thread。".to_string()
                };
            api.send_text_message(&message.chat_id, &text).await?;
            return Ok(true);
        }
        "/threads" => {
            send_thread_routing_list(state, api, message, None, None, 1).await?;
            return Ok(true);
        }
        "/s" | "/stop" => {
            let Some((thread_id, turn_id)) = active_turn_for_message(state, message).await else {
                api.send_text_message(&message.chat_id, "当前没有运行中的 turn。")
                    .await?;
                return Ok(true);
            };
            remote_control_backend::interrupt_turn(state, &thread_id, &turn_id).await?;
            state
                .runtime
                .lock()
                .await
                .mark_turn_completed(&thread_id, Some(&turn_id));
            api.send_text_message(&message.chat_id, "已中断当前任务。")
                .await?;
            return Ok(true);
        }
        "/q" => {
            if let Some((thread_id, turn_id)) = active_turn_for_message(state, message).await {
                let _ = remote_control_backend::interrupt_turn(state, &thread_id, &turn_id).await;
                state
                    .runtime
                    .lock()
                    .await
                    .mark_turn_completed(&thread_id, Some(&turn_id));
            }
            {
                let mut runtime = state.runtime.lock().await;
                runtime.unbind_routes_for_conversation(&message.conversation_key());
            }
            let mut persisted = state.persisted.lock().await;
            persisted.sessions.remove(&message.conversation_key());
            let config = state.config.lock().await.clone();
            persisted.save(&config.state_path)?;
            api.send_text_message(&message.chat_id, "已退出当前会话。")
                .await?;
            return Ok(true);
        }
        _ => {}
    }
    Ok(false)
}

async fn handle_inbound_action(
    state: SharedState,
    api: FeishuApi,
    message: InboundMessage,
    action: InboundAction,
) -> Result<()> {
    match action {
        InboundAction::ThreadRouteResumeSelected {
            request_id,
            thread_id,
        } => {
            handle_thread_route_resume_selected(state, api, message, &request_id, &thread_id).await
        }
        InboundAction::ThreadRouteListPage {
            request_id,
            direction,
        } => handle_thread_route_list_page(state, api, message, &request_id, direction).await,
    }
}

fn is_approval_reply(command: &str) -> bool {
    matches!(command, "/y" | "/yes" | "/n" | "/no")
        || command
            .strip_prefix('/')
            .and_then(|value| value.parse::<usize>().ok())
            .is_some()
}

async fn handle_approval_text_reply(
    state: &SharedState,
    api: &FeishuApi,
    message: &InboundMessage,
    command: &str,
) -> Result<bool> {
    let message_conversation_key = message.conversation_key();
    let pending = {
        let runtime = state.runtime.lock().await;
        if let Some(request_key) = message.approval_request_key.as_deref() {
            runtime.approval_by_request_key_anywhere(request_key)
        } else {
            runtime
                .current_approval(&message_conversation_key)
                .map(|pending| (message_conversation_key.clone(), pending))
        }
    };
    let Some((conversation_key, pending)) = pending else {
        api.send_text_message(&message.chat_id, "当前没有待处理审批。")
            .await?;
        return Ok(true);
    };
    if let Some(request_key) = message.approval_request_key.as_deref() {
        let is_current = state
            .runtime
            .lock()
            .await
            .is_current_approval(&conversation_key, request_key);
        if !is_current {
            api.send_text_message(&message.chat_id, "请先处理当前显示的审批。")
                .await?;
            return Ok(true);
        }
    }
    let Some((index, decision)) = approval_decision_by_input(&pending, command) else {
        api.send_text_message(
            &message.chat_id,
            &format!(
                "无法识别审批选项。请回复 {}。",
                approval_reply_hint(&pending)
            ),
        )
        .await?;
        return Ok(true);
    };
    respond_to_pending_approval(
        state,
        api,
        &message.chat_id,
        &conversation_key,
        pending,
        index,
        decision,
    )
    .await?;
    Ok(true)
}

async fn handle_thread_route_resume_selected(
    state: SharedState,
    api: FeishuApi,
    message: InboundMessage,
    request_id: &str,
    thread_id: &str,
) -> Result<()> {
    let request = {
        state
            .runtime
            .lock()
            .await
            .thread_routing_request(request_id)
    };
    let Some(request) = request else {
        api.send_text_message(
            &message.chat_id,
            "这张 thread 选择卡片已经失效，请重新发送消息。",
        )
        .await?;
        return Ok(());
    };
    if request.conversation_key != message.conversation_key() {
        api.send_text_message(&message.chat_id, "这个 thread 选择不属于当前会话。")
            .await?;
        return Ok(());
    }

    let card_message_id = request
        .message_id
        .clone()
        .or(message.card_message_id.clone());
    if let Some(message_id) = card_message_id.as_deref() {
        let loading = renderer::build_thread_routing_result_card(
            "正在接入会话",
            &format!("正在订阅 thread `{thread_id}` 的后续事件..."),
        );
        let _ = api.update_interactive_message(message_id, &loading).await;
    }

    let response = remote_control_backend::resume_thread(&state, thread_id, true).await?;
    let thread = response
        .get("thread")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let route = RouteTarget {
        conversation_key: message.conversation_key(),
        account_id: message.account_id.clone(),
        chat_id: message.chat_id.clone(),
    };
    {
        let mut runtime = state.runtime.lock().await;
        runtime.unbind_routes_for_conversation(&route.conversation_key);
        runtime.bind_route(thread_id, route.clone());
        runtime.clear_thread_routing_request(request_id);
    }
    {
        let mut persisted = state.persisted.lock().await;
        persisted
            .sessions
            .insert(route.conversation_key.clone(), thread_id.to_string());
        let config = state.config.lock().await.clone();
        persisted.save(&config.state_path)?;
    }
    let body = format!(
        "已接入 thread `{thread_id}`。\n\n{}\n{}\n{}",
        summarize_thread_title(&thread),
        summarize_thread_cwd(&thread),
        summarize_thread_status(&thread)
    );
    let card = renderer::build_thread_routing_result_card("已订阅会话", &body);
    if let Some(message_id) = card_message_id.as_deref() {
        let _ = api.update_interactive_message(message_id, &card).await;
    } else {
        let _ = send_interactive_to_target(&api, &route.chat_id, &card).await?;
    }
    state
        .push_event(
            "info",
            "thread_route_resumed",
            format!("conversation={} thread={thread_id}", route.conversation_key),
        )
        .await;
    Ok(())
}

async fn handle_thread_route_list_page(
    state: SharedState,
    api: FeishuApi,
    message: InboundMessage,
    request_id: &str,
    direction: ThreadRouteDirection,
) -> Result<()> {
    let request = {
        state
            .runtime
            .lock()
            .await
            .thread_routing_request(request_id)
    };
    let Some(request) = request else {
        api.send_text_message(
            &message.chat_id,
            "这张 thread 选择卡片已经失效，请重新发送消息。",
        )
        .await?;
        return Ok(());
    };
    if request.conversation_key != message.conversation_key() {
        api.send_text_message(&message.chat_id, "这个 thread 列表不属于当前会话。")
            .await?;
        return Ok(());
    }

    let target_page = match direction {
        ThreadRouteDirection::Prev => request.page.saturating_sub(1).max(1),
        ThreadRouteDirection::Next => request.page.saturating_add(1),
    };
    let cursor = request
        .page_cursors
        .get(target_page.saturating_sub(1))
        .cloned()
        .flatten();
    send_thread_routing_list(
        &state,
        &api,
        &message,
        Some(request),
        cursor.as_deref(),
        target_page,
    )
    .await
}

async fn respond_to_pending_approval(
    state: &SharedState,
    api: &FeishuApi,
    chat_id: &str,
    conversation_key: &str,
    pending: PendingApproval,
    option_index: usize,
    decision: crate::im_runtime::ApprovalDecisionOption,
) -> Result<()> {
    let response = approval_response(decision.decision);
    remote_control_backend::send_response(state, pending.request_id.clone(), response).await?;
    update_resolved_approval_card(api, &pending, option_index, &decision.label).await;
    let next = state
        .runtime
        .lock()
        .await
        .resolve_approval_request_with_context(&pending.request_id)
        .and_then(|resolved| {
            resolved
                .next_current
                .map(|next| (resolved.conversation_key, next))
        });
    let _ = chat_id;
    state
        .push_event(
            "info",
            "approval_decision_sent",
            format!(
                "conversation={} request_id={} option={} label={}",
                conversation_key, pending.request_id, option_index, decision.label
            ),
        )
        .await;
    if let Some((conversation_key, next_approval)) = next {
        send_next_approval_card(state, api, &conversation_key, &next_approval).await?;
    }
    Ok(())
}

async fn update_resolved_approval_card(
    api: &FeishuApi,
    pending: &PendingApproval,
    option_index: usize,
    decision_label: &str,
) {
    let Some(message_id) = pending.feishu_message_id.as_deref() else {
        return;
    };
    let card = renderer::build_resolved_approval_card(
        approval_kind_label(&pending.request_kind),
        &pending.summary,
        decision_label,
        option_index,
    );
    let _ = api.update_interactive_message(message_id, &card).await;
}

async fn send_next_approval_card(
    state: &SharedState,
    api: &FeishuApi,
    conversation_key: &str,
    approval: &PendingApproval,
) -> Result<()> {
    let Some(route) = route_from_conversation_key(conversation_key) else {
        state
            .push_event(
                "warn",
                "approval_next_route_missing",
                format!("conversation={conversation_key}"),
            )
            .await;
        return Ok(());
    };
    send_approval_card(state, api, &route, approval).await
}

async fn send_approval_card(
    state: &SharedState,
    api: &FeishuApi,
    route: &RouteTarget,
    approval: &PendingApproval,
) -> Result<()> {
    let request_key = approval.request_key();
    let card = renderer::build_approval_card(
        approval_kind_label(&approval.request_kind),
        &approval.summary,
        &approval.decisions,
        &request_key,
    );
    let message_id = send_interactive_to_target(api, &route.chat_id, &card).await?;
    state
        .runtime
        .lock()
        .await
        .remember_approval_message_id(&approval.request_id, message_id.clone());
    state
        .push_event(
            "info",
            "approval_card_sent",
            format!(
                "conversation={} request_id={} message={}",
                route.conversation_key, approval.request_id, message_id
            ),
        )
        .await;
    Ok(())
}

async fn send_thread_routing_list(
    state: &SharedState,
    api: &FeishuApi,
    message: &InboundMessage,
    existing_request: Option<ThreadRoutingRequestState>,
    cursor: Option<&str>,
    page: usize,
) -> Result<()> {
    let route = RouteTarget {
        conversation_key: message.conversation_key(),
        account_id: message.account_id.clone(),
        chat_id: message.chat_id.clone(),
    };
    let request_id = existing_request
        .as_ref()
        .map(|request| request.request_id.clone())
        .unwrap_or_else(next_thread_routing_request_id);
    let mut page_cursors = existing_request
        .as_ref()
        .map(|request| request.page_cursors.clone())
        .unwrap_or_else(|| vec![None]);
    if page_cursors.len() < page {
        page_cursors.resize(page, None);
    }
    page_cursors[page - 1] = cursor.map(str::to_string);

    let loaded =
        remote_control_backend::thread_loaded_list(state, None, Some(THREAD_LOADED_LIMIT)).await?;
    let history =
        remote_control_backend::thread_list(state, cursor, Some(THREAD_HISTORY_PAGE_SIZE), None)
            .await?;
    let loaded_ids = loaded
        .get("data")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect::<Vec<_>>();
    let history_threads = history
        .get("data")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let entries = build_thread_entries(&loaded_ids, &history_threads);
    let next_cursor = history
        .get("nextCursor")
        .or_else(|| history.get("next_cursor"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    if page_cursors.len() <= page {
        page_cursors.resize(page + 1, None);
    }
    page_cursors[page] = next_cursor.clone();

    let card = renderer::build_thread_list_card(
        &request_id,
        "选择 Codex 会话",
        "当前飞书会话还没有订阅任何 Codex thread。请选择一个会话接入后续事件。",
        &entries,
        page,
        page > 1,
        next_cursor.is_some(),
    );
    let message_id = if let Some(message_id) = existing_request
        .as_ref()
        .and_then(|request| request.message_id.clone())
    {
        api.update_interactive_message(&message_id, &card).await?;
        message_id
    } else {
        send_interactive_to_target(api, &route.chat_id, &card).await?
    };
    {
        let mut runtime = state.runtime.lock().await;
        runtime.remember_thread_routing_request(ThreadRoutingRequestState {
            request_id: request_id.clone(),
            conversation_key: route.conversation_key.clone(),
            account_id: route.account_id.clone(),
            chat_id: route.chat_id.clone(),
            message_id: Some(message_id.clone()),
            page,
            page_cursors,
            history_cursor: cursor.map(str::to_string),
            history_has_next: next_cursor.is_some(),
        });
    }
    state
        .push_event(
            "info",
            "thread_route_list_sent",
            format!(
                "conversation={} page={page} entries={}",
                route.conversation_key,
                entries.len()
            ),
        )
        .await;
    Ok(())
}

fn next_thread_routing_request_id() -> String {
    let value = THREAD_ROUTING_REQUEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("thread-route-{value}")
}

fn build_thread_entries(
    loaded_ids: &[String],
    history_threads: &[serde_json::Value],
) -> Vec<renderer::FeishuThreadListEntry> {
    let loaded_set = loaded_ids
        .iter()
        .cloned()
        .collect::<std::collections::HashSet<_>>();
    let mut entries = history_threads
        .iter()
        .map(|thread| renderer::FeishuThreadListEntry {
            thread_id: thread
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            title: summarize_thread_title(thread),
            summary: Some(summarize_thread_preview(thread)),
            last_activity_text: Some(format!(
                "{} · {}",
                summarize_thread_status(thread),
                summarize_thread_cwd(thread)
            )),
        })
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| {
        (
            !loaded_set.contains(&entry.thread_id),
            entry.thread_id.clone(),
        )
    });
    entries
}

fn summarize_thread_title(thread: &serde_json::Value) -> String {
    thread
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .or_else(|| {
            thread
                .get("preview")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(|v| truncate_text(v, 80))
        })
        .unwrap_or_else(|| {
            let thread_id = thread
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            format!("会话 {thread_id}")
        })
}

fn summarize_thread_preview(thread: &serde_json::Value) -> String {
    thread
        .get("preview")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(|v| truncate_text(v, 120))
        .unwrap_or_else(|| "无预览".to_string())
}

fn summarize_thread_cwd(thread: &serde_json::Value) -> String {
    let cwd = thread
        .get("cwd")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    if cwd.is_empty() {
        "目录未知".to_string()
    } else {
        format!("目录：`{cwd}`")
    }
}

fn summarize_thread_status(thread: &serde_json::Value) -> String {
    match thread
        .get("status")
        .and_then(|v| v.get("type"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
    {
        "active" => "运行中".to_string(),
        "idle" => "空闲".to_string(),
        "notLoaded" => "未加载".to_string(),
        "systemError" => "系统错误".to_string(),
        other => other.to_string(),
    }
}

fn truncate_text(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (index, ch) in text.chars().enumerate() {
        if index >= max_chars {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

fn approval_reply_hint(pending: &PendingApproval) -> String {
    let options = pending
        .decisions
        .iter()
        .enumerate()
        .map(|(index, _)| format!("/{}", index + 1))
        .collect::<Vec<_>>();
    if options.is_empty() {
        "`/y` 或 `/n`".to_string()
    } else {
        options.join("、")
    }
}

async fn active_turn_for_message(
    state: &SharedState,
    message: &InboundMessage,
) -> Option<(String, String)> {
    let route = route_for_message(message);
    let thread_id = live_thread_for_route(state, &route).await?;
    let runtime = state.runtime.lock().await;
    let turn_id = runtime.current_turn_by_thread.get(&thread_id)?.clone();
    Some((thread_id, turn_id))
}

fn route_for_message(message: &InboundMessage) -> RouteTarget {
    RouteTarget {
        conversation_key: message.conversation_key(),
        account_id: message.account_id.clone(),
        chat_id: message.chat_id.clone(),
    }
}

async fn resolve_thread_for_route(
    state: &SharedState,
    api: &FeishuApi,
    message: &InboundMessage,
    route: &RouteTarget,
) -> Result<Option<String>> {
    if let Some(thread_id) = live_thread_for_route(state, route).await {
        return Ok(Some(thread_id));
    }

    let persisted_thread_id = {
        state
            .persisted
            .lock()
            .await
            .sessions
            .get(&route.conversation_key)
            .cloned()
    };
    let Some(thread_id) = persisted_thread_id else {
        return Ok(None);
    };

    match remote_control_backend::resume_thread(state, &thread_id, true).await {
        Ok(_) => {
            {
                let mut runtime = state.runtime.lock().await;
                runtime.bind_route(&thread_id, route.clone());
            }
            state
                .push_event(
                    "info",
                    "thread_route_restored",
                    format!("conversation={} thread={thread_id}", route.conversation_key),
                )
                .await;
            Ok(Some(thread_id))
        }
        Err(err) if is_stale_thread_error(&err) => {
            clear_thread_binding(state, &route.conversation_key).await?;
            state
                .push_event(
                    "warn",
                    "thread_route_stale",
                    format!(
                        "conversation={} thread={} during=thread/resume err={err}",
                        route.conversation_key, thread_id
                    ),
                )
                .await;
            Ok(None)
        }
        Err(err) => {
            api.send_text_message(
                &message.chat_id,
                &format!(
                    "无法恢复 Codex thread `{thread_id}`：{err}\n\n请确认 Codex App 还打开着 remote-control，或发送 /threads 重新选择会话。"
                ),
            )
            .await?;
            Err(err)
        }
    }
}

async fn live_thread_for_route(state: &SharedState, route: &RouteTarget) -> Option<String> {
    state
        .runtime
        .lock()
        .await
        .route_by_thread
        .iter()
        .find_map(|(thread_id, existing_route)| {
            (existing_route.conversation_key == route.conversation_key).then(|| thread_id.clone())
        })
}

async fn clear_thread_binding(state: &SharedState, conversation_key: &str) -> Result<()> {
    {
        let mut runtime = state.runtime.lock().await;
        runtime.unbind_routes_for_conversation(conversation_key);
    }
    let mut persisted = state.persisted.lock().await;
    persisted.sessions.remove(conversation_key);
    let config = state.config.lock().await.clone();
    persisted.save(&config.state_path)?;
    Ok(())
}

fn is_stale_thread_error(err: &anyhow::Error) -> bool {
    let message = err.to_string();
    message.contains("thread not found") || message.contains("is closing")
}

async fn codex_event_router(state: SharedState, api: FeishuApi, generation: u64) {
    let mut rx = remote_control_backend::subscribe(&state);
    loop {
        if !is_current_generation(&state, generation).await {
            break;
        }
        let notification = match rx.recv().await {
            Ok(notification) => notification,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                state
                    .push_event(
                        "warn",
                        "codex_event_router_lagged",
                        format!("skipped={skipped}"),
                    )
                    .await;
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                state
                    .push_event(
                        "warn",
                        "codex_event_router_closed",
                        "notification channel closed",
                    )
                    .await;
                break;
            }
        };
        if !is_current_generation(&state, generation).await {
            break;
        }
        handle_codex_notification_for_feishu(state.clone(), api.clone(), &notification).await;
        if notification.method == "serverRequest/resolved" {
            let request_id = notification
                .params
                .as_ref()
                .and_then(|params| params.get("requestId"));
            if let Some(request_id) = request_id {
                let resolved = state
                    .runtime
                    .lock()
                    .await
                    .resolve_approval_request_with_context(request_id);
                if let Some(resolved) = resolved {
                    if resolved.was_current {
                        if let Some((conversation_key, next_approval)) = resolved
                            .next_current
                            .map(|next| (resolved.conversation_key.clone(), next))
                        {
                            let _ = send_next_approval_card(
                                &state,
                                &api,
                                &conversation_key,
                                &next_approval,
                            )
                            .await;
                        }
                    }
                    state
                        .push_event(
                            "info",
                            "approval_resolved",
                            format!("request_id={request_id}"),
                        )
                        .await;
                }
            }
            continue;
        }
        let Some(request_id) = notification.request_id.clone() else {
            continue;
        };
        if let Some(view) = approval_request_view(&notification) {
            let request_kind = view.request_kind;
            let summary = view.summary;
            let decisions = view.decisions;
            let Some(thread_id) = notification_thread_id(&notification) else {
                state
                    .push_event(
                        "warn",
                        "approval_no_thread",
                        format!("kind={request_kind} request_id={request_id}"),
                    )
                    .await;
                continue;
            };
            let route = { state.runtime.lock().await.route_for_thread(&thread_id) };
            let Some(route) = route else {
                state
                    .push_event(
                        "warn",
                        "approval_no_route",
                        format!("thread={thread_id} kind={request_kind}"),
                    )
                    .await;
                continue;
            };
            let (should_send_now, approval) = {
                let mut runtime = state.runtime.lock().await;
                runtime.bind_route(&thread_id, route.clone());
                let should_send_now = !runtime.has_pending_approvals(&route.conversation_key);
                let approval = PendingApproval {
                    request_id: request_id.clone(),
                    request_kind: request_kind.clone(),
                    method: notification.method.clone(),
                    params: notification
                        .params
                        .clone()
                        .unwrap_or(serde_json::Value::Null),
                    summary: summary.clone(),
                    decisions: decisions.clone(),
                    feishu_message_id: None,
                };
                if !runtime.push_approval(route.conversation_key.clone(), approval.clone()) {
                    drop(runtime);
                    state
                        .push_event(
                            "info",
                            "approval_replayed",
                            format!("thread={thread_id} request_id={request_id}"),
                        )
                        .await;
                    continue;
                }
                (should_send_now, approval)
            };
            if should_send_now {
                let _ = send_approval_card(&state, &api, &route, &approval).await;
            }
            let approval_details = approval_event_details(
                &request_kind,
                &request_id,
                notification.params.as_ref(),
                &summary,
            );
            state
                .push_event(
                    "info",
                    "approval_requested",
                    format!(
                        "thread={thread_id} account={} chat={} {approval_details}",
                        route.account_id, route.chat_id
                    ),
                )
                .await;
            continue;
        }
    }
}

async fn handle_codex_notification_for_feishu(
    state: SharedState,
    api: FeishuApi,
    notification: &crate::codex::CodexNotification,
) {
    let Some(params) = notification.params.as_ref() else {
        return;
    };
    match notification.method.as_str() {
        "turn/started" => {
            let Some(thread_id) = params.get("threadId").and_then(|v| v.as_str()) else {
                return;
            };
            let Some(turn_id) = params
                .get("turn")
                .and_then(|v| v.get("id"))
                .and_then(|v| v.as_str())
                .or_else(|| params.get("turnId").and_then(|v| v.as_str()))
            else {
                return;
            };
            if route_for_codex_output(&state, thread_id, params)
                .await
                .is_some()
            {
                state
                    .runtime
                    .lock()
                    .await
                    .mark_turn_started(thread_id, turn_id);
            }
        }
        "thread/started" => {}
        "thread/status/changed" => {}
        "item/started" => {
            let Some(thread_id) = params.get("threadId").and_then(|v| v.as_str()) else {
                return;
            };
            let Some(item) = params.get("item") else {
                return;
            };
            let Some(item_type) = item.get("type").and_then(|v| v.as_str()) else {
                return;
            };
            let Some(kind) = structured_streaming_kind(item_type) else {
                return;
            };
            let Some(item_id) = item.get("id").and_then(|v| v.as_str()) else {
                return;
            };
            let route = route_for_codex_output(&state, thread_id, params).await;
            let Some(route) = route else {
                return;
            };
            let initial_text = if kind == "commandExecution" {
                command_execution_started_text(item)
                    .or_else(|| renderer::item_markdown_summary(item))
            } else {
                renderer::item_markdown_summary(item)
            };
            ensure_started_streaming_card_state(
                state,
                api,
                thread_id,
                item_id,
                kind,
                &route.account_id,
                &route.chat_id,
                initial_text,
            )
            .await;
        }
        "item/agentMessage/delta" => {
            let Some(thread_id) = params.get("threadId").and_then(|v| v.as_str()) else {
                return;
            };
            let Some(item_id) = params.get("itemId").and_then(|v| v.as_str()) else {
                return;
            };
            let Some(delta) = params.get("delta").and_then(|v| v.as_str()) else {
                return;
            };
            let route = route_for_codex_output(&state, thread_id, params).await;
            let Some(route) = route else {
                state
                    .push_event(
                        "info",
                        "feishu_stream_skipped",
                        format!("thread={thread_id} reason=no_binding"),
                    )
                    .await;
                return;
            };
            upsert_streaming_card_state(
                state,
                api,
                thread_id,
                item_id,
                "agentMessage",
                &route.account_id,
                &route.chat_id,
                delta,
                false,
            )
            .await;
        }
        "item/reasoning/textDelta" | "item/reasoning/summaryTextDelta" => {
            let Some(thread_id) = params.get("threadId").and_then(|v| v.as_str()) else {
                return;
            };
            let Some(item_id) = params.get("itemId").and_then(|v| v.as_str()) else {
                return;
            };
            let Some(delta) = params.get("delta").and_then(|v| v.as_str()) else {
                return;
            };
            let route = route_for_codex_output(&state, thread_id, params).await;
            let Some(route) = route else {
                return;
            };
            upsert_streaming_card_state(
                state,
                api,
                thread_id,
                item_id,
                "reasoning",
                &route.account_id,
                &route.chat_id,
                delta,
                false,
            )
            .await;
        }
        "item/plan/delta" => {
            let Some(thread_id) = params.get("threadId").and_then(|v| v.as_str()) else {
                return;
            };
            let Some(item_id) = params.get("itemId").and_then(|v| v.as_str()) else {
                return;
            };
            let Some(delta) = params.get("delta").and_then(|v| v.as_str()) else {
                return;
            };
            let route = route_for_codex_output(&state, thread_id, params).await;
            let Some(route) = route else {
                return;
            };
            upsert_streaming_card_state(
                state,
                api,
                thread_id,
                item_id,
                "plan",
                &route.account_id,
                &route.chat_id,
                delta,
                false,
            )
            .await;
        }
        "item/commandExecution/outputDelta" | "item/fileChange/outputDelta" => {
            let Some(thread_id) = params.get("threadId").and_then(|v| v.as_str()) else {
                return;
            };
            let Some(item_id) = params.get("itemId").and_then(|v| v.as_str()) else {
                return;
            };
            let Some(delta) = params.get("delta").and_then(|v| v.as_str()) else {
                return;
            };
            let kind = if notification.method == "item/commandExecution/outputDelta" {
                "commandExecution"
            } else {
                "fileChange"
            };
            let route = route_for_codex_output(&state, thread_id, params).await;
            let Some(route) = route else {
                return;
            };
            upsert_streaming_card_state(
                state,
                api,
                thread_id,
                item_id,
                kind,
                &route.account_id,
                &route.chat_id,
                delta,
                false,
            )
            .await;
        }
        "item/mcpToolCall/progress" => {
            let Some(thread_id) = params.get("threadId").and_then(|v| v.as_str()) else {
                return;
            };
            let Some(item_id) = params.get("itemId").and_then(|v| v.as_str()) else {
                return;
            };
            let Some(message) = params.get("message").and_then(|v| v.as_str()) else {
                return;
            };
            let route = route_for_codex_output(&state, thread_id, params).await;
            let Some(route) = route else {
                return;
            };
            upsert_streaming_card_state(
                state,
                api,
                thread_id,
                item_id,
                "mcpToolCall",
                &route.account_id,
                &route.chat_id,
                message,
                false,
            )
            .await;
        }
        "item/updated" => {
            let Some(thread_id) = params.get("threadId").and_then(|v| v.as_str()) else {
                return;
            };
            let Some(item) = params.get("item") else {
                return;
            };
            let Some(item_type) = item.get("type").and_then(|v| v.as_str()) else {
                return;
            };
            let Some(kind) = structured_streaming_kind(item_type) else {
                return;
            };
            let Some(item_id) = item.get("id").and_then(|v| v.as_str()) else {
                return;
            };
            let route = route_for_codex_output(&state, thread_id, params).await;
            let Some(route) = route else {
                return;
            };
            let initial_text = if item_type == "commandExecution" {
                command_execution_full_text(item).or_else(|| renderer::item_markdown_summary(item))
            } else {
                renderer::item_markdown_summary(item)
            };
            ensure_started_streaming_card_state(
                state,
                api,
                thread_id,
                item_id,
                kind,
                &route.account_id,
                &route.chat_id,
                initial_text,
            )
            .await;
        }
        "item/completed" => {
            let Some(thread_id) = params.get("threadId").and_then(|v| v.as_str()) else {
                return;
            };
            let turn_id = params.get("turnId").and_then(|v| v.as_str());
            let Some(item) = params.get("item") else {
                return;
            };
            let Some(item_type) = item.get("type").and_then(|v| v.as_str()) else {
                return;
            };
            let Some(item_id) = item.get("id").and_then(|v| v.as_str()) else {
                return;
            };
            let route = route_for_codex_output(&state, thread_id, params).await;
            let Some(route) = route else {
                return;
            };
            let kind = structured_streaming_kind(item_type).unwrap_or(item_type);
            let text = if item_type == "agentMessage" {
                extract_agent_message_text(item)
            } else if item_type == "commandExecution" {
                command_execution_full_text(item).or_else(|| renderer::item_markdown_summary(item))
            } else {
                renderer::item_markdown_summary(item)
            };
            if matches!(
                item_type,
                "agentMessage"
                    | "reasoning"
                    | "plan"
                    | "commandExecution"
                    | "fileChange"
                    | "mcpToolCall"
            ) {
                let updated = complete_existing_item_card(
                    state.clone(),
                    api.clone(),
                    thread_id,
                    item_id,
                    kind,
                    &route.account_id,
                    &route.chat_id,
                    text.clone(),
                )
                .await;
                if updated {
                    return;
                }
                if let Some(text) = text {
                    upsert_streaming_card_state(
                        state,
                        api,
                        thread_id,
                        item_id,
                        kind,
                        &route.account_id,
                        &route.chat_id,
                        &text,
                        true,
                    )
                    .await;
                }
            } else if item_type == "userMessage" {
                let should_forward = if let Some(turn_id) = turn_id {
                    state.runtime.lock().await.turn_origin(turn_id) != Some(TurnOrigin::Feishu)
                } else {
                    true
                };
                if !should_forward {
                    state
                        .push_event(
                            "info",
                            "feishu_user_message_suppressed",
                            format!(
                                "thread={thread_id} item={item_id} turn={} chat={}",
                                turn_id.unwrap_or(""),
                                route.chat_id
                            ),
                        )
                        .await;
                    return;
                }
                if let Some(card) = renderer::build_item_card(item) {
                    if let Err(err) = send_interactive_to_target(&api, &route.chat_id, &card).await
                    {
                        state
                            .push_event(
                                "error",
                                "feishu_item_card_failed",
                                format!(
                                    "thread={thread_id} item={item_id} chat={} err={err}",
                                    route.chat_id
                                ),
                            )
                            .await;
                    }
                }
            } else if let Some(card) = renderer::build_item_card(item) {
                if let Err(err) = send_interactive_to_target(&api, &route.chat_id, &card).await {
                    state
                        .push_event(
                            "error",
                            "feishu_item_card_failed",
                            format!(
                                "thread={thread_id} item={item_id} chat={} err={err}",
                                route.chat_id
                            ),
                        )
                        .await;
                }
            }
        }
        "turn/completed" | "codex/event/turn_completed" => {
            let Some(thread_id) = params.get("threadId").and_then(|v| v.as_str()) else {
                return;
            };
            let turn_id = params
                .get("turn")
                .and_then(|turn| turn.get("id"))
                .and_then(|v| v.as_str())
                .or_else(|| params.get("turnId").and_then(|v| v.as_str()));
            state
                .runtime
                .lock()
                .await
                .mark_turn_completed(thread_id, turn_id);
            let Some(text) = extract_turn_reply_text(params) else {
                return;
            };
            let route = route_for_codex_output(&state, thread_id, params).await;
            let Some(route) = route else {
                return;
            };
            let should_send = {
                let mut runtime = state.runtime.lock().await;
                let key = format!("{}:turn-reply", route.conversation_key);
                if runtime.should_skip_duplicate_text(&key, &text) {
                    false
                } else {
                    runtime.remember_sent_text(&key, &text);
                    true
                }
            };
            if should_send {
                let card = renderer::build_turn_completed_card(&text);
                if let Err(err) = send_interactive_to_target(&api, &route.chat_id, &card).await {
                    state
                        .push_event(
                            "error",
                            "feishu_turn_completed_failed",
                            format!("thread={thread_id} chat={} err={err}", route.chat_id),
                        )
                        .await;
                }
            }
        }
        _ => {}
    }
}

async fn route_for_codex_output(
    state: &SharedState,
    thread_id: &str,
    _params: &serde_json::Value,
) -> Option<RouteTarget> {
    if let Some(route) = state.runtime.lock().await.route_for_thread(thread_id) {
        return Some(route);
    }
    None
}

fn structured_streaming_kind(item_type: &str) -> Option<&'static str> {
    match item_type {
        "agentMessage" => Some("agentMessage"),
        "reasoning" => Some("reasoning"),
        "plan" => Some("plan"),
        "commandExecution" => Some("commandExecution"),
        "fileChange" => Some("fileChange"),
        "mcpToolCall" => Some("mcpToolCall"),
        _ => None,
    }
}

fn command_execution_started_text(item: &serde_json::Value) -> Option<String> {
    let command = item
        .get("commandActions")
        .and_then(|v| v.as_array())
        .and_then(|actions| actions.first())
        .and_then(|action| action.get("command"))
        .and_then(|v| v.as_str())
        .or_else(|| item.get("command").and_then(|v| v.as_str()))
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    Some(format!(
        "__COMMAND__\n{}\n__OUTPUT__\n\n__META__\nStatus: in_progress",
        command
    ))
}

fn command_execution_full_text(item: &serde_json::Value) -> Option<String> {
    let command = item
        .get("commandActions")
        .and_then(|v| v.as_array())
        .and_then(|actions| actions.first())
        .and_then(|action| action.get("command"))
        .and_then(|v| v.as_str())
        .or_else(|| item.get("command").and_then(|v| v.as_str()))
        .map(str::trim)
        .filter(|value| !value.is_empty())?;

    let output = item
        .get("aggregatedOutput")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .trim()
        .to_string();

    let mut meta = Vec::new();
    if let Some(status) = item
        .get("status")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        meta.push(format!("Status: {}", status));
    }
    if let Some(exit_code) = item.get("exitCode").and_then(|v| v.as_i64()) {
        meta.push(format!("exit {}", exit_code));
    }
    if let Some(duration_ms) = item.get("durationMs").and_then(|v| v.as_u64()) {
        meta.push(format!("{}ms", duration_ms));
    }

    Some(format!(
        "__COMMAND__\n{}\n__OUTPUT__\n{}\n__META__\n{}",
        command,
        output,
        meta.join(" · ")
    ))
}

fn approval_kind_label(kind: &str) -> &str {
    match kind {
        "command" => "命令执行",
        "fileChange" => "文件修改",
        "review" => "补丁审查",
        other => other,
    }
}

fn approval_event_details(
    request_kind: &str,
    request_id: &serde_json::Value,
    params: Option<&serde_json::Value>,
    summary: &str,
) -> String {
    let turn = params
        .and_then(|params| params.get("turnId").and_then(|v| v.as_str()))
        .unwrap_or_default();
    let item = params
        .and_then(|params| params.get("itemId").and_then(|v| v.as_str()))
        .unwrap_or_default();
    let cwd = params
        .and_then(|params| params.get("cwd").and_then(|v| v.as_str()))
        .unwrap_or_default();
    let command = params
        .and_then(|params| {
            params.get("command").and_then(|command| {
                if let Some(text) = command.as_str() {
                    Some(text.to_string())
                } else {
                    command.as_array().map(|items| {
                        items
                            .iter()
                            .filter_map(|item| item.as_str())
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                }
            })
        })
        .or_else(|| {
            summary
                .lines()
                .find_map(|line| line.strip_prefix("命令：`"))
                .and_then(|line| line.strip_suffix('`'))
                .map(str::to_string)
        })
        .unwrap_or_default();
    format!(
        "kind={request_kind} request_id={request_id} turn={turn} item={item} cwd={cwd} command={command}"
    )
}

async fn send_interactive_to_target(
    api: &FeishuApi,
    target: &str,
    card: &serde_json::Value,
) -> Result<String> {
    if let Some(open_id) = target.strip_prefix("open_id:") {
        api.send_interactive_message_to("open_id", open_id, card)
            .await
    } else {
        api.send_interactive_message(target, card).await
    }
}
