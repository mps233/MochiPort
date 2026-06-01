use anyhow::Result;
use tracing::info;

use crate::{
    app_state::SharedState,
    im::{
        core::{
            approval::{ApprovalReplyOutcome, resolve_approval_reply, submit_approval_decision},
            routing::{
                active_turn_for_message, clear_thread_binding, live_thread_for_route,
                route_for_message,
            },
            session::{create_and_bind_thread, resume_and_bind_thread},
            thread::{
                is_approval_reply, next_thread_routing_request_id, summarize_thread_cwd,
                summarize_thread_start_options, summarize_thread_title,
                thread_start_options_with_current_provider,
            },
            thread_list::{empty_thread_routing_request, load_thread_routing_page},
            turn::{TurnStartOutcome, start_turn_for_route},
        },
        wechat::{adapter::WechatAdapter, api::WechatApi, types::WechatSettings},
    },
    im_runtime::{RouteTarget, ThreadRoutingRequestState, TurnOrigin},
    remote_control_backend,
    types::InboundMessage,
};

pub(crate) async fn handle_inbound(state: SharedState, message: InboundMessage) -> Result<()> {
    info!(
        "inbound wechat message chat={} sender={}",
        message.chat_id, message.sender_id
    );
    state
        .push_event(
            "info",
            "wechat_message",
            format!(
                "chat={} sender={} text_len={}",
                message.chat_id,
                message.sender_id,
                message.text.chars().count()
            ),
        )
        .await;

    let config = state.config.lock().await.clone();
    let settings = WechatSettings::from_app_config(&config.wechat);
    let api = WechatApi::new(settings);
    let adapter = WechatAdapter::new(api);
    let account_id = message.account_id.clone();
    let route = route_for_message(&message);
    {
        let mut runtime = state.runtime.lock().await;
        runtime.last_route = Some(route.clone());
    }

    let trimmed = message.text.trim();
    let normalized = command(trimmed);

    if let Some(command) = normalized.as_deref()
        && is_approval_reply(command)
        && state
            .runtime
            .lock()
            .await
            .has_pending_approvals(&message.conversation_key())
    {
        handle_wechat_approval_text_reply(&state, &adapter, &message, command).await?;
        return Ok(());
    }

    if let Some(command) = normalized.as_deref()
        && handle_thread_list_text_reply(&state, &adapter, &message, command).await?
    {
        return Ok(());
    }

    match normalized.as_deref() {
        Some("/start") | Some("/help") => {
            adapter
                .send_text(
                    &state,
                    &account_id,
                    &message.chat_id,
                    "Codex Remote 已连接微信。\n\n直接发送消息会进入 Codex。常用命令：\n/status 查看状态\n/new 创建新会话\n/load 恢复历史会话\n/s 中断当前任务\n/q 退出当前会话",
                )
                .await?;
            return Ok(());
        }
        Some("/status") => {
            send_status(&state, &adapter, &message, &route).await?;
            return Ok(());
        }
        Some("/new") => {
            if let Some((_, turn_id)) = active_turn_for_message(&state, &message).await {
                adapter
                    .send_text(
                        &state,
                        &account_id,
                        &message.chat_id,
                        &format!(
                            "当前任务仍在执行中（turn: {turn_id}）。请先发送 /s 中断，或等待完成。"
                        ),
                    )
                    .await?;
                return Ok(());
            }
            create_wechat_thread_for_route(&state, &adapter, &route).await?;
            return Ok(());
        }
        Some("/threads") | Some("/load") => {
            send_thread_routing_list(&state, &adapter, &message, None, None, 1).await?;
            return Ok(());
        }
        Some("/next") => {
            if let Some(request) =
                latest_thread_request_for_conversation(&state, &message.conversation_key()).await
            {
                if request.history_has_next {
                    let next_page = request.page + 1;
                    let cursor = request
                        .page_cursors
                        .get(request.page)
                        .and_then(|value| value.as_ref())
                        .cloned();
                    send_thread_routing_list(
                        &state,
                        &adapter,
                        &message,
                        Some(request),
                        cursor.as_deref(),
                        next_page,
                    )
                    .await?;
                } else {
                    adapter
                        .send_text(&state, &account_id, &message.chat_id, "已经是最后一页。")
                        .await?;
                }
            } else {
                adapter
                    .send_text(
                        &state,
                        &account_id,
                        &message.chat_id,
                        "没有可翻页的会话列表，请先发送 /load。",
                    )
                    .await?;
            }
            return Ok(());
        }
        Some("/prev") => {
            if let Some(request) =
                latest_thread_request_for_conversation(&state, &message.conversation_key()).await
            {
                if request.page > 1 {
                    let previous_page = request.page - 1;
                    let cursor = request
                        .page_cursors
                        .get(previous_page.saturating_sub(1))
                        .and_then(|value| value.as_ref())
                        .cloned();
                    send_thread_routing_list(
                        &state,
                        &adapter,
                        &message,
                        Some(request),
                        cursor.as_deref(),
                        previous_page,
                    )
                    .await?;
                } else {
                    adapter
                        .send_text(&state, &account_id, &message.chat_id, "已经是第一页。")
                        .await?;
                }
            } else {
                adapter
                    .send_text(
                        &state,
                        &account_id,
                        &message.chat_id,
                        "没有可翻页的会话列表，请先发送 /load。",
                    )
                    .await?;
            }
            return Ok(());
        }
        Some("/s") | Some("/stop") => {
            let Some((thread_id, turn_id)) = active_turn_for_message(&state, &message).await else {
                adapter
                    .send_text(
                        &state,
                        &account_id,
                        &message.chat_id,
                        "当前没有运行中的 turn。",
                    )
                    .await?;
                return Ok(());
            };
            remote_control_backend::interrupt_turn(&state, &thread_id, &turn_id).await?;
            state
                .runtime
                .lock()
                .await
                .mark_turn_completed(&thread_id, Some(&turn_id));
            adapter
                .send_text(&state, &account_id, &message.chat_id, "已中断当前任务。")
                .await?;
            return Ok(());
        }
        Some("/q") => {
            if let Some((thread_id, turn_id)) = active_turn_for_message(&state, &message).await {
                let _ = remote_control_backend::interrupt_turn(&state, &thread_id, &turn_id).await;
                state
                    .runtime
                    .lock()
                    .await
                    .mark_turn_completed(&thread_id, Some(&turn_id));
            }
            clear_thread_binding(&state, &route.conversation_key).await?;
            adapter
                .send_text(&state, &account_id, &message.chat_id, "已退出当前会话。")
                .await?;
            return Ok(());
        }
        Some(other) => {
            adapter
                .send_text(
                    &state,
                    &account_id,
                    &message.chat_id,
                    &format!("不支持的命令：{other}"),
                )
                .await?;
            return Ok(());
        }
        None => {}
    }

    let remote_status = remote_control_backend::status_snapshot(&state).await;
    if !remote_status.connected {
        adapter
            .send_text(
                &state,
                &account_id,
                &message.chat_id,
                "Codex remote-control 还没有连接。请在项目目录运行 Codex，并打开 remote-control 后再发送消息。",
            )
            .await?;
        return Ok(());
    }

    match start_turn_for_route(
        &state,
        &route,
        trimmed,
        &message.attachments,
        TurnOrigin::Wechat,
    )
    .await
    {
        TurnStartOutcome::Started { thread_id, turn_id } => {
            state
                .push_event(
                    "info",
                    "wechat_turn_started",
                    format!(
                        "chat={} thread={} turn={turn_id}",
                        message.chat_id, thread_id
                    ),
                )
                .await;
            Ok(())
        }
        TurnStartOutcome::NoThread => {
            send_thread_routing_choice(&state, &adapter, &message).await?;
            Ok(())
        }
        TurnStartOutcome::Stale { thread_id } => {
            state
                .push_event(
                    "warn",
                    "wechat_thread_route_stale",
                    format!("conversation={} thread={thread_id}", route.conversation_key),
                )
                .await;
            send_thread_routing_choice(&state, &adapter, &message).await
        }
        TurnStartOutcome::Failed { error } => {
            adapter
                .send_text(
                    &state,
                    &account_id,
                    &message.chat_id,
                    &format!("Codex App 没有接收这条消息：{error}\n\n请确认 Codex App 还打开着 remote-control。"),
                )
                .await?;
            Err(error)
        }
    }
}

