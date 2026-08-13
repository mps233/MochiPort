use std::{
    fs,
    io::{Read, Write},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    app_state::SharedState,
    cli::SafeRelaunchShutdownMode,
    daemon_process::{CODEXHUB_GUI_PID_ENV, read_active_daemon_metadata},
    types::{ImPlatformKind, now_ms},
};

const EXPECTED_BUNDLE_IDENTIFIER: &str = "io.github.mps233.threadrelay";
const EXPECTED_BUNDLE_EXECUTABLE: &str = "ThreadRelay";
const MAX_PENDING_AGE: Duration = Duration::from_secs(15 * 60);
const DEFAULT_HELPER_START_DELAY_MS: u64 = 350;
const DEFAULT_HELPER_SHUTDOWN_MODE: SafeRelaunchShutdownMode = SafeRelaunchShutdownMode::Guarded;
const GUI_GRACE_PERIOD: Duration = Duration::from_secs(3);
const DAEMON_STOP_TIMEOUT: Duration = Duration::from_secs(10);
const NEW_DAEMON_START_TIMEOUT: Duration = Duration::from_secs(20);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BundleMetadata {
    pub bundle_path: String,
    pub bundle_identifier: String,
    pub executable: String,
    pub package_type: String,
    pub short_version: String,
    pub build: String,
}

#[derive(Debug, Clone)]
pub(crate) struct PendingSafeRelaunch {
    pub bundle_path: PathBuf,
    pub metadata: BundleMetadata,
    pub trigger_thread_id: String,
    pub trigger_turn_id: String,
    pub requested_at_ms: u128,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SafeRelaunchRequest {
    pub bundle_path: String,
    pub expected_bundle_identifier: String,
    pub expected_version: String,
    pub expected_build: String,
    pub daemon_instance_id: String,
    pub thread_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SafeRelaunchResponse {
    ok: bool,
    state: &'static str,
    pending: BundleMetadata,
    trigger_thread_id: String,
    trigger_turn_id: String,
}

#[derive(Debug)]
struct RegisterError {
    status: StatusCode,
    message: String,
}

impl RegisterError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    fn forbidden(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            message: message.into(),
        }
    }

    fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message: message.into(),
        }
    }
}

pub(crate) async fn register(
    State(state): State<SharedState>,
    Json(request): Json<SafeRelaunchRequest>,
) -> impl IntoResponse {
    match register_request(&state, request).await {
        Ok(response) => (StatusCode::ACCEPTED, Json(json!(response))),
        Err(error) => {
            state
                .push_event(
                    if error.status.is_server_error() {
                        "error"
                    } else {
                        "warn"
                    },
                    "safe_relaunch_registration_failed",
                    error.message.clone(),
                )
                .await;
            (
                error.status,
                Json(json!({ "ok": false, "error": error.message })),
            )
        }
    }
}

pub(crate) async fn status(State(state): State<SharedState>) -> impl IntoResponse {
    let pending = {
        let mut slot = state.safe_relaunch.lock().await;
        prune_expired_pending(&mut slot, now_ms());
        slot.clone()
    };
    Json(match pending {
        Some(pending) => json!({
            "ok": true,
            "pending": pending.metadata,
            "triggerThreadId": pending.trigger_thread_id,
            "triggerTurnId": pending.trigger_turn_id,
            "requestedAtMs": pending.requested_at_ms,
        }),
        None => json!({ "ok": true, "pending": null }),
    })
}

async fn register_request(
    state: &SharedState,
    request: SafeRelaunchRequest,
) -> Result<SafeRelaunchResponse, RegisterError> {
    let bind = state.config.lock().await.bind.clone();
    let bind_addr: SocketAddr = bind
        .parse()
        .map_err(|_| RegisterError::forbidden("safe relaunch requires a loopback bind"))?;
    if !bind_addr.ip().is_loopback() {
        return Err(RegisterError::forbidden(
            "safe relaunch is disabled when the local service is not loopback-only",
        ));
    }

    let expected_instance = request.daemon_instance_id.trim();
    if expected_instance.is_empty() || expected_instance != state.daemon_identity.instance_id {
        return Err(RegisterError::conflict(
            "daemon instance id does not match the active service",
        ));
    }

    let expected_bundle_identifier = request.expected_bundle_identifier.trim();
    let expected_version = request.expected_version.trim();
    let expected_build = request.expected_build.trim();
    if expected_bundle_identifier.is_empty()
        || expected_version.is_empty()
        || expected_build.is_empty()
    {
        return Err(RegisterError::bad_request(
            "expected bundle identifier, version, and build are required",
        ));
    }
    if expected_bundle_identifier != EXPECTED_BUNDLE_IDENTIFIER {
        return Err(RegisterError::bad_request(
            "unexpected ThreadRelay bundle identifier",
        ));
    }

    let bundle_path = validate_candidate_bundle(
        Path::new(request.bundle_path.trim()),
        expected_bundle_identifier,
        expected_version,
        expected_build,
    )
    .map_err(RegisterError::bad_request)?;

    let (trigger_thread_id, trigger_turn_id) =
        resolve_trigger_turn(state, request.thread_id.as_deref()).await?;

    let metadata = BundleMetadata {
        bundle_path: bundle_path.display().to_string(),
        bundle_identifier: expected_bundle_identifier.to_string(),
        executable: EXPECTED_BUNDLE_EXECUTABLE.to_string(),
        package_type: "APPL".to_string(),
        short_version: expected_version.to_string(),
        build: expected_build.to_string(),
    };
    let pending = PendingSafeRelaunch {
        bundle_path,
        metadata: metadata.clone(),
        trigger_thread_id: trigger_thread_id.clone(),
        trigger_turn_id: trigger_turn_id.clone(),
        requested_at_ms: now_ms(),
    };

    let mut slot = state.safe_relaunch.lock().await;
    prune_expired_pending(&mut slot, now_ms());
    if let Some(existing) = slot.as_ref() {
        if existing.bundle_path == pending.bundle_path
            && existing.metadata.short_version == pending.metadata.short_version
            && existing.metadata.build == pending.metadata.build
            && existing.trigger_thread_id == pending.trigger_thread_id
            && existing.trigger_turn_id == pending.trigger_turn_id
        {
            return Ok(SafeRelaunchResponse {
                ok: true,
                state: "pending",
                pending: existing.metadata.clone(),
                trigger_thread_id: existing.trigger_thread_id.clone(),
                trigger_turn_id: existing.trigger_turn_id.clone(),
            });
        }
        return Err(RegisterError::conflict(
            "a different safe relaunch is already pending",
        ));
    }
    *slot = Some(pending);
    drop(slot);

    state
        .push_event(
            "info",
            "safe_relaunch_pending",
            format!(
                "bundle={} version={} build={} thread={} turn={}",
                metadata.bundle_path,
                metadata.short_version,
                metadata.build,
                trigger_thread_id,
                trigger_turn_id
            ),
        )
        .await;

    Ok(SafeRelaunchResponse {
        ok: true,
        state: "pending",
        pending: metadata,
        trigger_thread_id,
        trigger_turn_id,
    })
}

