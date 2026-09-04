use anyhow::{Result, anyhow};
use serde_json::json;

use crate::{
    app_state::SharedState,
    chain_log,
    im::core::routing::{clear_thread_binding_for_thread_with_reason, is_stale_thread_error},
    types::now_ms,
};

use super::client_state::{
    client_state_for_connection_locked, client_state_mut_for_connection_locked,
    connection_exists_locked, connection_for_epoch_mut_locked,
    ensure_client_state_for_connection_locked, normalize_remote_client_key,
    outbound_tx_for_connection_epoch_locked, resolve_remote_client_key_for_connection_locked,
};
use super::log_format::pending_requests_summary;
use super::outbound::send_initialize_for_client_on_connection;
use super::session_api::{
    request_once_with_timeout_for_client_on_connection, route_remote_client_key_matches,
    thread_status_type_from_payload, wait_for_remote_control_initialized_on_connection,
};
use super::{
    OutboundWsMessage, REMOTE_CONTROL_CLIENT_REINITIALIZED_ERROR,
    REMOTE_CONTROL_UNKNOWN_REINITIALIZE_TIMEOUT, REMOTE_DISCOVERY_REQUEST_TIMEOUT,
};

async fn reset_remote_control_client_for_key(
    state: &SharedState,
    connection_epoch: u64,
    client_key: &str,
) -> Result<()> {
    let requested_client_key = normalize_remote_client_key(client_key);
    let (client_key, pending, client_id, stream_id, pending_summary) = {
        let mut remote = state.remote_control.inner.lock().await;
        if !connection_exists_locked(&remote, connection_epoch) {
            return Ok(());
        }
        let client_key = resolve_remote_client_key_for_connection_locked(
            &remote,
            connection_epoch,
            &requested_client_key,
        );
        let Some(client) =
            ensure_client_state_for_connection_locked(&mut remote, connection_epoch, &client_key)
        else {
            return Ok(());
        };
        client.initialized = false;
        let client_id = client.client_id.clone();
        let stream_id = client.stream_id.clone();
        let pending_summary = pending_requests_summary(&client.pending);
        let pending = std::mem::take(&mut client.pending);
        (client_key, pending, client_id, stream_id, pending_summary)
    };
    chain_log::write_line(format!(
        "[remote_control] event=remote_control_client_reset connection_epoch={} client_key={} client_id={} stream_id={} pending_count={} pending={}",
        connection_epoch,
        client_key,
        client_id,
        stream_id,
        pending.len(),
        pending_summary
    ));
    for (_, pending) in pending {
        let _ = pending
            .response_tx
            .send(Err(anyhow!(REMOTE_CONTROL_CLIENT_REINITIALIZED_ERROR)));
    }
    send_initialize_for_client_on_connection(state, connection_epoch, &client_key)
        .await
        .map(|_| ())
}

