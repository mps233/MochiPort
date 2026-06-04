use anyhow::Result;

use crate::{
    app_state::SharedState,
    chain_log,
    im_runtime::{RouteTarget, route_from_conversation_key},
    types::InboundMessage,
};

pub(crate) fn route_for_message(message: &InboundMessage) -> RouteTarget {
    RouteTarget {
        platform: message.platform,
        conversation_key: message.conversation_key(),
        account_id: message.account_id.clone(),
        chat_id: message.chat_id.clone(),
    }
}

pub(crate) async fn live_thread_for_route(
    state: &SharedState,
    route: &RouteTarget,
) -> Option<String> {
    if let Some(thread_id) =
        state
            .runtime
            .lock()
            .await
            .route_by_thread
            .iter()
            .find_map(|(thread_id, existing_route)| {
                (existing_route.conversation_key == route.conversation_key)
                    .then(|| thread_id.clone())
            })
    {
        return Some(thread_id);
    }

    let thread_id = {
        let persisted = state.persisted.lock().await;
        persisted.sessions.get(&route.conversation_key).cloned()
    }?;

    {
        let mut runtime = state.runtime.lock().await;
        if runtime.route_for_thread(&thread_id).is_none() {
            runtime.bind_route(&thread_id, route.clone());
            chain_log::write_line(format!(
                "[im_route] level=warn event=restore_persisted_binding direction=inbound thread={} platform={} account={} chat={} conversation={}",
                thread_id,
                route.platform.key(),
                route.account_id,
                route.chat_id,
                route.conversation_key
            ));
        }
    }

    Some(thread_id)
}

pub(crate) async fn route_for_persisted_thread(
    state: &SharedState,
    thread_id: &str,
) -> Option<RouteTarget> {
    let conversation_key = {
        let persisted = state.persisted.lock().await;
        persisted
            .sessions
            .iter()
            .find_map(|(conversation_key, persisted_thread_id)| {
                (persisted_thread_id == thread_id).then(|| conversation_key.clone())
            })
    }?;
    let route = route_from_conversation_key(&conversation_key)?;
    {
        let mut runtime = state.runtime.lock().await;
        if runtime.route_for_thread(thread_id).is_none() {
            runtime.bind_route(thread_id, route.clone());
            chain_log::write_line(format!(
                "[im_route] level=warn event=restore_persisted_binding direction=outbound thread={} platform={} account={} chat={} conversation={}",
                thread_id,
                route.platform.key(),
                route.account_id,
                route.chat_id,
                route.conversation_key
            ));
        }
    }
    Some(route)
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

pub(crate) async fn persist_thread_binding(
    state: &SharedState,
    route: &RouteTarget,
    thread_id: &str,
) -> Result<()> {
    let mut persisted = state.persisted.lock().await;
    persisted
        .sessions
        .insert(route.conversation_key.clone(), thread_id.to_string());
    let config = state.config.lock().await.clone();
    persisted.save(&config.state_path)?;
    Ok(())
}

pub(crate) async fn clear_thread_binding(
    state: &SharedState,
    conversation_key: &str,
) -> Result<()> {
    clear_thread_binding_with_reason(state, conversation_key, "clear_thread_binding").await
}

pub(crate) async fn clear_thread_binding_with_reason(
    state: &SharedState,
    conversation_key: &str,
    reason: &str,
) -> Result<()> {
    {
        let mut runtime = state.runtime.lock().await;
        runtime.unbind_routes_for_conversation_with_reason(conversation_key, reason);
    }
    let mut persisted = state.persisted.lock().await;
    persisted.sessions.remove(conversation_key);
    let config = state.config.lock().await.clone();
    persisted.save(&config.state_path)?;
    Ok(())
}

pub(crate) fn is_stale_thread_error(err: &anyhow::Error) -> bool {
    let message = err.to_string();
    message.contains("thread not found") || message.contains("is closing")
}
