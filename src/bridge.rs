use anyhow::Result;
use std::path::PathBuf;
use tokio::{sync::mpsc, task::JoinSet};

use crate::{
    app_state::SharedState,
    codex::{approval_request_view, notification_thread_id},
    im::feishu::{FeishuApi, FeishuSettings, flow as feishu_flow, ws::listen_ws},
    im::telegram::{
        api::TelegramApi, flow as telegram_flow, polling::listen_polling, types::TelegramSettings,
    },
    im::wechat::{
        api::WechatApi, flow as wechat_flow, polling::listen_polling as listen_wechat_polling,
        types::WechatSettings,
    },
    im::{core::outbound, events},
    im_runtime::{PendingApproval, RouteTarget},
    remote_control_backend,
    types::{ImPlatformKind, InboundMessage},
};

pub async fn start_bridge(state: SharedState) {
    let config = state.config.lock().await.clone();
    let feishu_configured = config.feishu.enabled
        && !config.feishu.app_id.trim().is_empty()
        && !config.feishu.app_secret.trim().is_empty();
    let telegram_configured =
        config.telegram.enabled && !config.telegram.bot_token.trim().is_empty();
    let wechat_configured = config.wechat.enabled && !config.wechat.bot_token.trim().is_empty();
    if !feishu_configured && !telegram_configured && !wechat_configured {
        state
            .push_event(
                "error",
                "bridge_config_invalid",
                "no enabled IM channel is configured",
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
                "Codex remote-control is not connected yet; IM listener will still start",
            )
            .await;
    }

    let (tx, mut rx) = mpsc::channel::<InboundMessage>(128);
    let api = FeishuApi::new(FeishuSettings::from_app_config(&config.feishu));
    let telegram_api = TelegramApi::new(TelegramSettings::from_app_config(&config.telegram));
    let wechat_api = WechatApi::new(WechatSettings::from_app_config(&config.wechat));
    let generation = state.runtime.lock().await.start_bridge_generation();
    let (outbound_tx, outbound_rx) = outbound::channel();
    let mut tasks = JoinSet::new();
    tasks.spawn(outbound::run_worker(
        state.clone(),
        telegram_api.clone(),
        wechat_api.clone(),
        outbound_rx,
    ));
    tasks.spawn(codex_event_router(
        state.clone(),
        api.clone(),
        telegram_api.clone(),
        wechat_api.clone(),
        outbound_tx.clone(),
        generation,
    ));
    if feishu_configured {
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
        let feishu_tx = tx.clone();
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
                    feishu_tx.clone(),
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
    } else {
        let (kind, message) = if config.feishu.enabled {
            ("feishu_not_configured", "feishu app_id/app_secret is empty")
        } else {
            ("feishu_disabled", "feishu channel is disabled")
        };
        state.push_event("info", kind, message).await;
    }
    if !telegram_configured {
        let (kind, message) = if config.telegram.enabled {
            ("telegram_not_configured", "telegram bot_token is empty")
        } else {
            ("telegram_disabled", "telegram channel is disabled")
        };
        state.push_event("info", kind, message).await;
    } else {
        let telegram_state = state.clone();
        let telegram_tx = tx.clone();
        tasks.spawn(async move {
            loop {
                if !is_current_generation(&telegram_state, generation).await {
                    break;
                }
                telegram_state
                    .push_event("info", "telegram_polling_starting", "starting")
                    .await;
                match listen_polling(
                    telegram_state.clone(),
                    telegram_api.clone(),
                    telegram_tx.clone(),
                )
                .await
                {
                    Ok(()) => {
                        telegram_state
                            .push_event("warn", "telegram_polling_stopped", "polling stopped")
                            .await;
                    }
                    Err(err) => {
                        telegram_state
                            .push_event("error", "telegram_polling_failed", err.to_string())
                            .await;
                    }
                }
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        });
    }
    if !wechat_configured {
        let (kind, message) = if config.wechat.enabled {
            ("wechat_not_configured", "wechat bot_token is empty")
        } else {
            ("wechat_disabled", "wechat channel is disabled")
        };
        state.push_event("info", kind, message).await;
    } else {
        let wechat_state = state.clone();
        let wechat_tx = tx.clone();
        tasks.spawn(async move {
            loop {
                if !is_current_generation(&wechat_state, generation).await {
                    break;
                }
                wechat_state
                    .push_event("info", "wechat_polling_starting", "starting")
                    .await;
                match listen_wechat_polling(
                    wechat_state.clone(),
                    wechat_api.clone(),
                    wechat_tx.clone(),
                )
                .await
                {
                    Ok(()) => {
                        wechat_state
                            .push_event("warn", "wechat_polling_stopped", "polling stopped")
                            .await;
                    }
                    Err(err) => {
                        wechat_state
                            .push_event("error", "wechat_polling_failed", err.to_string())
                            .await;
                    }
                }
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        });
    }

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
        platform: ImPlatformKind::Feishu,
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
    if message.platform == ImPlatformKind::Telegram {
        return telegram_flow::handle_inbound(state, message).await;
    }
    if message.platform == ImPlatformKind::Wechat {
        return wechat_flow::handle_inbound(state, message).await;
    }
    feishu_flow::handle_inbound(state, api, message).await
}

fn attachment_root(state_path: &PathBuf) -> PathBuf {
    state_path
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".im")
        .join("attachments")
}

async fn codex_event_router(
    state: SharedState,
    api: FeishuApi,
    telegram_api: TelegramApi,
    wechat_api: WechatApi,
    outbound_tx: outbound::ImOutboundSender,
    generation: u64,
) {
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
        events::handle_codex_notification(
            state.clone(),
            api.clone(),
            telegram_api.clone(),
            wechat_api.clone(),
            outbound_tx.clone(),
            &notification,
        )
        .await;
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
                            let _ = events::send_next_approval(
                                &state,
                                &api,
                                &telegram_api,
                                &wechat_api,
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
                    message_id: None,
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
                let _ = events::send_approval(
                    &state,
                    &api,
                    &telegram_api,
                    &wechat_api,
                    &route,
                    &approval,
                )
                .await;
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
                        "thread={thread_id} platform={} account={} chat={} {approval_details}",
                        route.platform.key(),
                        route.account_id,
                        route.chat_id
                    ),
                )
                .await;
            continue;
        }
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
            summary.lines().find_map(|line| {
                line.strip_prefix("command: `")
                    .or_else(|| line.strip_prefix("命令：`"))
                    .and_then(|line| line.strip_suffix('`'))
                    .map(str::to_string)
            })
        })
        .unwrap_or_default();
    format!(
        "kind={request_kind} request_id={request_id} turn={turn} item={item} cwd={cwd} command={command}"
    )
}
