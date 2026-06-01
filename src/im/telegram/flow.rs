use std::path::Path;

use anyhow::{Result, anyhow};
use tracing::info;

use crate::{
    app_state::SharedState,
    im::core::{
        approval::{
            ApprovalReplyOutcome, resolve_approval_button_reply, resolve_approval_reply,
            submit_approval_decision,
        },
        routing::{
            active_turn_for_message, clear_thread_binding, live_thread_for_route, route_for_message,
        },
        session::{create_and_bind_thread, resume_and_bind_thread},
        thread::{
            ThreadCreateDefaults, ThreadCreateForm, expand_home_prefix, is_approval_reply,
            load_thread_create_defaults, next_thread_routing_request_id, summarize_thread_cwd,
            summarize_thread_start_options, summarize_thread_status, summarize_thread_title,
            thread_start_options_from_form, thread_start_options_with_current_provider,
        },
        thread_list::{empty_thread_routing_request, load_thread_routing_page},
        turn::{TurnStartOutcome, start_turn_for_route},
    },
    im::events,
    im::telegram::{
        adapter::{TelegramAdapter, TelegramCreateOption, TelegramThreadListEntry},
        api::TelegramApi,
        types::TelegramSettings,
    },
    im_runtime::{RouteTarget, ThreadCreateDraftState, ThreadRoutingRequestState, TurnOrigin},
    remote_control_backend,
    types::{InboundAction, InboundMessage, ThreadRouteDirection},
};

const TELEGRAM_CREATE_OPTION_PAGE_SIZE: usize = 8;

