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
    im_runtime::{RouteTarget, route_from_conversation_key},
    remote_control_backend,
    types::{
        ChatType, ImPlatformKind, InboundAction, InboundMessage, ThreadRouteDirection, now_ms,
        telegram_message_target,
    },
};

use super::{
    api::{
        TelegramApi, TelegramApiError, TelegramBotCommand, TelegramCallbackQuery,
        TelegramForumTopicEditOutcome, TelegramMessage,
    },
    types::TelegramSettings,
};

const TELEGRAM_LONG_POLL_TIMEOUT_SECONDS: u32 = 25;
const TELEGRAM_STARTUP_PROBE_RETRY_SECONDS: u64 = 5;
const TELEGRAM_CONFLICT_BACKOFF_SECONDS: u64 = 35;
const TELEGRAM_GENERIC_RETRY_SECONDS: u64 = 5;
const TELEGRAM_TOPIC_RECONCILIATION_INTERVAL: Duration = Duration::from_secs(300);
const TELEGRAM_TOPIC_STATE_GRACE: Duration = Duration::from_secs(300);
const TELEGRAM_TOPIC_DELETE_MAX_ATTEMPTS: usize = 6;
const TELEGRAM_TOPIC_DELETE_RETRY_SECONDS: u64 = 5;
const TELEGRAM_TOPIC_NAME_MARKER_TTL: Duration = Duration::from_secs(120);
const AUTO_TOPIC_SESSION_PAGE_LIMIT: u32 = 100;
const AUTO_TOPIC_SESSION_MAX_PAGES: usize = 20;
const AUTO_TOPIC_RESUME_MAX_ATTEMPTS: usize = 20;
const AUTO_TOPIC_RESUME_RETRY_DELAY: Duration = Duration::from_millis(500);

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
        generation,
        connection_epoch,
    )
    .await;
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

async fn is_current_bridge_generation(state: &SharedState, generation: u64) -> bool {
    state.runtime.lock().await.is_bridge_generation(generation)
}

async fn telegram_topic_creation_is_current(
    state: &SharedState,
    thread_id: &str,
    generation: u64,
) -> bool {
    is_current_bridge_generation(state, generation).await
        && state
            .telegram_thread_allows_topic_binding(thread_id, generation)
            .await
}

async fn keep_auto_created_topic_if_current<T, C, CleanupFuture>(
    state: &SharedState,
    thread_id: &str,
    generation: u64,
    topic: T,
    cleanup: C,
) -> Option<T>
where
    C: FnOnce(T) -> CleanupFuture,
    CleanupFuture: std::future::Future<Output = ()>,
{
    if telegram_topic_creation_is_current(state, thread_id, generation).await {
        Some(topic)
    } else {
        cleanup(topic).await;
        None
    }
}

async fn resume_auto_topic_thread(
    state: &SharedState,
    remote_client_key: &str,
    thread_id: &str,
    generation: u64,
    connection_epoch: Option<u64>,
) -> Result<Value> {
    let remote_client_key = remote_client_key.to_string();
    let thread_id_for_resume = thread_id.to_string();
    retry_auto_topic_resume(
        state,
        thread_id,
        generation,
        AUTO_TOPIC_RESUME_RETRY_DELAY,
        || {
            let state = state.clone();
            let remote_client_key = remote_client_key.clone();
            let thread_id = thread_id_for_resume.clone();
            async move {
                match connection_epoch {
                    Some(connection_epoch) => {
                        remote_control_backend::resume_thread_for_client_on_connection(
                            &state,
                            connection_epoch,
                            &remote_client_key,
                            &thread_id,
                            true,
                        )
                        .await
                    }
                    None => {
                        remote_control_backend::resume_thread_for_client(
                            &state,
                            &remote_client_key,
                            &thread_id,
                            true,
                        )
                        .await
                    }
                }
            }
        },
    )
    .await
}

async fn retry_auto_topic_resume<F, Fut>(
    state: &SharedState,
    thread_id: &str,
    generation: u64,
    retry_delay: Duration,
    mut resume: F,
) -> Result<Value>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<Value>>,
{
    for attempt in 1..=AUTO_TOPIC_RESUME_MAX_ATTEMPTS {
        if !telegram_topic_creation_is_current(state, thread_id, generation).await {
            return Err(anyhow!(
                "bridge generation changed or thread lifecycle changed while waiting for session rollout"
            ));
        }
        let result = resume().await;
        match result {
            Ok(response) => return Ok(response),
            Err(err) => {
                if !should_retry_auto_topic_resume(&err, attempt) {
                    return Err(err);
                }
                state
                    .push_event(
                        "info",
                        "telegram_auto_topic_resume_retry",
                        format!(
                            "thread={} attempt={}/{} reason=session rollout is not visible yet",
                            thread_id,
                            attempt + 1,
                            AUTO_TOPIC_RESUME_MAX_ATTEMPTS
                        ),
                    )
                    .await;
                sleep(retry_delay).await;
            }
        }
    }
    unreachable!("auto Topic resume retry loop always returns")
}

