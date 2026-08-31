use anyhow::{Result, anyhow};

use crate::{
    app_state::SharedState,
    im_runtime::{RouteTarget, ThreadSettingsSnapshot},
    remote_control_backend,
    types::ImPlatformKind,
};

pub(crate) async fn create_and_bind_thread(
    state: &SharedState,
    route: &RouteTarget,
    options: remote_control_backend::ThreadStartOptions,
    request_id: Option<&str>,
) -> Result<String> {
    let remote_client_key = route.remote_client_key.clone();
    let thread_id =
        remote_control_backend::start_thread_for_client(state, &remote_client_key, options).await?;
    bind_thread_to_route(state, route, &thread_id, request_id, remote_client_key).await?;
    Ok(thread_id)
}

pub(crate) async fn resume_and_bind_thread(
    state: &SharedState,
    route: &RouteTarget,
    thread_id: &str,
    request_id: Option<&str>,
) -> Result<serde_json::Value> {
    let remote_client_key = route.remote_client_key.clone();
    let response = remote_control_backend::resume_thread_for_client(
        state,
        &remote_client_key,
        thread_id,
        true,
    )
    .await?;
    state.runtime.lock().await.observe_thread_settings(
        thread_id,
        ThreadSettingsSnapshot::from_protocol_value(&response),
    );
    bind_thread_to_route(state, route, thread_id, request_id, remote_client_key).await?;
    Ok(response
        .get("thread")
        .cloned()
        .unwrap_or(serde_json::Value::Null))
}

pub(crate) async fn bind_thread_to_route(
    state: &SharedState,
    route: &RouteTarget,
    thread_id: &str,
    request_id: Option<&str>,
    remote_client_key: String,
) -> Result<()> {
    bind_thread_to_route_for_generation(
        state,
        route,
        thread_id,
        request_id,
        remote_client_key,
        None,
    )
    .await
}

