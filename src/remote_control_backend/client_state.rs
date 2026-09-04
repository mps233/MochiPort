use std::collections::HashMap;

use crate::{
    app_state::{
        RemoteControlClientState, RemoteControlInner, RemoteControlServerConnection,
        RemoteControlSourceKind,
    },
    chain_log,
};

#[cfg(test)]
use super::server_envelopes::server_ack_cursor_key;
use super::{
    DEFAULT_REMOTE_CLIENT_KEY, MOCHIPORT_BRIDGE_CLIENT_ID, OutboundWsMessage, stable_id, uuid_like,
};

pub(in crate::remote_control_backend) fn connection_for_epoch_locked(
    remote: &RemoteControlInner,
    connection_epoch: u64,
) -> Option<&RemoteControlServerConnection> {
    remote
        .connections
        .values()
        .find(|connection| connection.connection_epoch == connection_epoch)
}

pub(in crate::remote_control_backend) fn connection_for_epoch_mut_locked(
    remote: &mut RemoteControlInner,
    connection_epoch: u64,
) -> Option<&mut RemoteControlServerConnection> {
    remote
        .connections
        .values_mut()
        .find(|connection| connection.connection_epoch == connection_epoch)
}

pub(in crate::remote_control_backend) fn connection_initialized(
    connection: &RemoteControlServerConnection,
) -> bool {
    connection
        .clients
        .get(&connection.default_client_key)
        .is_some_and(|client| client.initialized)
}

pub(in crate::remote_control_backend) fn ensure_client_state_for_connection_locked<'a>(
    remote: &'a mut RemoteControlInner,
    connection_epoch: u64,
    client_key: &str,
) -> Option<&'a mut RemoteControlClientState> {
    let connection = connection_for_epoch_mut_locked(remote, connection_epoch)?;
    let client_key = if client_key.trim().is_empty() || client_key == DEFAULT_REMOTE_CLIENT_KEY {
        connection.default_client_key.clone()
    } else {
        normalize_remote_client_key(client_key)
    };
    if !connection.clients.contains_key(&client_key) {
        let default_stream_id = connection
            .clients
            .get(&connection.default_client_key)
            .map(|client| client.stream_id.clone())
            .unwrap_or_else(uuid_like);
        let stream_id = if client_key == connection.default_client_key {
            default_stream_id
        } else {
            stable_id("stream", &format!("{default_stream_id}:{client_key}"))
        };
        connection.clients.insert(
            client_key.clone(),
            RemoteControlClientState {
                client_id: MOCHIPORT_BRIDGE_CLIENT_ID.to_string(),
                stream_id,
                initialized: false,
                next_seq_id: 1,
                pending: HashMap::new(),
                current_thread_id: None,
                current_turn_id: None,
                last_app_ping_at_ms: None,
                last_app_pong_at_ms: None,
                last_app_pong_status: None,
                last_initialize_sent_at_ms: None,
                recovery_attempt: 0,
                recovery_started_at_ms: None,
            },
        );
    }
    connection.clients.get_mut(&client_key)
}

pub(in crate::remote_control_backend) fn client_state_for_connection_locked<'a>(
    remote: &'a RemoteControlInner,
    connection_epoch: u64,
    client_key: &str,
) -> Option<&'a RemoteControlClientState> {
    let connection = connection_for_epoch_locked(remote, connection_epoch)?;
    let client_key = if client_key.trim().is_empty() || client_key == DEFAULT_REMOTE_CLIENT_KEY {
        connection.default_client_key.clone()
    } else {
        normalize_remote_client_key(client_key)
    };
    connection.clients.get(&client_key)
}

pub(in crate::remote_control_backend) fn client_state_mut_for_connection_locked<'a>(
    remote: &'a mut RemoteControlInner,
    connection_epoch: u64,
    client_key: &str,
) -> Option<&'a mut RemoteControlClientState> {
    let connection = connection_for_epoch_mut_locked(remote, connection_epoch)?;
    let client_key = if client_key.trim().is_empty() || client_key == DEFAULT_REMOTE_CLIENT_KEY {
        connection.default_client_key.clone()
    } else {
        normalize_remote_client_key(client_key)
    };
    connection.clients.get_mut(&client_key)
}