fn should_retry_auto_topic_resume(err: &anyhow::Error, attempt: usize) -> bool {
    attempt < AUTO_TOPIC_RESUME_MAX_ATTEMPTS
        && err.to_string().contains("no rollout found for thread id")
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
    let context = format!("auto_create thread={thread_id}");
    match delete_forum_topic_with_retry(state, api, chat_id, topic_id, &context).await {
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

#[derive(Clone)]
struct TelegramTopicCleanupTarget {
    conversation_key: String,
    route: RouteTarget,
    thread_id: String,
    chat_id: String,
    topic_id: i64,
    lifecycle_revision: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TelegramTopicDeleteOutcome {
    Deleted,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TelegramTopicMutationDeadlineGate {
    Ready,
    RecheckLifecycle,
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

fn telegram_topic_mutation_cooldown(err: &anyhow::Error) -> Option<Duration> {
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
struct TelegramTopicDeleteRetry {
    attempt: usize,
    max_attempts: usize,
    delay: Duration,
    reason: String,
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
        "archived",
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
        "deleted",
        "codex_thread_deleted_notification",
    )
    .await;
}

async fn remove_telegram_topic_for_codex_thread(
    state: &SharedState,
    api_registry: &ImApiRegistry,
    thread_id: &str,
    generation: u64,
    codex_state: &'static str,
    reason: &'static str,
) {
    if !is_current_bridge_generation(state, generation).await {
        return;
    }
    let targets =
        mark_telegram_topic_bindings_for_cleanup(state, thread_id, codex_state, generation).await;
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
            binding.codex_state = "active".to_string();
            binding.archived_at_ms = None;
            binding.missing_at_ms = None;
            binding.lifecycle_generation = generation;
            binding.lifecycle_revision = binding.lifecycle_revision.saturating_add(1);
            binding.last_checked_at_ms = now_ms();
            changed_keys.push((
                key,
                binding.lifecycle_generation,
                binding.lifecycle_revision,
            ));
        }
    }
    drop(runtime);
    if !changed_keys.is_empty() {
        if let Err(err) = persisted.save(&path) {
            chain_log::write_diagnostic_lazy(|| {
                format!(
                    "[telegram_topic] event=unarchive_save_failed thread={} err={err}",
                    thread_id
                )
            });
        }
    }
    drop(persisted);
    drop(_binding_guard);

    for (key, lifecycle_generation, lifecycle_revision) in changed_keys {
        state
            .notify_telegram_topic_cleanup_if_older(&key, lifecycle_generation, lifecycle_revision)
            .await;
    }
}

async fn mark_telegram_topic_bindings_for_cleanup(
    state: &SharedState,
    thread_id: &str,
    codex_state: &str,
    generation: u64,
) -> Vec<TelegramTopicCleanupTarget> {
    let now = now_ms();
    let path = state.config.lock().await.state_path.clone();
    let _binding_guard = state.im_route_binding_ops.lock().await;
    let runtime = state.runtime.lock().await;
    if runtime.bridge_generation != generation {
        return Vec::new();
    }
    let lifecycle_state = if codex_state == "deleted" {
        TelegramThreadLifecycleState::Deleted
    } else {
        TelegramThreadLifecycleState::Archived
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
        if !same_generation || binding.codex_state != "deleted" || codex_state == "deleted" {
            binding.codex_state = codex_state.to_string();
        }
        binding.archived_at_ms.get_or_insert(now);
        binding.missing_at_ms = None;
        binding.lifecycle_generation = generation;
        binding.lifecycle_revision = binding.lifecycle_revision.saturating_add(1);
        binding.last_checked_at_ms = now;
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
    if !targets.is_empty() {
        if let Err(err) = persisted.save(&path) {
            chain_log::write_diagnostic_lazy(|| {
                format!(
                    "[telegram_topic] event=archive_save_failed thread={} err={err}",
                    thread_id
                )
            });
        }
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

async fn finish_telegram_topic_cleanup_worker_iteration(
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

async fn wait_for_telegram_topic_cleanup_retry_deadline(
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
        Some(&target.route.account_id),
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

async fn telegram_topic_cleanup_is_required(
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

async fn finish_telegram_topic_cleanup(
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

pub(crate) async fn delete_forum_topic_with_retry(
    state: &SharedState,
    api: &TelegramApi,
    chat_id: &str,
    topic_id: i64,
    context: &str,
) -> Result<bool> {
    let account_id = api.settings().account_id();
    let outcome = delete_forum_topic_with_retry_while(
        state,
        api,
        chat_id,
        topic_id,
        context,
        None,
        Some(&account_id),
        || std::future::ready(true),
    )
    .await?;
    Ok(matches!(outcome, TelegramTopicDeleteOutcome::Deleted))
}

async fn delete_forum_topic_with_retry_while<F, Fut>(
    state: &SharedState,
    api: &TelegramApi,
    chat_id: &str,
    topic_id: i64,
    context: &str,
    retry_notifier: Option<&tokio::sync::Notify>,
    retry_account_id: Option<&str>,
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
                retry_account_id.unwrap_or_else(|| api.settings().account_id.as_str()),
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
            if let Some(account_id) = retry_account_id
                && let Some(deadline) = state
                    .telegram_topic_cleanup_retry_deadline(account_id)
                    .await
                && deadline <= Instant::now()
            {
                state
                    .clear_telegram_topic_cleanup_retry_deadline_if_elapsed(account_id, deadline)
                    .await;
            }
        },
    )
    .await
}

async fn wait_for_telegram_topic_mutation_deadline(
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

async fn wait_for_telegram_topic_delete_retry(
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

async fn retry_telegram_topic_delete<D, DeleteFuture, W, WaitFuture>(
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

fn telegram_topic_delete_retry_delay(err: Option<&anyhow::Error>, attempt: usize) -> Duration {
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

fn telegram_topic_delete_should_retry(err: &anyhow::Error) -> bool {
    err.downcast_ref::<TelegramApiError>().map_or(true, |err| {
        let api_error = err.error_code.unwrap_or_default();
        err.is_rate_limited()
            || err.status == reqwest::StatusCode::REQUEST_TIMEOUT
            || err.status.is_server_error()
            || api_error == i64::from(reqwest::StatusCode::REQUEST_TIMEOUT.as_u16())
            || (500..=599).contains(&api_error)
    })
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
            crate::im_runtime::route_from_conversation_key(&key)
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

async fn persist_telegram_topic_service_state(
    state: &SharedState,
    conversation_key: &str,
    topic_state: &str,
) {
    let path = state.config.lock().await.state_path.clone();
    let mut persisted = state.persisted.lock().await;
    let Some(binding) = persisted
        .telegram_topic_binding_states
        .get_mut(conversation_key)
    else {
        return;
    };
    binding.telegram_state = topic_state.to_string();
    binding.last_checked_at_ms = crate::types::now_ms();
    if let Err(err) = persisted.save(&path) {
        chain_log::write_diagnostic_lazy(|| {
            format!(
                "[telegram_topic] event=state_save_failed key={} err={err}",
                conversation_key
            )
        });
    }
}

async fn record_telegram_topic_name_for_codex_sync(
    state: &SharedState,
    conversation_key: &str,
    topic_state: &str,
    topic_name: &str,
    operation_token: u64,
) -> Option<String> {
    let path = state.config.lock().await.state_path.clone();
    let operations = state.telegram_topic_name_update_ops.lock().await;
    if !operations
        .get(conversation_key)
        .is_some_and(|current| *current == operation_token)
    {
        return None;
    }
    let _binding_guard = state.im_route_binding_ops.lock().await;
    let mut persisted = state.persisted.lock().await;
    let thread_id = persisted
        .im_thread_bindings
        .get(conversation_key)
        .cloned()?;
    let binding = persisted
        .telegram_topic_binding_states
        .get_mut(conversation_key)?;
    if thread_id.trim().is_empty() || binding.thread_id != thread_id {
        return None;
    }
    binding.telegram_state = topic_state.to_string();
    binding.topic_name = topic_name.to_string();
    binding.last_checked_at_ms = crate::types::now_ms();
    if let Err(err) = persisted.save(&path) {
        chain_log::write_diagnostic_lazy(|| {
            format!(
                "[telegram_topic] event=state_save_failed key={} err={err}",
                conversation_key
            )
        });
    }
    drop(operations);
    Some(thread_id)
}

async fn commit_telegram_topic_name_to_codex_if_current(
    state: &SharedState,
    conversation_key: &str,
    thread_id: &str,
    topic_name: &str,
    operation_token: u64,
) -> bool {
    let path = state.config.lock().await.state_path.clone();
    let operations = state.telegram_topic_name_update_ops.lock().await;
    if !operations
        .get(conversation_key)
        .is_some_and(|current| *current == operation_token)
    {
        return false;
    }
    let _binding_guard = state.im_route_binding_ops.lock().await;
    let mut persisted = state.persisted.lock().await;
    if !persisted
        .im_thread_bindings
        .get(conversation_key)
        .is_some_and(|bound_thread_id| bound_thread_id == thread_id)
    {
        return false;
    }
    let Some(binding) = persisted
        .telegram_topic_binding_states
        .get_mut(conversation_key)
    else {
        return false;
    };
    if binding.thread_id != thread_id || binding.topic_name != topic_name {
        return false;
    }
    binding.codex_title = topic_name.to_string();
    binding.last_synced_codex_title = topic_name.to_string();
    binding.last_synced_topic_name = topic_name.to_string();
    binding.last_checked_at_ms = crate::types::now_ms();
    if let Err(err) = persisted.save(&path) {
        chain_log::write_diagnostic_lazy(|| {
            format!(
                "[telegram_topic] event=sync_save_failed key={} err={err}",
                conversation_key
            )
        });
    }
    drop(operations);
    true
}

async fn consume_telegram_topic_name_marker(
    state: &SharedState,
    conversation_key: &str,
    topic_name: &str,
) -> bool {
    let mut operations = state.telegram_topic_name_sync_ops.lock().await;
    let mut matched = false;
    let remove_key = operations.get_mut(conversation_key).is_some_and(|markers| {
        if let Some(index) = markers.iter().position(|marker| marker.name == topic_name) {
            markers.remove(index);
            matched = true;
        }
        markers.is_empty()
    });
    if remove_key {
        operations.remove(conversation_key);
    }
    matched
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TelegramTopicLifecycle {
    Active,
    Archived,
    Deleted,
    MissingGrace,
    MissingDelete,
}

async fn reconcile_telegram_topic_bindings(
    state: &SharedState,
    api: &TelegramApi,
    api_registry: &ImApiRegistry,
) {
    let account_id = api.settings().account_id();
    let (generation, mut bindings) = {
        let _binding_guard = state.im_route_binding_ops.lock().await;
        let runtime = state.runtime.lock().await;
        let persisted = state.persisted.lock().await;
        let bindings = persisted
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
            .collect::<Vec<_>>();
        drop(persisted);
        let generation = runtime.bridge_generation;
        let mut versioned_bindings = Vec::with_capacity(bindings.len());
        for (conversation_key, thread_id, binding_state) in bindings {
            let Some(lifecycle_revision) = state
                .telegram_thread_lifecycle_revision(&thread_id, generation)
                .await
            else {
                continue;
            };
            versioned_bindings.push((
                conversation_key,
                thread_id,
                binding_state,
                lifecycle_revision,
            ));
        }
        (generation, versioned_bindings)
    };
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
    if !is_current_bridge_generation(state, generation).await {
        return;
    }
    // Archive cleanup is the highest-priority mutation. Active Topic title
    // synchronization must never consume the rate-limit budget first.
    bindings.sort_by_key(|(_, thread_id, _, _)| {
        if archived_ids.contains(thread_id) {
            0
        } else if active_ids.contains(thread_id) {
            2
        } else {
            1
        }
    });
    let mut cleanup_queued = false;
    for (conversation_key, thread_id, state_snapshot, lifecycle_intent_revision) in bindings {
        let Some(route) = route_from_conversation_key(&conversation_key) else {
            continue;
        };
        let (raw_chat_id, Some(topic_id)) =
            crate::types::split_telegram_message_target(&route.chat_id)
        else {
            continue;
        };
        let raw_chat_id = raw_chat_id.to_string();
        let now = now_ms();
        let expected_state = state_snapshot;
        let mut next_state = expected_state.clone().unwrap_or_default();
        next_state.thread_id = thread_id.clone();
        let lifecycle = update_telegram_topic_lifecycle(
            &mut next_state,
            &thread_id,
            &active_ids,
            &archived_ids,
            generation,
            now,
        );
        let lifecycle_revision = next_state.lifecycle_revision;

        match lifecycle {
            TelegramTopicLifecycle::Archived
            | TelegramTopicLifecycle::Deleted
            | TelegramTopicLifecycle::MissingDelete => {
                let persisted = persist_telegram_topic_binding_state(
                    state,
                    &conversation_key,
                    &thread_id,
                    expected_state.as_ref(),
                    next_state,
                    generation,
                    lifecycle_intent_revision,
                )
                .await;
                if let Some(commit) = persisted {
                    cleanup_queued |= schedule_reconciled_telegram_topic_cleanup(
                        state,
                        api_registry,
                        commit,
                        thread_id,
                        generation,
                    )
                    .await;
                }
                continue;
            }
            TelegramTopicLifecycle::MissingGrace => {
                persist_telegram_topic_binding_state(
                    state,
                    &conversation_key,
                    &thread_id,
                    expected_state.as_ref(),
                    next_state,
                    generation,
                    lifecycle_intent_revision,
                )
                .await;
                continue;
            }
            TelegramTopicLifecycle::Active => {}
        }

        if cleanup_queued
            || state
                .telegram_topic_cleanup_pending_for_account(&account_id)
                .await
        {
            persist_telegram_topic_binding_state(
                state,
                &conversation_key,
                &thread_id,
                expected_state.as_ref(),
                next_state,
                generation,
                lifecycle_intent_revision,
            )
            .await;
            continue;
        }

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
        // If both sides changed, Codex remains authoritative. Unchanged active
        // bindings make no Telegram API call; service messages already report
        // user-initiated Topic renames.
        let telegram_to_codex = !codex_changed && telegram_changed;
        let needs_topic_edit = should_edit_telegram_topic_name(
            telegram_to_codex,
            &current_topic_name,
            &target_topic_name,
        );
        if needs_topic_edit {
            let edit_result = run_telegram_topic_mutation_while(
                state,
                &account_id,
                None,
                || {
                    telegram_topic_reconcile_edit_is_current(
                        state,
                        &account_id,
                        &conversation_key,
                        &thread_id,
                        expected_state.as_ref(),
                        generation,
                    )
                },
                || {
                    run_telegram_topic_edit_with_marker(
                        state,
                        &conversation_key,
                        &target_topic_name,
                        || api.edit_forum_topic(&raw_chat_id, topic_id, &target_topic_name),
                    )
                },
            )
            .await;
            let Some(edit_result) = edit_result else {
                continue;
            };
            match edit_result {
                Ok(true) => {
                    next_state.topic_name = target_topic_name.clone();
                }
                Ok(false) => {
                    state
                        .push_event(
                            "warn",
                            "telegram_topic_sync_failed",
                            format!(
                                "chat={} topic={} api returned false",
                                route.chat_id, topic_id
                            ),
                        )
                        .await;
                    persist_telegram_topic_binding_state(
                        state,
                        &conversation_key,
                        &thread_id,
                        expected_state.as_ref(),
                        next_state,
                        generation,
                        lifecycle_intent_revision,
                    )
                    .await;
                    continue;
                }
                Err(err)
                    if err
                        .downcast_ref::<TelegramApiError>()
                        .is_some_and(TelegramApiError::is_forum_topic_missing) =>
                {
                    finish_telegram_topic_cleanup(
                        state,
                        &TelegramTopicCleanupTarget {
                            conversation_key,
                            route,
                            thread_id,
                            chat_id: raw_chat_id.clone(),
                            topic_id,
                            lifecycle_revision,
                        },
                        "telegram_topic_missing_during_reconcile",
                        true,
                    )
                    .await;
                    continue;
                }
                Err(err) => {
                    let rate_limited = err
                        .downcast_ref::<TelegramApiError>()
                        .is_some_and(TelegramApiError::is_rate_limited);
                    state
                        .push_event(
                            "warn",
                            "telegram_topic_sync_failed",
                            format!("chat={} topic={} err={err}", route.chat_id, topic_id),
                        )
                        .await;
                    persist_telegram_topic_binding_state(
                        state,
                        &conversation_key,
                        &thread_id,
                        expected_state.as_ref(),
                        next_state,
                        generation,
                        lifecycle_intent_revision,
                    )
                    .await;
                    if rate_limited {
                        break;
                    }
                    continue;
                }
            }
        }

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
        persist_telegram_topic_binding_state(
            state,
            &conversation_key,
            &thread_id,
            expected_state.as_ref(),
            next_state,
            generation,
            lifecycle_intent_revision,
        )
        .await;
    }
}

fn update_telegram_topic_lifecycle(
    state: &mut crate::store::TelegramTopicBindingState,
    thread_id: &str,
    active_ids: &HashSet<String>,
    archived_ids: &HashSet<String>,
    generation: u64,
    now: u128,
) -> TelegramTopicLifecycle {
    let previous_generation = state.lifecycle_generation;
    let previous_codex_state = state.codex_state.clone();
    let same_generation = state.lifecycle_generation == generation;
    state.lifecycle_generation = generation;
    state.last_checked_at_ms = now;
    let lifecycle = if same_generation && state.codex_state == "deleted" {
        state.archived_at_ms.get_or_insert(now);
        state.missing_at_ms = None;
        TelegramTopicLifecycle::Deleted
    } else if active_ids.contains(thread_id) {
        state.codex_state = "active".to_string();
        state.archived_at_ms = None;
        state.missing_at_ms = None;
        TelegramTopicLifecycle::Active
    } else if archived_ids.contains(thread_id) {
        state.codex_state = "archived".to_string();
        state.archived_at_ms.get_or_insert(now);
        state.missing_at_ms = None;
        TelegramTopicLifecycle::Archived
    } else {
        state.codex_state = "missing".to_string();
        state.archived_at_ms = None;
        let missing_at = state.missing_at_ms.get_or_insert(now);
        if now.saturating_sub(*missing_at) >= TELEGRAM_TOPIC_STATE_GRACE.as_millis() {
            TelegramTopicLifecycle::MissingDelete
        } else {
            TelegramTopicLifecycle::MissingGrace
        }
    };
    if previous_generation != generation || previous_codex_state != state.codex_state {
        state.lifecycle_revision = state.lifecycle_revision.saturating_add(1);
    }
    lifecycle
}

fn should_edit_telegram_topic_name(
    telegram_to_codex: bool,
    current_topic_name: &str,
    target_topic_name: &str,
) -> bool {
    !telegram_to_codex && current_topic_name != target_topic_name
}

async fn telegram_topic_reconcile_edit_is_current(
    state: &SharedState,
    account_id: &str,
    conversation_key: &str,
    thread_id: &str,
    expected: Option<&crate::store::TelegramTopicBindingState>,
    generation: u64,
) -> bool {
    let name_updates = state.telegram_topic_name_update_ops.lock().await;
    if name_updates.contains_key(conversation_key) {
        return false;
    }
    let _binding_guard = state.im_route_binding_ops.lock().await;
    let runtime = state.runtime.lock().await;
    if runtime.bridge_generation != generation {
        return false;
    }
    let persisted = state.persisted.lock().await;
    let binding_matches = persisted
        .im_thread_bindings
        .get(conversation_key)
        .is_some_and(|bound_thread_id| bound_thread_id == thread_id);
    let state_matches = persisted
        .telegram_topic_binding_states
        .get(conversation_key)
        == expected;
    drop(persisted);
    drop(runtime);
    drop(_binding_guard);
    drop(name_updates);

    binding_matches
        && state_matches
        && !state
            .telegram_topic_cleanup_pending_for_account(account_id)
            .await
}

#[derive(Debug)]
struct TelegramTopicBindingCommit {
    conversation_key: Option<String>,
    lifecycle_revision: u64,
}

async fn schedule_reconciled_telegram_topic_cleanup(
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

fn merge_telegram_topic_lifecycle_state(
    current: &mut crate::store::TelegramTopicBindingState,
    next: &crate::store::TelegramTopicBindingState,
) {
    let lifecycle_changed = current.lifecycle_generation != next.lifecycle_generation
        || current.codex_state != next.codex_state;
    let current_revision = current.lifecycle_revision;
    current.thread_id = next.thread_id.clone();
    current.codex_state = next.codex_state.clone();
    current.archived_at_ms = next.archived_at_ms;
    current.missing_at_ms = next.missing_at_ms;
    current.lifecycle_generation = next.lifecycle_generation;
    current.lifecycle_revision = if lifecycle_changed {
        current_revision
            .saturating_add(1)
            .max(next.lifecycle_revision)
    } else {
        current_revision.max(next.lifecycle_revision)
    };
    current.last_checked_at_ms = next.last_checked_at_ms;
}

async fn persist_telegram_topic_binding_state(
    state: &SharedState,
    conversation_key: &str,
    thread_id: &str,
    expected: Option<&crate::store::TelegramTopicBindingState>,
    binding: crate::store::TelegramTopicBindingState,
    generation: u64,
    expected_lifecycle_revision: u64,
) -> Option<TelegramTopicBindingCommit> {
    let lifecycle_intent = telegram_thread_lifecycle_intent_for_binding(&binding);
    let wake_cleanup = binding.codex_state == "active";
    let path = state.config.lock().await.state_path.clone();
    let _binding_guard = state.im_route_binding_ops.lock().await;
    let runtime = state.runtime.lock().await;
    if runtime.bridge_generation != generation {
        return None;
    }
    let mut persisted = state.persisted.lock().await;
    let exact_binding_matches = persisted
        .im_thread_bindings
        .get(conversation_key)
        .is_some_and(|bound_thread_id| bound_thread_id == thread_id);
    let exact_state_matches = persisted
        .telegram_topic_binding_states
        .get(conversation_key)
        == expected;
    let current_key = if exact_binding_matches && exact_state_matches {
        Some(conversation_key.to_string())
    } else if lifecycle_intent.is_some() {
        persisted
            .im_thread_bindings
            .iter()
            .find_map(|(key, bound_thread_id)| {
                (bound_thread_id == thread_id
                    && route_from_conversation_key(key)
                        .is_some_and(|route| route.platform == ImPlatformKind::Telegram))
                .then(|| key.clone())
            })
    } else {
        return None;
    };
    if let Some(current_key) = current_key.as_deref() {
        let current = persisted
            .telegram_topic_binding_states
            .get(current_key)
            .cloned()
            .unwrap_or_default();
        let moved = current_key != conversation_key;
        let conflicts_with_newer_lifecycle = current.lifecycle_generation
            > binding.lifecycle_generation
            || (current.lifecycle_generation == binding.lifecycle_generation
                && ((current.codex_state == "deleted" && binding.codex_state != "deleted")
                    || (binding.codex_state == "active"
                        && matches!(
                            current.codex_state.as_str(),
                            "archived" | "deleted" | "missing"
                        ))
                    || (!moved && current.codex_state != binding.codex_state)));
        if !(exact_binding_matches && exact_state_matches) && conflicts_with_newer_lifecycle {
            return None;
        }
    }
    if let Some(lifecycle_intent) = lifecycle_intent {
        if state
            .observe_telegram_thread_lifecycle_if_revision(
                thread_id,
                generation,
                expected_lifecycle_revision,
                lifecycle_intent,
            )
            .await
            .is_none()
        {
            return None;
        }
    }
    let Some(current_key) = current_key else {
        return Some(TelegramTopicBindingCommit {
            conversation_key: None,
            lifecycle_revision: binding.lifecycle_revision,
        });
    };
    let committed_binding = if exact_binding_matches && exact_state_matches {
        persisted
            .telegram_topic_binding_states
            .insert(current_key.clone(), binding.clone());
        binding
    } else {
        let current = persisted
            .telegram_topic_binding_states
            .entry(current_key.clone())
            .or_default();
        merge_telegram_topic_lifecycle_state(current, &binding);
        current.clone()
    };
    drop(runtime);
    if let Err(err) = persisted.save(&path) {
        chain_log::write_diagnostic_lazy(|| {
            format!("[telegram_topic] event=reconcile_save_failed err={err}")
        });
    }
    drop(persisted);
    drop(_binding_guard);
    if wake_cleanup {
        state
            .notify_telegram_topic_cleanup_if_older(
                &current_key,
                committed_binding.lifecycle_generation,
                committed_binding.lifecycle_revision,
            )
            .await;
    }
    Some(TelegramTopicBindingCommit {
        conversation_key: Some(current_key),
        lifecycle_revision: committed_binding.lifecycle_revision,
    })
}

fn telegram_thread_lifecycle_intent_for_binding(
    binding: &crate::store::TelegramTopicBindingState,
) -> Option<TelegramThreadLifecycleState> {
    match binding.codex_state.as_str() {
        "active" => Some(TelegramThreadLifecycleState::Active),
        "archived" => Some(TelegramThreadLifecycleState::Archived),
        "deleted" => Some(TelegramThreadLifecycleState::Deleted),
        "missing"
            if binding.missing_at_ms.is_some_and(|missing_at| {
                now_ms().saturating_sub(missing_at) >= TELEGRAM_TOPIC_STATE_GRACE.as_millis()
            }) =>
        {
            Some(TelegramThreadLifecycleState::Deleted)
        }
        _ => None,
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
    use reqwest::StatusCode;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;
    use crate::im::telegram::api::{
        TelegramChat, TelegramForumTopicEdited, TelegramMediaFile, TelegramPhotoSize,
        TelegramStickerFile, TelegramUser,
    };

    async fn blocking_successful_topic_api(
        account_id: &str,
        bot_token: &str,
    ) -> (
        TelegramApi,
        tokio::sync::oneshot::Receiver<String>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock Telegram server");
        let address = listener.local_addr().expect("mock Telegram address");
        let (request_tx, request_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept Topic request");
            let mut request = vec![0_u8; 4_096];
            let count = stream.read(&mut request).await.expect("read Topic request");
            let first_line = String::from_utf8_lossy(&request[..count])
                .lines()
                .next()
                .unwrap_or_default()
                .to_string();
            let _ = request_tx.send(first_line);
            let _ = release_rx.await;
            let body = r#"{"ok":true,"result":true}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("write Topic response");
        });
        let api = TelegramApi::new(TelegramSettings {
            account_id: account_id.to_string(),
            bot_token: bot_token.to_string(),
            ..Default::default()
        })
        .with_test_api_base(format!("http://{address}"));
        (api, request_rx, release_tx)
    }

    #[test]
    fn archived_topics_delete_immediately_while_missing_topics_keep_the_grace_period() {
        let mut binding = crate::store::TelegramTopicBindingState::default();
        let active_ids = HashSet::new();
        let archived_ids = HashSet::from(["thread-archived".to_string()]);
        assert_eq!(
            update_telegram_topic_lifecycle(
                &mut binding,
                "thread-archived",
                &active_ids,
                &archived_ids,
                1,
                1_000,
            ),
            TelegramTopicLifecycle::Archived
        );
        assert_eq!(binding.codex_state, "archived");
        assert_eq!(binding.archived_at_ms, Some(1_000));

        let mut missing = crate::store::TelegramTopicBindingState::default();
        assert_eq!(
            update_telegram_topic_lifecycle(
                &mut missing,
                "thread-missing",
                &active_ids,
                &HashSet::new(),
                1,
                2_000,
            ),
            TelegramTopicLifecycle::MissingGrace
        );
        assert_eq!(
            update_telegram_topic_lifecycle(
                &mut missing,
                "thread-missing",
                &active_ids,
                &HashSet::new(),
                1,
                2_000 + TELEGRAM_TOPIC_STATE_GRACE.as_millis(),
            ),
            TelegramTopicLifecycle::MissingDelete
        );
    }

    #[test]
    fn unchanged_active_topic_names_do_not_consume_mutation_rate_limits() {
        assert!(!should_edit_telegram_topic_name(
            false,
            "Same title",
            "Same title"
        ));
        assert!(should_edit_telegram_topic_name(
            false,
            "Old title",
            "New title"
        ));
        assert!(!should_edit_telegram_topic_name(
            true,
            "Telegram title",
            "Codex title"
        ));
    }

    #[tokio::test]
    async fn delayed_bot_name_echo_consumes_only_its_marker_without_regressing_newer_state() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let mut config = crate::config::AppConfig::default();
        config.state_path = temp_dir.path().join("state.json");
        let state = crate::app_state::AppState::new(
            temp_dir.path().join("config.toml"),
            config,
            None,
            None,
        );
        let api = TelegramApi::new(TelegramSettings {
            account_id: "bot".to_string(),
            bot_token: "token".to_string(),
            ..Default::default()
        });
        let conversation_key = "telegram:bot:-100|topic=42";
        let expected_binding = crate::store::TelegramTopicBindingState {
            thread_id: "thread-name".to_string(),
            codex_title: "A".to_string(),
            topic_name: "A".to_string(),
            last_synced_codex_title: "A".to_string(),
            last_synced_topic_name: "A".to_string(),
            last_checked_at_ms: 123,
            ..Default::default()
        };
        {
            let mut persisted = state.persisted.lock().await;
            persisted
                .im_thread_bindings
                .insert(conversation_key.to_string(), "thread-name".to_string());
            persisted
                .telegram_topic_binding_states
                .insert(conversation_key.to_string(), expected_binding.clone());
        }
        let old_b_token = install_telegram_topic_name_marker(&state, conversation_key, "B").await;
        let new_a_token = install_telegram_topic_name_marker(&state, conversation_key, "A").await;

        assert!(
            handle_forum_topic_service_message(
                &state,
                &api,
                &TelegramMessage {
                    message_id: 1,
                    message_thread_id: Some(42),
                    chat: TelegramChat {
                        id: -100,
                        kind: "supergroup".to_string(),
                        ..Default::default()
                    },
                    forum_topic_edited: Some(TelegramForumTopicEdited {
                        name: Some("B".to_string()),
                        icon_custom_emoji_id: None,
                    }),
                    ..Default::default()
                },
            )
            .await
        );

        let persisted = state.persisted.lock().await;
        let binding = &persisted.telegram_topic_binding_states[conversation_key];
        assert_eq!(binding, &expected_binding);
        drop(persisted);
        let markers = state.telegram_topic_name_sync_ops.lock().await;
        assert_eq!(markers[conversation_key].len(), 1);
        assert_eq!(markers[conversation_key][0].name, "A");
        assert_eq!(markers[conversation_key][0].token, new_a_token);
        assert_ne!(markers[conversation_key][0].token, old_b_token);
        drop(markers);
        assert!(
            !state
                .events
                .lock()
                .await
                .iter()
                .any(|event| event.kind == "telegram_topic_name_synced_to_codex")
        );
    }

    #[tokio::test]
    async fn delayed_telegram_to_codex_rpc_cannot_overwrite_a_newer_realtime_name() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let mut config = crate::config::AppConfig::default();
        config.state_path = temp_dir.path().join("state.json");
        let state = crate::app_state::AppState::new(
            temp_dir.path().join("config.toml"),
            config,
            None,
            None,
        );
        let conversation_key = "telegram:bot:-100|topic=45";
        {
            let mut persisted = state.persisted.lock().await;
            persisted
                .im_thread_bindings
                .insert(conversation_key.to_string(), "thread-name".to_string());
            persisted.telegram_topic_binding_states.insert(
                conversation_key.to_string(),
                crate::store::TelegramTopicBindingState {
                    thread_id: "thread-name".to_string(),
                    codex_title: "A".to_string(),
                    topic_name: "A".to_string(),
                    last_synced_codex_title: "A".to_string(),
                    last_synced_topic_name: "A".to_string(),
                    ..Default::default()
                },
            );
        }

        let telegram_b_token = state
            .begin_telegram_topic_name_update(conversation_key)
            .await;
        assert_eq!(
            record_telegram_topic_name_for_codex_sync(
                &state,
                conversation_key,
                "open",
                "B",
                telegram_b_token,
            )
            .await
            .as_deref(),
            Some("thread-name")
        );

        let realtime_c_token = state
            .begin_telegram_topic_name_update(conversation_key)
            .await;
        {
            let mut persisted = state.persisted.lock().await;
            let binding = persisted
                .telegram_topic_binding_states
                .get_mut(conversation_key)
                .expect("binding");
            binding.codex_title = "C".to_string();
            binding.topic_name = "C".to_string();
            binding.last_synced_codex_title = "C".to_string();
            binding.last_synced_topic_name = "C".to_string();
        }
        state
            .finish_telegram_topic_name_update(conversation_key, realtime_c_token)
            .await;

        assert!(
            !commit_telegram_topic_name_to_codex_if_current(
                &state,
                conversation_key,
                "thread-name",
                "B",
                telegram_b_token,
            )
            .await
        );
        let persisted = state.persisted.lock().await;
        let binding = &persisted.telegram_topic_binding_states[conversation_key];
        assert_eq!(binding.codex_title, "C");
        assert_eq!(binding.topic_name, "C");
        assert_eq!(binding.last_synced_codex_title, "C");
        assert_eq!(binding.last_synced_topic_name, "C");
    }

    #[tokio::test]
    async fn clearing_one_failed_marker_preserves_another_marker_with_the_same_name() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let mut config = crate::config::AppConfig::default();
        config.state_path = temp_dir.path().join("state.json");
        let state = crate::app_state::AppState::new(
            temp_dir.path().join("config.toml"),
            config,
            None,
            None,
        );
        let conversation_key = "telegram:bot:-100|topic=43";
        let first = install_telegram_topic_name_marker(&state, conversation_key, "Same").await;
        let second = install_telegram_topic_name_marker(&state, conversation_key, "Same").await;

        clear_telegram_topic_name_marker(&state, conversation_key, second).await;

        let markers = state.telegram_topic_name_sync_ops.lock().await;
        assert_eq!(markers[conversation_key].len(), 1);
        assert_eq!(markers[conversation_key][0].token, first);
    }

    #[tokio::test]
    async fn topic_not_modified_clears_its_echo_marker_immediately() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let mut config = crate::config::AppConfig::default();
        config.state_path = temp_dir.path().join("state.json");
        let state = crate::app_state::AppState::new(
            temp_dir.path().join("config.toml"),
            config,
            None,
            None,
        );
        let conversation_key = "telegram:bot:-100|topic=44";

        assert!(
            run_telegram_topic_edit_with_marker(&state, conversation_key, "Same", || async {
                Ok(TelegramForumTopicEditOutcome::NotModified)
            })
            .await
            .expect("unchanged topic is a confirmed probe")
        );
        assert!(
            !state
                .telegram_topic_name_sync_ops
                .lock()
                .await
                .contains_key(conversation_key)
        );
    }

    #[test]
    fn topic_name_marker_ttl_outlasts_the_polling_conflict_backoff() {
        assert!(
            TELEGRAM_TOPIC_NAME_MARKER_TTL > Duration::from_secs(TELEGRAM_CONFLICT_BACKOFF_SECONDS)
        );
    }

    #[test]
    fn topic_delete_retry_honors_server_retry_after_and_caps_generic_backoff() {
        let server_error = anyhow::Error::new(TelegramApiError {
            method: "deleteForumTopic".to_string(),
            status: StatusCode::TOO_MANY_REQUESTS,
            error_code: Some(429),
            description: "Too Many Requests".to_string(),
            retry_after: Some(43),
        });
        assert_eq!(
            telegram_topic_delete_retry_delay(Some(&server_error), 1),
            Duration::from_secs(43)
        );
        let zero_retry = anyhow::Error::new(TelegramApiError {
            method: "deleteForumTopic".to_string(),
            status: StatusCode::TOO_MANY_REQUESTS,
            error_code: Some(429),
            description: "Too Many Requests".to_string(),
            retry_after: Some(0),
        });
        assert_eq!(
            telegram_topic_delete_retry_delay(Some(&zero_retry), 1),
            Duration::from_secs(1)
        );
        assert_eq!(
            telegram_topic_delete_retry_delay(None, 1),
            Duration::from_secs(5)
        );
        assert_eq!(
            telegram_topic_delete_retry_delay(None, 10),
            Duration::from_secs(60)
        );

        let forbidden = anyhow::Error::new(TelegramApiError {
            method: "deleteForumTopic".to_string(),
            status: StatusCode::FORBIDDEN,
            error_code: Some(403),
            description: "not enough rights".to_string(),
            retry_after: None,
        });
        assert!(!telegram_topic_delete_should_retry(&forbidden));
        let bad_request = anyhow::Error::new(TelegramApiError {
            method: "deleteForumTopic".to_string(),
            status: StatusCode::BAD_REQUEST,
            error_code: Some(400),
            description: "chat not found".to_string(),
            retry_after: None,
        });
        assert!(!telegram_topic_delete_should_retry(&bad_request));
        let server_failure = anyhow::Error::new(TelegramApiError {
            method: "deleteForumTopic".to_string(),
            status: StatusCode::BAD_GATEWAY,
            error_code: Some(502),
            description: "bad gateway".to_string(),
            retry_after: None,
        });
        assert!(telegram_topic_delete_should_retry(&server_failure));
        let http_timeout = anyhow::Error::new(TelegramApiError {
            method: "deleteForumTopic".to_string(),
            status: StatusCode::REQUEST_TIMEOUT,
            error_code: Some(400),
            description: "request timeout".to_string(),
            retry_after: None,
        });
        assert!(telegram_topic_delete_should_retry(&http_timeout));
        let api_timeout = anyhow::Error::new(TelegramApiError {
            method: "deleteForumTopic".to_string(),
            status: StatusCode::OK,
            error_code: Some(408),
            description: "request timeout".to_string(),
            retry_after: None,
        });
        assert!(telegram_topic_delete_should_retry(&api_timeout));
        let api_server_failure = anyhow::Error::new(TelegramApiError {
            method: "deleteForumTopic".to_string(),
            status: StatusCode::OK,
            error_code: Some(500),
            description: "internal error".to_string(),
            retry_after: None,
        });
        assert!(telegram_topic_delete_should_retry(&api_server_failure));
        assert!(telegram_topic_delete_should_retry(&anyhow!(
            "network unavailable"
        )));
    }

    #[tokio::test]
    async fn every_topic_mutation_records_retry_after_before_releasing_the_gate() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let mut config = crate::config::AppConfig::default();
        config.state_path = temp_dir.path().join("state.json");
        let state = crate::app_state::AppState::new(
            temp_dir.path().join("config.toml"),
            config,
            None,
            None,
        );
        let observed_at = Instant::now();

        let error = run_telegram_topic_mutation(&state, "account", || async {
            Err::<(), _>(anyhow::Error::new(TelegramApiError {
                method: "editForumTopic".to_string(),
                status: StatusCode::TOO_MANY_REQUESTS,
                error_code: Some(429),
                description: "Too Many Requests".to_string(),
                retry_after: Some(43),
            }))
        })
        .await
        .expect_err("rate limit is returned to the caller");
        assert!(error.to_string().contains("Too Many Requests"));
        assert!(
            state
                .telegram_topic_cleanup_retry_deadline("account")
                .await
                .is_some_and(|deadline| deadline >= observed_at + Duration::from_secs(43))
        );

        let fallback_at = Instant::now();
        let _ = run_telegram_topic_mutation(&state, "fallback", || async {
            Err::<(), _>(anyhow::Error::new(TelegramApiError {
                method: "createForumTopic".to_string(),
                status: StatusCode::TOO_MANY_REQUESTS,
                error_code: Some(429),
                description: "Too Many Requests".to_string(),
                retry_after: None,
            }))
        })
        .await;
        assert!(
            state
                .telegram_topic_cleanup_retry_deadline("fallback")
                .await
                .is_some_and(|deadline| {
                    deadline
                        >= fallback_at + Duration::from_secs(TELEGRAM_TOPIC_DELETE_RETRY_SECONDS)
                })
        );
    }

    #[tokio::test]
    async fn final_delete_attempt_still_records_its_account_cooldown() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let mut config = crate::config::AppConfig::default();
        config.state_path = temp_dir.path().join("state.json");
        let state = crate::app_state::AppState::new(
            temp_dir.path().join("config.toml"),
            config,
            None,
            None,
        );
        let observed_at = Instant::now();

        let error = retry_telegram_topic_delete(
            1,
            || {
                run_telegram_topic_mutation_while(
                    &state,
                    "account",
                    None,
                    || std::future::ready(true),
                    || async {
                        Err::<bool, _>(anyhow::Error::new(TelegramApiError {
                            method: "deleteForumTopic".to_string(),
                            status: StatusCode::TOO_MANY_REQUESTS,
                            error_code: Some(429),
                            description: "Too Many Requests".to_string(),
                            retry_after: Some(17),
                        }))
                    },
                )
            },
            |_| std::future::ready(()),
        )
        .await
        .expect_err("final rate-limited attempt is returned");

        assert!(error.to_string().contains("Too Many Requests"));
        assert!(
            state
                .telegram_topic_cleanup_retry_deadline("account")
                .await
                .is_some_and(|deadline| deadline >= observed_at + Duration::from_secs(17))
        );
    }

    #[tokio::test]
    async fn generic_delete_retry_backoff_does_not_hold_the_account_gate() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let mut config = crate::config::AppConfig::default();
        config.state_path = temp_dir.path().join("state.json");
        let state = crate::app_state::AppState::new(
            temp_dir.path().join("config.toml"),
            config,
            None,
            None,
        );
        let attempts = std::cell::Cell::new(0_usize);

        let outcome = retry_telegram_topic_delete(
            2,
            || {
                let attempt = attempts.get() + 1;
                attempts.set(attempt);
                run_telegram_topic_mutation_while(
                    &state,
                    "account",
                    None,
                    || std::future::ready(true),
                    move || async move { Ok(attempt > 1) },
                )
            },
            |_| async {
                let gate = state.telegram_topic_mutation_gate("account").await;
                assert!(gate.try_lock().is_ok());
            },
        )
        .await
        .expect("second attempt confirms deletion");

        assert_eq!(outcome, TelegramTopicDeleteOutcome::Deleted);
        assert_eq!(attempts.get(), 2);
    }

    #[tokio::test]
    async fn topic_mutation_waits_for_cooldown_without_holding_the_account_gate() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let mut config = crate::config::AppConfig::default();
        config.state_path = temp_dir.path().join("state.json");
        let state = crate::app_state::AppState::new(
            temp_dir.path().join("config.toml"),
            config,
            None,
            None,
        );
        state
            .extend_telegram_topic_cleanup_retry_deadline("account", Duration::from_millis(80))
            .await;
        let executed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let task = tokio::spawn({
            let state = state.clone();
            let executed = executed.clone();
            async move {
                run_telegram_topic_mutation(&state, "account", || async move {
                    executed.store(true, std::sync::atomic::Ordering::SeqCst);
                    Ok(())
                })
                .await
            }
        });
        tokio::task::yield_now().await;

        let gate = state.telegram_topic_mutation_gate("account").await;
        let guard = tokio::time::timeout(Duration::from_millis(30), gate.lock())
            .await
            .expect("cooldown waiter does not hold mutation gate");
        tokio::time::sleep(Duration::from_millis(90)).await;
        state
            .extend_telegram_topic_cleanup_retry_deadline("account", Duration::from_millis(80))
            .await;
        drop(guard);

        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!executed.load(std::sync::atomic::Ordering::SeqCst));
        tokio::time::timeout(Duration::from_millis(250), task)
            .await
            .expect("mutation observes the extended deadline")
            .expect("mutation task")
            .expect("mutation succeeds");
        assert!(executed.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn cleanup_cancels_while_queued_for_the_account_mutation_gate() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let mut config = crate::config::AppConfig::default();
        config.state_path = temp_dir.path().join("state.json");
        let state = crate::app_state::AppState::new(
            temp_dir.path().join("config.toml"),
            config,
            None,
            None,
        );
        let gate = state.telegram_topic_mutation_gate("account").await;
        let guard = gate.lock().await;
        let should_continue = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let executed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let notifier = Arc::new(tokio::sync::Notify::new());
        let task = tokio::spawn({
            let state = state.clone();
            let should_continue = should_continue.clone();
            let executed = executed.clone();
            let notifier = notifier.clone();
            async move {
                run_telegram_topic_mutation_while(
                    &state,
                    "account",
                    Some(&notifier),
                    || {
                        std::future::ready(
                            should_continue.load(std::sync::atomic::Ordering::SeqCst),
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
        should_continue.store(false, std::sync::atomic::Ordering::SeqCst);
        notifier.notify_one();

        let result = tokio::time::timeout(Duration::from_millis(100), task)
            .await
            .expect("lifecycle notification cancels gate wait")
            .expect("mutation task");
        assert!(result.is_none());
        assert!(!executed.load(std::sync::atomic::Ordering::SeqCst));
        drop(guard);
    }

    #[tokio::test]
    async fn topic_delete_retry_loop_waits_full_retry_after_before_success() {
        let attempts = std::cell::Cell::new(0_usize);
        let delays = std::cell::RefCell::new(Vec::new());

        let outcome = retry_telegram_topic_delete(
            3,
            || {
                let attempt = attempts.get() + 1;
                attempts.set(attempt);
                std::future::ready(Some(if attempt == 1 {
                    Err(anyhow::Error::new(TelegramApiError {
                        method: "deleteForumTopic".to_string(),
                        status: StatusCode::TOO_MANY_REQUESTS,
                        error_code: Some(429),
                        description: "Too Many Requests".to_string(),
                        retry_after: Some(43),
                    }))
                } else {
                    Ok(true)
                }))
            },
            |retry| {
                delays.borrow_mut().push(retry.delay);
                std::future::ready(())
            },
        )
        .await
        .expect("delete succeeds after retry");

        assert_eq!(outcome, TelegramTopicDeleteOutcome::Deleted);
        assert_eq!(attempts.get(), 2);
        assert_eq!(*delays.borrow(), vec![Duration::from_secs(43)]);
    }

    #[tokio::test]
    async fn topic_delete_retry_loop_is_bounded_and_stops_on_permanent_error() {
        let false_attempts = std::cell::Cell::new(0_usize);
        let false_waits = std::cell::Cell::new(0_usize);
        let error = retry_telegram_topic_delete(
            3,
            || {
                false_attempts.set(false_attempts.get() + 1);
                std::future::ready(Some(Ok(false)))
            },
            |_| {
                false_waits.set(false_waits.get() + 1);
                std::future::ready(())
            },
        )
        .await
        .expect_err("false result remains unconfirmed");
        assert!(error.to_string().contains("after 3 attempts"));
        assert_eq!(false_attempts.get(), 3);
        assert_eq!(false_waits.get(), 2);

        let permanent_attempts = std::cell::Cell::new(0_usize);
        let permanent_waits = std::cell::Cell::new(0_usize);
        let error = retry_telegram_topic_delete(
            3,
            || {
                permanent_attempts.set(permanent_attempts.get() + 1);
                std::future::ready(Some(Err(anyhow::Error::new(TelegramApiError {
                    method: "deleteForumTopic".to_string(),
                    status: StatusCode::FORBIDDEN,
                    error_code: Some(403),
                    description: "not enough rights".to_string(),
                    retry_after: None,
                }))))
            },
            |_| {
                permanent_waits.set(permanent_waits.get() + 1);
                std::future::ready(())
            },
        )
        .await
        .expect_err("permanent error is returned immediately");
        assert!(error.to_string().contains("not enough rights"));
        assert_eq!(permanent_attempts.get(), 1);
        assert_eq!(permanent_waits.get(), 0);
    }

    #[tokio::test]
    async fn unarchive_notification_interrupts_a_topic_delete_retry_delay() {
        let notifier = tokio::sync::Notify::new();
        notifier.notify_one();

        tokio::time::timeout(
            Duration::from_millis(100),
            wait_for_telegram_topic_delete_retry(Duration::from_secs(60), Some(&notifier)),
        )
        .await
        .expect("stored notification cancels retry wait");
    }

    #[tokio::test]
    async fn cleanup_worker_replays_a_newer_archive_revision_without_teardown_aba() {
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
        let route = RouteTarget {
            platform: ImPlatformKind::Telegram,
            conversation_key: "telegram:telegram:-100|topic=47".to_string(),
            account_id: "telegram".to_string(),
            chat_id: "-100|topic=47".to_string(),
            remote_client_key: "im:telegram:test".to_string(),
        };
        {
            let mut persisted = state.persisted.lock().await;
            persisted.im_thread_bindings.insert(
                route.conversation_key.clone(),
                "thread-rearchive".to_string(),
            );
            persisted.telegram_topic_binding_states.insert(
                route.conversation_key.clone(),
                crate::store::TelegramTopicBindingState {
                    thread_id: "thread-rearchive".to_string(),
                    codex_state: "archived".to_string(),
                    lifecycle_generation: generation,
                    lifecycle_revision: 2,
                    ..Default::default()
                },
            );
        }
        let notifier = Arc::new(tokio::sync::Notify::new());
        state
            .telegram_topic_cleanup_registrations
            .lock()
            .await
            .insert(
                route.conversation_key.clone(),
                TelegramTopicCleanupRegistration {
                    token: 41,
                    lifecycle_generation: generation,
                    lifecycle_revision: 1,
                    notifier: notifier.clone(),
                },
            );
        let target = TelegramTopicCleanupTarget {
            conversation_key: route.conversation_key.clone(),
            route,
            thread_id: "thread-rearchive".to_string(),
            chat_id: "-100".to_string(),
            topic_id: 47,
            lifecycle_revision: 1,
        };

        assert_eq!(
            finish_telegram_topic_cleanup_worker_iteration(&state, &target, 41, generation, 1,)
                .await,
            Some((generation, 2))
        );
        {
            let registrations = state.telegram_topic_cleanup_registrations.lock().await;
            let registration = &registrations[&target.conversation_key];
            assert_eq!(registration.token, 41);
            assert_eq!(registration.lifecycle_revision, 2);
        }

        state
            .telegram_topic_cleanup_registrations
            .lock()
            .await
            .insert(
                target.conversation_key.clone(),
                TelegramTopicCleanupRegistration {
                    token: 42,
                    lifecycle_generation: generation,
                    lifecycle_revision: 3,
                    notifier,
                },
            );
        assert_eq!(
            finish_telegram_topic_cleanup_worker_iteration(&state, &target, 41, generation, 2,)
                .await,
            None
        );
        assert_eq!(
            state.telegram_topic_cleanup_registrations.lock().await[&target.conversation_key].token,
            42
        );
    }

    #[tokio::test]
    async fn rearchive_notification_does_not_bypass_an_account_retry_after_deadline() {
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
        let route = RouteTarget {
            platform: ImPlatformKind::Telegram,
            conversation_key: "telegram:telegram:-100|topic=48".to_string(),
            account_id: "telegram".to_string(),
            chat_id: "-100|topic=48".to_string(),
            remote_client_key: "im:telegram:test".to_string(),
        };
        {
            let mut persisted = state.persisted.lock().await;
            persisted.im_thread_bindings.insert(
                route.conversation_key.clone(),
                "thread-retry-after".to_string(),
            );
            persisted.telegram_topic_binding_states.insert(
                route.conversation_key.clone(),
                crate::store::TelegramTopicBindingState {
                    thread_id: "thread-retry-after".to_string(),
                    codex_state: "archived".to_string(),
                    lifecycle_generation: generation,
                    lifecycle_revision: 3,
                    ..Default::default()
                },
            );
        }
        state
            .extend_telegram_topic_cleanup_retry_deadline("telegram", Duration::from_secs(60))
            .await;
        let notifier = tokio::sync::Notify::new();
        let target = TelegramTopicCleanupTarget {
            conversation_key: route.conversation_key.clone(),
            route,
            thread_id: "thread-retry-after".to_string(),
            chat_id: "-100".to_string(),
            topic_id: 48,
            lifecycle_revision: 3,
        };

        notifier.notify_one();
        let mut wait = Box::pin(wait_for_telegram_topic_cleanup_retry_deadline(
            &state, &target, generation, 3, &notifier,
        ));
        assert!(
            tokio::time::timeout(Duration::from_millis(25), &mut wait)
                .await
                .is_err(),
            "a rearchive wake must keep waiting for the Retry-After deadline"
        );

        {
            let mut persisted = state.persisted.lock().await;
            let binding = persisted
                .telegram_topic_binding_states
                .get_mut(&target.conversation_key)
                .expect("binding");
            binding.codex_state = "active".to_string();
            binding.lifecycle_revision = 4;
        }
        notifier.notify_one();
        assert!(
            !tokio::time::timeout(Duration::from_millis(250), &mut wait)
                .await
                .expect("active lifecycle wakes the cooldown wait")
        );

        {
            let mut persisted = state.persisted.lock().await;
            let binding = persisted
                .telegram_topic_binding_states
                .get_mut(&target.conversation_key)
                .expect("binding");
            binding.codex_state = "archived".to_string();
            binding.lifecycle_revision = 5;
        }
        let mut rearchive_target = target.clone();
        rearchive_target.lifecycle_revision = 5;
        notifier.notify_one();
        assert!(
            tokio::time::timeout(
                Duration::from_millis(25),
                wait_for_telegram_topic_cleanup_retry_deadline(
                    &state,
                    &rearchive_target,
                    generation,
                    5,
                    &notifier,
                ),
            )
            .await
            .is_err(),
            "the replacement worker must honor the remaining account deadline"
        );
    }

    #[tokio::test]
    async fn archive_unarchive_and_delete_notifications_update_bound_topic_state() {
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
        let route = RouteTarget {
            platform: ImPlatformKind::Telegram,
            conversation_key: "telegram:telegram:-100|topic=42".to_string(),
            account_id: "telegram".to_string(),
            chat_id: "-100|topic=42".to_string(),
            remote_client_key: "im:telegram:test".to_string(),
        };
        state
            .runtime
            .lock()
            .await
            .bind_route("thread-42", route.clone());
        {
            let mut persisted = state.persisted.lock().await;
            persisted
                .im_thread_bindings
                .insert(route.conversation_key.clone(), "thread-42".to_string());
            persisted.telegram_topic_binding_states.insert(
                route.conversation_key.clone(),
                crate::store::TelegramTopicBindingState {
                    thread_id: "thread-42".to_string(),
                    topic_name: "Topic 42".to_string(),
                    ..Default::default()
                },
            );
        }

        archive_telegram_topic_for_codex_thread(
            &state,
            &ImApiRegistry::default(),
            "thread-42",
            generation,
        )
        .await;
        assert_eq!(
            state.persisted.lock().await.telegram_topic_binding_states[&route.conversation_key]
                .codex_state,
            "archived"
        );

        unarchive_telegram_topic_for_codex_thread(&state, "thread-42", generation).await;
        assert_eq!(
            state.persisted.lock().await.telegram_topic_binding_states[&route.conversation_key]
                .codex_state,
            "active"
        );

        delete_telegram_topic_for_codex_thread(
            &state,
            &ImApiRegistry::default(),
            "thread-42",
            generation,
        )
        .await;
        assert_eq!(
            state.persisted.lock().await.telegram_topic_binding_states[&route.conversation_key]
                .codex_state,
            "deleted"
        );
        unarchive_telegram_topic_for_codex_thread(&state, "thread-42", generation).await;
        assert_eq!(
            state.persisted.lock().await.telegram_topic_binding_states[&route.conversation_key]
                .codex_state,
            "deleted"
        );

        archive_telegram_topic_for_codex_thread(
            &state,
            &ImApiRegistry::default(),
            "thread-42",
            generation,
        )
        .await;
        assert_eq!(
            state.persisted.lock().await.telegram_topic_binding_states[&route.conversation_key]
                .codex_state,
            "deleted"
        );
    }

    #[tokio::test]
    async fn stale_generation_cannot_commit_or_overwrite_topic_lifecycle() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let mut config = crate::config::AppConfig::default();
        config.state_path = temp_dir.path().join("state.json");
        let state = crate::app_state::AppState::new(
            temp_dir.path().join("config.toml"),
            config,
            None,
            None,
        );
        let first_generation = state.runtime.lock().await.start_bridge_generation();
        let route = RouteTarget {
            platform: ImPlatformKind::Telegram,
            conversation_key: "telegram:telegram:-100|topic=45".to_string(),
            account_id: "telegram".to_string(),
            chat_id: "-100|topic=45".to_string(),
            remote_client_key: "im:telegram:test".to_string(),
        };
        state
            .runtime
            .lock()
            .await
            .bind_route("thread-generation", route.clone());
        {
            let mut persisted = state.persisted.lock().await;
            persisted.im_thread_bindings.insert(
                route.conversation_key.clone(),
                "thread-generation".to_string(),
            );
            persisted.telegram_topic_binding_states.insert(
                route.conversation_key.clone(),
                crate::store::TelegramTopicBindingState {
                    thread_id: "thread-generation".to_string(),
                    ..Default::default()
                },
            );
        }
        let current_generation = state.runtime.lock().await.start_bridge_generation();

        archive_telegram_topic_for_codex_thread(
            &state,
            &ImApiRegistry::default(),
            "thread-generation",
            first_generation,
        )
        .await;
        assert_eq!(
            state.persisted.lock().await.telegram_topic_binding_states[&route.conversation_key]
                .codex_state,
            "active"
        );

        archive_telegram_topic_for_codex_thread(
            &state,
            &ImApiRegistry::default(),
            "thread-generation",
            current_generation,
        )
        .await;
        unarchive_telegram_topic_for_codex_thread(&state, "thread-generation", current_generation)
            .await;
        archive_telegram_topic_for_codex_thread(
            &state,
            &ImApiRegistry::default(),
            "thread-generation",
            first_generation,
        )
        .await;

        let persisted = state.persisted.lock().await;
        let binding = &persisted.telegram_topic_binding_states[&route.conversation_key];
        assert_eq!(binding.codex_state, "active");
        assert_eq!(binding.lifecycle_generation, current_generation);
    }

    #[test]
    fn deleted_topic_state_is_not_revived_by_a_stale_active_snapshot() {
        let mut binding = crate::store::TelegramTopicBindingState {
            thread_id: "thread-deleted".to_string(),
            codex_state: "deleted".to_string(),
            ..Default::default()
        };
        assert_eq!(
            update_telegram_topic_lifecycle(
                &mut binding,
                "thread-deleted",
                &HashSet::from(["thread-deleted".to_string()]),
                &HashSet::new(),
                0,
                4_000,
            ),
            TelegramTopicLifecycle::Deleted
        );
        assert_eq!(binding.codex_state, "deleted");
    }

    #[tokio::test]
    async fn reconciliation_cannot_overwrite_a_newer_archive_marker() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let mut config = crate::config::AppConfig::default();
        config.state_path = temp_dir.path().join("state.json");
        let state = crate::app_state::AppState::new(
            temp_dir.path().join("config.toml"),
            config,
            None,
            None,
        );
        let conversation_key = "telegram:telegram:-100|topic=43";
        let expected = crate::store::TelegramTopicBindingState {
            thread_id: "thread-43".to_string(),
            topic_name: "Topic 43".to_string(),
            ..Default::default()
        };
        let archived = crate::store::TelegramTopicBindingState {
            codex_state: "archived".to_string(),
            archived_at_ms: Some(2_000),
            last_checked_at_ms: 2_000,
            ..expected.clone()
        };
        {
            let mut persisted = state.persisted.lock().await;
            persisted
                .im_thread_bindings
                .insert(conversation_key.to_string(), "thread-43".to_string());
            persisted
                .telegram_topic_binding_states
                .insert(conversation_key.to_string(), archived.clone());
        }

        let wrote = persist_telegram_topic_binding_state(
            &state,
            conversation_key,
            "thread-43",
            Some(&expected),
            expected.clone(),
            0,
            0,
        )
        .await;

        assert!(wrote.is_none());
        assert_eq!(
            state.persisted.lock().await.telegram_topic_binding_states[conversation_key],
            archived
        );

        let stale_archived = archived.clone();
        let restored_active = crate::store::TelegramTopicBindingState {
            codex_state: "active".to_string(),
            archived_at_ms: None,
            last_checked_at_ms: 3_000,
            ..expected.clone()
        };
        state
            .persisted
            .lock()
            .await
            .telegram_topic_binding_states
            .insert(conversation_key.to_string(), restored_active.clone());
        let wrote = persist_telegram_topic_binding_state(
            &state,
            conversation_key,
            "thread-43",
            Some(&stale_archived),
            stale_archived.clone(),
            0,
            0,
        )
        .await;
        assert!(wrote.is_none());
        assert_eq!(
            state.persisted.lock().await.telegram_topic_binding_states[conversation_key],
            restored_active
        );
    }

    #[tokio::test]
    async fn archived_reconciliation_commits_a_thread_lifecycle_tombstone() {
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
        let conversation_key = "telegram:telegram:-100|topic=49";
        let active = crate::store::TelegramTopicBindingState {
            thread_id: "thread-reconcile-archive".to_string(),
            lifecycle_generation: generation,
            ..Default::default()
        };
        let archived = crate::store::TelegramTopicBindingState {
            codex_state: "archived".to_string(),
            archived_at_ms: Some(now_ms()),
            lifecycle_revision: 1,
            ..active.clone()
        };
        {
            let mut persisted = state.persisted.lock().await;
            persisted.im_thread_bindings.insert(
                conversation_key.to_string(),
                "thread-reconcile-archive".to_string(),
            );
            persisted
                .telegram_topic_binding_states
                .insert(conversation_key.to_string(), active.clone());
        }

        assert!(
            persist_telegram_topic_binding_state(
                &state,
                conversation_key,
                "thread-reconcile-archive",
                Some(&active),
                archived,
                generation,
                0,
            )
            .await
            .is_some()
        );
        assert!(
            !state
                .telegram_thread_allows_topic_binding("thread-reconcile-archive", generation)
                .await
        );
        state
            .persisted
            .lock()
            .await
            .im_thread_bindings
            .remove(conversation_key);
        assert!(
            !state
                .observe_telegram_thread_started("thread-reconcile-archive", generation)
                .await
        );
    }

    #[tokio::test]
    async fn archived_reconciliation_hands_cross_account_rebinding_to_the_current_api() {
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
        let thread_id = "thread-reconcile-rebound";
        assert!(
            state
                .observe_telegram_thread_started(thread_id, generation)
                .await
        );
        let old_route = RouteTarget {
            platform: ImPlatformKind::Telegram,
            conversation_key: "telegram:account-a:-100|topic=61".to_string(),
            account_id: "account-a".to_string(),
            chat_id: "-100|topic=61".to_string(),
            remote_client_key: "im:telegram:account-a".to_string(),
        };
        let new_route = RouteTarget {
            platform: ImPlatformKind::Telegram,
            conversation_key: "telegram:account-b:-100|topic=62".to_string(),
            account_id: "account-b".to_string(),
            chat_id: "-100|topic=62".to_string(),
            remote_client_key: "im:telegram:account-b".to_string(),
        };
        bind_thread_to_route_for_generation(
            &state,
            &old_route,
            thread_id,
            None,
            old_route.remote_client_key.clone(),
            Some(generation),
        )
        .await
        .expect("bind old Topic");
        let expected = state.persisted.lock().await.telegram_topic_binding_states
            [&old_route.conversation_key]
            .clone();
        let expected_lifecycle_revision = state
            .telegram_thread_lifecycle_revision(thread_id, generation)
            .await
            .expect("lifecycle snapshot");
        let mut archived = expected.clone();
        assert_eq!(
            update_telegram_topic_lifecycle(
                &mut archived,
                thread_id,
                &HashSet::new(),
                &HashSet::from([thread_id.to_string()]),
                generation,
                4_000,
            ),
            TelegramTopicLifecycle::Archived
        );

        let (query_started_tx, query_started_rx) = tokio::sync::oneshot::channel();
        let (release_query_tx, release_query_rx) = tokio::sync::oneshot::channel();
        let commit_task = tokio::spawn({
            let state = state.clone();
            let old_key = old_route.conversation_key.clone();
            let expected = expected.clone();
            async move {
                let _ = query_started_tx.send(());
                let _ = release_query_rx.await;
                persist_telegram_topic_binding_state(
                    &state,
                    &old_key,
                    thread_id,
                    Some(&expected),
                    archived,
                    generation,
                    expected_lifecycle_revision,
                )
                .await
            }
        });
        query_started_rx.await.expect("query started");
        bind_thread_to_route_for_generation(
            &state,
            &new_route,
            thread_id,
            None,
            new_route.remote_client_key.clone(),
            Some(generation),
        )
        .await
        .expect("rebind while query is in flight");
        {
            let mut persisted = state.persisted.lock().await;
            let rebound = persisted
                .telegram_topic_binding_states
                .get_mut(&new_route.conversation_key)
                .expect("new Topic binding");
            rebound.topic_name = "New Topic Name".to_string();
            rebound.last_synced_topic_name = "New Topic Name".to_string();
        }
        release_query_tx.send(()).expect("release query");
        let commit = commit_task
            .await
            .expect("commit task")
            .expect("unchanged lifecycle token commits");

        assert_eq!(
            commit.conversation_key.as_deref(),
            Some(new_route.conversation_key.as_str())
        );
        let persisted = state.persisted.lock().await;
        assert!(
            !persisted
                .im_thread_bindings
                .contains_key(&old_route.conversation_key)
        );
        let rebound = &persisted.telegram_topic_binding_states[&new_route.conversation_key];
        assert_eq!(rebound.codex_state, "archived");
        assert_eq!(rebound.topic_name, "New Topic Name");
        assert_eq!(rebound.last_synced_topic_name, "New Topic Name");
        drop(persisted);
        assert!(
            !state
                .telegram_thread_allows_topic_binding(thread_id, generation)
                .await
        );

        let (api_a, request_a, release_a) =
            blocking_successful_topic_api("account-a", "account-a-token").await;
        let (api_b, request_b, release_b) =
            blocking_successful_topic_api("account-b", "account-b-token").await;
        let mut api_registry = ImApiRegistry::default();
        api_registry.telegram.insert("account-a".to_string(), api_a);
        api_registry.telegram.insert("account-b".to_string(), api_b);
        assert!(
            schedule_reconciled_telegram_topic_cleanup(
                &state,
                &api_registry,
                commit,
                thread_id.to_string(),
                generation,
            )
            .await
        );
        let (selected_account, request_line) =
            tokio::time::timeout(Duration::from_secs(1), async move {
                tokio::select! {
                    request = request_a => ("account-a", request.expect("account A request")),
                    request = request_b => ("account-b", request.expect("account B request")),
                }
            })
            .await
            .expect("one account receives the delete request");
        assert_eq!(selected_account, "account-b");
        assert!(request_line.contains("/botaccount-b-token/deleteForumTopic"));
        assert!(
            state
                .telegram_topic_cleanup_registrations
                .lock()
                .await
                .contains_key(&new_route.conversation_key),
            "the cross-account cleanup remains registered until B responds"
        );
        release_b.send(()).expect("release account B response");
        let _ = release_a.send(());
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let binding_exists = state
                    .persisted
                    .lock()
                    .await
                    .im_thread_bindings
                    .contains_key(&new_route.conversation_key);
                let cleanup_registered = state
                    .telegram_topic_cleanup_registrations
                    .lock()
                    .await
                    .contains_key(&new_route.conversation_key);
                if !binding_exists && !cleanup_registered {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("account B cleanup finishes");
    }

    #[tokio::test]
    async fn archived_reconciliation_cannot_overwrite_unarchive_during_the_query() {
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
        let thread_id = "thread-reconcile-unarchived";
        assert!(
            state
                .observe_telegram_thread_started(thread_id, generation)
                .await
        );
        let route = RouteTarget {
            platform: ImPlatformKind::Telegram,
            conversation_key: "telegram:telegram:-100|topic=63".to_string(),
            account_id: "telegram".to_string(),
            chat_id: "-100|topic=63".to_string(),
            remote_client_key: "im:telegram:test".to_string(),
        };
        bind_thread_to_route_for_generation(
            &state,
            &route,
            thread_id,
            None,
            route.remote_client_key.clone(),
            Some(generation),
        )
        .await
        .expect("bind Topic");
        assert_eq!(
            mark_telegram_topic_bindings_for_cleanup(&state, thread_id, "archived", generation,)
                .await
                .len(),
            1
        );
        let expected = state.persisted.lock().await.telegram_topic_binding_states
            [&route.conversation_key]
            .clone();
        let expected_lifecycle_revision = state
            .telegram_thread_lifecycle_revision(thread_id, generation)
            .await
            .expect("lifecycle snapshot");
        let archived = expected.clone();

        let (query_started_tx, query_started_rx) = tokio::sync::oneshot::channel();
        let (release_query_tx, release_query_rx) = tokio::sync::oneshot::channel();
        let commit_task = tokio::spawn({
            let state = state.clone();
            let conversation_key = route.conversation_key.clone();
            async move {
                let _ = query_started_tx.send(());
                let _ = release_query_rx.await;
                persist_telegram_topic_binding_state(
                    &state,
                    &conversation_key,
                    thread_id,
                    Some(&expected),
                    archived,
                    generation,
                    expected_lifecycle_revision,
                )
                .await
            }
        });
        query_started_rx.await.expect("query started");
        unarchive_telegram_topic_for_codex_thread(&state, thread_id, generation).await;
        release_query_tx.send(()).expect("release query");
        assert!(
            commit_task.await.expect("commit task").is_none(),
            "stale archive snapshot must fail its lifecycle token check"
        );

        let persisted = state.persisted.lock().await;
        assert_eq!(
            persisted.telegram_topic_binding_states[&route.conversation_key].codex_state,
            "active"
        );
        drop(persisted);
        assert!(
            state
                .telegram_thread_allows_topic_binding(thread_id, generation)
                .await
        );
    }

    #[tokio::test]
    async fn active_reconciliation_cancels_a_queued_archive_cleanup() {
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
        let route = RouteTarget {
            platform: ImPlatformKind::Telegram,
            conversation_key: "telegram:telegram:-100|topic=46".to_string(),
            account_id: "telegram".to_string(),
            chat_id: "-100|topic=46".to_string(),
            remote_client_key: "im:telegram:test".to_string(),
        };
        let archived = crate::store::TelegramTopicBindingState {
            thread_id: "thread-active-again".to_string(),
            codex_state: "archived".to_string(),
            archived_at_ms: Some(2_000),
            lifecycle_generation: generation,
            lifecycle_revision: 1,
            last_checked_at_ms: 2_000,
            ..Default::default()
        };
        {
            let mut persisted = state.persisted.lock().await;
            persisted.im_thread_bindings.insert(
                route.conversation_key.clone(),
                "thread-active-again".to_string(),
            );
            persisted
                .telegram_topic_binding_states
                .insert(route.conversation_key.clone(), archived.clone());
        }
        let cleanup_notifier = Arc::new(tokio::sync::Notify::new());
        state
            .telegram_topic_cleanup_registrations
            .lock()
            .await
            .insert(
                route.conversation_key.clone(),
                TelegramTopicCleanupRegistration {
                    token: 51,
                    lifecycle_generation: generation,
                    lifecycle_revision: archived.lifecycle_revision,
                    notifier: cleanup_notifier.clone(),
                },
            );
        let mut active = archived.clone();
        assert_eq!(
            update_telegram_topic_lifecycle(
                &mut active,
                "thread-active-again",
                &HashSet::from(["thread-active-again".to_string()]),
                &HashSet::new(),
                generation,
                3_000,
            ),
            TelegramTopicLifecycle::Active
        );
        let active_revision = active.lifecycle_revision;
        assert!(
            persist_telegram_topic_binding_state(
                &state,
                &route.conversation_key,
                "thread-active-again",
                Some(&archived),
                active,
                generation,
                0,
            )
            .await
            .is_some()
        );
        tokio::time::timeout(Duration::from_millis(250), cleanup_notifier.notified())
            .await
            .expect("active reconciliation wakes the cleanup worker");
        let target = TelegramTopicCleanupTarget {
            conversation_key: route.conversation_key.clone(),
            route,
            thread_id: "thread-active-again".to_string(),
            chat_id: "-100".to_string(),
            topic_id: 46,
            lifecycle_revision: active_revision,
        };
        assert!(
            !telegram_topic_cleanup_is_required(
                &state,
                &target,
                generation,
                target.lifecycle_revision,
            )
            .await
        );
        assert_eq!(
            finish_telegram_topic_cleanup_worker_iteration(
                &state,
                &target,
                51,
                generation,
                archived.lifecycle_revision,
            )
            .await,
            None
        );
        assert!(
            !state
                .telegram_topic_cleanup_registrations
                .lock()
                .await
                .contains_key(&target.conversation_key)
        );
    }

    #[tokio::test]
    async fn successful_remote_delete_clears_a_binding_even_after_unarchive() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let mut config = crate::config::AppConfig::default();
        config.state_path = temp_dir.path().join("state.json");
        let state = crate::app_state::AppState::new(
            temp_dir.path().join("config.toml"),
            config,
            None,
            None,
        );
        let route = RouteTarget {
            platform: ImPlatformKind::Telegram,
            conversation_key: "telegram:telegram:-100|topic=44".to_string(),
            account_id: "telegram".to_string(),
            chat_id: "-100|topic=44".to_string(),
            remote_client_key: "im:telegram:test".to_string(),
        };
        state
            .runtime
            .lock()
            .await
            .bind_route("thread-44", route.clone());
        {
            let mut persisted = state.persisted.lock().await;
            persisted
                .im_thread_bindings
                .insert(route.conversation_key.clone(), "thread-44".to_string());
            persisted.telegram_topic_binding_states.insert(
                route.conversation_key.clone(),
                crate::store::TelegramTopicBindingState {
                    thread_id: "thread-44".to_string(),
                    codex_state: "active".to_string(),
                    ..Default::default()
                },
            );
        }
        let target = TelegramTopicCleanupTarget {
            conversation_key: route.conversation_key.clone(),
            route,
            thread_id: "thread-44".to_string(),
            chat_id: "-100".to_string(),
            topic_id: 44,
            lifecycle_revision: 0,
        };

        finish_telegram_topic_cleanup(&state, &target, "remote_delete_succeeded", false).await;

        assert!(
            !state
                .persisted
                .lock()
                .await
                .im_thread_bindings
                .contains_key(&target.conversation_key)
        );
    }

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

    #[tokio::test]
    async fn auto_topic_resume_retries_until_new_rollout_is_visible() {
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
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

        let result = retry_auto_topic_resume(
            &state,
            "thread-new",
            generation,
            Duration::ZERO,
            || {
                let attempts = attempts.clone();
                async move {
                    let attempt = attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if attempt == 0 {
                        Err(anyhow!(
                            "remote-control request failed: no rollout found for thread id thread-new"
                        ))
                    } else {
                        Ok(serde_json::json!({"thread": {"id": "thread-new"}}))
                    }
                }
            },
        )
        .await
        .expect("second resume should succeed");

        assert_eq!(result["thread"]["id"], "thread-new");
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn auto_topic_resume_stops_when_bridge_generation_changes() {
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
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

        let error =
            retry_auto_topic_resume(&state, "thread-stale", generation, Duration::ZERO, || {
                let state = state.clone();
                let attempts = attempts.clone();
                async move {
                    attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    state.runtime.lock().await.invalidate_bridge_generation();
                    Err(anyhow!(
                        "remote-control request failed: no rollout found for thread id thread-stale"
                    ))
                }
            })
            .await
            .expect_err("stale generation should cancel the retry");

        assert!(error.to_string().contains("bridge generation changed"));
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn auto_topic_resume_retries_only_missing_rollouts_within_the_attempt_budget() {
        let missing_rollout = anyhow!("no rollout found for thread id thread-new");
        assert!(should_retry_auto_topic_resume(&missing_rollout, 1));
        assert!(should_retry_auto_topic_resume(
            &missing_rollout,
            AUTO_TOPIC_RESUME_MAX_ATTEMPTS - 1
        ));
        assert!(!should_retry_auto_topic_resume(
            &missing_rollout,
            AUTO_TOPIC_RESUME_MAX_ATTEMPTS
        ));
        assert!(!should_retry_auto_topic_resume(
            &anyhow!("permission denied"),
            1
        ));
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

    #[tokio::test]
    async fn auto_topic_created_after_archive_is_cleaned_instead_of_bound() {
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
        let thread_id = "thread-create-archive";
        assert!(
            state
                .observe_telegram_thread_started(thread_id, generation)
                .await
        );
        let (create_started_tx, create_started_rx) = tokio::sync::oneshot::channel();
        let (release_create_tx, release_create_rx) = tokio::sync::oneshot::channel();
        let create_task = tokio::spawn({
            let state = state.clone();
            async move {
                run_telegram_topic_mutation_while(
                    &state,
                    "bot",
                    None,
                    || telegram_topic_creation_is_current(&state, thread_id, generation),
                    || async move {
                        let _ = create_started_tx.send(());
                        let _ = release_create_rx.await;
                        Ok::<_, anyhow::Error>(77_i64)
                    },
                )
                .await
            }
        });
        create_started_rx.await.expect("create API started");
        state
            .observe_telegram_thread_lifecycle(
                thread_id,
                generation,
                TelegramThreadLifecycleState::Archived,
            )
            .await;
        release_create_tx.send(()).expect("release create API");
        let topic_id = create_task
            .await
            .expect("create task")
            .expect("request began")
            .expect("create response");
        let cleaned = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let retained =
            keep_auto_created_topic_if_current(&state, thread_id, generation, topic_id, {
                let cleaned = cleaned.clone();
                move |topic_id| async move {
                    assert_eq!(topic_id, 77);
                    cleaned.store(true, std::sync::atomic::Ordering::SeqCst);
                }
            })
            .await;

        assert!(retained.is_none());
        assert!(cleaned.load(std::sync::atomic::Ordering::SeqCst));
        assert!(state.runtime.lock().await.route_by_thread.is_empty());
        assert!(state.persisted.lock().await.im_thread_bindings.is_empty());
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
