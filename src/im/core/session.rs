use anyhow::Result;

use crate::{
    app_state::SharedState, im_runtime::RouteTarget, remote_control_backend, types::ImPlatformKind,
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
    let _binding_guard = state.im_route_binding_ops.lock().await;
    {
        let mut runtime = state.runtime.lock().await;
        runtime.unbind_routes_for_conversation_with_reason(
            &route.conversation_key,
            "bind_thread_to_route",
        );
        let mut route = route.clone();
        route.remote_client_key = remote_client_key;
        runtime.bind_route(thread_id, route);
        if let Some(request_id) = request_id {
            runtime.clear_thread_routing_request(request_id);
        }
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
        let state_path = state.config.lock().await.state_path.clone();
        persisted.save(&state_path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::{
        app_state::AppState, config::AppConfig, store::PersistedState, types::ImPlatformKind,
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
}
