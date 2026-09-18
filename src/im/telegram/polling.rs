use anyhow::{Result, anyhow};
use sha2::Digest;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use serde_json::Value;
use tokio::sync::mpsc;
use tokio::time::{Duration, sleep};

use crate::{
    app_state::{
        ImAccountRuntimeState, SharedState, TelegramThreadLifecycleState,
        TelegramTopicCleanupRegistration, im_account_key,
    },
    chain_log,
    im::core::{
        accounts::ImApiRegistry, i18n::im_text_for_state,
        session::bind_thread_to_route_for_generation, thread::summarize_thread_title,
    },
    im::runtime::{RouteTarget, route_from_conversation_key},
    remote_control_backend,
    types::{
        ChatType, ImPlatformKind, InboundAction, InboundMessage, ThreadRouteDirection,
        ThreadSettingsField, now_ms, telegram_message_target,
    },
};

use super::{
    api::{
        TelegramApi, TelegramApiError, TelegramBotCommand, TelegramCallbackQuery,
        TelegramForumTopicEditOutcome, TelegramMessage,
    },
    types::TelegramSettings,
};

mod auto_topic;
#[cfg(test)]
mod tests;
mod topic_cleanup;
mod topic_reconcile;

use auto_topic::{
    delete_auto_created_topic, existing_binding_for_thread, find_auto_topic_target,
    is_current_bridge_generation, keep_auto_created_topic_if_current, persist_auto_topic_name,
    resume_auto_topic_thread, started_thread_id, started_thread_metadata,
    telegram_topic_creation_is_current,
};
use topic_cleanup::{
    TelegramTopicMutationDeadlineGate, delete_forum_topic_with_retry_while,
    remove_telegram_topic_for_codex_thread, telegram_topic_mutation_cooldown,
    wait_for_telegram_topic_mutation_deadline,
};
use topic_reconcile::{
    TelegramTopicLifecycle, apply_binding_lifecycle,
    commit_telegram_topic_name_to_codex_if_current, consume_telegram_topic_name_marker,
    persist_telegram_topic_service_state, reconcile_telegram_topic_bindings,
    record_telegram_topic_name_for_codex_sync,
};