async fn send_status(
    state: &SharedState,
    adapter: &WechatAdapter,
    message: &InboundMessage,
    route: &RouteTarget,
) -> Result<()> {
    let remote = remote_control_backend::status_snapshot(state).await;
    let bridge_enabled = state.config.lock().await.bridge.enabled;
    let bridge_status = if bridge_enabled { "启用" } else { "停用" };
    let remote_status = if remote.connected {
        "已连接"
    } else {
        "未连接"
    };
    let thread_status =
        if let Some((thread_id, turn_id)) = active_turn_for_message(state, message).await {
            format!("thread: {thread_id}\n执行: 执行中\nturn: {turn_id}")
        } else if let Some(thread_id) = live_thread_for_route(state, route).await {
            format!("thread: {thread_id}\n执行: 空闲")
        } else {
            "thread: 未绑定".to_string()
        };
    adapter
        .send_text(
            state,
            &message.account_id,
            &message.chat_id,
            &format!(
                "Codex Remote\nbridge: {bridge_status}\nremote-control: {remote_status}\n{thread_status}"
            ),
        )
        .await?;
    Ok(())
}

async fn create_wechat_thread_for_route(
    state: &SharedState,
    adapter: &WechatAdapter,
    route: &RouteTarget,
) -> Result<String> {
    adapter
        .send_text(
            state,
            &route.account_id,
            &route.chat_id,
            "正在创建新的 Codex thread...",
        )
        .await?;
    let options = thread_start_options_with_current_provider(
        remote_control_backend::ThreadStartOptions::default(),
    );
    let thread_id = create_and_bind_thread(state, route, options.clone(), None).await?;
    adapter
        .send_text(
            state,
            &route.account_id,
            &route.chat_id,
            &format!(
                "已创建新会话\n\n已接入新 thread `{thread_id}`。\n\n{}\n\n现在可以直接发送消息。",
                summarize_thread_start_options(&options)
            ),
        )
        .await?;
    state
        .push_event(
            "info",
            "wechat_thread_route_created",
            format!("conversation={} thread={thread_id}", route.conversation_key),
        )
        .await;
    Ok(thread_id)
}

