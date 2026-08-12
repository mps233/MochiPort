use crate::{
    app_state::SharedState,
    im_runtime::{RouteTarget, route_from_conversation_key},
    types::{ImPlatformKind, InboundMessage},
};

pub(crate) fn route_for_message(message: &InboundMessage) -> RouteTarget {
    RouteTarget {
        platform: message.platform,
        conversation_key: message.conversation_key(),
        account_id: message.account_id.clone(),
        chat_id: message.chat_id.clone(),
        remote_client_key: String::new(),
    }
    .with_deterministic_remote_client_key()
}

pub(crate) async fn live_thread_for_route(
    state: &SharedState,
    route: &RouteTarget,
) -> Option<String> {
    state
        .runtime
        .lock()
        .await
        .route_by_thread
        .iter()
        .find_map(|(thread_id, existing_route)| {
            (existing_route.conversation_key == route.conversation_key).then(|| thread_id.clone())
        })
}

pub(crate) async fn live_thread_binding_for_route(
    state: &SharedState,
    route: &RouteTarget,
) -> Option<(String, RouteTarget)> {
    state
        .runtime
        .lock()
        .await
        .route_by_thread
        .iter()
        .find_map(|(thread_id, existing_route)| {
            (existing_route.conversation_key == route.conversation_key)
                .then(|| (thread_id.clone(), existing_route.clone()))
        })
}

pub(crate) async fn active_turn_for_message(
    state: &SharedState,
    message: &InboundMessage,
) -> Option<(String, String)> {
    let route = route_for_message(message);
    let thread_id = live_thread_for_route(state, &route).await?;
    let runtime = state.runtime.lock().await;
    let turn_id = runtime.current_turn_by_thread.get(&thread_id)?.clone();
    Some((thread_id, turn_id))
}

pub(crate) async fn remote_client_key_for_thread(
    state: &SharedState,
    thread_id: &str,
) -> Option<String> {
    state
        .runtime
        .lock()
        .await
        .route_for_thread(thread_id)
        .map(|route| route.remote_client_key)
}

pub(crate) async fn clear_thread_binding(
    state: &SharedState,
    conversation_key: &str,
) -> anyhow::Result<()> {
    clear_thread_binding_with_reason(state, conversation_key, "clear_thread_binding").await
}

pub(crate) async fn clear_thread_binding_with_reason(
    state: &SharedState,
    conversation_key: &str,
    reason: &str,
) -> anyhow::Result<()> {
    let _binding_guard = state.im_route_binding_ops.lock().await;
    {
        let mut runtime = state.runtime.lock().await;
        runtime.unbind_routes_for_conversation_with_reason(conversation_key, reason);
    }
    forget_persisted_thread_binding(state, conversation_key).await?;
    Ok(())
}

pub(crate) async fn clear_thread_binding_for_thread_with_reason(
    state: &SharedState,
    thread_id: &str,
    remote_client_key: &str,
    reason: &str,
) -> anyhow::Result<bool> {
    let _binding_guard = state.im_route_binding_ops.lock().await;
    let conversation_key = {
        let mut runtime = state.runtime.lock().await;
        let Some(route) = runtime.route_for_thread(thread_id) else {
            return Ok(false);
        };
        if route.remote_client_key != remote_client_key {
            return Ok(false);
        }
        runtime.unbind_routes_for_conversation_with_reason(&route.conversation_key, reason);
        route.conversation_key
    };
    forget_persisted_thread_binding(state, &conversation_key).await?;
    Ok(true)
}

async fn forget_persisted_thread_binding(
    state: &SharedState,
    conversation_key: &str,
) -> anyhow::Result<()> {
    if !route_from_conversation_key(conversation_key)
        .is_some_and(|route| route.platform == ImPlatformKind::Telegram)
    {
        return Ok(());
    }

    let mut persisted = state.persisted.lock().await;
    if persisted
        .im_thread_bindings
        .remove(conversation_key)
        .is_none()
    {
        return Ok(());
    }
    let state_path = state.config.lock().await.state_path.clone();
    persisted.save(&state_path)
}

pub(crate) fn is_stale_thread_error(err: &anyhow::Error) -> bool {
    let message = err.to_string();
    message.contains("thread not found") || message.contains("is closing")
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::{app_state::AppState, config::AppConfig, store::PersistedState};

    fn test_state() -> (SharedState, AppConfig, tempfile::TempDir) {
        let temp_dir = tempdir().expect("temp dir");
        let mut config = AppConfig::default();
        config.state_path = temp_dir.path().join("state.json");
        let state = AppState::new(
            temp_dir.path().join("config.toml"),
            config.clone(),
            None,
            None,
        );
        (state, config, temp_dir)
    }

    #[tokio::test]
    async fn clearing_telegram_binding_removes_runtime_and_saved_state() {
        let (state, config, _temp_dir) = test_state();
        let route = RouteTarget {
            platform: ImPlatformKind::Telegram,
            conversation_key: "telegram:bot:42".to_string(),
            account_id: "bot".to_string(),
            chat_id: "42".to_string(),
            remote_client_key: "im:telegram:chat".to_string(),
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
            persisted.save(&config.state_path).expect("persist binding");
        }

        clear_thread_binding_with_reason(&state, &route.conversation_key, "test_clear")
            .await
            .expect("clear binding");

        assert!(state.runtime.lock().await.route_by_thread.is_empty());
        assert!(state.persisted.lock().await.im_thread_bindings.is_empty());
        assert!(
            PersistedState::load(&config.state_path)
                .im_thread_bindings
                .is_empty()
        );
    }

    #[tokio::test]
    async fn stale_cleanup_does_not_remove_thread_rebound_to_another_client() {
        let (state, config, _temp_dir) = test_state();
        let route = RouteTarget {
            platform: ImPlatformKind::Telegram,
            conversation_key: "telegram:new:42".to_string(),
            account_id: "new".to_string(),
            chat_id: "42".to_string(),
            remote_client_key: "im:telegram:new".to_string(),
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
            persisted.save(&config.state_path).expect("persist binding");
        }

        let cleared = clear_thread_binding_for_thread_with_reason(
            &state,
            "thread-42",
            "im:telegram:old",
            "late_stale_response",
        )
        .await
        .expect("ignore stale cleanup");

        assert!(!cleared);
        assert!(
            state
                .runtime
                .lock()
                .await
                .route_by_thread
                .contains_key("thread-42")
        );
        assert_eq!(
            state
                .persisted
                .lock()
                .await
                .im_thread_bindings
                .get("telegram:new:42")
                .map(String::as_str),
            Some("thread-42")
        );
    }
}