pub(super) async fn start_remote_control_client_recovery(
    state: &SharedState,
    connection_epoch: u64,
    client_key: &str,
    client_id: &str,
    stream_id: &str,
) -> Result<()> {
    let requested_client_key = normalize_remote_client_key(client_key);
    let (client_key, attempt, should_spawn) = {
        let mut remote = state.remote_control.inner.lock().await;
        if !connection_exists_locked(&remote, connection_epoch) {
            return Ok(());
        }
        let client_key = resolve_remote_client_key_for_connection_locked(
            &remote,
            connection_epoch,
            &requested_client_key,
        );
        let Some(client) =
            ensure_client_state_for_connection_locked(&mut remote, connection_epoch, &client_key)
        else {
            return Ok(());
        };
        if client.client_id != client_id || client.stream_id != stream_id {
            return Ok(());
        }
        if client.recovery_started_at_ms.is_some() {
            (client_key, client.recovery_attempt, false)
        } else {
            client.recovery_attempt = client.recovery_attempt.saturating_add(1);
            client.recovery_started_at_ms = Some(now_ms());
            client.initialized = false;
            client.last_app_pong_status = Some("unknown".to_string());
            let attempt = client.recovery_attempt;
            (client_key, attempt, true)
        }
    };
    if !should_spawn {
        chain_log::write_line(format!(
            "[remote_control] event=recovery_already_running connection_epoch={} client_key={} client_id={} stream_id={} attempt={}",
            connection_epoch, client_key, client_id, stream_id, attempt
        ));
        return Ok(());
    }
    chain_log::write_line(format!(
        "[remote_control] event=recovery_start connection_epoch={} client_key={} client_id={} stream_id={} attempt={} strategy=same_stream_reinitialize",
        connection_epoch, client_key, client_id, stream_id, attempt
    ));
    state
        .push_event(
            "warn",
            "remote_control_recovery_start",
            format!(
                "client_key={} stream_id={} attempt={} strategy=same_stream_reinitialize",
                client_key, stream_id, attempt
            ),
        )
        .await;
    let recovery_state = state.clone();
    tokio::spawn(async move {
        if let Err(err) =
            run_remote_control_client_recovery(recovery_state.clone(), connection_epoch, client_key)
                .await
        {
            recovery_state
                .push_event("error", "remote_control_recovery_failed", err.to_string())
                .await;
        }
    });
    Ok(())
}

async fn run_remote_control_client_recovery(
    state: SharedState,
    connection_epoch: u64,
    client_key: String,
) -> Result<()> {
    reset_remote_control_client_for_key(&state, connection_epoch, &client_key).await?;
    let initialize_result = tokio::time::timeout(
        REMOTE_CONTROL_UNKNOWN_REINITIALIZE_TIMEOUT,
        wait_for_remote_control_initialized_on_connection(&state, connection_epoch, &client_key),
    )
    .await;
    match initialize_result {
        Ok(Ok(_)) => {
            let (client_id, stream_id, attempt) =
                finish_remote_control_client_recovery(&state, connection_epoch, &client_key)
                    .await?;
            chain_log::write_line(format!(
                "[remote_control] event=recovery_ready connection_epoch={} client_key={} client_id={} stream_id={} attempt={}",
                connection_epoch, client_key, client_id, stream_id, attempt
            ));
            state
                .push_event(
                    "info",
                    "remote_control_recovery_ready",
                    format!(
                        "client_key={} stream_id={} attempt={}",
                        client_key, stream_id, attempt
                    ),
                )
                .await;
            if let Err(err) = resubscribe_bound_threads_after_recovery(
                &state,
                connection_epoch,
                &client_key,
                attempt,
            )
            .await
            {
                chain_log::write_line(format!(
                    "[remote_control] event=recovery_thread_resubscribe_failed connection_epoch={} client_key={} attempt={} err={}",
                    connection_epoch, client_key, attempt, err
                ));
                state
                    .push_event(
                        "warn",
                        "remote_control_recovery_thread_resubscribe_failed",
                        format!("client_key={} attempt={} err={}", client_key, attempt, err),
                    )
                    .await;
            }
            Ok(())
        }
        Ok(Err(err)) => {
            chain_log::write_line(format!(
                "[remote_control] event=recovery_initialize_failed connection_epoch={} client_key={} err={}",
                connection_epoch, client_key, err
            ));
            force_remote_control_ws_reconnect(
                &state,
                connection_epoch,
                &client_key,
                "same-stream initialize failed",
            )
            .await
        }
        Err(_) => {
            chain_log::write_line(format!(
                "[remote_control] event=recovery_initialize_timeout connection_epoch={} client_key={} timeout_ms={}",
                connection_epoch,
                client_key,
                REMOTE_CONTROL_UNKNOWN_REINITIALIZE_TIMEOUT.as_millis()
            ));
            force_remote_control_ws_reconnect(
                &state,
                connection_epoch,
                &client_key,
                "same-stream initialize timed out",
            )
            .await
        }
    }
}