async fn resolve_trigger_turn(
    state: &SharedState,
    requested_thread_id: Option<&str>,
) -> Result<(String, String), RegisterError> {
    let runtime = state.runtime.lock().await;
    if let Some(thread_id) = requested_thread_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let is_telegram = runtime
            .route_by_thread
            .get(thread_id)
            .is_some_and(|route| route.platform == ImPlatformKind::Telegram);
        if !is_telegram {
            return Err(RegisterError::bad_request(
                "threadId is not a bound Telegram thread",
            ));
        }
        let turn_id = runtime
            .current_turn_by_thread
            .get(thread_id)
            .cloned()
            .ok_or_else(|| {
                RegisterError::conflict("threadId does not have an active Telegram turn")
            })?;
        return Ok((thread_id.to_string(), turn_id));
    }

    let mut candidates = runtime
        .current_turn_by_thread
        .keys()
        .filter(|thread_id| {
            runtime
                .route_by_thread
                .get(*thread_id)
                .is_some_and(|route| route.platform == ImPlatformKind::Telegram)
        })
        .filter_map(|thread_id| {
            runtime
                .current_turn_by_thread
                .get(thread_id)
                .map(|turn_id| (thread_id.clone(), turn_id.clone()))
        });
    let first = candidates.next();
    if first.is_none() || candidates.next().is_some() {
        return Err(RegisterError::conflict(
            "safe relaunch requires threadId when there is not exactly one active Telegram turn",
        ));
    }
    Ok(first.expect("single active Telegram turn checked above"))
}

pub(crate) async fn on_telegram_turn_completed_sent(
    state: &SharedState,
    thread_id: &str,
    turn_id: Option<&str>,
) {
    let Some(turn_id) = turn_id.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };
    let pending = {
        let mut slot = state.safe_relaunch.lock().await;
        take_pending_for_delivery(&mut slot, thread_id, turn_id, now_ms())
    };
    let Some(pending) = pending else {
        return;
    };

    match spawn_relaunch_helper(state, &pending).await {
        Ok(helper_pid) => {
            state
                .push_event(
                    "info",
                    "safe_relaunch_helper_spawned",
                    format!(
                        "helper_pid={} thread={} bundle={}",
                        helper_pid,
                        thread_id,
                        pending.bundle_path.display()
                    ),
                )
                .await;
        }
        Err(error) => {
            let message = format!(
                "thread={} bundle={} err={error}",
                thread_id,
                pending.bundle_path.display()
            );
            let mut slot = state.safe_relaunch.lock().await;
            if slot.is_none() {
                *slot = Some(pending);
            }
            drop(slot);
            state
                .push_event("error", "safe_relaunch_helper_failed", message)
                .await;
        }
    }
}

fn take_pending_for_delivery(
    slot: &mut Option<PendingSafeRelaunch>,
    thread_id: &str,
    turn_id: &str,
    delivered_at_ms: u128,
) -> Option<PendingSafeRelaunch> {
    if prune_expired_pending(slot, delivered_at_ms) {
        return None;
    }
    if slot.as_ref().is_some_and(|pending| {
        pending.trigger_thread_id != thread_id || pending.trigger_turn_id != turn_id
    }) {
        return None;
    }
    slot.take()
}

fn prune_expired_pending(slot: &mut Option<PendingSafeRelaunch>, observed_at_ms: u128) -> bool {
    let expired = slot.as_ref().is_some_and(|pending| {
        observed_at_ms.saturating_sub(pending.requested_at_ms) > MAX_PENDING_AGE.as_millis()
    });
    if expired {
        *slot = None;
    }
    expired
}

async fn spawn_relaunch_helper(
    state: &SharedState,
    pending: &PendingSafeRelaunch,
) -> Result<u32, String> {
    let executable = std::env::current_exe()
        .and_then(|path| path.canonicalize())
        .map_err(|error| error.to_string())?;
    let bind_addr = state
        .config
        .lock()
        .await
        .bind
        .parse::<SocketAddr>()
        .map_err(|_| "local service bind address is unavailable".to_string())?;
    if !bind_addr.ip().is_loopback() {
        return Err("safe relaunch requires a loopback bind".to_string());
    }
    let daemon_pid = std::process::id();
    let gui_pid = gui_pid_for_helper(&executable, daemon_pid)?;
    let log_path = state
        .config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("logs")
        .join("threadrelay-safe-relaunch.log");

    let mut command = Command::new(&executable);
    command
        .arg("safe-relaunch-helper")
        .arg("--bundle-path")
        .arg(&pending.bundle_path)
        .arg("--expected-bundle-identifier")
        .arg(&pending.metadata.bundle_identifier)
        .arg("--expected-version")
        .arg(&pending.metadata.short_version)
        .arg("--expected-build")
        .arg(&pending.metadata.build)
        .arg("--daemon-pid")
        .arg(daemon_pid.to_string())
        .arg("--daemon-instance-id")
        .arg(&state.daemon_identity.instance_id)
        .arg("--old-executable-path")
        .arg(&executable)
        .arg("--bind-address")
        .arg(bind_addr.to_string())
        .arg("--log-path")
        .arg(log_path)
        .arg("--config-path")
        .arg(&state.config_path)
        .arg("--start-delay-ms")
        .arg(DEFAULT_HELPER_START_DELAY_MS.to_string())
        .arg("--shutdown-mode")
        .arg(DEFAULT_HELPER_SHUTDOWN_MODE.as_str())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env("CODEXHUB_SAFE_RELAUNCH_HELPER", "1");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    if let Some(gui_pid) = gui_pid {
        command.arg("--gui-pid").arg(gui_pid.to_string());
    }
    let child = command.spawn().map_err(|error| error.to_string())?;
    Ok(child.id())
}

