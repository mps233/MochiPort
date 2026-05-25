use std::path::PathBuf;

use anyhow::Context;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AppConfig {
    pub bind: String,
    pub state_path: PathBuf,
    pub feishu: FeishuConfig,
    pub bridge: BridgeConfig,
    pub shim: ShimConfig,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ShimConfig {
    pub bin_dir: PathBuf,
    pub real_codex_path: Option<PathBuf>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:3847".to_string(),
            state_path: PathBuf::from("codex-remote-state.json"),
            feishu: FeishuConfig::default(),
            bridge: BridgeConfig::default(),
            shim: ShimConfig::default(),
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

impl Default for ShimConfig {
    fn default() -> Self {
        Self {
            bin_dir: default_shim_bin_dir(),
            real_codex_path: None,
        }
    }
}

impl AppConfig {
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

fn default_shim_bin_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("codex-remote")
        .join("bin")
}