pub(in crate::remote_control_backend) fn is_legacy_default_client_key(client_key: &str) -> bool {
    client_key == DEFAULT_REMOTE_CLIENT_KEY || client_key.starts_with("default:")
}

pub(in crate::remote_control_backend) fn source_default_client_key(
    source_kind: RemoteControlSourceKind,
) -> String {
    match source_kind {
        RemoteControlSourceKind::CodexApp => "default:codex_app",
        RemoteControlSourceKind::Vscode => "default:vscode",
        RemoteControlSourceKind::Cli => "default:cli",
        RemoteControlSourceKind::Unknown => "default:unknown",
    }
    .to_string()
}

pub(in crate::remote_control_backend) fn source_kind_from_default_client_key(
    client_key: &str,
) -> Option<RemoteControlSourceKind> {
    match client_key {
        "default:codex_app" => Some(RemoteControlSourceKind::CodexApp),
        "default:vscode" => Some(RemoteControlSourceKind::Vscode),
        "default:cli" => Some(RemoteControlSourceKind::Cli),
        "default:unknown" => Some(RemoteControlSourceKind::Unknown),
        _ => None,
    }
}

pub(in crate::remote_control_backend) fn default_client_key_for_connection_locked(
    remote: &RemoteControlInner,
    connection_epoch: u64,
) -> String {
    connection_for_epoch_locked(remote, connection_epoch)
        .map(|connection| connection.default_client_key.clone())
        .unwrap_or_else(|| DEFAULT_REMOTE_CLIENT_KEY.to_string())
}

pub(in crate::remote_control_backend) fn migrate_source_default_client_key_locked(
    remote: &mut RemoteControlInner,
    connection_epoch: u64,
    old_client_key: &str,
    new_source_kind: RemoteControlSourceKind,
    client_id: &str,
    stream_id: &str,
) -> String {
    let old_client_key = normalize_remote_client_key(old_client_key);
    if new_source_kind == RemoteControlSourceKind::Unknown
        || !is_legacy_default_client_key(&old_client_key)
    {
        return old_client_key;
    }

    let new_client_key = source_default_client_key(new_source_kind);
    if old_client_key == new_client_key {
        return old_client_key;
    }

    let Some(connection) = connection_for_epoch_mut_locked(remote, connection_epoch) else {
        return old_client_key;
    };
    if connection.clients.contains_key(&new_client_key) {
        chain_log::write_line(format!(
            "[remote_control] event=source_default_client_key_migration_skipped connection_epoch={} old_client_key={} new_client_key={} source_kind={:?} reason=target_exists",
            connection_epoch, old_client_key, new_client_key, new_source_kind
        ));
        return old_client_key;
    }
    let old_matches_stream = connection
        .clients
        .get(&old_client_key)
        .is_some_and(|client| client.client_id == client_id && client.stream_id == stream_id);
    if !old_matches_stream {
        return old_client_key;
    }
    let Some(old_client) = connection.clients.remove(&old_client_key) else {
        return old_client_key;
    };
    connection
        .clients
        .insert(new_client_key.clone(), old_client);
    connection.default_client_key = new_client_key.clone();
    chain_log::write_line(format!(
        "[remote_control] event=source_default_client_key_migrated connection_epoch={} old_client_key={} new_client_key={} source_kind={:?} client_id={} stream_id={}",
        connection_epoch, old_client_key, new_client_key, new_source_kind, client_id, stream_id
    ));
    new_client_key
}

pub(in crate::remote_control_backend) fn active_default_client_key_locked(
    remote: &RemoteControlInner,
) -> String {
    select_active_connection_id_locked(remote)
        .as_ref()
        .and_then(|connection_id| remote.connections.get(connection_id))
        .map(|connection| connection.default_client_key.clone())
        .unwrap_or_else(|| DEFAULT_REMOTE_CLIENT_KEY.to_string())
}

