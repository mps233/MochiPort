use std::{net::SocketAddr, path::PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

const DEFAULT_BIND: &str = "127.0.0.1:3847";
const LEGACY_DEFAULT_BIND: &str = "127.0.0.1:8000";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AppConfig {
    pub bind: String,
    pub state_path: PathBuf,
    pub feishu: FeishuConfig,
    pub bridge: BridgeConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FeishuConfig {
    pub app_id: String,
    pub app_secret: String,
    pub mention_only: bool,
    pub allowed_open_ids: Vec<String>,
    pub allowed_chat_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BridgeConfig {
    pub enabled: bool,
    pub account_id: String,
    pub send_streaming: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            bind: DEFAULT_BIND.to_string(),
            state_path: PathBuf::from("codex-remote-state.json"),
            feishu: FeishuConfig::default(),
            bridge: BridgeConfig::default(),
        }
    }
}

impl Default for FeishuConfig {
    fn default() -> Self {
        Self {
            app_id: String::new(),
            app_secret: String::new(),
            mention_only: true,
            allowed_open_ids: Vec::new(),
            allowed_chat_ids: Vec::new(),
        }
    }
}

impl Default for BridgeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            account_id: "default".to_string(),
            send_streaming: true,
        }
    }
}

impl AppConfig {
    pub fn apply_platform_defaults(&mut self) -> bool {
        if self.bind == LEGACY_DEFAULT_BIND {
            self.bind = DEFAULT_BIND.to_string();
            return true;
        }

        false
    }

    pub fn remote_control_base_url(&self) -> String {
        let host_port = self
            .bind
            .parse::<SocketAddr>()
            .ok()
            .map(|addr| {
                let host = if addr.ip().is_loopback() || addr.ip().is_unspecified() {
                    "127.0.0.1".to_string()
                } else {
                    let host = addr.ip().to_string();
                    if host.contains(':') {
                        format!("[{host}]")
                    } else {
                        host
                    }
                };
                format!("{host}:{}", addr.port())
            })
            .unwrap_or_else(|| self.bind.clone());
        format!("http://{host_port}/backend-api")
    }

    pub fn load_or_default(path: &PathBuf) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read config {}", path.display()))?;
        toml::from_str(&raw).with_context(|| format!("failed to parse config {}", path.display()))
    }

    pub fn save(&self, path: &PathBuf) -> anyhow::Result<()> {
        let raw = toml::to_string_pretty(self)?;
        std::fs::write(path, raw)
            .with_context(|| format!("failed to write config {}", path.display()))
    }
}
