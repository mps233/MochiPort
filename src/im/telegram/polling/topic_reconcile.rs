//! Telegram Topic name synchronization and lifecycle reconciliation helpers.
//!
//! Moved verbatim out of `polling.rs`; no behavior changes.

use super::topic_cleanup::{
    TelegramTopicCleanupTarget, finish_telegram_topic_cleanup,
    schedule_reconciled_telegram_topic_cleanup,
};
use super::*;

pub(super) async fn persist_telegram_topic_service_state(
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

pub(super) async fn record_telegram_topic_name_for_codex_sync(
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

pub(super) async fn commit_telegram_topic_name_to_codex_if_current(
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

pub(super) async fn consume_telegram_topic_name_marker(
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TelegramTopicLifecycle {
    Active,
    Archived,
    Deleted,
    MissingGrace,
    MissingDelete,
}

pub(super) fn apply_binding_lifecycle(
    binding: &mut crate::store::TelegramTopicBindingState,
    lifecycle: TelegramTopicLifecycle,
    generation: u64,
    now: u128,
) -> bool {
    let previous_generation = binding.lifecycle_generation;
    let previous_codex_state = binding.codex_state.clone();
    let codex_state = match lifecycle {
        TelegramTopicLifecycle::Active => "active",
        TelegramTopicLifecycle::Archived => "archived",
        TelegramTopicLifecycle::Deleted => "deleted",
        TelegramTopicLifecycle::MissingGrace | TelegramTopicLifecycle::MissingDelete => "missing",
    };
    binding.codex_state.clear();
    binding.codex_state.push_str(codex_state);
    match lifecycle {
        TelegramTopicLifecycle::Active => {
            binding.archived_at_ms = None;
            binding.missing_at_ms = None;
        }
        TelegramTopicLifecycle::Archived | TelegramTopicLifecycle::Deleted => {
            binding.archived_at_ms.get_or_insert(now);
            binding.missing_at_ms = None;
        }
        TelegramTopicLifecycle::MissingGrace | TelegramTopicLifecycle::MissingDelete => {
            binding.archived_at_ms = None;
            binding.missing_at_ms.get_or_insert(now);
        }
    }
    binding.lifecycle_generation = generation;
    binding.last_checked_at_ms = now;
    previous_generation != generation || previous_codex_state != binding.codex_state
}

pub(super) async fn reconcile_telegram_topic_bindings(
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

pub(super) fn update_telegram_topic_lifecycle(
    state: &mut crate::store::TelegramTopicBindingState,
    thread_id: &str,
    active_ids: &HashSet<String>,
    archived_ids: &HashSet<String>,
    generation: u64,
    now: u128,
) -> TelegramTopicLifecycle {
    let same_generation = state.lifecycle_generation == generation;
    let lifecycle = if same_generation && state.codex_state == "deleted" {
        TelegramTopicLifecycle::Deleted
    } else if active_ids.contains(thread_id) {
        TelegramTopicLifecycle::Active
    } else if archived_ids.contains(thread_id) {
        TelegramTopicLifecycle::Archived
    } else {
        let missing_at = state.missing_at_ms.unwrap_or(now);
        if now.saturating_sub(missing_at) >= TELEGRAM_TOPIC_STATE_GRACE.as_millis() {
            TelegramTopicLifecycle::MissingDelete
        } else {
            TelegramTopicLifecycle::MissingGrace
        }
    };
    if apply_binding_lifecycle(state, lifecycle, generation, now) {
        state.lifecycle_revision = state.lifecycle_revision.saturating_add(1);
    }
    lifecycle
}

pub(super) fn should_edit_telegram_topic_name(
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
pub(super) struct TelegramTopicBindingCommit {
    pub(super) conversation_key: Option<String>,
    pub(super) lifecycle_revision: u64,
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

pub(super) async fn persist_telegram_topic_binding_state(
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
        state
            .observe_telegram_thread_lifecycle_if_revision(
                thread_id,
                generation,
                expected_lifecycle_revision,
                lifecycle_intent,
            )
            .await?;
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
