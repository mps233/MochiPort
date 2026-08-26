use anyhow::Result;
use sha2::Digest;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde_json::Value;
use tokio::sync::mpsc;
use tokio::time::{Duration, sleep};

use crate::{
    app_state::{ImAccountRuntimeState, SharedState, im_account_key},
    chain_log,
    im::core::{
        accounts::ImApiRegistry, i18n::im_text_for_state, session::bind_thread_to_route,
        thread::summarize_thread_title,
    },
    im_runtime::{RouteTarget, route_from_conversation_key},
    remote_control_backend,
    types::{
        ChatType, ImPlatformKind, InboundAction, InboundMessage, ThreadRouteDirection, now_ms,
        telegram_message_target,
    },
};

use super::{
    api::{
        TelegramApi, TelegramApiError, TelegramBotCommand, TelegramCallbackQuery, TelegramMessage,
    },
    types::TelegramSettings,
};

const TELEGRAM_LONG_POLL_TIMEOUT_SECONDS: u32 = 25;
const TELEGRAM_STARTUP_PROBE_RETRY_SECONDS: u64 = 5;
const TELEGRAM_CONFLICT_BACKOFF_SECONDS: u64 = 35;
const TELEGRAM_GENERIC_RETRY_SECONDS: u64 = 5;
const TELEGRAM_TOPIC_RECONCILIATION_INTERVAL: Duration = Duration::from_secs(300);
const TELEGRAM_TOPIC_STATE_GRACE: Duration = Duration::from_secs(300);
const AUTO_TOPIC_SESSION_PAGE_LIMIT: u32 = 100;
const AUTO_TOPIC_SESSION_MAX_PAGES: usize = 20;

/// Handle a `thread/started` notification from the official Codex client.
///
/// The notification is intentionally handled in a detached task: Telegram
/// network calls and the optional session-history lookup must not stall the
/// shared Codex notification router. `telegram_topic_sync_ops` serializes this
/// path with the manual import endpoint, making duplicate notifications
/// idempotent.
pub(crate) async fn auto_create_topic_for_codex_thread(
    state: &SharedState,
    api_registry: &ImApiRegistry,
    remote_client_key: &str,
    params: Value,
) {
    let generation = state.runtime.lock().await.bridge_generation;
    auto_create_topic_for_codex_thread_for_generation(
        state,
        api_registry,
        remote_client_key,
        params,
        generation,
        None,
    )
    .await;
}

/// Create and bind a Topic only while the bridge generation that received the
/// notification is still active. The event router intentionally lets this
/// workflow run outside its notification loop, so every await that can cross a
/// bridge restart needs a generation check before applying side effects.
pub(crate) async fn auto_create_topic_for_codex_thread_for_generation(
    state: &SharedState,
    api_registry: &ImApiRegistry,
    remote_client_key: &str,
    params: Value,
    generation: u64,
    connection_epoch: Option<u64>,
) {
    if !is_current_bridge_generation(state, generation).await {
        return;
    }
    let Some(thread_id) = started_thread_id(&params) else {
        state
            .push_event(
                "warn",
                "telegram_auto_topic_skipped",
                "thread/started notification did not include a thread id",
            )
            .await;
        return;
    };
    let remote_client_key = if remote_client_key.trim().is_empty() {
        remote_control_backend::default_remote_client_key().to_string()
    } else {
        remote_client_key.trim().to_string()
    };
    let Some((thread_cwd, thread_title)) = started_thread_metadata(
        state,
        &remote_client_key,
        connection_epoch,
        &params,
        &thread_id,
    )
    .await
    else {
        state
            .push_event(
                "warn",
                "telegram_auto_topic_skipped",
                format!("thread={thread_id} could not read session metadata"),
            )
            .await;
        return;
    };
    if !is_current_bridge_generation(state, generation).await {
        return;
    }
    if thread_cwd.trim().is_empty() {
        state
            .push_event(
                "info",
                "telegram_auto_topic_skipped",
                format!("thread={thread_id} reason=session has no project directory"),
            )
            .await;
        return;
    }

    let Some(target) = find_auto_topic_target(state, api_registry, &thread_cwd).await else {
        return;
    };
    let _sync_guard = state.telegram_topic_sync_ops.lock().await;
    if !is_current_bridge_generation(state, generation).await {
        return;
    }

    if let Some((conversation_key, route)) = existing_binding_for_thread(state, &thread_id).await {
        state
            .push_event(
                "info",
                "telegram_auto_topic_skipped",
                format!(
                    "thread={} reason=already bound conversation={} platform={} chat={}",
                    thread_id,
                    conversation_key,
                    route.platform.key(),
                    route.chat_id
                ),
            )
            .await;
        return;
    }

    let topic_name = truncate_topic_name(&thread_title);
    let topic = match target
        .api
        .create_forum_topic(&target.chat_id, &topic_name)
        .await
    {
        Ok(topic) if topic.message_thread_id > 0 => topic,
        Ok(topic) => {
            state
                .push_event(
                    "warn",
                    "telegram_auto_topic_failed",
                    format!(
                        "thread={} chat={} reason=invalid Topic ID {}",
                        thread_id, target.chat_id, topic.message_thread_id
                    ),
                )
                .await;
            return;
        }
        Err(err) => {
            state
                .push_event(
                    "warn",
                    "telegram_auto_topic_failed",
                    format!(
                        "thread={} chat={} reason=创建 Topic 失败：{}",
                        thread_id, target.chat_id, err
                    ),
                )
                .await;
            return;
        }
    };

    // The bridge can be stopped while Telegram is creating the Topic. Remove
    // the orphan before leaving the detached task behind.
    if !is_current_bridge_generation(state, generation).await {
        delete_auto_created_topic(
            state,
            &target.api,
            &target.chat_id,
            topic.message_thread_id,
            &thread_id,
        )
        .await;
        return;
    }

    // An inbound Telegram action can bind the same session while the create
    // request is in flight. Do not leave the newly-created topic orphaned.
    if existing_binding_for_thread(state, &thread_id)
        .await
        .is_some()
    {
        delete_auto_created_topic(
            state,
            &target.api,
            &target.chat_id,
            topic.message_thread_id,
            &thread_id,
        )
        .await;
        state
            .push_event(
                "info",
                "telegram_auto_topic_skipped",
                format!("thread={thread_id} reason=绑定在创建期间已存在"),
            )
            .await;
        return;
    }

    let target_chat = telegram_message_target(&target.chat_id, Some(topic.message_thread_id));
    let route = RouteTarget {
        platform: ImPlatformKind::Telegram,
        conversation_key: format!("telegram:{}:{}", target.account_id, target_chat),
        account_id: target.account_id.clone(),
        chat_id: target_chat,
        remote_client_key: String::new(),
    }
    .with_deterministic_remote_client_key();

    if !is_current_bridge_generation(state, generation).await {
        delete_auto_created_topic(
            state,
            &target.api,
            &target.chat_id,
            topic.message_thread_id,
            &thread_id,
        )
        .await;
        return;
    }

    let resume_result = match connection_epoch {
        Some(connection_epoch) => {
            remote_control_backend::resume_thread_for_client_on_connection(
                state,
                connection_epoch,
                &route.remote_client_key,
                &thread_id,
                true,
            )
            .await
        }
        None => {
            remote_control_backend::resume_thread_for_client(
                state,
                &route.remote_client_key,
                &thread_id,
                true,
            )
            .await
        }
    };
    if let Err(err) = resume_result {
        delete_auto_created_topic(
            state,
            &target.api,
            &target.chat_id,
            topic.message_thread_id,
            &thread_id,
        )
        .await;
        state
            .push_event(
                "warn",
                "telegram_auto_topic_failed",
                format!("thread={thread_id} reason=订阅会话失败：{err}"),
            )
            .await;
        return;
    }

    if !is_current_bridge_generation(state, generation).await {
        delete_auto_created_topic(
            state,
            &target.api,
            &target.chat_id,
            topic.message_thread_id,
            &thread_id,
        )
        .await;
        return;
    }

    if let Err(err) = bind_thread_to_route(
        state,
        &route,
        &thread_id,
        None,
        route.remote_client_key.clone(),
    )
    .await
    {
        delete_auto_created_topic(
            state,
            &target.api,
            &target.chat_id,
            topic.message_thread_id,
            &thread_id,
        )
        .await;
        let _ = crate::im::core::routing::clear_thread_binding_with_reason(
            state,
            &route.conversation_key,
            "telegram_auto_topic_binding_failed",
        )
        .await;
        state
            .push_event(
                "warn",
                "telegram_auto_topic_failed",
                format!("thread={thread_id} reason=保存绑定失败：{err}"),
            )
            .await;
        return;
    }

    if !is_current_bridge_generation(state, generation).await {
        let _ = crate::im::core::routing::clear_thread_binding_for_thread_with_reason(
            state,
            &thread_id,
            &route.remote_client_key,
            "telegram_auto_topic_stale_generation",
        )
        .await;
        delete_auto_created_topic(
            state,
            &target.api,
            &target.chat_id,
            topic.message_thread_id,
            &thread_id,
        )
        .await;
        return;
    }

    if let Err(err) = persist_auto_topic_name(
        state,
        &route.conversation_key,
        &thread_id,
        &thread_title,
        &topic_name,
    )
    .await
    {
        state
            .push_event(
                "warn",
                "telegram_auto_topic_binding_state_save_failed",
                format!("thread={thread_id} err={err}"),
            )
            .await;
    }
    if !is_current_bridge_generation(state, generation).await {
        let _ = crate::im::core::routing::clear_thread_binding_for_thread_with_reason(
            state,
            &thread_id,
            &route.remote_client_key,
            "telegram_auto_topic_stale_generation",
        )
        .await;
        delete_auto_created_topic(
            state,
            &target.api,
            &target.chat_id,
            topic.message_thread_id,
            &thread_id,
        )
        .await;
        return;
    }
    state
        .push_event(
            "info",
            "telegram_auto_topic_created",
            format!(
                "thread={} account={} chat={} topic={} name={}",
                thread_id, target.account_id, target.chat_id, topic.message_thread_id, topic_name
            ),
        )
        .await;
}

async fn is_current_bridge_generation(state: &SharedState, generation: u64) -> bool {
    state.runtime.lock().await.is_bridge_generation(generation)
}

#[derive(Clone)]
struct AutoTopicTarget {
    account_id: String,
    chat_id: String,
    api: TelegramApi,
}