async fn send_thread_routing_choice(
    state: &SharedState,
    adapter: &WechatAdapter,
    message: &InboundMessage,
) -> Result<()> {
    let route = route_for_message(message);
    let request_id = next_thread_routing_request_id();
    let message_id = adapter
        .send_text(
            state,
            &message.account_id,
            &message.chat_id,
            "当前微信会话还没有接入 Codex thread。\n\n回复 /new 创建新会话，或回复 /load 恢复历史会话。",
        )
        .await?;
    state
        .runtime
        .lock()
        .await
        .remember_thread_routing_request(empty_thread_routing_request(
            &route, request_id, message_id,
        ));
    Ok(())
}

async fn send_thread_routing_list(
    state: &SharedState,
    adapter: &WechatAdapter,
    message: &InboundMessage,
    existing_request: Option<ThreadRoutingRequestState>,
    cursor: Option<&str>,
    page: usize,
) -> Result<()> {
    let route = route_for_message(message);
    let loaded_page =
        load_thread_routing_page(state, existing_request.as_ref(), cursor, page).await?;
    if loaded_page.entries.is_empty() {
        let message_id = adapter
            .send_text(
                state,
                &message.account_id,
                &message.chat_id,
                "当前没有可恢复的历史会话。\n\n回复 /new 创建新会话。",
            )
            .await?;
        state
            .runtime
            .lock()
            .await
            .remember_thread_routing_request(empty_thread_routing_request(
                &route,
                loaded_page.request_id,
                message_id,
            ));
        return Ok(());
    }
    let text = thread_list_text(&loaded_page);
    let message_id = adapter
        .send_text(state, &message.account_id, &message.chat_id, &text)
        .await?;
    state
        .runtime
        .lock()
        .await
        .remember_thread_routing_request(loaded_page.into_request(
            &route,
            message_id,
            existing_request.as_ref(),
            cursor,
        ));
    Ok(())
}

async fn handle_thread_list_text_reply(
    state: &SharedState,
    adapter: &WechatAdapter,
    message: &InboundMessage,
    command: &str,
) -> Result<bool> {
    let Some(index) = command
        .strip_prefix('/')
        .and_then(|value| value.parse::<usize>().ok())
    else {
        return Ok(false);
    };
    let Some(request) =
        latest_thread_request_for_conversation(state, &message.conversation_key()).await
    else {
        return Ok(false);
    };
    let Some(thread_id) = request
        .thread_ids_by_page
        .get(request.page.saturating_sub(1))
        .and_then(|threads| threads.get(index.saturating_sub(1)))
        .cloned()
    else {
        adapter
            .send_text(
                state,
                &message.account_id,
                &message.chat_id,
                "这个序号不在当前会话列表里，请按列表里的 /1、/2 选择。",
            )
            .await?;
        return Ok(true);
    };
    let route = route_for_message(message);
    let thread =
        resume_and_bind_thread(state, &route, &thread_id, Some(&request.request_id)).await?;
    adapter
        .send_text(
            state,
            &message.account_id,
            &message.chat_id,
            &format!(
                "已接入历史会话\n\n{}\n{}\n\n现在可以直接发送消息。",
                summarize_thread_title(&thread),
                summarize_thread_cwd(&thread)
            ),
        )
        .await?;
    state
        .push_event(
            "info",
            "wechat_thread_route_resumed",
            format!("conversation={} thread={thread_id}", route.conversation_key),
        )
        .await;
    Ok(true)
}