pub(crate) async fn handle_inbound(state: SharedState, message: InboundMessage) -> Result<()> {
    info!(
        "inbound telegram message chat={} sender={}",
        message.chat_id, message.sender_id
    );
    state
        .push_event(
            "info",
            "telegram_message",
            format!(
                "chat={} sender={} text_len={}",
                message.chat_id,
                message.sender_id,
                message.text.chars().count()
            ),
        )
        .await;

    let config = state.config.lock().await.clone();
    let api = TelegramApi::new(TelegramSettings::from_app_config(&config.telegram));
    let adapter = TelegramAdapter::new(api);
    let trimmed = message.text.trim();
    let route = route_for_message(&message);
    {
        let mut runtime = state.runtime.lock().await;
        runtime.last_route = Some(route.clone());
    }
    if let Some(action) = message.action.clone() {
        return handle_inbound_action(state, adapter, message, action).await;
    }

    if handle_telegram_thread_create_text_input(&state, &adapter, &message, trimmed).await? {
        return Ok(());
    }

    let command = command(trimmed);
    if let Some(command) = command.as_deref()
        && handle_telegram_thread_create_option_text_reply(
            state.clone(),
            adapter.clone(),
            message.clone(),
            command,
        )
        .await?
    {
        return Ok(());
    }
    if let Some(command) = command.as_deref()
        && is_approval_reply(command)
        && state
            .runtime
            .lock()
            .await
            .has_pending_approvals(&message.conversation_key())
    {
        handle_telegram_approval_text_reply(&state, &adapter, &message, command).await?;
        return Ok(());
    }
    if let Some(command) = command.as_deref()
        && handle_telegram_thread_list_text_reply(
            state.clone(),
            adapter.clone(),
            message.clone(),
            command,
        )
        .await?
    {
        return Ok(());
    }
    if let Some(command) = command.as_deref()
        && is_approval_reply(command)
    {
        handle_telegram_approval_text_reply(&state, &adapter, &message, command).await?;
        return Ok(());
    }

    match command.as_deref() {
        Some("/start") | Some("/help") => {
            adapter
                .send_text(
                    &message.chat_id,
                    "Codex Remote 已连接 Telegram。\n\n直接发送消息会进入 Codex。常用命令：\n/status 查看状态\n/new 创建新会话\n/s 中断当前任务\n/q 退出当前会话",
                )
                .await?;
            return Ok(());
        }
        Some("/status") => {
            let remote = remote_control_backend::status_snapshot(&state).await;
            let bridge_status = if config.bridge.enabled {
                "启用"
            } else {
                "停用"
            };
            let remote_status = if remote.connected {
                "已连接"
            } else {
                "未连接"
            };
            let thread_status = if let Some((thread_id, turn_id)) =
                active_turn_for_message(&state, &message).await
            {
                format!("thread: {thread_id}\n执行: 执行中\nturn: {turn_id}")
            } else if let Some(thread_id) = live_thread_for_route(&state, &route).await {
                format!("thread: {thread_id}\n执行: 空闲")
            } else {
                "thread: 未绑定".to_string()
            };
            adapter
                .send_text(
                    &message.chat_id,
                    &format!(
                        "Codex Remote\nbridge: {bridge_status}\nremote-control: {remote_status}\n{thread_status}"
                    ),
                )
                .await?;
            return Ok(());
        }
        Some("/new") => {
            if let Some((_, turn_id)) = active_turn_for_message(&state, &message).await {
                adapter
                    .send_text(
                        &message.chat_id,
                        &format!(
                            "当前任务仍在执行中（turn: {turn_id}）。请先发送 /s 中断，或等待完成。"
                        ),
                    )
                    .await?;
                return Ok(());
            }
            if !remote_control_backend::status_snapshot(&state)
                .await
                .connected
            {
                clear_thread_binding(&state, &route.conversation_key).await?;
                adapter
                    .send_text(
                        &message.chat_id,
                        "已解除当前绑定，但 Codex remote-control 还没有连接。请在项目目录运行 codex 后再发送消息。",
                    )
                    .await?;
                return Ok(());
            }
            send_telegram_thread_create_settings(&state, &adapter, &message, None).await?;
            return Ok(());
        }
        Some("/threads") | Some("/load") => {
            send_telegram_thread_routing_choice(&state, &adapter, &message, None).await?;
            return Ok(());
        }
        Some("/s") | Some("/stop") => {
            let Some((thread_id, turn_id)) = active_turn_for_message(&state, &message).await else {
                adapter
                    .send_text(&message.chat_id, "当前没有运行中的 turn。")
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
                .send_text(&message.chat_id, "已中断当前任务。")
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
                .send_text(&message.chat_id, "已退出当前会话。")
                .await?;
            return Ok(());
        }
        Some(other) => {
            adapter
                .send_text(&message.chat_id, &format!("不支持的命令：{other}"))
                .await?;
            return Ok(());
        }
        None => {}
    }

    let remote_status = remote_control_backend::status_snapshot(&state).await;
    if !remote_status.connected {
        adapter
            .send_text(
                &message.chat_id,
                "Codex remote-control 还没有连接。请在项目目录运行 codex，确认它已经通过 remote-control 连接到 codex-remote。",
            )
            .await?;
        return Ok(());
    }

    match start_turn_for_route(
        &state,
        &route,
        trimmed,
        &message.attachments,
        TurnOrigin::Telegram,
    )
    .await
    {
        TurnStartOutcome::Started { thread_id, turn_id } => {
            state
                .push_event(
                    "info",
                    "telegram_turn_started",
                    format!(
                        "chat={} thread={} turn={turn_id}",
                        message.chat_id, thread_id
                    ),
                )
                .await;
            Ok(())
        }
        TurnStartOutcome::NoThread => {
            send_telegram_thread_routing_choice(&state, &adapter, &message, None).await?;
            Ok(())
        }
        TurnStartOutcome::Stale { thread_id } => {
            state
                .push_event(
                    "warn",
                    "telegram_thread_route_stale",
                    format!(
                        "conversation={} thread={} during=turn/start",
                        route.conversation_key, thread_id
                    ),
                )
                .await;
            adapter
                .send_text(
                    &message.chat_id,
                    "当前绑定的 Codex thread 已失效，已解除绑定。",
                )
                .await?;
            send_telegram_thread_routing_choice(&state, &adapter, &message, None).await
        }
        TurnStartOutcome::Failed { error } => {
            adapter
                .send_text(
                    &message.chat_id,
                    &format!(
                        "Codex App 没有接收这条消息：{error}\n\n请确认 Codex App 还打开着 remote-control。"
                    ),
                )
                .await?;
            Err(error)
        }
    }
}

async fn create_telegram_thread_for_route(
    state: &SharedState,
    adapter: &TelegramAdapter,
    route: &RouteTarget,
    options: remote_control_backend::ThreadStartOptions,
    request_id: Option<&str>,
) -> Result<String> {
    adapter
        .send_text(&route.chat_id, "正在创建新的 Codex thread...")
        .await?;
    let thread_id = create_and_bind_thread(state, route, options.clone(), request_id).await?;
    adapter
        .send_thread_routing_result(
            &route.chat_id,
            "已创建新会话",
            &format!(
                "已接入新 thread `{thread_id}`。\n\n{}\n\n现在可以直接发送消息。",
                summarize_thread_start_options(&options)
            ),
        )
        .await?;
    state
        .push_event(
            "info",
            "telegram_thread_route_created",
            format!("conversation={} thread={thread_id}", route.conversation_key),
        )
        .await;
    Ok(thread_id)
}

pub(crate) async fn handle_inbound_action(
    state: SharedState,
    adapter: TelegramAdapter,
    message: InboundMessage,
    action: InboundAction,
) -> Result<()> {
    match action {
        InboundAction::ApprovalDecision {
            request_fingerprint,
            option_index,
        } => {
            handle_telegram_approval_button_reply(
                &state,
                &adapter,
                &message,
                &request_fingerprint,
                option_index,
            )
            .await?;
            Ok(())
        }
        InboundAction::ThreadRouteChoice { request_id, action } => {
            handle_telegram_thread_route_choice(state, adapter, message, &request_id, &action).await
        }
        InboundAction::ThreadRouteCreateSubmit {
            request_id,
            cwd_choice,
            cwd_custom,
            model,
            effort,
            permission,
        } => {
            let Some(request) =
                checked_telegram_thread_routing_request(&state, &adapter, &message, &request_id)
                    .await?
            else {
                return Ok(());
            };
            let options = match thread_start_options_from_form(
                &state,
                ThreadCreateForm {
                    cwd_choice,
                    cwd_custom,
                    model,
                    effort,
                    permission,
                },
            )
            .await
            {
                Ok(options) => options,
                Err(err) => {
                    adapter
                        .send_text(&message.chat_id, &format!("新建会话参数不正确：{err}"))
                        .await?;
                    return Ok(());
                }
            };
            let route = route_for_message(&message);
            let _ = request;
            create_telegram_thread_for_route(&state, &adapter, &route, options, Some(&request_id))
                .await?;
            Ok(())
        }
        InboundAction::ThreadRouteCreateDefault { request_id } => {
            let Some(_) =
                checked_telegram_thread_routing_request(&state, &adapter, &message, &request_id)
                    .await?
            else {
                return Ok(());
            };
            let route = route_for_message(&message);
            let options = thread_start_options_with_current_provider(
                remote_control_backend::ThreadStartOptions::default(),
            );
            create_telegram_thread_for_route(&state, &adapter, &route, options, Some(&request_id))
                .await?;
            Ok(())
        }
        InboundAction::ThreadRouteCreateConfigured { request_id } => {
            let Some(request) =
                checked_telegram_thread_routing_request(&state, &adapter, &message, &request_id)
                    .await?
            else {
                return Ok(());
            };
            let options = match thread_start_options_from_form(
                &state,
                thread_create_form_from_draft(&request.create_draft),
            )
            .await
            {
                Ok(options) => options,
                Err(err) => {
                    adapter
                        .send_text(&message.chat_id, &format!("新建会话参数不正确：{err}"))
                        .await?;
                    return Ok(());
                }
            };
            let route = route_for_message(&message);
            create_telegram_thread_for_route(&state, &adapter, &route, options, Some(&request_id))
                .await?;
            Ok(())
        }
        InboundAction::ThreadRouteCreateEdit { request_id, field } => {
            let Some(request) =
                checked_telegram_thread_routing_request(&state, &adapter, &message, &request_id)
                    .await?
            else {
                return Ok(());
            };
            send_telegram_thread_create_options(&state, &adapter, &message, request, &field, 1)
                .await
        }
        InboundAction::ThreadRouteCreateSetIndex {
            request_id,
            field,
            page,
            index,
        } => {
            let Some(mut request) =
                checked_telegram_thread_routing_request(&state, &adapter, &message, &request_id)
                    .await?
            else {
                return Ok(());
            };
            let Some(field) = normalize_thread_create_field(&field) else {
                adapter
                    .send_text(&message.chat_id, "这个创建选项不可用，请重新打开创建设置。")
                    .await?;
                return Ok(());
            };
            let Some(value) = request
                .create_option_values_by_field_page
                .get(field)
                .and_then(|pages| pages.get(page.saturating_sub(1)))
                .and_then(|values| values.get(index))
                .cloned()
            else {
                adapter
                    .send_text(
                        &message.chat_id,
                        "这个创建选项已经失效，请重新打开创建设置。",
                    )
                    .await?;
                return Ok(());
            };
            apply_thread_create_draft_value(&mut request.create_draft, field, &value)?;
            state
                .runtime
                .lock()
                .await
                .remember_thread_routing_request(request.clone());
            if field == "cwd" && value == "__custom__" {
                send_telegram_thread_create_custom_cwd_prompt(&adapter, &message).await?;
                return Ok(());
            }
            send_telegram_thread_create_settings(&state, &adapter, &message, Some(request)).await
        }
        InboundAction::ThreadRouteCreateSetValue {
            request_id,
            field,
            value,
        } => {
            let Some(mut request) =
                checked_telegram_thread_routing_request(&state, &adapter, &message, &request_id)
                    .await?
            else {
                return Ok(());
            };
            let Some(field) = normalize_thread_create_field(&field) else {
                adapter
                    .send_text(&message.chat_id, "这个创建选项不可用，请重新打开创建设置。")
                    .await?;
                return Ok(());
            };
            apply_thread_create_draft_value(&mut request.create_draft, field, &value)?;
            state
                .runtime
                .lock()
                .await
                .remember_thread_routing_request(request.clone());
            if field == "cwd" && value == "__custom__" {
                send_telegram_thread_create_custom_cwd_prompt(&adapter, &message).await?;
                return Ok(());
            }
            send_telegram_thread_create_settings(&state, &adapter, &message, Some(request)).await
        }
        InboundAction::ThreadRouteCreateOptionsPage {
            request_id,
            field,
            direction,
        } => {
            let Some(request) =
                checked_telegram_thread_routing_request(&state, &adapter, &message, &request_id)
                    .await?
            else {
                return Ok(());
            };
            let current_page = request.page.max(1);
            let target_page = match direction {
                ThreadRouteDirection::Prev => current_page.saturating_sub(1).max(1),
                ThreadRouteDirection::Next => current_page.saturating_add(1),
            };
            send_telegram_thread_create_options(
                &state,
                &adapter,
                &message,
                request,
                &field,
                target_page,
            )
            .await
        }
        InboundAction::ThreadRouteResumeSelected {
            request_id,
            thread_id,
        } => {
            handle_telegram_thread_route_resume_selected(
                state,
                adapter,
                message,
                &request_id,
                &thread_id,
            )
            .await
        }
        InboundAction::ThreadRouteResumeIndex {
            request_id,
            page,
            index,
        } => {
            let Some(request) =
                checked_telegram_thread_routing_request(&state, &adapter, &message, &request_id)
                    .await?
            else {
                return Ok(());
            };
            let Some(thread_id) = request
                .thread_ids_by_page
                .get(page.saturating_sub(1))
                .and_then(|thread_ids| thread_ids.get(index))
                .cloned()
            else {
                adapter
                    .send_text(
                        &message.chat_id,
                        "这个 thread 选择已经失效，请重新打开列表。",
                    )
                    .await?;
                return Ok(());
            };
            handle_telegram_thread_route_resume_selected(
                state,
                adapter,
                message,
                &request_id,
                &thread_id,
            )
            .await
        }
        InboundAction::ThreadRouteListPage {
            request_id,
            direction,
        } => {
            handle_telegram_thread_route_list_page(state, adapter, message, &request_id, direction)
                .await
        }
    }
}

async fn handle_telegram_thread_route_choice(
    state: SharedState,
    adapter: TelegramAdapter,
    message: InboundMessage,
    request_id: &str,
    action: &str,
) -> Result<()> {
    let request =
        checked_telegram_thread_routing_request(&state, &adapter, &message, request_id).await?;
    let Some(request) = request else {
        return Ok(());
    };

    match action {
        "create_new" => {
            send_telegram_thread_create_settings(&state, &adapter, &message, Some(request)).await
        }
        "resume_history" => {
            send_telegram_thread_routing_list(&state, &adapter, &message, Some(request), None, 1)
                .await
        }
        "back" => {
            send_telegram_thread_routing_choice(&state, &adapter, &message, Some(request)).await
        }
        other => {
            adapter
                .send_text(&message.chat_id, &format!("不支持的 thread 操作：{other}"))
                .await?;
            Ok(())
        }
    }
}

async fn checked_telegram_thread_routing_request(
    state: &SharedState,
    adapter: &TelegramAdapter,
    message: &InboundMessage,
    request_id: &str,
) -> Result<Option<ThreadRoutingRequestState>> {
    let request = {
        state
            .runtime
            .lock()
            .await
            .thread_routing_request(request_id)
    };
    let Some(request) = request else {
        adapter
            .send_text(
                &message.chat_id,
                "这个 thread 操作已经失效，请重新发送 /threads。",
            )
            .await?;
        return Ok(None);
    };
    if request.conversation_key != message.conversation_key() {
        adapter
            .send_text(&message.chat_id, "这个 thread 操作不属于当前会话。")
            .await?;
        return Ok(None);
    }
    Ok(Some(request))
}

async fn handle_telegram_thread_list_text_reply(
    state: SharedState,
    adapter: TelegramAdapter,
    message: InboundMessage,
    command: &str,
) -> Result<bool> {
    let Some(index) = numeric_command_index(command) else {
        return Ok(false);
    };
    let Some(request) =
        pending_telegram_thread_list_request(&state, &message.conversation_key()).await
    else {
        return Ok(false);
    };
    let page = request.page.max(1);
    let Some(thread_id) = request
        .thread_ids_by_page
        .get(page.saturating_sub(1))
        .and_then(|thread_ids| thread_ids.get(index))
        .cloned()
    else {
        adapter
            .send_text(
                &message.chat_id,
                "这个会话序号不可用，请重新打开 /threads。",
            )
            .await?;
        return Ok(true);
    };
    handle_telegram_thread_route_resume_selected(
        state,
        adapter,
        message,
        &request.request_id,
        &thread_id,
    )
    .await?;
    Ok(true)
}

async fn handle_telegram_thread_create_option_text_reply(
    state: SharedState,
    adapter: TelegramAdapter,
    message: InboundMessage,
    command: &str,
) -> Result<bool> {
    let Some(index) = numeric_command_index(command) else {
        return Ok(false);
    };
    let Some(mut request) =
        pending_telegram_thread_create_options_request(&state, &message.conversation_key()).await
    else {
        return Ok(false);
    };
    let page = request.page.max(1);
    let Some((field, value)) =
        request
            .create_option_values_by_field_page
            .iter()
            .find_map(|(field, pages)| {
                pages
                    .get(page.saturating_sub(1))
                    .and_then(|values| values.get(index))
                    .cloned()
                    .map(|value| (field.clone(), value))
            })
    else {
        adapter
            .send_text(&message.chat_id, "这个选项序号不可用，请重新打开创建设置。")
            .await?;
        return Ok(true);
    };
    apply_thread_create_draft_value(&mut request.create_draft, &field, &value)?;
    state
        .runtime
        .lock()
        .await
        .remember_thread_routing_request(request.clone());
    if field == "cwd" && value == "__custom__" {
        send_telegram_thread_create_custom_cwd_prompt(&adapter, &message).await?;
        return Ok(true);
    }
    send_telegram_thread_create_settings(&state, &adapter, &message, Some(request)).await?;
    Ok(true)
}

async fn pending_telegram_thread_create_options_request(
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
        .filter(|request| !request.create_option_values_by_field_page.is_empty())
        .max_by_key(|request| thread_routing_request_rank(&request.request_id))
        .cloned()
}

async fn pending_telegram_thread_list_request(
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
        .filter(|request| {
            request
                .thread_ids_by_page
                .get(request.page.saturating_sub(1))
                .is_some_and(|thread_ids| !thread_ids.is_empty())
        })
        .max_by_key(|request| thread_routing_request_rank(&request.request_id))
        .cloned()
}

fn thread_routing_request_rank(request_id: &str) -> u64 {
    request_id
        .strip_prefix("thread-route-")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or_default()
}

async fn handle_telegram_thread_create_text_input(
    state: &SharedState,
    adapter: &TelegramAdapter,
    message: &InboundMessage,
    text: &str,
) -> Result<bool> {
    let Some(mut request) =
        pending_telegram_thread_create_custom_cwd_request(state, &message.conversation_key()).await
    else {
        return Ok(false);
    };
    if text.eq_ignore_ascii_case("/cancel") {
        request.create_draft.cwd_choice = None;
        request.create_draft.cwd_custom = None;
        state
            .runtime
            .lock()
            .await
            .remember_thread_routing_request(request.clone());
        send_telegram_thread_create_settings(state, adapter, message, Some(request)).await?;
        return Ok(true);
    }
    if command(text).is_some_and(|command| is_control_command(&command)) {
        request.create_draft.cwd_choice = None;
        request.create_draft.cwd_custom = None;
        state
            .runtime
            .lock()
            .await
            .remember_thread_routing_request(request);
        return Ok(false);
    }
    let path = text.trim();
    if path.is_empty() {
        send_telegram_thread_create_custom_cwd_prompt(adapter, message).await?;
        return Ok(true);
    }
    if !expand_home_prefix(path).is_absolute() {
        adapter
            .send_text(
                &message.chat_id,
                "项目目录需要是绝对路径。请重新发送一个绝对路径，或发送 /cancel 取消。",
            )
            .await?;
        return Ok(true);
    }
    request.create_draft.cwd_choice = None;
    request.create_draft.cwd_custom = Some(path.to_string());
    state
        .runtime
        .lock()
        .await
        .remember_thread_routing_request(request.clone());
    send_telegram_thread_create_settings(state, adapter, message, Some(request)).await?;
    Ok(true)
}

async fn pending_telegram_thread_create_custom_cwd_request(
    state: &SharedState,
    conversation_key: &str,
) -> Option<ThreadRoutingRequestState> {
    state
        .runtime
        .lock()
        .await
        .thread_routing_requests
        .values()
        .find(|request| {
            request.conversation_key == conversation_key
                && request.create_draft.cwd_choice.as_deref() == Some("__custom__")
                && request.create_draft.cwd_custom.is_none()
        })
        .cloned()
}

async fn send_telegram_thread_create_custom_cwd_prompt(
    adapter: &TelegramAdapter,
    message: &InboundMessage,
) -> Result<()> {
    adapter
        .send_text(
            &message.chat_id,
            "请发送项目目录的绝对路径。目录不存在时，创建 thread 时会自动创建。\n\n发送 /cancel 取消。",
        )
        .await?;
    Ok(())
}

async fn send_telegram_thread_routing_choice(
    state: &SharedState,
    adapter: &TelegramAdapter,
    message: &InboundMessage,
    existing_request: Option<ThreadRoutingRequestState>,
) -> Result<()> {
    let route = route_for_message(message);
    let request_id = existing_request
        .as_ref()
        .map(|request| request.request_id.clone())
        .unwrap_or_else(next_thread_routing_request_id);
    let message_id = adapter
        .send_thread_routing_choice(&route.chat_id, &request_id)
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

async fn send_telegram_thread_create_settings(
    state: &SharedState,
    adapter: &TelegramAdapter,
    message: &InboundMessage,
    existing_request: Option<ThreadRoutingRequestState>,
) -> Result<()> {
    let route = route_for_message(message);
    let request_id = existing_request
        .as_ref()
        .map(|request| request.request_id.clone())
        .unwrap_or_else(next_thread_routing_request_id);
    let create_draft = existing_request
        .as_ref()
        .map(|request| request.create_draft.clone())
        .unwrap_or_default();
    let defaults = load_thread_create_defaults(state).await;
    let text = thread_create_help_text(&defaults, &create_draft);
    let message_id = adapter
        .send_thread_create_settings(&route.chat_id, &request_id, &text)
        .await?;
    state
        .runtime
        .lock()
        .await
        .remember_thread_routing_request(ThreadRoutingRequestState {
            request_id: request_id.clone(),
            conversation_key: route.conversation_key,
            account_id: route.account_id,
            chat_id: route.chat_id,
            message_id: Some(message_id),
            page: 1,
            page_cursors: vec![None],
            thread_ids_by_page: vec![vec![]],
            create_draft,
            create_option_values_by_field_page: Default::default(),
            history_cursor: None,
            history_has_next: false,
        });
    Ok(())
}

fn thread_create_form_from_draft(draft: &ThreadCreateDraftState) -> ThreadCreateForm {
    ThreadCreateForm {
        cwd_choice: draft.cwd_choice.clone(),
        cwd_custom: draft.cwd_custom.clone(),
        model: draft.model.clone(),
        effort: draft.effort.clone(),
        permission: draft.permission.clone(),
    }
}

async fn send_telegram_thread_create_options(
    state: &SharedState,
    adapter: &TelegramAdapter,
    message: &InboundMessage,
    mut request: ThreadRoutingRequestState,
    field: &str,
    page: usize,
) -> Result<()> {
    let Some(field) = normalize_thread_create_field(field) else {
        adapter
            .send_text(&message.chat_id, "这个创建选项不可用，请重新打开创建设置。")
            .await?;
        return Ok(());
    };
    let defaults = load_thread_create_defaults(state).await;
    let (title, body, options) = create_options_for_field(&defaults, &request.create_draft, field)?;
    let total_pages = ((options.len() + TELEGRAM_CREATE_OPTION_PAGE_SIZE - 1)
        / TELEGRAM_CREATE_OPTION_PAGE_SIZE)
        .max(1);
    let page = page.clamp(1, total_pages);
    let start = (page - 1) * TELEGRAM_CREATE_OPTION_PAGE_SIZE;
    let end = (start + TELEGRAM_CREATE_OPTION_PAGE_SIZE).min(options.len());
    let page_options = options[start..end]
        .iter()
        .map(|(_, option)| option.clone())
        .collect::<Vec<_>>();
    let value_pages = options
        .chunks(TELEGRAM_CREATE_OPTION_PAGE_SIZE)
        .map(|chunk| {
            chunk
                .iter()
                .map(|(value, _)| value.clone())
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    request.create_option_values_by_field_page.clear();
    request
        .create_option_values_by_field_page
        .insert(field.to_string(), value_pages);
    request.page = page;
    let message_id = adapter
        .send_thread_create_options(
            &request.chat_id,
            &request.request_id,
            field,
            &title,
            &body,
            &page_options,
            page,
            page > 1,
            page < total_pages,
        )
        .await?;
    request.message_id = Some(message_id);
    state
        .runtime
        .lock()
        .await
        .remember_thread_routing_request(request.clone());
    state
        .push_event(
            "info",
            "telegram_thread_create_options_sent",
            format!(
                "conversation={} field={} page={page} options={}",
                request.conversation_key,
                field,
                options.len()
            ),
        )
        .await;
    Ok(())
}

async fn send_telegram_thread_routing_list(
    state: &SharedState,
    adapter: &TelegramAdapter,
    message: &InboundMessage,
    existing_request: Option<ThreadRoutingRequestState>,
    cursor: Option<&str>,
    page: usize,
) -> Result<()> {
    let route = route_for_message(message);
    let loaded_page =
        load_thread_routing_page(state, existing_request.as_ref(), cursor, page).await?;
    let telegram_entries = loaded_page
        .entries
        .iter()
        .map(|entry| TelegramThreadListEntry {
            title: entry.title.clone(),
            summary: entry.summary.clone(),
            detail: entry.last_activity_text.clone(),
        })
        .collect::<Vec<_>>();

    let body = thread_list_body(loaded_page.model_provider_filter.as_deref());
    let message_id = adapter
        .send_thread_list(
            &route.chat_id,
            &loaded_page.request_id,
            "选择 Codex 会话",
            &body,
            &telegram_entries,
            loaded_page.page,
            loaded_page.page > 1,
            loaded_page.next_cursor.is_some(),
        )
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

async fn handle_telegram_thread_route_list_page(
    state: SharedState,
    adapter: TelegramAdapter,
    message: InboundMessage,
    request_id: &str,
    direction: ThreadRouteDirection,
) -> Result<()> {
    let Some(request) =
        checked_telegram_thread_routing_request(&state, &adapter, &message, request_id).await?
    else {
        return Ok(());
    };
    let target_page = match direction {
        ThreadRouteDirection::Prev => request.page.saturating_sub(1).max(1),
        ThreadRouteDirection::Next => request.page.saturating_add(1),
    };
    let cursor = request
        .page_cursors
        .get(target_page.saturating_sub(1))
        .cloned()
        .flatten();
    send_telegram_thread_routing_list(
        &state,
        &adapter,
        &message,
        Some(request),
        cursor.as_deref(),
        target_page,
    )
    .await
}

async fn handle_telegram_thread_route_resume_selected(
    state: SharedState,
    adapter: TelegramAdapter,
    message: InboundMessage,
    request_id: &str,
    thread_id: &str,
) -> Result<()> {
    let Some(_) =
        checked_telegram_thread_routing_request(&state, &adapter, &message, request_id).await?
    else {
        return Ok(());
    };
    adapter
        .send_text(
            &message.chat_id,
            &format!("正在订阅 thread `{thread_id}` 的后续事件..."),
        )
        .await?;
    let route = route_for_message(&message);
    let thread = resume_and_bind_thread(&state, &route, thread_id, Some(request_id)).await?;
    let body = format!(
        "已接入 thread `{thread_id}`。\n\n{}\n{}\n{}",
        summarize_thread_title(&thread),
        summarize_thread_cwd(&thread),
        summarize_thread_status(&thread)
    );
    adapter
        .send_thread_routing_result(&route.chat_id, "已订阅会话", &body)
        .await?;
    state
        .push_event(
            "info",
            "telegram_thread_route_resumed",
            format!("conversation={} thread={thread_id}", route.conversation_key),
        )
        .await;
    Ok(())
}

async fn handle_telegram_approval_text_reply(
    state: &SharedState,
    adapter: &TelegramAdapter,
    message: &InboundMessage,
    command: &str,
) -> Result<bool> {
    handle_telegram_approval_outcome(
        state,
        adapter,
        message,
        resolve_approval_reply(state, message, command).await,
    )
    .await
}

async fn handle_telegram_approval_button_reply(
    state: &SharedState,
    adapter: &TelegramAdapter,
    message: &InboundMessage,
    request_fingerprint: &str,
    option_index: usize,
) -> Result<bool> {
    handle_telegram_approval_outcome(
        state,
        adapter,
        message,
        resolve_approval_button_reply(state, message, request_fingerprint, option_index).await,
    )
    .await
}

async fn handle_telegram_approval_outcome(
    state: &SharedState,
    adapter: &TelegramAdapter,
    message: &InboundMessage,
    outcome: ApprovalReplyOutcome,
) -> Result<bool> {
    match outcome {
        ApprovalReplyOutcome::Ready {
            conversation_key,
            pending,
            option_index,
            decision,
        } => {
            let next = submit_approval_decision(state, &pending, &decision).await?;
            adapter
                .send_text(&message.chat_id, &format!("submitted: {}", decision.label))
                .await?;
            state
                .push_event(
                    "info",
                    "telegram_approval_decision_sent",
                    format!(
                        "conversation={} request_id={} option={} label={}",
                        conversation_key, pending.request_id, option_index, decision.label
                    ),
                )
                .await;
            if let Some((conversation_key, next_approval)) = next {
                events::send_next_telegram_approval(
                    state,
                    adapter,
                    &conversation_key,
                    &next_approval,
                )
                .await?;
            }
        }
        ApprovalReplyOutcome::NoPending => {
            adapter
                .send_text(&message.chat_id, "No pending approval.")
                .await?;
        }
        ApprovalReplyOutcome::NotCurrent => {
            adapter
                .send_text(&message.chat_id, "This approval is no longer current.")
                .await?;
        }
        ApprovalReplyOutcome::InvalidInput { hint } => {
            adapter
                .send_text(
                    &message.chat_id,
                    &format!("Invalid approval option. Reply {hint}."),
                )
                .await?;
        }
    }
    Ok(true)
}

pub(crate) fn command(text: &str) -> Option<String> {
    let first = text.split_whitespace().next()?.trim();
    if !first.starts_with('/') {
        return None;
    }
    let command = first
        .split_once('@')
        .map(|(command, _)| command)
        .unwrap_or(first)
        .to_ascii_lowercase();
    Some(command)
}

pub(crate) fn is_control_command(command: &str) -> bool {
    matches!(
        command,
        "/start" | "/help" | "/status" | "/new" | "/threads" | "/load" | "/s" | "/stop" | "/q"
    )
}

pub(crate) fn numeric_command_index(command: &str) -> Option<usize> {
    let number = command.strip_prefix('/')?.parse::<usize>().ok()?;
    number.checked_sub(1)
}

pub(crate) fn thread_list_body(model_provider_filter: Option<&str>) -> String {
    let mut body = "请选择一个会话接入后续事件。".to_string();
    if let Some(provider) = model_provider_filter {
        body.push_str(&format!(
            "\n已按当前 Codex App provider `{provider}` 过滤。"
        ));
    }
    body
}

pub(crate) fn thread_create_help_text(
    defaults: &ThreadCreateDefaults,
    draft: &ThreadCreateDraftState,
) -> String {
    let lines = vec![
        "创建新 Codex thread".to_string(),
        String::new(),
        "当前设置：".to_string(),
        format!("目录：{}", selected_cwd_text(defaults, draft)),
        format!(
            "Provider：{}",
            defaults
                .model_provider
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("使用 Codex App 当前 provider")
        ),
        format!("模型：{}", selected_model_text(defaults, draft)),
        format!("推理强度：{}", selected_effort_text(defaults, draft)),
        format!("权限：{}", selected_permission_text(defaults, draft)),
        String::new(),
        "点下面按钮修改设置，确认后点“创建”。".to_string(),
    ];
    lines.join("\n")
}

pub(crate) fn create_options_for_field(
    defaults: &ThreadCreateDefaults,
    draft: &ThreadCreateDraftState,
    field: &str,
) -> Result<(String, String, Vec<(String, TelegramCreateOption)>)> {
    match field {
        "cwd" => Ok(cwd_create_options(defaults, draft)),
        "model" => Ok(model_create_options(defaults, draft)),
        "effort" => Ok(effort_create_options(defaults, draft)),
        "perm" => Ok(permission_create_options(defaults, draft)),
        _ => Err(anyhow!("不支持的创建字段：{field}")),
    }
}

pub(crate) fn apply_thread_create_draft_value(
    draft: &mut ThreadCreateDraftState,
    field: &str,
    value: &str,
) -> Result<()> {
    let Some(field) = normalize_thread_create_field(field) else {
        return Err(anyhow!("不支持的创建字段：{field}"));
    };
    let value = value.trim();
    match field {
        "cwd" => {
            draft.cwd_custom = None;
            draft.cwd_choice = (!is_default_value(value)).then(|| value.to_string());
        }
        "model" => {
            draft.model = (!is_default_value(value)).then(|| value.to_string());
        }
        "effort" => {
            draft.effort = (!is_default_value(value)).then(|| value.to_string());
        }
        "perm" => {
            draft.permission = (!is_default_value(value)).then(|| value.to_string());
        }
        _ => return Err(anyhow!("不支持的创建字段：{field}")),
    }
    Ok(())
}

pub(crate) fn normalize_thread_create_field(field: &str) -> Option<&'static str> {
    match field.trim().to_ascii_lowercase().as_str() {
        "cwd" | "dir" | "path" => Some("cwd"),
        "model" => Some("model"),
        "effort" | "reasoning" | "reasoning_effort" => Some("effort"),
        "perm" | "permission" | "permissions" => Some("perm"),
        _ => None,
    }
}

fn cwd_create_options(
    defaults: &ThreadCreateDefaults,
    draft: &ThreadCreateDraftState,
) -> (String, String, Vec<(String, TelegramCreateOption)>) {
    let mut options = Vec::new();
    push_create_option(
        &mut options,
        "__default__",
        "使用 Codex App 默认目录",
        Some(
            defaults
                .cwd
                .as_deref()
                .map(|cwd| format!("当前默认：{cwd}"))
                .unwrap_or_else(|| "不覆盖 cwd，由 Codex App 决定".to_string()),
        ),
        draft.cwd_custom.is_none() && is_default_selection(draft.cwd_choice.as_deref()),
    );
    for project in defaults
        .projects
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        push_create_option(
            &mut options,
            project,
            &project_option_label(project),
            Some(project.to_string()),
            draft.cwd_custom.is_none() && draft.cwd_choice.as_deref() == Some(project),
        );
    }
    (
        "选择项目目录".to_string(),
        format!("当前：{}", selected_cwd_text(defaults, draft)),
        options,
    )
}

fn model_create_options(
    defaults: &ThreadCreateDefaults,
    draft: &ThreadCreateDraftState,
) -> (String, String, Vec<(String, TelegramCreateOption)>) {
    let mut options = Vec::new();
    push_create_option(
        &mut options,
        "__default__",
        "使用当前模型",
        Some(
            defaults
                .model
                .as_deref()
                .map(|model| format!("当前默认：{model}"))
                .unwrap_or_else(|| "不覆盖模型，由 Codex App 决定".to_string()),
        ),
        is_default_selection(draft.model.as_deref()),
    );
    for model in defaults
        .models
        .iter()
        .filter(|model| !model.value.trim().is_empty())
    {
        push_create_option(
            &mut options,
            &model.value,
            &model.label,
            None,
            draft.model.as_deref() == Some(model.value.as_str()),
        );
    }
    (
        "选择模型".to_string(),
        format!("当前：{}", selected_model_text(defaults, draft)),
        options,
    )
}

fn effort_create_options(
    defaults: &ThreadCreateDefaults,
    draft: &ThreadCreateDraftState,
) -> (String, String, Vec<(String, TelegramCreateOption)>) {
    let mut options = Vec::new();
    push_create_option(
        &mut options,
        "__default__",
        "使用模型默认推理强度",
        Some(
            defaults
                .effort
                .as_deref()
                .map(|effort| format!("当前默认：{}", reasoning_effort_label(effort)))
                .unwrap_or_else(|| "不覆盖推理强度，由模型决定".to_string()),
        ),
        is_default_selection(draft.effort.as_deref()),
    );
    if let Some(effort) = defaults
        .effort
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        push_create_option(
            &mut options,
            effort,
            &reasoning_effort_label(effort),
            None,
            draft.effort.as_deref() == Some(effort),
        );
    }
    for effort in defaults
        .efforts
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        push_create_option(
            &mut options,
            effort,
            &reasoning_effort_label(effort),
            None,
            draft.effort.as_deref() == Some(effort),
        );
    }
    (
        "选择推理强度".to_string(),
        format!("当前：{}", selected_effort_text(defaults, draft)),
        options,
    )
}

fn permission_create_options(
    defaults: &ThreadCreateDefaults,
    draft: &ThreadCreateDraftState,
) -> (String, String, Vec<(String, TelegramCreateOption)>) {
    let mut options = Vec::new();
    push_create_option(
        &mut options,
        "__default__",
        "使用 Codex App 当前权限",
        Some(
            defaults
                .permission
                .as_deref()
                .map(|permission| format!("当前：{permission}"))
                .unwrap_or_else(|| "不覆盖权限配置".to_string()),
        ),
        is_default_selection(draft.permission.as_deref()),
    );
    for (value, label, summary) in [
        (
            "workspace_user",
            "默认权限",
            "适合常规项目，需要时由用户确认。",
        ),
        ("auto_review", "自动审查", "需要审批时优先交给自动审查。"),
        (
            "full_access",
            "完全访问权限",
            "不再请求确认，允许完整本机访问。",
        ),
    ] {
        push_create_option(
            &mut options,
            value,
            label,
            Some(summary.to_string()),
            draft.permission.as_deref() == Some(value),
        );
    }
    (
        "选择权限".to_string(),
        format!("当前：{}", selected_permission_text(defaults, draft)),
        options,
    )
}

fn push_create_option(
    options: &mut Vec<(String, TelegramCreateOption)>,
    value: &str,
    label: &str,
    summary: Option<String>,
    selected: bool,
) {
    let value = value.trim();
    if value.is_empty() || options.iter().any(|(existing, _)| existing == value) {
        return;
    }
    let label = if selected {
        format!("已选：{}", label.trim())
    } else {
        label.trim().to_string()
    };
    let summary = match (selected, summary) {
        (true, Some(summary)) if !summary.trim().is_empty() => {
            Some(format!("已选 - {}", summary.trim()))
        }
        (true, _) => Some("已选".to_string()),
        (false, Some(summary)) if !summary.trim().is_empty() => Some(summary.trim().to_string()),
        _ => None,
    };
    options.push((value.to_string(), TelegramCreateOption { label, summary }));
}

fn is_default_selection(value: Option<&str>) -> bool {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_none_or(is_default_value)
}

fn is_default_value(value: &str) -> bool {
    matches!(value.trim(), "" | "__default__" | "default" | "默认")
}

fn selected_cwd_text(defaults: &ThreadCreateDefaults, draft: &ThreadCreateDraftState) -> String {
    if let Some(cwd) = draft
        .cwd_custom
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        return cwd.to_string();
    }
    if let Some(cwd) = draft
        .cwd_choice
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty() && !is_default_value(v))
    {
        if cwd == "__custom__" {
            return "等待输入自定义目录".to_string();
        }
        return cwd.to_string();
    }
    defaults
        .cwd
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|cwd| format!("使用 Codex App 默认目录（{cwd}）"))
        .unwrap_or_else(|| "使用 Codex App 默认目录".to_string())
}