async fn find_auto_topic_target(
    state: &SharedState,
    api_registry: &ImApiRegistry,
    thread_cwd: &str,
) -> Option<AutoTopicTarget> {
    let accounts = state.config.lock().await.effective_telegram_accounts();
    let mut matches = Vec::new();
    for account in accounts.into_iter().filter(|account| account.is_active()) {
        let account_id = account.account_id.trim().to_string();
        let Some(api) = api_registry.telegram.get(&account_id).cloned() else {
            continue;
        };
        for group in account.project_groups {
            let project_cwd = group.cwd.trim();
            if project_cwd.is_empty() || !cwd_is_within_project(thread_cwd, project_cwd) {
                continue;
            }
            let specificity = Path::new(project_cwd).components().count();
            matches.push((
                specificity,
                AutoTopicTarget {
                    account_id: account_id.clone(),
                    chat_id: group.chat_id.trim().to_string(),
                    api: api.clone(),
                },
            ));
        }
    }
    if matches.is_empty() {
        state
            .push_event(
                "info",
                "telegram_auto_topic_skipped",
                format!(
                    "cwd={} reason=没有匹配的 Telegram 项目群",
                    thread_cwd.trim()
                ),
            )
            .await;
        return None;
    }
    let best_specificity = matches.iter().map(|(specificity, _)| *specificity).max()?;
    let mut best = matches
        .into_iter()
        .filter(|(specificity, _)| *specificity == best_specificity)
        .map(|(_, target)| target);
    let target = best.next()?;
    if best.next().is_some() {
        state
            .push_event(
                "warn",
                "telegram_auto_topic_skipped",
                format!(
                    "cwd={} reason=匹配到多个同级 Telegram 项目群",
                    thread_cwd.trim()
                ),
            )
            .await;
        return None;
    }
    Some(target)
}

async fn started_thread_metadata(
    state: &SharedState,
    remote_client_key: &str,
    connection_epoch: Option<u64>,
    params: &Value,
    thread_id: &str,
) -> Option<(String, String)> {
    let thread_value = params.get("thread").unwrap_or(params);
    let mut cwd = started_thread_field(params, thread_value, &["cwd", "workingDirectory"]);
    let mut title = started_thread_field(params, thread_value, &["name", "title", "threadName"]);

    if cwd.is_none() || title.is_none() {
        let history = match connection_epoch {
            Some(connection_epoch) => {
                remote_control_backend::session_history_threads_for_client_on_connection(
                    state,
                    connection_epoch,
                    remote_client_key,
                    AUTO_TOPIC_SESSION_PAGE_LIMIT,
                    AUTO_TOPIC_SESSION_MAX_PAGES,
                    false,
                )
                .await
            }
            None => {
                remote_control_backend::session_history_threads(
                    state,
                    remote_client_key,
                    AUTO_TOPIC_SESSION_PAGE_LIMIT,
                    AUTO_TOPIC_SESSION_MAX_PAGES,
                    false,
                )
                .await
            }
        };
        let threads = match history {
            Ok(threads) => threads,
            Err(err) => {
                state
                    .push_event(
                        "warn",
                        "telegram_auto_topic_metadata_query_failed",
                        format!(
                            "thread={} connection_epoch={} err={}",
                            thread_id,
                            connection_epoch
                                .map(|epoch| epoch.to_string())
                                .unwrap_or_else(|| "any".to_string()),
                            err
                        ),
                    )
                    .await;
                // A notification with a known source must never query a
                // different Codex connection after that source disappears.
                if connection_epoch.is_some() {
                    return None;
                }
                Vec::new()
            }
        };
        if let Some(thread) = threads.iter().find(|thread| {
            thread
                .get("id")
                .or_else(|| thread.get("threadId"))
                .and_then(Value::as_str)
                .is_some_and(|id| id.trim() == thread_id)
        }) {
            cwd = cwd.or_else(|| {
                thread
                    .get("cwd")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
            });
            title =
                title.or_else(|| Some(summarize_thread_title(thread, im_text_for_state(state))));
        }
    }

    let title = title.unwrap_or_else(|| "未命名会话".to_string());
    Some((cwd.unwrap_or_default(), title))
}

fn started_thread_id(params: &Value) -> Option<String> {
    let non_empty = |value: Option<&Value>| {
        value
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    non_empty(params.get("threadId"))
        .or_else(|| non_empty(params.get("thread_id")))
        .or_else(|| non_empty(params.get("thread").and_then(|thread| thread.get("id"))))
}

fn started_thread_field(params: &Value, thread: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        thread
            .get(*key)
            .and_then(Value::as_str)
            .or_else(|| params.get(*key).and_then(Value::as_str))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

fn cwd_is_within_project(thread_cwd: &str, project_cwd: &str) -> bool {
    let thread_cwd = thread_cwd.trim();
    let project_cwd = project_cwd.trim();
    if thread_cwd.is_empty() || project_cwd.is_empty() {
        return false;
    }
    let thread_path = Path::new(thread_cwd);
    let project_path = Path::new(project_cwd);
    thread_path == project_path || thread_path.starts_with(project_path)
}

async fn existing_binding_for_thread(
    state: &SharedState,
    thread_id: &str,
) -> Option<(String, RouteTarget)> {
    if let Some(route) = state.runtime.lock().await.route_for_thread(thread_id) {
        return Some((route.conversation_key.clone(), route));
    }
    let persisted = state.persisted.lock().await;
    persisted
        .im_thread_bindings
        .iter()
        .find(|(_, bound_thread_id)| bound_thread_id.trim() == thread_id)
        .and_then(|(conversation_key, _)| {
            route_from_conversation_key(conversation_key)
                .map(|route| (conversation_key.clone(), route))
        })
}

async fn persist_auto_topic_name(
    state: &SharedState,
    conversation_key: &str,
    thread_id: &str,
    codex_title: &str,
    topic_name: &str,
) -> anyhow::Result<()> {
    let mut persisted = state.persisted.lock().await;
    let Some(binding) = persisted
        .telegram_topic_binding_states
        .get_mut(conversation_key)
    else {
        return Ok(());
    };
    if binding.thread_id != thread_id {
        return Ok(());
    }
    binding.codex_title = codex_title.to_string();
    binding.topic_name = topic_name.to_string();
    binding.last_synced_codex_title = codex_title.to_string();
    binding.last_synced_topic_name = topic_name.to_string();
    binding.last_checked_at_ms = now_ms();
    let path = state.config.lock().await.state_path.clone();
    persisted.save(&path)
}

async fn delete_auto_created_topic(
    state: &SharedState,
    api: &TelegramApi,
    chat_id: &str,
    topic_id: i64,
    thread_id: &str,
) {
    match api.delete_forum_topic(chat_id, topic_id).await {
        Ok(true) => {
            state
                .push_event(
                    "info",
                    "telegram_auto_topic_cleanup",
                    format!(
                        "thread={} chat={} topic={} deleted",
                        thread_id, chat_id, topic_id
                    ),
                )
                .await;
        }
        Ok(false) => {
            state
                .push_event(
                    "warn",
                    "telegram_auto_topic_cleanup_failed",
                    format!(
                        "thread={} chat={} topic={} api returned false",
                        thread_id, chat_id, topic_id
                    ),
                )
                .await;
        }
        Err(err) => {
            state
                .push_event(
                    "warn",
                    "telegram_auto_topic_cleanup_failed",
                    format!(
                        "thread={} chat={} topic={} err={err}",
                        thread_id, chat_id, topic_id
                    ),
                )
                .await;
        }
    }
}

pub async fn listen_polling(
    state: SharedState,
    api: TelegramApi,
    attachment_root: PathBuf,
    tx: mpsc::Sender<InboundMessage>,
) -> Result<()> {
    let account_id = api.settings().account_id();
    let mut chat_access = TelegramChatAccess::new(api.settings().allowed_chat_ids.clone());
    let mut offset = None;
    set_polling_state(&state, &account_id, true, false, None).await;
    claim_polling_slot(&state, &api, &mut offset).await;
    let mut last_reconciliation_at = Instant::now() - TELEGRAM_TOPIC_RECONCILIATION_INTERVAL;
    loop {
        let updates = match api
            .get_updates(offset, TELEGRAM_LONG_POLL_TIMEOUT_SECONDS)
            .await
        {
            Ok(updates) => updates,
            Err(err) => {
                handle_polling_error(&state, &account_id, &err).await;
                continue;
            }
        };
        set_polling_state(&state, &account_id, true, true, None).await;
        let update_count = updates.len();
        for update in updates {
            offset = Some(update.update_id + 1);
            if let Some(callback) = update.callback_query {
                let callback_id = callback.id.clone();
                let access = ensure_callback_chat_allowed(
                    &state,
                    &api,
                    &mut chat_access,
                    callback.message.as_ref().map(|message| &message.chat),
                )
                .await;
                if access == TelegramChatAccessDecision::Allowed
                    && let Some(inbound) = inbound_from_callback(
                        api.settings(),
                        &chat_access.allowed_chat_ids,
                        callback,
                    )
                {
                    let _ = api
                        .answer_callback_query(&callback_id, Some("已收到"))
                        .await;
                    tx.send(inbound)
                        .await
                        .map_err(|_| anyhow::anyhow!("telegram inbound pump closed"))?;
                    update_last_inbound(&state, &account_id).await;
                } else {
                    let message = match access {
                        TelegramChatAccessDecision::Denied => "当前聊天未授权",
                        _ => "这个操作不可用",
                    };
                    let _ = api.answer_callback_query(&callback_id, Some(message)).await;
                }
                continue;
            }
            if let Some(message) = update.message {
                if handle_forum_topic_service_message(&state, &api, &message).await {
                    continue;
                }
                if message.from.as_ref().is_some_and(|user| user.is_bot) {
                    continue;
                }
                match ensure_message_chat_allowed(&state, &api, &mut chat_access, &message).await {
                    TelegramChatAccessDecision::Allowed => {
                        let inbound = inbound_from_message(
                            api.settings(),
                            &chat_access.allowed_chat_ids,
                            &message,
                        );
                        let inbound = match inbound {
                            Some(inbound) => Some(inbound),
                            None => {
                                inbound_from_message_after_topic_creation(
                                    &api,
                                    api.settings(),
                                    &chat_access.allowed_chat_ids,
                                    &message,
                                )
                                .await
                            }
                        };
                        if let Some(mut inbound) = inbound {
                            let collection =
                                collect_telegram_attachments(&api, &attachment_root, &message)
                                    .await;
                            let media_not_delivered =
                                message_has_media(&message) && collection.attachments.is_empty();
                            if !collection.failures.is_empty() {
                                let notice = attachment_failure_notice(
                                    &collection.failures,
                                    media_not_delivered && !inbound.text.trim().is_empty(),
                                );
                                if let Err(err) = api.send_text(&inbound.chat_id, &notice).await {
                                    chain_log::write_diagnostic_lazy(|| {
                                        format!(
                                            "[telegram_attachment] event=failure_notice_failed message={} chat={} err={}",
                                            message.message_id, inbound.chat_id, err
                                        )
                                    });
                                }
                            }
                            inbound.attachments = collection.attachments;
                            if media_not_delivered {
                                update_last_inbound(&state, &account_id).await;
                                continue;
                            }
                            if inbound.text.trim().is_empty() && inbound.attachments.is_empty() {
                                continue;
                            }
                            let _ = api.send_chat_action(&inbound.chat_id, "typing").await;
                            tx.send(inbound)
                                .await
                                .map_err(|_| anyhow::anyhow!("telegram inbound pump closed"))?;
                            update_last_inbound(&state, &account_id).await;
                        }
                    }
                    TelegramChatAccessDecision::Denied => {
                        let chat_id = message.chat.id.to_string();
                        let _ = api
                            .send_text(
                                &chat_id,
                                "当前 Telegram 私聊未授权。请在本机 MochiPort 配置 allowedChatIds。",
                            )
                            .await;
                    }
                    TelegramChatAccessDecision::Ignored => {}
                }
            }
        }
        if update_count > 0 {
            state
                .push_event(
                    "info",
                    "telegram_poll_ok",
                    format!("updates={update_count}"),
                )
                .await;
        }
        if last_reconciliation_at.elapsed() >= TELEGRAM_TOPIC_RECONCILIATION_INTERVAL {
            reconcile_telegram_topic_bindings(&state, &api).await;
            last_reconciliation_at = Instant::now();
        }
    }
}

async fn handle_forum_topic_service_message(
    state: &SharedState,
    api: &TelegramApi,
    message: &TelegramMessage,
) -> bool {
    let Some(topic_id) = message.message_thread_id else {
        return false;
    };
    let topic_state = if message.forum_topic_closed.is_some() {
        Some("closed")
    } else if message.forum_topic_reopened.is_some() {
        Some("open")
    } else if message.forum_topic_created.is_some() || message.forum_topic_edited.is_some() {
        Some("open")
    } else {
        None
    };
    let Some(topic_state) = topic_state else {
        return false;
    };
    let topic_name = message
        .forum_topic_created
        .as_ref()
        .map(|topic| topic.name.trim())
        .filter(|name| !name.is_empty())
        .or_else(|| {
            message
                .forum_topic_edited
                .as_ref()
                .and_then(|topic| topic.name.as_deref())
                .map(str::trim)
                .filter(|name| !name.is_empty())
        })
        .map(str::to_string);
    let key = format!(
        "telegram:{}:{}",
        api.settings().account_id(),
        crate::types::telegram_message_target(&message.chat.id.to_string(), Some(topic_id))
    );
    // Telegram echoes edits made through editForumTopic as a service message.
    // Consume our own expected value before touching persistence so a bot
    // initiated rename does not trigger a reverse Codex request.
    let topic_name_was_expected = if let Some(name) = topic_name.as_deref() {
        let mut ops = state.telegram_topic_name_sync_ops.lock().await;
        match ops.get(&key) {
            Some(expected) if expected == name => {
                ops.remove(&key);
                true
            }
            Some(_) => {
                // A different name means the user renamed the Topic again;
                // discard the stale bot marker and process this as a real
                // external change.
                ops.remove(&key);
                false
            }
            None => false,
        }
    } else {
        false
    };
    let topic_name = topic_name.map(|name| truncate_topic_name(&name));
    let topic_name_for_sync = topic_name.clone();
    let mut pending_codex_sync = None;
    {
        let mut persisted = state.persisted.lock().await;
        if let Some(binding) = persisted.telegram_topic_binding_states.get_mut(&key) {
            let bound_thread_id = binding.thread_id.clone();
            binding.telegram_state = topic_state.to_string();
            if let Some(topic_name) = topic_name {
                binding.topic_name = topic_name.clone();
                if topic_name_was_expected {
                    binding.codex_title = topic_name.clone();
                    binding.last_synced_codex_title = topic_name.clone();
                    binding.last_synced_topic_name = topic_name;
                } else if !bound_thread_id.trim().is_empty() {
                    pending_codex_sync = Some((
                        bound_thread_id,
                        topic_name_for_sync.clone().unwrap_or_default(),
                    ));
                }
            }
            binding.last_checked_at_ms = crate::types::now_ms();
            let path = state.config.lock().await.state_path.clone();
            if let Err(err) = persisted.save(&path) {
                chain_log::write_diagnostic_lazy(|| {
                    format!(
                        "[telegram_topic] event=state_save_failed key={} err={err}",
                        key
                    )
                });
            }
        }
    }

    if let Some((thread_id, topic_name)) = pending_codex_sync
        && !topic_name.trim().is_empty()
    {
        let remote_client_key = state
            .runtime
            .lock()
            .await
            .route_for_thread(&thread_id)
            .map(|route| route.remote_client_key)
            .or_else(|| {
                crate::im_runtime::route_from_conversation_key(&key)
                    .map(|route| route.remote_client_key)
            })
            .unwrap_or_default();
        {
            let mut ops = state.codex_thread_name_sync_ops.lock().await;
            ops.insert(thread_id.clone(), topic_name.clone());
        }
        let marker_state = state.clone();
        let marker_thread_id = thread_id.clone();
        let marker_topic_name = topic_name.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(30)).await;
            let mut ops = marker_state.codex_thread_name_sync_ops.lock().await;
            if ops
                .get(&marker_thread_id)
                .is_some_and(|expected| expected == &marker_topic_name)
            {
                ops.remove(&marker_thread_id);
            }
        });
        match crate::remote_control_backend::set_thread_name_for_client(
            state,
            &remote_client_key,
            &thread_id,
            &topic_name,
        )
        .await
        {
            Ok(()) => {
                let mut persisted = state.persisted.lock().await;
                if let Some(binding) = persisted.telegram_topic_binding_states.get_mut(&key)
                    && binding.thread_id == thread_id
                {
                    binding.codex_title = topic_name.clone();
                    binding.topic_name = topic_name.clone();
                    binding.last_synced_codex_title = topic_name.clone();
                    binding.last_synced_topic_name = topic_name.clone();
                    binding.last_checked_at_ms = crate::types::now_ms();
                    let path = state.config.lock().await.state_path.clone();
                    if let Err(err) = persisted.save(&path) {
                        chain_log::write_diagnostic_lazy(|| {
                            format!(
                                "[telegram_topic] event=sync_save_failed key={} err={err}",
                                key
                            )
                        });
                    }
                }
                state
                    .push_event(
                        "info",
                        "telegram_topic_name_synced_to_codex",
                        format!("thread={} name={}", thread_id, topic_name),
                    )
                    .await;
            }
            Err(err) => {
                state
                    .codex_thread_name_sync_ops
                    .lock()
                    .await
                    .remove(&thread_id);
                state
                    .push_event(
                        "warn",
                        "telegram_topic_name_sync_to_codex_failed",
                        format!("thread={} err={err}", thread_id),
                    )
                    .await;
            }
        }
    }
    true
}

