use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
};

use tokio::{
    sync::{Mutex, broadcast, oneshot},
    task::JoinHandle,
};

use serde::Serialize;
use serde_json::Value;

use crate::{
    chain_log,
    codex::CodexNotification,
    config::AppConfig,
    im_runtime::RuntimeState,
    store::PersistedState,
    types::{EventRecord, now_ms},
};

pub type SharedState = Arc<AppState>;

pub struct AppState {
    pub config_path: PathBuf,
    pub config: Mutex<AppConfig>,
    pub persisted: Mutex<PersistedState>,
    pub runtime: Mutex<RuntimeState>,
    pub remote_control: RemoteControlState,
    pub events: Mutex<Vec<EventRecord>>,
    pub bridge_task: Mutex<Option<JoinHandle<()>>>,
    pub feishu_ws: Mutex<FeishuWsState>,
    pub telegram: Mutex<TelegramState>,
    pub wechat: Mutex<WechatState>,
    pub wechat_onboard: Mutex<Option<WechatOnboardSession>>,
    pub shutdown_tx: Mutex<Option<oneshot::Sender<()>>>,
}

pub struct RemoteControlState {
    pub inner: Mutex<RemoteControlInner>,
    pub notifications: broadcast::Sender<CodexNotification>,
}

pub struct RemoteControlInner {
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
    pub outbound_tx: Option<
        tokio::sync::mpsc::UnboundedSender<crate::remote_control_backend::OutboundWsMessage>,
    >,
    pub connection_epoch: u64,
    pub next_seq_id: u64,
    pub pending:
        std::collections::HashMap<String, tokio::sync::oneshot::Sender<anyhow::Result<Value>>>,
    pub client_request_methods: std::collections::HashMap<String, String>,
    pub client_request_thread_ids: std::collections::HashMap<String, String>,
    pub authorized_clients: HashMap<String, AuthorizedRemoteControlClient>,
    pub revoked_clients: HashSet<String>,
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
                connected: false,
                initialized: false,
                client_id: "codex-remote-feishu".to_string(),
                stream_id: String::new(),
                server_id: None,
                environment_id: None,
                server_name: None,
                installation_id: None,
                account_id: None,
                current_thread_id: None,
                current_turn_id: None,
                last_error: None,
                outbound_tx: None,
                connection_epoch: 0,
                next_seq_id: 1,
                pending: std::collections::HashMap::new(),
                client_request_methods: std::collections::HashMap::new(),
                client_request_thread_ids: std::collections::HashMap::new(),
                authorized_clients: HashMap::new(),
                revoked_clients: HashSet::new(),
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

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramState {
    pub polling: bool,
    pub connected: bool,
    pub last_error: Option<String>,
    pub last_event_at_ms: Option<u128>,
    pub last_inbound_at_ms: Option<u128>,
}

#[derive(Debug, Clone)]
pub struct WechatOnboardSession {
    pub session_key: String,
    pub qrcode: String,
    pub started_at_ms: u128,
    pub current_api_base_url: String,
}

impl AppState {
    pub fn new(
        config_path: PathBuf,
        config: AppConfig,
        shutdown_tx: Option<oneshot::Sender<()>>,
    ) -> SharedState {
        let persisted = PersistedState::load(&config.state_path);
        Arc::new(Self {
            config_path,
            config: Mutex::new(config),
            persisted: Mutex::new(persisted),
            runtime: Mutex::new(RuntimeState::default()),
            remote_control: RemoteControlState::new(),
            events: Mutex::new(Vec::new()),
            bridge_task: Mutex::new(None),
            feishu_ws: Mutex::new(FeishuWsState::default()),
            telegram: Mutex::new(TelegramState::default()),
            wechat: Mutex::new(WechatState::default()),
            wechat_onboard: Mutex::new(None),
            shutdown_tx: Mutex::new(shutdown_tx),
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
                target: "codex_remote::event",
                event_kind = kind,
                message = %message,
                "app event"
            ),
            "warn" => tracing::warn!(
                target: "codex_remote::event",
                event_kind = kind,
                message = %message,
                "app event"
            ),
            _ => tracing::info!(
                target: "codex_remote::event",
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
            let _ = tx.send(());
            true
        } else {
            false
        }
    }
}