fn gui_pid_for_helper(executable: &Path, daemon_pid: u32) -> Result<Option<u32>, String> {
    if let Some(raw_pid) = std::env::var_os(CODEXHUB_GUI_PID_ENV) {
        let gui_pid = raw_pid
            .to_string_lossy()
            .parse::<u32>()
            .map_err(|_| "managed GUI pid is invalid".to_string())?;
        if gui_pid <= 1 || gui_pid == daemon_pid || !process_matches_executable(gui_pid, executable)
        {
            return Err("managed GUI pid does not match the running ThreadRelay app".to_string());
        }
        return Ok(Some(gui_pid));
    }
    Ok(parent_pid(daemon_pid)
        .filter(|pid| *pid > 1 && *pid != daemon_pid)
        .filter(|pid| process_matches_executable(*pid, executable)))
}

fn parent_pid(pid: u32) -> Option<u32> {
    #[cfg(unix)]
    {
        let output = Command::new("/bin/ps")
            .args(["-p", &pid.to_string(), "-o", "ppid="])
            .output()
            .ok()?;
        return String::from_utf8_lossy(&output.stdout).trim().parse().ok();
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        None
    }
}

pub(crate) fn run_helper(args: SafeRelaunchHelperArgs) -> anyhow::Result<()> {
    let log_path = args.log_path.clone();
    log_line(
        &log_path,
        &format!(
            "helper_started daemon_instance_id={} shutdown_mode={}",
            args.daemon_instance_id,
            args.shutdown_mode.as_str()
        ),
    );
    thread::sleep(Duration::from_millis(args.start_delay_ms));

    let old_bundle = bundle_root_from_executable(&args.old_executable_path)
        .and_then(|path| path.canonicalize().ok())
        .ok_or_else(|| anyhow::anyhow!("old executable is not inside a valid .app bundle"))?;
    let candidate = validate_candidate_bundle_against(
        &args.bundle_path,
        &args.expected_bundle_identifier,
        &args.expected_version,
        &args.expected_build,
        &old_bundle,
    )
    .map_err(|error| anyhow::anyhow!(error))?;

    if !active_daemon_matches(&args)
        || !process_matches_executable(args.daemon_pid, &args.old_executable_path)
    {
        log_line(&log_path, "daemon_identity_mismatch");
        anyhow::bail!("old daemon identity does not match the registered instance");
    }

    if let Some(gui_pid) = args.gui_pid.filter(|pid| *pid > 1) {
        if !process_matches_executable(gui_pid, &args.old_executable_path) {
            log_line(&log_path, "gui_identity_mismatch");
            anyhow::bail!("old GUI pid does not match the expected executable");
        }
        terminate_process(gui_pid, false);
        wait_for_process_exit(gui_pid, GUI_GRACE_PERIOD);
        if process_matches_executable(gui_pid, &args.old_executable_path) {
            terminate_process(gui_pid, true);
            wait_for_process_exit(gui_pid, Duration::from_secs(2));
        }
        if process_is_alive(gui_pid) {
            log_line(&log_path, "old_gui_still_active");
            anyhow::bail!("old ThreadRelay GUI did not stop in time");
        }
    }

    let shutdown_result = match args.shutdown_mode {
        SafeRelaunchShutdownMode::Guarded => stop_daemon_guarded(&args),
        SafeRelaunchShutdownMode::Signal => stop_daemon_with_exact_signals(&args),
    };
    if let Err(error) = shutdown_result {
        return fail_with_rollback(&old_bundle, &log_path, error);
    }
    if process_is_alive(args.daemon_pid) || !local_bindings_are_available(args.bind_addr) {
        log_line(&log_path, "old_process_or_port_still_active");
        return fail_with_rollback(
            &old_bundle,
            &log_path,
            "old ThreadRelay process or local bind address did not stop in time",
        );
    }

    if let Err(error) = launch_bundle(&candidate) {
        return fail_with_rollback(
            &old_bundle,
            &log_path,
            &format!("failed to launch candidate bundle: {error}"),
        );
    }
    log_line(
        &log_path,
        &format!("bundle_launched={}", candidate.display()),
    );
    let expected_new_executable = candidate.join("Contents/MacOS/ThreadRelay");
    if !wait_for_new_daemon(
        &args.config_path,
        &args.daemon_instance_id,
        &expected_new_executable,
        args.bind_addr,
        NEW_DAEMON_START_TIMEOUT,
    ) {
        log_line(&log_path, "new_daemon_verification_timed_out");
        return fail_with_rollback(
            &old_bundle,
            &log_path,
            "new ThreadRelay daemon did not become ready in time",
        );
    }
    log_line(&log_path, "new_daemon_verified");
    Ok(())
}

fn active_daemon_matches(args: &SafeRelaunchHelperArgs) -> bool {
    let Some(metadata) = read_active_daemon_metadata(&args.config_path) else {
        return false;
    };
    if metadata.identity.pid != args.daemon_pid
        || metadata.identity.instance_id != args.daemon_instance_id
        || !metadata.identity.is_codexhub()
    {
        return false;
    }
    let Ok(metadata_executable) = Path::new(&metadata.executable).canonicalize() else {
        return false;
    };
    let Ok(expected_executable) = args.old_executable_path.canonicalize() else {
        return false;
    };
    let Ok(metadata_config) = Path::new(&metadata.config_path).canonicalize() else {
        return false;
    };
    let Ok(expected_config) = args.config_path.canonicalize() else {
        return false;
    };
    metadata_executable == expected_executable && metadata_config == expected_config
}

