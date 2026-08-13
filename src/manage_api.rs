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

use crate::app_state::SharedState;

pub const API_MAJOR: u16 = 1;
const CONTROL_FILE_NAME: &str = "threadrelay-control.json";

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

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ControlFile {
    management_token: String,
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("failed to prepare management control file")]
    Io(#[source] std::io::Error),
    #[error("management control file is invalid")]
    InvalidControlFile,
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
}