fn selected_model_text(defaults: &ThreadCreateDefaults, draft: &ThreadCreateDraftState) -> String {
    if let Some(model) = draft
        .model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty() && !is_default_value(value))
    {
        return model.to_string();
    }
    defaults
        .model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|model| format!("使用当前模型（{model}）"))
        .unwrap_or_else(|| "使用当前模型".to_string())
}

fn selected_effort_text(defaults: &ThreadCreateDefaults, draft: &ThreadCreateDraftState) -> String {
    if let Some(effort) = draft
        .effort
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty() && !is_default_value(value))
    {
        return reasoning_effort_label(effort);
    }
    defaults
        .effort
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|effort| format!("使用默认推理强度（{}）", reasoning_effort_label(effort)))
        .unwrap_or_else(|| "使用模型默认推理强度".to_string())
}

fn selected_permission_text(
    defaults: &ThreadCreateDefaults,
    draft: &ThreadCreateDraftState,
) -> String {
    if let Some(permission) = draft
        .permission
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty() && !is_default_value(value))
    {
        return permission_label(permission);
    }
    defaults
        .permission
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|permission| format!("使用 Codex App 当前权限（{permission}）"))
        .unwrap_or_else(|| "使用 Codex App 当前权限".to_string())
}

fn project_option_label(path: &str) -> String {
    let name = Path::new(path)
        .file_name()
        .and_then(|value| value.to_str())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    match name {
        Some(name) => format!("{name} - {path}"),
        None => path.to_string(),
    }
}

fn reasoning_effort_label(effort: &str) -> String {
    match effort.trim() {
        "none" => "无 (none)".to_string(),
        "minimal" => "极低 (minimal)".to_string(),
        "low" => "低 (low)".to_string(),
        "medium" => "中 (medium)".to_string(),
        "high" => "高 (high)".to_string(),
        "xhigh" => "超高 (xhigh)".to_string(),
        other => other.to_string(),
    }
}

fn permission_label(permission: &str) -> String {
    match permission.trim() {
        "workspace_user" | "default" | "default_permissions" | "auto" => "默认权限".to_string(),
        "auto_review" | "guardian-approvals" | "guardian_approvals" => "自动审查".to_string(),
        "full_access" | "full-access" => "完全访问权限".to_string(),
        other => other.to_string(),
    }
}