pub(in crate::remote_control_backend) fn connection_epoch_for_client_key_locked(
    remote: &RemoteControlInner,
    client_key: &str,
) -> Option<u64> {
    if client_key == DEFAULT_REMOTE_CLIENT_KEY || !is_legacy_default_client_key(client_key) {
        return active_connection_epoch_locked(remote);
    }
    let source_kind = source_kind_from_default_client_key(client_key);
    remote
        .connections
        .values()
        .filter(|connection| {
            connection.connected
                && connection_initialized(connection)
                && connection.outbound_tx.is_some()
                && (connection.default_client_key == client_key
                    || source_kind.is_some_and(|source_kind| connection.source_kind == source_kind))
        })
        .max_by_key(|connection| {
            (
                connection
                    .last_ws_inbound_at_ms
                    .or(connection.connected_at_ms)
                    .unwrap_or_default(),
                connection.connection_epoch,
            )
        })
        .map(|connection| connection.connection_epoch)
}

pub(in crate::remote_control_backend) fn resolve_remote_client_key_for_connection_locked(
    remote: &RemoteControlInner,
    connection_epoch: u64,
    client_key: &str,
) -> String {
    let client_key = normalize_remote_client_key(client_key);
    if client_key == DEFAULT_REMOTE_CLIENT_KEY
        || source_kind_from_default_client_key(&client_key).is_some()
    {
        default_client_key_for_connection_locked(remote, connection_epoch)
    } else {
        client_key
    }
}

pub(in crate::remote_control_backend) fn resolve_remote_client_key_locked(
    remote: &RemoteControlInner,
    client_key: &str,
) -> String {
    let client_key = normalize_remote_client_key(client_key);
    if client_key == DEFAULT_REMOTE_CLIENT_KEY {
        return active_default_client_key_locked(remote);
    }
    if !is_legacy_default_client_key(&client_key) {
        return client_key;
    }
    let Some(connection_epoch) = connection_epoch_for_client_key_locked(remote, &client_key) else {
        return client_key;
    };
    resolve_remote_client_key_for_connection_locked(remote, connection_epoch, &client_key)
}

pub(in crate::remote_control_backend) fn normalize_remote_client_key(client_key: &str) -> String {
    let client_key = client_key.trim();
    if client_key.is_empty() {
        DEFAULT_REMOTE_CLIENT_KEY.to_string()
    } else {
        client_key.to_string()
    }
}

pub(in crate::remote_control_backend) fn remote_client_key_for_stream_on_connection_locked(
    remote: &RemoteControlInner,
    connection_epoch: u64,
    client_id: &str,
    stream_id: &str,
) -> Option<String> {
    let connection = connection_for_epoch_locked(remote, connection_epoch)?;
    connection
        .clients
        .iter()
        .find(|(_, client)| client.client_id == client_id && client.stream_id == stream_id)
        .map(|(client_key, _)| client_key.clone())
}

pub(in crate::remote_control_backend) fn source_kind_from_user_agent(
    user_agent: &str,
) -> RemoteControlSourceKind {
    let user_agent = user_agent.trim();
    if user_agent.starts_with("Codex Desktop/") {
        RemoteControlSourceKind::CodexApp
    } else if user_agent.starts_with("codex_vscode/") {
        RemoteControlSourceKind::Vscode
    } else if user_agent.starts_with("mochiport/")
        || user_agent.starts_with("codexhub/")
        || user_agent.contains("WindowsTerminal")
        || user_agent.contains("Terminal")
    {
        RemoteControlSourceKind::Cli
    } else {
        RemoteControlSourceKind::Unknown
    }
}

fn source_kind_priority(kind: RemoteControlSourceKind) -> u8 {
    match kind {
        RemoteControlSourceKind::CodexApp => 40,
        RemoteControlSourceKind::Vscode => 30,
        RemoteControlSourceKind::Cli => 20,
        RemoteControlSourceKind::Unknown => 10,
    }
}