async fn handle_wechat_approval_text_reply(
    state: &SharedState,
    adapter: &WechatAdapter,
    message: &InboundMessage,
    command: &str,
) -> Result<()> {
    match resolve_approval_reply(state, message, command).await {
        ApprovalReplyOutcome::Ready {
            pending, decision, ..
        } => {
            let next = submit_approval_decision(state, &pending, &decision).await?;
            adapter
                .send_text(
                    state,
                    &message.account_id,
                    &message.chat_id,
                    "审批决定已提交。",
                )
                .await?;
            if let Some((conversation_key, next_approval)) = next
                && let Some(route) =
                    crate::im_runtime::route_from_conversation_key(&conversation_key)
                && route.platform == crate::types::ImPlatformKind::Wechat
            {
                adapter
                    .send_approval(state, &route.account_id, &route.chat_id, &next_approval)
                    .await?;
            }
        }
        ApprovalReplyOutcome::NoPending => {
            adapter
                .send_text(
                    state,
                    &message.account_id,
                    &message.chat_id,
                    "当前没有待处理审批。",
                )
                .await?;
        }
        ApprovalReplyOutcome::NotCurrent => {
            adapter
                .send_text(
                    state,
                    &message.account_id,
                    &message.chat_id,
                    "这个审批请求已经不是当前待处理项。",
                )
                .await?;
        }
        ApprovalReplyOutcome::InvalidInput { hint } => {
            adapter
                .send_text(
                    state,
                    &message.account_id,
                    &message.chat_id,
                    &format!("审批回复无效，请回复 {hint}。"),
                )
                .await?;
        }
    }
    Ok(())
}

async fn latest_thread_request_for_conversation(
    state: &SharedState,
    conversation_key: &str,
) -> Option<ThreadRoutingRequestState> {
    state
        .runtime
        .lock()
        .await
        .thread_routing_requests
        .values()
        .filter(|request| request.conversation_key == conversation_key)
        .max_by_key(|request| thread_routing_request_rank(&request.request_id))
        .cloned()
}

fn thread_routing_request_rank(request_id: &str) -> u64 {
    request_id
        .rsplit('-')
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0)
}

fn thread_list_text(page: &crate::im::core::thread_list::ThreadRoutingPage) -> String {
    let mut lines = Vec::new();
    lines.push("选择 Codex 会话".to_string());
    if let Some(provider) = page.model_provider_filter.as_deref() {
        lines.push(format!("已按当前 Codex App provider `{provider}` 过滤。"));
    }
    lines.push(format!("第 {} 页", page.page.max(1)));
    lines.push(format!(
        "回复 /1 ~ /{} 选择会话。{}{} 也可以回复 /new 创建新会话。",
        page.entries.len(),
        if page.page > 1 {
            " /prev 上一页。"
        } else {
            ""
        },
        if page.next_cursor.is_some() {
            " /next 下一页。"
        } else {
            ""
        },
    ));
    lines.push(String::new());
    for (index, entry) in page.entries.iter().enumerate() {
        lines.push(format!(
            "/{} {}",
            index + 1,
            truncate_line(entry.title.trim(), 64)
        ));
        if let Some(summary) = entry
            .summary
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            lines.push(truncate_line(summary, 96));
        }
        if let Some(detail) = entry
            .last_activity_text
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            lines.push(detail.to_string());
        }
        lines.push(String::new());
    }
    lines.join("\n").trim_end().to_string()
}

fn command(text: &str) -> Option<String> {
    let first = text.split_whitespace().next()?.trim();
    first.starts_with('/').then(|| first.to_ascii_lowercase())
}

fn truncate_line(text: &str, max_chars: usize) -> String {
    let text = text.replace('\r', " ").replace('\n', " ");
    if text.chars().count() <= max_chars {
        return text;
    }
    let mut output = text
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    output.push('…');
    output
}
