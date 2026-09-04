use serde::Serialize;

use crate::{
    app_state::{RemoteControlSourceKind, RemoteControlStaleReasonCode, SharedState},
    types::now_ms,
};

use super::{
    active_connection_locked, connection_initialized, prune_inactive_remote_connections_locked,
    remote_control_stale_reason_locked, select_active_connection_id_locked,
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteControlStatusResponse {
    pub connected: bool,
    pub initialized: bool,
    pub active_connection_id: Option<String>,
    pub active_source_kind: Option<RemoteControlSourceKind>,
    pub active_user_agent: Option<String>,
    pub connections: Vec<RemoteControlConnectionStatusResponse>,
    pub client_id: String,
    pub stream_id: Option<String>,
    pub server_id: Option<String>,
    pub environment_id: Option<String>,
    pub server_name: Option<String>,
    pub installation_id: Option<String>,
    pub account_id: Option<String>,
    pub current_thread_id: Option<String>,
    pub current_turn_id: Option<String>,
    pub last_error: Option<String>,
    pub healthy: bool,
    pub stale: bool,
    pub stale_reason_code: Option<RemoteControlStaleReasonCode>,
    pub connected_at_ms: Option<u128>,
    pub last_ws_inbound_at_ms: Option<u128>,
    pub last_ws_ping_at_ms: Option<u128>,
    pub last_ws_pong_at_ms: Option<u128>,
    pub last_app_ping_at_ms: Option<u128>,
    pub last_app_pong_at_ms: Option<u128>,
    pub last_app_pong_status: Option<String>,
    pub last_initialize_sent_at_ms: Option<u128>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteControlConnectionStatusResponse {
    pub id: String,
    pub connection_epoch: u64,
    pub connected: bool,
    pub initialized: bool,
    pub healthy: bool,
    pub source_kind: RemoteControlSourceKind,
    pub user_agent: Option<String>,
    pub server_id: Option<String>,
    pub server_name: Option<String>,
    pub installation_id: Option<String>,
    pub account_id: Option<String>,
    pub connected_at_ms: Option<u128>,
    pub last_ws_inbound_at_ms: Option<u128>,
    pub last_ws_ping_at_ms: Option<u128>,
    pub last_ws_pong_at_ms: Option<u128>,
    pub last_error: Option<String>,
}

pub async fn status_snapshot(state: &SharedState) -> RemoteControlStatusResponse {
    let mut remote = state.remote_control.inner.lock().await;
    prune_inactive_remote_connections_locked(&mut remote);
    let stale_reason = remote_control_stale_reason_locked(&remote, now_ms());
    let stale = stale_reason.is_some();
    let stale_reason_code = stale_reason.map(|(code, _)| code);
    let active_connection_id = select_active_connection_id_locked(&remote);
    let active_connection = active_connection_locked(&remote);
    let active_source_kind = active_connection.map(|connection| connection.source_kind);
    let active_user_agent = active_connection.and_then(|connection| connection.user_agent.clone());
    let mut connection_values = remote.connections.values().collect::<Vec<_>>();
    connection_values.sort_by_key(|connection| {
        std::cmp::Reverse((
            connection.connected && connection.outbound_tx.is_some(),
            connection_initialized(connection),
            connection
                .last_ws_inbound_at_ms
                .or(connection.connected_at_ms)
                .unwrap_or_default(),
            connection.connection_epoch,
        ))
    });
    let connections = connection_values
        .into_iter()
        .map(|connection| RemoteControlConnectionStatusResponse {
            id: connection.connection_id.clone(),
            connection_epoch: connection.connection_epoch,
            connected: connection.connected,
            initialized: connection_initialized(connection),
            healthy: connection.connected
                && connection_initialized(connection)
                && connection.outbound_tx.is_some(),
            source_kind: connection.source_kind,
            user_agent: connection.user_agent.clone(),
            server_id: connection.server_id.clone(),
            server_name: connection.server_name.clone(),
            installation_id: connection.installation_id.clone(),
            account_id: connection.account_id.clone(),
            connected_at_ms: connection.connected_at_ms,
            last_ws_inbound_at_ms: connection.last_ws_inbound_at_ms,
            last_ws_ping_at_ms: connection.last_ws_ping_at_ms,
            last_ws_pong_at_ms: connection.last_ws_pong_at_ms,
            last_error: connection.last_error.clone(),
        })
        .collect::<Vec<_>>();
    let default_client = active_connection
        .and_then(|connection| connection.clients.get(&connection.default_client_key));
    let initialized = active_connection.is_some_and(connection_initialized);
    let client_id = default_client
        .map(|client| client.client_id.clone())
        .unwrap_or_default();
    let stream_id = default_client.map(|client| client.stream_id.clone());
    let current_thread_id = default_client.and_then(|client| client.current_thread_id.clone());
    let current_turn_id = default_client.and_then(|client| client.current_turn_id.clone());
    let last_app_ping_at_ms = default_client.and_then(|client| client.last_app_ping_at_ms);
    let last_app_pong_at_ms = default_client.and_then(|client| client.last_app_pong_at_ms);
    let last_app_pong_status =
        default_client.and_then(|client| client.last_app_pong_status.clone());
    let last_initialize_sent_at_ms =
        default_client.and_then(|client| client.last_initialize_sent_at_ms);
    let connected = active_connection.is_some_and(|connection| connection.connected);
    let healthy = connected && initialized && !stale;
    RemoteControlStatusResponse {
        connected,
        initialized,
        active_connection_id,
        active_source_kind,
        active_user_agent,
        connections,
        client_id,
        stream_id,
        server_id: active_connection.and_then(|connection| connection.server_id.clone()),
        environment_id: active_connection.and_then(|connection| connection.environment_id.clone()),
        server_name: active_connection.and_then(|connection| connection.server_name.clone()),
        installation_id: active_connection
            .and_then(|connection| connection.installation_id.clone()),
        account_id: active_connection.and_then(|connection| connection.account_id.clone()),
        current_thread_id,
        current_turn_id,
        last_error: active_connection.and_then(|connection| connection.last_error.clone()),
        healthy,
        stale,
        stale_reason_code,
        connected_at_ms: active_connection.and_then(|connection| connection.connected_at_ms),
        last_ws_inbound_at_ms: active_connection
            .and_then(|connection| connection.last_ws_inbound_at_ms),
        last_ws_ping_at_ms: active_connection.and_then(|connection| connection.last_ws_ping_at_ms),
        last_ws_pong_at_ms: active_connection.and_then(|connection| connection.last_ws_pong_at_ms),
        last_app_ping_at_ms,
        last_app_pong_at_ms,
        last_app_pong_status,
        last_initialize_sent_at_ms,
    }
}
