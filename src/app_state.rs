use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::{
        Arc, Weak,
        atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
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
    types::{EventRecord, ImPlatformKind, now_ms, split_telegram_message_target},
};

pub type SharedState = Arc<AppState>;

pub(crate) struct TelegramTopicCleanupRegistration {
    pub(crate) token: u64,
    pub(crate) lifecycle_generation: u64,
    pub(crate) lifecycle_revision: u64,
    pub(crate) notifier: Arc<Notify>,
}

pub(crate) struct TelegramTopicNameSyncMarker {
    pub(crate) name: String,
    pub(crate) token: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TelegramThreadLifecycleState {
    Active,
    Archived,
    Deleted,
}

#[derive(Debug, Clone, Copy)]
struct TelegramThreadLifecycleIntent {
    generation: u64,
    revision: u64,
    state: TelegramThreadLifecycleState,
}

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
    active_permits: AtomicUsize,
    drained: Notify,
}

impl LifecycleAdmission {
    pub fn new() -> Self {
        Self {
            state: AtomicU8::new(LifecycleAdmissionState::Active as u8),
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
        self.state
            .compare_exchange(
                LifecycleAdmissionState::Draining as u8,
                LifecycleAdmissionState::ShutdownCommitted as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    pub fn cancel_draining(&self) -> bool {
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
    pub telegram_queue_start_ops: Mutex<()>,
    /// Serializes complete Topic discovery/import workflows per account so a
    /// timed-out management request cannot start a duplicate pass.
    pub telegram_topic_sync_ops: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    /// Serializes Topic discovery and binding for one Codex thread across
    /// Telegram accounts. Weak entries disappear after the workflow releases
    /// its gate, so session history cannot grow this map without bound.
    telegram_topic_creation_ops: Mutex<HashMap<String, Weak<Mutex<()>>>>,
    /// Serializes only the individual Telegram Topic API attempt. Retry-After
    /// and generic backoff waits deliberately happen outside this gate.
    pub telegram_topic_mutation_ops: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    /// Names MochiPort is currently applying to Telegram Topics. Telegram
    /// echoes bot edits as service messages; these markers prevent that echo
    /// from being mistaken for a user rename.
    pub telegram_topic_name_sync_ops: Mutex<HashMap<String, VecDeque<TelegramTopicNameSyncMarker>>>,
    telegram_topic_name_sync_next_token: AtomicU64,
    /// Orders Codex-to-Telegram name notifications independently from Telegram
    /// service-message echo markers. A newer token supersedes stale API work.
    pub telegram_topic_name_update_ops: Mutex<HashMap<String, u64>>,
    telegram_topic_name_update_next_token: AtomicU64,
    telegram_thread_lifecycle_intents: Mutex<HashMap<String, TelegramThreadLifecycleIntent>>,
    telegram_thread_lifecycle_latest_generation: AtomicU64,
    telegram_thread_lifecycle_next_revision: AtomicU64,
    /// Telegram Topic deletions can be requested by both Codex lifecycle
    /// notifications and reconciliation. Each registration atomically owns
    /// the worker token, latest lifecycle revision, and cancellation notifier.
    pub telegram_topic_cleanup_registrations:
        Mutex<HashMap<String, TelegramTopicCleanupRegistration>>,
    telegram_topic_cleanup_next_token: AtomicU64,
    /// Account-wide Retry-After deadlines shared by every Telegram Topic
    /// create, edit, and delete attempt.
    telegram_topic_cleanup_retry_deadlines: Mutex<HashMap<String, Instant>>,
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
    pub im_account_profiles: Mutex<HashMap<String, ImAccountProfile>>,
    pub im_account_profile_refresh: AtomicU8,
    pub shutdown_tx: Mutex<Option<oneshot::Sender<()>>>,
    pub lifecycle_admission: Arc<LifecycleAdmission>,
    pub enhanced_launch_operations: Arc<crate::codex_app_enhanced::EnhancedLaunchOperationManager>,
    pub codex_app_mutations: Arc<Mutex<()>>,
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
    pub next_connection_epoch: u64,
    pub pending_source_hints_by_installation: HashMap<String, RemoteControlSourceHint>,
    pub authorized_clients: HashMap<String, AuthorizedRemoteControlClient>,
    pub revoked_clients: HashSet<String>,
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
    pub server_ack_cursors: HashMap<String, (u64, Option<usize>)>,
    pub stream_diagnostics: HashMap<String, RemoteControlStreamDiagnostics>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum RemoteControlSourceKind {
    CodexApp,
    Vscode,
    Cli,
    #[default]
    Unknown,
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
                next_connection_epoch: 0,
                pending_source_hints_by_installation: HashMap::new(),
                authorized_clients: HashMap::new(),
                revoked_clients: HashSet::new(),
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

/// Cached presentation metadata for a messaging account. This stays separate
/// from connection runtime state so loading an avatar cannot make an account
/// look connected or alter the daemon status snapshot.
#[derive(Debug, Clone, Default)]
pub struct ImAccountProfile {
    pub avatar_data: Option<String>,
    pub avatar_mime_type: Option<String>,
    pub avatar_checked_at_ms: Option<u128>,
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
            telegram_queue_start_ops: Mutex::new(()),
            telegram_topic_sync_ops: Mutex::new(HashMap::new()),
            telegram_topic_creation_ops: Mutex::new(HashMap::new()),
            telegram_topic_mutation_ops: Mutex::new(HashMap::new()),
            telegram_topic_name_sync_ops: Mutex::new(HashMap::new()),
            telegram_topic_name_sync_next_token: AtomicU64::new(1),
            telegram_topic_name_update_ops: Mutex::new(HashMap::new()),
            telegram_topic_name_update_next_token: AtomicU64::new(1),
            telegram_thread_lifecycle_intents: Mutex::new(HashMap::new()),
            telegram_thread_lifecycle_latest_generation: AtomicU64::new(0),
            telegram_thread_lifecycle_next_revision: AtomicU64::new(1),
            telegram_topic_cleanup_registrations: Mutex::new(HashMap::new()),
            telegram_topic_cleanup_next_token: AtomicU64::new(1),
            telegram_topic_cleanup_retry_deadlines: Mutex::new(HashMap::new()),
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
            im_account_profiles: Mutex::new(HashMap::new()),
            im_account_profile_refresh: AtomicU8::new(0),
            shutdown_tx: Mutex::new(shutdown_tx),
            lifecycle_admission: Arc::new(LifecycleAdmission::new()),
            enhanced_launch_operations: Arc::new(
                crate::codex_app_enhanced::EnhancedLaunchOperationManager::new(),
            ),
            codex_app_mutations: Arc::new(Mutex::new(())),
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
                target: "mochiport::event",
                event_kind = kind,
                message = %message,
                "app event"
            ),
            "warn" => tracing::warn!(
                target: "mochiport::event",
                event_kind = kind,
                message = %message,
                "app event"
            ),
            _ => tracing::info!(
                target: "mochiport::event",
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

    pub async fn telegram_topic_sync_gate(&self, account_id: &str) -> Arc<Mutex<()>> {
        let account_id = normalized_telegram_account_id(account_id);
        self.telegram_topic_sync_ops
            .lock()
            .await
            .entry(account_id)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    pub async fn telegram_topic_mutation_gate(&self, account_id: &str) -> Arc<Mutex<()>> {
        let account_id = normalized_telegram_account_id(account_id);
        self.telegram_topic_mutation_ops
            .lock()
            .await
            .entry(account_id)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    pub async fn telegram_topic_creation_gate(&self, thread_id: &str) -> Arc<Mutex<()>> {
        let thread_id = thread_id.trim();
        let mut gates = self.telegram_topic_creation_ops.lock().await;
        gates.retain(|_, gate| gate.strong_count() > 0);
        if let Some(gate) = gates.get(thread_id).and_then(Weak::upgrade) {
            return gate;
        }
        let gate = Arc::new(Mutex::new(()));
        gates.insert(thread_id.to_string(), Arc::downgrade(&gate));
        gate
    }

    pub async fn telegram_topic_cleanup_pending_for_account(&self, account_id: &str) -> bool {
        let account_id = normalized_telegram_account_id(account_id);
        self.telegram_topic_cleanup_registrations
            .lock()
            .await
            .keys()
            .any(|conversation_key| {
                route_from_conversation_key(conversation_key)
                    .is_some_and(|route| route.account_id == account_id)
            })
    }

    pub(crate) async fn begin_telegram_topic_name_update(&self, conversation_key: &str) -> u64 {
        let token = self
            .telegram_topic_name_update_next_token
            .fetch_add(1, Ordering::Relaxed);
        self.telegram_topic_name_update_ops
            .lock()
            .await
            .insert(conversation_key.to_string(), token);
        token
    }

    pub(crate) fn next_telegram_topic_name_sync_token(&self) -> u64 {
        self.telegram_topic_name_sync_next_token
            .fetch_add(1, Ordering::Relaxed)
    }

    pub(crate) async fn telegram_topic_name_update_is_current(
        &self,
        conversation_key: &str,
        token: u64,
    ) -> bool {
        self.telegram_topic_name_update_ops
            .lock()
            .await
            .get(conversation_key)
            .is_some_and(|current| *current == token)
    }

    pub(crate) async fn finish_telegram_topic_name_update(
        &self,
        conversation_key: &str,
        token: u64,
    ) {
        let mut operations = self.telegram_topic_name_update_ops.lock().await;
        if operations
            .get(conversation_key)
            .is_some_and(|current| *current == token)
        {
            operations.remove(conversation_key);
        }
    }

    /// Record a start without reviving an archive/delete already observed in
    /// the same bridge generation. This closes the detached auto-create race.
    pub(crate) async fn observe_telegram_thread_started(
        &self,
        thread_id: &str,
        generation: u64,
    ) -> bool {
        let previous_generation = self
            .telegram_thread_lifecycle_latest_generation
            .fetch_max(generation, Ordering::Relaxed);
        if generation < previous_generation {
            return false;
        }
        let mut intents = self.telegram_thread_lifecycle_intents.lock().await;
        if generation > previous_generation {
            intents.retain(|_, intent| intent.generation >= generation);
        }
        match intents.get_mut(thread_id) {
            Some(intent) if intent.generation > generation => false,
            Some(intent) if intent.generation == generation => {
                intent.state == TelegramThreadLifecycleState::Active
            }
            Some(intent) => {
                *intent = TelegramThreadLifecycleIntent {
                    generation,
                    revision: 0,
                    state: TelegramThreadLifecycleState::Active,
                };
                true
            }
            None => {
                intents.insert(
                    thread_id.to_string(),
                    TelegramThreadLifecycleIntent {
                        generation,
                        revision: 0,
                        state: TelegramThreadLifecycleState::Active,
                    },
                );
                true
            }
        }
    }

    pub(crate) async fn observe_telegram_thread_lifecycle(
        &self,
        thread_id: &str,
        generation: u64,
        next_state: TelegramThreadLifecycleState,
    ) -> bool {
        self.observe_telegram_thread_lifecycle_inner(thread_id, generation, None, next_state)
            .await
            .unwrap_or(false)
    }

    pub(crate) async fn telegram_thread_lifecycle_revision(
        &self,
        thread_id: &str,
        generation: u64,
    ) -> Option<u64> {
        if generation
            < self
                .telegram_thread_lifecycle_latest_generation
                .load(Ordering::Relaxed)
        {
            return None;
        }
        let intents = self.telegram_thread_lifecycle_intents.lock().await;
        match intents.get(thread_id) {
            Some(intent) if intent.generation > generation => None,
            Some(intent) if intent.generation == generation => Some(intent.revision),
            _ => Some(0),
        }
    }

    pub(crate) async fn observe_telegram_thread_lifecycle_if_revision(
        &self,
        thread_id: &str,
        generation: u64,
        expected_revision: u64,
        next_state: TelegramThreadLifecycleState,
    ) -> Option<bool> {
        self.observe_telegram_thread_lifecycle_inner(
            thread_id,
            generation,
            Some(expected_revision),
            next_state,
        )
        .await
    }

    async fn observe_telegram_thread_lifecycle_inner(
        &self,
        thread_id: &str,
        generation: u64,
        expected_revision: Option<u64>,
        next_state: TelegramThreadLifecycleState,
    ) -> Option<bool> {
        let previous_generation = self
            .telegram_thread_lifecycle_latest_generation
            .fetch_max(generation, Ordering::Relaxed);
        if generation < previous_generation {
            return None;
        }
        let mut intents = self.telegram_thread_lifecycle_intents.lock().await;
        if generation > previous_generation {
            intents.retain(|_, intent| intent.generation >= generation);
        }
        let current = intents.get(thread_id).copied();
        if current.is_some_and(|intent| intent.generation > generation) {
            return None;
        }
        let current_revision = current
            .filter(|intent| intent.generation == generation)
            .map_or(0, |intent| intent.revision);
        if expected_revision.is_some_and(|expected| expected != current_revision) {
            return None;
        }
        if current.is_some_and(|intent| {
            intent.generation == generation
                && intent.state == TelegramThreadLifecycleState::Deleted
                && next_state != TelegramThreadLifecycleState::Deleted
        }) {
            return None;
        }
        let revision = self
            .telegram_thread_lifecycle_next_revision
            .fetch_add(1, Ordering::Relaxed);
        intents.insert(
            thread_id.to_string(),
            TelegramThreadLifecycleIntent {
                generation,
                revision,
                state: next_state,
            },
        );
        Some(next_state == TelegramThreadLifecycleState::Active)
    }

    pub(crate) async fn telegram_thread_allows_topic_binding(
        &self,
        thread_id: &str,
        generation: u64,
    ) -> bool {
        if generation
            < self
                .telegram_thread_lifecycle_latest_generation
                .load(Ordering::Relaxed)
        {
            return false;
        }
        self.telegram_thread_lifecycle_intents
            .lock()
            .await
            .get(thread_id)
            .is_none_or(|intent| {
                intent.generation < generation
                    || (intent.generation == generation
                        && intent.state == TelegramThreadLifecycleState::Active)
            })
    }

    pub(crate) fn next_telegram_topic_cleanup_token(&self) -> u64 {
        self.telegram_topic_cleanup_next_token
            .fetch_add(1, Ordering::Relaxed)
    }

    pub(crate) async fn notify_telegram_topic_cleanup_if_older(
        &self,
        conversation_key: &str,
        lifecycle_generation: u64,
        lifecycle_revision: u64,
    ) {
        let notifier = self
            .telegram_topic_cleanup_registrations
            .lock()
            .await
            .get(conversation_key)
            .filter(|registration| {
                (
                    registration.lifecycle_generation,
                    registration.lifecycle_revision,
                ) < (lifecycle_generation, lifecycle_revision)
            })
            .map(|registration| registration.notifier.clone());
        if let Some(notifier) = notifier {
            notifier.notify_one();
        }
    }

    pub(crate) async fn extend_telegram_topic_cleanup_retry_deadline(
        &self,
        account_id: &str,
        delay: Duration,
    ) -> Instant {
        let account_id = normalized_telegram_account_id(account_id);
        let requested = Instant::now() + delay;
        let mut deadlines = self.telegram_topic_cleanup_retry_deadlines.lock().await;
        let deadline = deadlines.entry(account_id).or_insert(requested);
        if *deadline < requested {
            *deadline = requested;
        }
        *deadline
    }

    pub(crate) async fn telegram_topic_cleanup_retry_deadline(
        &self,
        account_id: &str,
    ) -> Option<Instant> {
        let account_id = normalized_telegram_account_id(account_id);
        self.telegram_topic_cleanup_retry_deadlines
            .lock()
            .await
            .get(&account_id)
            .copied()
    }

    pub(crate) async fn clear_telegram_topic_cleanup_retry_deadline_if_elapsed(
        &self,
        account_id: &str,
        observed_deadline: Instant,
    ) {
        let account_id = normalized_telegram_account_id(account_id);
        let mut deadlines = self.telegram_topic_cleanup_retry_deadlines.lock().await;
        if deadlines
            .get(&account_id)
            .is_some_and(|deadline| *deadline <= observed_deadline && *deadline <= Instant::now())
        {
            deadlines.remove(&account_id);
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

fn normalized_telegram_account_id(account_id: &str) -> String {
    let account_id = account_id.trim();
    if account_id.is_empty() {
        "telegram".to_string()
    } else {
        account_id.to_string()
    }
}

fn restore_persisted_im_bindings(
    config: &AppConfig,
    persisted: &mut PersistedState,
) -> (RuntimeState, bool) {
    let original_bindings = persisted.im_thread_bindings.clone();
    let original_topic_states = persisted.telegram_topic_binding_states.clone();
    let mut bindings = original_bindings.iter().collect::<Vec<_>>();
    bindings.sort_unstable_by_key(|(left, _)| *left);

    let mut runtime = RuntimeState::default();
    let mut restored_bindings = HashMap::new();
    let mut restored_topic_states = HashMap::new();
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
        let mut binding_state = persisted
            .telegram_topic_binding_states
            .get(conversation_key)
            .cloned()
            .unwrap_or_default();
        binding_state.thread_id = thread_id.to_string();
        binding_state.lifecycle_generation = 0;
        restored_topic_states.insert(conversation_key.clone(), binding_state);
    }

    let changed =
        restored_bindings != original_bindings || restored_topic_states != original_topic_states;
    persisted.im_thread_bindings = restored_bindings;
    persisted.telegram_topic_binding_states = restored_topic_states;
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
    let (raw_chat_id, topic_id) = split_telegram_message_target(&route.chat_id);
    let allowed_private_chat = account
        .allowed_chat_ids
        .iter()
        .any(|chat_id| chat_id.trim() == raw_chat_id);
    let configured_project_group = account.project_group_for_chat(raw_chat_id).is_some();
    let target_is_valid = if topic_id.is_some() {
        configured_project_group
    } else {
        allowed_private_chat || configured_project_group
    };
    if !account.is_active() || !target_is_valid {
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
    async fn telegram_topic_sync_gates_are_shared_per_account_and_independent_across_accounts() {
        let temp_dir = tempdir().expect("temp dir");
        let mut config = AppConfig::default();
        config.state_path = temp_dir.path().join("state.json");
        let state = AppState::new(temp_dir.path().join("config.toml"), config, None, None);

        let account_a = state.telegram_topic_sync_gate(" account-a ").await;
        let account_a_again = state.telegram_topic_sync_gate("account-a").await;
        let account_b = state.telegram_topic_sync_gate("account-b").await;

        assert!(Arc::ptr_eq(&account_a, &account_a_again));
        assert!(!Arc::ptr_eq(&account_a, &account_b));
        let _account_a_guard = account_a.lock().await;
        assert!(account_a_again.try_lock().is_err());
        assert!(account_b.try_lock().is_ok());
    }

    #[tokio::test]
    async fn telegram_topic_mutation_gates_are_separate_from_workflow_gates() {
        let temp_dir = tempdir().expect("temp dir");
        let mut config = AppConfig::default();
        config.state_path = temp_dir.path().join("state.json");
        let state = AppState::new(temp_dir.path().join("config.toml"), config, None, None);

        let workflow_gate = state.telegram_topic_sync_gate("account-a").await;
        let mutation_gate = state.telegram_topic_mutation_gate("account-a").await;
        let mutation_gate_again = state.telegram_topic_mutation_gate(" account-a ").await;
        let other_account_gate = state.telegram_topic_mutation_gate("account-b").await;

        assert!(!Arc::ptr_eq(&workflow_gate, &mutation_gate));
        assert!(Arc::ptr_eq(&mutation_gate, &mutation_gate_again));
        assert!(!Arc::ptr_eq(&mutation_gate, &other_account_gate));
    }

    #[tokio::test]
    async fn telegram_topic_creation_gate_is_shared_per_thread_without_retaining_history() {
        let temp_dir = tempdir().expect("temp dir");
        let mut config = AppConfig::default();
        config.state_path = temp_dir.path().join("state.json");
        let state = AppState::new(temp_dir.path().join("config.toml"), config, None, None);

        let first = state.telegram_topic_creation_gate(" thread-1 ").await;
        let same = state.telegram_topic_creation_gate("thread-1").await;
        let other = state.telegram_topic_creation_gate("thread-2").await;
        assert!(Arc::ptr_eq(&first, &same));
        assert!(!Arc::ptr_eq(&first, &other));
        let expired = Arc::downgrade(&first);
        drop(first);
        drop(same);
        drop(other);
        assert!(expired.upgrade().is_none());

        let replacement = state.telegram_topic_creation_gate("thread-1").await;
        assert_eq!(state.telegram_topic_creation_ops.lock().await.len(), 1);
        assert_eq!(Arc::strong_count(&replacement), 1);
    }

    #[tokio::test]
    async fn telegram_thread_lifecycle_tombstones_survive_stale_started_snapshots() {
        let temp_dir = tempdir().expect("temp dir");
        let mut config = AppConfig::default();
        config.state_path = temp_dir.path().join("state.json");
        let state = AppState::new(temp_dir.path().join("config.toml"), config, None, None);

        assert!(state.observe_telegram_thread_started("thread-1", 7).await);
        state
            .observe_telegram_thread_lifecycle(
                "thread-1",
                7,
                TelegramThreadLifecycleState::Archived,
            )
            .await;
        assert!(!state.observe_telegram_thread_started("thread-1", 7).await);
        assert!(
            !state
                .telegram_thread_allows_topic_binding("thread-1", 7)
                .await
        );

        assert!(
            state
                .observe_telegram_thread_lifecycle(
                    "thread-1",
                    7,
                    TelegramThreadLifecycleState::Active,
                )
                .await
        );
        assert!(
            state
                .telegram_thread_allows_topic_binding("thread-1", 7)
                .await
        );

        state
            .observe_telegram_thread_lifecycle("thread-1", 7, TelegramThreadLifecycleState::Deleted)
            .await;
        assert!(
            !state
                .observe_telegram_thread_lifecycle(
                    "thread-1",
                    7,
                    TelegramThreadLifecycleState::Active,
                )
                .await
        );
        assert!(!state.observe_telegram_thread_started("thread-1", 7).await);

        assert!(state.observe_telegram_thread_started("thread-1", 8).await);
        assert!(
            state
                .telegram_thread_allows_topic_binding("thread-1", 8)
                .await
        );
    }

    #[tokio::test]
    async fn telegram_topic_cleanup_pending_is_scoped_to_account() {
        let temp_dir = tempdir().expect("temp dir");
        let mut config = AppConfig::default();
        config.state_path = temp_dir.path().join("state.json");
        let state = AppState::new(temp_dir.path().join("config.toml"), config, None, None);
        state
            .telegram_topic_cleanup_registrations
            .lock()
            .await
            .insert(
                "telegram:account-a:-100|topic=42".to_string(),
                TelegramTopicCleanupRegistration {
                    token: 1,
                    lifecycle_generation: 1,
                    lifecycle_revision: 1,
                    notifier: Arc::new(Notify::new()),
                },
            );

        assert!(
            state
                .telegram_topic_cleanup_pending_for_account("account-a")
                .await
        );
        assert!(
            !state
                .telegram_topic_cleanup_pending_for_account("account-b")
                .await
        );
    }

    #[tokio::test]
    async fn active_lifecycle_only_wakes_an_older_topic_cleanup_registration() {
        let temp_dir = tempdir().expect("temp dir");
        let mut config = AppConfig::default();
        config.state_path = temp_dir.path().join("state.json");
        let state = AppState::new(temp_dir.path().join("config.toml"), config, None, None);
        let conversation_key = "telegram:account-a:-100|topic=42";
        let notifier = Arc::new(Notify::new());
        state
            .telegram_topic_cleanup_registrations
            .lock()
            .await
            .insert(
                conversation_key.to_string(),
                TelegramTopicCleanupRegistration {
                    token: 1,
                    lifecycle_generation: 3,
                    lifecycle_revision: 5,
                    notifier: notifier.clone(),
                },
            );

        state
            .notify_telegram_topic_cleanup_if_older(conversation_key, 3, 4)
            .await;
        assert!(
            tokio::time::timeout(Duration::from_millis(20), notifier.notified())
                .await
                .is_err()
        );

        state
            .notify_telegram_topic_cleanup_if_older(conversation_key, 3, 6)
            .await;
        tokio::time::timeout(Duration::from_millis(100), notifier.notified())
            .await
            .expect("newer active lifecycle wakes stale cleanup");
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
                "telegram:active:42|topic=17".to_string(),
                "thread-private-topic".to_string(),
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
    fn configured_project_group_topic_binding_is_restored() {
        let mut config = AppConfig::default();
        let mut account = telegram_account("active", true, &[]);
        account.project_groups = vec![crate::config::TelegramProjectGroupConfig {
            chat_id: "-100".to_string(),
            project_name: "MochiPort".to_string(),
            cwd: "/tmp/mochiport".to_string(),
        }];
        config.telegram_accounts = vec![account];
        let mut persisted = PersistedState::default();
        persisted.im_thread_bindings = HashMap::from([(
            "telegram:active:-100|topic=17".to_string(),
            "topic-thread".to_string(),
        )]);
        persisted.telegram_topic_binding_states.insert(
            "telegram:active:-100|topic=17".to_string(),
            crate::store::TelegramTopicBindingState {
                thread_id: "topic-thread".to_string(),
                lifecycle_generation: 9,
                ..Default::default()
            },
        );

        let (runtime, changed) = restore_persisted_im_bindings(&config, &mut persisted);

        assert!(changed);
        assert_eq!(
            persisted
                .telegram_topic_binding_states
                .get("telegram:active:-100|topic=17")
                .map(|state| state.thread_id.as_str()),
            Some("topic-thread")
        );
        assert_eq!(
            persisted.telegram_topic_binding_states["telegram:active:-100|topic=17"]
                .lifecycle_generation,
            0
        );
        assert_eq!(
            runtime
                .route_by_thread
                .get("topic-thread")
                .map(|route| route.conversation_key.as_str()),
            Some("telegram:active:-100|topic=17")
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
