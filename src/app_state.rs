use std::{path::PathBuf, sync::Arc};

use tokio::{
    sync::{Mutex, broadcast, oneshot},
    task::JoinHandle,
};

use serde::Serialize;
use serde_json::Value;

use crate::{
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
    pub relay: RelayState,
    pub events: Mutex<Vec<EventRecord>>,
    pub bridge_task: Mutex<Option<JoinHandle<()>>>,
    pub feishu_ws: Mutex<FeishuWsState>,
}

pub struct RelayState {
    pub inner: Mutex<RelayInner>,
    pub notifications: broadcast::Sender<CodexNotification>,
}

pub struct RelayInner {
    pub public_ws_url: String,
    pub upstream_ws_url: String,
    pub running: bool,
    pub tui_connected: bool,
    pub upstream_connected: bool,
    pub current_thread_id: Option<String>,
    pub current_turn_id: Option<String>,
    pub last_error: Option<String>,
    pub inject_tx: Option<tokio::sync::mpsc::Sender<Value>>,
    pub connection_epoch: u64,
    pub disconnect_tx: Option<oneshot::Sender<()>>,
    pub pending:
        std::collections::HashMap<u64, tokio::sync::oneshot::Sender<anyhow::Result<Value>>>,
    pub client_request_methods: std::collections::HashMap<String, String>,
    pub client_request_thread_ids: std::collections::HashMap<String, String>,
}

impl RelayState {
    pub fn new(public_ws_url: String, upstream_ws_url: String) -> Self {
        let (notifications, _) = broadcast::channel(512);
        Self {
            inner: Mutex::new(RelayInner {
                public_ws_url,
                upstream_ws_url,
                running: false,
                tui_connected: false,
                upstream_connected: false,
                current_thread_id: None,
                current_turn_id: None,
                last_error: None,
                inject_tx: None,
                connection_epoch: 0,
                disconnect_tx: None,
                pending: std::collections::HashMap::new(),
                client_request_methods: std::collections::HashMap::new(),
                client_request_thread_ids: std::collections::HashMap::new(),
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

impl AppState {
    pub fn new(config_path: PathBuf, config: AppConfig) -> SharedState {
        let persisted = PersistedState::load(&config.state_path);
        let public_ws_url = format!("ws://{}", config.relay.public_ws);
        let upstream_ws_url = format!("ws://{}", config.relay.upstream_ws);
        Arc::new(Self {
            config_path,
            config: Mutex::new(config),
            persisted: Mutex::new(persisted),
            runtime: Mutex::new(RuntimeState::default()),
            relay: RelayState::new(public_ws_url, upstream_ws_url),
            events: Mutex::new(Vec::new()),
            bridge_task: Mutex::new(None),
            feishu_ws: Mutex::new(FeishuWsState::default()),
        })
    }

    pub async fn push_event(&self, level: &str, kind: &str, message: impl Into<String>) {
        let mut events = self.events.lock().await;
        events.push(EventRecord {
            at_ms: now_ms(),
            level: level.to_string(),
            kind: kind.to_string(),
            message: message.into(),
        });
        if events.len() > 300 {
            let drain = events.len().saturating_sub(300);
            events.drain(0..drain);
        }
    }
}
