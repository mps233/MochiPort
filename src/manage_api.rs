//! Versioned local management API primitives.
//!
//! The management API deliberately has its own bearer credential.  Existing
//! legacy routes remain untouched during the SwiftUI migration; new clients
//! use the versioned routes defined in `web.rs`.

use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
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
use serde_json::json;
use uuid::Uuid;

use crate::{app_state::SharedState, daemon_process::DaemonIdentity};

pub const API_MAJOR: u16 = 1;
const CONTROL_FILE_NAME: &str = "threadrelay-control.json";
const ACTIVE_DAEMON_FILE_NAME: &str = "threadrelay-active-daemon.json";

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
    pub api_major: u16,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleProtectedWorkItems {
    pub ai_gateway_requests: usize,
    pub codex_turns: usize,
    pub im_streams: usize,
    pub pending_approvals: usize,
    pub remote_control_requests: usize,
    pub total: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleManagementOwnership {
    /// Phase 2's read-only endpoint must not imply that this process owns a
    /// lifecycle lease before the lease protocol is implemented.
    pub state: &'static str,
    pub mode: &'static str,
    pub can_control: bool,
    pub installation_id: Option<String>,
    pub lease_generation: Option<u64>,
    pub lease_expires_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleResponse {
    pub service: LifecycleServiceIdentity,
    pub executable: String,
    pub config_path: String,
    pub bind: String,
    pub runtime: LifecycleRuntimeStatus,
    pub protected_work_items: LifecycleProtectedWorkItems,
    pub management: LifecycleManagementOwnership,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ControlFile {
    management_token: String,
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
    let path = control_file_path(config_path);
    let _ = load_or_create_management_token(&path)?;
    Ok(())
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
        .ok_or_else(|| AuthError::InvalidDiscoveryFile)?;
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
        if let Err(error) = fs::rename(&temporary, path) {
            if path.exists() {
                fs::remove_file(path).map_err(AuthError::Io)?;
                fs::rename(&temporary, path).map_err(AuthError::Io)?;
            } else {
                return Err(AuthError::Io(error));
            }
        }
        enforce_private_permissions(path).map_err(AuthError::Io)
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

/// Build a side-effect-free lifecycle snapshot for the authenticated
/// management API.  Control-plane ownership is deliberately reported as
/// unmanaged until the lease protocol lands; a read endpoint must never grant
/// or renew a lease as a hidden side effect.
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
        .saturating_add(im_streams)
        .saturating_add(pending_approvals)
        .saturating_add(remote_control_requests);
    let executable = std::env::current_exe()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();

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
        config_path,
        bind,
        runtime: LifecycleRuntimeStatus {
            state: "active",
            product_version: env!("CARGO_PKG_VERSION"),
            api_major: API_MAJOR,
        },
        protected_work_items: LifecycleProtectedWorkItems {
            ai_gateway_requests,
            codex_turns,
            im_streams,
            pending_approvals,
            remote_control_requests,
            total,
        },
        management: LifecycleManagementOwnership {
            state: "unmanaged",
            mode: "readOnly",
            can_control: false,
            installation_id: None,
            lease_generation: None,
            lease_expires_at_ms: None,
        },
    }
}

/// Middleware for all `/api/v1/manage/*` routes.
pub async fn require_bearer(
    State(state): State<SharedState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    match authorize(&state.config_path, request.headers()) {
        Ok(true) => next.run(request).await,
        Ok(false) => unauthorized_response(),
        // Do not include the path, token, or parser details in the response.
        // This keeps control-file contents out of HTTP logs and client errors.
        Err(_) => (
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
    let expected = load_or_create_management_token(&control_file_path(config_path))?;
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

fn load_or_create_management_token(path: &Path) -> Result<String, AuthError> {
    if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(AuthError::Io)?;
    }

    let token = Uuid::new_v4().simple().to_string();
    let contents = serde_json::to_vec(&ControlFile {
        management_token: token.clone(),
    })
    .map_err(|_| AuthError::InvalidControlFile)?;

    match open_control_file(path, true) {
        Ok(mut file) => {
            file.lock_exclusive().map_err(AuthError::Io)?;
            file.write_all(&contents).map_err(AuthError::Io)?;
            file.write_all(b"\n").map_err(AuthError::Io)?;
            file.sync_all().map_err(AuthError::Io)?;
            let _ = FileExt::unlock(&file);
            enforce_private_permissions(path).map_err(AuthError::Io)?;
            Ok(token)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            read_existing_token(path)
        }
        Err(error) => Err(AuthError::Io(error)),
    }
}

fn read_existing_token(path: &Path) -> Result<String, AuthError> {
    let mut file = open_control_file(path, false).map_err(AuthError::Io)?;
    file.lock_shared().map_err(AuthError::Io)?;
    let mut raw = String::new();
    let read_result = file.read_to_string(&mut raw);
    let _ = FileExt::unlock(&file);
    read_result.map_err(AuthError::Io)?;
    enforce_private_permissions(path).map_err(AuthError::Io)?;

    let control: ControlFile =
        serde_json::from_str(&raw).map_err(|_| AuthError::InvalidControlFile)?;
    let token = control.management_token.trim();
    if token.is_empty() || token != control.management_token || token.len() > 256 {
        return Err(AuthError::InvalidControlFile);
    }
    Ok(token.to_string())
}

fn open_control_file(path: &Path, create_new: bool) -> std::io::Result<std::fs::File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    if create_new {
        options.create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
    }
    options.open(path)
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
    fn management_auth_accepts_shared_token_and_rejects_missing_or_wrong_token() {
        let temp = tempdir().expect("tempdir");
        let config_path = temp.path().join("config.toml");
        ensure_management_token(&config_path).expect("create control file");
        let token_path = control_file_path(&config_path);
        let token = read_existing_token(&token_path).expect("read control file");

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
