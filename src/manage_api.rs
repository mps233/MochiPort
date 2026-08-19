//! Versioned local management API primitives.
//!
//! The management API deliberately has its own bearer credential.  Existing
//! legacy routes remain untouched during the SwiftUI migration; new clients
//! use the versioned routes defined in `web.rs`.

use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::OnceLock,
};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use axum::{
    Json,
    body::Body,
    extract::State,
    http::{
        HeaderMap, Request, StatusCode,
        header::{AUTHORIZATION, WWW_AUTHENTICATE},
    },
    middleware::Next,
    response::{IntoResponse, Response},
};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{app_state::SharedState, daemon_process::DaemonIdentity, types::now_ms, version};

pub const API_MAJOR: u16 = 1;
const CONTROL_FILE_NAME: &str = "threadrelay-control.json";
const CONTROL_LOCK_FILE_NAME: &str = "threadrelay-control.lock";
const ACTIVE_DAEMON_FILE_NAME: &str = "threadrelay-active-daemon.json";
const MANAGEMENT_LEASE_DURATION_MS: u64 = 30_000;
const CREDENTIAL_ROTATION_REASON_TAKEOVER: &str = "trustedTakeover";
const CREDENTIAL_ROTATION_REASON_LEAK: &str = "leakRecovery";

static EXECUTABLE_IDENTITY: OnceLock<Result<ExecutableIdentity, String>> = OnceLock::new();

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HealthResponse {
    pub service: &'static str,
    pub api_major: u16,
    pub ready: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ManageStatusResponse {
    pub service: String,
    pub api_major: u16,
    pub ready: bool,
    pub instance_id: String,
    pub pid: u32,
    pub started_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleServiceIdentity {
    pub service: String,
    pub api_major: u16,
    pub ready: bool,
    pub instance_id: String,
    pub pid: u32,
    pub started_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleRuntimeStatus {
    pub state: &'static str,
    pub product_version: &'static str,
    pub build_number: Option<u64>,
    pub api_major: u16,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleProtectedWorkItems {
    pub ai_gateway_requests: usize,
    pub codex_turns: usize,
    pub enhanced_launches: usize,
    pub im_streams: usize,
    pub pending_approvals: usize,
    pub remote_control_requests: usize,
    pub total: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleManagementOwnership {
    /// The daemon reports an active lease without exposing the management
    /// credential. Clients compare `installationId` with their own identity.
    pub state: &'static str,
    pub mode: &'static str,
    pub can_control: bool,
    pub installation_id: Option<String>,
    pub lease_generation: Option<u64>,
    pub lease_expires_at_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub management_token_generation: Option<u64>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleResponse {
    pub service: LifecycleServiceIdentity,
    pub executable: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub executable_sha256: Option<String>,
    pub config_path: String,
    pub bind: String,
    pub runtime: LifecycleRuntimeStatus,
    pub protected_work_items: LifecycleProtectedWorkItems,
    pub management: LifecycleManagementOwnership,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ControlFile {
    management_token: String,
    #[serde(default = "default_management_token_generation")]
    management_token_generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lease: Option<ControlLease>,
    #[serde(default, skip_serializing_if = "is_zero")]
    lease_generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_credential_rotation: Option<CredentialRotationRecord>,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct ControlLease {
    installation_id: String,
    daemon_instance_id: String,
    generation: u64,
    expires_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct CredentialRotationRecord {
    request_id: String,
    installation_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lease_generation: Option<u64>,
    previous_generation: u64,
    generation: u64,
    reason: String,
    rotated_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleDaemonIdentityProof {
    pub pid: u32,
    pub started_at_ms: u64,
    pub executable: String,
    pub executable_sha256: String,
    pub bind: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleLeaseRequest {
    pub installation_id: String,
    pub daemon_instance_id: String,
    #[serde(default)]
    pub daemon_identity: Option<LifecycleDaemonIdentityProof>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleLeaseTakeoverRequest {
    pub installation_id: String,
    pub daemon_instance_id: String,
    pub expected_lease_generation: u64,
    pub expected_management_token_generation: u64,
    pub request_id: String,
    pub force: bool,
    pub daemon_identity: LifecycleDaemonIdentityProof,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleCredentialRotateRequest {
    pub installation_id: String,
    pub daemon_instance_id: String,
    pub lease_generation: u64,
    pub expected_management_token_generation: u64,
    pub request_id: String,
    pub reason: String,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CredentialRotationResponse {
    pub ok: bool,
    pub rotated: bool,
    pub request_id: String,
    pub management_token_generation: u64,
}

#[derive(Debug, Clone)]
struct ExecutableIdentity {
    path: String,
    sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleControlRequest {
    pub installation_id: String,
    pub daemon_instance_id: String,
    #[serde(default)]
    pub force: bool,
    /// New clients send the lease generation to fence a restart request if
    /// ownership changes while existing work drains. It stays optional so
    /// older GUI clients remain compatible.
    #[serde(default)]
    pub lease_generation: Option<u64>,
}

#[derive(Debug, Clone, Copy)]
enum LeaseOperation {
    Claim,
    Renew,
    Release,
}

#[derive(Debug)]
pub(crate) struct LeaseError {
    status: StatusCode,
    message: String,
}

#[derive(Debug)]
pub(crate) enum LifecycleShutdownResult {
    Accepted,
    AlreadyInProgress,
    ProtectedWork(LifecycleProtectedWorkItems),
    LeaseRejected(LeaseError),
    NotRunning,
}

/// Run the single in-process shutdown path used by lifecycle restart and the
/// compatibility shutdown routes.  Admission closes before the first work
/// snapshot, so a request cannot enter between the safety check and the
/// shutdown signal. The second snapshot catches work that was already in
/// flight while the bridge was being stopped.
pub(crate) async fn request_shutdown_with_drain(
    state: &SharedState,
    force: bool,
    event_message: &'static str,
) -> LifecycleShutdownResult {
    let _lifecycle_control = state.lifecycle_control.lock().await;
    request_shutdown_with_drain_inner(state, force, event_message, None, None).await
}

pub(crate) async fn request_restart_with_drain(
    state: &SharedState,
    force: bool,
    event_message: &'static str,
    installation_id: &str,
    lease_generation: Option<u64>,
) -> LifecycleShutdownResult {
    // Keep lease mutations serialized with the complete asynchronous drain.
    // This is a Tokio mutex, so it does not block executor workers while the
    // protected work finishes.
    let _lifecycle_control = state.lifecycle_control.lock().await;
    let lease_lock =
        acquire_validated_lifecycle_lease_lock(state, installation_id, lease_generation).await;
    match lease_lock {
        Ok(lock) => {
            let _ = FileExt::unlock(&lock);
        }
        Err(error) => return LifecycleShutdownResult::LeaseRejected(error),
    }
    request_shutdown_with_drain_inner(
        state,
        force,
        event_message,
        Some(installation_id.to_string()),
        lease_generation,
    )
    .await
}

async fn request_shutdown_with_drain_inner(
    state: &SharedState,
    force: bool,
    event_message: &'static str,
    lease_installation_id: Option<String>,
    lease_generation: Option<u64>,
) -> LifecycleShutdownResult {
    if !state.lifecycle_admission.begin_draining() {
        return LifecycleShutdownResult::AlreadyInProgress;
    }

    state
        .push_event("warn", "shutdown_requested", event_message)
        .await;

    let initial = lifecycle_snapshot(state).await;
    if initial.protected_work_items.total > 0 && !force {
        state.lifecycle_admission.cancel_draining();
        return LifecycleShutdownResult::ProtectedWork(initial.protected_work_items);
    }

    // Stop polling/streaming before the final snapshot. Any handler that had
    // already passed the admission gate keeps its permit until it returns.
    crate::web::stop_bridge_for_lifecycle_shutdown(state).await;
    state.lifecycle_admission.wait_for_drain().await;

    let final_snapshot = lifecycle_snapshot(state).await;
    if final_snapshot.protected_work_items.total > 0 && !force {
        cancel_draining_and_restore_bridge(state).await;
        return LifecycleShutdownResult::ProtectedWork(final_snapshot.protected_work_items);
    }

    let lifecycle_lock = if let Some(installation_id) = lease_installation_id {
        let lease_lock =
            acquire_validated_lifecycle_lease_lock(state, &installation_id, lease_generation).await;
        match lease_lock {
            Ok(lock) => Some(lock),
            Err(error) => {
                cancel_draining_and_restore_bridge(state).await;
                return LifecycleShutdownResult::LeaseRejected(error);
            }
        }
    } else {
        None
    };

    // Keep the validated lease lock across the shutdown signal and the local
    // admission commit. The await only acquires the in-process shutdown
    // sender mutex; holding this Send file handle here fences lease takeover
    // until this daemon has committed to shutting down.
    if state.request_shutdown().await {
        state.lifecycle_admission.commit_shutdown();
        if let Some(lock) = lifecycle_lock.as_ref() {
            let _ = FileExt::unlock(lock);
        }
        LifecycleShutdownResult::Accepted
    } else {
        if let Some(lock) = lifecycle_lock.as_ref() {
            let _ = FileExt::unlock(lock);
        }
        cancel_draining_and_restore_bridge(state).await;
        LifecycleShutdownResult::NotRunning
    }
}

async fn cancel_draining_and_restore_bridge(state: &SharedState) {
    if state.lifecycle_admission.cancel_draining() {
        let _ = crate::web::start_bridge_if_ready(state, "lifecycle drain cancelled").await;
    }
}

impl LeaseError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ActiveDaemonLocator {
    pub service: String,
    pub api_major: u16,
    pub instance_id: String,
    pub pid: u32,
    pub started_at_ms: u64,
    pub base_url: String,
    pub control_file: String,
}

pub struct ActiveDaemonLocatorGuard {
    path: PathBuf,
    instance_id: String,
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("failed to prepare management control file")]
    Io(#[source] std::io::Error),
    #[error("management control file is invalid")]
    InvalidControlFile,
    #[error("active daemon discovery file is invalid")]
    InvalidDiscoveryFile,
}

/// Return the control-plane file for the user-data domain containing the
/// daemon config.  Stable, preview and bridge binaries therefore discover the
/// same credential when they share a config directory.
pub fn control_file_path(config_path: &Path) -> PathBuf {
    let parent = config_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    parent.join(CONTROL_FILE_NAME)
}

/// Ensure the shared management credential exists.  The token is never
/// returned to logs or serialized into an HTTP response by this module.
pub fn ensure_management_token(config_path: &Path) -> Result<(), AuthError> {
    let _ = load_or_create_management_token(config_path)?;
    Ok(())
}

/// Load the user-domain management credential for a trusted local client.
/// Callers must keep the returned token out of logs and serialized responses.
pub fn management_token(config_path: &Path) -> Result<String, AuthError> {
    load_or_create_management_token(config_path)
}

pub fn active_daemon_locator_path() -> Result<PathBuf, AuthError> {
    #[cfg(target_os = "windows")]
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("APPDATA"))
        .map(PathBuf::from);

    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join("Library/Application Support"));

    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join(".local/state"))
        });

    base.map(|base| base.join("ThreadRelay").join(ACTIVE_DAEMON_FILE_NAME))
        .ok_or_else(|| {
            AuthError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "user data directory is unavailable",
            ))
        })
}

pub fn publish_active_daemon_locator(
    config_path: &Path,
    identity: &DaemonIdentity,
    base_url: &str,
) -> Result<ActiveDaemonLocatorGuard, AuthError> {
    let path = active_daemon_locator_path()?;
    publish_active_daemon_locator_at(&path, config_path, identity, base_url)
}

fn publish_active_daemon_locator_at(
    path: &Path,
    config_path: &Path,
    identity: &DaemonIdentity,
    base_url: &str,
) -> Result<ActiveDaemonLocatorGuard, AuthError> {
    let locator = ActiveDaemonLocator {
        service: identity.service.clone(),
        api_major: API_MAJOR,
        instance_id: identity.instance_id.clone(),
        pid: identity.pid,
        started_at_ms: identity.started_at_ms,
        base_url: base_url.to_string(),
        control_file: control_file_path(config_path)
            .to_string_lossy()
            .into_owned(),
    };
    let contents = serde_json::to_vec(&locator).map_err(|_| AuthError::InvalidDiscoveryFile)?;
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or(AuthError::InvalidDiscoveryFile)?;
    fs::create_dir_all(parent).map_err(AuthError::Io)?;

    let temporary = parent.join(format!(
        ".{ACTIVE_DAEMON_FILE_NAME}.{}.{}.tmp",
        std::process::id(),
        Uuid::new_v4().simple()
    ));
    let write_result = (|| {
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&temporary).map_err(AuthError::Io)?;
        file.write_all(&contents).map_err(AuthError::Io)?;
        file.write_all(b"\n").map_err(AuthError::Io)?;
        file.sync_all().map_err(AuthError::Io)?;
        atomic_replace_file(&temporary, path).map_err(AuthError::Io)?;
        enforce_private_permissions(path).map_err(AuthError::Io)?;
        sync_parent_directory(parent).map_err(AuthError::Io)
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write_result?;

    Ok(ActiveDaemonLocatorGuard {
        path: path.to_path_buf(),
        instance_id: identity.instance_id.clone(),
    })
}

impl Drop for ActiveDaemonLocatorGuard {
    fn drop(&mut self) {
        let belongs_to_this_instance = fs::read(&self.path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<ActiveDaemonLocator>(&bytes).ok())
            .is_some_and(|locator| locator.instance_id == self.instance_id);
        if belongs_to_this_instance {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub async fn healthz() -> Json<HealthResponse> {
    Json(HealthResponse {
        service: "threadrelay",
        api_major: API_MAJOR,
        ready: true,
    })
}

pub async fn status(State(state): State<SharedState>) -> Json<ManageStatusResponse> {
    Json(status_snapshot(&state))
}

pub fn status_snapshot(state: &SharedState) -> ManageStatusResponse {
    ManageStatusResponse {
        service: state.daemon_identity.service.clone(),
        api_major: API_MAJOR,
        ready: true,
        instance_id: state.daemon_identity.instance_id.clone(),
        pid: state.daemon_identity.pid,
        started_at_ms: state.daemon_identity.started_at_ms,
    }
}

pub async fn claim_lifecycle_lease(
    State(state): State<SharedState>,
    Json(request): Json<LifecycleLeaseRequest>,
) -> Response {
    lifecycle_lease_operation(&state, request, LeaseOperation::Claim).await
}

pub async fn renew_lifecycle_lease(
    State(state): State<SharedState>,
    Json(request): Json<LifecycleLeaseRequest>,
) -> Response {
    lifecycle_lease_operation(&state, request, LeaseOperation::Renew).await
}

pub async fn release_lifecycle_lease(
    State(state): State<SharedState>,
    Json(request): Json<LifecycleLeaseRequest>,
) -> Response {
    lifecycle_lease_operation(&state, request, LeaseOperation::Release).await
}

pub async fn takeover_lifecycle_lease(
    State(state): State<SharedState>,
    Json(request): Json<LifecycleLeaseTakeoverRequest>,
) -> Response {
    let _lifecycle_control = state.lifecycle_control.lock().await;
    let installation_id = match normalized_installation_id(&request.installation_id) {
        Ok(value) => value,
        Err(error) => return lease_error_response(error),
    };
    let request_id = match normalized_request_id(&request.request_id) {
        Ok(value) => value,
        Err(error) => return lease_error_response(error),
    };
    if !request.force {
        return lease_error_response(LeaseError::bad_request("接管后台服务必须由用户明确确认。"));
    }
    if request.daemon_instance_id.trim() != state.daemon_identity.instance_id {
        return lease_error_response(LeaseError::conflict("后台服务实例已变化，请刷新后重试。"));
    }
    if let Err(error) = validate_daemon_identity_proof(&state, &request.daemon_identity).await {
        return lease_error_response(error);
    }

    match takeover_lifecycle_lease_async(
        &state,
        installation_id,
        request.expected_lease_generation,
        request.expected_management_token_generation,
        request_id,
    )
    .await
    {
        Ok(response) => Json(response).into_response(),
        Err(error) => lease_error_response(error),
    }
}

pub async fn rotate_management_credential(
    State(state): State<SharedState>,
    Json(request): Json<LifecycleCredentialRotateRequest>,
) -> Response {
    let _lifecycle_control = state.lifecycle_control.lock().await;
    let installation_id = match normalized_installation_id(&request.installation_id) {
        Ok(value) => value,
        Err(error) => return lease_error_response(error),
    };
    let request_id = match normalized_request_id(&request.request_id) {
        Ok(value) => value,
        Err(error) => return lease_error_response(error),
    };
    if request.daemon_instance_id.trim() != state.daemon_identity.instance_id {
        return lease_error_response(LeaseError::conflict("后台服务实例已变化，请刷新后重试。"));
    }
    if request.reason != CREDENTIAL_ROTATION_REASON_LEAK {
        return lease_error_response(LeaseError::bad_request("凭据轮换原因无效。"));
    }

    match rotate_management_credential_async(
        &state,
        installation_id,
        request.lease_generation,
        request.expected_management_token_generation,
        request_id,
        request.reason,
    )
    .await
    {
        Ok(response) => Json(response).into_response(),
        Err(error) => lease_error_response(error),
    }
}

pub async fn restart_lifecycle(
    State(state): State<SharedState>,
    Json(request): Json<LifecycleControlRequest>,
) -> Response {
    let installation_id = match normalized_installation_id(&request.installation_id) {
        Ok(value) => value,
        Err(error) => return lease_error_response(error),
    };
    if request.daemon_instance_id.trim() != state.daemon_identity.instance_id {
        return lease_error_response(LeaseError::conflict("后台服务实例已变化，请刷新后重试。"));
    }
    match request_restart_with_drain(
        &state,
        request.force,
        "lifecycle restart requested",
        &installation_id,
        request.lease_generation,
    )
    .await
    {
        LifecycleShutdownResult::Accepted => (
            StatusCode::OK,
            Json(json!({ "ok": true, "state": "restarting" })),
        )
            .into_response(),
        LifecycleShutdownResult::NotRunning => (
            StatusCode::OK,
            Json(json!({ "ok": false, "state": "not_running" })),
        )
            .into_response(),
        LifecycleShutdownResult::AlreadyInProgress => (
            StatusCode::CONFLICT,
            Json(json!({
                "ok": false,
                "state": "draining",
                "error": "后台服务正在关闭或重启，请稍后重试。",
            })),
        )
            .into_response(),
        LifecycleShutdownResult::ProtectedWork(protected_work_items) => (
            StatusCode::CONFLICT,
            Json(json!({
                "ok": false,
                "state": "active",
                "error": format!(
                    "后台服务仍有 {} 项受保护任务，已取消重启。",
                    protected_work_items.total
                ),
                "protectedWorkItems": protected_work_items,
            })),
        )
            .into_response(),
        LifecycleShutdownResult::LeaseRejected(error) => lease_error_response(error),
    }
}

async fn lifecycle_lease_operation(
    state: &SharedState,
    request: LifecycleLeaseRequest,
    operation: LeaseOperation,
) -> Response {
    let _lifecycle_control = state.lifecycle_control.lock().await;
    let installation_id = match normalized_installation_id(&request.installation_id) {
        Ok(value) => value,
        Err(error) => return lease_error_response(error),
    };
    if request.daemon_instance_id.trim() != state.daemon_identity.instance_id {
        return lease_error_response(LeaseError::conflict("后台服务实例已变化，请刷新后重试。"));
    }
    if matches!(operation, LeaseOperation::Claim) {
        let Some(proof) = request.daemon_identity.as_ref() else {
            return lease_error_response(LeaseError::bad_request(
                "申请管理租约需要后台服务身份信息。",
            ));
        };
        if let Err(error) = validate_daemon_identity_proof(state, proof).await {
            return lease_error_response(error);
        }
    }

    // Keep lease mutations behind the same in-process lifecycle mutex used by
    // restart. This prevents a heartbeat/release from changing ownership in
    // the middle of a local drain.
    match update_lifecycle_lease_async(state, installation_id.clone(), operation).await {
        Ok(()) => Json(lifecycle_snapshot(state).await).into_response(),
        Err(error) => lease_error_response(error),
    }
}

fn lease_error_response(error: LeaseError) -> Response {
    (
        error.status,
        Json(json!({
            "error": error.message,
        })),
    )
        .into_response()
}

/// Build a lifecycle snapshot for the authenticated management API. Reading
/// the snapshot never grants or renews a lease.
pub async fn lifecycle_snapshot(state: &SharedState) -> LifecycleResponse {
    let config = state.config.lock().await;
    let bind = config.bind.clone();
    let config_path = state.config_path.to_string_lossy().into_owned();
    drop(config);

    let (codex_turns, im_streams, pending_approvals) = {
        let runtime = state.runtime.lock().await;
        runtime.protected_work_item_counts()
    };
    let ai_gateway_requests = crate::ai_gateway::handler::in_flight_count();
    let enhanced_launches = state.enhanced_launch_operations.protected_work_count();
    let remote_control_requests = {
        let remote = state.remote_control.inner.lock().await;
        if remote.connections.is_empty() {
            remote
                .clients
                .values()
                .map(|client| client.pending.len())
                .sum()
        } else {
            remote
                .connections
                .values()
                .flat_map(|connection| connection.clients.values())
                .map(|client| client.pending.len())
                .sum()
        }
    };
    let total = ai_gateway_requests
        .saturating_add(codex_turns)
        .saturating_add(enhanced_launches)
        .saturating_add(im_streams)
        .saturating_add(pending_approvals)
        .saturating_add(remote_control_requests);
    let executable_identity = current_executable_identity().ok();
    let executable = executable_identity
        .map(|identity| identity.path.clone())
        .or_else(|| {
            std::env::current_exe()
                .ok()
                .map(|path| path.to_string_lossy().into_owned())
        })
        .unwrap_or_default();

    let management = lifecycle_management_snapshot(&state.config_path, &state.daemon_identity);

    LifecycleResponse {
        service: LifecycleServiceIdentity {
            service: state.daemon_identity.service.clone(),
            api_major: API_MAJOR,
            ready: true,
            instance_id: state.daemon_identity.instance_id.clone(),
            pid: state.daemon_identity.pid,
            started_at_ms: state.daemon_identity.started_at_ms,
        },
        executable,
        executable_sha256: executable_identity.map(|identity| identity.sha256.clone()),
        config_path,
        bind,
        runtime: LifecycleRuntimeStatus {
            state: state.lifecycle_admission.state().as_str(),
            product_version: version::PRODUCT_VERSION,
            build_number: version::build_number(),
            api_major: API_MAJOR,
        },
        protected_work_items: LifecycleProtectedWorkItems {
            ai_gateway_requests,
            codex_turns,
            enhanced_launches,
            im_streams,
            pending_approvals,
            remote_control_requests,
            total,
        },
        management,
    }
}

fn lifecycle_management_snapshot(
    config_path: &Path,
    identity: &DaemonIdentity,
) -> LifecycleManagementOwnership {
    let Ok(control) = read_control_file(config_path) else {
        return read_only_management(None);
    };
    let management_token_generation = Some(control.management_token_generation);
    let Some(lease) = control.lease else {
        return read_only_management(management_token_generation);
    };
    if lease.expires_at_ms <= current_time_ms() {
        return read_only_management(management_token_generation);
    }
    if lease.daemon_instance_id != identity.instance_id {
        return LifecycleManagementOwnership {
            state: "conflict",
            mode: "readOnly",
            can_control: false,
            installation_id: Some(lease.installation_id),
            lease_generation: Some(lease.generation),
            lease_expires_at_ms: Some(lease.expires_at_ms),
            management_token_generation,
        };
    }
    LifecycleManagementOwnership {
        state: "managed",
        mode: "managed",
        can_control: true,
        installation_id: Some(lease.installation_id),
        lease_generation: Some(lease.generation),
        lease_expires_at_ms: Some(lease.expires_at_ms),
        management_token_generation,
    }
}

fn read_only_management(management_token_generation: Option<u64>) -> LifecycleManagementOwnership {
    LifecycleManagementOwnership {
        state: "unmanaged",
        mode: "readOnly",
        can_control: false,
        installation_id: None,
        lease_generation: None,
        lease_expires_at_ms: None,
        management_token_generation,
    }
}

fn normalized_installation_id(value: &str) -> Result<String, LeaseError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() > 128 || trimmed != value {
        return Err(LeaseError::bad_request("installationId 无效。"));
    }
    Ok(trimmed.to_string())
}

fn normalized_request_id(value: &str) -> Result<String, LeaseError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() > 128 || trimmed != value {
        return Err(LeaseError::bad_request("requestId 无效。"));
    }
    Ok(trimmed.to_string())
}

fn current_time_ms() -> u64 {
    now_ms().min(u64::MAX as u128) as u64
}

fn default_management_token_generation() -> u64 {
    1
}

fn current_executable_identity() -> Result<&'static ExecutableIdentity, LeaseError> {
    EXECUTABLE_IDENTITY
        .get_or_init(|| {
            let path = std::env::current_exe().map_err(|error| error.to_string())?;
            let mut file = File::open(&path).map_err(|error| error.to_string())?;
            let mut hasher = Sha256::new();
            let mut buffer = [0_u8; 64 * 1024];
            loop {
                let read = file.read(&mut buffer).map_err(|error| error.to_string())?;
                if read == 0 {
                    break;
                }
                hasher.update(&buffer[..read]);
            }
            Ok(ExecutableIdentity {
                path: path.to_string_lossy().into_owned(),
                sha256: hex::encode(hasher.finalize()),
            })
        })
        .as_ref()
        .map_err(|_| LeaseError::internal("无法校验后台服务可执行文件。"))
}

async fn validate_daemon_identity_proof(
    state: &SharedState,
    proof: &LifecycleDaemonIdentityProof,
) -> Result<(), LeaseError> {
    let executable = current_executable_identity()?;
    let bind = state.config.lock().await.bind.clone();
    if proof.pid != state.daemon_identity.pid
        || proof.started_at_ms != state.daemon_identity.started_at_ms
        || proof.executable != executable.path
        || proof.executable_sha256 != executable.sha256
        || proof.bind != bind
    {
        return Err(LeaseError::conflict("后台服务身份校验失败，请刷新后重试。"));
    }
    Ok(())
}

fn checked_generation(value: u64, message: &'static str) -> Result<u64, LeaseError> {
    value
        .checked_add(1)
        .ok_or_else(|| LeaseError::conflict(message))
}

fn checked_lease_expiry(now_ms: u64) -> Result<u64, LeaseError> {
    now_ms
        .checked_add(MANAGEMENT_LEASE_DURATION_MS)
        .ok_or_else(|| LeaseError::internal("无法计算后台服务管理租约有效期。"))
}

fn credential_rotation_replay(
    control: &ControlFile,
    request_id: &str,
    installation_id: &str,
    lease_generation: u64,
    expected_management_token_generation: u64,
    reason: &str,
) -> Result<Option<CredentialRotationResponse>, LeaseError> {
    let Some(record) = control.last_credential_rotation.as_ref() else {
        return Ok(None);
    };
    if record.request_id != request_id {
        return Ok(None);
    }
    if record.installation_id != installation_id
        || record.lease_generation != Some(lease_generation)
        || record.previous_generation != expected_management_token_generation
        || record.reason != reason
        || record.generation != control.management_token_generation
    {
        return Err(LeaseError::conflict("requestId 已用于另一项管理凭据操作。"));
    }
    Ok(Some(CredentialRotationResponse {
        ok: true,
        rotated: false,
        request_id: request_id.to_string(),
        management_token_generation: record.generation,
    }))
}

fn rotate_management_credential_locked(
    config_path: &Path,
    identity: &DaemonIdentity,
    installation_id: &str,
    lease_generation: u64,
    expected_management_token_generation: u64,
    request_id: &str,
    reason: &str,
    now_ms: u64,
) -> Result<CredentialRotationResponse, LeaseError> {
    let mut control = read_control_file_unlocked(config_path)
        .map_err(|_| LeaseError::internal("后台服务管理控制文件格式无效。"))?;
    if let Some(response) = credential_rotation_replay(
        &control,
        request_id,
        installation_id,
        lease_generation,
        expected_management_token_generation,
        reason,
    )? {
        return Ok(response);
    }
    if control.management_token_generation != expected_management_token_generation {
        return Err(LeaseError::conflict(
            "管理凭据已由其他操作更新，请刷新后重试。",
        ));
    }
    let Some(lease) = control
        .lease
        .as_ref()
        .filter(|lease| lease.expires_at_ms > now_ms)
    else {
        return Err(LeaseError::conflict("管理租约已过期，请重新申请。"));
    };
    if lease.installation_id != installation_id || lease.daemon_instance_id != identity.instance_id
    {
        return Err(LeaseError::conflict("当前安装不持有后台服务管理租约。"));
    }
    if lease.generation != lease_generation {
        return Err(LeaseError::conflict(
            "后台服务管理租约已换代，请刷新后重试。",
        ));
    }

    let generation = checked_generation(
        control.management_token_generation,
        "管理凭据 generation 已达到上限。",
    )?;
    control.management_token = Uuid::new_v4().simple().to_string();
    control.management_token_generation = generation;
    control.last_credential_rotation = Some(CredentialRotationRecord {
        request_id: request_id.to_string(),
        installation_id: installation_id.to_string(),
        lease_generation: Some(lease_generation),
        previous_generation: expected_management_token_generation,
        generation,
        reason: reason.to_string(),
        rotated_at_ms: now_ms,
    });
    write_control_file_unlocked(config_path, &control)
        .map_err(|_| LeaseError::internal("无法保存新的后台服务管理凭据。"))?;
    Ok(CredentialRotationResponse {
        ok: true,
        rotated: true,
        request_id: request_id.to_string(),
        management_token_generation: generation,
    })
}

fn rotate_management_credential_transaction(
    config_path: &Path,
    identity: &DaemonIdentity,
    installation_id: &str,
    lease_generation: u64,
    expected_management_token_generation: u64,
    request_id: &str,
    reason: &str,
    now_ms: u64,
) -> Result<CredentialRotationResponse, LeaseError> {
    ensure_management_token(config_path)
        .map_err(|_| LeaseError::internal("无法读取后台服务管理控制文件。"))?;
    let lock = acquire_lifecycle_lock(config_path)?;
    let result = rotate_management_credential_locked(
        config_path,
        identity,
        installation_id,
        lease_generation,
        expected_management_token_generation,
        request_id,
        reason,
        now_ms,
    );
    let _ = FileExt::unlock(&lock);
    result
}

async fn rotate_management_credential_async(
    state: &SharedState,
    installation_id: String,
    lease_generation: u64,
    expected_management_token_generation: u64,
    request_id: String,
    reason: String,
) -> Result<CredentialRotationResponse, LeaseError> {
    let config_path = state.config_path.clone();
    let identity = state.daemon_identity.clone();
    tokio::task::spawn_blocking(move || {
        rotate_management_credential_transaction(
            &config_path,
            &identity,
            &installation_id,
            lease_generation,
            expected_management_token_generation,
            &request_id,
            &reason,
            current_time_ms(),
        )
    })
    .await
    .map_err(|_| LeaseError::internal("后台服务管理锁任务失败。"))?
}

fn takeover_lifecycle_lease_transaction(
    config_path: &Path,
    identity: &DaemonIdentity,
    installation_id: &str,
    expected_lease_generation: u64,
    expected_management_token_generation: u64,
    request_id: &str,
    now_ms: u64,
) -> Result<CredentialRotationResponse, LeaseError> {
    ensure_management_token(config_path)
        .map_err(|_| LeaseError::internal("无法读取后台服务管理控制文件。"))?;
    let lock = acquire_lifecycle_lock(config_path)?;
    let result = (|| {
        let mut control = read_control_file_unlocked(config_path)
            .map_err(|_| LeaseError::internal("后台服务管理控制文件格式无效。"))?;
        if let Some(response) = credential_rotation_replay(
            &control,
            request_id,
            installation_id,
            expected_lease_generation,
            expected_management_token_generation,
            CREDENTIAL_ROTATION_REASON_TAKEOVER,
        )? {
            let lease_matches_replay = control.lease.as_ref().is_some_and(|lease| {
                lease.expires_at_ms > now_ms
                    && lease.installation_id == installation_id
                    && lease.daemon_instance_id == identity.instance_id
            });
            if !lease_matches_replay {
                return Err(LeaseError::conflict(
                    "后台服务管理租约已变化，请刷新后重试。",
                ));
            }
            return Ok(response);
        }
        if control.management_token_generation != expected_management_token_generation {
            return Err(LeaseError::conflict(
                "管理凭据已由其他操作更新，请刷新后重试。",
            ));
        }
        let Some(existing) = control
            .lease
            .as_ref()
            .filter(|lease| lease.expires_at_ms > now_ms)
            .cloned()
        else {
            return Err(LeaseError::conflict(
                "原管理租约已经失效，请刷新后直接申请管理权。",
            ));
        };
        if existing.installation_id == installation_id {
            return Err(LeaseError::conflict("当前安装已经持有后台服务管理租约。"));
        }
        if existing.generation != expected_lease_generation {
            return Err(LeaseError::conflict(
                "后台服务管理租约已换代，请重新确认接管。",
            ));
        }

        let lease_generation = checked_generation(
            control.lease_generation.max(existing.generation),
            "后台服务管理租约 generation 已达到上限。",
        )?;
        let management_token_generation = checked_generation(
            control.management_token_generation,
            "管理凭据 generation 已达到上限。",
        )?;
        let expires_at_ms = checked_lease_expiry(now_ms)?;
        control.lease_generation = lease_generation;
        control.lease = Some(ControlLease {
            installation_id: installation_id.to_string(),
            daemon_instance_id: identity.instance_id.clone(),
            generation: lease_generation,
            expires_at_ms,
        });
        control.management_token = Uuid::new_v4().simple().to_string();
        control.management_token_generation = management_token_generation;
        control.last_credential_rotation = Some(CredentialRotationRecord {
            request_id: request_id.to_string(),
            installation_id: installation_id.to_string(),
            lease_generation: Some(expected_lease_generation),
            previous_generation: expected_management_token_generation,
            generation: management_token_generation,
            reason: CREDENTIAL_ROTATION_REASON_TAKEOVER.to_string(),
            rotated_at_ms: now_ms,
        });
        write_control_file_unlocked(config_path, &control)
            .map_err(|_| LeaseError::internal("无法保存后台服务管理接管状态。"))?;
        Ok(CredentialRotationResponse {
            ok: true,
            rotated: true,
            request_id: request_id.to_string(),
            management_token_generation,
        })
    })();
    let _ = FileExt::unlock(&lock);
    result
}

async fn takeover_lifecycle_lease_async(
    state: &SharedState,
    installation_id: String,
    expected_lease_generation: u64,
    expected_management_token_generation: u64,
    request_id: String,
) -> Result<CredentialRotationResponse, LeaseError> {
    let config_path = state.config_path.clone();
    let identity = state.daemon_identity.clone();
    tokio::task::spawn_blocking(move || {
        takeover_lifecycle_lease_transaction(
            &config_path,
            &identity,
            &installation_id,
            expected_lease_generation,
            expected_management_token_generation,
            &request_id,
            current_time_ms(),
        )
    })
    .await
    .map_err(|_| LeaseError::internal("后台服务管理锁任务失败。"))?
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

fn update_lifecycle_lease(
    config_path: &Path,
    identity: &DaemonIdentity,
    installation_id: &str,
    operation: LeaseOperation,
    now_ms: u64,
) -> Result<(), LeaseError> {
    ensure_management_token(config_path)
        .map_err(|_| LeaseError::internal("无法读取后台服务管理控制文件。"))?;

    let lock = acquire_lifecycle_lock(config_path)?;

    let result = (|| {
        let mut control = read_control_file_unlocked(config_path)
            .map_err(|_| LeaseError::internal("后台服务管理控制文件格式无效。"))?;
        let active = control
            .lease
            .as_ref()
            .filter(|lease| lease.expires_at_ms > now_ms);

        match operation {
            LeaseOperation::Claim => {
                if let Some(existing) = active
                    && existing.installation_id != installation_id
                {
                    return Err(LeaseError::conflict(
                        "后台服务已由其他安装管理，请先释放或等待管理租约过期。",
                    ));
                }

                let generation = if let Some(existing) = active
                    && existing.daemon_instance_id == identity.instance_id
                {
                    existing.generation
                } else {
                    control.lease_generation = checked_generation(
                        control.lease_generation,
                        "后台服务管理租约 generation 已达到上限。",
                    )?;
                    control.lease_generation
                };
                control.lease = Some(ControlLease {
                    installation_id: installation_id.to_string(),
                    daemon_instance_id: identity.instance_id.clone(),
                    generation,
                    expires_at_ms: checked_lease_expiry(now_ms)?,
                });
            }
            LeaseOperation::Renew => {
                let Some(existing) = active else {
                    return Err(LeaseError::conflict("管理租约已过期，请重新申请。"));
                };
                if existing.installation_id != installation_id
                    || existing.daemon_instance_id != identity.instance_id
                {
                    return Err(LeaseError::conflict("当前安装不持有后台服务管理租约。"));
                }
                control.lease = Some(ControlLease {
                    expires_at_ms: checked_lease_expiry(now_ms)?,
                    ..existing.clone()
                });
            }
            LeaseOperation::Release => {
                let Some(existing) = control.lease.as_ref() else {
                    return Ok(());
                };
                if existing.installation_id != installation_id
                    || existing.daemon_instance_id != identity.instance_id
                {
                    return Err(LeaseError::conflict("当前安装不持有后台服务管理租约。"));
                }
                control.lease = None;
            }
        }
        write_control_file_unlocked(config_path, &control)
            .map_err(|_| LeaseError::internal("无法保存后台服务管理租约。"))
    })();
    let _ = FileExt::unlock(&lock);
    result
}

async fn update_lifecycle_lease_async(
    state: &SharedState,
    installation_id: String,
    operation: LeaseOperation,
) -> Result<(), LeaseError> {
    let config_path = state.config_path.clone();
    let identity = state.daemon_identity.clone();
    tokio::task::spawn_blocking(move || {
        update_lifecycle_lease(
            &config_path,
            &identity,
            &installation_id,
            operation,
            current_time_ms(),
        )
    })
    .await
    .map_err(|_| LeaseError::internal("后台服务管理锁任务失败。"))?
}

fn acquire_lifecycle_lock(config_path: &Path) -> Result<File, LeaseError> {
    let lock = open_control_lock(config_path)
        .map_err(|_| LeaseError::internal("无法打开后台服务管理锁。"))?;
    lock.lock_exclusive()
        .map_err(|_| LeaseError::internal("无法锁定后台服务管理控制文件。"))?;
    Ok(lock)
}

fn open_control_lock(config_path: &Path) -> Result<File, AuthError> {
    let lock_path = control_lock_path(config_path);
    let parent = lock_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or(AuthError::InvalidControlFile)?;
    fs::create_dir_all(parent).map_err(AuthError::Io)?;
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    let lock = options.open(&lock_path).map_err(AuthError::Io)?;
    enforce_private_permissions(&lock_path).map_err(AuthError::Io)?;
    Ok(lock)
}

fn acquire_control_shared_lock(config_path: &Path) -> Result<File, AuthError> {
    let lock = open_control_lock(config_path)?;
    lock.lock_shared().map_err(AuthError::Io)?;
    Ok(lock)
}

fn validate_active_lifecycle_lease(
    config_path: &Path,
    identity: &DaemonIdentity,
    installation_id: &str,
    expected_generation: Option<u64>,
    now_ms: u64,
) -> Result<(), LeaseError> {
    let control = read_control_file_unlocked(config_path)
        .map_err(|_| LeaseError::internal("后台服务管理控制文件格式无效。"))?;
    let Some(lease) = control.lease else {
        return Err(LeaseError::conflict("当前安装不持有后台服务管理租约。"));
    };
    if lease.expires_at_ms <= now_ms {
        return Err(LeaseError::conflict("管理租约已过期，请重新申请。"));
    }
    if lease.installation_id != installation_id || lease.daemon_instance_id != identity.instance_id
    {
        return Err(LeaseError::conflict("当前安装不持有后台服务管理租约。"));
    }
    if expected_generation.is_some_and(|generation| lease.generation != generation) {
        return Err(LeaseError::conflict(
            "后台服务管理租约已换代，请刷新后重试。",
        ));
    }
    Ok(())
}

/// Validate a lifecycle lease while keeping fs2 lock acquisition and file I/O
/// off the Tokio worker. A successful call returns the still-locked file so a
/// caller can fence the final shutdown commit; the caller must unlock it.
async fn acquire_validated_lifecycle_lease_lock(
    state: &SharedState,
    installation_id: &str,
    expected_generation: Option<u64>,
) -> Result<File, LeaseError> {
    let config_path = state.config_path.clone();
    let identity = state.daemon_identity.clone();
    let installation_id = installation_id.to_string();
    tokio::task::spawn_blocking(move || {
        let lock = acquire_lifecycle_lock(&config_path)?;
        let result = validate_active_lifecycle_lease(
            &config_path,
            &identity,
            &installation_id,
            expected_generation,
            current_time_ms(),
        );
        match result {
            Ok(()) => Ok(lock),
            Err(error) => {
                let _ = FileExt::unlock(&lock);
                Err(error)
            }
        }
    })
    .await
    .map_err(|_| LeaseError::internal("后台服务管理锁任务失败。"))?
}

fn control_lock_path(config_path: &Path) -> PathBuf {
    control_file_path(config_path)
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .join(CONTROL_LOCK_FILE_NAME)
}

fn read_control_file(config_path: &Path) -> Result<ControlFile, AuthError> {
    let lock = acquire_control_shared_lock(config_path)?;
    let result = read_control_file_unlocked(config_path);
    let _ = FileExt::unlock(&lock);
    result
}

fn read_control_file_unlocked(config_path: &Path) -> Result<ControlFile, AuthError> {
    let path = control_file_path(config_path);
    let raw = fs::read_to_string(&path).map_err(AuthError::Io)?;
    enforce_private_permissions(&path).map_err(AuthError::Io)?;
    let control: ControlFile =
        serde_json::from_str(&raw).map_err(|_| AuthError::InvalidControlFile)?;
    let token = control.management_token.trim();
    if token.is_empty()
        || token != control.management_token
        || token.len() > 256
        || control.management_token_generation == 0
        || control
            .lease
            .as_ref()
            .is_some_and(|lease| lease.generation == 0)
        || control
            .last_credential_rotation
            .as_ref()
            .is_some_and(|record| {
                record.request_id.trim().is_empty()
                    || record.request_id.trim() != record.request_id
                    || record.installation_id.trim().is_empty()
                    || record.installation_id.trim() != record.installation_id
                    || record.lease_generation == Some(0)
                    || record.previous_generation == 0
                    || record.generation == 0
                    || record.generation > control.management_token_generation
            })
    {
        return Err(AuthError::InvalidControlFile);
    }
    Ok(control)
}

fn write_control_file_unlocked(config_path: &Path, control: &ControlFile) -> Result<(), AuthError> {
    let path = control_file_path(config_path);
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or(AuthError::InvalidControlFile)?;
    fs::create_dir_all(parent).map_err(AuthError::Io)?;
    let contents = serde_json::to_vec(control).map_err(|_| AuthError::InvalidControlFile)?;
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        CONTROL_FILE_NAME,
        Uuid::new_v4().simple()
    ));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&temporary).map_err(AuthError::Io)?;
        file.write_all(&contents).map_err(AuthError::Io)?;
        file.write_all(b"\n").map_err(AuthError::Io)?;
        file.sync_all().map_err(AuthError::Io)?;
        atomic_replace_file(&temporary, &path).map_err(AuthError::Io)?;
        enforce_private_permissions(&path).map_err(AuthError::Io)?;
        sync_parent_directory(parent).map_err(AuthError::Io)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(not(windows))]
fn atomic_replace_file(temporary: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(temporary, destination)
}

#[cfg(windows)]
fn atomic_replace_file(temporary: &Path, destination: &Path) -> std::io::Result<()> {
    use std::{os::windows::ffi::OsStrExt, ptr};
    use windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW, REPLACEFILE_WRITE_THROUGH,
        ReplaceFileW,
    };

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    let temporary = wide(temporary);
    let destination = wide(destination);
    let replaced = unsafe {
        ReplaceFileW(
            destination.as_ptr(),
            temporary.as_ptr(),
            ptr::null(),
            REPLACEFILE_WRITE_THROUGH,
            ptr::null(),
            ptr::null(),
        )
    };
    if replaced != 0 {
        return Ok(());
    }
    let replace_error = std::io::Error::last_os_error();
    if replace_error.raw_os_error() != Some(ERROR_FILE_NOT_FOUND as i32) {
        return Err(replace_error);
    }
    let moved = unsafe {
        MoveFileExW(
            temporary.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn sync_parent_directory(parent: &Path) -> std::io::Result<()> {
    File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent_directory(_parent: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Middleware for all `/api/v1/manage/*` routes.
pub async fn require_bearer(
    State(state): State<SharedState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let config_path = state.config_path.clone();
    let headers = request.headers().clone();
    let authorization =
        tokio::task::spawn_blocking(move || authorize(&config_path, &headers)).await;
    match authorization {
        Ok(Ok(true)) => next.run(request).await,
        Ok(Ok(false)) => unauthorized_response(),
        Ok(Err(_)) | Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "error": "management authentication unavailable",
            })),
        )
            .into_response(),
    }
}

/// Authenticate a request against the user-domain control file.
///
/// This function is kept small and synchronous so it can be exercised without
/// starting a daemon in unit tests.  The control file is loaded before parsing
/// the header so an installation with no file is initialized on its first
/// management request as well as during router construction.
pub fn authorize(config_path: &Path, headers: &HeaderMap) -> Result<bool, AuthError> {
    let expected = load_or_create_management_token(config_path)?;
    let Some(presented) = bearer_token(headers) else {
        return Ok(false);
    };
    Ok(constant_time_eq(expected.as_bytes(), presented.as_bytes()))
}

fn unauthorized_response() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(WWW_AUTHENTICATE, "Bearer")],
        Json(json!({ "error": "unauthorized" })),
    )
        .into_response()
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") || token.is_empty() || token.trim() != token {
        return None;
    }
    Some(token)
}

fn constant_time_eq(expected: &[u8], presented: &[u8]) -> bool {
    let mut difference = expected.len() ^ presented.len();
    for index in 0..expected.len().max(presented.len()) {
        let left = expected.get(index).copied().unwrap_or_default();
        let right = presented.get(index).copied().unwrap_or_default();
        difference |= usize::from(left ^ right);
    }
    difference == 0
}

fn load_or_create_management_token(config_path: &Path) -> Result<String, AuthError> {
    match read_control_file(config_path) {
        Ok(control) => return Ok(control.management_token),
        Err(AuthError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    let lock = open_control_lock(config_path)?;
    lock.lock_exclusive().map_err(AuthError::Io)?;
    let result = match read_control_file_unlocked(config_path) {
        Ok(control) => Ok(control.management_token),
        Err(AuthError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            let token = Uuid::new_v4().simple().to_string();
            let control = ControlFile {
                management_token: token.clone(),
                management_token_generation: default_management_token_generation(),
                lease: None,
                lease_generation: 0,
                last_credential_rotation: None,
                extra: BTreeMap::new(),
            };
            write_control_file_unlocked(config_path, &control).map(|()| token)
        }
        Err(error) => Err(error),
    };
    let _ = FileExt::unlock(&lock);
    result
}

fn enforce_private_permissions(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(0o600);
        fs::set_permissions(path, permissions)?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::header::AUTHORIZATION;
    use tempfile::tempdir;

    #[tokio::test]
    async fn health_response_contains_only_public_fields() {
        let Json(response) = healthz().await;
        assert_eq!(
            serde_json::to_value(response).expect("serialize health response"),
            json!({
                "service": "threadrelay",
                "apiMajor": 1,
                "ready": true,
            })
        );
    }

    #[test]
    fn lifecycle_runtime_includes_product_and_build_versions() {
        let runtime = LifecycleRuntimeStatus {
            state: "active",
            product_version: version::PRODUCT_VERSION,
            build_number: version::build_number(),
            api_major: API_MAJOR,
        };
        assert_eq!(
            serde_json::to_value(runtime).expect("serialize lifecycle runtime"),
            json!({
                "state": "active",
                "productVersion": version::PRODUCT_VERSION,
                "buildNumber": version::build_number(),
                "apiMajor": API_MAJOR,
            })
        );
    }

    #[test]
    fn management_auth_accepts_shared_token_and_rejects_missing_or_wrong_token() {
        let temp = tempdir().expect("tempdir");
        let config_path = temp.path().join("config.toml");
        ensure_management_token(&config_path).expect("create control file");
        let token = management_token(&config_path).expect("read control file");

        let mut valid = HeaderMap::new();
        valid.insert(
            AUTHORIZATION,
            format!("Bearer {token}").parse().expect("header value"),
        );
        assert!(authorize(&config_path, &valid).expect("valid auth"));

        let missing = HeaderMap::new();
        assert!(!authorize(&config_path, &missing).expect("missing auth"));

        let mut wrong = HeaderMap::new();
        wrong.insert(AUTHORIZATION, "Bearer definitely-wrong".parse().unwrap());
        assert!(!authorize(&config_path, &wrong).expect("wrong auth"));
    }

    #[test]
    fn explicit_zero_management_token_generation_is_rejected() {
        let temp = tempdir().expect("tempdir");
        let config_path = temp.path().join("config.toml");
        fs::write(
            control_file_path(&config_path),
            br#"{"managementToken":"legacy-token","managementTokenGeneration":0}"#,
        )
        .expect("write invalid control file");

        assert!(matches!(
            read_control_file(&config_path),
            Err(AuthError::InvalidControlFile)
        ));
    }

    #[test]
    fn lifecycle_lease_claim_renew_release_is_instance_and_installation_bound() {
        let temp = tempdir().expect("tempdir");
        let config_path = temp.path().join("config.toml");
        let identity = DaemonIdentity {
            service: "threadrelay".to_string(),
            pid: 42,
            instance_id: "daemon-instance".to_string(),
            started_at_ms: 123,
        };
        ensure_management_token(&config_path).expect("create control file");

        let now = current_time_ms();
        update_lifecycle_lease(
            &config_path,
            &identity,
            "installation-a",
            LeaseOperation::Claim,
            now,
        )
        .expect("claim lease");
        let claimed = read_control_file(&config_path).expect("read claimed control file");
        let lease = claimed.lease.expect("claimed lease");
        assert_eq!(lease.installation_id, "installation-a");
        assert_eq!(lease.daemon_instance_id, "daemon-instance");
        assert_eq!(lease.generation, 1);
        assert!(lease.expires_at_ms > now);
        assert!(lifecycle_management_snapshot(&config_path, &identity).can_control);

        update_lifecycle_lease(
            &config_path,
            &identity,
            "installation-a",
            LeaseOperation::Renew,
            now + 1_000,
        )
        .expect("renew lease");
        let renewed = read_control_file(&config_path)
            .expect("read renewed control file")
            .lease
            .expect("renewed lease");
        assert_eq!(renewed.generation, lease.generation);
        assert!(renewed.expires_at_ms > lease.expires_at_ms);

        let other = DaemonIdentity {
            instance_id: "other-daemon".to_string(),
            ..identity.clone()
        };
        let conflict = update_lifecycle_lease(
            &config_path,
            &other,
            "installation-b",
            LeaseOperation::Claim,
            now + 2_000,
        )
        .expect_err("second installation must not steal active lease");
        assert_eq!(conflict.status, StatusCode::CONFLICT);

        update_lifecycle_lease(
            &config_path,
            &other,
            "installation-a",
            LeaseOperation::Claim,
            now + 2_500,
        )
        .expect("same installation should rebind after daemon replacement");
        let rebound = read_control_file(&config_path)
            .expect("read rebound control file")
            .lease
            .expect("rebound lease");
        assert_eq!(rebound.installation_id, "installation-a");
        assert_eq!(rebound.daemon_instance_id, "other-daemon");
        assert!(rebound.generation > renewed.generation);
        assert!(!lifecycle_management_snapshot(&config_path, &identity).can_control);
        assert!(lifecycle_management_snapshot(&config_path, &other).can_control);

        update_lifecycle_lease(
            &config_path,
            &other,
            "installation-a",
            LeaseOperation::Release,
            now + 3_000,
        )
        .expect("release lease");
        assert!(
            read_control_file(&config_path)
                .expect("read released control file")
                .lease
                .is_none()
        );
        assert!(!lifecycle_management_snapshot(&config_path, &identity).can_control);
    }

    #[test]
    fn legacy_control_file_defaults_generation_and_preserves_unknown_fields() {
        let temp = tempdir().expect("tempdir");
        let config_path = temp.path().join("config.toml");
        let control_path = control_file_path(&config_path);
        fs::write(
            &control_path,
            br#"{"managementToken":"legacy-token","futureField":{"kept":true}}"#,
        )
        .expect("write legacy control file");

        let legacy = read_control_file(&config_path).expect("read legacy control file");
        assert_eq!(legacy.management_token_generation, 1);
        assert_eq!(legacy.extra["futureField"], json!({ "kept": true }));

        let identity = DaemonIdentity {
            service: "threadrelay".to_string(),
            pid: 42,
            instance_id: "daemon-instance".to_string(),
            started_at_ms: 123,
        };
        let now = current_time_ms();
        update_lifecycle_lease(
            &config_path,
            &identity,
            "installation-a",
            LeaseOperation::Claim,
            now,
        )
        .expect("claim legacy lease");
        let lease_generation = read_control_file(&config_path)
            .expect("read claimed control file")
            .lease
            .expect("claimed lease")
            .generation;
        let response = rotate_management_credential_transaction(
            &config_path,
            &identity,
            "installation-a",
            lease_generation,
            1,
            "rotation-request",
            CREDENTIAL_ROTATION_REASON_LEAK,
            now + 1,
        )
        .expect("rotate legacy credential");
        assert!(response.rotated);
        assert_eq!(response.management_token_generation, 2);

        let rotated = read_control_file(&config_path).expect("read rotated control file");
        assert_eq!(rotated.management_token_generation, 2);
        assert_eq!(rotated.extra["futureField"], json!({ "kept": true }));
        assert_ne!(rotated.management_token, "legacy-token");
        let serialized = serde_json::to_string(&response).expect("serialize response");
        assert!(!serialized.contains("legacy-token"));
        assert!(!serialized.contains(&rotated.management_token));

        let mismatched_replay = rotate_management_credential_transaction(
            &config_path,
            &identity,
            "installation-a",
            lease_generation,
            2,
            "rotation-request",
            CREDENTIAL_ROTATION_REASON_LEAK,
            now + 2,
        )
        .expect_err("same request ID with different CAS input must conflict");
        assert_eq!(mismatched_replay.status, StatusCode::CONFLICT);
        assert_eq!(
            management_token(&config_path).expect("token after rejected replay"),
            rotated.management_token
        );

        let mismatched_lease_replay = rotate_management_credential_transaction(
            &config_path,
            &identity,
            "installation-a",
            lease_generation + 1,
            1,
            "rotation-request",
            CREDENTIAL_ROTATION_REASON_LEAK,
            now + 2,
        )
        .expect_err("same request ID with different lease generation must conflict");
        assert_eq!(mismatched_lease_replay.status, StatusCode::CONFLICT);
        assert_eq!(
            management_token(&config_path).expect("token after rejected lease replay"),
            rotated.management_token
        );

        let replay = rotate_management_credential_transaction(
            &config_path,
            &identity,
            "installation-a",
            lease_generation,
            1,
            "rotation-request",
            CREDENTIAL_ROTATION_REASON_LEAK,
            now + 3,
        )
        .expect("replay credential rotation");
        assert!(!replay.rotated);
        assert_eq!(replay.management_token_generation, 2);

        assert_eq!(
            management_token(&config_path).expect("token after replay"),
            rotated.management_token
        );
    }

    #[test]
    fn credential_rotation_requires_matching_active_lease_and_generations() {
        let temp = tempdir().expect("tempdir");
        let config_path = temp.path().join("config.toml");
        let identity = DaemonIdentity {
            service: "threadrelay".to_string(),
            pid: 42,
            instance_id: "daemon-instance".to_string(),
            started_at_ms: 123,
        };
        ensure_management_token(&config_path).expect("create control file");
        let now = current_time_ms();
        update_lifecycle_lease(
            &config_path,
            &identity,
            "installation-a",
            LeaseOperation::Claim,
            now,
        )
        .expect("claim lease");
        let control = read_control_file(&config_path).expect("claimed control file");
        let lease_generation = control.lease.expect("claimed lease").generation;

        let wrong_lease = rotate_management_credential_transaction(
            &config_path,
            &identity,
            "installation-a",
            lease_generation + 1,
            control.management_token_generation,
            "wrong-lease",
            CREDENTIAL_ROTATION_REASON_LEAK,
            now + 1,
        )
        .expect_err("stale lease generation must conflict");
        assert_eq!(wrong_lease.status, StatusCode::CONFLICT);

        let wrong_token = rotate_management_credential_transaction(
            &config_path,
            &identity,
            "installation-a",
            lease_generation,
            control.management_token_generation + 1,
            "wrong-token",
            CREDENTIAL_ROTATION_REASON_LEAK,
            now + 1,
        )
        .expect_err("stale token generation must conflict");
        assert_eq!(wrong_token.status, StatusCode::CONFLICT);

        let other_identity = DaemonIdentity {
            instance_id: "other-daemon".to_string(),
            ..identity
        };
        let wrong_owner = rotate_management_credential_transaction(
            &config_path,
            &other_identity,
            "installation-a",
            lease_generation,
            control.management_token_generation,
            "wrong-owner",
            CREDENTIAL_ROTATION_REASON_LEAK,
            now + 1,
        )
        .expect_err("another daemon instance must not rotate the credential");
        assert_eq!(wrong_owner.status, StatusCode::CONFLICT);
        assert_eq!(
            read_control_file(&config_path)
                .expect("unchanged control file")
                .management_token_generation,
            control.management_token_generation
        );
    }

    #[test]
    fn trusted_takeover_replaces_lease_and_revokes_the_old_token() {
        let temp = tempdir().expect("tempdir");
        let config_path = temp.path().join("config.toml");
        let identity = DaemonIdentity {
            service: "threadrelay".to_string(),
            pid: 42,
            instance_id: "daemon-instance".to_string(),
            started_at_ms: 123,
        };
        ensure_management_token(&config_path).expect("create control file");
        let old_token = management_token(&config_path).expect("old token");
        let now = current_time_ms();
        update_lifecycle_lease(
            &config_path,
            &identity,
            "installation-a",
            LeaseOperation::Claim,
            now,
        )
        .expect("claim original lease");
        let original = read_control_file(&config_path).expect("original control file");
        let original_lease = original.lease.expect("original lease");

        let takeover = takeover_lifecycle_lease_transaction(
            &config_path,
            &identity,
            "installation-b",
            original_lease.generation,
            original.management_token_generation,
            "takeover-request",
            now + 1,
        )
        .expect("trusted takeover");
        assert!(takeover.rotated);
        let taken = read_control_file(&config_path).expect("taken control file");
        let taken_lease = taken.lease.as_ref().expect("taken lease");
        assert_eq!(taken_lease.installation_id, "installation-b");
        assert_eq!(taken_lease.daemon_instance_id, identity.instance_id);
        assert!(taken_lease.generation > original_lease.generation);
        assert_eq!(taken.management_token_generation, 2);
        assert_ne!(taken.management_token, old_token);

        let mut old_headers = HeaderMap::new();
        old_headers.insert(
            AUTHORIZATION,
            format!("Bearer {old_token}").parse().expect("old header"),
        );
        assert!(!authorize(&config_path, &old_headers).expect("old auth"));
        let mut new_headers = HeaderMap::new();
        new_headers.insert(
            AUTHORIZATION,
            format!("Bearer {}", taken.management_token)
                .parse()
                .expect("new header"),
        );
        assert!(authorize(&config_path, &new_headers).expect("new auth"));

        let replay = takeover_lifecycle_lease_transaction(
            &config_path,
            &identity,
            "installation-b",
            original_lease.generation,
            original.management_token_generation,
            "takeover-request",
            now + 2,
        )
        .expect("replay takeover");
        assert!(!replay.rotated);
        assert_eq!(replay.management_token_generation, 2);

        let mismatched_lease_replay = takeover_lifecycle_lease_transaction(
            &config_path,
            &identity,
            "installation-b",
            original_lease.generation + 1,
            original.management_token_generation,
            "takeover-request",
            now + 3,
        )
        .expect_err("same takeover request ID with different lease generation must conflict");
        assert_eq!(mismatched_lease_replay.status, StatusCode::CONFLICT);
    }

    #[test]
    fn concurrent_initialization_and_rotation_are_serialized() {
        let temp = tempdir().expect("tempdir");
        let config_path = temp.path().join("config.toml");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(32));
        let mut initializers = Vec::new();
        for _ in 0..32 {
            let config_path = config_path.clone();
            let barrier = barrier.clone();
            initializers.push(std::thread::spawn(move || {
                barrier.wait();
                management_token(&config_path).expect("initialize token")
            }));
        }
        let tokens: Vec<String> = initializers
            .into_iter()
            .map(|thread| thread.join().expect("initializer thread"))
            .collect();
        assert!(tokens.iter().all(|token| token == &tokens[0]));

        let identity = DaemonIdentity {
            service: "threadrelay".to_string(),
            pid: 42,
            instance_id: "daemon-instance".to_string(),
            started_at_ms: 123,
        };
        let now = current_time_ms();
        update_lifecycle_lease(
            &config_path,
            &identity,
            "installation-a",
            LeaseOperation::Claim,
            now,
        )
        .expect("claim lease");
        let control = read_control_file(&config_path).expect("claimed control file");
        let lease_generation = control.lease.expect("claimed lease").generation;
        let token_generation = control.management_token_generation;
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let mut rotations = Vec::new();
        for request_id in ["rotation-a", "rotation-b"] {
            let config_path = config_path.clone();
            let identity = identity.clone();
            let barrier = barrier.clone();
            rotations.push(std::thread::spawn(move || {
                barrier.wait();
                rotate_management_credential_transaction(
                    &config_path,
                    &identity,
                    "installation-a",
                    lease_generation,
                    token_generation,
                    request_id,
                    CREDENTIAL_ROTATION_REASON_LEAK,
                    now + 1,
                )
            }));
        }
        let results: Vec<_> = rotations
            .into_iter()
            .map(|thread| thread.join().expect("rotation thread"))
            .collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| result
                    .as_ref()
                    .is_err_and(|error| error.status == StatusCode::CONFLICT))
                .count(),
            1
        );
        assert_eq!(
            read_control_file(&config_path)
                .expect("rotated control file")
                .management_token_generation,
            token_generation + 1
        );
    }

    #[cfg(unix)]
    #[test]
    fn control_file_is_private_to_the_current_user() {
        let temp = tempdir().expect("tempdir");
        let config_path = temp.path().join("config.toml");
        ensure_management_token(&config_path).expect("create control file");
        let mode = fs::metadata(control_file_path(&config_path))
            .expect("control metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        let lock_mode = fs::metadata(control_lock_path(&config_path))
            .expect("control lock metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(lock_mode, 0o600);
    }

    #[test]
    fn active_daemon_locator_is_private_and_instance_guarded() {
        let temp = tempdir().expect("tempdir");
        let locator_path = temp.path().join(ACTIVE_DAEMON_FILE_NAME);
        let config_path = temp.path().join("custom-domain/config.toml");
        let identity = DaemonIdentity {
            service: "threadrelay".to_string(),
            pid: 42,
            instance_id: "active-instance".to_string(),
            started_at_ms: 123,
        };
        let guard = publish_active_daemon_locator_at(
            &locator_path,
            &config_path,
            &identity,
            "http://127.0.0.1:3847",
        )
        .expect("publish locator");
        let locator: ActiveDaemonLocator =
            serde_json::from_slice(&fs::read(&locator_path).expect("read locator"))
                .expect("decode locator");
        assert_eq!(
            locator,
            ActiveDaemonLocator {
                service: "threadrelay".to_string(),
                api_major: API_MAJOR,
                instance_id: "active-instance".to_string(),
                pid: 42,
                started_at_ms: 123,
                base_url: "http://127.0.0.1:3847".to_string(),
                control_file: control_file_path(&config_path)
                    .to_string_lossy()
                    .into_owned(),
            }
        );
        assert!(
            !String::from_utf8_lossy(&fs::read(&locator_path).unwrap()).contains("managementToken")
        );
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(&locator_path)
                .expect("locator metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );

        let replacement = ActiveDaemonLocator {
            instance_id: "replacement-instance".to_string(),
            ..locator
        };
        fs::write(
            &locator_path,
            serde_json::to_vec(&replacement).expect("encode replacement"),
        )
        .expect("write replacement");
        drop(guard);
        assert!(locator_path.exists(), "old guard removed newer locator");
    }
}