const TELEGRAM_LONG_POLL_TIMEOUT_SECONDS: u32 = 25;
const TELEGRAM_STARTUP_PROBE_RETRY_SECONDS: u64 = 5;
const TELEGRAM_CONFLICT_BACKOFF_SECONDS: u64 = 35;
const TELEGRAM_GENERIC_RETRY_SECONDS: u64 = 5;
const TELEGRAM_RETRY_JITTER_FRACTION: f64 = 0.2;
const TELEGRAM_TOPIC_RECONCILIATION_INTERVAL: Duration = Duration::from_secs(300);
const TELEGRAM_TOPIC_STATE_GRACE: Duration = Duration::from_secs(300);
const TELEGRAM_TOPIC_DELETE_MAX_ATTEMPTS: usize = 6;
const TELEGRAM_TOPIC_DELETE_RETRY_SECONDS: u64 = 5;
const TELEGRAM_TOPIC_NAME_MARKER_TTL: Duration = Duration::from_secs(120);
const AUTO_TOPIC_SESSION_PAGE_LIMIT: u32 = 100;
const AUTO_TOPIC_SESSION_MAX_PAGES: usize = 20;
const AUTO_TOPIC_RESUME_MAX_ATTEMPTS: usize = 20;
const AUTO_TOPIC_RESUME_RETRY_DELAY: Duration = Duration::from_millis(500);

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
    if !state
        .observe_telegram_thread_started(&thread_id, generation)
        .await
        || !telegram_topic_creation_is_current(state, &thread_id, generation).await
    {
        return;
    }
    let remote_client_key = if remote_client_key.trim().is_empty() {
        remote_control_backend::default_remote_client_key().to_string()
    } else {
        remote_client_key.trim().to_string()
    };
    let Some((thread_cwd, thread_title, rollout_path)) = started_thread_metadata(
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
    if !telegram_topic_creation_is_current(state, &thread_id, generation).await {
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
    let sync_gate = state.telegram_topic_sync_gate(&target.account_id).await;
    let _sync_guard = sync_gate.lock().await;
    let creation_gate = state.telegram_topic_creation_gate(&thread_id).await;
    let _creation_guard = creation_gate.lock().await;
    if !telegram_topic_creation_is_current(state, &thread_id, generation).await {
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
    let create_result = run_telegram_topic_mutation_while(
        state,
        &target.account_id,
        None,
        || async {
            telegram_topic_creation_is_current(state, &thread_id, generation).await
                && existing_binding_for_thread(state, &thread_id)
                    .await
                    .is_none()
        },
        || target.api.create_forum_topic(&target.chat_id, &topic_name),
    )
    .await;
    let Some(create_result) = create_result else {
        return;
    };
    let topic = match create_result {
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

    // The bridge or thread lifecycle can change while Telegram is creating the
    // Topic. The returned Topic is owned by this worker until this check passes.
    let cleanup_state = state.clone();
    let cleanup_api = target.api.clone();
    let cleanup_chat_id = target.chat_id.clone();
    let cleanup_thread_id = thread_id.clone();
    let Some(topic) = keep_auto_created_topic_if_current(
        state,
        &thread_id,
        generation,
        topic,
        move |topic| async move {
            delete_auto_created_topic(
                &cleanup_state,
                &cleanup_api,
                &cleanup_chat_id,
                topic.message_thread_id,
                &cleanup_thread_id,
            )
            .await;
        },
    )
    .await
    else {
        return;
    };

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

    if !telegram_topic_creation_is_current(state, &thread_id, generation).await {
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

    let resume_result = resume_auto_topic_thread(
        state,
        &route.remote_client_key,
        &thread_id,
        rollout_path.as_deref(),
        generation,
        connection_epoch,
    )
    .await;
    if let Ok(response) = resume_result.as_ref() {
        state.runtime.lock().await.observe_thread_settings(
            &thread_id,
            crate::im::runtime::ThreadSettingsSnapshot::from_protocol_value(response),
        );
    }
    if !telegram_topic_creation_is_current(state, &thread_id, generation).await {
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

    if let Err(err) = bind_thread_to_route_for_generation(
        state,
        &route,
        &thread_id,
        None,
        route.remote_client_key.clone(),
        Some(generation),
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

    if !telegram_topic_creation_is_current(state, &thread_id, generation).await {
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
    if !telegram_topic_creation_is_current(state, &thread_id, generation).await {
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

/// Run one Telegram Topic mutation without holding the per-account gate while
/// waiting for a Retry-After deadline. `None` means the caller's lifecycle or
/// operation token became stale before the API request started.
pub(crate) async fn run_telegram_topic_mutation_while<T, C, ContinueFuture, M, MutationFuture>(
    state: &SharedState,
    account_id: &str,
    notifier: Option<&tokio::sync::Notify>,
    mut should_continue: C,
    mutation: M,
) -> Option<Result<T>>
where
    C: FnMut() -> ContinueFuture,
    ContinueFuture: std::future::Future<Output = bool>,
    M: FnOnce() -> MutationFuture,
    MutationFuture: std::future::Future<Output = Result<T>>,
{
    let mut mutation = Some(mutation);
    loop {
        if !should_continue().await {
            return None;
        }
        if wait_for_telegram_topic_mutation_deadline(state, account_id, notifier).await
            == TelegramTopicMutationDeadlineGate::RecheckLifecycle
        {
            continue;
        }
        if !should_continue().await {
            return None;
        }

        let gate = state.telegram_topic_mutation_gate(account_id).await;
        let mutation_guard = if let Some(notifier) = notifier {
            tokio::select! {
                guard = gate.lock() => guard,
                _ = notifier.notified() => continue,
            }
        } else {
            gate.lock().await
        };

        // Another mutation may have received a newer Retry-After while this
        // caller was queued for the gate. Release it before waiting.
        if let Some(deadline) = state
            .telegram_topic_cleanup_retry_deadline(account_id)
            .await
        {
            if deadline > Instant::now() {
                drop(mutation_guard);
                continue;
            }
            state
                .clear_telegram_topic_cleanup_retry_deadline_if_elapsed(account_id, deadline)
                .await;
        }
        if !should_continue().await {
            return None;
        }

        let result = mutation
            .take()
            .expect("Telegram Topic mutation executes at most once")()
        .await;
        if let Err(err) = &result
            && let Some(delay) = telegram_topic_mutation_cooldown(err)
        {
            // Record the shared deadline before releasing the mutation gate so
            // the next waiter cannot slip through after a 429 response.
            state
                .extend_telegram_topic_cleanup_retry_deadline(account_id, delay)
                .await;
        }
        drop(mutation_guard);
        return Some(result);
    }
}

pub(crate) async fn run_telegram_topic_mutation<T, M, MutationFuture>(
    state: &SharedState,
    account_id: &str,
    mutation: M,
) -> Result<T>
where
    M: FnOnce() -> MutationFuture,
    MutationFuture: std::future::Future<Output = Result<T>>,
{
    run_telegram_topic_mutation_while(
        state,
        account_id,
        None,
        || std::future::ready(true),
        mutation,
    )
    .await
    .expect("unconditional Telegram Topic mutation cannot be cancelled")
}

/// Delete a bound Topic as soon as Codex reports that its thread was archived
/// or deleted. The periodic reconciliation path calls the same scheduler when
/// the client does not emit a lifecycle notification.
pub(crate) async fn archive_telegram_topic_for_codex_thread(
    state: &SharedState,
    api_registry: &ImApiRegistry,
    thread_id: &str,
    generation: u64,
) {
    remove_telegram_topic_for_codex_thread(
        state,
        api_registry,
        thread_id,
        generation,
        TelegramTopicLifecycle::Archived,
        "codex_thread_archived_notification",
    )
    .await;
}

pub(crate) async fn delete_telegram_topic_for_codex_thread(
    state: &SharedState,
    api_registry: &ImApiRegistry,
    thread_id: &str,
    generation: u64,
) {
    remove_telegram_topic_for_codex_thread(
        state,
        api_registry,
        thread_id,
        generation,
        TelegramTopicLifecycle::Deleted,
        "codex_thread_deleted_notification",
    )
    .await;
}

/// Cancel a queued archive cleanup if Codex restores the thread before the
/// Telegram deletion request succeeds.
pub(crate) async fn unarchive_telegram_topic_for_codex_thread(
    state: &SharedState,
    thread_id: &str,
    generation: u64,
) {
    let path = state.config.lock().await.state_path.clone();
    let _binding_guard = state.im_route_binding_ops.lock().await;
    let runtime = state.runtime.lock().await;
    if runtime.bridge_generation != generation {
        return;
    }
    if !state
        .observe_telegram_thread_lifecycle(
            thread_id,
            generation,
            TelegramThreadLifecycleState::Active,
        )
        .await
    {
        return;
    }
    let mut persisted = state.persisted.lock().await;
    let keys = persisted
        .im_thread_bindings
        .iter()
        .filter(|(_, bound_thread_id)| bound_thread_id.as_str() == thread_id)
        .filter_map(|(key, _)| {
            route_from_conversation_key(key)
                .filter(|route| route.platform == ImPlatformKind::Telegram)
                .map(|_| key.clone())
        })
        .collect::<Vec<_>>();
    let mut changed_keys = Vec::new();
    for key in keys {
        if let Some(binding) = persisted.telegram_topic_binding_states.get_mut(&key) {
            if binding.lifecycle_generation > generation
                || (binding.codex_state == "deleted" && binding.lifecycle_generation == generation)
            {
                continue;
            }
            apply_binding_lifecycle(
                binding,
                TelegramTopicLifecycle::Active,
                generation,
                now_ms(),
            );
            binding.lifecycle_revision = binding.lifecycle_revision.saturating_add(1);
            changed_keys.push((
                key,
                binding.lifecycle_generation,
                binding.lifecycle_revision,
            ));
        }
    }
    drop(runtime);
    if !changed_keys.is_empty()
        && let Err(err) = persisted.save(&path)
    {
        chain_log::write_diagnostic_lazy(|| {
            format!(
                "[telegram_topic] event=unarchive_save_failed thread={} err={err}",
                thread_id
            )
        });
    }
    drop(persisted);
    drop(_binding_guard);

    for (key, lifecycle_generation, lifecycle_revision) in changed_keys {
        state
            .notify_telegram_topic_cleanup_if_older(&key, lifecycle_generation, lifecycle_revision)
            .await;
    }
}

pub(crate) async fn delete_forum_topic_with_retry(
    state: &SharedState,
    api: &TelegramApi,
    chat_id: &str,
    topic_id: i64,
    context: &str,
) -> Result<()> {
    let account_id = api.settings().account_id();
    delete_forum_topic_with_retry_while(
        state,
        api,
        chat_id,
        topic_id,
        context,
        None,
        &account_id,
        || std::future::ready(true),
    )
    .await?;
    Ok(())
}

pub async fn listen_polling(
    state: SharedState,
    api: TelegramApi,
    api_registry: ImApiRegistry,
    attachment_root: PathBuf,
    tx: mpsc::Sender<InboundMessage>,
) -> Result<()> {
    let account_id = api.settings().account_id();
    let mut chat_access = TelegramChatAccess::new(api.settings().allowed_chat_ids.clone());
    let mut pairing_attempts = PairingAttempts::default();
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
                    &mut pairing_attempts,
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
                        TelegramChatAccessDecision::Denied
                        | TelegramChatAccessDecision::DeniedWith(_) => "当前聊天未授权",
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
                let decision = ensure_message_chat_allowed(
                    &state,
                    &api,
                    &mut chat_access,
                    &mut pairing_attempts,
                    &message,
                )
                .await;
                match decision {
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
                                    &state,
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
                    TelegramChatAccessDecision::Denied
                    | TelegramChatAccessDecision::DeniedWith(_) => {
                        let chat_id = message.chat.id.to_string();
                        let pairing_mode = {
                            let config = state.config.lock().await;
                            config
                                .telegram_pairing_code(&account_id)
                                .is_some_and(|code| !code.is_empty())
                        };
                        let text = match decision {
                            TelegramChatAccessDecision::DeniedWith(text) => text,
                            _ if pairing_mode => PAIRING_HINT_TEXT,
                            _ => {
                                "当前 Telegram 私聊未授权。请在本机 MochiPort 配置 allowedChatIds。"
                            }
                        };
                        let _ = api.send_text(&chat_id, text).await;
                    }
                    TelegramChatAccessDecision::Ignored | TelegramChatAccessDecision::Consumed => {}
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
            reconcile_telegram_topic_bindings(&state, &api, &api_registry).await;
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
    } else if message.forum_topic_reopened.is_some()
        || message.forum_topic_created.is_some()
        || message.forum_topic_edited.is_some()
    {
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
        consume_telegram_topic_name_marker(state, &key, name).await
    } else {
        false
    };
    if topic_name_was_expected {
        // The API writer commits the name and freshness metadata with its own
        // token/CAS. Touching the binding here can make that commit look stale
        // when Telegram delivers the service update before the HTTP response.
        return true;
    }
    let Some(topic_name) = topic_name.map(|name| truncate_topic_name(&name)) else {
        persist_telegram_topic_service_state(state, &key, topic_state).await;
        return true;
    };
    let operation_token = state.begin_telegram_topic_name_update(&key).await;
    let Some(thread_id) = record_telegram_topic_name_for_codex_sync(
        state,
        &key,
        topic_state,
        &topic_name,
        operation_token,
    )
    .await
    else {
        state
            .finish_telegram_topic_name_update(&key, operation_token)
            .await;
        return true;
    };

    let remote_client_key = state
        .runtime
        .lock()
        .await
        .route_for_thread(&thread_id)
        .map(|route| route.remote_client_key)
        .or_else(|| {
            crate::im::runtime::route_from_conversation_key(&key)
                .map(|route| route.remote_client_key)
        })
        .unwrap_or_default();
    match crate::remote_control_backend::set_thread_name_for_client(
        state,
        &remote_client_key,
        &thread_id,
        &topic_name,
    )
    .await
    {
        Ok(()) => {
            if commit_telegram_topic_name_to_codex_if_current(
                state,
                &key,
                &thread_id,
                &topic_name,
                operation_token,
            )
            .await
            {
                state
                    .push_event(
                        "info",
                        "telegram_topic_name_synced_to_codex",
                        format!("thread={} name={}", thread_id, topic_name),
                    )
                    .await;
            }
        }
        Err(err) => {
            state
                .push_event(
                    "warn",
                    "telegram_topic_name_sync_to_codex_failed",
                    format!("thread={} err={err}", thread_id),
                )
                .await;
        }
    }
    state
        .finish_telegram_topic_name_update(&key, operation_token)
        .await;
    true
}

pub(crate) async fn install_telegram_topic_name_marker(
    state: &SharedState,
    conversation_key: &str,
    topic_name: &str,
) -> u64 {
    let token = state.next_telegram_topic_name_sync_token();
    state
        .telegram_topic_name_sync_ops
        .lock()
        .await
        .entry(conversation_key.to_string())
        .or_default()
        .push_back(crate::app_state::TelegramTopicNameSyncMarker {
            name: topic_name.to_string(),
            token,
        });

    let marker_state = state.clone();
    let marker_key = conversation_key.to_string();
    tokio::spawn(async move {
        tokio::time::sleep(TELEGRAM_TOPIC_NAME_MARKER_TTL).await;
        clear_telegram_topic_name_marker(&marker_state, &marker_key, token).await;
    });
    token
}

pub(crate) async fn clear_telegram_topic_name_marker(
    state: &SharedState,
    conversation_key: &str,
    token: u64,
) {
    let mut operations = state.telegram_topic_name_sync_ops.lock().await;
    let remove_key = operations.get_mut(conversation_key).is_some_and(|markers| {
        if let Some(index) = markers.iter().position(|marker| marker.token == token) {
            markers.remove(index);
        }
        markers.is_empty()
    });
    if remove_key {
        operations.remove(conversation_key);
    }
}

pub(crate) async fn run_telegram_topic_edit_with_marker<M, MutationFuture>(
    state: &SharedState,
    conversation_key: &str,
    topic_name: &str,
    mutation: M,
) -> Result<bool>
where
    M: FnOnce() -> MutationFuture,
    MutationFuture: std::future::Future<Output = Result<TelegramForumTopicEditOutcome>>,
{
    let marker_token =
        install_telegram_topic_name_marker(state, conversation_key, topic_name).await;
    match mutation().await {
        Ok(TelegramForumTopicEditOutcome::Changed) => Ok(true),
        Ok(TelegramForumTopicEditOutcome::NotModified) => {
            clear_telegram_topic_name_marker(state, conversation_key, marker_token).await;
            Ok(true)
        }
        Ok(TelegramForumTopicEditOutcome::Rejected) => {
            clear_telegram_topic_name_marker(state, conversation_key, marker_token).await;
            Ok(false)
        }
        Err(err) => {
            clear_telegram_topic_name_marker(state, conversation_key, marker_token).await;
            Err(err)
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
    state: &SharedState,
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
    let account_id = settings.account_id();
    let topic = match run_telegram_topic_mutation(state, &account_id, || {
        api.create_forum_topic(&raw_chat_id, &topic_name)
    })
    .await
    {
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
                let (delay, jitterable) =
                    retry_delay_seconds(&err, TELEGRAM_STARTUP_PROBE_RETRY_SECONDS);
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
                sleep_retry_delay(delay, jitterable).await;
            }
        }
    }
}

async fn handle_polling_error(state: &SharedState, account_id: &str, err: &anyhow::Error) {
    let (delay, jitterable) = retry_delay_seconds(err, TELEGRAM_GENERIC_RETRY_SECONDS);
    let kind = err
        .downcast_ref::<TelegramApiError>()
        .filter(|api_error| api_error.is_conflict())
        .map(|_| "telegram_poll_conflict")
        .unwrap_or("telegram_poll_failed");
    set_polling_state(state, account_id, true, false, Some(err.to_string())).await;
    state
        .push_event("warn", kind, format!("retry_in={delay}s err={err}"))
        .await;
    sleep_retry_delay(delay, jitterable).await;
}

fn retry_delay_seconds(err: &anyhow::Error, default_delay: u64) -> (u64, bool) {
    if let Some(api_error) = err.downcast_ref::<TelegramApiError>() {
        if api_error.is_conflict() {
            return (TELEGRAM_CONFLICT_BACKOFF_SECONDS, true);
        }
        if let Some(retry_after) = api_error.retry_after {
            // Server-mandated wait; jitter must not shorten it.
            return (retry_after.max(1), false);
        }
    }
    (default_delay, true)
}

async fn sleep_retry_delay(seconds: u64, jitterable: bool) {
    let duration = if jitterable {
        crate::timing::jittered(Duration::from_secs(seconds), TELEGRAM_RETRY_JITTER_FRACTION)
    } else {
        Duration::from_secs(seconds)
    };
    sleep(duration).await;
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
    /// 拒绝并回复定制文案（覆盖 Denied 分支的默认文案）。
    DeniedWith(&'static str),
    Ignored,
    /// 配对消息已被消费（完成绑定），不再进入会话流。
    Consumed,
}

const PAIRING_HINT_TEXT: &str = "该私聊尚未绑定。请在 MochiPort「聊天工具接入」页查看配对码，发送 /start <配对码> 或直接发送配对码完成绑定。";
const PAIRING_LOCKOUT_TEXT: &str = "配对码尝试次数过多，请约 10 分钟后再试。";
const PAIRING_SUCCESS_TEXT: &str = "绑定成功！发送 /help 查看可用命令，或直接描述你的任务。";
const PAIRING_MAX_FAILURES: u32 = 5;
const PAIRING_COOLDOWN_MS: i64 = 10 * 60 * 1000;

/// 未绑定私聊的配对码失败记录，只在单次 polling 生命周期内生效。
#[derive(Debug, Default)]
struct PairingAttempts {
    failures: HashMap<String, u32>,
    locked_until_ms: HashMap<String, i64>,
}

impl PairingAttempts {
    fn is_locked(&self, chat_id: &str, now_ms: i64) -> bool {
        self.locked_until_ms
            .get(chat_id)
            .is_some_and(|until| *until > now_ms)
    }

    /// 记一次失败；达到上限时进入冷却并清零计数，返回是否刚刚触发冷却。
    fn record_failure(&mut self, chat_id: &str, now_ms: i64) -> bool {
        let failures = self
            .failures
            .entry(chat_id.to_string())
            .and_modify(|count| *count += 1)
            .or_insert(1);
        if *failures < PAIRING_MAX_FAILURES {
            return false;
        }
        self.failures.remove(chat_id);
        self.locked_until_ms
            .insert(chat_id.to_string(), now_ms + PAIRING_COOLDOWN_MS);
        true
    }

    fn record_success(&mut self, chat_id: &str) {
        self.failures.remove(chat_id);
        self.locked_until_ms.remove(chat_id);
    }
}

/// 文本等于配对码，或为 `/start <配对码>`（兼容 /start@botname 深链 payload）。
fn pairing_text_matches(message_text: Option<&str>, pairing_code: &str) -> bool {
    let Some(text) = message_text else {
        return false;
    };
    let text = text.trim();
    if text == pairing_code {
        return true;
    }
    let Some(rest) = text.strip_prefix('/') else {
        return false;
    };
    let mut parts = rest.splitn(2, char::is_whitespace);
    let command = parts.next().unwrap_or("").split('@').next().unwrap_or("");
    let payload = parts.next().unwrap_or("").trim();
    command.eq_ignore_ascii_case("start") && payload == pairing_code
}

async fn ensure_message_chat_allowed(
    state: &SharedState,
    api: &TelegramApi,
    access: &mut TelegramChatAccess,
    attempts: &mut PairingAttempts,
    message: &TelegramMessage,
) -> TelegramChatAccessDecision {
    ensure_chat_allowed(
        state,
        api,
        access,
        &message.chat,
        message.text.as_deref(),
        attempts,
    )
    .await
}

async fn ensure_callback_chat_allowed(
    state: &SharedState,
    api: &TelegramApi,
    access: &mut TelegramChatAccess,
    attempts: &mut PairingAttempts,
    chat: Option<&super::api::TelegramChat>,
) -> TelegramChatAccessDecision {
    let Some(chat) = chat else {
        return TelegramChatAccessDecision::Ignored;
    };
    ensure_chat_allowed(state, api, access, chat, None, attempts).await
}

async fn ensure_chat_allowed(
    state: &SharedState,
    api: &TelegramApi,
    access: &mut TelegramChatAccess,
    chat: &super::api::TelegramChat,
    message_text: Option<&str>,
    attempts: &mut PairingAttempts,
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
    let pairing_code = {
        let config = state.config.lock().await;
        config
            .telegram_pairing_code(&account_id)
            .unwrap_or_default()
    };
    if !pairing_code.is_empty() {
        return ensure_pairing(
            state,
            api,
            access,
            attempts,
            &account_id,
            &chat_id,
            &pairing_code,
            message_text,
        )
        .await;
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

/// 配对码模式的绑定闸门：校验通过才写入白名单，配对消息本身被吞掉不进会话流。
async fn ensure_pairing(
    state: &SharedState,
    api: &TelegramApi,
    access: &mut TelegramChatAccess,
    attempts: &mut PairingAttempts,
    account_id: &str,
    chat_id: &str,
    pairing_code: &str,
    message_text: Option<&str>,
) -> TelegramChatAccessDecision {
    let Some(message_text) = message_text else {
        return TelegramChatAccessDecision::DeniedWith(PAIRING_HINT_TEXT);
    };
    let now = now_ms() as i64;
    if attempts.is_locked(chat_id, now) {
        return TelegramChatAccessDecision::Ignored;
    }
    if !pairing_text_matches(Some(message_text), pairing_code) {
        let just_locked = attempts.record_failure(chat_id, now);
        if just_locked {
            state
                .push_event(
                    "warn",
                    "telegram_pairing_failed",
                    format!("account={account_id} chat={chat_id} locked"),
                )
                .await;
            return TelegramChatAccessDecision::DeniedWith(PAIRING_LOCKOUT_TEXT);
        }
        state
            .push_event(
                "warn",
                "telegram_pairing_failed",
                format!("account={account_id} chat={chat_id}"),
            )
            .await;
        return TelegramChatAccessDecision::DeniedWith(PAIRING_HINT_TEXT);
    }

    let (bind_result, save_error) = {
        let mut config = state.config.lock().await;
        let result = config.add_telegram_allowed_chat_id(account_id, chat_id);
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
            access.remember(chat_id);
            attempts.record_success(chat_id);
            if bind_result == crate::config::TelegramChatAllowResult::Bound {
                state
                    .push_event(
                        "info",
                        "telegram_chat_bound",
                        format!("account={account_id} chat={chat_id}"),
                    )
                    .await;
            }
            let _ = api.send_text(chat_id, PAIRING_SUCCESS_TEXT).await;
            TelegramChatAccessDecision::Consumed
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
        crate::config::TelegramChatAllowResult::Denied => {
            log_denied_chat(state, account_id, chat_id).await;
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
        ["tmo", request_id, revision, field] => Some(InboundAction::ThreadSettingsOpenField {
            request_id: (*request_id).to_string(),
            revision: revision.parse().ok()?,
            field: match *field {
                "model" => ThreadSettingsField::Model,
                "effort" => ThreadSettingsField::Effort,
                "speed" => ThreadSettingsField::Speed,
                _ => return None,
            },
        }),
        ["tmp", request_id, revision, direction] => Some(InboundAction::ThreadSettingsModelPage {
            request_id: (*request_id).to_string(),
            revision: revision.parse().ok()?,
            direction: match *direction {
                "prev" => ThreadRouteDirection::Prev,
                "next" => ThreadRouteDirection::Next,
                _ => return None,
            },
        }),
        ["tms", request_id, revision, page, index] => {
            Some(InboundAction::ThreadSettingsChooseModel {
                request_id: (*request_id).to_string(),
                revision: revision.parse().ok()?,
                page: page.parse().ok()?,
                index: index.parse().ok()?,
            })
        }
        ["tme", request_id, revision, index] => Some(InboundAction::ThreadSettingsChooseEffort {
            request_id: (*request_id).to_string(),
            revision: revision.parse().ok()?,
            index: index.parse().ok()?,
        }),
        ["tmv", request_id, revision, value] => Some(InboundAction::ThreadSettingsChooseSpeed {
            request_id: (*request_id).to_string(),
            revision: revision.parse().ok()?,
            fast: match *value {
                "std" => false,
                "fast" => true,
                _ => return None,
            },
        }),
        ["tmb", request_id, revision] => Some(InboundAction::ThreadSettingsBack {
            request_id: (*request_id).to_string(),
            revision: revision.parse().ok()?,
        }),
        ["tma", request_id, revision] => Some(InboundAction::ThreadSettingsApply {
            request_id: (*request_id).to_string(),
            revision: revision.parse().ok()?,
        }),
        ["tmq", request_id, revision, decision] => {
            Some(InboundAction::ThreadSettingsCompatibilityConfirm {
                request_id: (*request_id).to_string(),
                revision: revision.parse().ok()?,
                accept: match *decision {
                    "yes" => true,
                    "no" => false,
                    _ => return None,
                },
            })
        }
        ["tmc", request_id, revision] => Some(InboundAction::ThreadSettingsCancel {
            request_id: (*request_id).to_string(),
            revision: revision.parse().ok()?,
        }),
        _ => None,
    }
}