pub(in crate::remote_control_backend) fn prune_inactive_remote_connections_locked(
    remote: &mut RemoteControlInner,
) {
    remote
        .connections
        .retain(|_, connection| connection.connected && connection.outbound_tx.is_some());
}

pub(in crate::remote_control_backend) fn select_active_connection_id_locked(
    remote: &RemoteControlInner,
) -> Option<String> {
    remote
        .connections
        .values()
        .filter(|connection| connection.connected && connection.outbound_tx.is_some())
        .max_by_key(|connection| {
            (
                connection_initialized(connection),
                source_kind_priority(connection.source_kind),
                connection
                    .last_ws_inbound_at_ms
                    .or(connection.connected_at_ms)
                    .unwrap_or_default(),
                connection.connection_epoch,
            )
        })
        .map(|connection| connection.connection_id.clone())
}

pub(in crate::remote_control_backend) fn active_connection_locked(
    remote: &RemoteControlInner,
) -> Option<&RemoteControlServerConnection> {
    select_active_connection_id_locked(remote)
        .as_ref()
        .and_then(|connection_id| remote.connections.get(connection_id))
}

pub(in crate::remote_control_backend) fn active_connection_mut_locked(
    remote: &mut RemoteControlInner,
) -> Option<&mut RemoteControlServerConnection> {
    let connection_id = select_active_connection_id_locked(remote)?;
    remote.connections.get_mut(&connection_id)
}

pub(in crate::remote_control_backend) fn active_connection_epoch_locked(
    remote: &RemoteControlInner,
) -> Option<u64> {
    active_connection_locked(remote).map(|connection| connection.connection_epoch)
}

pub(in crate::remote_control_backend) fn outbound_tx_for_connection_epoch_locked(
    remote: &RemoteControlInner,
    connection_epoch: u64,
) -> Option<tokio::sync::mpsc::UnboundedSender<OutboundWsMessage>> {
    connection_for_epoch_locked(remote, connection_epoch)
        .filter(|connection| connection.connected)
        .and_then(|connection| connection.outbound_tx.clone())
}

pub(in crate::remote_control_backend) fn connection_exists_locked(
    remote: &RemoteControlInner,
    connection_epoch: u64,
) -> bool {
    connection_for_epoch_locked(remote, connection_epoch)
        .is_some_and(|connection| connection.connected)
}

pub(in crate::remote_control_backend) fn remove_pending_initialize_for_connection_locked(
    remote: &mut RemoteControlInner,
    connection_epoch: u64,
) -> usize {
    let Some(connection) = connection_for_epoch_mut_locked(remote, connection_epoch) else {
        return 0;
    };
    let mut removed = 0;
    for client in connection.clients.values_mut() {
        let before = client.pending.len();
        client.pending.retain(|_, pending| {
            pending.method != "initialize" || pending.connection_epoch != connection_epoch
        });
        removed += before.saturating_sub(client.pending.len());
    }
    removed
}

#[cfg(test)]
pub(in crate::remote_control_backend) fn reset_remote_clients_for_connection_locked(
    remote: &mut RemoteControlInner,
    connection_epoch: u64,
) -> Vec<String> {
    let Some(connection) = connection_for_epoch_mut_locked(remote, connection_epoch) else {
        return Vec::new();
    };
    let ack_cursor_keys = connection
        .clients
        .values()
        .map(|client| server_ack_cursor_key(connection_epoch, &client.client_id, &client.stream_id))
        .collect::<Vec<_>>();
    for client in connection.clients.values_mut() {
        client.initialized = false;
        client.last_app_ping_at_ms = None;
        client.last_app_pong_at_ms = None;
        client.last_app_pong_status = None;
        client.last_initialize_sent_at_ms = None;
        client.recovery_started_at_ms = None;
        client
            .pending
            .retain(|_, pending| pending.method != "initialize");
    }
    ack_cursor_keys
}