async fn reconcile_telegram_topic_bindings(state: &SharedState, api: &TelegramApi) {
    let account_id = api.settings().account_id();
    let (active, archived) = tokio::join!(
        crate::remote_control_backend::session_history_threads(
            state,
            crate::remote_control_backend::default_remote_client_key(),
            100,
            20,
            false,
        ),
        crate::remote_control_backend::session_history_threads(
            state,
            crate::remote_control_backend::default_remote_client_key(),
            100,
            20,
            true,
        )
    );
    let Ok(active) = active else {
        state
            .push_event(
                "warn",
                "telegram_topic_reconcile_skipped",
                "active session query failed",
            )
            .await;
        return;
    };
    let Ok(archived) = archived else {
        state
            .push_event(
                "warn",
                "telegram_topic_reconcile_skipped",
                "archived session query failed",
            )
            .await;
        return;
    };
    let active_ids = session_ids(&active);
    let archived_ids = session_ids(&archived);
    let text = im_text_for_state(state);
    let mut codex_titles = thread_titles(&archived, text);
    codex_titles.extend(thread_titles(&active, text));
    let bindings = {
        let persisted = state.persisted.lock().await;
        persisted
            .im_thread_bindings
            .iter()
            .filter(|(key, _)| key.starts_with(&format!("telegram:{account_id}:")))
            .map(|(key, thread_id)| {
                (
                    key.clone(),
                    thread_id.clone(),
                    persisted.telegram_topic_binding_states.get(key).cloned(),
                )
            })
            .collect::<Vec<_>>()
    };
    for (conversation_key, thread_id, state_snapshot) in bindings {
        let Some(route) = crate::im_runtime::route_from_conversation_key(&conversation_key) else {
            continue;
        };
        let (_, topic_id) = crate::types::split_telegram_message_target(&route.chat_id);
        let Some(topic_id) = topic_id else {
            continue;
        };
        let (raw_chat_id, _) = crate::types::split_telegram_message_target(&route.chat_id);
        let mut next_state = state_snapshot.unwrap_or_default();
        next_state.thread_id = thread_id.clone();
        let current_topic_name = next_state.topic_name.trim().to_string();
        let codex_title = codex_titles.get(&thread_id).cloned();
        let target_topic_name = codex_title
            .as_deref()
            .map(truncate_topic_name)
            .unwrap_or_else(|| {
                if current_topic_name.is_empty() {
                    "未命名会话".to_string()
                } else {
                    current_topic_name.clone()
                }
            });
        let baseline_codex_title = next_state.last_synced_codex_title.trim();
        let baseline_topic_name = if next_state.last_synced_topic_name.trim().is_empty() {
            current_topic_name.as_str()
        } else {
            next_state.last_synced_topic_name.trim()
        };
        let initial_sync = baseline_codex_title.is_empty();
        let codex_changed = !initial_sync
            && codex_title
                .as_deref()
                .is_some_and(|title| title != baseline_codex_title);
        let telegram_changed = !initial_sync
            && !current_topic_name.is_empty()
            && !baseline_topic_name.is_empty()
            && current_topic_name != baseline_topic_name;
        // If both sides changed since the last successful pass, Codex remains
        // authoritative and the Topic is brought back to the Codex title.
        let telegram_to_codex = !codex_changed && telegram_changed;
        let probe_name = if telegram_to_codex {
            current_topic_name.as_str()
        } else {
            target_topic_name.as_str()
        };
        match api
            .edit_forum_topic(raw_chat_id, topic_id, probe_name)
            .await
        {
            Ok(true) => {
                if current_topic_name.is_empty() {
                    next_state.topic_name = probe_name.to_string();
                    if next_state.last_synced_topic_name.trim().is_empty() {
                        next_state.last_synced_topic_name = probe_name.to_string();
                    }
                }
            }
            Ok(false) => {
                state
                    .push_event(
                        "warn",
                        "telegram_topic_probe_failed",
                        format!(
                            "chat={} topic={} api returned false",
                            route.chat_id, topic_id
                        ),
                    )
                    .await;
                continue;
            }
            Err(err)
                if err
                    .downcast_ref::<TelegramApiError>()
                    .is_some_and(|error| error.is_forum_topic_missing()) =>
            {
                let clear_result = crate::im::core::routing::clear_thread_binding_with_reason(
                    state,
                    &conversation_key,
                    "telegram_topic_missing_during_reconcile",
                )
                .await;
                match clear_result {
                    Ok(()) => {
                        state
                            .push_event(
                                "info",
                                "telegram_topic_binding_removed",
                                format!(
                                    "chat={} topic={} no longer exists",
                                    route.chat_id, topic_id
                                ),
                            )
                            .await;
                    }
                    Err(err) => {
                        state
                            .push_event(
                                "warn",
                                "telegram_topic_binding_remove_failed",
                                format!(
                                    "chat={} topic={} clear binding failed: {err}",
                                    route.chat_id, topic_id
                                ),
                            )
                            .await;
                    }
                }
                continue;
            }
            Err(err) => {
                state
                    .push_event(
                        "warn",
                        "telegram_topic_probe_failed",
                        format!("chat={} topic={} err={err}", route.chat_id, topic_id),
                    )
                    .await;
                continue;
            }
        }
        let now = crate::types::now_ms();
        if let Some(codex_title) = codex_title.as_deref() {
            if telegram_to_codex {
                match crate::remote_control_backend::set_thread_name_for_client(
                    state,
                    &route.remote_client_key,
                    &thread_id,
                    &current_topic_name,
                )
                .await
                {
                    Ok(()) => {
                        next_state.codex_title = current_topic_name.clone();
                        next_state.topic_name = current_topic_name.clone();
                        next_state.last_synced_codex_title = current_topic_name.clone();
                        next_state.last_synced_topic_name = current_topic_name.clone();
                        state
                            .push_event(
                                "info",
                                "telegram_topic_name_synced_to_codex",
                                format!("thread={} name={}", thread_id, current_topic_name),
                            )
                            .await;
                    }
                    Err(err) => {
                        next_state.codex_title = codex_title.to_string();
                        state
                            .push_event(
                                "warn",
                                "telegram_topic_name_sync_to_codex_failed",
                                format!("thread={} err={err}", thread_id),
                            )
                            .await;
                    }
                }
            } else {
                next_state.codex_title = codex_title.to_string();
                next_state.topic_name = target_topic_name.clone();
                next_state.last_synced_codex_title = codex_title.to_string();
                next_state.last_synced_topic_name = target_topic_name.clone();
            }
        } else if !current_topic_name.is_empty() {
            next_state.topic_name = current_topic_name;
        }
        // Older bindings may have neither a saved Topic name nor a Codex
        // title. Once the probe above succeeds, persist a complete baseline
        // so later reconciliations can detect a real rename or deletion.
        if next_state.codex_title.trim().is_empty() {
            next_state.codex_title = target_topic_name.clone();
        }
        if next_state.last_synced_codex_title.trim().is_empty() {
            next_state.last_synced_codex_title = next_state.codex_title.clone();
        }
        if next_state.last_synced_topic_name.trim().is_empty()
            && !next_state.topic_name.trim().is_empty()
        {
            next_state.last_synced_topic_name = next_state.topic_name.clone();
        }
        next_state.last_checked_at_ms = now;
        let should_delete = if active_ids.contains(&thread_id) {
            next_state.codex_state = "active".to_string();
            next_state.archived_at_ms = None;
            next_state.missing_at_ms = None;
            false
        } else if archived_ids.contains(&thread_id) {
            next_state.codex_state = "archived".to_string();
            next_state.missing_at_ms = None;
            let archived_at = next_state.archived_at_ms.get_or_insert(now);
            now.saturating_sub(*archived_at) >= TELEGRAM_TOPIC_STATE_GRACE.as_millis()
        } else {
            next_state.codex_state = "missing".to_string();
            let missing_at = next_state.missing_at_ms.get_or_insert(now);
            now.saturating_sub(*missing_at) >= TELEGRAM_TOPIC_STATE_GRACE.as_millis()
        };

        if should_delete {
            match api.delete_forum_topic(raw_chat_id, topic_id).await {
                Ok(true) => {
                    let _ = crate::im::core::routing::clear_thread_binding_with_reason(
                        state,
                        &conversation_key,
                        "codex_session_archived_or_deleted",
                    )
                    .await;
                    continue;
                }
                Ok(false) => {
                    state
                        .push_event(
                            "warn",
                            "telegram_topic_delete_failed",
                            format!(
                                "chat={} topic={} api returned false",
                                route.chat_id, topic_id
                            ),
                        )
                        .await;
                }
                Err(err) => {
                    state
                        .push_event(
                            "warn",
                            "telegram_topic_delete_failed",
                            format!("chat={} topic={} err={err}", route.chat_id, topic_id),
                        )
                        .await;
                }
            }
        }
        let mut persisted = state.persisted.lock().await;
        persisted
            .telegram_topic_binding_states
            .insert(conversation_key, next_state);
        let path = state.config.lock().await.state_path.clone();
        if let Err(err) = persisted.save(&path) {
            chain_log::write_diagnostic_lazy(|| {
                format!("[telegram_topic] event=reconcile_save_failed err={err}")
            });
        }
    }
}

