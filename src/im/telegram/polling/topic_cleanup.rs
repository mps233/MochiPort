//! Telegram Topic deletion, cleanup scheduling and delete retry helpers.
//!
//! Moved verbatim out of `polling.rs`; no behavior changes.

use super::auto_topic::is_current_bridge_generation;
use super::topic_reconcile::{
    TelegramTopicBindingCommit, TelegramTopicLifecycle, apply_binding_lifecycle,
};
use super::*;

#[derive(Clone)]
pub(super) struct TelegramTopicCleanupTarget {
    pub(super) conversation_key: String,
    pub(super) route: RouteTarget,
    pub(super) thread_id: String,
    pub(super) chat_id: String,
    pub(super) topic_id: i64,
    pub(super) lifecycle_revision: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TelegramTopicDeleteOutcome {
    Deleted,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TelegramTopicMutationDeadlineGate {
    Ready,
    RecheckLifecycle,
}

pub(super) fn telegram_topic_mutation_cooldown(err: &anyhow::Error) -> Option<Duration> {
    let error = err.downcast_ref::<TelegramApiError>()?;
    error
        .retry_after
        .map(|seconds| Duration::from_secs(seconds.max(1)))
        .or_else(|| {
            error
                .is_rate_limited()
                .then_some(Duration::from_secs(TELEGRAM_TOPIC_DELETE_RETRY_SECONDS))
        })
}

#[derive(Debug)]
pub(super) struct TelegramTopicDeleteRetry {
    attempt: usize,
    max_attempts: usize,
    pub(super) delay: Duration,
    reason: String,
}

pub(super) async fn remove_telegram_topic_for_codex_thread(
    state: &SharedState,
    api_registry: &ImApiRegistry,
    thread_id: &str,
    generation: u64,
    lifecycle: TelegramTopicLifecycle,
    reason: &'static str,
) {
    if !is_current_bridge_generation(state, generation).await {
        return;
    }
    let targets =
        mark_telegram_topic_bindings_for_cleanup(state, thread_id, lifecycle, generation).await;
    for target in targets {
        let Some(api) = api_registry.telegram_for_route(&target.route) else {
            state
                .push_event(
                    "warn",
                    "telegram_topic_cleanup_api_missing",
                    format!(
                        "thread={} account={} topic={}",
                        thread_id, target.route.account_id, target.topic_id
                    ),
                )
                .await;
            continue;
        };
        let _ = schedule_telegram_topic_cleanup(state, &api, target, generation, reason).await;
    }
}

pub(super) async fn mark_telegram_topic_bindings_for_cleanup(
    state: &SharedState,
    thread_id: &str,
    lifecycle: TelegramTopicLifecycle,
    generation: u64,
) -> Vec<TelegramTopicCleanupTarget> {
    let now = now_ms();
    let path = state.config.lock().await.state_path.clone();
    let _binding_guard = state.im_route_binding_ops.lock().await;
    let runtime = state.runtime.lock().await;
    if runtime.bridge_generation != generation {
        return Vec::new();
    }
    let lifecycle_state = match lifecycle {
        TelegramTopicLifecycle::Deleted => TelegramThreadLifecycleState::Deleted,
        TelegramTopicLifecycle::Archived => TelegramThreadLifecycleState::Archived,
        _ => unreachable!("Topic cleanup notifications are archive or delete events"),
    };
    state
        .observe_telegram_thread_lifecycle(thread_id, generation, lifecycle_state)
        .await;
    let mut persisted = state.persisted.lock().await;
    let keys = persisted
        .im_thread_bindings
        .iter()
        .filter(|(_, bound_thread_id)| bound_thread_id.as_str() == thread_id)
        .map(|(key, _)| key.clone())
        .collect::<Vec<_>>();
    let mut targets = Vec::new();
    for conversation_key in keys {
        let Some(route) = route_from_conversation_key(&conversation_key) else {
            continue;
        };
        if route.platform != ImPlatformKind::Telegram {
            continue;
        }
        let (chat_id, Some(topic_id)) = crate::types::split_telegram_message_target(&route.chat_id)
        else {
            continue;
        };
        let chat_id = chat_id.to_string();
        let binding = persisted
            .telegram_topic_binding_states
            .entry(conversation_key.clone())
            .or_default();
        if binding.lifecycle_generation > generation {
            continue;
        }
        let same_generation = binding.lifecycle_generation == generation;
        binding.thread_id = thread_id.to_string();
        let lifecycle = if same_generation && binding.codex_state == "deleted" {
            TelegramTopicLifecycle::Deleted
        } else {
            lifecycle
        };
        apply_binding_lifecycle(binding, lifecycle, generation, now);
        binding.lifecycle_revision = binding.lifecycle_revision.saturating_add(1);
        targets.push(TelegramTopicCleanupTarget {
            conversation_key,
            route,
            thread_id: thread_id.to_string(),
            chat_id,
            topic_id,
            lifecycle_revision: binding.lifecycle_revision,
        });
    }
    drop(runtime);
    if !targets.is_empty()
        && let Err(err) = persisted.save(&path)
    {
        chain_log::write_diagnostic_lazy(|| {
            format!(
                "[telegram_topic] event=archive_save_failed thread={} err={err}",
                thread_id
            )
        });
    }
    targets
}

async fn schedule_telegram_topic_cleanup(
    state: &SharedState,
    api: &TelegramApi,
    target: TelegramTopicCleanupTarget,
    generation: u64,
    reason: &'static str,
) -> bool {
    // Register the cleanup intent at the same linearization point used by
    // create/edit/delete attempts. Mutations that acquire the gate afterward
    // can observe the pending cleanup before touching Telegram.
    let mutation_gate = state
        .telegram_topic_mutation_gate(&target.route.account_id)
        .await;
    let scheduling_guard = mutation_gate.lock().await;
    if !telegram_topic_cleanup_can_continue(state, &target, generation, target.lifecycle_revision)
        .await
    {
        return false;
    }
    let (token, notifier, replaced_notifier, should_spawn, should_wake, accepted) = {
        let mut registrations = state.telegram_topic_cleanup_registrations.lock().await;
        if let Some(registration) = registrations.get_mut(&target.conversation_key) {
            if generation < registration.lifecycle_generation {
                (
                    registration.token,
                    registration.notifier.clone(),
                    None,
                    false,
                    false,
                    false,
                )
            } else if generation > registration.lifecycle_generation {
                let replaced_notifier = registration.notifier.clone();
                let token = state.next_telegram_topic_cleanup_token();
                let notifier = Arc::new(tokio::sync::Notify::new());
                *registration = TelegramTopicCleanupRegistration {
                    token,
                    lifecycle_generation: generation,
                    lifecycle_revision: target.lifecycle_revision,
                    notifier: notifier.clone(),
                };
                (token, notifier, Some(replaced_notifier), true, false, true)
            } else {
                let should_wake = target.lifecycle_revision > registration.lifecycle_revision;
                if should_wake {
                    registration.lifecycle_revision = target.lifecycle_revision;
                }
                (
                    registration.token,
                    registration.notifier.clone(),
                    None,
                    false,
                    should_wake,
                    should_wake,
                )
            }
        } else {
            let token = state.next_telegram_topic_cleanup_token();
            let notifier = Arc::new(tokio::sync::Notify::new());
            registrations.insert(
                target.conversation_key.clone(),
                TelegramTopicCleanupRegistration {
                    token,
                    lifecycle_generation: generation,
                    lifecycle_revision: target.lifecycle_revision,
                    notifier: notifier.clone(),
                },
            );
            (token, notifier, None, true, false, true)
        }
    };
    drop(scheduling_guard);

    if let Some(replaced_notifier) = replaced_notifier {
        replaced_notifier.notify_one();
    }
    if should_wake {
        notifier.notify_one();
    }
    if !accepted {
        return false;
    }
    if !should_spawn {
        return true;
    }

    let state = state.clone();
    let api = api.clone();
    tokio::spawn(async move {
        drive_telegram_topic_cleanup(&state, &api, target, generation, reason, token, &notifier)
            .await;
    });
    true
}

async fn drive_telegram_topic_cleanup(
    state: &SharedState,
    api: &TelegramApi,
    mut target: TelegramTopicCleanupTarget,
    mut generation: u64,
    reason: &'static str,
    token: u64,
    notifier: &tokio::sync::Notify,
) {
    loop {
        if wait_for_telegram_topic_cleanup_retry_deadline(
            state,
            &target,
            generation,
            target.lifecycle_revision,
            notifier,
        )
        .await
        {
            run_telegram_topic_cleanup(
                state,
                api,
                &target,
                generation,
                target.lifecycle_revision,
                reason,
                notifier,
            )
            .await;
        }

        let Some((next_generation, next_revision)) =
            finish_telegram_topic_cleanup_worker_iteration(
                state,
                &target,
                token,
                generation,
                target.lifecycle_revision,
            )
            .await
        else {
            return;
        };
        generation = next_generation;
        target.lifecycle_revision = next_revision;
    }
}

pub(super) async fn finish_telegram_topic_cleanup_worker_iteration(
    state: &SharedState,
    target: &TelegramTopicCleanupTarget,
    token: u64,
    completed_generation: u64,
    completed_revision: u64,
) -> Option<(u64, u64)> {
    let _binding_guard = state.im_route_binding_ops.lock().await;
    let runtime = state.runtime.lock().await;
    let mut registrations = state.telegram_topic_cleanup_registrations.lock().await;
    let registration = registrations.get(&target.conversation_key)?;
    if registration.token != token {
        return None;
    }
    let registered_generation = registration.lifecycle_generation;

    let persisted = state.persisted.lock().await;
    let next_lifecycle = persisted
        .im_thread_bindings
        .get(&target.conversation_key)
        .is_some_and(|thread_id| thread_id == &target.thread_id)
        .then(|| {
            persisted
                .telegram_topic_binding_states
                .get(&target.conversation_key)
        })
        .flatten()
        .filter(|binding| {
            binding.lifecycle_generation == runtime.bridge_generation
                && binding.lifecycle_generation == registered_generation
                && binding.lifecycle_generation == completed_generation
                && binding.lifecycle_revision > completed_revision
                && matches!(
                    binding.codex_state.as_str(),
                    "archived" | "deleted" | "missing"
                )
        })
        .map(|binding| (binding.lifecycle_generation, binding.lifecycle_revision));
    drop(persisted);
    drop(runtime);

    if let Some((generation, revision)) = next_lifecycle {
        if let Some(registration) = registrations.get_mut(&target.conversation_key)
            && registration.token == token
        {
            registration.lifecycle_revision = registration.lifecycle_revision.max(revision);
            return Some((generation, revision));
        }
        return None;
    }

    if registrations
        .get(&target.conversation_key)
        .is_some_and(|registration| registration.token == token)
    {
        registrations.remove(&target.conversation_key);
    }
    None
}

pub(super) async fn wait_for_telegram_topic_cleanup_retry_deadline(
    state: &SharedState,
    target: &TelegramTopicCleanupTarget,
    generation: u64,
    lifecycle_revision: u64,
    notifier: &tokio::sync::Notify,
) -> bool {
    loop {
        if !telegram_topic_cleanup_can_continue(state, target, generation, lifecycle_revision).await
        {
            return false;
        }
        let Some(deadline) = state
            .telegram_topic_cleanup_retry_deadline(&target.route.account_id)
            .await
        else {
            return true;
        };
        if deadline <= Instant::now() {
            state
                .clear_telegram_topic_cleanup_retry_deadline_if_elapsed(
                    &target.route.account_id,
                    deadline,
                )
                .await;
            continue;
        }
        tokio::select! {
            _ = tokio::time::sleep_until(deadline.into()) => {
                state
                    .clear_telegram_topic_cleanup_retry_deadline_if_elapsed(
                        &target.route.account_id,
                        deadline,
                    )
                    .await;
            }
            _ = notifier.notified() => {}
        }
    }
}

async fn run_telegram_topic_cleanup(
    state: &SharedState,
    api: &TelegramApi,
    target: &TelegramTopicCleanupTarget,
    generation: u64,
    lifecycle_revision: u64,
    reason: &'static str,
    retry_notifier: &tokio::sync::Notify,
) {
    let context = format!("thread={} topic={}", target.thread_id, target.topic_id);
    match delete_forum_topic_with_retry_while(
        state,
        api,
        &target.chat_id,
        target.topic_id,
        &context,
        Some(retry_notifier),
        &target.route.account_id,
        || telegram_topic_cleanup_can_continue(state, target, generation, lifecycle_revision),
    )
    .await
    {
        Ok(TelegramTopicDeleteOutcome::Deleted) => {
            // The remote Topic is already gone. Even if an unarchive or a
            // bridge restart raced the request, retaining this binding would
            // point future messages at a non-existent Topic.
            finish_telegram_topic_cleanup(state, target, reason, false).await;
        }
        Ok(TelegramTopicDeleteOutcome::Cancelled) => {}
        Err(err) => {
            state
                .push_event(
                    "warn",
                    "telegram_topic_delete_failed",
                    format!(
                        "thread={} topic={} err={err}",
                        target.thread_id, target.topic_id
                    ),
                )
                .await;
        }
    }
}

async fn telegram_topic_cleanup_can_continue(
    state: &SharedState,
    target: &TelegramTopicCleanupTarget,
    generation: u64,
    lifecycle_revision: u64,
) -> bool {
    is_current_bridge_generation(state, generation).await
        && telegram_topic_cleanup_is_required(state, target, generation, lifecycle_revision).await
}

pub(super) async fn telegram_topic_cleanup_is_required(
    state: &SharedState,
    target: &TelegramTopicCleanupTarget,
    generation: u64,
    lifecycle_revision: u64,
) -> bool {
    let persisted = state.persisted.lock().await;
    persisted
        .im_thread_bindings
        .get(&target.conversation_key)
        .is_some_and(|thread_id| thread_id == &target.thread_id)
        && persisted
            .telegram_topic_binding_states
            .get(&target.conversation_key)
            .is_some_and(|binding| {
                binding.lifecycle_generation == generation
                    && binding.lifecycle_revision == lifecycle_revision
                    && matches!(
                        binding.codex_state.as_str(),
                        "archived" | "deleted" | "missing"
                    )
            })
}

pub(super) async fn finish_telegram_topic_cleanup(
    state: &SharedState,
    target: &TelegramTopicCleanupTarget,
    reason: &str,
    already_missing: bool,
) {
    match crate::im::core::routing::clear_thread_binding_if_matches_with_reason(
        state,
        &target.conversation_key,
        &target.thread_id,
        reason,
    )
    .await
    {
        Ok(true) => {
            state
                .push_event(
                    "info",
                    "telegram_topic_deleted",
                    format!(
                        "thread={} topic={} already_missing={}",
                        target.thread_id, target.topic_id, already_missing
                    ),
                )
                .await;
        }
        Ok(false) => {
            state
                .push_event(
                    "info",
                    "telegram_topic_binding_remove_skipped",
                    format!(
                        "thread={} topic={} reason=binding_changed",
                        target.thread_id, target.topic_id
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
                        "thread={} topic={} err={err}",
                        target.thread_id, target.topic_id
                    ),
                )
                .await;
        }
    }
}

pub(super) async fn delete_forum_topic_with_retry_while<F, Fut>(
    state: &SharedState,
    api: &TelegramApi,
    chat_id: &str,
    topic_id: i64,
    context: &str,
    retry_notifier: Option<&tokio::sync::Notify>,
    retry_account_id: &str,
    should_continue: F,
) -> Result<TelegramTopicDeleteOutcome>
where
    F: Fn() -> Fut + Clone,
    Fut: std::future::Future<Output = bool>,
{
    retry_telegram_topic_delete(
        TELEGRAM_TOPIC_DELETE_MAX_ATTEMPTS,
        || {
            run_telegram_topic_mutation_while(
                state,
                retry_account_id,
                retry_notifier,
                should_continue.clone(),
                || api.delete_forum_topic(chat_id, topic_id),
            )
        },
        |retry| async move {
            state
                .push_event(
                    "warn",
                    "telegram_topic_delete_retry",
                    format!(
                        "{} attempt={}/{} retry_in={}s reason={}",
                        context,
                        retry.attempt,
                        retry.max_attempts,
                        retry.delay.as_secs(),
                        retry.reason
                    ),
                )
                .await;
            wait_for_telegram_topic_delete_retry(retry.delay, retry_notifier).await;
            if let Some(deadline) = state
                .telegram_topic_cleanup_retry_deadline(retry_account_id)
                .await
                && deadline <= Instant::now()
            {
                state
                    .clear_telegram_topic_cleanup_retry_deadline_if_elapsed(
                        retry_account_id,
                        deadline,
                    )
                    .await;
            }
        },
    )
    .await
}

pub(super) async fn wait_for_telegram_topic_mutation_deadline(
    state: &SharedState,
    account_id: &str,
    retry_notifier: Option<&tokio::sync::Notify>,
) -> TelegramTopicMutationDeadlineGate {
    let Some(deadline) = state
        .telegram_topic_cleanup_retry_deadline(account_id)
        .await
    else {
        return TelegramTopicMutationDeadlineGate::Ready;
    };
    if deadline <= Instant::now() {
        state
            .clear_telegram_topic_cleanup_retry_deadline_if_elapsed(account_id, deadline)
            .await;
        return TelegramTopicMutationDeadlineGate::Ready;
    }
    if let Some(notifier) = retry_notifier {
        tokio::select! {
            _ = tokio::time::sleep_until(deadline.into()) => {
                state
                    .clear_telegram_topic_cleanup_retry_deadline_if_elapsed(account_id, deadline)
                    .await;
                TelegramTopicMutationDeadlineGate::Ready
            }
            _ = notifier.notified() => TelegramTopicMutationDeadlineGate::RecheckLifecycle,
        }
    } else {
        tokio::time::sleep_until(deadline.into()).await;
        state
            .clear_telegram_topic_cleanup_retry_deadline_if_elapsed(account_id, deadline)
            .await;
        TelegramTopicMutationDeadlineGate::Ready
    }
}

pub(super) async fn wait_for_telegram_topic_delete_retry(
    delay: Duration,
    retry_notifier: Option<&tokio::sync::Notify>,
) {
    if let Some(notifier) = retry_notifier {
        tokio::select! {
            _ = sleep(delay) => {}
            _ = notifier.notified() => {}
        }
    } else {
        sleep(delay).await;
    }
}

pub(super) async fn retry_telegram_topic_delete<D, DeleteFuture, W, WaitFuture>(
    max_attempts: usize,
    mut delete: D,
    mut wait_before_retry: W,
) -> Result<TelegramTopicDeleteOutcome>
where
    D: FnMut() -> DeleteFuture,
    DeleteFuture: std::future::Future<Output = Option<Result<bool>>>,
    W: FnMut(TelegramTopicDeleteRetry) -> WaitFuture,
    WaitFuture: std::future::Future<Output = ()>,
{
    let max_attempts = max_attempts.max(1);
    for attempt in 1..=max_attempts {
        let Some(result) = delete().await else {
            return Ok(TelegramTopicDeleteOutcome::Cancelled);
        };
        let retry = match result {
            Ok(true) => return Ok(TelegramTopicDeleteOutcome::Deleted),
            Ok(false) if attempt == max_attempts => {
                return Err(anyhow!(
                    "telegram api deleteForumTopic returned false after {attempt} attempts"
                ));
            }
            Ok(false) => TelegramTopicDeleteRetry {
                attempt,
                max_attempts,
                delay: telegram_topic_delete_retry_delay(None, attempt),
                reason: "api_returned_false".to_string(),
            },
            Err(err) if !telegram_topic_delete_should_retry(&err) || attempt == max_attempts => {
                return Err(err);
            }
            Err(err) => TelegramTopicDeleteRetry {
                attempt,
                max_attempts,
                delay: telegram_topic_delete_retry_delay(Some(&err), attempt),
                reason: err.to_string(),
            },
        };
        wait_before_retry(retry).await;
    }
    unreachable!("bounded Telegram Topic delete retry always returns")
}

pub(super) fn telegram_topic_delete_retry_delay(
    err: Option<&anyhow::Error>,
    attempt: usize,
) -> Duration {
    if let Some(retry_after) = err
        .and_then(|err| err.downcast_ref::<TelegramApiError>())
        .and_then(|err| err.retry_after)
    {
        return Duration::from_secs(retry_after.max(1));
    }
    let exponent = u32::try_from(attempt.saturating_sub(1))
        .unwrap_or(u32::MAX)
        .min(4);
    Duration::from_secs(
        TELEGRAM_TOPIC_DELETE_RETRY_SECONDS
            .saturating_mul(1_u64 << exponent)
            .min(60),
    )
}

pub(super) fn telegram_topic_delete_should_retry(err: &anyhow::Error) -> bool {
    err.downcast_ref::<TelegramApiError>().is_none_or(|err| {
        let api_error = err.error_code.unwrap_or_default();
        err.is_rate_limited()
            || err.status == reqwest::StatusCode::REQUEST_TIMEOUT
            || err.status.is_server_error()
            || api_error == i64::from(reqwest::StatusCode::REQUEST_TIMEOUT.as_u16())
            || (500..=599).contains(&api_error)
    })
}

pub(super) async fn schedule_reconciled_telegram_topic_cleanup(
    state: &SharedState,
    api_registry: &ImApiRegistry,
    commit: TelegramTopicBindingCommit,
    thread_id: String,
    generation: u64,
) -> bool {
    let Some(conversation_key) = commit.conversation_key else {
        return false;
    };
    let Some(route) = route_from_conversation_key(&conversation_key) else {
        return false;
    };
    let (chat_id, Some(topic_id)) = crate::types::split_telegram_message_target(&route.chat_id)
    else {
        return false;
    };
    let chat_id = chat_id.to_string();
    let Some(api) = api_registry.telegram_for_route(&route) else {
        state
            .push_event(
                "warn",
                "telegram_topic_cleanup_api_missing",
                format!(
                    "thread={} account={} topic={}",
                    thread_id, route.account_id, topic_id
                ),
            )
            .await;
        return false;
    };
    schedule_telegram_topic_cleanup(
        state,
        &api,
        TelegramTopicCleanupTarget {
            conversation_key,
            route,
            thread_id,
            chat_id,
            topic_id,
            lifecycle_revision: commit.lifecycle_revision,
        },
        generation,
        "codex_session_archived_or_deleted",
    )
    .await
}
