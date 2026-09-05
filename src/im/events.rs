use std::{path::PathBuf, time::Duration};

use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose};

use crate::{
    app_state::SharedState,
    chain_log,
    codex::{agent_message_is_final_answer, extract_agent_message_text, extract_turn_reply_text},
    config::TelegramReplyGranularity,
    im::{
        core::{
            accounts::ImApiRegistry,
            i18n::im_text_for_state,
            outbound::{
                ImOutboundImage, ImOutboundKind, ImOutboundMessage, ImOutboundPayload,
                ImOutboundSender,
            },
            text_renderer,
            text_utils::log_text_preview,
        },
        feishu::{
            FeishuAdapter, flow as feishu_flow, renderer,
            runtime::{
                self as feishu_runtime, complete_existing_item_card,
                ensure_started_streaming_card_state, upsert_streaming_card_state,
            },
        },
        telegram::{
            adapter::TelegramAdapter,
            api::{TelegramApiError, TelegramForumTopicEditOutcome},
            collab_progress as telegram_collab_progress, commentary as telegram_commentary,
            flow as telegram_flow, polling as telegram_polling, progress as telegram_progress,
            search as telegram_search, typing as telegram_typing,
        },
        wechat::adapter::WechatAdapter,
    },
    im_runtime::{
        PendingApproval, RouteTarget, TelegramCommandProgressSnapshot,
        TelegramThreadSettingsObservation, TelegramWebSearchProgressEntry, ThreadSettingsSnapshot,
        TurnOrigin,
    },
    types::ImPlatformKind,
};

const COMMAND_OUTPUT_PREVIEW_CHARS: usize = 2400;
const TURN_ERROR_SUMMARY_MAX_CHARS: usize = 600;
const TERMINAL_STATUS_FALLBACK_DELAY_MS: u64 = 60_000;

enum TelegramCommandTurn {
    Active(String),
    Missing,
    Stale { current: String, received: String },
}

/// 回复颗粒度的热查询；每次事件分发时读取，改配置即时生效。
async fn telegram_granularity(
    state: &SharedState,
    route: &RouteTarget,
) -> TelegramReplyGranularity {
    state
        .config
        .lock()
        .await
        .telegram_reply_granularity(&route.account_id)
}

/// 完整颗粒度下的独立事件消息：每条事件单独成一条 Telegram 消息，不进聚合气泡。
async fn send_telegram_standalone_item(
    state: &SharedState,
    outbound_tx: &ImOutboundSender,
    thread_id: &str,
    route: &RouteTarget,
    item_id: Option<&str>,
    markdown: &str,
) {
    let rendered = text_renderer::render_agent_message_body(markdown);
    if let Err(err) = outbound_tx.enqueue(ImOutboundMessage {
        thread_id: thread_id.to_string(),
        turn_id: None,
        route: route.clone(),
        item_id: item_id.map(str::to_string),
        item_type: Some("item".to_string()),
        kind: ImOutboundKind::Item,
        payload: ImOutboundPayload::Text(rendered),
    }) {
        state
            .push_event(
                "error",
                "telegram_standalone_item_enqueue_failed",
                format!("thread={thread_id} chat={} err={err}", route.chat_id),
            )
            .await;
    }
}

async fn telegram_command_turn(
    state: &SharedState,
    thread_id: &str,
    params: &serde_json::Value,
) -> TelegramCommandTurn {
    let received = params
        .get("turnId")
        .and_then(|value| value.as_str())
        .map(str::to_string);
    let current = state
        .runtime
        .lock()
        .await
        .current_turn_id(thread_id)
        .map(str::to_string);
    match (received, current) {
        (Some(received), Some(current)) if received != current => {
            TelegramCommandTurn::Stale { current, received }
        }
        (Some(received), Some(_)) => TelegramCommandTurn::Active(received),
        (None, Some(current)) => TelegramCommandTurn::Active(current),
        _ => TelegramCommandTurn::Missing,
    }
}

async fn update_telegram_task_progress(
    state: &SharedState,
    api_registry: &ImApiRegistry,
    thread_id: &str,
    route: &RouteTarget,
    params: &serde_json::Value,
    item_id: &str,
    item: &serde_json::Value,
    completed: bool,
) -> bool {
    let turn_id = match telegram_command_turn(state, thread_id, params).await {
        TelegramCommandTurn::Active(turn_id) => turn_id,
        TelegramCommandTurn::Missing => return false,
        TelegramCommandTurn::Stale { current, received } => {
            state
                .push_event(
                    "warn",
                    "telegram_command_progress_stale",
                    format!(
                        "thread={thread_id} current_turn={current} received_turn={received} item={item_id}"
                    ),
                )
                .await;
            return true;
        }
    };
    let is_mcp_tool = item.get("type").and_then(|value| value.as_str()) == Some("mcpToolCall");
    let entry = match (is_mcp_tool, completed) {
        (true, true) => telegram_progress::mcp_completed_entry(item_id, item),
        (true, false) => telegram_progress::mcp_running_entry(item_id, item),
        (false, true) => telegram_progress::completed_entry(item_id, item),
        (false, false) => telegram_progress::running_entry(item_id, item),
    };
    let snapshot = state
        .runtime
        .lock()
        .await
        .upsert_telegram_command_progress(thread_id, &turn_id, entry, completed);
    if let Some(snapshot) = snapshot {
        deliver_telegram_command_progress(state, api_registry, thread_id, route, snapshot).await;
    }
    true
}

async fn update_telegram_web_search_progress(
    state: &SharedState,
    api_registry: &ImApiRegistry,
    thread_id: &str,
    route: &RouteTarget,
    params: &serde_json::Value,
    item_id: &str,
    item: &serde_json::Value,
) -> bool {
    let turn_id = match telegram_command_turn(state, thread_id, params).await {
        TelegramCommandTurn::Active(turn_id) => turn_id,
        TelegramCommandTurn::Missing | TelegramCommandTurn::Stale { .. } => return true,
    };
    let Some(rendered) = telegram_search::render_web_search(item) else {
        return true;
    };
    let entry = TelegramWebSearchProgressEntry {
        item_id: item_id.to_string(),
        summary: rendered.summary,
        blocks: rendered.blocks,
        fallback_markdown: rendered.fallback_markdown,
    };
    let snapshot = state
        .runtime
        .lock()
        .await
        .upsert_telegram_web_search_progress(thread_id, &turn_id, entry);
    if let Some(snapshot) = snapshot {
        deliver_telegram_command_progress(state, api_registry, thread_id, route, snapshot).await;
    }
    true
}

async fn update_telegram_reasoning_progress(
    state: &SharedState,
    api_registry: &ImApiRegistry,
    thread_id: &str,
    route: &RouteTarget,
    params: &serde_json::Value,
    item_id: &str,
    item: &serde_json::Value,
) -> bool {
    let turn_id = match telegram_command_turn(state, thread_id, params).await {
        TelegramCommandTurn::Active(turn_id) => turn_id,
        TelegramCommandTurn::Missing => return true,
        TelegramCommandTurn::Stale { .. } => return true,
    };
    let summary = telegram_progress::reasoning_summary_from_item(item);
    let snapshot = state
        .runtime
        .lock()
        .await
        .complete_telegram_reasoning(thread_id, &turn_id, item_id, summary);
    if let Some(snapshot) = snapshot {
        deliver_telegram_command_progress(state, api_registry, thread_id, route, snapshot).await;
    }
    true
}

async fn update_telegram_plan_progress(
    state: &SharedState,
    api_registry: &ImApiRegistry,
    thread_id: &str,
    route: &RouteTarget,
    params: &serde_json::Value,
    deliver_update: bool,
) -> bool {
    let Some(turn_id) = params.get("turnId").and_then(|value| value.as_str()) else {
        return true;
    };
    if state.runtime.lock().await.current_turn_id(thread_id) != Some(turn_id) {
        return true;
    }
    let (explanation, plan) = telegram_progress::plan_from_params(params);
    let snapshot = state.runtime.lock().await.update_telegram_plan(
        thread_id,
        turn_id,
        explanation,
        plan,
        deliver_update,
    );
    if let Some(snapshot) = snapshot {
        deliver_telegram_command_progress(state, api_registry, thread_id, route, snapshot).await;
    }
    true
}

async fn update_telegram_diff_progress(
    state: &SharedState,
    api_registry: &ImApiRegistry,
    thread_id: &str,
    route: &RouteTarget,
    params: &serde_json::Value,
) -> bool {
    let Some(turn_id) = params.get("turnId").and_then(|value| value.as_str()) else {
        return true;
    };
    if state.runtime.lock().await.current_turn_id(thread_id) != Some(turn_id) {
        return true;
    }
    let diff = params
        .get("diff")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    // turn/diff/updated can be emitted for every hunk. Cache the latest
    // complete diff and flush it with the corresponding fileChange item or
    // terminal event instead of editing Telegram for every notification.
    let summary = telegram_progress::diff_summary_from_diff(diff);
    let snapshot = state
        .runtime
        .lock()
        .await
        .update_telegram_diff(thread_id, turn_id, summary, false);
    if let Some(snapshot) = snapshot {
        deliver_telegram_command_progress(state, api_registry, thread_id, route, snapshot).await;
    }
    true
}

async fn update_telegram_file_change_progress(
    state: &SharedState,
    api_registry: &ImApiRegistry,
    thread_id: &str,
    route: &RouteTarget,
    params: &serde_json::Value,
    item: &serde_json::Value,
) -> bool {
    let turn_id = match telegram_command_turn(state, thread_id, params).await {
        TelegramCommandTurn::Active(turn_id) => turn_id,
        TelegramCommandTurn::Missing => return true,
        TelegramCommandTurn::Stale { .. } => return true,
    };
    let fallback = telegram_progress::diff_summary_from_item(item);
    let snapshot = state
        .runtime
        .lock()
        .await
        .complete_telegram_file_change(thread_id, &turn_id, fallback);
    if let Some(snapshot) = snapshot {
        deliver_telegram_command_progress(state, api_registry, thread_id, route, snapshot).await;
    }
    true
}

async fn update_telegram_collab_progress(
    state: &SharedState,
    api_registry: &ImApiRegistry,
    thread_id: &str,
    route: &RouteTarget,
    params: &serde_json::Value,
    item_id: &str,
    item: &serde_json::Value,
) -> bool {
    let item_type = item.get("type").and_then(|value| value.as_str());
    if !item_type.is_some_and(telegram_collab_progress::is_collab_item_type) {
        return false;
    }
    let turn_id = match telegram_command_turn(state, thread_id, params).await {
        TelegramCommandTurn::Active(turn_id) => turn_id,
        TelegramCommandTurn::Missing => {
            state
                .push_event(
                    "info",
                    "telegram_collab_progress_skipped",
                    format!("thread={thread_id} item={item_id} reason=no_active_turn"),
                )
                .await;
            return true;
        }
        TelegramCommandTurn::Stale { current, received } => {
            state
                .push_event(
                    "warn",
                    "telegram_collab_progress_stale",
                    format!(
                        "thread={thread_id} current_turn={current} received_turn={received} item={item_id}"
                    ),
                )
                .await;
            return true;
        }
    };
    let updates = telegram_collab_progress::updates_for_item(item, crate::types::now_ms());
    if updates.is_empty() {
        state
            .push_event(
                "info",
                "telegram_collab_progress_suppressed",
                format!(
                    "thread={thread_id} turn={turn_id} item={item_id} type={}",
                    item_type.unwrap_or("unknown")
                ),
            )
            .await;
        return true;
    }
    let snapshot = state
        .runtime
        .lock()
        .await
        .upsert_telegram_collab_task_progress(thread_id, &turn_id, updates);
    if let Some(snapshot) = snapshot {
        deliver_telegram_command_progress(state, api_registry, thread_id, route, snapshot).await;
    }
    true
}

async fn finish_telegram_command_progress(
    state: &SharedState,
    api_registry: &ImApiRegistry,
    thread_id: &str,
    route: &RouteTarget,
    turn_id: Option<&str>,
    failed: bool,
) {
    let turn_id = match turn_id.map(str::to_string) {
        Some(turn_id) => Some(turn_id),
        None => state
            .runtime
            .lock()
            .await
            .current_turn_id(thread_id)
            .map(str::to_string),
    };
    let Some(turn_id) = turn_id else {
        return;
    };
    let snapshot = state
        .runtime
        .lock()
        .await
        .finish_telegram_command_progress_with_outcome(thread_id, &turn_id, failed);
    if let Some(snapshot) = snapshot {
        let Some(api) = api_registry.telegram_for_route(route) else {
            log_missing_api(state, route, "telegram_command_progress").await;
            return;
        };
        deliver_telegram_command_progress_with_api(state, api, thread_id, route, snapshot).await;
    }
}

pub(crate) async fn finish_telegram_command_progress_with_api(
    state: &SharedState,
    api: crate::im::telegram::api::TelegramApi,
    thread_id: &str,
    route: &RouteTarget,
    turn_id: &str,
) {
    let snapshot = state
        .runtime
        .lock()
        .await
        .finish_telegram_command_progress(thread_id, turn_id);
    if let Some(snapshot) = snapshot {
        deliver_telegram_command_progress_with_api(state, api, thread_id, route, snapshot).await;
    }
}

async fn start_next_telegram_queued_turn_if_ready(
    state: &SharedState,
    api_registry: &ImApiRegistry,
    route: &RouteTarget,
) {
    if route.platform != ImPlatformKind::Telegram {
        return;
    }
    let Some(api) = api_registry.telegram_for_route(route) else {
        log_missing_api(state, route, "telegram_queued_turn").await;
        return;
    };
    telegram_flow::start_next_telegram_queued_turn(state, api, route).await;
}

async fn finish_telegram_command_progress_with_api_and_outcome(
    state: &SharedState,
    api: crate::im::telegram::api::TelegramApi,
    thread_id: &str,
    route: &RouteTarget,
    turn_id: &str,
    failed: bool,
) {
    let snapshot = state
        .runtime
        .lock()
        .await
        .finish_telegram_command_progress_with_outcome(thread_id, turn_id, failed);
    if let Some(snapshot) = snapshot {
        deliver_telegram_command_progress_with_api(state, api, thread_id, route, snapshot).await;
    }
}

async fn deliver_telegram_command_progress(
    state: &SharedState,
    api_registry: &ImApiRegistry,
    thread_id: &str,
    route: &RouteTarget,
    snapshot: TelegramCommandProgressSnapshot,
) {
    let Some(api) = api_registry.telegram_for_route(route) else {
        log_missing_api(state, route, "telegram_command_progress").await;
        return;
    };
    deliver_telegram_command_progress_with_api(state, api, thread_id, route, snapshot).await;
}

async fn deliver_telegram_command_progress_with_api(
    state: &SharedState,
    api: crate::im::telegram::api::TelegramApi,
    thread_id: &str,
    route: &RouteTarget,
    snapshot: TelegramCommandProgressSnapshot,
) {
    let snapshot = state
        .runtime
        .lock()
        .await
        .claim_telegram_command_progress_delivery(thread_id, &snapshot.turn_id);
    let Some(snapshot) = snapshot else {
        return;
    };
    spawn_telegram_command_progress_driver(state, api, thread_id, route, snapshot);
}

fn spawn_telegram_command_progress_driver(
    state: &SharedState,
    api: crate::im::telegram::api::TelegramApi,
    thread_id: &str,
    route: &RouteTarget,
    snapshot: TelegramCommandProgressSnapshot,
) {
    let state = state.clone();
    let thread_id = thread_id.to_string();
    let route = route.clone();
    tokio::spawn(async move {
        telegram_command_progress_driver(state, api, thread_id, route, snapshot).await;
    });
}

async fn telegram_command_progress_driver(
    state: SharedState,
    api: crate::im::telegram::api::TelegramApi,
    thread_id: String,
    route: RouteTarget,
    mut snapshot: TelegramCommandProgressSnapshot,
) {
    let adapter = TelegramAdapter::new(api);
    let mut consecutive_failures = 0_u32;
    loop {
        let is_current = state
            .runtime
            .lock()
            .await
            .telegram_command_progress_delivery_is_current(
                &thread_id,
                &snapshot.turn_id,
                snapshot.revision,
            );
        if !is_current {
            return;
        }

        let rendered =
            telegram_progress::render_task_progress(&snapshot, im_text_for_state(&state));
        let result = adapter
            .send_or_update_rich_blocks(
                &route.chat_id,
                snapshot.message_id.as_deref(),
                rendered.blocks,
                &rendered.fallback_markdown,
            )
            .await;

        match result {
            Ok(message_id) => {
                consecutive_failures = 0;
                let next = state
                    .runtime
                    .lock()
                    .await
                    .complete_telegram_command_progress_delivery(
                        &thread_id,
                        &snapshot.turn_id,
                        snapshot.revision,
                        message_id.clone(),
                    );
                state
                    .push_event(
                        "info",
                        "telegram_command_progress_sent",
                        format!(
                            "thread={thread_id} turn={} chat={} message={} revision={} steps={} searches={} completed={}",
                            snapshot.turn_id,
                            route.chat_id,
                            message_id,
                            snapshot.revision,
                            snapshot.dropped_entries.saturating_add(snapshot.entries.len()),
                            snapshot
                                .dropped_web_searches
                                .saturating_add(snapshot.web_searches.len()),
                            snapshot.completed
                        ),
                    )
                    .await;
                let Some(next) = next else {
                    return;
                };
                snapshot = next;
            }
            Err(err) => {
                let retry_delay = telegram_command_progress_retry_delay_ms(
                    &err,
                    snapshot.message_id.is_some(),
                    snapshot.completed,
                );
                let delivery_retained = state
                    .runtime
                    .lock()
                    .await
                    .fail_telegram_command_progress_delivery(
                        &thread_id,
                        &snapshot.turn_id,
                        snapshot.revision,
                    );
                let should_retry =
                    delivery_retained || (!snapshot.completed && retry_delay.is_some());
                state
                    .push_event(
                        "warn",
                        "telegram_command_progress_failed",
                        format!(
                            "thread={thread_id} turn={} chat={} revision={} completed={} retry={} err={err}",
                            snapshot.turn_id,
                            route.chat_id,
                            snapshot.revision,
                            snapshot.completed,
                            should_retry
                        ),
                    )
                    .await;
                if !should_retry {
                    return;
                }
                consecutive_failures = consecutive_failures.saturating_add(1);
                let delay_ms = retry_delay.unwrap_or_else(|| {
                    let exponent = consecutive_failures.saturating_sub(1).min(6);
                    500_u64.saturating_mul(1_u64 << exponent).min(30_000)
                });
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                let next = {
                    let mut runtime = state.runtime.lock().await;
                    if delivery_retained {
                        runtime
                            .retry_telegram_command_progress_delivery(&thread_id, &snapshot.turn_id)
                    } else {
                        runtime
                            .claim_telegram_command_progress_delivery(&thread_id, &snapshot.turn_id)
                    }
                };
                let Some(next) = next else {
                    return;
                };
                snapshot = next;
            }
        }
    }
}

fn telegram_command_progress_retry_delay_ms(
    err: &anyhow::Error,
    has_message_id: bool,
    final_update: bool,
) -> Option<u64> {
    if let Some(api_err) = err.downcast_ref::<TelegramApiError>()
        && let Some(retry_after) = api_err.retry_after
    {
        return Some(retry_after.max(1).saturating_mul(1_000));
    }
    (has_message_id && final_update).then_some(500)
}

