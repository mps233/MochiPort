//! Automatic Telegram Topic creation helpers for Codex threads.
//!
//! Moved verbatim out of `polling.rs`; no behavior changes.

use super::*;

pub(super) async fn is_current_bridge_generation(state: &SharedState, generation: u64) -> bool {
    state.runtime.lock().await.is_bridge_generation(generation)
}

pub(super) async fn telegram_topic_creation_is_current(
    state: &SharedState,
    thread_id: &str,
    generation: u64,
) -> bool {
    is_current_bridge_generation(state, generation).await
        && state
            .telegram_thread_allows_topic_binding(thread_id, generation)
            .await
}

pub(super) async fn keep_auto_created_topic_if_current<T, C, CleanupFuture>(
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

pub(super) async fn resume_auto_topic_thread(
    state: &SharedState,
    remote_client_key: &str,
    thread_id: &str,
    rollout_path: Option<&str>,
    generation: u64,
    connection_epoch: Option<u64>,
) -> Result<Value> {
    let remote_client_key = remote_client_key.to_string();
    let thread_id_for_resume = thread_id.to_string();
    let rollout_path = rollout_path.map(str::to_string);
    retry_auto_topic_resume(
        state,
        thread_id,
        generation,
        AUTO_TOPIC_RESUME_RETRY_DELAY,
        || {
            let state = state.clone();
            let remote_client_key = remote_client_key.clone();
            let thread_id = thread_id_for_resume.clone();
            let rollout_path = rollout_path.clone();
            async move {
                match connection_epoch {
                    Some(connection_epoch) => {
                        remote_control_backend::resume_thread_for_client_on_connection_with_path(
                            &state,
                            connection_epoch,
                            &remote_client_key,
                            &thread_id,
                            rollout_path.as_deref(),
                            true,
                        )
                        .await
                    }
                    None => {
                        remote_control_backend::resume_thread_for_client_with_path(
                            &state,
                            &remote_client_key,
                            &thread_id,
                            rollout_path.as_deref(),
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

pub(super) async fn retry_auto_topic_resume<F, Fut>(
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

pub(super) fn should_retry_auto_topic_resume(err: &anyhow::Error, attempt: usize) -> bool {
    attempt < AUTO_TOPIC_RESUME_MAX_ATTEMPTS
        && err.to_string().contains("no rollout found for thread id")
}

#[derive(Clone)]
pub(super) struct AutoTopicTarget {
    pub(super) account_id: String,
    pub(super) chat_id: String,
    pub(super) api: TelegramApi,
}

pub(super) async fn find_auto_topic_target(
    state: &SharedState,
    api_registry: &ImApiRegistry,
    thread_cwd: &str,
) -> Option<AutoTopicTarget> {
    let accounts = state.config.lock().await.telegram_accounts.clone();
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

pub(super) async fn started_thread_metadata(
    state: &SharedState,
    remote_client_key: &str,
    connection_epoch: Option<u64>,
    params: &Value,
    thread_id: &str,
) -> Option<(String, String, Option<String>)> {
    let thread_value = params.get("thread").unwrap_or(params);
    let mut cwd = started_thread_field(params, thread_value, &["cwd", "workingDirectory"]);
    let mut title = started_thread_field(params, thread_value, &["name", "title", "threadName"]);
    let mut rollout_path = started_thread_path_field(params, thread_value);

    if cwd.is_none() || title.is_none() || rollout_path.is_none() {
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
            rollout_path = rollout_path.or_else(|| thread_rollout_path(thread));
        }
    }

    let title = title.unwrap_or_else(|| "未命名会话".to_string());
    Some((cwd.unwrap_or_default(), title, rollout_path))
}

pub(super) fn started_thread_id(params: &Value) -> Option<String> {
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

pub(super) fn started_thread_field(
    params: &Value,
    thread: &Value,
    keys: &[&str],
) -> Option<String> {
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

fn started_thread_path_field(params: &Value, thread: &Value) -> Option<String> {
    ["path", "rolloutPath", "rollout_path"]
        .iter()
        .find_map(|key| {
            thread
                .get(*key)
                .or_else(|| params.get(*key))
                .and_then(thread_path_value)
        })
}

fn thread_rollout_path(thread: &Value) -> Option<String> {
    ["path", "rolloutPath", "rollout_path"]
        .iter()
        .find_map(|key| thread.get(*key).and_then(thread_path_value))
}

fn thread_path_value(value: &Value) -> Option<String> {
    if let Some(path) = value.as_str() {
        let path = path.trim();
        return (!path.is_empty()).then(|| path.to_string());
    }
    if let Some(values) = value.as_array() {
        return values.iter().find_map(thread_path_value);
    }
    let object = value.as_object()?;
    ["path", "value", "text", "uri"]
        .iter()
        .find_map(|key| object.get(*key).and_then(thread_path_value))
}

pub(super) fn cwd_is_within_project(thread_cwd: &str, project_cwd: &str) -> bool {
    let thread_cwd = thread_cwd.trim();
    let project_cwd = project_cwd.trim();
    if thread_cwd.is_empty() || project_cwd.is_empty() {
        return false;
    }
    let thread_path = Path::new(thread_cwd);
    let project_path = Path::new(project_cwd);
    thread_path == project_path || thread_path.starts_with(project_path)
}

pub(super) async fn existing_binding_for_thread(
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

pub(super) async fn persist_auto_topic_name(
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

pub(super) async fn delete_auto_created_topic(
    state: &SharedState,
    api: &TelegramApi,
    chat_id: &str,
    topic_id: i64,
    thread_id: &str,
) {
    let context = format!("auto_create thread={thread_id}");
    match delete_forum_topic_with_retry(state, api, chat_id, topic_id, &context).await {
        Ok(()) => {
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