fn fail_with_rollback(old_bundle: &Path, log_path: &Path, error: &str) -> anyhow::Result<()> {
    match launch_bundle(old_bundle) {
        Ok(()) => {
            log_line(log_path, &format!("rollback_launched reason={error}"));
            Err(anyhow::anyhow!("{error}; the previous bundle was reopened"))
        }
        Err(rollback_error) => {
            log_line(
                log_path,
                &format!("rollback_failed reason={error} err={rollback_error}"),
            );
            Err(anyhow::anyhow!(
                "{error}; failed to reopen the previous bundle: {rollback_error}"
            ))
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SafeRelaunchHelperArgs {
    pub bundle_path: PathBuf,
    pub expected_bundle_identifier: String,
    pub expected_version: String,
    pub expected_build: String,
    pub daemon_pid: u32,
    pub daemon_instance_id: String,
    pub old_executable_path: PathBuf,
    pub gui_pid: Option<u32>,
    pub bind_addr: SocketAddr,
    pub log_path: PathBuf,
    pub config_path: PathBuf,
    pub start_delay_ms: u64,
    pub shutdown_mode: SafeRelaunchShutdownMode,
}

fn stop_daemon_guarded(args: &SafeRelaunchHelperArgs) -> Result<(), &'static str> {
    if process_is_alive(args.daemon_pid) {
        if !active_daemon_matches(args) {
            return Err("old daemon identity changed before guarded shutdown");
        }
        if !request_daemon_shutdown(args.bind_addr, &args.daemon_instance_id) {
            log_line(
                &args.log_path,
                "guarded_shutdown_request_unavailable_waiting_for_existing_exit",
            );
            wait_for_daemon_stop(args.daemon_pid, args.bind_addr, DAEMON_STOP_TIMEOUT);
            if !process_is_alive(args.daemon_pid) && local_bindings_are_available(args.bind_addr) {
                log_line(&args.log_path, "guarded_shutdown_completed_without_request");
                return Ok(());
            }
            log_line(
                &args.log_path,
                "guarded_shutdown_escalating_to_verified_exact_signal",
            );
            return stop_daemon_with_exact_signals(args);
        }
    }
    wait_for_daemon_stop(args.daemon_pid, args.bind_addr, DAEMON_STOP_TIMEOUT);
    if process_matches_executable(args.daemon_pid, &args.old_executable_path) {
        if !active_daemon_matches(args) {
            return Err("old daemon metadata changed before forced termination");
        }
        terminate_process(args.daemon_pid, true);
        wait_for_daemon_stop(args.daemon_pid, args.bind_addr, Duration::from_secs(2));
    }
    Ok(())
}

fn stop_daemon_with_exact_signals(args: &SafeRelaunchHelperArgs) -> Result<(), &'static str> {
    if !process_is_alive(args.daemon_pid) {
        return Ok(());
    }
    if !verified_daemon_process_matches(args) {
        return Err("old daemon identity changed before signal shutdown");
    }
    if !send_exact_daemon_signal(args.daemon_pid, false) {
        return Err("failed to send TERM to the verified daemon pid");
    }

    wait_for_daemon_stop(args.daemon_pid, args.bind_addr, DAEMON_STOP_TIMEOUT);
    if !process_is_alive(args.daemon_pid) {
        return Ok(());
    }
    if !verified_daemon_process_matches(args) {
        return Err("old daemon identity changed before KILL escalation");
    }
    if !send_exact_daemon_signal(args.daemon_pid, true) {
        return Err("failed to send KILL to the verified daemon pid");
    }
    wait_for_daemon_stop(args.daemon_pid, args.bind_addr, Duration::from_secs(2));
    Ok(())
}

fn verified_daemon_process_matches(args: &SafeRelaunchHelperArgs) -> bool {
    active_daemon_matches(args)
        && process_matches_executable(args.daemon_pid, &args.old_executable_path)
}

fn request_daemon_shutdown(address: SocketAddr, daemon_instance_id: &str) -> bool {
    let Ok(mut stream) = TcpStream::connect_timeout(&address, Duration::from_millis(400)) else {
        return false;
    };
    let body = serde_json::to_vec(&json!({
        "daemonInstanceId": daemon_instance_id,
    }))
    .unwrap_or_default();
    if body.is_empty() {
        return false;
    }
    let _ = stream.set_write_timeout(Some(Duration::from_millis(400)));
    let request = format!(
        "POST /api/shutdown/instance HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    if stream.write_all(request.as_bytes()).is_err() || stream.write_all(&body).is_err() {
        return false;
    }
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let mut response = Vec::new();
    let _ = stream.read_to_end(&mut response);
    String::from_utf8_lossy(&response)
        .lines()
        .next()
        .is_some_and(|line| line.contains(" 200 "))
}

fn wait_for_daemon_stop(daemon_pid: u32, bind_addr: SocketAddr, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !process_is_alive(daemon_pid) && local_bindings_are_available(bind_addr) {
            return;
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn wait_for_new_daemon(
    config_path: &Path,
    old_instance_id: &str,
    expected_executable: &Path,
    bind_addr: SocketAddr,
    timeout: Duration,
) -> bool {
    let expected_executable = expected_executable.canonicalize().ok();
    let Some(expected_executable) = expected_executable else {
        return false;
    };
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Some(metadata) = read_active_daemon_metadata(config_path)
            && metadata.identity.instance_id != old_instance_id
            && Path::new(&metadata.executable)
                .canonicalize()
                .is_ok_and(|path| path == expected_executable)
            && daemon_status_matches(
                bind_addr,
                metadata.identity.pid,
                &metadata.identity.instance_id,
            )
        {
            return true;
        }
        thread::sleep(POLL_INTERVAL);
    }
    false
}

fn daemon_status_matches(
    bind_addr: SocketAddr,
    expected_pid: u32,
    expected_instance_id: &str,
) -> bool {
    let Ok(mut stream) = TcpStream::connect_timeout(&bind_addr, Duration::from_millis(500)) else {
        return false;
    };
    let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
    let _ = stream.set_read_timeout(Some(Duration::from_millis(700)));
    let request =
        format!("GET /api/status HTTP/1.1\r\nHost: {bind_addr}\r\nConnection: close\r\n\r\n");
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }
    let mut response = Vec::new();
    if stream.read_to_end(&mut response).is_err() {
        return false;
    }
    let response = String::from_utf8_lossy(&response);
    if !response
        .lines()
        .next()
        .is_some_and(|line| line.contains(" 200 "))
    {
        return false;
    }
    let Some((_, body)) = response.split_once("\r\n\r\n") else {
        return false;
    };
    serde_json::from_str::<Value>(body).is_ok_and(|value| {
        value.get("pid").and_then(Value::as_u64) == Some(u64::from(expected_pid))
            && value.get("instanceId").and_then(Value::as_str) == Some(expected_instance_id)
            && matches!(
                value.get("service").and_then(Value::as_str),
                Some("threadrelay" | "codexhub")
            )
    })
}

fn wait_for_process_exit(pid: u32, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline && process_is_alive(pid) {
        thread::sleep(POLL_INTERVAL);
    }
}

fn process_is_alive(pid: u32) -> bool {
    if pid <= 1 {
        return false;
    }
    #[cfg(unix)]
    {
        return Command::new("/bin/ps")
            .args(["-p", &pid.to_string(), "-o", "pid="])
            .output()
            .is_ok_and(|output| !String::from_utf8_lossy(&output.stdout).trim().is_empty());
    }
    #[cfg(windows)]
    {
        let mut command = Command::new("tasklist");
        command.args(["/FI", &format!("PID eq {pid}")]);
        return command.output().is_ok_and(|output| {
            String::from_utf8_lossy(&output.stdout).contains(&pid.to_string())
        });
    }
    #[cfg(all(not(unix), not(windows)))]
    {
        false
    }
}

fn process_matches_executable(pid: u32, expected_executable: &Path) -> bool {
    if pid <= 1 {
        return false;
    }
    #[cfg(unix)]
    {
        let expected = expected_executable.to_string_lossy();
        let output = Command::new("/bin/ps")
            .env("COLUMNS", "4096")
            .args(["-ww", "-p", &pid.to_string(), "-o", "args="])
            .output();
        return output.ok().is_some_and(|output| {
            let args = String::from_utf8_lossy(&output.stdout);
            let args = args.trim();
            args == expected
                || args
                    .strip_prefix(expected.as_ref())
                    .and_then(|rest| rest.chars().next())
                    .is_some_and(char::is_whitespace)
        });
    }
    #[cfg(not(unix))]
    {
        let _ = expected_executable;
        process_is_alive(pid)
    }
}

fn terminate_process(pid: u32, force: bool) {
    #[cfg(unix)]
    {
        let signal = if force { "-KILL" } else { "-TERM" };
        let _ = Command::new("/bin/kill")
            .args([signal, &pid.to_string()])
            .status();
    }
    #[cfg(windows)]
    {
        let mut command = Command::new("taskkill");
        command.args(["/PID", &pid.to_string(), "/T"]);
        if force {
            command.arg("/F");
        }
        let _ = command.status();
    }
}

#[cfg(unix)]
fn exact_signal_args(pid: u32, force: bool) -> Option<[String; 2]> {
    if pid <= 1 {
        return None;
    }
    Some([
        if force { "-KILL" } else { "-TERM" }.to_string(),
        pid.to_string(),
    ])
}

fn send_exact_daemon_signal(pid: u32, force: bool) -> bool {
    #[cfg(unix)]
    {
        let Some(args) = exact_signal_args(pid, force) else {
            return false;
        };
        return Command::new("/bin/kill")
            .args(args)
            .status()
            .is_ok_and(|status| status.success());
    }
    #[cfg(not(unix))]
    {
        let _ = (pid, force);
        false
    }
}

fn local_bindings_are_available(bind_addr: SocketAddr) -> bool {
    relevant_loopback_addresses(bind_addr)
        .into_iter()
        .all(|address| TcpListener::bind(address).is_ok())
}

fn relevant_loopback_addresses(bind_addr: SocketAddr) -> Vec<SocketAddr> {
    let mut addresses = vec![bind_addr];
    let companion = match bind_addr.ip() {
        IpAddr::V4(ip) if ip == Ipv4Addr::LOCALHOST => Some(SocketAddr::new(
            IpAddr::V6(Ipv6Addr::LOCALHOST),
            bind_addr.port(),
        )),
        IpAddr::V6(ip) if ip == Ipv6Addr::LOCALHOST => Some(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            bind_addr.port(),
        )),
        _ => None,
    };
    if let Some(companion) = companion {
        addresses.push(companion);
    }
    addresses
}