pub(crate) async fn bind_thread_to_route_for_generation(
    state: &SharedState,
    route: &RouteTarget,
    thread_id: &str,
    request_id: Option<&str>,
    remote_client_key: String,
    expected_generation: Option<u64>,
) -> Result<()> {
    let state_path = state.config.lock().await.state_path.clone();
    let _binding_guard = state.im_route_binding_ops.lock().await;
    let mut lifecycle_generation = state.runtime.lock().await.bridge_generation;
    if expected_generation.is_some_and(|expected| lifecycle_generation != expected) {
        return Err(anyhow!(
            "bridge generation changed before binding Telegram Topic for {}",
            thread_id
        ));
    }
    let telegram_topic_route = route.platform == ImPlatformKind::Telegram
        && crate::types::split_telegram_message_target(&route.chat_id)
            .1
            .is_some();
    if telegram_topic_route {
        if !state
            .telegram_thread_allows_topic_binding(thread_id, lifecycle_generation)
            .await
        {
            return Err(anyhow!(
                "Telegram Topic binding is blocked by the current Codex thread lifecycle for {}",
                thread_id
            ));
        }
        let cleanup_worker_pending = state
            .telegram_topic_cleanup_registrations
            .lock()
            .await
            .contains_key(&route.conversation_key);
        let cleanup_state_pending = state
            .persisted
            .lock()
            .await
            .telegram_topic_binding_states
            .get(&route.conversation_key)
            .is_some_and(|binding| {
                matches!(
                    binding.codex_state.as_str(),
                    "archived" | "deleted" | "missing"
                )
            });
        if cleanup_worker_pending || cleanup_state_pending {
            return Err(anyhow!(
                "Telegram Topic cleanup is still in progress for {}",
                route.conversation_key
            ));
        }
    }
    let mut runtime = state.runtime.lock().await;
    if expected_generation.is_some_and(|expected| runtime.bridge_generation != expected)
        || (telegram_topic_route && runtime.bridge_generation != lifecycle_generation)
    {
        return Err(anyhow!(
            "bridge generation changed while binding Telegram Topic for {}",
            thread_id
        ));
    }
    lifecycle_generation = runtime.bridge_generation;
    runtime.unbind_routes_for_conversation_with_reason(
        &route.conversation_key,
        "bind_thread_to_route",
    );
    let mut runtime_route = route.clone();
    runtime_route.remote_client_key = remote_client_key;
    runtime.bind_route(thread_id, runtime_route);
    if let Some(request_id) = request_id {
        runtime.clear_thread_routing_request(request_id);
    }

    let mut persisted = state.persisted.lock().await;
    let previous_bindings = persisted.im_thread_bindings.clone();
    let previous_topic_states = persisted.telegram_topic_binding_states.clone();
    persisted
        .im_thread_bindings
        .retain(|_, bound_thread_id| bound_thread_id != thread_id);
    persisted
        .telegram_topic_binding_states
        .retain(|_, binding| binding.thread_id != thread_id);
    if route.platform == ImPlatformKind::Telegram {
        persisted
            .im_thread_bindings
            .insert(route.conversation_key.clone(), thread_id.to_string());
        let mut binding_state = crate::store::TelegramTopicBindingState {
            thread_id: thread_id.to_string(),
            lifecycle_generation,
            ..Default::default()
        };
        if crate::types::split_telegram_message_target(&route.chat_id)
            .1
            .is_none()
        {
            binding_state.telegram_state = "open".to_string();
        }
        persisted
            .telegram_topic_binding_states
            .insert(route.conversation_key.clone(), binding_state);
    }
    if persisted.im_thread_bindings != previous_bindings
        || persisted.telegram_topic_binding_states != previous_topic_states
    {
        persisted.save(&state_path)?;
    }
    drop(persisted);
    drop(runtime);
    Ok(())
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::{
        app_state::{AppState, TelegramThreadLifecycleState},
        config::AppConfig,
        store::PersistedState,
        types::ImPlatformKind,
    };

    #[tokio::test]
    async fn persisted_binding_tracks_latest_owner_of_same_thread() {
        let temp_dir = tempdir().expect("temp dir");
        let mut config = AppConfig::default();
        config.state_path = temp_dir.path().join("state.json");
        let state = AppState::new(
            temp_dir.path().join("config.toml"),
            config.clone(),
            None,
            None,
        );
        let first_route = RouteTarget {
            platform: ImPlatformKind::Telegram,
            conversation_key: "telegram:bot:41".to_string(),
            account_id: "bot".to_string(),
            chat_id: "41".to_string(),
            remote_client_key: "im:telegram:first".to_string(),
        };
        let second_route = RouteTarget {
            platform: ImPlatformKind::Telegram,
            conversation_key: "telegram:bot:42".to_string(),
            account_id: "bot".to_string(),
            chat_id: "42".to_string(),
            remote_client_key: "im:telegram:second".to_string(),
        };

        bind_thread_to_route(
            &state,
            &first_route,
            "thread-shared",
            None,
            first_route.remote_client_key.clone(),
        )
        .await
        .expect("bind first route");
        bind_thread_to_route(
            &state,
            &second_route,
            "thread-shared",
            None,
            second_route.remote_client_key.clone(),
        )
        .await
        .expect("move thread to second route");

        let runtime = state.runtime.lock().await;
        assert_eq!(runtime.route_by_thread.len(), 1);
        assert_eq!(
            runtime
                .route_by_thread
                .get("thread-shared")
                .map(|route| route.conversation_key.as_str()),
            Some("telegram:bot:42")
        );
        drop(runtime);

        let expected = std::collections::HashMap::from([(
            "telegram:bot:42".to_string(),
            "thread-shared".to_string(),
        )]);
        assert_eq!(state.persisted.lock().await.im_thread_bindings, expected);
        assert_eq!(
            PersistedState::load(&config.state_path).im_thread_bindings,
            expected
        );

        let feishu_route = RouteTarget {
            platform: ImPlatformKind::Feishu,
            conversation_key: "feishu:bot:chat".to_string(),
            account_id: "bot".to_string(),
            chat_id: "chat".to_string(),
            remote_client_key: "im:feishu:chat".to_string(),
        };
        bind_thread_to_route(
            &state,
            &feishu_route,
            "thread-shared",
            None,
            feishu_route.remote_client_key.clone(),
        )
        .await
        .expect("move thread to non-Telegram route");

        assert_eq!(
            state
                .runtime
                .lock()
                .await
                .route_by_thread
                .get("thread-shared")
                .map(|route| route.conversation_key.as_str()),
            Some("feishu:bot:chat")
        );
        assert!(state.persisted.lock().await.im_thread_bindings.is_empty());
        assert!(
            PersistedState::load(&config.state_path)
                .im_thread_bindings
                .is_empty()
        );
    }

    #[tokio::test]
    async fn telegram_topic_cannot_be_rebound_while_cleanup_is_pending() {
        let temp_dir = tempdir().expect("temp dir");
        let mut config = AppConfig::default();
        config.state_path = temp_dir.path().join("state.json");
        let state = AppState::new(temp_dir.path().join("config.toml"), config, None, None);
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
            .bind_route("thread-old", route.clone());
        {
            let mut persisted = state.persisted.lock().await;
            persisted
                .im_thread_bindings
                .insert(route.conversation_key.clone(), "thread-old".to_string());
            persisted.telegram_topic_binding_states.insert(
                route.conversation_key.clone(),
                crate::store::TelegramTopicBindingState {
                    thread_id: "thread-old".to_string(),
                    codex_state: "archived".to_string(),
                    ..Default::default()
                },
            );
        }

        let error = bind_thread_to_route(
            &state,
            &route,
            "thread-new",
            None,
            route.remote_client_key.clone(),
        )
        .await
        .expect_err("durable cleanup marker rejects rebind");
        assert!(error.to_string().contains("cleanup is still in progress"));
        assert!(
            state
                .runtime
                .lock()
                .await
                .route_by_thread
                .contains_key("thread-old")
        );

        state
            .persisted
            .lock()
            .await
            .telegram_topic_binding_states
            .get_mut(&route.conversation_key)
            .expect("binding state")
            .codex_state = "active".to_string();
        state
            .telegram_topic_cleanup_registrations
            .lock()
            .await
            .insert(
                route.conversation_key.clone(),
                crate::app_state::TelegramTopicCleanupRegistration {
                    token: 1,
                    lifecycle_generation: 0,
                    lifecycle_revision: 1,
                    notifier: std::sync::Arc::new(tokio::sync::Notify::new()),
                },
            );
        assert!(
            bind_thread_to_route(
                &state,
                &route,
                "thread-new",
                None,
                route.remote_client_key.clone(),
            )
            .await
            .is_err(),
            "in-flight cleanup worker rejects rebind"
        );

        state
            .telegram_topic_cleanup_registrations
            .lock()
            .await
            .remove(&route.conversation_key);
        bind_thread_to_route(
            &state,
            &route,
            "thread-new",
            None,
            route.remote_client_key.clone(),
        )
        .await
        .expect("active Topic can be rebound after cleanup cancellation");
        assert!(
            state
                .runtime
                .lock()
                .await
                .route_by_thread
                .contains_key("thread-new")
        );
    }

    #[tokio::test]
    async fn telegram_topic_binding_requires_active_lifecycle_and_expected_generation() {
        let temp_dir = tempdir().expect("temp dir");
        let mut config = AppConfig::default();
        config.state_path = temp_dir.path().join("state.json");
        let state = AppState::new(temp_dir.path().join("config.toml"), config, None, None);
        let generation = state.runtime.lock().await.start_bridge_generation();
        let route = RouteTarget {
            platform: ImPlatformKind::Telegram,
            conversation_key: "telegram:bot:-100|topic=77".to_string(),
            account_id: "bot".to_string(),
            chat_id: "-100|topic=77".to_string(),
            remote_client_key: "im:telegram:owner".to_string(),
        };
        assert!(
            state
                .observe_telegram_thread_started("thread-lifecycle", generation)
                .await
        );
        state
            .observe_telegram_thread_lifecycle(
                "thread-lifecycle",
                generation,
                TelegramThreadLifecycleState::Archived,
            )
            .await;

        let lifecycle_error = bind_thread_to_route_for_generation(
            &state,
            &route,
            "thread-lifecycle",
            None,
            route.remote_client_key.clone(),
            Some(generation),
        )
        .await
        .expect_err("archived thread cannot bind a Topic");
        assert!(lifecycle_error.to_string().contains("lifecycle"));

        let current_generation = state.runtime.lock().await.start_bridge_generation();
        assert!(
            state
                .observe_telegram_thread_started("thread-lifecycle", current_generation)
                .await
        );
        let generation_error = bind_thread_to_route_for_generation(
            &state,
            &route,
            "thread-lifecycle",
            None,
            route.remote_client_key.clone(),
            Some(generation),
        )
        .await
        .expect_err("stale creator cannot bind into a newer bridge generation");
        assert!(generation_error.to_string().contains("generation changed"));
        assert!(state.runtime.lock().await.route_by_thread.is_empty());
        assert!(state.persisted.lock().await.im_thread_bindings.is_empty());
    }

    #[tokio::test]
    async fn generation_cannot_roll_over_during_runtime_and_persisted_binding_commit() {
        let temp_dir = tempdir().expect("temp dir");
        let mut config = AppConfig::default();
        config.state_path = temp_dir.path().join("state.json");
        let state = AppState::new(temp_dir.path().join("config.toml"), config, None, None);
        let generation = state.runtime.lock().await.start_bridge_generation();
        let route = RouteTarget {
            platform: ImPlatformKind::Telegram,
            conversation_key: "telegram:bot:42".to_string(),
            account_id: "bot".to_string(),
            chat_id: "42".to_string(),
            remote_client_key: "im:telegram:private".to_string(),
        };
        let persisted_guard = state.persisted.lock().await;
        let bind_task = tokio::spawn({
            let state = state.clone();
            let route = route.clone();
            async move {
                bind_thread_to_route_for_generation(
                    &state,
                    &route,
                    "thread-atomic-bind",
                    None,
                    route.remote_client_key.clone(),
                    Some(generation),
                )
                .await
            }
        });
        let mut runtime_is_held = false;
        for _ in 0..100 {
            if state.runtime.try_lock().is_err() {
                runtime_is_held = true;
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(
            runtime_is_held,
            "bind reached the atomic runtime/persisted commit"
        );
        let rollover_task = tokio::spawn({
            let state = state.clone();
            async move { state.runtime.lock().await.start_bridge_generation() }
        });
        tokio::task::yield_now().await;
        assert!(
            !rollover_task.is_finished(),
            "bridge generation must wait until persisted binding commit finishes"
        );

        drop(persisted_guard);
        bind_task
            .await
            .expect("bind task")
            .expect("binding commits before rollover");
        assert_eq!(rollover_task.await.expect("rollover task"), generation + 1);
        let persisted = state.persisted.lock().await;
        assert_eq!(
            persisted.im_thread_bindings.get(&route.conversation_key),
            Some(&"thread-atomic-bind".to_string())
        );
        assert_eq!(
            persisted.telegram_topic_binding_states[&route.conversation_key].lifecycle_generation,
            generation
        );
    }
}