pub(crate) async fn send_next_approval(
    state: &SharedState,
    outbound_tx: &ImOutboundSender,
    conversation_key: &str,
    approval: &PendingApproval,
) -> Result<()> {
    let Some(route) = crate::im_runtime::route_from_conversation_key(conversation_key) else {
        state
            .push_event(
                "warn",
                "approval_next_route_missing",
                format!("conversation={conversation_key}"),
            )
            .await;
        return Ok(());
    };
    enqueue_approval(state, outbound_tx, &route, approval).await
}

pub(crate) async fn send_approval(
    state: &SharedState,
    outbound_tx: &ImOutboundSender,
    route: &RouteTarget,
    approval: &PendingApproval,
) -> Result<()> {
    enqueue_approval(state, outbound_tx, route, approval).await
}

async fn enqueue_approval(
    state: &SharedState,
    outbound_tx: &ImOutboundSender,
    route: &RouteTarget,
    approval: &PendingApproval,
) -> Result<()> {
    let request_key = approval.request_key();
    let thread_id = approval_thread_id(&approval.params)
        .unwrap_or_default()
        .to_string();
    let turn_id = approval_turn_id(&approval.params).map(str::to_string);
    let effective_turn_id = if turn_id.is_some() || thread_id.is_empty() {
        turn_id
    } else {
        state
            .runtime
            .lock()
            .await
            .current_turn_id(&thread_id)
            .map(str::to_string)
    };
    outbound_tx.enqueue(ImOutboundMessage {
        thread_id,
        turn_id: effective_turn_id,
        route: route.clone(),
        item_id: Some(request_key),
        item_type: Some("approval".to_string()),
        kind: ImOutboundKind::Approval,
        payload: ImOutboundPayload::Approval(approval.clone()),
    })?;
    state
        .push_event(
            "info",
            "approval_queued",
            format!(
                "platform={} conversation={} request_id={} chat={}",
                route.platform.key(),
                route.conversation_key,
                approval.request_id,
                route.chat_id
            ),
        )
        .await;
    Ok(())
}

fn approval_thread_id(params: &serde_json::Value) -> Option<&str> {
    params
        .get("threadId")
        .and_then(|value| value.as_str())
        .or_else(|| params.get("thread_id").and_then(|value| value.as_str()))
        .or_else(|| {
            params
                .get("thread")
                .and_then(|thread| thread.get("id"))
                .and_then(|value| value.as_str())
        })
        .or_else(|| {
            params
                .get("turn")
                .and_then(|turn| turn.get("threadId").or_else(|| turn.get("thread_id")))
                .and_then(|value| value.as_str())
        })
}

fn approval_turn_id(params: &serde_json::Value) -> Option<&str> {
    params
        .get("turnId")
        .and_then(|value| value.as_str())
        .or_else(|| params.get("turn_id").and_then(|value| value.as_str()))
        .or_else(|| {
            params
                .get("turn")
                .and_then(|turn| turn.get("id"))
                .and_then(|value| value.as_str())
        })
}

pub(crate) async fn send_turn_reply(
    state: &SharedState,
    api_registry: &ImApiRegistry,
    outbound_tx: Option<&ImOutboundSender>,
    thread_id: &str,
    turn_id: Option<&str>,
    route: &RouteTarget,
    item_id: Option<&str>,
    text: &str,
    is_final_answer: bool,
) {
    log_remote_to_im_enqueue(
        "turn_reply_input",
        thread_id,
        route,
        "",
        "agentMessage",
        text,
    );
    let (should_send, effective_turn_id) = {
        let mut runtime = state.runtime.lock().await;
        let effective_turn_id = turn_id
            .map(str::to_string)
            .or_else(|| runtime.current_turn_id(thread_id).map(str::to_string));
        let key = turn_reply_dedupe_key(
            route,
            thread_id,
            effective_turn_id.as_deref(),
            item_id,
            is_final_answer,
        );
        if runtime.should_skip_duplicate_text(&key, text) {
            (false, effective_turn_id)
        } else {
            runtime.remember_sent_text(&key, text);
            (true, effective_turn_id)
        }
    };
    if !should_send {
        let event_kind = format!("{}_turn_reply_skipped", route.platform.key());
        state
            .push_event(
                "info",
                &event_kind,
                format!(
                    "thread={thread_id} chat={} reason=duplicate text_len={}",
                    route.chat_id,
                    text.chars().count()
                ),
            )
            .await;
        return;
    }
    match route.platform {
        ImPlatformKind::Feishu => {
            let Some(api) = api_registry.feishu_for_route(route) else {
                log_missing_api(state, route, "turn_reply").await;
                return;
            };
            let text = feishu_runtime::resolve_agent_message_markdown_images(&api, text).await;
            let adapter = FeishuAdapter::new(api);
            if let Err(err) = adapter.send_turn_completed(&route.chat_id, &text).await {
                state
                    .push_event(
                        "error",
                        "feishu_turn_completed_failed",
                        format!("thread={thread_id} chat={} err={err}", route.chat_id),
                    )
                    .await;
            }
        }
        ImPlatformKind::Telegram => {
            let rendered = text_renderer::render_agent_message_body(text);
            if let Some(outbound_tx) = outbound_tx {
                if let Err(err) =
                    queue_agent_message_images(outbound_tx, thread_id, route, None, text)
                {
                    state
                        .push_event(
                            "error",
                            "telegram_agent_message_images_enqueue_failed",
                            format!("thread={thread_id} chat={} err={err}", route.chat_id),
                        )
                        .await;
                }
                log_remote_to_im_enqueue(
                    "turn_reply_enqueue",
                    thread_id,
                    route,
                    "",
                    "agentMessage",
                    &rendered,
                );
                let payload = if !is_final_answer {
                    let Some(turn_id) = effective_turn_id.as_deref() else {
                        return;
                    };
                    let item_id = item_id.unwrap_or(text);
                    let snapshot = state.runtime.lock().await.append_telegram_commentary(
                        thread_id,
                        turn_id,
                        item_id,
                        rendered.clone(),
                    );
                    let Some(snapshot) = snapshot else {
                        return;
                    };
                    let entries = snapshot
                        .entries
                        .iter()
                        .map(|entry| entry.text.clone())
                        .collect::<Vec<_>>();
                    let rendered = telegram_commentary::render_commentary(
                        &entries,
                        snapshot.dropped_entries,
                        im_text_for_state(state),
                    );
                    Some(ImOutboundPayload::TelegramCommentary {
                        segment: snapshot.segment,
                        rich_markdown: rendered.rich_markdown,
                        fallback_text: rendered.fallback_markdown,
                    })
                } else {
                    Some(ImOutboundPayload::Text(rendered))
                };
                let Some(payload) = payload else {
                    return;
                };
                if let Err(err) = outbound_tx.enqueue(ImOutboundMessage {
                    thread_id: thread_id.to_string(),
                    turn_id: effective_turn_id.clone(),
                    route: route.clone(),
                    item_id: item_id.map(str::to_string),
                    item_type: Some("agentMessage".to_string()),
                    kind: if is_final_answer {
                        ImOutboundKind::TurnReply
                    } else {
                        ImOutboundKind::Item
                    },
                    payload,
                }) {
                    state
                        .push_event(
                            "error",
                            "telegram_turn_enqueue_failed",
                            format!("thread={thread_id} chat={} err={err}", route.chat_id),
                        )
                        .await;
                }
            } else {
                let Some(api) = api_registry.telegram_for_route(route) else {
                    log_missing_api(state, route, "turn_reply").await;
                    return;
                };
                let adapter = TelegramAdapter::new(api);
                let (sent_event, failed_event) = if is_final_answer {
                    (
                        "telegram_turn_completed_sent",
                        "telegram_turn_completed_failed",
                    )
                } else {
                    ("telegram_item_sent", "telegram_item_failed")
                };
                let result = if is_final_answer {
                    adapter
                        .send_turn_completed(
                            &route.chat_id,
                            &rendered,
                            im_text_for_state(state).telegram_turn_completed_footer(),
                        )
                        .await
                } else {
                    adapter.send_text(&route.chat_id, &rendered).await
                };
                match result {
                    Ok(message_id) => {
                        state
                            .push_event(
                                "info",
                                sent_event,
                                format!(
                                    "thread={thread_id} chat={} message={message_id}",
                                    route.chat_id
                                ),
                            )
                            .await;
                    }
                    Err(err) => {
                        state
                            .push_event(
                                "error",
                                failed_event,
                                format!("thread={thread_id} chat={} err={err}", route.chat_id),
                            )
                            .await;
                    }
                }
            }
        }
        ImPlatformKind::Wechat => {
            let rendered = text_renderer::render_agent_message_text(text);
            if let Some(outbound_tx) = outbound_tx {
                log_remote_to_im_enqueue(
                    "turn_reply_enqueue",
                    thread_id,
                    route,
                    "",
                    "agentMessage",
                    &rendered,
                );
                if let Err(err) = outbound_tx.enqueue(ImOutboundMessage {
                    thread_id: thread_id.to_string(),
                    turn_id: effective_turn_id.clone(),
                    route: route.clone(),
                    item_id: None,
                    item_type: Some("agentMessage".to_string()),
                    kind: ImOutboundKind::TurnReply,
                    payload: ImOutboundPayload::Text(rendered),
                }) {
                    state
                        .push_event(
                            "error",
                            "wechat_turn_enqueue_failed",
                            format!("thread={thread_id} peer={} err={err}", route.chat_id),
                        )
                        .await;
                }
                if let Err(err) =
                    queue_agent_message_images(outbound_tx, thread_id, route, None, text)
                {
                    state
                        .push_event(
                            "error",
                            "wechat_agent_message_images_enqueue_failed",
                            format!("thread={thread_id} peer={} err={err}", route.chat_id),
                        )
                        .await;
                }
            } else {
                let Some(api) = api_registry.wechat_for_route(route) else {
                    log_missing_api(state, route, "turn_reply").await;
                    return;
                };
                let adapter = WechatAdapter::new(api);
                match adapter
                    .send_turn_completed(state, &route.account_id, &route.chat_id, &rendered)
                    .await
                {
                    Ok(message_id) => {
                        state
                            .push_event(
                                "info",
                                "wechat_turn_completed_sent",
                                format!(
                                    "thread={thread_id} peer={} message={message_id}",
                                    route.chat_id
                                ),
                            )
                            .await;
                    }
                    Err(err) => {
                        state
                            .push_event(
                                "error",
                                "wechat_turn_completed_failed",
                                format!("thread={thread_id} peer={} err={err}", route.chat_id),
                            )
                            .await;
                    }
                }
            }
        }
        ImPlatformKind::Wecom => {
            let rendered = text_renderer::render_agent_message_text(text);
            let Some(outbound_tx) = outbound_tx else {
                state
                    .push_event(
                        "error",
                        "wecom_turn_enqueue_failed",
                        format!(
                            "thread={thread_id} chat={} outbound queue unavailable",
                            route.chat_id
                        ),
                    )
                    .await;
                return;
            };
            if let Err(err) = outbound_tx.enqueue(ImOutboundMessage {
                thread_id: thread_id.to_string(),
                turn_id: effective_turn_id.clone(),
                route: route.clone(),
                item_id: None,
                item_type: Some("agentMessage".to_string()),
                kind: ImOutboundKind::TurnReply,
                payload: ImOutboundPayload::Text(rendered),
            }) {
                state
                    .push_event(
                        "error",
                        "wecom_turn_enqueue_failed",
                        format!("thread={thread_id} chat={} err={err}", route.chat_id),
                    )
                    .await;
            }
        }
    }
}

async fn start_telegram_agent_typing(
    state: &SharedState,
    api_registry: &ImApiRegistry,
    thread_id: &str,
    turn_id: Option<&str>,
    route: &RouteTarget,
) {
    let current_turn_id = state
        .runtime
        .lock()
        .await
        .current_turn_id(thread_id)
        .map(str::to_string);
    let Some(current_turn_id) = current_turn_id else {
        return;
    };
    if turn_id.is_some_and(|turn_id| turn_id != current_turn_id) {
        return;
    }
    let Some(api) = api_registry.telegram_for_route(route) else {
        log_missing_api(state, route, "agent_typing").await;
        return;
    };
    telegram_typing::start_turn(state, api, thread_id, &current_turn_id, route).await;
}

fn turn_reply_dedupe_key(
    route: &RouteTarget,
    thread_id: &str,
    turn_id: Option<&str>,
    item_id: Option<&str>,
    is_final_answer: bool,
) -> String {
    let turn_scope = turn_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(thread_id);
    let phase = if is_final_answer {
        "final-answer".to_string()
    } else if route.platform == ImPlatformKind::Telegram {
        let item_scope = item_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("unknown-item");
        format!("commentary:{item_scope}")
    } else {
        "commentary".to_string()
    };
    format!("{}:turn-reply:{turn_scope}:{phase}", route.conversation_key)
}

fn queue_agent_message_images(
    outbound_tx: &ImOutboundSender,
    thread_id: &str,
    route: &RouteTarget,
    item_id: Option<&str>,
    text: &str,
) -> Result<usize> {
    let images = text_renderer::local_markdown_image_refs(text)
        .into_iter()
        .map(|image| ImOutboundImage {
            path: image.path,
            caption: (!image.alt.trim().is_empty()).then_some(image.alt.clone()),
            fallback_text: Some(agent_message_image_fallback_text(&image.alt, &image.target)),
        })
        .collect();
    enqueue_image_payloads(
        outbound_tx,
        thread_id,
        route,
        item_id,
        "agentMessageImage",
        "agent-message-image",
        images,
    )
}

fn enqueue_image_payloads(
    outbound_tx: &ImOutboundSender,
    thread_id: &str,
    route: &RouteTarget,
    item_id: Option<&str>,
    item_type: &str,
    fallback_item_id_prefix: &str,
    images: Vec<ImOutboundImage>,
) -> Result<usize> {
    let count = images.len();
    if count == 0 {
        return Ok(0);
    }

    if route.platform == ImPlatformKind::Telegram && count > 1 {
        outbound_tx.enqueue(ImOutboundMessage {
            thread_id: thread_id.to_string(),
            turn_id: None,
            route: route.clone(),
            item_id: item_id
                .map(str::to_string)
                .or_else(|| Some(format!("{fallback_item_id_prefix}-group"))),
            item_type: Some(item_type.to_string()),
            kind: ImOutboundKind::ImageItem,
            payload: ImOutboundPayload::ImageGroup { images },
        })?;
        return Ok(count);
    }

    for (index, image) in images.into_iter().enumerate() {
        outbound_tx.enqueue(ImOutboundMessage {
            thread_id: thread_id.to_string(),
            turn_id: None,
            route: route.clone(),
            item_id: item_id
                .map(str::to_string)
                .or_else(|| Some(format!("{fallback_item_id_prefix}-{index}"))),
            item_type: Some(item_type.to_string()),
            kind: ImOutboundKind::ImageItem,
            payload: ImOutboundPayload::Image {
                path: image.path,
                caption: image.caption,
                fallback_text: image.fallback_text,
            },
        })?;
    }
    Ok(count)
}

fn agent_message_image_fallback_text(alt: &str, target: &str) -> String {
    let alt = alt.trim();
    let target = target.trim();
    match (alt.is_empty(), target.is_empty()) {
        (true, true) => "图片".to_string(),
        (true, false) => format!("图片：`{target}`"),
        (false, true) => format!("图片：{alt}"),
        (false, false) => format!("图片：{alt}（`{target}`）"),
    }
}

fn turn_error_value(params: &serde_json::Value) -> Option<&serde_json::Value> {
    params
        .get("error")
        .filter(|value| !value.is_null())
        .or_else(|| {
            params
                .get("turn")
                .and_then(|turn| turn.get("error"))
                .filter(|value| !value.is_null())
        })
}

async fn effective_terminal_turn_id(
    state: &SharedState,
    thread_id: &str,
    params: &serde_json::Value,
) -> Option<String> {
    if let Some(turn_id) = params
        .get("turnId")
        .and_then(|value| value.as_str())
        .or_else(|| {
            params
                .get("turn")
                .and_then(|turn| turn.get("id"))
                .and_then(|value| value.as_str())
        })
    {
        return Some(turn_id.to_string());
    }
    state
        .runtime
        .lock()
        .await
        .current_turn_id(thread_id)
        .map(str::to_string)
}

async fn terminal_turn_is_stale(
    state: &SharedState,
    thread_id: &str,
    turn_id: Option<&str>,
) -> bool {
    let Some(turn_id) = turn_id else {
        return false;
    };
    let current = state
        .runtime
        .lock()
        .await
        .current_turn_id(thread_id)
        .map(str::to_string);
    let Some(current) = current else {
        return false;
    };
    if current == turn_id {
        return false;
    }
    state
        .push_event(
            "warn",
            "codex_terminal_event_stale",
            format!("thread={thread_id} current_turn={current} received_turn={turn_id}"),
        )
        .await;
    true
}

fn turn_is_failed(params: &serde_json::Value) -> bool {
    if turn_error_value(params).is_some() {
        return true;
    }
    let status = params
        .get("turn")
        .and_then(|turn| turn.get("status"))
        .or_else(|| params.get("status"));
    match status {
        Some(value) if value.is_string() => matches!(
            value.as_str().unwrap_or_default(),
            "failed" | "error" | "systemError"
        ),
        Some(value) => value
            .get("type")
            .and_then(|value| value.as_str())
            .is_some_and(|value| matches!(value, "failed" | "error" | "systemError")),
        None => false,
    }
}

fn turn_failure_summary(params: &serde_json::Value) -> Option<String> {
    let error = turn_error_value(params)?;
    let message = error
        .as_str()
        .or_else(|| error.get("message").and_then(|value| value.as_str()))
        .or_else(|| error.get("error").and_then(|value| value.as_str()))
        .or_else(|| {
            error
                .get("additionalDetails")
                .and_then(|value| value.as_str())
        })
        .map(sanitize_turn_error_summary)
        .filter(|value| !value.is_empty());
    let status_code = find_http_status_code(error);
    match (message, status_code) {
        (Some(message), Some(status_code)) if !contains_status_code(&message, status_code) => {
            Some(truncate_chars(
                &format!("{status_code} {message}"),
                TURN_ERROR_SUMMARY_MAX_CHARS,
            ))
        }
        (Some(message), _) => Some(message),
        (None, Some(status_code)) => Some(status_code.to_string()),
        (None, None) => None,
    }
}

fn find_http_status_code(value: &serde_json::Value) -> Option<u16> {
    if let Some(object) = value.as_object() {
        for key in [
            "httpStatusCode",
            "http_status_code",
            "statusCode",
            "status_code",
        ] {
            if let Some(code) = object
                .get(key)
                .and_then(|value| value.as_u64())
                .and_then(|code| u16::try_from(code).ok())
                .filter(|code| (100..=599).contains(code))
            {
                return Some(code);
            }
        }
        for child in object.values() {
            if let Some(code) = find_http_status_code(child) {
                return Some(code);
            }
        }
    } else if let Some(array) = value.as_array() {
        for child in array {
            if let Some(code) = find_http_status_code(child) {
                return Some(code);
            }
        }
    }
    None
}

fn contains_status_code(text: &str, status_code: u16) -> bool {
    let code = status_code.to_string();
    text.split(|ch: char| !ch.is_ascii_digit())
        .any(|part| part == code)
}