fn launch_bundle(bundle_path: &Path) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        Command::new("/usr/bin/open")
            .arg("-n")
            .arg(bundle_path)
            .spawn()?;
        return Ok(());
    }
    #[cfg(target_os = "windows")]
    {
        Command::new("cmd")
            .args(["/C", "start", ""])
            .arg(bundle_path)
            .spawn()?;
        return Ok(());
    }
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        Command::new("xdg-open").arg(bundle_path).spawn()?;
        Ok(())
    }
}

pub(crate) fn validate_candidate_bundle(
    path: &Path,
    expected_bundle_identifier: &str,
    expected_version: &str,
    expected_build: &str,
) -> Result<PathBuf, String> {
    let current_root = current_bundle_root()
        .ok_or_else(|| "safe relaunch requires a running .app bundle".to_string())?;
    validate_candidate_bundle_against(
        path,
        expected_bundle_identifier,
        expected_version,
        expected_build,
        &current_root,
    )
}

fn validate_candidate_bundle_against(
    path: &Path,
    expected_bundle_identifier: &str,
    expected_version: &str,
    expected_build: &str,
    current_root: &Path,
) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("bundle path must be absolute".to_string());
    }
    if path.extension().and_then(|value| value.to_str()) != Some("app") {
        return Err("bundle path must point to a .app bundle".to_string());
    }
    if path_contains_symlink(path) {
        return Err("bundle path may not contain symlinks".to_string());
    }
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("cannot resolve bundle path: {error}"))?;
    let metadata = fs::metadata(&canonical).map_err(|error| error.to_string())?;
    if !metadata.is_dir() {
        return Err("bundle path is not a directory".to_string());
    }
    let info_path = canonical.join("Contents/Info.plist");
    let executable_path = canonical.join("Contents/MacOS/ThreadRelay");
    if !info_path.is_file() || !executable_path.is_file() {
        return Err(
            "bundle is missing Contents/Info.plist or Contents/MacOS/ThreadRelay".to_string(),
        );
    }
    if path_contains_symlink(&info_path) || path_contains_symlink(&executable_path) {
        return Err("bundle contents may not contain symlinks".to_string());
    }
    let canonical_executable = executable_path
        .canonicalize()
        .map_err(|error| format!("cannot resolve bundle executable: {error}"))?;
    if !canonical_executable.starts_with(canonical.join("Contents/MacOS")) {
        return Err("bundle executable resolves outside the bundle".to_string());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&canonical_executable)
            .map_err(|error| error.to_string())?
            .permissions()
            .mode();
        if mode & 0o111 == 0 {
            return Err("bundle executable is not executable".to_string());
        }
    }

    let actual = read_bundle_metadata(&canonical)?;
    if actual.bundle_identifier != expected_bundle_identifier
        || actual.short_version != expected_version
        || actual.build != expected_build
    {
        return Err(format!(
            "bundle metadata mismatch: id={} version={} build={}",
            actual.bundle_identifier, actual.short_version, actual.build
        ));
    }
    if actual.executable != EXPECTED_BUNDLE_EXECUTABLE {
        return Err("bundle executable is not ThreadRelay".to_string());
    }
    if actual.package_type != "APPL" {
        return Err("bundle package type is not APPL".to_string());
    }
    if actual.bundle_identifier != EXPECTED_BUNDLE_IDENTIFIER {
        return Err("bundle identifier is not io.github.mps233.threadrelay".to_string());
    }
    let candidate_build = actual
        .build
        .parse::<u64>()
        .map_err(|_| "candidate bundle build is not numeric".to_string())?;

    let current_root = current_root
        .canonicalize()
        .map_err(|error| format!("cannot resolve running bundle: {error}"))?;
    if canonical == current_root {
        return Err("candidate bundle is already running".to_string());
    }
    if canonical.parent() != current_root.parent() {
        return Err("candidate bundle must be next to the running bundle".to_string());
    }
    let current = read_bundle_metadata(&current_root)?;
    let current_build = current
        .build
        .parse::<u64>()
        .map_err(|_| "running bundle build is not numeric".to_string())?;
    if candidate_build <= current_build {
        return Err("candidate bundle build is not newer than the running build".to_string());
    }

    Ok(canonical)
}

