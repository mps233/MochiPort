use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU8, AtomicUsize, Ordering},
    },
};

use tokio::{
    sync::{Mutex, Notify, broadcast, oneshot},
    task::JoinHandle,
};

use serde::Serialize;
use serde_json::Value;

use crate::{
    ai_gateway::request_log::RequestLogStore,
    ai_gateway::routing_state::GatewayRoutingState,
    chain_log,
    codex::CodexNotification,
    config::AppConfig,
    daemon_process::DaemonIdentity,
    im_runtime::{RouteTarget, RuntimeState, route_from_conversation_key},
    store::PersistedState,
    types::{EventRecord, ImPlatformKind, now_ms},
};

pub type SharedState = Arc<AppState>;

/// Admission state for work that must not be cut off during a daemon restart.
///
/// This is deliberately process-local. The management lease still fences the
/// cross-process restart request; the gate closes the race between the final
/// protected-work check and the shutdown signal inside this process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum LifecycleAdmissionState {
    Active = 0,
    Draining = 1,
    ShutdownCommitted = 2,
}

impl LifecycleAdmissionState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Draining => "draining",
            Self::ShutdownCommitted => "shutdownCommitted",
        }
    }

    fn from_raw(raw: u8) -> Self {
        match raw {
            1 => Self::Draining,
            2 => Self::ShutdownCommitted,
            _ => Self::Active,
        }
    }
}

/// Coordinates admission of new protected work with a lifecycle drain.
pub struct LifecycleAdmission {
    state: AtomicU8,
    candidate_hold: AtomicU8,
    active_permits: AtomicUsize,
    drained: Notify,
}

impl LifecycleAdmission {
    pub fn new() -> Self {
        Self {
            state: AtomicU8::new(LifecycleAdmissionState::Active as u8),
            candidate_hold: AtomicU8::new(0),
            active_permits: AtomicUsize::new(0),
            drained: Notify::new(),
        }
    }

    pub fn state(&self) -> LifecycleAdmissionState {
        LifecycleAdmissionState::from_raw(self.state.load(Ordering::Acquire))
    }

    /// Atomically admit work only while the daemon is active.
    pub fn try_admit(self: &Arc<Self>) -> Option<LifecycleAdmissionPermit> {
        if self.state() != LifecycleAdmissionState::Active {
            return None;
        }
        self.active_permits.fetch_add(1, Ordering::AcqRel);
        // A drain may begin between the first state read and the increment.
        // Re-check after increment; the permit keeps the drain waiting until
        // this failed admission has been removed.
        if self.state() != LifecycleAdmissionState::Active {
            self.release_permit();
            return None;
        }
        Some(LifecycleAdmissionPermit {
            admission: Arc::clone(self),
        })
    }