fn sanitize_turn_error_summary(value: &str) -> String {
    let compact = value
        .replace("\r\n", " ")
        .replace(['\r', '\n'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let lower = compact.to_ascii_lowercase();
    let mut end = compact.len();
    for marker in [
        ", url:",
        "; url:",
        " url: http",
        ", request id:",
        "; request id:",
        " request id:",
        ", request_id:",
        "; request_id:",
        " request_id:",
        " https://",
        " http://",
    ] {
        if let Some(index) = lower.find(marker) {
            end = end.min(index);
        }
    }
    let compact = compact[..end]
        .trim()
        .replace("```", "'''")
        .replace('`', "'");
    truncate_chars(&compact, TURN_ERROR_SUMMARY_MAX_CHARS)
}

fn truncate_chars(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

async fn send_turn_terminal_mark(
    state: &SharedState,
    api_registry: &ImApiRegistry,
    outbound_tx: &ImOutboundSender,
    thread_id: &str,
    route: &RouteTarget,
    text: &str,
    failed: bool,
) {
    match route.platform {
        ImPlatformKind::Feishu => {
            let Some(api) = api_registry.feishu_for_route(route) else {
                log_missing_api(state, route, "turn_completed_mark").await;
                return;
            };
            let adapter = FeishuAdapter::new(api);
            match adapter.send_turn_completed_mark(&route.chat_id, text).await {
                Ok(message_id) => {
                    state
                        .push_event(
                            "info",
                            "feishu_turn_completed_mark_sent",
                            format!(
                                "thread={thread_id} chat={} message={message_id}",
                                route.chat_id
                            ),
                        )
                        .await;
                }
                Err(err) => {
                    state
                        .push_event(
                            "error",
                            "feishu_turn_completed_mark_failed",
                            format!("thread={thread_id} chat={} err={err}", route.chat_id),
                        )
                        .await;
                }
            }
        }
        ImPlatformKind::Telegram | ImPlatformKind::Wechat | ImPlatformKind::Wecom => {
            if let Err(err) = outbound_tx.enqueue(ImOutboundMessage {
                thread_id: thread_id.to_string(),
                turn_id: None,
                route: route.clone(),
                item_id: None,
                item_type: Some(if failed {
                    "turnFailed".to_string()
                } else {
                    "turnCompleted".to_string()
                }),
                kind: ImOutboundKind::TurnReply,
                payload: ImOutboundPayload::Text(text.to_string()),
            }) {
                state
                    .push_event(
                        "error",
                        "im_turn_completed_mark_enqueue_failed",
                        format!(
                            "thread={thread_id} platform={} chat={} err={err}",
                            route.platform.key(),
                            route.chat_id
                        ),
                    )
                    .await;
            }
        }
    }
}

async fn send_turn_terminal_mark_once(
    state: &SharedState,
    api_registry: &ImApiRegistry,
    outbound_tx: &ImOutboundSender,
    thread_id: &str,
    route: &RouteTarget,
    turn_id: Option<&str>,
    failed: bool,
    error_summary: Option<&str>,
) {
    let should_send = match turn_id {
        Some(turn_id) => state
            .runtime
            .lock()
            .await
            .claim_terminal_notice(turn_id, failed),
        None => true,
    };
    if !should_send {
        state
            .push_event(
                "info",
                "im_turn_terminal_mark_skipped",
                format!(
                    "thread={thread_id} platform={} chat={} reason=duplicate turn={} failed={failed}",
                    route.platform.key(),
                    route.chat_id,
                    turn_id.unwrap_or("")
                ),
            )
            .await;
        return;
    }
    if route.platform == ImPlatformKind::Telegram && !failed {
        state
            .push_event(
                "info",
                "im_turn_terminal_mark_skipped",
                format!(
                    "thread={thread_id} platform={} chat={} reason=telegram_success_covered_by_reply turn={}",
                    route.platform.key(),
                    route.chat_id,
                    turn_id.unwrap_or("")
                ),
            )
            .await;
        return;
    }
    let text = if failed {
        im_text_for_state(state).turn_failed_notice(error_summary)
    } else {
        im_text_for_state(state).turn_completed_notice().to_string()
    };
    send_turn_terminal_mark(
        state,
        api_registry,
        outbound_tx,
        thread_id,
        route,
        &text,
        failed,
    )
    .await;
}

async fn schedule_terminal_status_fallback(
    state: &SharedState,
    api_registry: &ImApiRegistry,
    outbound_tx: &ImOutboundSender,
    thread_id: &str,
    status_type: &str,
) {
    let (turn_id, route, fallback_token) = {
        let mut runtime = state.runtime.lock().await;
        let Some(turn_id) = runtime.current_turn_id(thread_id).map(str::to_string) else {
            return;
        };
        let Some(route) = runtime.route_for_thread(thread_id) else {
            return;
        };
        let Some(fallback_token) = runtime.start_terminal_status_fallback(thread_id, &turn_id)
        else {
            return;
        };
        (turn_id, route, fallback_token)
    };
    let state = state.clone();
    let api_registry = api_registry.clone();
    let outbound_tx = outbound_tx.clone();
    let thread_id = thread_id.to_string();
    let status_type = status_type.to_string();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(TERMINAL_STATUS_FALLBACK_DELAY_MS)).await;
        let command_snapshot = {
            let mut runtime = state.runtime.lock().await;
            if !runtime.claim_terminal_status_fallback(&thread_id, &turn_id, fallback_token) {
                return;
            }
            let failed = status_type == "systemError";

            (route.platform == ImPlatformKind::Telegram)
                .then(|| {
                    runtime
                        .finish_telegram_command_progress_with_outcome(&thread_id, &turn_id, failed)
                })
                .flatten()
        };
        let deliver_progress = route.platform == ImPlatformKind::Telegram
            && state.config.lock().await.telegram_reply_granularity(&route.account_id)
                == TelegramReplyGranularity::Standard;
        if let Some(snapshot) = command_snapshot
            && deliver_progress
        {
            deliver_telegram_command_progress(&state, &api_registry, &thread_id, &route, snapshot)
                .await;
        }
        if route.platform == ImPlatformKind::Telegram {
            if let Some(api) = api_registry.telegram_for_route(&route) {
                telegram_typing::finish_thread(&state, api, &thread_id, &route).await;
            } else {
                log_missing_api(&state, &route, "terminal_fallback_typing").await;
            }
        }
        if !state
            .runtime
            .lock()
            .await
            .mark_turn_completed(&thread_id, Some(&turn_id))
        {
            return;
        }
        if status_type == "systemError" {
            send_turn_terminal_mark_once(
                &state,
                &api_registry,
                &outbound_tx,
                &thread_id,
                &route,
                Some(&turn_id),
                true,
                None,
            )
            .await;
        }
        start_next_telegram_queued_turn_if_ready(&state, &api_registry, &route).await;
        state
            .push_event(
                "warn",
                "codex_terminal_status_fallback",
                format!("thread={thread_id} turn={turn_id} status={status_type}"),
            )
            .await;
    });
}

#[allow(dead_code)]
pub(crate) async fn handle_codex_notification(
    state: SharedState,
    api_registry: ImApiRegistry,
    outbound_tx: ImOutboundSender,
    notification: &crate::codex::CodexNotification,
) {
    let generation = state.runtime.lock().await.bridge_generation;
    handle_codex_notification_for_generation(
        state,
        api_registry,
        outbound_tx,
        notification,
        generation,
    )
    .await;
}