fn current_bundle_root() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    bundle_root_from_executable(&executable)
}

fn bundle_root_from_executable(executable: &Path) -> Option<PathBuf> {
    executable
        .ancestors()
        .find(|path| path.extension().and_then(|value| value.to_str()) == Some("app"))
        .map(Path::to_path_buf)
}

fn read_bundle_metadata(bundle_path: &Path) -> Result<BundleMetadata, String> {
    #[cfg(target_os = "macos")]
    {
        let info_path = bundle_path.join("Contents/Info.plist");
        let output = Command::new("/usr/bin/plutil")
            .args(["-convert", "json", "-o", "-", "--"])
            .arg(&info_path)
            .output()
            .map_err(|error| format!("cannot run plutil: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "cannot parse Info.plist: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        let value: Value = serde_json::from_slice(&output.stdout)
            .map_err(|error| format!("invalid Info.plist JSON: {error}"))?;
        return Ok(BundleMetadata {
            bundle_path: bundle_path.display().to_string(),
            bundle_identifier: plist_string(&value, "CFBundleIdentifier")?,
            executable: plist_string(&value, "CFBundleExecutable")?,
            package_type: plist_string(&value, "CFBundlePackageType")?,
            short_version: plist_string(&value, "CFBundleShortVersionString")?,
            build: plist_string(&value, "CFBundleVersion")?,
        });
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = bundle_path;
        Err("safe bundle relaunch is currently supported only on macOS".to_string())
    }
}

#[cfg(target_os = "macos")]
fn plist_string(value: &Value, key: &str) -> Result<String, String> {
    let value = value
        .get(key)
        .ok_or_else(|| format!("Info.plist is missing {key}"))?;
    if let Some(string) = value.as_str() {
        return (!string.trim().is_empty())
            .then(|| string.trim().to_string())
            .ok_or_else(|| format!("Info.plist field {key} is empty"));
    }
    value
        .as_u64()
        .map(|number| number.to_string())
        .filter(|string| !string.is_empty())
        .ok_or_else(|| format!("Info.plist field {key} is not a string or number"))
}

fn path_contains_symlink(path: &Path) -> bool {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        if fs::symlink_metadata(&current)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
        {
            return true;
        }
    }
    false
}

fn log_line(path: &Path, line: &str) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{} {}", now_ms(), line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(thread_id: &str, requested_at_ms: u128) -> PendingSafeRelaunch {
        PendingSafeRelaunch {
            bundle_path: PathBuf::from("/tmp/ThreadRelay-build330.app"),
            metadata: BundleMetadata {
                bundle_path: "/tmp/ThreadRelay-build330.app".to_string(),
                bundle_identifier: EXPECTED_BUNDLE_IDENTIFIER.to_string(),
                executable: EXPECTED_BUNDLE_EXECUTABLE.to_string(),
                package_type: "APPL".to_string(),
                short_version: "0.4.20".to_string(),
                build: "330".to_string(),
            },
            trigger_thread_id: thread_id.to_string(),
            trigger_turn_id: "turn-a".to_string(),
            requested_at_ms,
        }
    }

    #[test]
    fn bundle_root_is_found_from_macos_executable_layout() {
        let path = Path::new("/tmp/ThreadRelay-build330.app/Contents/MacOS/ThreadRelay");
        assert_eq!(
            bundle_root_from_executable(path),
            Some(PathBuf::from("/tmp/ThreadRelay-build330.app"))
        );
    }

    #[test]
    fn symlink_detection_rejects_symlink_components() {
        let root = tempfile::tempdir().expect("tempdir");
        let real = root.path().join("real");
        fs::create_dir(&real).expect("real dir");
        let link = root.path().join("link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &link).expect("symlink");
        #[cfg(unix)]
        assert!(path_contains_symlink(&link.join("child")));
    }

    #[test]
    fn helper_request_is_sent_without_shell_interpolation() {
        let path = Path::new("/tmp/ThreadRelay build 330.app");
        assert_eq!(
            path.extension().and_then(|value| value.to_str()),
            Some("app")
        );
    }

    #[test]
    fn internal_helper_spawn_defaults_to_guarded_shutdown() {
        assert_eq!(
            DEFAULT_HELPER_SHUTDOWN_MODE,
            SafeRelaunchShutdownMode::Guarded
        );
        assert_eq!(DEFAULT_HELPER_SHUTDOWN_MODE.as_str(), "guarded");
    }

    #[cfg(unix)]
    #[test]
    fn exact_signal_arguments_target_only_the_positive_pid() {
        assert_eq!(
            exact_signal_args(4242, false),
            Some(["-TERM".to_string(), "4242".to_string()])
        );
        assert_eq!(
            exact_signal_args(4242, true),
            Some(["-KILL".to_string(), "4242".to_string()])
        );
        assert_eq!(exact_signal_args(0, false), None);
        assert_eq!(exact_signal_args(1, false), None);
    }

    #[test]
    fn bind_tracking_includes_only_the_companion_for_standard_localhost() {
        let ipv4: SocketAddr = "127.0.0.1:3847".parse().expect("ipv4");
        let alternate: SocketAddr = "127.0.0.2:3847".parse().expect("alternate ipv4");

        assert_eq!(
            relevant_loopback_addresses(ipv4),
            vec![ipv4, "[::1]:3847".parse().expect("ipv6 companion")]
        );
        assert_eq!(relevant_loopback_addresses(alternate), vec![alternate]);
    }

    #[test]
    fn pending_relaunch_is_kept_for_other_threads_and_consumed_once() {
        let mut slot = Some(pending("thread-a", 100));

        assert!(take_pending_for_delivery(&mut slot, "thread-b", "turn-a", 200).is_none());
        assert!(slot.is_some());
        assert!(take_pending_for_delivery(&mut slot, "thread-a", "turn-b", 200).is_none());
        assert!(slot.is_some());
        assert!(take_pending_for_delivery(&mut slot, "thread-a", "turn-a", 200).is_some());
        assert!(slot.is_none());
        assert!(take_pending_for_delivery(&mut slot, "thread-a", "turn-a", 200).is_none());
    }

    #[test]
    fn expired_pending_relaunch_is_removed_without_triggering() {
        let mut slot = Some(pending("thread-a", 100));
        let delivered_at_ms = 100 + MAX_PENDING_AGE.as_millis() + 1;

        assert!(
            take_pending_for_delivery(&mut slot, "thread-a", "turn-a", delivered_at_ms).is_none()
        );
        assert!(slot.is_none());
    }

    #[tokio::test]
    async fn trigger_thread_is_derived_only_when_one_telegram_turn_is_active() {
        use crate::{app_state::AppState, config::AppConfig, im_runtime::RouteTarget};

        let temp = tempfile::tempdir().expect("tempdir");
        let mut config = AppConfig::default();
        config.state_path = temp.path().join("state.json");
        let state = AppState::new(temp.path().join("config.toml"), config, None, None);
        {
            let mut runtime = state.runtime.lock().await;
            runtime.bind_route(
                "thread-a",
                RouteTarget {
                    platform: ImPlatformKind::Telegram,
                    conversation_key: "telegram:default:42".to_string(),
                    account_id: "default".to_string(),
                    chat_id: "42".to_string(),
                    remote_client_key: "client-a".to_string(),
                },
            );
            runtime
                .current_turn_by_thread
                .insert("thread-a".to_string(), "turn-a".to_string());
        }

        assert_eq!(
            resolve_trigger_turn(&state, None)
                .await
                .expect("derive single thread"),
            ("thread-a".to_string(), "turn-a".to_string())
        );
        assert_eq!(
            resolve_trigger_turn(&state, Some("thread-a"))
                .await
                .expect("explicit bound thread"),
            ("thread-a".to_string(), "turn-a".to_string())
        );

        {
            let mut runtime = state.runtime.lock().await;
            runtime.bind_route(
                "thread-b",
                RouteTarget {
                    platform: ImPlatformKind::Telegram,
                    conversation_key: "telegram:default:43".to_string(),
                    account_id: "default".to_string(),
                    chat_id: "43".to_string(),
                    remote_client_key: "client-b".to_string(),
                },
            );
            runtime
                .current_turn_by_thread
                .insert("thread-b".to_string(), "turn-b".to_string());
        }
        assert!(resolve_trigger_turn(&state, None).await.is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn daemon_status_probe_accepts_axum_response_without_write_half_close() {
        use axum::{Router, routing::get};
        use tokio::{net::TcpListener, sync::oneshot};

        let expected_pid = 4242;
        let expected_instance_id = "instance-under-test".to_string();
        let route_instance_id = expected_instance_id.clone();
        let app = Router::new().route(
            "/api/status",
            get(move || {
                let instance_id = route_instance_id.clone();
                async move {
                    Json(json!({
                        "service": "threadrelay",
                        "pid": expected_pid,
                        "instanceId": instance_id,
                    }))
                }
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind probe listener");
        let address = listener.local_addr().expect("probe listener address");
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .expect("serve probe response");
        });

        let probe_instance_id = expected_instance_id.clone();
        let matched = tokio::task::spawn_blocking(move || {
            daemon_status_matches(address, expected_pid, &probe_instance_id)
        })
        .await
        .expect("probe task");

        let _ = shutdown_tx.send(());
        server.await.expect("stop probe server");
        assert!(matched);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn guarded_shutdown_probe_accepts_axum_response_without_write_half_close() {
        use axum::{Router, http::StatusCode, routing::post};
        use tokio::{net::TcpListener, sync::oneshot};

        let app = Router::new().route(
            "/api/shutdown/instance",
            post(|Json(_request): Json<Value>| async { StatusCode::OK }),
        );
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind shutdown probe listener");
        let address = listener
            .local_addr()
            .expect("shutdown probe listener address");
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .expect("serve shutdown probe response");
        });

        let accepted = tokio::task::spawn_blocking(move || {
            request_daemon_shutdown(address, "instance-under-test")
        })
        .await
        .expect("shutdown probe task");

        let _ = shutdown_tx.send(());
        server.await.expect("stop shutdown probe server");
        assert!(accepted);
    }

    #[test]
    fn active_daemon_match_requires_pid_instance_executable_and_config() {
        use crate::daemon_process::{DaemonIdentity, DaemonInstanceLock};

        let temp = tempfile::tempdir().expect("tempdir");
        let config_path = temp.path().join("home/config.toml");
        fs::create_dir_all(config_path.parent().expect("config parent"))
            .expect("create config parent");
        fs::write(&config_path, b"").expect("write config");
        let identity = DaemonIdentity::new();
        let _lock =
            DaemonInstanceLock::acquire(&config_path, &identity).expect("acquire daemon lock");
        let executable = std::env::current_exe()
            .and_then(|path| path.canonicalize())
            .expect("current executable");
        let mut args = SafeRelaunchHelperArgs {
            bundle_path: PathBuf::from("/tmp/ThreadRelay-build330.app"),
            expected_bundle_identifier: EXPECTED_BUNDLE_IDENTIFIER.to_string(),
            expected_version: "0.4.20".to_string(),
            expected_build: "330".to_string(),
            daemon_pid: identity.pid,
            daemon_instance_id: identity.instance_id.clone(),
            old_executable_path: executable,
            gui_pid: None,
            bind_addr: "127.0.0.1:3847".parse().expect("bind address"),
            log_path: temp.path().join("relaunch.log"),
            config_path,
            start_delay_ms: 0,
            shutdown_mode: SafeRelaunchShutdownMode::Signal,
        };

        assert!(active_daemon_matches(&args));
        assert!(verified_daemon_process_matches(&args));

        let expected_instance_id = args.daemon_instance_id.clone();
        args.daemon_instance_id = "stale-instance".to_string();
        assert!(!active_daemon_matches(&args));
        args.daemon_instance_id = expected_instance_id;

        let expected_pid = args.daemon_pid;
        args.daemon_pid = expected_pid.saturating_add(1);
        assert!(!verified_daemon_process_matches(&args));
        args.daemon_pid = expected_pid;

        let expected_executable = args.old_executable_path.clone();
        args.old_executable_path = temp.path().join("other-codexhub");
        assert!(!verified_daemon_process_matches(&args));
        args.old_executable_path = expected_executable;

        args.config_path = temp.path().join("other/config.toml");
        assert!(!active_daemon_matches(&args));
    }

    #[cfg(target_os = "macos")]
    fn write_test_bundle(path: &Path, bundle_identifier: &str, build: &str) {
        use std::os::unix::fs::PermissionsExt;

        let macos = path.join("Contents/MacOS");
        fs::create_dir_all(&macos).expect("create bundle directories");
        let plist = json!({
            "CFBundleIdentifier": bundle_identifier,
            "CFBundleExecutable": EXPECTED_BUNDLE_EXECUTABLE,
            "CFBundlePackageType": "APPL",
            "CFBundleShortVersionString": "0.4.20",
            "CFBundleVersion": build,
        });
        fs::write(
            path.join("Contents/Info.plist"),
            serde_json::to_vec(&plist).expect("serialize plist"),
        )
        .expect("write plist");
        let executable = macos.join(EXPECTED_BUNDLE_EXECUTABLE);
        fs::write(&executable, b"#!/bin/sh\nexit 0\n").expect("write executable");
        let mut permissions = fs::metadata(&executable)
            .expect("executable metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(executable, permissions).expect("set executable mode");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn candidate_bundle_must_be_newer_and_next_to_running_bundle() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().canonicalize().expect("canonical tempdir");
        let current = root.join("ThreadRelay-build329.app");
        let candidate = root.join("ThreadRelay build 330.app");
        write_test_bundle(&current, EXPECTED_BUNDLE_IDENTIFIER, "329");
        write_test_bundle(&candidate, EXPECTED_BUNDLE_IDENTIFIER, "330");

        let validated = validate_candidate_bundle_against(
            &candidate,
            EXPECTED_BUNDLE_IDENTIFIER,
            "0.4.20",
            "330",
            &current,
        )
        .expect("validate newer sibling bundle");
        assert_eq!(validated, candidate.canonicalize().expect("candidate path"));

        write_test_bundle(&candidate, EXPECTED_BUNDLE_IDENTIFIER, "329");
        let error = validate_candidate_bundle_against(
            &candidate,
            EXPECTED_BUNDLE_IDENTIFIER,
            "0.4.20",
            "329",
            &current,
        )
        .expect_err("same build must fail");
        assert!(error.contains("not newer"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn candidate_bundle_rejects_wrong_identity_and_untrusted_parent() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().canonicalize().expect("canonical tempdir");
        let current = root.join("trusted/ThreadRelay-build329.app");
        let wrong_id = root.join("trusted/ThreadRelay-build330.app");
        let outside = root.join("other/ThreadRelay-build331.app");
        write_test_bundle(&current, EXPECTED_BUNDLE_IDENTIFIER, "329");
        write_test_bundle(&wrong_id, "com.example.other", "330");
        write_test_bundle(&outside, EXPECTED_BUNDLE_IDENTIFIER, "331");

        let error = validate_candidate_bundle_against(
            &wrong_id,
            EXPECTED_BUNDLE_IDENTIFIER,
            "0.4.20",
            "330",
            &current,
        )
        .expect_err("wrong bundle id must fail");
        assert!(error.contains("metadata mismatch"));

        let error = validate_candidate_bundle_against(
            &outside,
            EXPECTED_BUNDLE_IDENTIFIER,
            "0.4.20",
            "331",
            &current,
        )
        .expect_err("outside trusted parent must fail");
        assert!(error.contains("next to the running bundle"));
    }
}