fn thread_titles(
    threads: &[serde_json::Value],
    text: crate::im::core::i18n::ImText,
) -> HashMap<String, String> {
    threads
        .iter()
        .filter_map(|thread| {
            let thread_id = thread
                .get("id")
                .or_else(|| thread.get("threadId"))
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())?
                .to_string();
            let title = summarize_thread_title(thread, text);
            Some((thread_id, title))
        })
        .collect()
}

pub(crate) fn truncate_topic_name(title: &str) -> String {
    let title = title.trim();
    let title = if title.is_empty() {
        "未命名会话"
    } else {
        title
    };
    title.chars().take(64).collect()
}

fn session_ids(threads: &[serde_json::Value]) -> HashSet<String> {
    threads
        .iter()
        .filter_map(|thread| {
            thread
                .get("id")
                .or_else(|| thread.get("threadId"))
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
        .collect()
}

async fn inbound_from_message_after_topic_creation(
    api: &TelegramApi,
    settings: &TelegramSettings,
    allowed_chat_ids: &[String],
    message: &TelegramMessage,
) -> Option<InboundMessage> {
    if message.chat.kind == "private" || message.message_thread_id.is_some() {
        return None;
    }
    let raw_chat_id = message.chat.id.to_string();
    let project = settings.project_group_for_chat(&raw_chat_id)?;
    let text = message
        .text
        .as_deref()
        .or(message.caption.as_deref())
        .unwrap_or_default()
        .trim();
    if text.is_empty() && !message_has_media(message) {
        return None;
    }

    let topic_name = forum_topic_name(project.project_name.as_str(), text);
    let topic = match api.create_forum_topic(&raw_chat_id, &topic_name).await {
        Ok(topic) if topic.message_thread_id > 0 => topic,
        Ok(topic) => {
            chain_log::write_diagnostic_lazy(|| {
                format!(
                    "[telegram_topic] event=invalid_topic_id chat={} topic_id={}",
                    raw_chat_id, topic.message_thread_id
                )
            });
            return None;
        }
        Err(error) => {
            chain_log::write_diagnostic_lazy(|| {
                format!(
                    "[telegram_topic] event=create_failed chat={} project={} err={}",
                    raw_chat_id, project.project_name, error
                )
            });
            let _ = api
                .send_text(
                    &raw_chat_id,
                    "这个项目群需要开启 Telegram 论坛模式，机器人也需要管理主题的权限。",
                )
                .await;
            return None;
        }
    };

    let mut routed_message = message.clone();
    routed_message.message_thread_id = Some(topic.message_thread_id);
    let inbound = inbound_from_message(settings, allowed_chat_ids, &routed_message)?;
    chain_log::write_diagnostic_lazy(|| {
        format!(
            "[telegram_topic] event=created chat={} topic_id={} project={}",
            raw_chat_id, topic.message_thread_id, project.project_name
        )
    });
    Some(inbound)
}

fn forum_topic_name(project_name: &str, text: &str) -> String {
    let first_line = text.lines().next().unwrap_or_default().trim();
    let candidate = if first_line.is_empty() {
        project_name.trim()
    } else {
        first_line
    };
    let candidate = candidate.trim_start_matches('/').trim();
    let candidate = if candidate.is_empty() {
        "新任务"
    } else {
        candidate
    };
    candidate.chars().take(64).collect()
}

async fn claim_polling_slot(state: &SharedState, api: &TelegramApi, offset: &mut Option<i64>) {
    loop {
        match api.get_updates(*offset, 0).await {
            Ok(updates) => {
                for update in updates {
                    *offset = Some(update.update_id + 1);
                }
                set_polling_state(state, &api.settings().account_id(), true, true, None).await;
                let text = im_text_for_state(state);
                let commands = text
                    .telegram_command_menu()
                    .into_iter()
                    .map(|(command, description)| TelegramBotCommand {
                        command: command.to_string(),
                        description: description.to_string(),
                    })
                    .collect::<Vec<_>>();
                if let Err(err) = api.set_my_commands(&commands).await {
                    state
                        .push_event("warn", "telegram_command_menu_failed", format!("err={err}"))
                        .await;
                }
                state
                    .push_event(
                        "info",
                        "telegram_poll_ready",
                        "startup probe ok".to_string(),
                    )
                    .await;
                return;
            }
            Err(err) => {
                let delay = retry_delay_seconds(&err, TELEGRAM_STARTUP_PROBE_RETRY_SECONDS);
                set_polling_state(
                    state,
                    &api.settings().account_id(),
                    true,
                    false,
                    Some(err.to_string()),
                )
                .await;
                state
                    .push_event(
                        "warn",
                        "telegram_poll_probe_failed",
                        format!("retry_in={delay}s err={err}"),
                    )
                    .await;
                sleep(Duration::from_secs(delay)).await;
            }
        }
    }
}

async fn handle_polling_error(state: &SharedState, account_id: &str, err: &anyhow::Error) {
    let delay = retry_delay_seconds(err, TELEGRAM_GENERIC_RETRY_SECONDS);
    let kind = err
        .downcast_ref::<TelegramApiError>()
        .filter(|api_error| api_error.is_conflict())
        .map(|_| "telegram_poll_conflict")
        .unwrap_or("telegram_poll_failed");
    set_polling_state(state, account_id, true, false, Some(err.to_string())).await;
    state
        .push_event("warn", kind, format!("retry_in={delay}s err={err}"))
        .await;
    sleep(Duration::from_secs(delay)).await;
}

fn retry_delay_seconds(err: &anyhow::Error, default_delay: u64) -> u64 {
    if let Some(api_error) = err.downcast_ref::<TelegramApiError>() {
        if api_error.is_conflict() {
            return TELEGRAM_CONFLICT_BACKOFF_SECONDS;
        }
        if let Some(retry_after) = api_error.retry_after {
            return retry_after.max(1);
        }
    }
    default_delay
}

fn inbound_from_message(
    settings: &TelegramSettings,
    allowed_chat_ids: &[String],
    message: &TelegramMessage,
) -> Option<InboundMessage> {
    let is_private = message.chat.kind == "private";
    let text = message
        .text
        .as_deref()
        .or(message.caption.as_deref())
        .unwrap_or_default()
        .trim()
        .to_string();
    if text.is_empty() && !message_has_media(message) {
        return None;
    }
    let raw_chat_id = message.chat.id.to_string();
    if is_private && !chat_allowed(allowed_chat_ids, &raw_chat_id) {
        return None;
    }
    if !is_private
        && (settings.project_group_for_chat(&raw_chat_id).is_none()
            || message.message_thread_id.is_none())
    {
        return None;
    }
    let chat_id = telegram_message_target(&raw_chat_id, message.message_thread_id);
    let sender_id = message
        .from
        .as_ref()
        .map(|user| user.id.to_string())
        .unwrap_or_else(|| chat_id.clone());

    Some(InboundMessage {
        platform: ImPlatformKind::Telegram,
        account_id: settings.account_id(),
        sender_id,
        chat_id,
        chat_type: if is_private {
            ChatType::Direct
        } else {
            ChatType::Group
        },
        message_id: message.message_id.to_string(),
        received_at_ms: now_ms(),
        text,
        mentioned: is_private,
        approval_request_key: None,
        action: None,
        card_message_id: None,
        callback_req_id: None,
        callback_kind: None,
        attachments: vec![],
    })
}

const TELEGRAM_MAX_FILE_BYTES: u64 = 20 * 1024 * 1024;
const TELEGRAM_MAX_ATTACHMENTS_PER_MESSAGE: usize = 8;

fn message_has_media(message: &TelegramMessage) -> bool {
    message.photo.is_some()
        || message.document.is_some()
        || message.audio.is_some()
        || message.video.is_some()
        || message.animation.is_some()
        || message.voice.is_some()
        || message.video_note.is_some()
        || message.sticker.is_some()
}

#[derive(Debug, Clone)]
struct TelegramAttachmentSpec {
    file_id: String,
    kind: &'static str,
    directory: &'static str,
    name: String,
    mime_type: Option<String>,
    file_size: Option<u64>,
}

fn attachment_specs(message: &TelegramMessage) -> Vec<TelegramAttachmentSpec> {
    let mut specs = Vec::new();
    if let Some(photo) = message.photo.as_ref().and_then(|photos| {
        photos
            .iter()
            .max_by_key(|photo| u64::from(photo.width) * u64::from(photo.height))
    }) {
        specs.push(TelegramAttachmentSpec {
            file_id: photo.file_id.clone(),
            kind: "image",
            directory: "images",
            name: format!("telegram-{}.jpg", message.message_id),
            mime_type: Some("image/jpeg".to_string()),
            file_size: photo.file_size,
        });
    }
    if let Some(audio) = message.audio.as_ref() {
        specs.push(TelegramAttachmentSpec {
            file_id: audio.file_id.clone(),
            kind: "audio",
            directory: "files",
            name: audio
                .file_name
                .clone()
                .unwrap_or_else(|| format!("telegram-{}-audio.bin", message.message_id)),
            mime_type: audio.mime_type.clone(),
            file_size: audio.file_size,
        });
    }
    if let Some(video) = message.video.as_ref() {
        specs.push(TelegramAttachmentSpec {
            file_id: video.file_id.clone(),
            kind: "video",
            directory: "videos",
            name: format!("telegram-{}.mp4", message.message_id),
            mime_type: video.mime_type.clone(),
            file_size: video.file_size,
        });
    }
    if let Some(animation) = message.animation.as_ref() {
        specs.push(TelegramAttachmentSpec {
            file_id: animation.file_id.clone(),
            kind: "video",
            directory: "videos",
            name: animation
                .file_name
                .clone()
                .unwrap_or_else(|| format!("telegram-{}-animation.mp4", message.message_id)),
            mime_type: animation.mime_type.clone(),
            file_size: animation.file_size,
        });
    }
    if let Some(voice) = message.voice.as_ref() {
        specs.push(TelegramAttachmentSpec {
            file_id: voice.file_id.clone(),
            kind: "audio",
            directory: "files",
            name: format!("telegram-{}-voice.ogg", message.message_id),
            mime_type: voice.mime_type.clone(),
            file_size: voice.file_size,
        });
    }
    if let Some(video_note) = message.video_note.as_ref() {
        specs.push(TelegramAttachmentSpec {
            file_id: video_note.file_id.clone(),
            kind: "video",
            directory: "videos",
            name: format!("telegram-{}-video-note.mp4", message.message_id),
            mime_type: Some("video/mp4".to_string()),
            file_size: video_note.file_size,
        });
    }
    if let Some(sticker) = message.sticker.as_ref() {
        let (kind, extension, mime_type) = if sticker.is_animated {
            ("file", "tgs", "application/x-tgsticker")
        } else if sticker.is_video {
            ("video", "webm", "video/webm")
        } else {
            ("image", "webp", "image/webp")
        };
        specs.push(TelegramAttachmentSpec {
            file_id: sticker.file_id.clone(),
            kind,
            directory: match kind {
                "image" => "images",
                "video" => "videos",
                _ => "files",
            },
            name: format!("telegram-{}-sticker.{extension}", message.message_id),
            mime_type: Some(mime_type.to_string()),
            file_size: sticker.file_size,
        });
    }
    // Telegram includes a compatibility `document` alongside some dedicated
    // media fields, notably `animation`. Add the generic representation last
    // so de-duplication preserves the richer media kind and filename.
    if let Some(document) = message.document.as_ref() {
        let is_image = document
            .mime_type
            .as_deref()
            .is_some_and(|mime_type| mime_type.starts_with("image/"))
            || document.file_name.as_deref().is_some_and(|file_name| {
                mime_guess::from_path(file_name)
                    .first()
                    .is_some_and(|mime_type| mime_type.essence_str().starts_with("image/"))
            });
        specs.push(TelegramAttachmentSpec {
            file_id: document.file_id.clone(),
            kind: if is_image { "image" } else { "file" },
            directory: if is_image { "images" } else { "files" },
            name: document
                .file_name
                .clone()
                .unwrap_or_else(|| format!("telegram-{}-document.bin", message.message_id)),
            mime_type: document.mime_type.clone(),
            file_size: document.file_size,
        });
    }
    let mut seen_file_ids = HashSet::new();
    specs.retain(|spec| seen_file_ids.insert(spec.file_id.clone()));
    specs.truncate(TELEGRAM_MAX_ATTACHMENTS_PER_MESSAGE);
    specs
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TelegramAttachmentFailureReason {
    TooLarge { bytes: Option<u64> },
    MetadataUnavailable,
    DownloadFailed,
    PersistFailed,
}

impl TelegramAttachmentFailureReason {
    fn user_description(&self) -> String {
        match self {
            Self::TooLarge { bytes } => bytes.map_or_else(
                || "超过 Telegram Bot 的 20 MB 下载上限".to_string(),
                |bytes| {
                    format!(
                        "大小为 {:.1} MB，超过 Telegram Bot 的 20 MB 下载上限",
                        bytes as f64 / (1024.0 * 1024.0)
                    )
                },
            ),
            Self::MetadataUnavailable => "无法读取 Telegram 文件信息".to_string(),
            Self::DownloadFailed => "从 Telegram 下载失败".to_string(),
            Self::PersistFailed => "下载后无法保存到本机".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TelegramAttachmentFailure {
    name: String,
    reason: TelegramAttachmentFailureReason,
}

#[derive(Debug, Default)]
struct TelegramAttachmentCollection {
    attachments: Vec<crate::types::InboundAttachment>,
    failures: Vec<TelegramAttachmentFailure>,
}

fn attachment_failure(
    spec: &TelegramAttachmentSpec,
    reason: TelegramAttachmentFailureReason,
) -> TelegramAttachmentFailure {
    TelegramAttachmentFailure {
        name: spec.name.clone(),
        reason,
    }
}

fn attachment_failure_notice(
    failures: &[TelegramAttachmentFailure],
    caption_was_blocked: bool,
) -> String {
    let mut lines = vec!["附件未成功交给 Agent：".to_string()];
    lines.extend(failures.iter().map(|failure| {
        let name = failure
            .name
            .chars()
            .filter(|ch| !ch.is_control())
            .take(120)
            .collect::<String>();
        format!("- {name}：{}", failure.reason.user_description())
    }));
    if caption_was_blocked {
        lines.push("为避免 Agent 在缺少附件时误处理，这条消息的说明文字也没有提交。".to_string());
    }
    lines.push("请修正后重新发送。".to_string());
    lines.join("\n")
}

async fn collect_telegram_attachments(
    api: &TelegramApi,
    attachment_root: &Path,
    message: &TelegramMessage,
) -> TelegramAttachmentCollection {
    let mut collection = TelegramAttachmentCollection::default();
    let specs = attachment_specs(message);
    if specs.is_empty() && message_has_media(message) {
        collection.failures.push(TelegramAttachmentFailure {
            name: "Telegram 附件".to_string(),
            reason: TelegramAttachmentFailureReason::MetadataUnavailable,
        });
        return collection;
    }
    for spec in specs {
        if spec
            .file_size
            .is_some_and(|size| size > TELEGRAM_MAX_FILE_BYTES)
        {
            chain_log::write_diagnostic_lazy(|| {
                format!(
                    "[telegram_attachment] event=download_skipped message={} file_id={} reason=size_limit size={}",
                    message.message_id,
                    spec.file_id,
                    spec.file_size.unwrap_or_default()
                )
            });
            collection.failures.push(attachment_failure(
                &spec,
                TelegramAttachmentFailureReason::TooLarge {
                    bytes: spec.file_size,
                },
            ));
            continue;
        }
        let file = match api.get_file(&spec.file_id).await {
            Ok(file) => file,
            Err(err) => {
                chain_log::write_diagnostic_lazy(|| {
                    format!(
                        "[telegram_attachment] event=get_file_failed message={} file_id={} err={}",
                        message.message_id, spec.file_id, err
                    )
                });
                collection.failures.push(attachment_failure(
                    &spec,
                    TelegramAttachmentFailureReason::MetadataUnavailable,
                ));
                continue;
            }
        };
        if file
            .file_size
            .is_some_and(|size| size > TELEGRAM_MAX_FILE_BYTES)
        {
            chain_log::write_diagnostic_lazy(|| {
                format!(
                    "[telegram_attachment] event=download_skipped message={} file_id={} reason=size_limit size={}",
                    message.message_id,
                    spec.file_id,
                    file.file_size.unwrap_or_default()
                )
            });
            collection.failures.push(attachment_failure(
                &spec,
                TelegramAttachmentFailureReason::TooLarge {
                    bytes: file.file_size,
                },
            ));
            continue;
        }
        let Some(file_path) = file.file_path.as_deref() else {
            chain_log::write_diagnostic_lazy(|| {
                format!(
                    "[telegram_attachment] event=get_file_missing_path message={} file_id={}",
                    message.message_id, spec.file_id
                )
            });
            collection.failures.push(attachment_failure(
                &spec,
                TelegramAttachmentFailureReason::MetadataUnavailable,
            ));
            continue;
        };
        let bytes = match api.download_file(file_path).await {
            Ok(bytes) if (bytes.len() as u64) <= TELEGRAM_MAX_FILE_BYTES => bytes,
            Ok(bytes) => {
                chain_log::write_diagnostic_lazy(|| {
                    format!(
                        "[telegram_attachment] event=download_skipped message={} file_id={} reason=size_limit size={}",
                        message.message_id,
                        spec.file_id,
                        bytes.len()
                    )
                });
                collection.failures.push(attachment_failure(
                    &spec,
                    TelegramAttachmentFailureReason::TooLarge {
                        bytes: Some(bytes.len() as u64),
                    },
                ));
                continue;
            }
            Err(err) => {
                chain_log::write_diagnostic_lazy(|| {
                    format!(
                        "[telegram_attachment] event=download_failed message={} file_id={} err={}",
                        message.message_id, spec.file_id, err
                    )
                });
                collection.failures.push(attachment_failure(
                    &spec,
                    TelegramAttachmentFailureReason::DownloadFailed,
                ));
                continue;
            }
        };
        let account_directory = sanitized_path_component(&api.settings().account_id(), "default");
        let dir = attachment_root
            .join("telegram")
            .join(account_directory)
            .join(spec.directory);
        if let Err(err) = tokio::fs::create_dir_all(&dir).await {
            chain_log::write_diagnostic_lazy(|| {
                format!(
                    "[telegram_attachment] event=directory_failed path={} err={}",
                    dir.display(),
                    err
                )
            });
            collection.failures.push(attachment_failure(
                &spec,
                TelegramAttachmentFailureReason::PersistFailed,
            ));
            continue;
        }
        let file_name = unique_attachment_name(&spec.name, &spec.file_id);
        let path = dir.join(file_name);
        if let Err(err) = tokio::fs::write(&path, &bytes).await {
            chain_log::write_diagnostic_lazy(|| {
                format!(
                    "[telegram_attachment] event=persist_failed path={} err={}",
                    path.display(),
                    err
                )
            });
            collection.failures.push(attachment_failure(
                &spec,
                TelegramAttachmentFailureReason::PersistFailed,
            ));
            continue;
        }
        collection
            .attachments
            .push(crate::types::InboundAttachment {
                kind: spec.kind.to_string(),
                name: Some(spec.name),
                mime_type: spec
                    .mime_type
                    .or_else(|| mime_guess::from_path(&path).first().map(|m| m.to_string())),
                text_hint: None,
                local_path: Some(path.to_string_lossy().to_string()),
            });
    }
    collection
}

fn unique_attachment_name(name: &str, file_id: &str) -> String {
    let sanitized = sanitized_path_component(name, "attachment.bin");
    let digest = sha2::Sha256::digest(file_id.as_bytes());
    let suffix = hex::encode(digest);
    if let Some((stem, extension)) = sanitized.rsplit_once('.') {
        let stem = stem.chars().take(120).collect::<String>();
        let extension = extension.chars().take(16).collect::<String>();
        format!("{stem}-{suffix}.{extension}")
    } else {
        let sanitized = sanitized.chars().take(120).collect::<String>();
        format!("{sanitized}-{suffix}")
    }
}

fn sanitized_path_component(value: &str, fallback: &str) -> String {
    let sanitized = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() || sanitized == "." || sanitized == ".." {
        fallback.to_string()
    } else {
        sanitized
    }
}

fn inbound_from_callback(
    settings: &TelegramSettings,
    allowed_chat_ids: &[String],
    callback: TelegramCallbackQuery,
) -> Option<InboundMessage> {
    let data = callback.data?;
    let action = action_from_callback_data(&data)?;
    let message = callback.message?;
    let is_private = message.chat.kind == "private";
    let raw_chat_id = message.chat.id.to_string();
    if is_private && !chat_allowed(allowed_chat_ids, &raw_chat_id) {
        return None;
    }
    if !is_private
        && (settings.project_group_for_chat(&raw_chat_id).is_none()
            || message.message_thread_id.is_none())
    {
        return None;
    }
    let chat_id = telegram_message_target(&raw_chat_id, message.message_thread_id);

    Some(InboundMessage {
        platform: ImPlatformKind::Telegram,
        account_id: settings.account_id(),
        sender_id: callback.from.id.to_string(),
        chat_id,
        chat_type: if is_private {
            ChatType::Direct
        } else {
            ChatType::Group
        },
        message_id: message.message_id.to_string(),
        received_at_ms: now_ms(),
        text: data,
        mentioned: is_private,
        approval_request_key: None,
        action: Some(action),
        card_message_id: Some(message.message_id.to_string()),
        callback_req_id: None,
        callback_kind: None,
        attachments: vec![],
    })
}

async fn set_polling_state(
    state: &SharedState,
    account_id: &str,
    polling: bool,
    connected: bool,
    last_error: Option<String>,
) {
    let now = now_ms();
    let mut telegram = state.telegram.lock().await;
    telegram.polling = polling;
    telegram.connected = connected;
    telegram.last_error = last_error.clone();
    telegram.last_event_at_ms = Some(now);
    let key = im_account_key(ImPlatformKind::Telegram, account_id);
    let mut accounts = state.im_accounts.lock().await;
    let entry = accounts
        .entry(key)
        .or_insert_with(|| ImAccountRuntimeState::new(ImPlatformKind::Telegram, account_id));
    entry.polling = polling;
    entry.connecting = false;
    entry.connected = connected;
    entry.last_error = last_error;
    entry.last_event_at_ms = Some(now);
}

async fn update_last_inbound(state: &SharedState, account_id: &str) {
    let mut telegram = state.telegram.lock().await;
    let now = now_ms();
    telegram.last_event_at_ms = Some(now);
    telegram.last_inbound_at_ms = Some(now);
    let key = im_account_key(ImPlatformKind::Telegram, account_id);
    let mut accounts = state.im_accounts.lock().await;
    let entry = accounts
        .entry(key)
        .or_insert_with(|| ImAccountRuntimeState::new(ImPlatformKind::Telegram, account_id));
    entry.last_event_at_ms = Some(now);
    entry.last_inbound_at_ms = Some(now);
}

#[derive(Debug, Clone)]
struct TelegramChatAccess {
    allowed_chat_ids: Vec<String>,
}

impl TelegramChatAccess {
    fn new(allowed_chat_ids: Vec<String>) -> Self {
        Self { allowed_chat_ids }
    }

    fn is_allowed(&self, chat_id: &str) -> bool {
        chat_allowed(&self.allowed_chat_ids, chat_id)
    }

    fn remember(&mut self, chat_id: &str) {
        if !self.is_allowed(chat_id) {
            self.allowed_chat_ids.push(chat_id.to_string());
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TelegramChatAccessDecision {
    Allowed,
    Denied,
    Ignored,
}

async fn ensure_message_chat_allowed(
    state: &SharedState,
    api: &TelegramApi,
    access: &mut TelegramChatAccess,
    message: &TelegramMessage,
) -> TelegramChatAccessDecision {
    ensure_chat_allowed(state, api, access, &message.chat).await
}

async fn ensure_callback_chat_allowed(
    state: &SharedState,
    api: &TelegramApi,
    access: &mut TelegramChatAccess,
    chat: Option<&super::api::TelegramChat>,
) -> TelegramChatAccessDecision {
    let Some(chat) = chat else {
        return TelegramChatAccessDecision::Ignored;
    };
    ensure_chat_allowed(state, api, access, chat).await
}

async fn ensure_chat_allowed(
    state: &SharedState,
    api: &TelegramApi,
    access: &mut TelegramChatAccess,
    chat: &super::api::TelegramChat,
) -> TelegramChatAccessDecision {
    let account_id = api.settings().account_id();
    let chat_id = chat.id.to_string();
    if chat.kind != "private" {
        return if api.settings().project_group_for_chat(&chat_id).is_some() {
            TelegramChatAccessDecision::Allowed
        } else {
            TelegramChatAccessDecision::Ignored
        };
    }
    if access.is_allowed(&chat_id) {
        return TelegramChatAccessDecision::Allowed;
    }
    if !access.allowed_chat_ids.is_empty() {
        log_denied_chat(state, &account_id, &chat_id).await;
        return TelegramChatAccessDecision::Denied;
    }

    let (bind_result, save_error) = {
        let mut config = state.config.lock().await;
        let result = config.ensure_telegram_allowed_chat_id(&account_id, &chat_id);
        let save_error = if result.should_save() {
            config
                .save(&state.config_path)
                .err()
                .map(|err| err.to_string())
        } else {
            None
        };
        (result, save_error)
    };
    if let Some(err) = save_error {
        state
            .push_event(
                "error",
                "telegram_chat_bind_failed",
                format!("account={account_id} chat={chat_id} err={err}"),
            )
            .await;
        return TelegramChatAccessDecision::Denied;
    }

    match bind_result {
        crate::config::TelegramChatAllowResult::Allowed
        | crate::config::TelegramChatAllowResult::Bound => {
            access.remember(&chat_id);
            if bind_result == crate::config::TelegramChatAllowResult::Bound {
                state
                    .push_event(
                        "info",
                        "telegram_chat_bound",
                        format!("account={account_id} chat={chat_id}"),
                    )
                    .await;
            }
            TelegramChatAccessDecision::Allowed
        }
        crate::config::TelegramChatAllowResult::Denied => {
            log_denied_chat(state, &account_id, &chat_id).await;
            TelegramChatAccessDecision::Denied
        }
        crate::config::TelegramChatAllowResult::AccountNotFound => {
            state
                .push_event(
                    "warn",
                    "telegram_chat_bind_account_missing",
                    format!("account={account_id} chat={chat_id}"),
                )
                .await;
            TelegramChatAccessDecision::Denied
        }
    }
}

async fn log_denied_chat(state: &SharedState, account_id: &str, chat_id: &str) {
    state
        .push_event(
            "warn",
            "telegram_chat_denied",
            format!("account={account_id} chat={chat_id}"),
        )
        .await;
}

fn chat_allowed(allowed_chat_ids: &[String], chat_id: &str) -> bool {
    allowed_chat_ids
        .iter()
        .any(|allowed| allowed.trim() == chat_id)
}

fn action_from_callback_data(data: &str) -> Option<InboundAction> {
    let parts = data.split(':').collect::<Vec<_>>();
    match parts.as_slice() {
        ["ap", request_fingerprint, option_index] => Some(InboundAction::ApprovalDecision {
            request_fingerprint: (*request_fingerprint).to_string(),
            option_index: option_index.parse().ok()?,
        }),
        ["trc", request_id, action] => Some(InboundAction::ThreadRouteChoice {
            request_id: (*request_id).to_string(),
            action: match *action {
                "new" => "create_new",
                "load" => "resume_history",
                "back" => "back",
                _ => return None,
            }
            .to_string(),
        }),
        ["trd", request_id] => Some(InboundAction::ThreadRouteCreateDefault {
            request_id: (*request_id).to_string(),
        }),
        ["tcc", request_id] => Some(InboundAction::ThreadRouteCreateConfigured {
            request_id: (*request_id).to_string(),
        }),
        ["tce", request_id, field] => Some(InboundAction::ThreadRouteCreateEdit {
            request_id: (*request_id).to_string(),
            field: (*field).to_string(),
        }),
        ["tcs", request_id, field, page, index] => Some(InboundAction::ThreadRouteCreateSetIndex {
            request_id: (*request_id).to_string(),
            field: (*field).to_string(),
            page: page.parse().ok()?,
            index: index.parse().ok()?,
        }),
        ["tcv", request_id, field, value] => Some(InboundAction::ThreadRouteCreateSetValue {
            request_id: (*request_id).to_string(),
            field: (*field).to_string(),
            value: (*value).to_string(),
        }),
        ["tcp", request_id, field, direction] => {
            Some(InboundAction::ThreadRouteCreateOptionsPage {
                request_id: (*request_id).to_string(),
                field: (*field).to_string(),
                direction: match *direction {
                    "prev" => ThreadRouteDirection::Prev,
                    "next" => ThreadRouteDirection::Next,
                    _ => return None,
                },
            })
        }
        ["trs", request_id, page, index] => Some(InboundAction::ThreadRouteResumeIndex {
            request_id: (*request_id).to_string(),
            page: page.parse().ok()?,
            index: index.parse().ok()?,
        }),
        ["tlp", request_id, direction] => Some(InboundAction::ThreadRouteListPage {
            request_id: (*request_id).to_string(),
            direction: match *direction {
                "prev" => ThreadRouteDirection::Prev,
                "next" => ThreadRouteDirection::Next,
                _ => return None,
            },
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::im::telegram::api::{
        TelegramChat, TelegramMediaFile, TelegramPhotoSize, TelegramStickerFile, TelegramUser,
    };

    #[test]
    fn converts_private_message_to_inbound() {
        let settings = TelegramSettings::default();
        let inbound = inbound_from_message(
            &settings,
            &["42".to_string()],
            &TelegramMessage {
                message_id: 9,
                from: Some(TelegramUser {
                    id: 42,
                    is_bot: false,
                    username: Some("ada".to_string()),
                    first_name: Some("Ada".to_string()),
                    last_name: None,
                }),
                chat: TelegramChat {
                    id: 42,
                    kind: "private".to_string(),
                    title: None,
                    username: Some("ada".to_string()),
                    first_name: Some("Ada".to_string()),
                    last_name: None,
                },
                text: Some("/status".to_string()),
                ..TelegramMessage::default()
            },
        )
        .expect("inbound message");

        assert_eq!(inbound.platform, ImPlatformKind::Telegram);
        assert_eq!(inbound.conversation_key(), "telegram:telegram:42");
        assert_eq!(inbound.chat_type, ChatType::Direct);
        assert_eq!(inbound.text, "/status");
    }

    #[test]
    fn accepts_photo_without_text_and_selects_largest_size() {
        let settings = TelegramSettings::default();
        let message = TelegramMessage {
            message_id: 12,
            chat: TelegramChat {
                id: 42,
                kind: "private".to_string(),
                ..TelegramChat::default()
            },
            photo: Some(vec![
                TelegramPhotoSize {
                    file_id: "wide".to_string(),
                    file_unique_id: "wide-unique".to_string(),
                    width: 1280,
                    height: 720,
                    file_size: Some(1_000),
                },
                TelegramPhotoSize {
                    file_id: "large".to_string(),
                    file_unique_id: "large-unique".to_string(),
                    width: 1024,
                    height: 1024,
                    file_size: Some(10_000),
                },
            ]),
            ..TelegramMessage::default()
        };

        let inbound = inbound_from_message(&settings, &["42".to_string()], &message)
            .expect("photo message should be accepted");
        let specs = attachment_specs(&message);

        assert!(inbound.text.is_empty());
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].file_id, "large");
        assert_eq!(specs[0].kind, "image");
    }

    #[test]
    fn treats_image_documents_as_visual_input() {
        let message = TelegramMessage {
            message_id: 14,
            chat: TelegramChat {
                id: 42,
                kind: "private".to_string(),
                ..TelegramChat::default()
            },
            document: Some(TelegramMediaFile {
                file_id: "image-document".to_string(),
                file_unique_id: "image-document-unique".to_string(),
                file_size: Some(8_192),
                file_name: Some("diagram.png".to_string()),
                mime_type: Some("image/png".to_string()),
            }),
            ..TelegramMessage::default()
        };

        let specs = attachment_specs(&message);

        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].kind, "image");
        assert_eq!(specs[0].directory, "images");
    }

    #[test]
    fn deduplicates_animation_compatibility_document_by_file_id() {
        let message = TelegramMessage {
            message_id: 15,
            chat: TelegramChat {
                id: 42,
                kind: "private".to_string(),
                ..TelegramChat::default()
            },
            animation: Some(TelegramMediaFile {
                file_id: "shared-animation-file".to_string(),
                file_unique_id: "shared-animation-unique".to_string(),
                file_size: Some(8_192),
                file_name: Some("demo.mp4".to_string()),
                mime_type: Some("video/mp4".to_string()),
            }),
            document: Some(TelegramMediaFile {
                file_id: "shared-animation-file".to_string(),
                file_unique_id: "shared-animation-unique".to_string(),
                file_size: Some(8_192),
                file_name: Some("compatibility.mp4".to_string()),
                mime_type: Some("video/mp4".to_string()),
            }),
            ..TelegramMessage::default()
        };

        let specs = attachment_specs(&message);

        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].file_id, "shared-animation-file");
        assert_eq!(specs[0].kind, "video");
        assert_eq!(specs[0].directory, "videos");
        assert_eq!(specs[0].name, "demo.mp4");
    }

    #[test]
    fn preserves_each_telegram_sticker_format() {
        let cases = [
            (false, false, "image", "images", ".webp", "image/webp"),
            (
                true,
                false,
                "file",
                "files",
                ".tgs",
                "application/x-tgsticker",
            ),
            (false, true, "video", "videos", ".webm", "video/webm"),
        ];

        for (is_animated, is_video, kind, directory, extension, mime_type) in cases {
            let message = TelegramMessage {
                message_id: 18,
                chat: TelegramChat {
                    id: 42,
                    kind: "private".to_string(),
                    ..TelegramChat::default()
                },
                sticker: Some(TelegramStickerFile {
                    file_id: format!("sticker-{extension}"),
                    file_unique_id: format!("sticker-unique-{extension}"),
                    file_size: Some(4_096),
                    is_animated,
                    is_video,
                }),
                ..TelegramMessage::default()
            };

            let specs = attachment_specs(&message);

            assert_eq!(specs.len(), 1);
            assert_eq!(specs[0].kind, kind);
            assert_eq!(specs[0].directory, directory);
            assert!(specs[0].name.ends_with(extension));
            assert_eq!(specs[0].mime_type.as_deref(), Some(mime_type));
        }
    }

    #[tokio::test]
    async fn oversized_attachment_returns_user_visible_failure_without_network() {
        let api = TelegramApi::new(TelegramSettings {
            account_id: "test".to_string(),
            bot_token: String::new(),
            allowed_chat_ids: vec!["42".to_string()],
            ..TelegramSettings::default()
        });
        let message = TelegramMessage {
            message_id: 16,
            chat: TelegramChat {
                id: 42,
                kind: "private".to_string(),
                ..TelegramChat::default()
            },
            caption: Some("review this archive".to_string()),
            document: Some(TelegramMediaFile {
                file_id: "oversized-file".to_string(),
                file_unique_id: "oversized-unique".to_string(),
                file_size: Some(TELEGRAM_MAX_FILE_BYTES + 1),
                file_name: Some("archive.zip".to_string()),
                mime_type: Some("application/zip".to_string()),
            }),
            ..TelegramMessage::default()
        };

        let collection =
            collect_telegram_attachments(&api, Path::new("/unused-test-root"), &message).await;
        let notice = attachment_failure_notice(&collection.failures, true);

        assert!(collection.attachments.is_empty());
        assert_eq!(collection.failures.len(), 1);
        assert_eq!(
            collection.failures[0].reason,
            TelegramAttachmentFailureReason::TooLarge {
                bytes: Some(TELEGRAM_MAX_FILE_BYTES + 1),
            }
        );
        assert!(notice.contains("archive.zip"));
        assert!(notice.contains("20 MB"));
        assert!(notice.contains("说明文字也没有提交"));
    }

    #[test]
    fn failure_notice_explains_metadata_download_and_persist_errors() {
        let failures = [
            TelegramAttachmentFailure {
                name: "metadata.bin".to_string(),
                reason: TelegramAttachmentFailureReason::MetadataUnavailable,
            },
            TelegramAttachmentFailure {
                name: "download.bin".to_string(),
                reason: TelegramAttachmentFailureReason::DownloadFailed,
            },
            TelegramAttachmentFailure {
                name: "persist.bin".to_string(),
                reason: TelegramAttachmentFailureReason::PersistFailed,
            },
        ];

        let notice = attachment_failure_notice(&failures, false);

        assert!(notice.contains("metadata.bin：无法读取 Telegram 文件信息"));
        assert!(notice.contains("download.bin：从 Telegram 下载失败"));
        assert!(notice.contains("persist.bin：下载后无法保存到本机"));
        assert!(!notice.contains("说明文字也没有提交"));
    }

    #[tokio::test]
    async fn malformed_media_returns_metadata_failure_without_network() {
        let api = TelegramApi::new(TelegramSettings::default());
        let message = TelegramMessage {
            message_id: 17,
            chat: TelegramChat {
                id: 42,
                kind: "private".to_string(),
                ..TelegramChat::default()
            },
            photo: Some(Vec::new()),
            ..TelegramMessage::default()
        };

        let collection =
            collect_telegram_attachments(&api, Path::new("/unused-test-root"), &message).await;

        assert!(collection.attachments.is_empty());
        assert_eq!(
            collection.failures,
            vec![TelegramAttachmentFailure {
                name: "Telegram 附件".to_string(),
                reason: TelegramAttachmentFailureReason::MetadataUnavailable,
            }]
        );
    }

    #[test]
    fn attachment_names_hash_the_complete_telegram_file_id() {
        let first = unique_attachment_name("report.pdf", "shared-prefix-file-a");
        let second = unique_attachment_name("report.pdf", "shared-prefix-file-b");

        assert_ne!(first, second);
        assert!(first.starts_with("report-"));
        assert!(first.ends_with(".pdf"));
        assert_eq!(first.len(), "report-".len() + 64 + ".pdf".len());
    }

    #[test]
    fn uses_document_caption_as_turn_text() {
        let settings = TelegramSettings::default();
        let message = TelegramMessage {
            message_id: 13,
            chat: TelegramChat {
                id: 42,
                kind: "private".to_string(),
                ..TelegramChat::default()
            },
            caption: Some("review this file".to_string()),
            document: Some(TelegramMediaFile {
                file_id: "document".to_string(),
                file_unique_id: "document-unique".to_string(),
                file_size: Some(4_096),
                file_name: Some("report.pdf".to_string()),
                mime_type: Some("application/pdf".to_string()),
            }),
            ..TelegramMessage::default()
        };

        let inbound = inbound_from_message(&settings, &["42".to_string()], &message)
            .expect("document message should be accepted");
        let specs = attachment_specs(&message);

        assert_eq!(inbound.text, "review this file");
        assert_eq!(specs[0].name, "report.pdf");
        assert_eq!(specs[0].kind, "file");
    }

    #[test]
    fn empty_allowed_chat_ids_do_not_pass_message_conversion() {
        let settings = TelegramSettings::default();
        let inbound = inbound_from_message(
            &settings,
            &[],
            &TelegramMessage {
                message_id: 9,
                from: Some(TelegramUser {
                    id: 42,
                    is_bot: false,
                    username: Some("ada".to_string()),
                    first_name: Some("Ada".to_string()),
                    last_name: None,
                }),
                chat: TelegramChat {
                    id: 42,
                    kind: "private".to_string(),
                    title: None,
                    username: Some("ada".to_string()),
                    first_name: Some("Ada".to_string()),
                    last_name: None,
                },
                text: Some("/status".to_string()),
                ..TelegramMessage::default()
            },
        );

        assert!(inbound.is_none());
    }

    #[test]
    fn rejects_private_message_from_unlisted_chat() {
        let settings = TelegramSettings::default();
        let inbound = inbound_from_message(
            &settings,
            &["99".to_string()],
            &TelegramMessage {
                message_id: 9,
                from: Some(TelegramUser {
                    id: 42,
                    is_bot: false,
                    username: Some("ada".to_string()),
                    first_name: Some("Ada".to_string()),
                    last_name: None,
                }),
                chat: TelegramChat {
                    id: 42,
                    kind: "private".to_string(),
                    title: None,
                    username: Some("ada".to_string()),
                    first_name: Some("Ada".to_string()),
                    last_name: None,
                },
                text: Some("/status".to_string()),
                ..TelegramMessage::default()
            },
        );

        assert!(inbound.is_none());
    }

    #[test]
    fn ignores_group_messages() {
        let settings = TelegramSettings {
            mention_only: true,
            ..TelegramSettings::default()
        };
        let message = TelegramMessage {
            message_id: 10,
            from: Some(TelegramUser {
                id: 42,
                is_bot: false,
                username: Some("ada".to_string()),
                first_name: Some("Ada".to_string()),
                last_name: None,
            }),
            chat: TelegramChat {
                id: -100,
                kind: "group".to_string(),
                title: Some("Codex".to_string()),
                username: None,
                first_name: None,
                last_name: None,
            },
            text: Some("hello".to_string()),
            ..TelegramMessage::default()
        };

        assert!(inbound_from_message(&settings, &["42".to_string()], &message).is_none());
    }

    #[test]
    fn ignores_group_messages_even_when_mentioned() {
        let settings = TelegramSettings {
            mention_only: true,
            ..TelegramSettings::default()
        };
        let inbound = inbound_from_message(
            &settings,
            &["42".to_string()],
            &TelegramMessage {
                message_id: 11,
                from: Some(TelegramUser {
                    id: 42,
                    is_bot: false,
                    username: Some("ada".to_string()),
                    first_name: Some("Ada".to_string()),
                    last_name: None,
                }),
                chat: TelegramChat {
                    id: -100,
                    kind: "group".to_string(),
                    title: Some("Codex".to_string()),
                    username: None,
                    first_name: None,
                    last_name: None,
                },
                text: Some("@codex_bot hello".to_string()),
                ..TelegramMessage::default()
            },
        );

        assert!(inbound.is_none());
    }

    #[test]
    fn routes_configured_forum_topic_as_its_own_conversation() {
        let settings = TelegramSettings {
            project_groups: vec![crate::config::TelegramProjectGroupConfig {
                chat_id: "-100".to_string(),
                project_name: "MochiPort".to_string(),
                cwd: "/tmp/mochiport".to_string(),
            }],
            ..TelegramSettings::default()
        };
        let message = TelegramMessage {
            message_id: 12,
            from: Some(TelegramUser {
                id: 42,
                is_bot: false,
                username: Some("ada".to_string()),
                first_name: Some("Ada".to_string()),
                last_name: None,
            }),
            chat: TelegramChat {
                id: -100,
                kind: "supergroup".to_string(),
                title: Some("MochiPort".to_string()),
                ..TelegramChat::default()
            },
            message_thread_id: Some(17),
            text: Some("修一下路由".to_string()),
            ..TelegramMessage::default()
        };

        let inbound = inbound_from_message(&settings, &[], &message).expect("topic inbound");
        assert_eq!(inbound.chat_type, ChatType::Group);
        assert_eq!(inbound.chat_id, "-100|topic=17");
        assert_eq!(
            inbound.conversation_key(),
            "telegram:telegram:-100|topic=17"
        );
    }

    #[test]
    fn forum_topic_name_uses_first_line_and_is_bounded() {
        assert_eq!(
            forum_topic_name("MochiPort", "修一下启动流程\n补充日志"),
            "修一下启动流程"
        );
        assert_eq!(forum_topic_name("MochiPort", "   "), "MochiPort");
        let long = "a".repeat(100);
        assert_eq!(forum_topic_name("MochiPort", &long).chars().count(), 64);
    }

    #[test]
    fn started_thread_metadata_accepts_nested_fields_and_fallback_keys() {
        let params = serde_json::json!({
            "threadId": "",
            "thread": {
                "id": "thread-1",
                "cwd": "/tmp/project",
                "name": null,
                "title": "会话标题"
            }
        });

        assert_eq!(started_thread_id(&params).as_deref(), Some("thread-1"));
        let thread = params.get("thread").expect("nested thread");
        assert_eq!(
            started_thread_field(&params, thread, &["name", "title"]).as_deref(),
            Some("会话标题")
        );
        assert_eq!(
            started_thread_field(&params, thread, &["cwd"]).as_deref(),
            Some("/tmp/project")
        );
    }

    #[test]
    fn cwd_matching_is_component_aware() {
        assert!(cwd_is_within_project("/tmp/project", "/tmp/project"));
        assert!(cwd_is_within_project("/tmp/project/src", "/tmp/project"));
        assert!(!cwd_is_within_project("/tmp/project-old", "/tmp/project"));
        assert!(!cwd_is_within_project("", "/tmp/project"));
    }

    #[tokio::test]
    async fn auto_topic_target_prefers_the_most_specific_project_group() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let mut config = crate::config::AppConfig::default();
        config.state_path = temp_dir.path().join("state.json");
        config.telegram_accounts = vec![crate::config::TelegramConfig {
            enabled: true,
            account_id: "telegram".to_string(),
            bot_token: "token".to_string(),
            project_groups: vec![
                crate::config::TelegramProjectGroupConfig {
                    chat_id: "-100-parent".to_string(),
                    project_name: "Parent".to_string(),
                    cwd: "/tmp/project".to_string(),
                },
                crate::config::TelegramProjectGroupConfig {
                    chat_id: "-100-child".to_string(),
                    project_name: "Child".to_string(),
                    cwd: "/tmp/project/frontend".to_string(),
                },
            ],
            ..Default::default()
        }];
        let state = crate::app_state::AppState::new(
            temp_dir.path().join("config.toml"),
            config,
            None,
            None,
        );
        let mut registry = ImApiRegistry::default();
        registry.telegram.insert(
            "telegram".to_string(),
            TelegramApi::new(TelegramSettings {
                account_id: "telegram".to_string(),
                bot_token: "token".to_string(),
                ..Default::default()
            }),
        );

        let target = find_auto_topic_target(&state, &registry, "/tmp/project/frontend/src")
            .await
            .expect("matching project group");
        assert_eq!(target.chat_id, "-100-child");
    }

    #[tokio::test]
    async fn auto_topic_creation_stops_when_bridge_generation_is_invalidated() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let mut config = crate::config::AppConfig::default();
        config.state_path = temp_dir.path().join("state.json");
        let state = crate::app_state::AppState::new(
            temp_dir.path().join("config.toml"),
            config,
            None,
            None,
        );
        let generation = state.runtime.lock().await.start_bridge_generation();
        state.runtime.lock().await.invalidate_bridge_generation();

        auto_create_topic_for_codex_thread_for_generation(
            &state,
            &ImApiRegistry::default(),
            "default:codex_app",
            serde_json::json!({
                "threadId": "thread-stale",
                "cwd": "/tmp/project",
                "name": "stale session"
            }),
            generation,
            None,
        )
        .await;

        assert!(state.events.lock().await.is_empty());
    }

    #[test]
    fn parses_thread_route_callback_data() {
        let action = action_from_callback_data("trs:thread-route-7:2:3").expect("resume action");
        match action {
            InboundAction::ThreadRouteResumeIndex {
                request_id,
                page,
                index,
            } => {
                assert_eq!(request_id, "thread-route-7");
                assert_eq!(page, 2);
                assert_eq!(index, 3);
            }
            other => panic!("unexpected action: {other:?}"),
        }

        let action = action_from_callback_data("ap:abc123:2").expect("approval action");
        match action {
            InboundAction::ApprovalDecision {
                request_fingerprint,
                option_index,
            } => {
                assert_eq!(request_fingerprint, "abc123");
                assert_eq!(option_index, 2);
            }
            other => panic!("unexpected action: {other:?}"),
        }

        let action = action_from_callback_data("tlp:thread-route-7:next").expect("page action");
        match action {
            InboundAction::ThreadRouteListPage {
                request_id,
                direction,
            } => {
                assert_eq!(request_id, "thread-route-7");
                assert_eq!(direction, ThreadRouteDirection::Next);
            }
            other => panic!("unexpected action: {other:?}"),
        }
    }
}