    pub fn begin_draining(&self) -> bool {
        self.state
            .compare_exchange(
                LifecycleAdmissionState::Active as u8,
                LifecycleAdmissionState::Draining as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    /// Start a newly launched runtime in a hold state.  The daemon remains
    /// reachable for its management API, but no bridge work may be admitted
    /// until the controlling GUI commits the runtime switch.
    pub fn begin_candidate_hold(&self) -> bool {
        if self
            .state
            .compare_exchange(
                LifecycleAdmissionState::Active as u8,
                LifecycleAdmissionState::Draining as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            return false;
        }
        self.candidate_hold.store(1, Ordering::Release);
        true
    }

    pub fn is_candidate_hold(&self) -> bool {
        self.candidate_hold.load(Ordering::Acquire) != 0
            && self.state() == LifecycleAdmissionState::Draining
    }

    /// Consume a candidate hold when the daemon is asked to shut down before
    /// the switch is committed.  The state stays draining so the normal
    /// shutdown path can finish without admitting new work.
    pub fn take_candidate_hold_for_shutdown(&self) -> bool {
        self.candidate_hold.swap(0, Ordering::AcqRel) != 0
            && self.state() == LifecycleAdmissionState::Draining
    }

    /// Restore a candidate hold after a guarded shutdown request is rejected.
    pub fn restore_candidate_hold(&self) -> bool {
        if self.state() != LifecycleAdmissionState::Draining {
            return false;
        }
        self.candidate_hold.store(1, Ordering::Release);
        true
    }

    /// Release a candidate hold and allow normal bridge admission.
    pub fn commit_candidate_hold(&self) -> bool {
        if self.candidate_hold.load(Ordering::Acquire) == 0 {
            return false;
        }
        if self
            .state
            .compare_exchange(
                LifecycleAdmissionState::Draining as u8,
                LifecycleAdmissionState::Active as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            return false;
        }
        self.candidate_hold.store(0, Ordering::Release);
        true
    }

    pub async fn wait_for_drain(&self) {
        loop {
            let notified = self.drained.notified();
            if self.active_permits.load(Ordering::Acquire) == 0 {
                return;
            }
            notified.await;
        }
    }

    pub fn commit_shutdown(&self) -> bool {
        let committed = self
            .state
            .compare_exchange(
                LifecycleAdmissionState::Draining as u8,
                LifecycleAdmissionState::ShutdownCommitted as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok();
        if committed {
            self.candidate_hold.store(0, Ordering::Release);
        }
        committed
    }

    pub fn cancel_draining(&self) -> bool {
        if self.is_candidate_hold() {
            return false;
        }
        self.state
            .compare_exchange(
                LifecycleAdmissionState::Draining as u8,
                LifecycleAdmissionState::Active as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    fn release_permit(&self) {
        if self.active_permits.fetch_sub(1, Ordering::AcqRel) == 1 {
            // Keep one notification as a permit if the waiter has not been
            // scheduled yet; this avoids a check/await race in wait_for_drain.
            self.drained.notify_one();
        }
    }
}

impl Default for LifecycleAdmission {
    fn default() -> Self {
        Self::new()
    }
}

pub struct LifecycleAdmissionPermit {
    admission: Arc<LifecycleAdmission>,
}

impl Drop for LifecycleAdmissionPermit {
    fn drop(&mut self) {
        self.admission.release_permit();
    }
}

pub struct AppState {
    pub config_path: PathBuf,
    pub daemon_identity: DaemonIdentity,
    pub config: Mutex<AppConfig>,
    pub ai_gateway_request_logs: RequestLogStore,
    pub ai_gateway_routing: Mutex<GatewayRoutingState>,
    pub persisted: Mutex<PersistedState>,
    pub runtime: Mutex<RuntimeState>,
    pub im_route_binding_ops: Mutex<()>,
    pub remote_control: RemoteControlState,
    pub events: Mutex<Vec<EventRecord>>,
    pub bridge_task: Mutex<Option<JoinHandle<()>>>,
    pub feishu_ws: Mutex<FeishuWsState>,
    pub telegram: Mutex<TelegramState>,
    pub wechat: Mutex<WechatState>,
    pub wechat_recovery: Mutex<WechatRecoveryState>,
    pub im_accounts: Mutex<HashMap<String, ImAccountRuntimeState>>,
    pub wechat_onboard: Mutex<Option<WechatOnboardSession>>,
    pub wecom_onboard: Mutex<Option<WecomOnboardSession>>,
    pub safe_relaunch: Mutex<Option<crate::safe_relaunch::PendingSafeRelaunch>>,
    pub shutdown_tx: Mutex<Option<oneshot::Sender<()>>>,
    pub lifecycle_admission: Arc<LifecycleAdmission>,
    /// Serializes lease mutations with an in-progress async drain without
    /// holding the filesystem lock across await points.
    pub lifecycle_control: Mutex<()>,
}

pub struct RemoteControlState {
    pub inner: Mutex<RemoteControlInner>,
    pub notifications: broadcast::Sender<CodexNotification>,
}

pub struct RemoteControlInner {
    pub connections: HashMap<String, RemoteControlServerConnection>,
    pub active_connection_id: Option<String>,
    pub next_connection_epoch: u64,
    pub pending_source_hints_by_installation: HashMap<String, RemoteControlSourceHint>,
    pub connected: bool,
    pub initialized: bool,
    pub client_id: String,
    pub stream_id: String,
    pub server_id: Option<String>,
    pub environment_id: Option<String>,
    pub server_name: Option<String>,
    pub installation_id: Option<String>,
    pub account_id: Option<String>,
    pub current_thread_id: Option<String>,
    pub current_turn_id: Option<String>,
    pub last_error: Option<String>,
    pub connected_at_ms: Option<u128>,
    pub last_ws_inbound_at_ms: Option<u128>,
    pub last_ws_ping_at_ms: Option<u128>,
    pub last_ws_pong_at_ms: Option<u128>,
    pub last_app_ping_at_ms: Option<u128>,
    pub last_app_pong_at_ms: Option<u128>,
    pub last_app_pong_status: Option<String>,
    pub last_initialize_sent_at_ms: Option<u128>,
    pub subscribe_cursor: Option<String>,
    pub server_ack_cursors: std::collections::HashMap<String, (u64, Option<usize>)>,
    pub outbound_tx: Option<
        tokio::sync::mpsc::UnboundedSender<crate::remote_control_backend::OutboundWsMessage>,
    >,
    pub connection_epoch: u64,
    pub clients: HashMap<String, RemoteControlClientState>,
    pub authorized_clients: HashMap<String, AuthorizedRemoteControlClient>,
    pub revoked_clients: HashSet<String>,
    pub stream_diagnostics: HashMap<String, RemoteControlStreamDiagnostics>,
    pub recent_events: VecDeque<RemoteControlRecentEvent>,
}

pub struct RemoteControlSourceHint {
    pub source_kind: RemoteControlSourceKind,
    pub user_agent: Option<String>,
    pub captured_at_ms: u128,
}

pub struct RemoteControlServerConnection {
    pub connection_id: String,
    pub connection_epoch: u64,
    pub default_client_key: String,
    pub connected: bool,
    pub initialized: bool,
    pub source_kind: RemoteControlSourceKind,
    pub user_agent: Option<String>,
    pub server_id: Option<String>,
    pub environment_id: Option<String>,
    pub server_name: Option<String>,
    pub installation_id: Option<String>,
    pub account_id: Option<String>,
    pub subscribe_cursor: Option<String>,
    pub outbound_tx: Option<
        tokio::sync::mpsc::UnboundedSender<crate::remote_control_backend::OutboundWsMessage>,
    >,
    pub connected_at_ms: Option<u128>,
    pub last_ws_inbound_at_ms: Option<u128>,
    pub last_ws_ping_at_ms: Option<u128>,
    pub last_ws_pong_at_ms: Option<u128>,
    pub last_error: Option<String>,
    pub clients: HashMap<String, RemoteControlClientState>,
    #[allow(dead_code)]
    pub stream_diagnostics: HashMap<String, RemoteControlStreamDiagnostics>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteControlSourceKind {
    CodexApp,
    Vscode,
    Cli,
    Unknown,
}

impl Default for RemoteControlSourceKind {
    fn default() -> Self {
        Self::Unknown
    }
}

pub struct PendingRemoteRequest {
    pub connection_epoch: u64,
    pub method: String,
    pub thread_id: Option<String>,
    pub track_thread_active: bool,
    /// Keeps the lifecycle admission open for the whole pending request,
    /// rather than only until its outbound envelope has been written.
    #[allow(dead_code)]
    pub lifecycle_permit: Option<LifecycleAdmissionPermit>,
    pub response_tx: oneshot::Sender<anyhow::Result<Value>>,
    pub message: Value,
    pub envelopes: Vec<Value>,
}

pub struct RemoteControlClientState {
    pub client_id: String,
    pub stream_id: String,
    pub initialized: bool,
    pub next_seq_id: u64,
    pub pending: std::collections::HashMap<String, PendingRemoteRequest>,
    pub current_thread_id: Option<String>,
    pub current_turn_id: Option<String>,
    pub last_app_ping_at_ms: Option<u128>,
    pub last_app_pong_at_ms: Option<u128>,
    pub last_app_pong_status: Option<String>,
    pub last_initialize_sent_at_ms: Option<u128>,
    pub recovery_attempt: u64,
    pub recovery_started_at_ms: Option<u128>,
}

pub struct RemoteControlRecentEvent {
    pub ts_ms: u128,
    pub direction: &'static str,
    pub connection_epoch: u64,
    pub client_id: String,
    pub stream_id: String,
    pub seq_id: Option<u64>,
    pub kind: String,
    pub summary: String,
}

#[derive(Default)]
pub struct RemoteControlStreamDiagnostics {
    pub output_delta_count: u64,
    pub output_delta_last_seq_id: Option<u64>,
    pub output_delta_last_item_id: Option<String>,
    pub output_delta_last_thread_id: Option<String>,
    pub output_delta_last_seen_at_ms: Option<u128>,
    pub output_delta_last_worker_capacity: Option<usize>,
    pub window_started_at_ms: Option<u128>,
    pub window_server_in_count: u64,
    pub window_output_delta_count: u64,
    pub window_ack_count: u64,
    pub window_first_seq_id: Option<u64>,
    pub window_last_seq_id: Option<u64>,
    pub max_window_server_in_count: u64,
    pub max_window_output_delta_count: u64,
    pub max_window_ack_count: u64,
    pub max_window_started_at_ms: Option<u128>,
    pub max_window_last_at_ms: Option<u128>,
    pub ack_count: u64,
    pub max_ack_elapsed_ms: u128,
    pub last_ack_elapsed_ms: Option<u128>,
    pub last_ack_seq_id: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct AuthorizedRemoteControlClient {
    pub client_id: String,
    pub account_user_id: String,
    pub device_identity: Option<Value>,
    pub display_name: String,
    pub last_seen_at_ms: u64,
}

impl RemoteControlState {
    pub fn new() -> Self {
        let (notifications, _) = broadcast::channel(512);
        Self {
            inner: Mutex::new(RemoteControlInner {
                connections: HashMap::new(),
                active_connection_id: None,
                next_connection_epoch: 0,
                pending_source_hints_by_installation: HashMap::new(),
                connected: false,
                initialized: false,
                client_id: "codexhub-feishu".to_string(),
                stream_id: String::new(),
                server_id: None,
                environment_id: None,
                server_name: None,
                installation_id: None,
                account_id: None,
                current_thread_id: None,
                current_turn_id: None,
                last_error: None,
                connected_at_ms: None,
                last_ws_inbound_at_ms: None,
                last_ws_ping_at_ms: None,
                last_ws_pong_at_ms: None,
                last_app_ping_at_ms: None,
                last_app_pong_at_ms: None,
                last_app_pong_status: None,
                last_initialize_sent_at_ms: None,
                subscribe_cursor: None,
                server_ack_cursors: std::collections::HashMap::new(),
                outbound_tx: None,
                connection_epoch: 0,
                clients: HashMap::new(),
                authorized_clients: HashMap::new(),
                revoked_clients: HashSet::new(),
                stream_diagnostics: HashMap::new(),
                recent_events: VecDeque::new(),
            }),
            notifications,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeishuWsState {
    pub connecting: bool,
    pub connected: bool,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WechatState {
    pub polling: bool,
    pub connected: bool,
    pub last_error: Option<String>,
    pub last_event_at_ms: Option<u128>,
    pub last_inbound_at_ms: Option<u128>,
}

#[derive(Debug, Default)]
pub struct WechatRecoveryState {
    pub awaiting_fresh_context_token: HashSet<String>,
    pub pending_outbound_by_peer:
        HashMap<String, VecDeque<crate::im::core::outbound::ImOutboundMessage>>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramState {
    pub polling: bool,
    pub connected: bool,
    pub last_error: Option<String>,
    pub last_event_at_ms: Option<u128>,
    pub last_inbound_at_ms: Option<u128>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImAccountRuntimeState {
    pub platform: ImPlatformKind,
    pub account_id: String,
    pub connecting: bool,
    pub polling: bool,
    pub connected: bool,
    pub last_error: Option<String>,
    pub last_event_at_ms: Option<u128>,
    pub last_inbound_at_ms: Option<u128>,
}

impl ImAccountRuntimeState {
    pub fn new(platform: ImPlatformKind, account_id: impl Into<String>) -> Self {
        Self {
            platform,
            account_id: account_id.into(),
            connecting: false,
            polling: false,
            connected: false,
            last_error: None,
            last_event_at_ms: None,
            last_inbound_at_ms: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct WechatOnboardSession {
    pub generation: u64,
    pub session_key: String,
    pub qrcode: String,
    pub started_at_ms: u128,
    pub current_api_base_url: String,
}

#[derive(Debug, Clone)]
pub struct WecomOnboardSession {
    pub generation: u64,
    pub session_key: String,
    pub scode: String,
    pub started_at_ms: u128,
}

impl AppState {
    pub fn new(
        config_path: PathBuf,
        config: AppConfig,
        shutdown_tx: Option<oneshot::Sender<()>>,
        daemon_identity: Option<DaemonIdentity>,
    ) -> SharedState {
        let mut persisted = PersistedState::load(&config.state_path);
        let (runtime, bindings_changed) = restore_persisted_im_bindings(&config, &mut persisted);
        if bindings_changed && let Err(err) = persisted.save(&config.state_path) {
            chain_log::write_line(format!(
                "[im_route] level=warn event=persisted_binding_cleanup_save_failed path={} err={err}",
                config.state_path.display()
            ));
        }
        let request_log_db_path = crate::ai_gateway::request_log::database_path(&config);
        crate::ai_gateway::request_log::migrate_legacy_database(&config, &request_log_db_path);
        let ai_gateway_request_logs = RequestLogStore::new(request_log_db_path);
        Arc::new(Self {
            config_path,
            daemon_identity: daemon_identity.unwrap_or_else(DaemonIdentity::new),
            config: Mutex::new(config),
            ai_gateway_request_logs,
            ai_gateway_routing: Mutex::new(GatewayRoutingState::default()),
            persisted: Mutex::new(persisted),
            runtime: Mutex::new(runtime),
            im_route_binding_ops: Mutex::new(()),
            remote_control: RemoteControlState::new(),
            events: Mutex::new(Vec::new()),
            bridge_task: Mutex::new(None),
            feishu_ws: Mutex::new(FeishuWsState::default()),
            telegram: Mutex::new(TelegramState::default()),
            wechat: Mutex::new(WechatState::default()),
            wechat_recovery: Mutex::new(WechatRecoveryState::default()),
            im_accounts: Mutex::new(HashMap::new()),
            wechat_onboard: Mutex::new(None),
            wecom_onboard: Mutex::new(None),
            safe_relaunch: Mutex::new(None),
            shutdown_tx: Mutex::new(shutdown_tx),
            lifecycle_admission: Arc::new(LifecycleAdmission::new()),
            lifecycle_control: Mutex::new(()),
        })
    }

    pub async fn push_event(&self, level: &str, kind: &str, message: impl Into<String>) {
        let message = message.into();
        chain_log::write_line(format!(
            "[event] level={} kind={} message={}",
            level, kind, message
        ));
        match level {
            "error" => tracing::error!(
                target: "threadrelay::event",
                event_kind = kind,
                message = %message,
                "app event"
            ),
            "warn" => tracing::warn!(
                target: "threadrelay::event",
                event_kind = kind,
                message = %message,
                "app event"
            ),
            _ => tracing::info!(
                target: "threadrelay::event",
                event_kind = kind,
                message = %message,
                "app event"
            ),
        }
        let mut events = self.events.lock().await;
        events.push(EventRecord {
            at_ms: now_ms(),
            level: level.to_string(),
            kind: kind.to_string(),
            message,
        });
        if events.len() > 300 {
            let drain = events.len().saturating_sub(300);
            events.drain(0..drain);
        }
    }

    pub async fn request_shutdown(&self) -> bool {
        let mut shutdown_tx = self.shutdown_tx.lock().await;
        if let Some(tx) = shutdown_tx.take() {
            tx.send(()).is_ok()
        } else {
            false
        }
    }
}

fn restore_persisted_im_bindings(
    config: &AppConfig,
    persisted: &mut PersistedState,
) -> (RuntimeState, bool) {
    let original_bindings = persisted.im_thread_bindings.clone();
    let mut bindings = original_bindings.iter().collect::<Vec<_>>();
    bindings.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));

    let mut runtime = RuntimeState::default();
    let mut restored_bindings = HashMap::new();
    let mut claimed_threads = HashSet::new();

    for (conversation_key, thread_id) in bindings {
        let thread_id = thread_id.trim();
        if thread_id.is_empty() {
            continue;
        }
        let Some(route) = valid_persisted_telegram_route(config, conversation_key) else {
            continue;
        };
        if !claimed_threads.insert(thread_id.to_string()) {
            continue;
        }

        runtime.bind_route(thread_id, route);
        restored_bindings.insert(conversation_key.clone(), thread_id.to_string());
    }

    let changed = restored_bindings != original_bindings;
    persisted.im_thread_bindings = restored_bindings;
    (runtime, changed)
}

fn valid_persisted_telegram_route(
    config: &AppConfig,
    conversation_key: &str,
) -> Option<RouteTarget> {
    let route = route_from_conversation_key(conversation_key)?;
    if route.platform != ImPlatformKind::Telegram
        || route.account_id.trim() != route.account_id
        || route.chat_id.trim() != route.chat_id
    {
        return None;
    }

    let account = config.telegram_account(&route.account_id)?;
    if !account.is_active()
        || !account
            .allowed_chat_ids
            .iter()
            .any(|chat_id| chat_id.trim() == route.chat_id)
    {
        return None;
    }

    Some(route.with_deterministic_remote_client_key())
}

pub fn im_account_key(platform: ImPlatformKind, account_id: &str) -> String {
    format!("{}:{}", platform.key(), account_id.trim())
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[tokio::test]
    async fn lifecycle_admission_closes_race_and_commits_once() {
        let admission = Arc::new(LifecycleAdmission::new());
        let permit = admission.try_admit().expect("active work admitted");

        assert!(admission.begin_draining());
        assert_eq!(admission.state(), LifecycleAdmissionState::Draining);
        assert!(admission.try_admit().is_none());

        let wait = tokio::spawn({
            let admission = Arc::clone(&admission);
            async move { admission.wait_for_drain().await }
        });
        tokio::task::yield_now().await;
        assert!(!wait.is_finished());

        drop(permit);
        wait.await.expect("drain waiter");
        assert!(admission.commit_shutdown());
        assert_eq!(
            admission.state(),
            LifecycleAdmissionState::ShutdownCommitted
        );
        assert!(!admission.commit_shutdown());
        assert!(!admission.begin_draining());
    }

    #[tokio::test]
    async fn lifecycle_admission_can_cancel_draining_after_protected_conflict() {
        let admission = Arc::new(LifecycleAdmission::new());
        assert!(admission.begin_draining());
        assert!(admission.cancel_draining());
        assert_eq!(admission.state(), LifecycleAdmissionState::Active);
        assert!(admission.try_admit().is_some());
    }

    #[tokio::test]
    async fn candidate_hold_stays_draining_until_commit() {
        let admission = Arc::new(LifecycleAdmission::new());
        assert!(admission.begin_candidate_hold());
        assert_eq!(admission.state(), LifecycleAdmissionState::Draining);
        assert!(admission.is_candidate_hold());
        assert!(admission.try_admit().is_none());
        assert!(!admission.begin_draining());
        assert!(admission.commit_candidate_hold());
        assert_eq!(admission.state(), LifecycleAdmissionState::Active);
        assert!(!admission.is_candidate_hold());
        assert!(admission.try_admit().is_some());
    }

    #[tokio::test]
    async fn candidate_hold_can_be_consumed_for_safe_shutdown_and_restored() {
        let admission = Arc::new(LifecycleAdmission::new());
        assert!(admission.begin_candidate_hold());
        assert!(admission.take_candidate_hold_for_shutdown());
        assert!(!admission.is_candidate_hold());
        assert!(admission.restore_candidate_hold());
        assert!(admission.is_candidate_hold());
        assert!(admission.take_candidate_hold_for_shutdown());
        assert!(admission.commit_shutdown());
        assert_eq!(
            admission.state(),
            LifecycleAdmissionState::ShutdownCommitted
        );
        assert!(!admission.is_candidate_hold());
    }
    use crate::config::TelegramConfig;

    fn telegram_account(
        account_id: &str,
        enabled: bool,
        allowed_chat_ids: &[&str],
    ) -> TelegramConfig {
        TelegramConfig {
            enabled,
            account_id: account_id.to_string(),
            bot_token: "token".to_string(),
            allowed_chat_ids: allowed_chat_ids
                .iter()
                .map(|chat_id| (*chat_id).to_string())
                .collect(),
            ..TelegramConfig::default()
        }
    }

    #[tokio::test]
    async fn startup_restores_only_valid_telegram_thread_bindings() {
        let temp_dir = tempdir().expect("temp dir");
        let state_path = temp_dir.path().join("state.json");
        let config_path = temp_dir.path().join("config.toml");
        let mut persisted = PersistedState::default();
        persisted.im_thread_bindings = HashMap::from([
            ("telegram:active:42".to_string(), "thread-42".to_string()),
            (
                "telegram:active:99".to_string(),
                "thread-disallowed".to_string(),
            ),
            (
                "telegram:disabled:43".to_string(),
                "thread-disabled".to_string(),
            ),
            (
                "telegram:missing:44".to_string(),
                "thread-missing".to_string(),
            ),
            ("feishu:active:42".to_string(), "thread-feishu".to_string()),
            ("telegram:active:45".to_string(), "   ".to_string()),
        ]);
        persisted.save(&state_path).expect("save initial state");

        let mut config = AppConfig::default();
        config.state_path = state_path.clone();
        config.telegram_accounts = vec![
            telegram_account("active", true, &["42"]),
            telegram_account("disabled", false, &["43"]),
        ];

        let state = AppState::new(config_path, config, None, None);
        let runtime = state.runtime.lock().await;
        let route = runtime
            .route_by_thread
            .get("thread-42")
            .expect("restored route");
        assert_eq!(route.conversation_key, "telegram:active:42");
        assert_eq!(
            route.remote_client_key,
            RouteTarget::deterministic_remote_client_key_for(
                ImPlatformKind::Telegram,
                "active",
                "42"
            )
        );
        assert_eq!(runtime.route_by_thread.len(), 1);
        drop(runtime);

        let persisted = state.persisted.lock().await;
        assert_eq!(
            persisted.im_thread_bindings,
            HashMap::from([("telegram:active:42".to_string(), "thread-42".to_string())])
        );
        drop(persisted);

        let saved = PersistedState::load(&state_path);
        assert_eq!(
            saved.im_thread_bindings,
            HashMap::from([("telegram:active:42".to_string(), "thread-42".to_string())])
        );
    }

    #[test]
    fn duplicate_thread_binding_restores_one_conversation_deterministically() {
        let mut config = AppConfig::default();
        config.telegram_accounts = vec![telegram_account("active", true, &["41", "42"])];
        let mut persisted = PersistedState::default();
        persisted.im_thread_bindings = HashMap::from([
            ("telegram:active:42".to_string(), "same-thread".to_string()),
            ("telegram:active:41".to_string(), "same-thread".to_string()),
        ]);

        let (runtime, changed) = restore_persisted_im_bindings(&config, &mut persisted);

        assert!(changed);
        assert_eq!(runtime.route_by_thread.len(), 1);
        assert_eq!(
            runtime
                .route_by_thread
                .get("same-thread")
                .map(|route| route.conversation_key.as_str()),
            Some("telegram:active:41")
        );
        assert_eq!(
            persisted.im_thread_bindings,
            HashMap::from([("telegram:active:41".to_string(), "same-thread".to_string())])
        );
    }

    #[test]
    fn invalid_binding_does_not_claim_thread_needed_by_valid_binding() {
        let mut config = AppConfig::default();
        config.telegram_accounts = vec![telegram_account("active", true, &["42"])];
        let mut persisted = PersistedState::default();
        persisted.im_thread_bindings = HashMap::from([
            ("feishu:active:41".to_string(), "same-thread".to_string()),
            ("telegram:active:42".to_string(), "same-thread".to_string()),
        ]);

        let (runtime, changed) = restore_persisted_im_bindings(&config, &mut persisted);

        assert!(changed);
        assert_eq!(
            runtime
                .route_by_thread
                .get("same-thread")
                .map(|route| route.conversation_key.as_str()),
            Some("telegram:active:42")
        );
        assert_eq!(
            persisted.im_thread_bindings,
            HashMap::from([("telegram:active:42".to_string(), "same-thread".to_string())])
        );
    }
}