/// Handle a notification only for the bridge generation that received it.
/// Keeping the generation explicit lets the long-running automatic Topic
/// workflow exit when its router is superseded by a bridge restart.
pub(crate) async fn handle_codex_notification_for_generation(
    state: SharedState,
    api_registry: ImApiRegistry,
    outbound_tx: ImOutboundSender,
    notification: &crate::codex::CodexNotification,
    generation: u64,
) {
    if !state.runtime.lock().await.is_bridge_generation(generation) {
        return;
    }
    let Some(params) = notification.params.as_ref() else {
        return;
    };
    log_codex_to_im_handler(notification);
    match notification.method.as_str() {
        "thread/settings/updated" => {
            let Some(thread_id) = params.get("threadId").and_then(|value| value.as_str()) else {
                return;
            };
            let route = state.runtime.lock().await.route_for_thread(thread_id);
            let Some(route) = route else {
                return;
            };
            if notification
                .remote_client_key
                .as_deref()
                .is_some_and(|client_key| client_key != route.remote_client_key)
            {
                return;
            }
            let settings = params.get("threadSettings").unwrap_or(params);
            let outcome = state.runtime.lock().await.observe_thread_settings(
                thread_id,
                ThreadSettingsSnapshot::from_protocol_value(settings),
            );
            match outcome {
                TelegramThreadSettingsObservation::None => {}
                TelegramThreadSettingsObservation::Confirmed(request) => {
                    if let Some(api) = api_registry.telegram_for_route(&route) {
                        let adapter = TelegramAdapter::new(api);
                        let body = im_text_for_state(&state).telegram_thread_settings_applied();
                        let _ = adapter
                            .send_or_update_text(
                                &request.chat_id,
                                request.message_id.as_deref(),
                                body,
                            )
                            .await;
                        let _ = adapter
                            .clear_reply_markup(&request.chat_id, request.message_id.as_deref())
                            .await;
                    }
                }
                TelegramThreadSettingsObservation::Stale(request) => {
                    if let Some(api) = api_registry.telegram_for_route(&route) {
                        let adapter = TelegramAdapter::new(api);
                        let body = im_text_for_state(&state).telegram_thread_settings_stale();
                        let _ = adapter
                            .send_or_update_text(
                                &request.chat_id,
                                request.message_id.as_deref(),
                                body,
                            )
                            .await;
                        let _ = adapter
                            .clear_reply_markup(&request.chat_id, request.message_id.as_deref())
                            .await;
                    }
                }
            }
        }
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
            if route_for_codex_output(&state, &notification.method, thread_id, params)
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
        "error" => {
            let will_retry = params
                .get("willRetry")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            let Some(thread_id) = params.get("threadId").and_then(|value| value.as_str()) else {
                return;
            };
            let turn_id = effective_terminal_turn_id(&state, thread_id, params).await;
            if terminal_turn_is_stale(&state, thread_id, turn_id.as_deref()).await {
                return;
            }
            if will_retry {
                let Some(turn_id) = turn_id.as_deref() else {
                    return;
                };
                let route =
                    route_for_codex_output(&state, &notification.method, thread_id, params).await;
                let Some(route) = route.filter(|route| route.platform == ImPlatformKind::Telegram)
                else {
                    return;
                };
                let snapshot = state.runtime.lock().await.record_telegram_retry(
                    thread_id,
                    turn_id,
                    turn_failure_summary(params),
                );
                if let Some(snapshot) = snapshot {
                    deliver_telegram_command_progress(
                        &state,
                        &api_registry,
                        thread_id,
                        &route,
                        snapshot,
                    )
                    .await;
                }
                return;
            }
            if let Some(turn_id) = turn_id.as_deref() {
                state
                    .runtime
                    .lock()
                    .await
                    .cancel_terminal_status_fallback(thread_id, turn_id);
            }
            let route =
                route_for_codex_output(&state, &notification.method, thread_id, params).await;
            let Some(route) = route else {
                state
                    .runtime
                    .lock()
                    .await
                    .mark_turn_completed(thread_id, turn_id.as_deref());
                return;
            };
            if route.platform == ImPlatformKind::Telegram {
                if let Some(api) = api_registry.telegram_for_route(&route) {
                    telegram_typing::finish_thread(&state, api.clone(), thread_id, &route).await;
                    if let Some(turn_id) = turn_id.as_deref() {
                        finish_telegram_command_progress_with_api_and_outcome(
                            &state, api, thread_id, &route, turn_id, true,
                        )
                        .await;
                    }
                } else {
                    log_missing_api(&state, &route, "error_typing").await;
                }
            }
            let turn_completed = state
                .runtime
                .lock()
                .await
                .mark_turn_completed(thread_id, turn_id.as_deref());
            let summary = turn_failure_summary(params);
            send_turn_terminal_mark_once(
                &state,
                &api_registry,
                &outbound_tx,
                thread_id,
                &route,
                turn_id.as_deref(),
                true,
                summary.as_deref(),
            )
            .await;
            if turn_completed {
                start_next_telegram_queued_turn_if_ready(&state, &api_registry, &route).await;
            }
        }
        "thread/started" => {
            // Only the official Codex app's root client should create Topics
            // automatically. CLI and VS Code sessions keep their existing
            // explicit/manual routing behavior.
            if matches!(
                notification.remote_client_key.as_deref(),
                Some("default") | Some("default:codex_app")
            ) && match lifecycle_notification_thread_id(params) {
                Some(thread_id) => {
                    state
                        .observe_telegram_thread_started(&thread_id, generation)
                        .await
                }
                None => true,
            } {
                let state_for_auto_topic = state.clone();
                let api_registry_for_auto_topic = api_registry.clone();
                let remote_client_key =
                    notification.remote_client_key.clone().unwrap_or_else(|| {
                        crate::remote_control_backend::default_remote_client_key().to_string()
                    });
                let connection_epoch = notification.remote_connection_epoch;
                let params_for_auto_topic = params.clone();
                tokio::spawn(async move {
                    crate::im::telegram::polling::auto_create_topic_for_codex_thread_for_generation(
                        &state_for_auto_topic,
                        &api_registry_for_auto_topic,
                        &remote_client_key,
                        params_for_auto_topic,
                        generation,
                        connection_epoch,
                    )
                    .await;
                });
            }
        }
        "thread/archived" => {
            if let Some(thread_id) = lifecycle_notification_thread_id(params) {
                telegram_polling::archive_telegram_topic_for_codex_thread(
                    &state,
                    &api_registry,
                    &thread_id,
                    generation,
                )
                .await;
            }
        }
        "thread/deleted" => {
            if let Some(thread_id) = lifecycle_notification_thread_id(params) {
                telegram_polling::delete_telegram_topic_for_codex_thread(
                    &state,
                    &api_registry,
                    &thread_id,
                    generation,
                )
                .await;
            }
        }
        "thread/unarchived" => {
            if let Some(thread_id) = lifecycle_notification_thread_id(params) {
                telegram_polling::unarchive_telegram_topic_for_codex_thread(
                    &state, &thread_id, generation,
                )
                .await;
            }
        }
        "thread/name/updated" => {
            sync_codex_thread_name_to_telegram(&state, &api_registry, params).await;
        }
        "thread/status/changed" => {
            let Some(thread_id) = params.get("threadId").and_then(|v| v.as_str()) else {
                return;
            };
            let Some(status_type) = params
                .get("status")
                .and_then(|status| {
                    status
                        .get("type")
                        .and_then(|value| value.as_str())
                        .or_else(|| status.as_str())
                })
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                return;
            };
            if matches!(status_type, "idle" | "notLoaded" | "systemError") {
                schedule_terminal_status_fallback(
                    &state,
                    &api_registry,
                    &outbound_tx,
                    thread_id,
                    status_type,
                )
                .await;
            }
        }
        "turn/plan/updated" => {
            let Some(thread_id) = params.get("threadId").and_then(|v| v.as_str()) else {
                return;
            };
            let Some(route) =
                route_for_codex_output(&state, &notification.method, thread_id, params).await
            else {
                return;
            };
            if route.platform == ImPlatformKind::Telegram {
                match telegram_granularity(&state, &route).await {
                    TelegramReplyGranularity::Summary => {}
                    TelegramReplyGranularity::Standard => {
                        update_telegram_plan_progress(
                            &state,
                            &api_registry,
                            thread_id,
                            &route,
                            params,
                            true,
                        )
                        .await;
                    }
                    TelegramReplyGranularity::Full => {
                        let (explanation, plan) = telegram_progress::plan_from_params(params);
                        let text = im_text_for_state(&state);
                        if let Some(rendered) = telegram_progress::render_plan_standalone(
                            explanation.as_deref(),
                            &plan,
                            text,
                        ) {
                            send_telegram_standalone_item(
                                &state,
                                &outbound_tx,
                                thread_id,
                                &route,
                                None,
                                &rendered,
                            )
                            .await;
                        }
                    }
                }
            }
        }
        "turn/diff/updated" => {
            let Some(thread_id) = params.get("threadId").and_then(|v| v.as_str()) else {
                return;
            };
            let Some(route) =
                route_for_codex_output(&state, &notification.method, thread_id, params).await
            else {
                return;
            };
            if route.platform == ImPlatformKind::Telegram
                && telegram_granularity(&state, &route).await == TelegramReplyGranularity::Standard
            {
                update_telegram_diff_progress(&state, &api_registry, thread_id, &route, params)
                    .await;
            }
        }
        "item/reasoning/summaryPartAdded" => {
            // This notification only opens a new summary segment. There is no
            // text to send yet; the following summaryTextDelta fills it.
        }
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
            let route =
                route_for_codex_output(&state, &notification.method, thread_id, params).await;
            let Some(route) = route else {
                return;
            };
            if route.platform == ImPlatformKind::Telegram {
                if matches!(item_type, "commandExecution" | "mcpToolCall")
                    && telegram_granularity(&state, &route).await
                        == TelegramReplyGranularity::Standard
                {
                    let _ = update_telegram_task_progress(
                        &state,
                        &api_registry,
                        thread_id,
                        &route,
                        params,
                        item_id,
                        item,
                        false,
                    )
                    .await;
                }
                return;
            }
            if route.platform != ImPlatformKind::Feishu {
                return;
            }
            let Some(api) = api_registry.feishu_for_route(&route) else {
                log_missing_api(&state, &route, "item_started").await;
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
            let Some(delta) = params.get("delta").and_then(|v| v.as_str()) else {
                return;
            };
            let route =
                route_for_codex_output(&state, &notification.method, thread_id, params).await;
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
            if route.platform == ImPlatformKind::Wecom {
                let Some(api) = api_registry.wecom_for_route(&route) else {
                    log_missing_api(&state, &route, "wecom_agent_delta").await;
                    return;
                };
                send_wecom_stream_delta(&state, &api, thread_id, &route, delta, false).await;
                return;
            }
            if route.platform == ImPlatformKind::Telegram {
                start_telegram_agent_typing(
                    &state,
                    &api_registry,
                    thread_id,
                    params.get("turnId").and_then(|value| value.as_str()),
                    &route,
                )
                .await;
                return;
            }
            if route.platform != ImPlatformKind::Feishu {
                return;
            }
            let Some(item_id) = params.get("itemId").and_then(|value| value.as_str()) else {
                return;
            };
            let Some(api) = api_registry.feishu_for_route(&route) else {
                log_missing_api(&state, &route, "agent_delta").await;
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
            let route =
                route_for_codex_output(&state, &notification.method, thread_id, params).await;
            let Some(route) = route else {
                return;
            };
            if notification.method == "item/reasoning/summaryTextDelta"
                && route.platform == ImPlatformKind::Telegram
            {
                let Some(turn_id) = params.get("turnId").and_then(|value| value.as_str()) else {
                    return;
                };
                let summary_index = params
                    .get("summaryIndex")
                    .and_then(|value| value.as_i64())
                    .unwrap_or_default();
                state.runtime.lock().await.append_telegram_reasoning_delta(
                    thread_id,
                    turn_id,
                    item_id,
                    summary_index,
                    delta,
                );
                return;
            }
            if route.platform != ImPlatformKind::Feishu {
                return;
            }
            let Some(api) = api_registry.feishu_for_route(&route) else {
                log_missing_api(&state, &route, "reasoning_delta").await;
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
            let route =
                feishu_route_for_codex_output(&state, &notification.method, thread_id, params)
                    .await;
            let Some(route) = route else {
                return;
            };
            let Some(api) = api_registry.feishu_for_route(&route) else {
                log_missing_api(&state, &route, "plan_delta").await;
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
            if notification.method == "item/commandExecution/outputDelta" {
                return;
            }
            let Some(delta) = params.get("delta").and_then(|v| v.as_str()) else {
                return;
            };
            let kind = if notification.method == "item/commandExecution/outputDelta" {
                "commandExecution"
            } else {
                "fileChange"
            };
            let route =
                feishu_route_for_codex_output(&state, &notification.method, thread_id, params)
                    .await;
            let Some(route) = route else {
                return;
            };
            let Some(api) = api_registry.feishu_for_route(&route) else {
                log_missing_api(&state, &route, "output_delta").await;
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
            let route =
                feishu_route_for_codex_output(&state, &notification.method, thread_id, params)
                    .await;
            let Some(route) = route else {
                return;
            };
            let Some(api) = api_registry.feishu_for_route(&route) else {
                log_missing_api(&state, &route, "mcp_progress").await;
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
            let route =
                route_for_codex_output(&state, &notification.method, thread_id, params).await;
            let Some(route) = route else {
                return;
            };
            if route.platform == ImPlatformKind::Telegram {
                if matches!(item_type, "commandExecution" | "mcpToolCall")
                    && telegram_granularity(&state, &route).await
                        == TelegramReplyGranularity::Standard
                {
                    let _ = update_telegram_task_progress(
                        &state,
                        &api_registry,
                        thread_id,
                        &route,
                        params,
                        item_id,
                        item,
                        false,
                    )
                    .await;
                }
                return;
            }
            if route.platform != ImPlatformKind::Feishu {
                return;
            }
            let Some(api) = api_registry.feishu_for_route(&route) else {
                log_missing_api(&state, &route, "item_updated").await;
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
            let route =
                route_for_codex_output(&state, &notification.method, thread_id, params).await;
            let Some(route) = route else {
                return;
            };
            if route.platform == ImPlatformKind::Telegram
                && !matches!(
                    telegram_command_turn(&state, thread_id, params).await,
                    TelegramCommandTurn::Active(_)
                )
            {
                return;
            }
            if matches!(
                route.platform,
                ImPlatformKind::Telegram | ImPlatformKind::Wechat | ImPlatformKind::Wecom
            ) {
                if route.platform == ImPlatformKind::Telegram
                    && telegram_collab_progress::is_collab_item_type(item_type)
                {
                    match telegram_granularity(&state, &route).await {
                        TelegramReplyGranularity::Summary => {}
                        TelegramReplyGranularity::Standard => {
                            let _ = update_telegram_collab_progress(
                                &state,
                                &api_registry,
                                thread_id,
                                &route,
                                params,
                                item_id,
                                item,
                            )
                            .await;
                        }
                        TelegramReplyGranularity::Full => {
                            let text = im_text_for_state(&state);
                            let lines = telegram_collab_progress::render_collab_item_standalone(
                                item,
                                crate::types::now_ms(),
                                text,
                            );
                            if !lines.is_empty() {
                                send_telegram_standalone_item(
                                    &state,
                                    &outbound_tx,
                                    thread_id,
                                    &route,
                                    Some(item_id),
                                    &lines.join("\n\n"),
                                )
                                .await;
                            }
                        }
                    }
                    return;
                }
                if route.platform == ImPlatformKind::Telegram && item_type == "webSearch" {
                    match telegram_granularity(&state, &route).await {
                        TelegramReplyGranularity::Summary => {}
                        TelegramReplyGranularity::Standard => {
                            update_telegram_web_search_progress(
                                &state,
                                &api_registry,
                                thread_id,
                                &route,
                                params,
                                item_id,
                                item,
                            )
                            .await;
                        }
                        TelegramReplyGranularity::Full => {
                            if let Some(rendered) = telegram_search::render_web_search(item) {
                                send_telegram_standalone_item(
                                    &state,
                                    &outbound_tx,
                                    thread_id,
                                    &route,
                                    Some(item_id),
                                    &rendered.fallback_markdown,
                                )
                                .await;
                            }
                        }
                    }
                    return;
                }
                if route.platform == ImPlatformKind::Telegram && item_type == "reasoning" {
                    match telegram_granularity(&state, &route).await {
                        TelegramReplyGranularity::Summary => {}
                        TelegramReplyGranularity::Standard => {
                            update_telegram_reasoning_progress(
                                &state,
                                &api_registry,
                                thread_id,
                                &route,
                                params,
                                item_id,
                                item,
                            )
                            .await;
                        }
                        TelegramReplyGranularity::Full => {
                            if let Some(summary) =
                                telegram_progress::reasoning_summary_from_item(item)
                            {
                                let text = im_text_for_state(&state);
                                let rendered =
                                    format!("{}\n{}", text.telegram_reasoning_heading(), summary);
                                send_telegram_standalone_item(
                                    &state,
                                    &outbound_tx,
                                    thread_id,
                                    &route,
                                    Some(item_id),
                                    &rendered,
                                )
                                .await;
                            }
                        }
                    }
                    return;
                }
                if route.platform == ImPlatformKind::Telegram && item_type == "fileChange" {
                    match telegram_granularity(&state, &route).await {
                        TelegramReplyGranularity::Summary => {}
                        TelegramReplyGranularity::Standard => {
                            update_telegram_file_change_progress(
                                &state,
                                &api_registry,
                                thread_id,
                                &route,
                                params,
                                item,
                            )
                            .await;
                        }
                        TelegramReplyGranularity::Full => {
                            if let Some(diff) =
                                telegram_progress::file_change_diff_summary(item)
                            {
                                let text = im_text_for_state(&state);
                                let rendered =
                                    telegram_progress::render_diff_standalone(&diff, text);
                                send_telegram_standalone_item(
                                    &state,
                                    &outbound_tx,
                                    thread_id,
                                    &route,
                                    Some(item_id),
                                    &rendered,
                                )
                                .await;
                            }
                        }
                    }
                    return;
                }
                if route.platform == ImPlatformKind::Telegram && item_type == "plan" {
                    match telegram_granularity(&state, &route).await {
                        TelegramReplyGranularity::Summary => {}
                        TelegramReplyGranularity::Standard => {
                            let (explanation, plan) = telegram_progress::plan_from_item(item);
                            let turn_id = telegram_command_turn(&state, thread_id, params).await;
                            if let TelegramCommandTurn::Active(turn_id) = turn_id {
                                let snapshot = state.runtime.lock().await.update_telegram_plan(
                                    thread_id,
                                    &turn_id,
                                    explanation,
                                    plan,
                                    true,
                                );
                                if let Some(snapshot) = snapshot {
                                    deliver_telegram_command_progress(
                                        &state,
                                        &api_registry,
                                        thread_id,
                                        &route,
                                        snapshot,
                                    )
                                    .await;
                                }
                            }
                        }
                        TelegramReplyGranularity::Full => {
                            let (explanation, plan) = telegram_progress::plan_from_item(item);
                            let text = im_text_for_state(&state);
                            if let Some(rendered) = telegram_progress::render_plan_standalone(
                                explanation.as_deref(),
                                &plan,
                                text,
                            ) {
                                send_telegram_standalone_item(
                                    &state,
                                    &outbound_tx,
                                    thread_id,
                                    &route,
                                    Some(item_id),
                                    &rendered,
                                )
                                .await;
                            }
                        }
                    }
                    return;
                }
                if route.platform == ImPlatformKind::Telegram
                    && matches!(item_type, "commandExecution" | "mcpToolCall")
                {
                    // Commands and MCP calls belong to the aggregate progress
                    // message. Keep late events from falling through to the
                    // ordinary item sender after the turn has been cleaned up.
                    match telegram_granularity(&state, &route).await {
                        TelegramReplyGranularity::Summary => {}
                        TelegramReplyGranularity::Standard => {
                            let _ = update_telegram_task_progress(
                                &state,
                                &api_registry,
                                thread_id,
                                &route,
                                params,
                                item_id,
                                item,
                                true,
                            )
                            .await;
                        }
                        TelegramReplyGranularity::Full => {
                            let text = im_text_for_state(&state);
                            let rendered = telegram_progress::render_entry(
                                &telegram_progress::completed_entry(item_id, item),
                                text,
                                telegram_progress::TELEGRAM_COMMAND_PROGRESS_FAILURE_CHARS,
                            );
                            send_telegram_standalone_item(
                                &state,
                                &outbound_tx,
                                thread_id,
                                &route,
                                Some(item_id),
                                &rendered,
                            )
                            .await;
                        }
                    }
                    if item_type == "mcpToolCall"
                        && let Err(err) = queue_telegram_mcp_tool_images(
                            &state,
                            &outbound_tx,
                            thread_id,
                            &route,
                            item_id,
                            item,
                        )
                        .await
                    {
                        state
                            .push_event(
                                "error",
                                "telegram_mcp_tool_images_failed",
                                format!(
                                    "thread={thread_id} item={item_id} chat={} err={err}",
                                    route.chat_id
                                ),
                            )
                            .await;
                    }
                    return;
                }
                if route.platform == ImPlatformKind::Wecom && item_type == "agentMessage" {
                    if let Some(text) = extract_agent_message_text(item) {
                        let Some(api) = api_registry.wecom_for_route(&route) else {
                            log_missing_api(&state, &route, "wecom_agent_complete").await;
                            return;
                        };
                        send_wecom_stream_final(
                            &state,
                            &api,
                            thread_id,
                            &route,
                            Some(&text),
                            false,
                        )
                        .await;
                    }
                    return;
                }
                if item_type == "agentMessage"
                    && let Some(text) = extract_agent_message_text(item)
                {
                    let is_final_answer = agent_message_is_final_answer(item);
                    send_turn_reply(
                        &state,
                        &api_registry,
                        Some(&outbound_tx),
                        thread_id,
                        turn_id,
                        &route,
                        Some(item_id),
                        &text,
                        is_final_answer,
                    )
                    .await;
                } else if item_type == "userMessage" {
                    let should_forward = if let Some(turn_id) = turn_id {
                        state.runtime.lock().await.turn_origin(turn_id)
                            != turn_origin_for_platform(route.platform)
                    } else {
                        true
                    };
                    if should_forward {
                        let _ = send_text_im_codex_item(
                            &state,
                            &outbound_tx,
                            thread_id,
                            turn_id,
                            &route,
                            item_id,
                            item,
                        )
                        .await;
                    }
                } else if let Err(err) = send_text_im_codex_item(
                    &state,
                    &outbound_tx,
                    thread_id,
                    turn_id,
                    &route,
                    item_id,
                    item,
                )
                .await
                {
                    let event_kind = format!("{}_item_failed", route.platform.key());
                    state
                        .push_event(
                            "error",
                            &event_kind,
                            format!(
                                "thread={thread_id} item={item_id} type={item_type} chat={} err={err}",
                                route.chat_id
                            ),
                        )
                        .await;
                }
                return;
            }
            let Some(api) = api_registry.feishu_for_route(&route) else {
                log_missing_api(&state, &route, "item_completed").await;
                return;
            };
            if matches!(item_type, "imageGeneration" | "imageView")
                && feishu_flow::send_image_item_card(
                    &state, &api, &route, item_type, item, thread_id, item_id,
                )
                .await
            {
                return;
            }
            let kind = structured_streaming_kind(item_type).unwrap_or(item_type);
            let text = if item_type == "agentMessage" {
                extract_agent_message_text(item)
            } else if item_type == "commandExecution" {
                command_execution_full_text(item).or_else(|| renderer::item_markdown_summary(item))
            } else {
                renderer::item_markdown_summary(item)
            };
            if item_type == "agentMessage"
                && let Some(text) = text.as_deref()
            {
                let mut runtime = state.runtime.lock().await;
                let turn_id = turn_id.or_else(|| runtime.current_turn_id(thread_id));
                let key = turn_reply_dedupe_key(
                    &route,
                    thread_id,
                    turn_id,
                    Some(item_id),
                    agent_message_is_final_answer(item),
                );
                runtime.remember_sent_text(&key, text);
            }
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
                    let adapter = FeishuAdapter::new(api.clone());
                    if let Err(err) = adapter.send_interactive(&route.chat_id, &card).await {
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
                let adapter = FeishuAdapter::new(api.clone());
                if let Err(err) = adapter.send_interactive(&route.chat_id, &card).await {
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
            let effective_turn_id = effective_terminal_turn_id(&state, thread_id, params).await;
            if terminal_turn_is_stale(&state, thread_id, effective_turn_id.as_deref()).await {
                return;
            }
            if let Some(turn_id) = effective_turn_id.as_deref() {
                state
                    .runtime
                    .lock()
                    .await
                    .cancel_terminal_status_fallback(thread_id, turn_id);
            }
            let failed = turn_is_failed(params);
            let failure_summary = turn_failure_summary(params);
            let route =
                route_for_codex_output(&state, &notification.method, thread_id, params).await;
            let Some(route) = route else {
                state
                    .runtime
                    .lock()
                    .await
                    .mark_turn_completed(thread_id, effective_turn_id.as_deref());
                return;
            };
            if route.platform == ImPlatformKind::Telegram {
                if let Some(api) = api_registry.telegram_for_route(&route) {
                    telegram_typing::finish_thread(&state, api, thread_id, &route).await;
                } else {
                    log_missing_api(&state, &route, "turn_typing_final").await;
                }
                finish_telegram_command_progress(
                    &state,
                    &api_registry,
                    thread_id,
                    &route,
                    effective_turn_id.as_deref(),
                    failed,
                )
                .await;
            }
            let wecom_stream_finished = if route.platform == ImPlatformKind::Wecom {
                let stream = state
                    .runtime
                    .lock()
                    .await
                    .wecom_streams_by_thread
                    .get(&route.conversation_key)
                    .cloned();
                if stream.is_some()
                    && let Some(api) = api_registry.wecom_for_route(&route)
                {
                    send_wecom_stream_final(
                        &state,
                        &api,
                        thread_id,
                        &route,
                        extract_turn_reply_text(params).as_deref(),
                        true,
                    )
                    .await
                } else {
                    false
                }
            } else {
                false
            };
            if !wecom_stream_finished && let Some(text) = extract_turn_reply_text(params) {
                send_turn_reply(
                    &state,
                    &api_registry,
                    Some(&outbound_tx),
                    thread_id,
                    effective_turn_id.as_deref(),
                    &route,
                    None,
                    &text,
                    true,
                )
                .await;
            }
            let turn_completed = state
                .runtime
                .lock()
                .await
                .mark_turn_completed(thread_id, effective_turn_id.as_deref());
            send_turn_terminal_mark_once(
                &state,
                &api_registry,
                &outbound_tx,
                thread_id,
                &route,
                effective_turn_id.as_deref(),
                failed,
                failure_summary.as_deref(),
            )
            .await;
            if turn_completed {
                start_next_telegram_queued_turn_if_ready(&state, &api_registry, &route).await;
            }
        }
        _ => {}
    }
}

fn lifecycle_notification_thread_id(params: &serde_json::Value) -> Option<String> {
    params
        .get("threadId")
        .and_then(|value| value.as_str())
        .or_else(|| params.get("thread_id").and_then(|value| value.as_str()))
        .or_else(|| {
            params
                .get("thread")
                .and_then(|thread| thread.get("id"))
                .and_then(|value| value.as_str())
        })
        .or_else(|| params.get("id").and_then(|value| value.as_str()))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TelegramTopicNameDecision {
    Committed,
    StaleBinding,
    Superseded,
}

async fn commit_telegram_topic_name_if_current(
    state: &SharedState,
    conversation_key: &str,
    thread_id: &str,
    codex_title: &str,
    topic_name: &str,
    operation_token: u64,
) -> TelegramTopicNameDecision {
    let path = state.config.lock().await.state_path.clone();
    let operations = state.telegram_topic_name_update_ops.lock().await;
    if !operations
        .get(conversation_key)
        .is_some_and(|current| *current == operation_token)
    {
        return TelegramTopicNameDecision::Superseded;
    }
    let mut persisted = state.persisted.lock().await;
    if !persisted
        .im_thread_bindings
        .get(conversation_key)
        .is_some_and(|bound_thread_id| bound_thread_id == thread_id)
    {
        return TelegramTopicNameDecision::StaleBinding;
    }
    let Some(binding) = persisted
        .telegram_topic_binding_states
        .get_mut(conversation_key)
    else {
        return TelegramTopicNameDecision::StaleBinding;
    };
    if binding.thread_id != thread_id {
        return TelegramTopicNameDecision::StaleBinding;
    }
    binding.codex_title = codex_title.to_string();
    binding.topic_name = topic_name.to_string();
    binding.last_synced_codex_title = codex_title.to_string();
    binding.last_synced_topic_name = topic_name.to_string();
    binding.last_checked_at_ms = crate::types::now_ms();
    if let Err(err) = persisted.save(&path) {
        chain_log::write_diagnostic_lazy(|| {
            format!(
                "[telegram_topic] event=real_time_sync_save_failed key={} err={err}",
                conversation_key
            )
        });
    }
    TelegramTopicNameDecision::Committed
}

async fn telegram_topic_name_edit_is_current(
    state: &SharedState,
    route: &RouteTarget,
    thread_id: &str,
    operation_token: u64,
) -> bool {
    if !state
        .telegram_topic_name_update_is_current(&route.conversation_key, operation_token)
        .await
        || state
            .telegram_topic_cleanup_pending_for_account(&route.account_id)
            .await
    {
        return false;
    }

    let _binding_guard = state.im_route_binding_ops.lock().await;
    let runtime_route = state.runtime.lock().await.route_for_thread(thread_id);
    let runtime_matches = runtime_route.as_ref().is_some_and(|current| {
        current.conversation_key == route.conversation_key
            && current.account_id == route.account_id
            && current.chat_id == route.chat_id
    });
    if !runtime_matches {
        return false;
    }
    let persisted = state.persisted.lock().await;
    persisted
        .im_thread_bindings
        .get(&route.conversation_key)
        .is_some_and(|bound_thread_id| bound_thread_id == thread_id)
        && persisted
            .telegram_topic_binding_states
            .get(&route.conversation_key)
            .is_some_and(|binding| {
                binding.thread_id == thread_id && binding.codex_state == "active"
            })
}

async fn run_telegram_topic_name_edit_while_current<M, MutationFuture>(
    state: &SharedState,
    route: &RouteTarget,
    thread_id: &str,
    topic_name: &str,
    operation_token: u64,
    mutation: M,
) -> Option<Result<bool>>
where
    M: FnOnce() -> MutationFuture,
    MutationFuture: std::future::Future<Output = Result<TelegramForumTopicEditOutcome>>,
{
    telegram_polling::run_telegram_topic_mutation_while(
        state,
        &route.account_id,
        None,
        || telegram_topic_name_edit_is_current(state, route, thread_id, operation_token),
        || {
            telegram_polling::run_telegram_topic_edit_with_marker(
                state,
                &route.conversation_key,
                topic_name,
                mutation,
            )
        },
    )
    .await
}

async fn sync_codex_thread_name_to_telegram(
    state: &SharedState,
    api_registry: &ImApiRegistry,
    params: &serde_json::Value,
) {
    let Some(thread_id) = params.get("threadId").and_then(|value| value.as_str()) else {
        return;
    };
    let Some(thread_name) = params
        .get("threadName")
        .or_else(|| params.get("name"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return;
    };
    let Some(route) = route_for_codex_output(state, "thread/name/updated", thread_id, params).await
    else {
        return;
    };
    if route.platform != ImPlatformKind::Telegram {
        return;
    }
    let (_, Some(topic_id)) = crate::types::split_telegram_message_target(&route.chat_id) else {
        return;
    };
    let conversation_key = route.conversation_key.clone();
    let topic_name = crate::im::telegram::polling::truncate_topic_name(thread_name);
    let operation_token = state
        .begin_telegram_topic_name_update(&conversation_key)
        .await;

    let Some(api) = api_registry.telegram_for_route(&route) else {
        persist_codex_title_only(
            state,
            &conversation_key,
            thread_id,
            thread_name,
            operation_token,
        )
        .await;
        state
            .finish_telegram_topic_name_update(&conversation_key, operation_token)
            .await;
        log_missing_api(state, &route, "thread_name_sync").await;
        return;
    };

    let edit_result = run_telegram_topic_name_edit_while_current(
        state,
        &route,
        thread_id,
        &topic_name,
        operation_token,
        || {
            api.edit_forum_topic(
                crate::types::split_telegram_message_target(&route.chat_id).0,
                topic_id,
                &topic_name,
            )
        },
    )
    .await;
    let Some(edit_result) = edit_result else {
        persist_codex_title_only(
            state,
            &conversation_key,
            thread_id,
            thread_name,
            operation_token,
        )
        .await;
        state
            .finish_telegram_topic_name_update(&conversation_key, operation_token)
            .await;
        return;
    };
    match edit_result {
        Ok(true) => {
            let committed = commit_telegram_topic_name_if_current(
                state,
                &conversation_key,
                thread_id,
                thread_name,
                &topic_name,
                operation_token,
            )
            .await;
            if committed == TelegramTopicNameDecision::Committed {
                state
                    .push_event(
                        "info",
                        "codex_thread_name_synced_to_telegram",
                        format!("thread={} name={}", thread_id, topic_name),
                    )
                    .await;
            }
        }
        Ok(false) | Err(_) => {
            state
                .push_event(
                    "warn",
                    "codex_thread_name_sync_to_telegram_failed",
                    format!("thread={} name={}", thread_id, topic_name),
                )
                .await;
            persist_codex_title_only(
                state,
                &conversation_key,
                thread_id,
                thread_name,
                operation_token,
            )
            .await;
        }
    }
    state
        .finish_telegram_topic_name_update(&conversation_key, operation_token)
        .await;
}

async fn persist_codex_title_only(
    state: &SharedState,
    conversation_key: &str,
    thread_id: &str,
    title: &str,
    operation_token: u64,
) {
    let path = state.config.lock().await.state_path.clone();
    let operations = state.telegram_topic_name_update_ops.lock().await;
    if !operations
        .get(conversation_key)
        .is_some_and(|current| *current == operation_token)
    {
        return;
    }
    let mut persisted = state.persisted.lock().await;
    if !persisted
        .im_thread_bindings
        .get(conversation_key)
        .is_some_and(|bound_thread_id| bound_thread_id == thread_id)
    {
        return;
    }
    let Some(binding) = persisted
        .telegram_topic_binding_states
        .get_mut(conversation_key)
    else {
        return;
    };
    if binding.thread_id != thread_id {
        return;
    }
    binding.codex_title = title.to_string();
    binding.last_checked_at_ms = crate::types::now_ms();
    if let Err(err) = persisted.save(&path) {
        chain_log::write_diagnostic_lazy(|| {
            format!(
                "[telegram_topic] event=codex_title_save_failed key={} err={err}",
                conversation_key
            )
        });
    }
}

async fn send_wecom_stream_delta(
    state: &SharedState,
    api: &crate::im::wecom::WecomApi,
    thread_id: &str,
    route: &RouteTarget,
    delta: &str,
    finish: bool,
) {
    let key = route.conversation_key.clone();
    let driver = {
        let mut runtime = state.runtime.lock().await;
        let Some(stream) = runtime.wecom_streams_by_thread.get_mut(&key) else {
            return;
        };
        stream.content.push_str(delta);
        if finish {
            stream.finished = true;
        }
        stream.dirty = true;
        stream.revision = stream.revision.saturating_add(1);
        if stream.sending {
            None
        } else {
            stream.sending = true;
            Some((stream.req_id.clone(), stream.stream_id.clone()))
        }
    };
    if let Some((req_id, stream_id)) = driver {
        spawn_wecom_stream_driver(
            state,
            api,
            key,
            thread_id,
            route.chat_id.clone(),
            req_id,
            stream_id,
        );
    }
}

async fn send_wecom_stream_final(
    state: &SharedState,
    api: &crate::im::wecom::WecomApi,
    thread_id: &str,
    route: &RouteTarget,
    final_text: Option<&str>,
    cleanup_after_delivery: bool,
) -> bool {
    let key = route.conversation_key.clone();
    let driver = {
        let mut runtime = state.runtime.lock().await;
        let Some(stream) = runtime.wecom_streams_by_thread.get_mut(&key) else {
            return false;
        };
        let content_changed = final_text.is_some_and(|text| text != stream.content);
        if cleanup_after_delivery && stream.delivered && stream.finished && !content_changed {
            runtime.wecom_streams_by_thread.remove(&key);
            return true;
        }
        if let Some(final_text) = final_text {
            stream.content = final_text.to_string();
        }
        stream.finished = true;
        stream.cleanup_after_delivery |= cleanup_after_delivery;
        stream.dirty = true;
        stream.revision = stream.revision.saturating_add(1);
        if stream.sending {
            None
        } else {
            stream.sending = true;
            Some((stream.req_id.clone(), stream.stream_id.clone()))
        }
    };
    if let Some((req_id, stream_id)) = driver {
        spawn_wecom_stream_driver(
            state,
            api,
            key,
            thread_id,
            route.chat_id.clone(),
            req_id,
            stream_id,
        );
    }
    true
}

const WECOM_STREAM_UPDATE_INTERVAL: Duration = Duration::from_millis(220);

fn spawn_wecom_stream_driver(
    state: &SharedState,
    api: &crate::im::wecom::WecomApi,
    key: String,
    thread_id: &str,
    chat_id: String,
    req_id: String,
    stream_id: String,
) {
    let state = state.clone();
    let api = api.clone();
    let thread_id = thread_id.to_string();
    tokio::spawn(async move {
        wecom_stream_driver(state, api, key, thread_id, chat_id, req_id, stream_id).await;
    });
}

async fn wecom_stream_driver(
    state: SharedState,
    api: crate::im::wecom::WecomApi,
    key: String,
    thread_id: String,
    chat_id: String,
    expected_req_id: String,
    expected_stream_id: String,
) {
    loop {
        tokio::time::sleep(WECOM_STREAM_UPDATE_INTERVAL).await;
        let snapshot = {
            let mut runtime = state.runtime.lock().await;
            let Some(stream) = runtime.wecom_streams_by_thread.get_mut(&key) else {
                return;
            };
            if stream.req_id != expected_req_id || stream.stream_id != expected_stream_id {
                return;
            }
            if !stream.dirty {
                stream.sending = false;
                return;
            }
            stream.dirty = false;
            (
                stream.req_id.clone(),
                stream.stream_id.clone(),
                truncate_utf8_bytes(&stream.content, 20 * 1024),
                stream.finished,
                stream.revision,
            )
        };

        if let Err(err) = api
            .reply_stream(&snapshot.0, &snapshot.1, &snapshot.2, snapshot.3)
            .await
        {
            let mut runtime = state.runtime.lock().await;
            if let Some(stream) = runtime.wecom_streams_by_thread.get_mut(&key)
                && stream.req_id == expected_req_id
                && stream.stream_id == expected_stream_id
            {
                stream.sending = false;
                stream.dirty = false;
            }
            drop(runtime);
            state
                .push_event(
                    "warn",
                    if snapshot.3 {
                        "wecom_stream_final_failed"
                    } else {
                        "wecom_stream_failed"
                    },
                    format!("thread={thread_id} chat={chat_id} err={err}"),
                )
                .await;
            return;
        }

        let should_stop = {
            let mut runtime = state.runtime.lock().await;
            let Some(stream) = runtime.wecom_streams_by_thread.get_mut(&key) else {
                return;
            };
            if stream.req_id != expected_req_id || stream.stream_id != expected_stream_id {
                return;
            }
            stream.sent_content = snapshot.2;
            if snapshot.3 {
                stream.delivered = true;
            }
            if stream.cleanup_after_delivery && stream.delivered && !stream.dirty {
                runtime.wecom_streams_by_thread.remove(&key);
                true
            } else if stream.dirty {
                false
            } else {
                stream.sending = false;
                true
            }
        };
        if should_stop {
            return;
        }
    }
}

fn truncate_utf8_bytes(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        return value.to_string();
    }
    let mut end = limit;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

async fn route_for_codex_output(
    state: &SharedState,
    method: &str,
    thread_id: &str,
    _params: &serde_json::Value,
) -> Option<RouteTarget> {
    if let Some(route) = state.runtime.lock().await.route_for_thread(thread_id) {
        chain_log::write_diagnostic_lazy(|| {
            format!(
                "[im_route] event=codex_route_hit method={} thread={} platform={} account={} chat={} conversation={}",
                method,
                thread_id,
                route.platform.key(),
                route.account_id,
                route.chat_id,
                route.conversation_key
            )
        });
        return Some(route);
    }
    chain_log::write_line(format!(
        "[im_route] level=warn event=codex_route_missing method={} thread={}",
        method, thread_id
    ));
    None
}

async fn log_missing_api(state: &SharedState, route: &RouteTarget, context: &str) {
    state
        .push_event(
            "error",
            "im_api_missing",
            format!(
                "context={} platform={} account={} chat={}",
                context,
                route.platform.key(),
                route.account_id,
                route.chat_id
            ),
        )
        .await;
}

async fn feishu_route_for_codex_output(
    state: &SharedState,
    method: &str,
    thread_id: &str,
    params: &serde_json::Value,
) -> Option<RouteTarget> {
    let route = route_for_codex_output(state, method, thread_id, params).await?;
    if route.platform != ImPlatformKind::Feishu {
        chain_log::write_diagnostic_lazy(|| {
            format!(
                "[im_route] event=codex_route_platform_skip method={} thread={} wanted=feishu actual={} account={} chat={} conversation={}",
                method,
                thread_id,
                route.platform.key(),
                route.account_id,
                route.chat_id,
                route.conversation_key
            )
        });
        return None;
    }
    Some(route)
}

fn turn_origin_for_platform(platform: ImPlatformKind) -> Option<TurnOrigin> {
    match platform {
        ImPlatformKind::Feishu => Some(TurnOrigin::Feishu),
        ImPlatformKind::Telegram => Some(TurnOrigin::Telegram),
        ImPlatformKind::Wechat => Some(TurnOrigin::Wechat),
        ImPlatformKind::Wecom => Some(TurnOrigin::Wecom),
    }
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
    let output = truncate_command_output_preview(&output);

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

fn truncate_command_output_preview(text: &str) -> String {
    let mut out = String::new();
    let mut count = 0usize;
    for ch in text.chars() {
        if count >= COMMAND_OUTPUT_PREVIEW_CHARS {
            out.push_str("\n... output truncated ...");
            break;
        }
        out.push(ch);
        count += 1;
    }
    out
}

async fn send_text_im_codex_item(
    state: &SharedState,
    outbound_tx: &ImOutboundSender,
    thread_id: &str,
    turn_id: Option<&str>,
    route: &RouteTarget,
    item_id: &str,
    item: &serde_json::Value,
) -> Result<()> {
    let item_type = item
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let platform = route.platform.key();
    if route.platform == ImPlatformKind::Telegram && item_type == "webSearch" {
        state
            .push_event(
                "info",
                "telegram_item_skipped",
                format!(
                    "thread={thread_id} item={item_id} type={item_type} chat={} reason=handled_by_progress",
                    route.chat_id
                ),
            )
            .await;
        return Ok(());
    }
    let dedupe_payload = item.to_string();
    let should_send = {
        let mut runtime = state.runtime.lock().await;
        let key = format!("{}:item:{item_id}:{item_type}", route.conversation_key);
        if runtime.should_skip_duplicate_text(&key, &dedupe_payload) {
            false
        } else {
            runtime.remember_sent_text(&key, &dedupe_payload);
            true
        }
    };
    if !should_send {
        let event_kind = format!("{platform}_item_skipped");
        state
            .push_event(
                "info",
                &event_kind,
                format!(
                    "thread={thread_id} item={item_id} type={item_type} chat={} reason=duplicate",
                    route.chat_id
                ),
            )
            .await;
        return Ok(());
    }

    if matches!(item_type, "imageGeneration" | "imageView")
        && let Some(path) =
            text_im_image_path_for_item(state, platform, item_type, item, item_id).await?
    {
        let caption = text_renderer::image_item_caption(item);
        let fallback_text = text_renderer::render_item_text(item);
        outbound_tx.enqueue(ImOutboundMessage {
            thread_id: thread_id.to_string(),
            turn_id: None,
            route: route.clone(),
            item_id: Some(item_id.to_string()),
            item_type: Some(item_type.to_string()),
            kind: ImOutboundKind::ImageItem,
            payload: ImOutboundPayload::Image {
                path,
                caption: Some(caption),
                fallback_text,
            },
        })?;
        let event_kind = format!("{platform}_image_queued");
        state
            .push_event(
                "info",
                &event_kind,
                format!(
                    "thread={thread_id} item={item_id} type={item_type} chat={}",
                    route.chat_id
                ),
            )
            .await;
        return Ok(());
    }

    let text = if route.platform == ImPlatformKind::Telegram && item_type == "userMessage" {
        text_renderer::render_user_message_body(item)
    } else {
        text_renderer::render_item_text(item)
    };
    let Some(text) = text else {
        let event_kind = format!("{platform}_item_skipped");
        state
            .push_event(
                "info",
                &event_kind,
                format!(
                    "thread={thread_id} item={item_id} type={item_type} chat={} reason=empty_render",
                    route.chat_id
                ),
            )
            .await;
        return Ok(());
    };
    let mcp_tool_image_paths = if item_type == "mcpToolCall" {
        mcp_tool_image_paths_for_item(state, platform, item, item_id).await?
    } else {
        Vec::new()
    };

    // Capture the turn while the event is being queued. The outbound worker
    // may deliver this item later, after another turn has become current.
    let outbound_turn_id =
        if route.platform == ImPlatformKind::Telegram && item_type == "contextCompaction" {
            let runtime = state.runtime.lock().await;
            turn_id
                .map(str::to_string)
                .or_else(|| runtime.current_turn_id(thread_id).map(str::to_string))
        } else {
            turn_id.map(str::to_string)
        };

    log_remote_to_im_enqueue("item_enqueue", thread_id, route, item_id, item_type, &text);
    outbound_tx.enqueue(ImOutboundMessage {
        thread_id: thread_id.to_string(),
        turn_id: outbound_turn_id.clone(),
        route: route.clone(),
        item_id: Some(item_id.to_string()),
        item_type: Some(item_type.to_string()),
        kind: ImOutboundKind::Item,
        payload: ImOutboundPayload::Text(text.clone()),
    })?;
    if route.platform == ImPlatformKind::Telegram
        && item_type == "contextCompaction"
        && let Some(turn_id) = outbound_turn_id.as_deref()
    {
        state
            .runtime
            .lock()
            .await
            .start_new_telegram_commentary_segment(thread_id, turn_id);
    }
    let mcp_tool_image_count =
        queue_mcp_tool_images(outbound_tx, thread_id, route, item_id, mcp_tool_image_paths)?;
    let event_kind = format!("{platform}_item_queued");
    state
        .push_event(
            "info",
            &event_kind,
            format!(
                "thread={thread_id} item={item_id} type={item_type} chat={} text_len={}",
                route.chat_id,
                text.chars().count()
            ),
        )
        .await;
    if mcp_tool_image_count > 0 {
        let event_kind = format!("{platform}_mcp_tool_images_queued");
        state
            .push_event(
                "info",
                &event_kind,
                format!(
                    "thread={thread_id} item={item_id} type=mcpToolCall chat={} image_count={mcp_tool_image_count}",
                    route.chat_id
                ),
            )
            .await;
    }
    Ok(())
}

async fn queue_telegram_mcp_tool_images(
    state: &SharedState,
    outbound_tx: &ImOutboundSender,
    thread_id: &str,
    route: &RouteTarget,
    item_id: &str,
    item: &serde_json::Value,
) -> Result<()> {
    let paths = mcp_tool_image_paths_for_item(state, "telegram", item, item_id).await?;
    if paths.is_empty() {
        return Ok(());
    }

    let should_queue = {
        let mut runtime = state.runtime.lock().await;
        let key = format!(
            "{}:item:{item_id}:mcpToolCall:images",
            route.conversation_key
        );
        if runtime.should_skip_duplicate_text(&key, "queued") {
            false
        } else {
            runtime.remember_sent_text(&key, "queued");
            true
        }
    };
    if !should_queue {
        return Ok(());
    }

    let image_count = queue_mcp_tool_images(outbound_tx, thread_id, route, item_id, paths)?;
    state
        .push_event(
            "info",
            "telegram_mcp_tool_images_queued",
            format!(
                "thread={thread_id} item={item_id} type=mcpToolCall chat={} image_count={image_count}",
                route.chat_id
            ),
        )
        .await;
    Ok(())
}

fn queue_mcp_tool_images(
    outbound_tx: &ImOutboundSender,
    thread_id: &str,
    route: &RouteTarget,
    item_id: &str,
    paths: Vec<PathBuf>,
) -> Result<usize> {
    let images = paths
        .into_iter()
        .map(|path| ImOutboundImage {
            path,
            caption: None,
            fallback_text: None,
        })
        .collect();
    enqueue_image_payloads(
        outbound_tx,
        thread_id,
        route,
        Some(item_id),
        "mcpToolCall",
        "mcp-tool-image",
        images,
    )
}

fn log_codex_to_im_handler(notification: &crate::codex::CodexNotification) {
    if !chain_log::diagnostic_enabled() {
        return;
    }
    let Some(params) = notification.params.as_ref() else {
        return;
    };
    let thread_id = params
        .get("threadId")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let turn_id = params
        .get("turnId")
        .and_then(|v| v.as_str())
        .or_else(|| {
            params
                .get("turn")
                .and_then(|turn| turn.get("id"))
                .and_then(|v| v.as_str())
        })
        .unwrap_or("");
    let item = params.get("item");
    let item_id = item
        .and_then(|v| v.get("id"))
        .and_then(|v| v.as_str())
        .or_else(|| params.get("itemId").and_then(|v| v.as_str()))
        .unwrap_or("");
    let item_type = item
        .and_then(|v| v.get("type"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let text = trace_text_for_notification(&notification.method, params, item);
    chain_log::write_diagnostic_lazy(|| {
        format!(
            "[im_trace] event=codex_to_im_handler method={} thread={} turn={} item={} type={} text_len={} preview={}",
            notification.method,
            thread_id,
            turn_id,
            item_id,
            item_type,
            text.chars().count(),
            log_text_preview(&text, 500)
        )
    });
}

fn log_remote_to_im_enqueue(
    event: &str,
    thread_id: &str,
    route: &RouteTarget,
    item_id: &str,
    item_type: &str,
    text: &str,
) {
    chain_log::write_diagnostic_lazy(|| {
        format!(
            "[im_trace] event=remote_to_im_{} platform={} account={} chat={} thread={} item={} type={} text_len={} preview={}",
            event,
            route.platform.key(),
            route.account_id,
            route.chat_id,
            thread_id,
            item_id,
            item_type,
            text.chars().count(),
            log_text_preview(text, 500)
        )
    });
}

fn trace_text_for_notification(
    method: &str,
    params: &serde_json::Value,
    item: Option<&serde_json::Value>,
) -> String {
    if let Some(delta) = params.get("delta").and_then(|v| v.as_str()) {
        return delta.to_string();
    }
    if let Some(message) = params.get("message").and_then(|v| v.as_str()) {
        return message.to_string();
    }
    if let Some(item) = item {
        if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
            return text.to_string();
        }
        if let Some(text) = item.get("aggregatedOutput").and_then(|v| v.as_str()) {
            return text.to_string();
        }
        if method.contains("commandExecution")
            && let Some(command) = item
                .get("commandActions")
                .and_then(|v| v.as_array())
                .and_then(|actions| actions.first())
                .and_then(|action| action.get("command"))
                .and_then(|v| v.as_str())
                .or_else(|| item.get("command").and_then(|v| v.as_str()))
        {
            return command.to_string();
        }
        return item.to_string();
    }
    params.to_string()
}

async fn text_im_image_path_for_item(
    state: &SharedState,
    platform: &str,
    item_type: &str,
    item: &serde_json::Value,
    item_id: &str,
) -> Result<Option<PathBuf>> {
    if let Some(path) = text_renderer::image_item_path(item)
        && path.is_file()
    {
        return Ok(Some(path));
    }
    if item_type != "imageGeneration" {
        return Ok(None);
    }
    let Some(result) = item.get("result").and_then(|v| v.as_str()) else {
        return Ok(None);
    };
    let Some(decoded) = decode_image_string(result) else {
        return Ok(None);
    };
    Ok(Some(
        write_im_image_cache(state, platform, item_id, decoded).await?,
    ))
}

async fn mcp_tool_image_paths_for_item(
    state: &SharedState,
    platform: &str,
    item: &serde_json::Value,
    item_id: &str,
) -> Result<Vec<PathBuf>> {
    let Some(content) = item
        .get("result")
        .and_then(|result| result.get("content"))
        .and_then(|value| value.as_array())
    else {
        return Ok(Vec::new());
    };
    let mut paths = Vec::new();
    for (index, entry) in content
        .iter()
        .filter(|entry| entry.get("type").and_then(|value| value.as_str()) == Some("image"))
        .enumerate()
    {
        let Some(data) = entry.get("data").and_then(|value| value.as_str()) else {
            continue;
        };
        let mime_type = entry
            .get("mimeType")
            .or_else(|| entry.get("mime_type"))
            .and_then(|value| value.as_str());
        let Some(decoded) = decode_image_content(data, mime_type) else {
            continue;
        };
        let cache_key = format!("{item_id}-mcp-{index}");
        paths.push(write_im_image_cache(state, platform, &cache_key, decoded).await?);
    }
    Ok(paths)
}

async fn write_im_image_cache(
    state: &SharedState,
    platform: &str,
    item_id: &str,
    decoded: DecodedImage,
) -> Result<PathBuf> {
    let state_path = state.config.lock().await.state_path.clone();
    let root = state_path
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".im")
        .join("images");
    std::fs::create_dir_all(&root)
        .with_context(|| format!("failed to create image cache {}", root.display()))?;
    let path = root.join(format!(
        "{}-{}.{}",
        platform,
        safe_file_stem(item_id),
        decoded.extension
    ));
    std::fs::write(&path, decoded.bytes)
        .with_context(|| format!("failed to write image cache {}", path.display()))?;
    Ok(path)
}

struct DecodedImage {
    bytes: Vec<u8>,
    extension: &'static str,
}

fn decode_image_string(value: &str) -> Option<DecodedImage> {
    let trimmed = value.trim();
    if let Some((mime, payload)) = parse_image_data_url(trimmed) {
        let bytes = general_purpose::STANDARD.decode(payload).ok()?;
        let extension =
            image_extension_from_mime(mime).or_else(|| image_extension_from_bytes(&bytes))?;
        return Some(DecodedImage { bytes, extension });
    }
    if !looks_like_inline_image_base64(trimmed) {
        return None;
    }
    let bytes = general_purpose::STANDARD.decode(trimmed).ok()?;
    let extension = image_extension_from_bytes(&bytes)?;
    Some(DecodedImage { bytes, extension })
}

fn decode_image_content(value: &str, mime_type: Option<&str>) -> Option<DecodedImage> {
    let trimmed = value.trim();
    if let Some((mime, payload)) = parse_image_data_url(trimmed) {
        let bytes = general_purpose::STANDARD.decode(payload).ok()?;
        let extension =
            image_extension_from_mime(mime).or_else(|| image_extension_from_bytes(&bytes))?;
        return Some(DecodedImage { bytes, extension });
    }
    let bytes = general_purpose::STANDARD.decode(trimmed).ok()?;
    let extension = mime_type
        .and_then(image_extension_from_mime)
        .or_else(|| image_extension_from_bytes(&bytes))?;
    Some(DecodedImage { bytes, extension })
}

fn parse_image_data_url(value: &str) -> Option<(&str, &str)> {
    let (metadata, payload) = value.split_once(',')?;
    let metadata = metadata.strip_prefix("data:")?;
    let mut parts = metadata.split(';');
    let mime = parts.next()?;
    if !mime.starts_with("image/") || !parts.any(|part| part == "base64") {
        return None;
    }
    Some((mime, payload))
}

fn looks_like_inline_image_base64(value: &str) -> bool {
    value.len() > 1024
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '/' | '=' | '\r' | '\n'))
}

fn image_extension_from_mime(mime: &str) -> Option<&'static str> {
    match mime {
        "image/png" => Some("png"),
        "image/jpeg" | "image/jpg" => Some("jpg"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        _ => None,
    }
}

fn image_extension_from_bytes(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some("jpg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("gif")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("webp")
    } else {
        None
    }
}

fn safe_file_stem(value: &str) -> String {
    let stem = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if stem.trim_matches('_').is_empty() {
        "image".to_string()
    } else {
        stem
    }
}

#[cfg(test)]
mod tests {
    use reqwest::StatusCode;
    use serde_json::json;

    use super::*;
    use crate::{
        app_state::AppState,
        config::AppConfig,
        im::{
            core::outbound::{channel as outbound_channel, try_recv_for_test},
            wecom::{WecomApi, WecomSettings},
        },
        im_runtime::{RouteTarget, WecomStreamState},
    };

    fn test_state() -> SharedState {
        AppState::new(
            std::env::temp_dir().join(format!(
                "codexhub-wecom-stream-{}.toml",
                uuid::Uuid::new_v4()
            )),
            AppConfig::default(),
            None,
            None,
        )
    }

    fn test_api() -> WecomApi {
        WecomApi::new(WecomSettings {
            bot_id: "bot".to_string(),
            secret: "secret".to_string(),
            websocket_url: String::new(),
            allowed_user_ids: Vec::new(),
            allowed_chat_ids: Vec::new(),
        })
    }

    fn test_route() -> RouteTarget {
        RouteTarget {
            platform: ImPlatformKind::Wecom,
            conversation_key: "wecom:account:single:user".to_string(),
            account_id: "account".to_string(),
            chat_id: "single:user".to_string(),
            remote_client_key: "remote".to_string(),
        }
    }

    fn test_telegram_route() -> RouteTarget {
        RouteTarget {
            platform: ImPlatformKind::Telegram,
            conversation_key: "telegram:account:chat".to_string(),
            account_id: "account".to_string(),
            chat_id: "chat".to_string(),
            remote_client_key: "remote".to_string(),
        }
    }

    async fn bind_active_telegram_topic(
        state: &SharedState,
        route: &RouteTarget,
        thread_id: &str,
        topic_name: &str,
    ) {
        state
            .runtime
            .lock()
            .await
            .bind_route(thread_id, route.clone());
        let mut persisted = state.persisted.lock().await;
        persisted
            .im_thread_bindings
            .insert(route.conversation_key.clone(), thread_id.to_string());
        persisted.telegram_topic_binding_states.insert(
            route.conversation_key.clone(),
            crate::store::TelegramTopicBindingState {
                thread_id: thread_id.to_string(),
                topic_name: topic_name.to_string(),
                codex_state: "active".to_string(),
                ..Default::default()
            },
        );
    }

    #[test]
    fn telegram_command_progress_retry_honors_server_retry_after() {
        let error = anyhow::Error::from(TelegramApiError {
            method: "editMessageText".to_string(),
            status: StatusCode::TOO_MANY_REQUESTS,
            error_code: Some(429),
            description: "Too Many Requests".to_string(),
            retry_after: Some(30),
        });

        assert_eq!(
            telegram_command_progress_retry_delay_ms(&error, false, false),
            Some(30_000)
        );
    }

    #[test]
    fn lifecycle_notifications_extract_supported_thread_id_shapes() {
        assert_eq!(
            lifecycle_notification_thread_id(&json!({"threadId": " thread-direct "})).as_deref(),
            Some("thread-direct")
        );
        assert_eq!(
            lifecycle_notification_thread_id(&json!({"thread": {"id": "thread-nested"}}))
                .as_deref(),
            Some("thread-nested")
        );
        assert_eq!(
            lifecycle_notification_thread_id(&json!({"thread_id": "thread-snake"})).as_deref(),
            Some("thread-snake")
        );
        assert_eq!(
            lifecycle_notification_thread_id(&json!({"id": "thread-id"})).as_deref(),
            Some("thread-id")
        );
        assert!(lifecycle_notification_thread_id(&json!({"threadId": "  "})).is_none());
    }

    #[tokio::test]
    async fn confirmed_codex_name_update_commits_only_the_current_binding() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let mut config = AppConfig::default();
        config.state_path = temp_dir.path().join("state.json");
        let state = AppState::new(temp_dir.path().join("config.toml"), config, None, None);
        let conversation_key = "telegram:bot:-100|topic=41";
        {
            let mut persisted = state.persisted.lock().await;
            persisted
                .im_thread_bindings
                .insert(conversation_key.to_string(), "thread-name".to_string());
            persisted.telegram_topic_binding_states.insert(
                conversation_key.to_string(),
                crate::store::TelegramTopicBindingState {
                    thread_id: "thread-name".to_string(),
                    topic_name: "Same title".to_string(),
                    codex_state: "archived".to_string(),
                    lifecycle_revision: 7,
                    ..Default::default()
                },
            );
        }
        let operation_token = state
            .begin_telegram_topic_name_update(conversation_key)
            .await;

        assert_eq!(
            commit_telegram_topic_name_if_current(
                &state,
                conversation_key,
                "thread-name",
                "Same title with full Codex metadata",
                "Same title",
                operation_token,
            )
            .await,
            TelegramTopicNameDecision::Committed
        );
        {
            let persisted = state.persisted.lock().await;
            let binding = &persisted.telegram_topic_binding_states[conversation_key];
            assert_eq!(binding.codex_title, "Same title with full Codex metadata");
            assert_eq!(binding.last_synced_topic_name, "Same title");
            assert_eq!(binding.codex_state, "archived");
            assert_eq!(binding.lifecycle_revision, 7);
        }

        state
            .persisted
            .lock()
            .await
            .im_thread_bindings
            .insert(conversation_key.to_string(), "newer-thread".to_string());
        assert_eq!(
            commit_telegram_topic_name_if_current(
                &state,
                conversation_key,
                "thread-name",
                "Stale title",
                "Same title",
                operation_token,
            )
            .await,
            TelegramTopicNameDecision::StaleBinding
        );
        assert_eq!(
            state.persisted.lock().await.telegram_topic_binding_states[conversation_key]
                .codex_title,
            "Same title with full Codex metadata"
        );
    }

    #[tokio::test]
    async fn superseded_topic_name_update_cannot_mutate_or_commit_its_old_name() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let mut config = AppConfig::default();
        config.state_path = temp_dir.path().join("state.json");
        let state = AppState::new(temp_dir.path().join("config.toml"), config, None, None);
        let route = RouteTarget {
            platform: ImPlatformKind::Telegram,
            conversation_key: "telegram:bot:-100|topic=51".to_string(),
            account_id: "bot".to_string(),
            chat_id: "-100|topic=51".to_string(),
            remote_client_key: "im:telegram:owner".to_string(),
        };
        bind_active_telegram_topic(&state, &route, "thread-name", "Before").await;
        let mutation_gate = state.telegram_topic_mutation_gate(&route.account_id).await;
        let guard = mutation_gate.lock().await;
        let old_token = state
            .begin_telegram_topic_name_update(&route.conversation_key)
            .await;
        let executed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let task = tokio::spawn({
            let state = state.clone();
            let route = route.clone();
            let executed = executed.clone();
            async move {
                telegram_polling::run_telegram_topic_mutation_while(
                    &state,
                    &route.account_id,
                    None,
                    || {
                        telegram_topic_name_edit_is_current(
                            &state,
                            &route,
                            "thread-name",
                            old_token,
                        )
                    },
                    || async move {
                        executed.store(true, std::sync::atomic::Ordering::SeqCst);
                        Ok(())
                    },
                )
                .await
            }
        });
        tokio::task::yield_now().await;
        let new_token = state
            .begin_telegram_topic_name_update(&route.conversation_key)
            .await;
        drop(guard);

        let result = tokio::time::timeout(Duration::from_millis(100), task)
            .await
            .expect("superseded update leaves the mutation queue")
            .expect("name update task");
        assert!(result.is_none());
        assert!(!executed.load(std::sync::atomic::Ordering::SeqCst));
        assert!(
            state
                .telegram_topic_name_update_is_current(&route.conversation_key, new_token)
                .await
        );

        assert_eq!(
            commit_telegram_topic_name_if_current(
                &state,
                &route.conversation_key,
                "thread-name",
                "Old title",
                "Old title",
                old_token,
            )
            .await,
            TelegramTopicNameDecision::Superseded
        );
        assert_eq!(
            state.persisted.lock().await.telegram_topic_binding_states[&route.conversation_key]
                .topic_name,
            "Before"
        );
    }

    #[tokio::test]
    async fn stale_realtime_failure_cannot_overwrite_a_telegram_originated_name() {
        let state = test_state();
        let route = RouteTarget {
            platform: ImPlatformKind::Telegram,
            conversation_key: "telegram:bot:-100|topic=55".to_string(),
            account_id: "bot".to_string(),
            chat_id: "-100|topic=55".to_string(),
            remote_client_key: "im:telegram:owner".to_string(),
        };
        bind_active_telegram_topic(&state, &route, "thread-name", "A").await;
        let stale_c_token = state
            .begin_telegram_topic_name_update(&route.conversation_key)
            .await;
        let telegram_b_token = state
            .begin_telegram_topic_name_update(&route.conversation_key)
            .await;
        {
            let mut persisted = state.persisted.lock().await;
            let binding = persisted
                .telegram_topic_binding_states
                .get_mut(&route.conversation_key)
                .expect("binding");
            binding.codex_title = "B".to_string();
            binding.topic_name = "B".to_string();
            binding.last_synced_codex_title = "B".to_string();
            binding.last_synced_topic_name = "B".to_string();
        }

        persist_codex_title_only(
            &state,
            &route.conversation_key,
            "thread-name",
            "C",
            stale_c_token,
        )
        .await;

        let persisted = state.persisted.lock().await;
        let binding = &persisted.telegram_topic_binding_states[&route.conversation_key];
        assert_eq!(binding.codex_title, "B");
        assert_eq!(binding.topic_name, "B");
        drop(persisted);
        assert!(
            state
                .telegram_topic_name_update_is_current(&route.conversation_key, telegram_b_token)
                .await
        );
    }

    #[tokio::test]
    async fn a_to_b_to_a_name_race_executes_the_final_a_edit() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let mut config = AppConfig::default();
        config.state_path = temp_dir.path().join("state.json");
        let state = AppState::new(temp_dir.path().join("config.toml"), config, None, None);
        let route = RouteTarget {
            platform: ImPlatformKind::Telegram,
            conversation_key: "telegram:bot:-100|topic=54".to_string(),
            account_id: "bot".to_string(),
            chat_id: "-100|topic=54".to_string(),
            remote_client_key: "im:telegram:owner".to_string(),
        };
        bind_active_telegram_topic(&state, &route, "thread-name", "A").await;

        let calls = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let (b_started_tx, b_started_rx) = tokio::sync::oneshot::channel();
        let (release_b_tx, release_b_rx) = tokio::sync::oneshot::channel();
        let b_token = state
            .begin_telegram_topic_name_update(&route.conversation_key)
            .await;
        let b_task = tokio::spawn({
            let state = state.clone();
            let route = route.clone();
            let calls = calls.clone();
            async move {
                let result = run_telegram_topic_name_edit_while_current(
                    &state,
                    &route,
                    "thread-name",
                    "B",
                    b_token,
                    || async move {
                        calls.lock().await.push("B".to_string());
                        let _ = b_started_tx.send(());
                        let _ = release_b_rx.await;
                        Ok(TelegramForumTopicEditOutcome::Changed)
                    },
                )
                .await;
                let decision = commit_telegram_topic_name_if_current(
                    &state,
                    &route.conversation_key,
                    "thread-name",
                    "B",
                    "B",
                    b_token,
                )
                .await;
                (result, decision)
            }
        });
        b_started_rx.await.expect("B edit started");

        let a_token = state
            .begin_telegram_topic_name_update(&route.conversation_key)
            .await;
        let a_task = tokio::spawn({
            let state = state.clone();
            let route = route.clone();
            let calls = calls.clone();
            async move {
                let result = run_telegram_topic_name_edit_while_current(
                    &state,
                    &route,
                    "thread-name",
                    "A",
                    a_token,
                    || async move {
                        calls.lock().await.push("A".to_string());
                        Ok(TelegramForumTopicEditOutcome::Changed)
                    },
                )
                .await;
                let decision = commit_telegram_topic_name_if_current(
                    &state,
                    &route.conversation_key,
                    "thread-name",
                    "A",
                    "A",
                    a_token,
                )
                .await;
                (result, decision)
            }
        });
        tokio::task::yield_now().await;
        assert!(!a_task.is_finished());
        release_b_tx.send(()).expect("release B edit");

        let (b_result, b_decision) = b_task.await.expect("B task");
        let (a_result, a_decision) = a_task.await.expect("A task");
        assert!(matches!(b_result, Some(Ok(true))));
        assert_eq!(b_decision, TelegramTopicNameDecision::Superseded);
        assert!(matches!(a_result, Some(Ok(true))));
        assert_eq!(a_decision, TelegramTopicNameDecision::Committed);
        assert_eq!(&*calls.lock().await, &["B".to_string(), "A".to_string()]);

        let persisted = state.persisted.lock().await;
        let binding = &persisted.telegram_topic_binding_states[&route.conversation_key];
        assert_eq!(binding.codex_title, "A");
        assert_eq!(binding.topic_name, "A");
        assert_eq!(binding.last_synced_codex_title, "A");
        assert_eq!(binding.last_synced_topic_name, "A");
    }

    #[tokio::test]
    async fn stale_name_failure_cannot_clear_a_newer_echo_marker() {
        let state = test_state();
        let conversation_key = "telegram:bot:-100|topic=52";
        let old_token = telegram_polling::install_telegram_topic_name_marker(
            &state,
            conversation_key,
            "Old title",
        )
        .await;
        let new_token = telegram_polling::install_telegram_topic_name_marker(
            &state,
            conversation_key,
            "New title",
        )
        .await;

        telegram_polling::clear_telegram_topic_name_marker(&state, conversation_key, old_token)
            .await;
        let markers = state.telegram_topic_name_sync_ops.lock().await;
        assert_eq!(markers[conversation_key].len(), 1);
        let marker = &markers[conversation_key][0];
        assert_eq!(marker.name, "New title");
        assert_eq!(marker.token, new_token);
    }

    #[tokio::test]
    async fn binding_change_while_name_update_waits_prevents_wrong_topic_edit() {
        let state = test_state();
        let route = RouteTarget {
            platform: ImPlatformKind::Telegram,
            conversation_key: "telegram:bot:-100|topic=53".to_string(),
            account_id: "bot".to_string(),
            chat_id: "-100|topic=53".to_string(),
            remote_client_key: "im:telegram:owner".to_string(),
        };
        bind_active_telegram_topic(&state, &route, "thread-name", "Before").await;
        let mutation_gate = state.telegram_topic_mutation_gate(&route.account_id).await;
        let guard = mutation_gate.lock().await;
        let operation_token = state
            .begin_telegram_topic_name_update(&route.conversation_key)
            .await;
        let executed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let task = tokio::spawn({
            let state = state.clone();
            let route = route.clone();
            let executed = executed.clone();
            async move {
                telegram_polling::run_telegram_topic_mutation_while(
                    &state,
                    &route.account_id,
                    None,
                    || {
                        telegram_topic_name_edit_is_current(
                            &state,
                            &route,
                            "thread-name",
                            operation_token,
                        )
                    },
                    || async move {
                        executed.store(true, std::sync::atomic::Ordering::SeqCst);
                        Ok(())
                    },
                )
                .await
            }
        });
        tokio::task::yield_now().await;
        state.persisted.lock().await.im_thread_bindings.insert(
            route.conversation_key.clone(),
            "different-thread".to_string(),
        );
        drop(guard);

        let result = tokio::time::timeout(Duration::from_millis(100), task)
            .await
            .expect("stale binding leaves the mutation queue")
            .expect("name update task");
        assert!(result.is_none());
        assert!(!executed.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn lifecycle_notifications_update_telegram_binding_state_in_order() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let mut config = AppConfig::default();
        config.state_path = temp_dir.path().join("state.json");
        let state = AppState::new(temp_dir.path().join("config.toml"), config, None, None);
        let generation = state.runtime.lock().await.start_bridge_generation();
        let route = RouteTarget {
            platform: ImPlatformKind::Telegram,
            conversation_key: "telegram:bot:-100|topic=42".to_string(),
            account_id: "bot".to_string(),
            chat_id: "-100|topic=42".to_string(),
            remote_client_key: "im:telegram:owner".to_string(),
        };
        state
            .runtime
            .lock()
            .await
            .bind_route("thread-lifecycle", route.clone());
        {
            let mut persisted = state.persisted.lock().await;
            persisted.im_thread_bindings.insert(
                route.conversation_key.clone(),
                "thread-lifecycle".to_string(),
            );
            persisted.telegram_topic_binding_states.insert(
                route.conversation_key.clone(),
                crate::store::TelegramTopicBindingState {
                    thread_id: "thread-lifecycle".to_string(),
                    ..Default::default()
                },
            );
        }
        let (outbound_tx, _outbound_rx) = outbound_channel();

        for (method, expected_state) in [
            ("thread/archived", "archived"),
            ("thread/unarchived", "active"),
            ("thread/deleted", "deleted"),
        ] {
            let notification = crate::codex::CodexNotification {
                method: method.to_string(),
                params: Some(json!({"threadId": "thread-lifecycle"})),
                request_id: None,
                remote_client_key: Some("default:codex_app".to_string()),
                remote_connection_epoch: None,
            };
            handle_codex_notification_for_generation(
                state.clone(),
                ImApiRegistry::default(),
                outbound_tx.clone(),
                &notification,
                generation,
            )
            .await;
            assert_eq!(
                state.persisted.lock().await.telegram_topic_binding_states[&route.conversation_key]
                    .codex_state,
                expected_state
            );
        }
    }

    #[tokio::test]
    async fn missing_telegram_api_does_not_claim_an_unchanged_name_was_synced() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let mut config = AppConfig::default();
        config.state_path = temp_dir.path().join("state.json");
        let state = AppState::new(temp_dir.path().join("config.toml"), config, None, None);
        let generation = state.runtime.lock().await.start_bridge_generation();
        let route = RouteTarget {
            platform: ImPlatformKind::Telegram,
            conversation_key: "telegram:bot:-100|topic=43".to_string(),
            account_id: "bot".to_string(),
            chat_id: "-100|topic=43".to_string(),
            remote_client_key: "im:telegram:owner".to_string(),
        };
        state
            .runtime
            .lock()
            .await
            .bind_route("thread-name-lifecycle", route.clone());
        {
            let mut persisted = state.persisted.lock().await;
            persisted.im_thread_bindings.insert(
                route.conversation_key.clone(),
                "thread-name-lifecycle".to_string(),
            );
            persisted.telegram_topic_binding_states.insert(
                route.conversation_key.clone(),
                crate::store::TelegramTopicBindingState {
                    thread_id: "thread-name-lifecycle".to_string(),
                    topic_name: "Same title".to_string(),
                    codex_state: "archived".to_string(),
                    archived_at_ms: Some(1),
                    lifecycle_generation: generation,
                    lifecycle_revision: 1,
                    ..Default::default()
                },
            );
        }

        telegram_polling::unarchive_telegram_topic_for_codex_thread(
            &state,
            "thread-name-lifecycle",
            generation,
        )
        .await;
        sync_codex_thread_name_to_telegram(
            &state,
            &ImApiRegistry::default(),
            &json!({
                "threadId": "thread-name-lifecycle",
                "threadName": "Same title"
            }),
        )
        .await;

        let persisted = state.persisted.lock().await;
        let binding = &persisted.telegram_topic_binding_states[&route.conversation_key];
        assert_eq!(binding.codex_state, "active");
        assert_eq!(binding.codex_title, "Same title");
        assert_eq!(binding.topic_name, "Same title");
        assert!(binding.last_synced_codex_title.is_empty());
        assert!(binding.last_synced_topic_name.is_empty());
    }

    #[test]
    fn telegram_command_progress_only_retries_generic_errors_for_final_edits() {
        let error = anyhow::anyhow!("temporary network failure");

        assert_eq!(
            telegram_command_progress_retry_delay_ms(&error, true, true),
            Some(500)
        );
        assert_eq!(
            telegram_command_progress_retry_delay_ms(&error, false, true),
            None
        );
        assert_eq!(
            telegram_command_progress_retry_delay_ms(&error, true, false),
            None
        );
    }

    #[tokio::test]
    async fn telegram_final_reply_is_queued_after_its_images() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let image_path = temp_dir.path().join("result.png");
        std::fs::write(&image_path, b"image").expect("test image");
        let state = test_state();
        let route = test_telegram_route();
        let api_registry = ImApiRegistry::default();
        let (outbound_tx, mut outbound_rx) = outbound_channel();
        let reply = format!("result\n\n![preview]({})", image_path.display());

        send_turn_reply(
            &state,
            &api_registry,
            Some(&outbound_tx),
            "thread",
            Some("turn"),
            &route,
            None,
            &reply,
            true,
        )
        .await;

        let image = try_recv_for_test(&mut outbound_rx).expect("queued image");
        assert_eq!(image.kind, ImOutboundKind::ImageItem);
        assert_eq!(image.item_type.as_deref(), Some("agentMessageImage"));

        let reply = try_recv_for_test(&mut outbound_rx).expect("queued final reply");
        assert_eq!(reply.kind, ImOutboundKind::TurnReply);
        assert_eq!(reply.item_type.as_deref(), Some("agentMessage"));
        match reply.payload {
            ImOutboundPayload::Text(text) => {
                assert!(text.starts_with("result"));
                assert!(!text.contains("🤖 Codex"));
            }
            ImOutboundPayload::TelegramCommentary { .. }
            | ImOutboundPayload::Approval(_)
            | ImOutboundPayload::Image { .. }
            | ImOutboundPayload::ImageGroup { .. } => {
                panic!("Telegram final reply must be queued as text")
            }
        }
        assert!(try_recv_for_test(&mut outbound_rx).is_none());
    }

    #[test]
    fn telegram_agent_images_are_queued_as_one_album_payload() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let first = temp_dir.path().join("first.png");
        let second = temp_dir.path().join("second.jpg");
        let third = temp_dir.path().join("third.webp");
        for path in [&first, &second, &third] {
            std::fs::write(path, b"image").expect("write test image");
        }
        let text = format!(
            "![first]({})\n![second]({})\n![third]({})",
            first.display(),
            second.display(),
            third.display()
        );
        let route = test_telegram_route();
        let (outbound_tx, mut outbound_rx) = outbound_channel();

        let count = queue_agent_message_images(&outbound_tx, "thread", &route, None, &text)
            .expect("queue image album");

        assert_eq!(count, 3);
        let message = try_recv_for_test(&mut outbound_rx).expect("queued image album");
        assert_eq!(message.kind, ImOutboundKind::ImageItem);
        assert_eq!(
            message.item_id.as_deref(),
            Some("agent-message-image-group")
        );
        match message.payload {
            ImOutboundPayload::ImageGroup { images } => {
                assert_eq!(images.len(), 3);
                assert_eq!(images[0].path, first);
                assert_eq!(images[0].caption.as_deref(), Some("first"));
                assert_eq!(images[1].path, second);
                assert_eq!(images[1].caption.as_deref(), Some("second"));
                assert_eq!(images[2].path, third);
                assert_eq!(images[2].caption.as_deref(), Some("third"));
            }
            ImOutboundPayload::Text(_)
            | ImOutboundPayload::TelegramCommentary { .. }
            | ImOutboundPayload::Approval(_)
            | ImOutboundPayload::Image { .. } => {
                panic!("multiple Telegram images should use one album payload")
            }
        }
        assert!(try_recv_for_test(&mut outbound_rx).is_none());
    }

    #[test]
    fn telegram_single_image_keeps_the_single_image_payload() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let path = temp_dir.path().join("single.png");
        std::fs::write(&path, b"image").expect("write test image");
        let text = format!("![single]({})", path.display());
        let route = test_telegram_route();
        let (outbound_tx, mut outbound_rx) = outbound_channel();

        assert_eq!(
            queue_agent_message_images(&outbound_tx, "thread", &route, None, &text)
                .expect("queue single image"),
            1
        );
        let message = try_recv_for_test(&mut outbound_rx).expect("queued single image");
        match message.payload {
            ImOutboundPayload::Image {
                path: queued_path,
                caption,
                ..
            } => {
                assert_eq!(queued_path, path);
                assert_eq!(caption.as_deref(), Some("single"));
            }
            ImOutboundPayload::Text(_)
            | ImOutboundPayload::TelegramCommentary { .. }
            | ImOutboundPayload::Approval(_)
            | ImOutboundPayload::ImageGroup { .. } => {
                panic!("a single Telegram image should use the single-image payload")
            }
        }
        assert!(try_recv_for_test(&mut outbound_rx).is_none());
    }

    #[test]
    fn non_telegram_images_remain_individual_payloads() {
        let route = test_route();
        let (outbound_tx, mut outbound_rx) = outbound_channel();
        let images = (0..3)
            .map(|index| ImOutboundImage {
                path: PathBuf::from(format!("image-{index}.png")),
                caption: Some(format!("image {index}")),
                fallback_text: None,
            })
            .collect();

        assert_eq!(
            enqueue_image_payloads(
                &outbound_tx,
                "thread",
                &route,
                Some("item"),
                "mcpToolCall",
                "mcp-tool-image",
                images,
            )
            .expect("queue non-Telegram images"),
            3
        );
        for index in 0..3 {
            let message = try_recv_for_test(&mut outbound_rx).expect("queued image");
            match message.payload {
                ImOutboundPayload::Image { path, caption, .. } => {
                    assert_eq!(path, PathBuf::from(format!("image-{index}.png")));
                    assert_eq!(caption.as_deref(), Some(format!("image {index}").as_str()));
                }
                ImOutboundPayload::Text(_)
                | ImOutboundPayload::TelegramCommentary { .. }
                | ImOutboundPayload::Approval(_)
                | ImOutboundPayload::ImageGroup { .. } => {
                    panic!("non-Telegram images should remain individual payloads")
                }
            }
        }
        assert!(try_recv_for_test(&mut outbound_rx).is_none());
    }

    #[tokio::test]
    async fn telegram_approval_captures_current_turn_when_turn_is_omitted() {
        let state = test_state();
        let route = test_telegram_route();
        let (outbound_tx, mut outbound_rx) = outbound_channel();
        state
            .runtime
            .lock()
            .await
            .mark_turn_started("thread", "turn");
        let approval = PendingApproval {
            request_id: json!("request-1"),
            request_kind: "commandExecution".to_string(),
            method: "item/commandExecution/requestApproval".to_string(),
            params: json!({"turn": {"threadId": "thread"}}),
            summary: "Run cargo test".to_string(),
            decisions: Vec::new(),
            message_id: None,
            remote_client_key: None,
        };

        send_approval(&state, &outbound_tx, &route, &approval)
            .await
            .expect("queue approval");

        let message = try_recv_for_test(&mut outbound_rx).expect("queued approval");
        assert_eq!(message.kind, ImOutboundKind::Approval);
        assert_eq!(message.thread_id, "thread");
        assert_eq!(message.turn_id.as_deref(), Some("turn"));
    }

    #[tokio::test]
    async fn telegram_web_search_is_folded_into_task_progress() {
        let state = test_state();
        let route = test_telegram_route();
        let api_registry = ImApiRegistry::default();
        let (outbound_tx, mut outbound_rx) = outbound_channel();
        let item = json!({
            "id": "search-item",
            "type": "webSearch",
            "query": "Telegram typing indicator",
            "action": {"type": "openPage", "url": "https://core.telegram.org/bots/api"},
            "results": [{
                "title": "Telegram Bot API",
                "domain": "core.telegram.org",
                "snippet": "Total lines: 6874",
                "url": "https://core.telegram.org/bots/api"
            }]
        });
        state
            .runtime
            .lock()
            .await
            .bind_route("thread", route.clone());
        state
            .runtime
            .lock()
            .await
            .mark_turn_started("thread", "turn");

        let notification = crate::codex::CodexNotification {
            method: "item/completed".to_string(),
            params: Some(json!({
                "threadId": "thread",
                "turnId": "turn",
                "item": item,
            })),
            request_id: None,
            remote_client_key: None,
            remote_connection_epoch: None,
        };
        handle_codex_notification(state.clone(), api_registry, outbound_tx, &notification).await;

        assert!(try_recv_for_test(&mut outbound_rx).is_none());
        let snapshot = state
            .runtime
            .lock()
            .await
            .claim_telegram_command_progress_delivery("thread", "turn")
            .expect("search should update task progress");
        assert_eq!(snapshot.web_searches.len(), 1);
        let rendered =
            telegram_progress::render_task_progress(&snapshot, im_text_for_state(&state));
        let blocks = serde_json::Value::Array(rendered.blocks);
        assert!(blocks.as_array().unwrap().iter().any(|block| {
            block["summary"] == "搜索 · Telegram typing indicator · 1 条结果"
        }));
        assert!(rendered.fallback_markdown.contains("Telegram Bot API"));
        assert!(!rendered.fallback_markdown.contains("\"openPage\""));
    }

    #[tokio::test]
    async fn telegram_final_reply_dedupe_is_scoped_to_turn() {
        let state = test_state();
        let route = test_telegram_route();
        let api_registry = ImApiRegistry::default();
        let (outbound_tx, mut outbound_rx) = outbound_channel();

        for turn_id in ["turn-1", "turn-1", "turn-2"] {
            send_turn_reply(
                &state,
                &api_registry,
                Some(&outbound_tx),
                "thread",
                Some(turn_id),
                &route,
                None,
                "same final answer",
                true,
            )
            .await;
        }

        let first = try_recv_for_test(&mut outbound_rx).expect("first turn reply");
        assert_eq!(first.kind, ImOutboundKind::TurnReply);
        assert_eq!(first.item_type.as_deref(), Some("agentMessage"));

        let second = try_recv_for_test(&mut outbound_rx).expect("second turn reply");
        assert_eq!(second.kind, ImOutboundKind::TurnReply);
        assert_eq!(second.item_type.as_deref(), Some("agentMessage"));
        assert!(try_recv_for_test(&mut outbound_rx).is_none());
    }

    #[test]
    fn agent_message_phase_distinguishes_commentary_from_final_answers() {
        assert!(!agent_message_is_final_answer(&json!({
            "type": "agentMessage",
            "phase": "commentary"
        })));
        assert!(agent_message_is_final_answer(&json!({
            "type": "agentMessage",
            "phase": "final_answer"
        })));
        assert!(agent_message_is_final_answer(&json!({
            "type": "agentMessage"
        })));
    }

    #[tokio::test]
    async fn telegram_commentary_does_not_hide_an_identical_final_answer() {
        let state = test_state();
        let route = test_telegram_route();
        let (outbound_tx, mut outbound_rx) = outbound_channel();
        {
            let mut runtime = state.runtime.lock().await;
            runtime.bind_route("thread", route);
            runtime.mark_turn_started("thread", "turn");
        }

        for (item_id, phase) in [
            ("commentary-item", "commentary"),
            ("final-item", "final_answer"),
        ] {
            let notification = crate::codex::CodexNotification {
                method: "item/completed".to_string(),
                params: Some(json!({
                    "threadId": "thread",
                    "turnId": "turn",
                    "item": {
                        "id": item_id,
                        "type": "agentMessage",
                        "phase": phase,
                        "text": "same text"
                    }
                })),
                request_id: None,
                remote_client_key: None,
                remote_connection_epoch: None,
            };
            handle_codex_notification(
                state.clone(),
                ImApiRegistry::default(),
                outbound_tx.clone(),
                &notification,
            )
            .await;
        }

        let completed = crate::codex::CodexNotification {
            method: "turn/completed".to_string(),
            params: Some(json!({
                "threadId": "thread",
                "turnId": "turn",
                "turn": {
                    "id": "turn",
                    "status": "completed",
                    "items": [
                        {
                            "id": "final-item",
                            "type": "agentMessage",
                            "phase": "final_answer",
                            "text": "same text"
                        },
                        {
                            "id": "commentary-item",
                            "type": "agentMessage",
                            "phase": "commentary",
                            "text": "same text"
                        }
                    ]
                }
            })),
            request_id: None,
            remote_client_key: None,
            remote_connection_epoch: None,
        };
        handle_codex_notification(
            state.clone(),
            ImApiRegistry::default(),
            outbound_tx.clone(),
            &completed,
        )
        .await;

        let commentary = try_recv_for_test(&mut outbound_rx).expect("queued commentary");
        assert_eq!(commentary.kind, ImOutboundKind::Item);
        assert_eq!(commentary.item_type.as_deref(), Some("agentMessage"));
        assert_eq!(commentary.turn_id.as_deref(), Some("turn"));
        match &commentary.payload {
            ImOutboundPayload::TelegramCommentary {
                rich_markdown,
                fallback_text,
                ..
            } => {
                assert_eq!(rich_markdown, "same text");
                assert_eq!(fallback_text, "same text");
            }
            _ => panic!("commentary should use the aggregated Telegram payload"),
        }

        let final_answer = try_recv_for_test(&mut outbound_rx).expect("queued final answer");
        assert_eq!(final_answer.kind, ImOutboundKind::TurnReply);
        assert_eq!(final_answer.item_type.as_deref(), Some("agentMessage"));
        assert_eq!(final_answer.turn_id.as_deref(), Some("turn"));
        assert!(matches!(
            &final_answer.payload,
            ImOutboundPayload::Text(text) if text == "same text"
        ));
        assert!(try_recv_for_test(&mut outbound_rx).is_none());
    }

    #[tokio::test]
    async fn telegram_commentary_folds_eight_updates_and_deduplicates_item_replays() {
        let state = test_state();
        let route = test_telegram_route();
        let (outbound_tx, mut outbound_rx) = outbound_channel();
        {
            let mut runtime = state.runtime.lock().await;
            runtime.bind_route("thread", route);
            runtime.mark_turn_started("thread", "turn");
        }

        for index in 1..=8 {
            let notification = crate::codex::CodexNotification {
                method: "item/completed".to_string(),
                params: Some(json!({
                    "threadId": "thread",
                    "turnId": "turn",
                    "item": {
                        "id": format!("commentary-{index}"),
                        "type": "agentMessage",
                        "phase": "commentary",
                        "text": format!("update {index}")
                    }
                })),
                request_id: None,
                remote_client_key: None,
                remote_connection_epoch: None,
            };
            handle_codex_notification(
                state.clone(),
                ImApiRegistry::default(),
                outbound_tx.clone(),
                &notification,
            )
            .await;
            if index == 3 {
                handle_codex_notification(
                    state.clone(),
                    ImApiRegistry::default(),
                    outbound_tx.clone(),
                    &notification,
                )
                .await;
            }
        }

        send_turn_reply(
            &state,
            &ImApiRegistry::default(),
            Some(&outbound_tx),
            "thread",
            Some("turn"),
            &test_telegram_route(),
            Some("final-answer"),
            "final answer",
            true,
        )
        .await;

        let mut queued = 0;
        let mut last_rich = String::new();
        let mut last_fallback = String::new();
        for _ in 0..8 {
            let message = try_recv_for_test(&mut outbound_rx).expect("commentary snapshot");
            queued += 1;
            assert_eq!(message.kind, ImOutboundKind::Item);
            match message.payload {
                ImOutboundPayload::TelegramCommentary {
                    rich_markdown,
                    fallback_text,
                    ..
                } => {
                    last_rich = rich_markdown;
                    last_fallback = fallback_text;
                }
                _ => panic!("commentary should use the aggregated Telegram payload"),
            }
        }

        assert_eq!(
            queued, 8,
            "the replayed item must not enqueue a ninth update"
        );
        assert!(last_rich.contains("<summary>较早进展 · 6 条</summary>"));
        assert!(last_rich.contains("update 1"));
        assert!(last_rich.contains("update 6"));
        assert!(last_rich.contains("update 1\n\n---\n\nupdate 2"));
        assert!(last_rich.ends_with("update 7\n\n---\n\nupdate 8"));
        assert_eq!(last_fallback, "较早进展 · 6 条\n\nupdate 7\n\nupdate 8");

        let final_answer = try_recv_for_test(&mut outbound_rx).expect("final answer");
        assert_eq!(final_answer.kind, ImOutboundKind::TurnReply);
        assert!(matches!(
            final_answer.payload,
            ImOutboundPayload::Text(text) if text == "final answer"
        ));
        assert!(try_recv_for_test(&mut outbound_rx).is_none());
    }

    #[tokio::test]
    async fn telegram_commentary_keeps_equal_text_from_different_items() {
        let state = test_state();
        let route = test_telegram_route();
        let api_registry = ImApiRegistry::default();
        let (outbound_tx, mut outbound_rx) = outbound_channel();
        state
            .runtime
            .lock()
            .await
            .mark_turn_started("thread", "turn");

        for item_id in ["commentary-1", "commentary-2"] {
            send_turn_reply(
                &state,
                &api_registry,
                Some(&outbound_tx),
                "thread",
                Some("turn"),
                &route,
                Some(item_id),
                "same update",
                false,
            )
            .await;
        }

        let _first = try_recv_for_test(&mut outbound_rx).expect("first commentary snapshot");
        let second = try_recv_for_test(&mut outbound_rx).expect("second commentary snapshot");
        match second.payload {
            ImOutboundPayload::TelegramCommentary {
                rich_markdown,
                fallback_text,
                ..
            } => {
                assert_eq!(rich_markdown, "same update\n\n---\n\nsame update");
                assert_eq!(fallback_text, "same update\n\nsame update");
            }
            _ => panic!("commentary should use the aggregated Telegram payload"),
        }
        assert!(try_recv_for_test(&mut outbound_rx).is_none());
    }

    #[tokio::test]
    async fn telegram_context_compaction_splits_following_commentary_into_a_new_card() {
        let state = test_state();
        let route = test_telegram_route();
        let api_registry = ImApiRegistry::default();
        let (outbound_tx, mut outbound_rx) = outbound_channel();
        {
            let mut runtime = state.runtime.lock().await;
            runtime.bind_route("thread", route.clone());
            runtime.mark_turn_started("thread", "turn");
        }

        send_turn_reply(
            &state,
            &api_registry,
            Some(&outbound_tx),
            "thread",
            Some("turn"),
            &route,
            Some("before"),
            "before compaction",
            false,
        )
        .await;
        let before = try_recv_for_test(&mut outbound_rx).expect("commentary before compaction");
        let before_segment = match before.payload {
            ImOutboundPayload::TelegramCommentary { segment, .. } => segment,
            _ => panic!("commentary should use an aggregate payload"),
        };

        send_text_im_codex_item(
            &state,
            &outbound_tx,
            "thread",
            Some("turn"),
            &route,
            "compaction",
            &json!({"id": "compaction", "type": "contextCompaction"}),
        )
        .await
        .expect("queue context compaction");
        let compaction = try_recv_for_test(&mut outbound_rx).expect("context compaction");
        assert_eq!(compaction.item_type.as_deref(), Some("contextCompaction"));

        send_turn_reply(
            &state,
            &api_registry,
            Some(&outbound_tx),
            "thread",
            Some("turn"),
            &route,
            Some("after"),
            "after compaction",
            false,
        )
        .await;
        let after = try_recv_for_test(&mut outbound_rx).expect("commentary after compaction");
        match after.payload {
            ImOutboundPayload::TelegramCommentary {
                segment,
                rich_markdown,
                fallback_text,
            } => {
                assert_eq!(segment, before_segment + 1);
                assert_eq!(rich_markdown, "after compaction");
                assert_eq!(fallback_text, "after compaction");
            }
            _ => panic!("commentary should use an aggregate payload"),
        }
        assert!(try_recv_for_test(&mut outbound_rx).is_none());
    }

    #[tokio::test]
    async fn telegram_context_compaction_captures_current_turn_when_turn_is_omitted() {
        let state = test_state();
        let route = test_telegram_route();
        let (outbound_tx, mut outbound_rx) = outbound_channel();
        {
            let mut runtime = state.runtime.lock().await;
            runtime.bind_route("thread", route.clone());
            runtime.mark_turn_started("thread", "turn");
        }

        send_text_im_codex_item(
            &state,
            &outbound_tx,
            "thread",
            None,
            &route,
            "compaction",
            &json!({"id": "compaction", "type": "contextCompaction"}),
        )
        .await
        .expect("queue context compaction");

        let compaction = try_recv_for_test(&mut outbound_rx).expect("context compaction");
        assert_eq!(compaction.turn_id.as_deref(), Some("turn"));
    }

    #[tokio::test]
    async fn telegram_context_compaction_does_not_split_when_enqueue_fails() {
        let state = test_state();
        let route = test_telegram_route();
        let (outbound_tx, outbound_rx) = outbound_channel();
        {
            let mut runtime = state.runtime.lock().await;
            runtime.bind_route("thread", route.clone());
            runtime.mark_turn_started("thread", "turn");
        }
        let before = state
            .runtime
            .lock()
            .await
            .append_telegram_commentary("thread", "turn", "before", "before".to_string())
            .expect("commentary before compaction");
        drop(outbound_rx);

        assert!(
            send_text_im_codex_item(
                &state,
                &outbound_tx,
                "thread",
                Some("turn"),
                &route,
                "compaction",
                &json!({"id": "compaction", "type": "contextCompaction"}),
            )
            .await
            .is_err()
        );

        let after = state
            .runtime
            .lock()
            .await
            .append_telegram_commentary("thread", "turn", "after", "after".to_string())
            .expect("commentary should stay in the current segment");
        assert_eq!(after.segment, before.segment);
        assert_eq!(after.entries.len(), 2);
    }

    #[tokio::test]
    async fn telegram_only_forwards_desktop_user_messages_without_an_emoji_header() {
        let route = test_telegram_route();
        let notification = crate::codex::CodexNotification {
            method: "item/completed".to_string(),
            params: Some(json!({
                "threadId": "thread",
                "turnId": "turn",
                "item": {
                    "id": "user-item",
                    "type": "userMessage",
                    "content": [{"type": "text", "text": "从电脑端继续"}]
                }
            })),
            request_id: None,
            remote_client_key: None,
            remote_connection_epoch: None,
        };

        let desktop_state = test_state();
        let (desktop_tx, mut desktop_rx) = outbound_channel();
        {
            let mut runtime = desktop_state.runtime.lock().await;
            runtime.bind_route("thread", route.clone());
            runtime.mark_turn_started("thread", "turn");
        }
        handle_codex_notification(
            desktop_state,
            ImApiRegistry::default(),
            desktop_tx,
            &notification,
        )
        .await;

        let forwarded = try_recv_for_test(&mut desktop_rx).expect("desktop user message");
        assert_eq!(forwarded.item_type.as_deref(), Some("userMessage"));
        assert!(matches!(
            forwarded.payload,
            ImOutboundPayload::Text(text) if text == "从电脑端继续"
        ));
        assert!(try_recv_for_test(&mut desktop_rx).is_none());

        let telegram_state = test_state();
        let (telegram_tx, mut telegram_rx) = outbound_channel();
        {
            let mut runtime = telegram_state.runtime.lock().await;
            runtime.bind_route("thread", route);
            runtime.mark_turn_started("thread", "turn");
            runtime.remember_turn_origin("turn", TurnOrigin::Telegram);
        }
        handle_codex_notification(
            telegram_state,
            ImApiRegistry::default(),
            telegram_tx,
            &notification,
        )
        .await;

        assert!(try_recv_for_test(&mut telegram_rx).is_none());
    }

    #[tokio::test]
    async fn telegram_mcp_images_are_queued_once_without_an_item_text_message() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let state = AppState::new(
            temp_dir.path().join("state.toml"),
            AppConfig::default(),
            None,
            None,
        );
        let route = test_telegram_route();
        let (outbound_tx, mut outbound_rx) = outbound_channel();
        let item = json!({
            "type": "mcpToolCall",
            "result": {
                "content": [{
                    "type": "image",
                    "mimeType": "image/png",
                    "data": "iVBORw0KGgo="
                }]
            }
        });

        queue_telegram_mcp_tool_images(&state, &outbound_tx, "thread", &route, "mcp-item", &item)
            .await
            .expect("first image queue");
        queue_telegram_mcp_tool_images(&state, &outbound_tx, "thread", &route, "mcp-item", &item)
            .await
            .expect("duplicate image queue");

        let message = try_recv_for_test(&mut outbound_rx).expect("queued image");
        assert_eq!(message.kind, ImOutboundKind::ImageItem);
        assert_eq!(message.item_type.as_deref(), Some("mcpToolCall"));
        match message.payload {
            ImOutboundPayload::Image { path, .. } => assert!(path.is_file()),
            ImOutboundPayload::ImageGroup { images } => {
                assert_eq!(images.len(), 1);
                assert!(images[0].path.is_file());
            }
            ImOutboundPayload::Text(_)
            | ImOutboundPayload::TelegramCommentary { .. }
            | ImOutboundPayload::Approval(_) => {
                panic!("MCP aggregation must not queue an item text message")
            }
        }
        assert!(try_recv_for_test(&mut outbound_rx).is_none());
    }

    #[tokio::test]
    async fn telegram_success_terminal_notice_is_suppressed_but_late_failure_is_delivered() {
        let state = test_state();
        let route = test_telegram_route();
        let api_registry = ImApiRegistry::default();
        let (outbound_tx, mut outbound_rx) = outbound_channel();

        send_turn_terminal_mark_once(
            &state,
            &api_registry,
            &outbound_tx,
            "thread",
            &route,
            Some("turn"),
            false,
            None,
        )
        .await;
        assert!(try_recv_for_test(&mut outbound_rx).is_none());

        send_turn_terminal_mark_once(
            &state,
            &api_registry,
            &outbound_tx,
            "thread",
            &route,
            Some("turn"),
            true,
            Some("503 Service Unavailable"),
        )
        .await;

        let message = try_recv_for_test(&mut outbound_rx).expect("late failure notice");
        assert_eq!(message.kind, ImOutboundKind::TurnReply);
        assert_eq!(message.item_type.as_deref(), Some("turnFailed"));
        match message.payload {
            ImOutboundPayload::Text(text) => {
                assert!(text.contains("任务失败"));
                assert!(text.contains("503 Service Unavailable"));
            }
            ImOutboundPayload::TelegramCommentary { .. }
            | ImOutboundPayload::Approval(_)
            | ImOutboundPayload::Image { .. }
            | ImOutboundPayload::ImageGroup { .. } => {
                panic!("terminal failure must be queued as text")
            }
        }
        assert!(try_recv_for_test(&mut outbound_rx).is_none());
    }

    #[tokio::test]
    async fn non_telegram_success_terminal_notice_is_still_queued() {
        let state = test_state();
        let route = test_route();
        let api_registry = ImApiRegistry::default();
        let (outbound_tx, mut outbound_rx) = outbound_channel();

        send_turn_terminal_mark_once(
            &state,
            &api_registry,
            &outbound_tx,
            "thread",
            &route,
            Some("turn"),
            false,
            None,
        )
        .await;

        let message = try_recv_for_test(&mut outbound_rx).expect("success notice");
        assert_eq!(message.kind, ImOutboundKind::TurnReply);
        assert_eq!(message.item_type.as_deref(), Some("turnCompleted"));
        match message.payload {
            ImOutboundPayload::Text(text) => assert!(text.contains("已完成")),
            ImOutboundPayload::TelegramCommentary { .. }
            | ImOutboundPayload::Approval(_)
            | ImOutboundPayload::Image { .. }
            | ImOutboundPayload::ImageGroup { .. } => {
                panic!("terminal success must be queued as text")
            }
        }
        assert!(try_recv_for_test(&mut outbound_rx).is_none());
    }

    #[test]
    fn turn_failure_summary_keeps_503_and_removes_diagnostics() {
        let params = json!({
            "threadId": "thread",
            "turnId": "turn",
            "willRetry": false,
            "error": {
                "message": "unexpected status 503 Service Unavailable: Service temporarily unavailable, url: http://127.0.0.1:8090/responses, request id: secret-request-id",
                "codexErrorInfo": "other"
            }
        });

        assert!(turn_is_failed(&params));
        let summary = turn_failure_summary(&params).expect("error summary");
        assert!(summary.contains("503 Service Unavailable"));
        assert!(!summary.contains("127.0.0.1"));
        assert!(!summary.contains("secret-request-id"));
    }

    #[test]
    fn turn_failure_summary_reads_nested_error_and_http_status_code() {
        let params = json!({
            "threadId": "thread",
            "turn": {
                "id": "turn",
                "status": "failed",
                "error": {
                    "message": "provider unavailable",
                    "codexErrorInfo": {
                        "responseStreamConnectionFailed": {"httpStatusCode": 503}
                    }
                }
            }
        });

        assert!(turn_is_failed(&params));
        assert_eq!(
            turn_failure_summary(&params).as_deref(),
            Some("503 provider unavailable")
        );
    }

    #[test]
    fn failed_turn_without_error_body_still_has_a_failure_state() {
        let params = json!({
            "threadId": "thread",
            "turn": {"id": "turn", "status": "failed", "error": null}
        });

        assert!(turn_is_failed(&params));
        assert!(turn_failure_summary(&params).is_none());
    }

    #[test]
    fn turn_failure_summary_accepts_string_and_additional_details_errors() {
        let string_error = json!({
            "threadId": "thread",
            "error": "503 Service Unavailable"
        });
        assert_eq!(
            turn_failure_summary(&string_error).as_deref(),
            Some("503 Service Unavailable")
        );

        let details_error = json!({
            "threadId": "thread",
            "error": {"additionalDetails": "gateway unavailable"}
        });
        assert_eq!(
            turn_failure_summary(&details_error).as_deref(),
            Some("gateway unavailable")
        );
    }

    #[tokio::test]
    async fn stale_terminal_event_does_not_match_a_new_turn() {
        let state = test_state();
        state
            .runtime
            .lock()
            .await
            .mark_turn_started("thread", "new-turn");

        assert!(terminal_turn_is_stale(&state, "thread", Some("old-turn")).await);
        assert!(!terminal_turn_is_stale(&state, "thread", Some("new-turn")).await);
    }

    #[tokio::test]
    async fn wecom_stream_coalesces_deltas_and_does_not_duplicate_final() {
        let state = test_state();
        let api = test_api();
        let route = test_route();
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        api.install_sender(Some(sender)).await;
        state.runtime.lock().await.wecom_streams_by_thread.insert(
            route.conversation_key.clone(),
            WecomStreamState {
                req_id: "callback".to_string(),
                stream_id: "stream-1".to_string(),
                content: String::new(),
                sent_content: String::new(),
                finished: false,
                sending: false,
                dirty: false,
                delivered: false,
                cleanup_after_delivery: false,
                revision: 0,
            },
        );

        send_wecom_stream_delta(&state, &api, "thread-1", &route, "hello", false).await;
        send_wecom_stream_delta(&state, &api, "thread-1", &route, " world", false).await;

        let command = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
            .await
            .expect("stream driver should send an update")
            .expect("stream command");
        assert_eq!(
            command.body.pointer("/stream/content"),
            Some(&json!("hello world"))
        );
        assert_eq!(command.body.pointer("/stream/finish"), Some(&json!(false)));
        command
            .result
            .send(Ok(json!({ "headers": { "req_id": "ack" } })))
            .expect("stream acknowledgement");

        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(80), receiver.recv())
                .await
                .is_err()
        );

        assert!(
            send_wecom_stream_final(&state, &api, "thread-1", &route, Some("hello world"), true,)
                .await
        );
        let final_command = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
            .await
            .expect("stream driver should send the final update")
            .expect("final stream command");
        assert_eq!(
            final_command.body.pointer("/stream/content"),
            Some(&json!("hello world"))
        );
        assert_eq!(
            final_command.body.pointer("/stream/finish"),
            Some(&json!(true))
        );
        final_command
            .result
            .send(Ok(json!({ "headers": { "req_id": "final-ack" } })))
            .expect("final stream acknowledgement");
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(80), receiver.recv())
                .await
                .is_err()
        );
        assert!(
            !state
                .runtime
                .lock()
                .await
                .wecom_streams_by_thread
                .contains_key(&route.conversation_key)
        );
    }
}