async fn finish_remote_control_client_recovery(
    state: &SharedState,
    connection_epoch: u64,
    client_key: &str,
) -> Result<(String, String, u64)> {
    let mut remote = state.remote_control.inner.lock().await;
    if !connection_exists_locked(&remote, connection_epoch) {
        return Err(anyhow!("remote-control recovery epoch changed"));
    }
    let client_key =
        resolve_remote_client_key_for_connection_locked(&remote, connection_epoch, client_key);
    let (client_id, stream_id, attempt) = {
        let client =
            client_state_mut_for_connection_locked(&mut remote, connection_epoch, &client_key)
                .ok_or_else(|| {
                    anyhow!("remote-control recovery client disappeared: {client_key}")
                })?;
        let client_id = client.client_id.clone();
        let stream_id = client.stream_id.clone();
        let attempt = client.recovery_attempt;
        client.recovery_started_at_ms = None;
        (client_id, stream_id, attempt)
    };
    if let Some(connection) = connection_for_epoch_mut_locked(&mut remote, connection_epoch) {
        connection.last_error = None;
    }
    Ok((client_id, stream_id, attempt))
}

pub(super) async fn resubscribe_bound_threads_after_recovery(
    state: &SharedState,
    connection_epoch: u64,
    client_key: &str,
    attempt: u64,
) -> Result<()> {
    let requested_client_key = normalize_remote_client_key(client_key);
    let (client_key, client_id, stream_id) = {
        let remote = state.remote_control.inner.lock().await;
        if !connection_exists_locked(&remote, connection_epoch) {
            return Ok(());
        }
        let client_key = resolve_remote_client_key_for_connection_locked(
            &remote,
            connection_epoch,
            &requested_client_key,
        );
        let Some(client) =
            client_state_for_connection_locked(&remote, connection_epoch, &client_key)
        else {
            return Ok(());
        };
        if !client.initialized {
            return Ok(());
        }
        (
            client_key,
            client.client_id.clone(),
            client.stream_id.clone(),
        )
    };
    let mut targets = {
        let runtime = state.runtime.lock().await;
        runtime
            .route_by_thread
            .iter()
            .filter(|&(_, route)| {
                route_remote_client_key_matches(&route.remote_client_key, &client_key)
            })
            .map(|(thread_id, _)| (thread_id.clone(), "bound_route"))
            .collect::<Vec<_>>()
    };
    targets.sort_by(|left, right| left.0.cmp(&right.0));
    targets.dedup_by(|left, right| left.0 == right.0);

    if targets.is_empty() {
        chain_log::write_line(format!(
            "[remote_control] event=recovery_thread_resubscribe_skipped connection_epoch={} client_key={} attempt={} reason=no_bound_threads",
            connection_epoch, client_key, attempt
        ));
        return Ok(());
    };

    for (thread_id, source) in targets {
        resubscribe_thread_after_recovery(
            state,
            connection_epoch,
            &client_key,
            &client_id,
            &stream_id,
            &thread_id,
            attempt,
            source,
        )
        .await?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn resubscribe_thread_after_recovery(
    state: &SharedState,
    connection_epoch: u64,
    client_key: &str,
    client_id: &str,
    stream_id: &str,
    thread_id: &str,
    attempt: u64,
    source: &str,
) -> Result<()> {
    chain_log::write_line(format!(
        "[remote_control] event=recovery_thread_resubscribe_start connection_epoch={} client_key={} client_id={} stream_id={} thread={} attempt={} source={} method=thread/resume exclude_turns=true",
        connection_epoch, client_key, client_id, stream_id, thread_id, attempt, source
    ));
    state
        .push_event(
            "info",
            "remote_control_recovery_thread_resubscribe_start",
            format!(
                "client_key={} thread={} attempt={}",
                client_key, thread_id, attempt
            ),
        )
        .await;

    let response = match request_once_with_timeout_for_client_on_connection(
        state,
        connection_epoch,
        client_key,
        "thread/resume",
        json!({
            "threadId": thread_id,
            "excludeTurns": true,
        }),
        REMOTE_DISCOVERY_REQUEST_TIMEOUT,
    )
    .await
    {
        Ok(response) => response,
        Err(err) if is_missing_rollout_error(&err) => {
            chain_log::write_line(format!(
                "[remote_control] event=recovery_thread_resubscribe_missing_rollout connection_epoch={} client_key={} thread={} attempt={} source={} err={}",
                connection_epoch, client_key, thread_id, attempt, source, err
            ));
            state
                .push_event(
                    "warn",
                    "remote_control_recovery_thread_resubscribe_missing_rollout",
                    format!(
                        "client_key={} thread={} attempt={} source={} err={}",
                        client_key, thread_id, attempt, source, err
                    ),
                )
                .await;
            clear_thread_binding_for_thread_with_reason(
                state,
                thread_id,
                client_key,
                "recovery_missing_rollout",
            )
            .await?;
            return Ok(());
        }
        Err(err) if is_stale_thread_error(&err) => {
            chain_log::write_line(format!(
                "[remote_control] event=recovery_thread_resubscribe_stale connection_epoch={} client_key={} thread={} attempt={} source={} err={}",
                connection_epoch, client_key, thread_id, attempt, source, err
            ));
            state
                .push_event(
                    "warn",
                    "remote_control_recovery_thread_resubscribe_stale",
                    format!(
                        "client_key={} thread={} attempt={} source={} err={}",
                        client_key, thread_id, attempt, source, err
                    ),
                )
                .await;
            clear_thread_binding_for_thread_with_reason(
                state,
                thread_id,
                client_key,
                "recovery_stale_thread",
            )
            .await?;
            return Ok(());
        }
        Err(err) => return Err(err),
    };
    let status_type = response
        .get("thread")
        .and_then(thread_status_type_from_payload)
        .or_else(|| thread_status_type_from_payload(&response))
        .unwrap_or_default();
    chain_log::write_line(format!(
        "[remote_control] event=recovery_thread_resubscribe_ready connection_epoch={} client_key={} thread={} attempt={} status={}",
        connection_epoch, client_key, thread_id, attempt, status_type
    ));
    state
        .push_event(
            "info",
            "remote_control_recovery_thread_resubscribe_ready",
            format!(
                "client_key={} thread={} attempt={} status={}",
                client_key, thread_id, attempt, status_type
            ),
        )
        .await;
    Ok(())
}

fn is_missing_rollout_error(err: &anyhow::Error) -> bool {
    err.to_string().contains("no rollout found for thread id")
}

pub(super) async fn force_remote_control_ws_reconnect(
    state: &SharedState,
    connection_epoch: u64,
    client_key: &str,
    reason: &str,
) -> Result<()> {
    let outbound_tx = {
        let mut remote = state.remote_control.inner.lock().await;
        if !connection_exists_locked(&remote, connection_epoch) {
            return Ok(());
        }
        if let Some(connection) = connection_for_epoch_mut_locked(&mut remote, connection_epoch) {
            connection.last_error = Some(format!(
                "remote-control recovery forcing websocket reconnect: client_key={} reason={}",
                client_key, reason
            ));
        }
        outbound_tx_for_connection_epoch_locked(&remote, connection_epoch)
            .ok_or_else(|| anyhow!("remote-control websocket is not connected"))?
    };
    chain_log::write_line(format!(
        "[remote_control] event=force_ws_reconnect connection_epoch={} client_key={} reason={}",
        connection_epoch, client_key, reason
    ));
    outbound_tx
        .send(OutboundWsMessage::Close(reason.to_string()))
        .map_err(|_| anyhow!("remote-control outbound channel closed"))?;
    Ok(())
}
