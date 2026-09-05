use std::{
    collections::HashSet,
    io::Write,
    net::SocketAddr,
    path::{Path, PathBuf},
};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::types::ImPlatformKind;

const DEFAULT_BIND: &str = "127.0.0.1:3847";
const LEGACY_DEFAULT_BIND: &str = "127.0.0.1:8000";

pub(crate) fn normalize_config_paths(config: &mut AppConfig, config_path: &Path) {
    let base = config_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    if config.state_path.is_relative() {
        config.state_path = base.join(&config.state_path);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AppConfig {
    pub bind: String,
    pub local_connection_mode: LocalConnectionMode,
    pub outbound_proxy: OutboundProxyConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    #[serde(default)]
    pub state_path: PathBuf,
    pub logging: LoggingConfig,
    #[serde(default, rename = "feishu", skip_serializing)]
    legacy_feishu: FeishuConfig,
    #[serde(default, rename = "telegram", skip_serializing)]
    legacy_telegram: TelegramConfig,
    #[serde(default, rename = "wechat", skip_serializing)]
    legacy_wechat: WechatConfig,
    #[serde(default, rename = "wecom", skip_serializing)]
    legacy_wecom: WecomConfig,
    pub feishu_accounts: Vec<FeishuConfig>,
    pub telegram_accounts: Vec<TelegramConfig>,
    pub wechat_accounts: Vec<WechatConfig>,
    pub wecom_accounts: Vec<WecomConfig>,
    pub bridge: BridgeConfig,
    pub ai_gateway: crate::ai_gateway::config::AiGatewayConfig,
    #[serde(skip)]
    pending_v2_save: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LocalConnectionMode {
    #[default]
    Standard,
    VpnCompatible,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OutboundProxyMode {
    #[default]
    System,
    Direct,
    Custom,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct OutboundProxyConfig {
    pub mode: OutboundProxyMode,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FeishuConfig {
    pub enabled: bool,
    pub account_id: String,
    pub app_id: String,
    pub app_secret: String,
    pub display_name: String,
    pub mention_only: bool,
    pub allowed_open_ids: Vec<String>,
    pub allowed_chat_ids: Vec<String>,
}

/// Telegram 账号的回复颗粒度；档位按"消息包含哪些成分与形态"划分，
/// 与 GUI 的「回复颗粒度」选择和 Telegram 内的 /回复 命令共享。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TelegramReplyGranularity {
    /// 只发过程文本（助手说明）和最终结果；工具执行、文件修改等一律静默。
    Summary,
    /// 现有默认行为：所有信息合并进一条聚合气泡原地更新。
    #[default]
    Standard,
    /// 所有信息逐条独立发送：每条工具执行、文件修改各自一条消息，不合并更新。
    Full,
}

impl TelegramReplyGranularity {
    pub fn as_str(self) -> &'static str {
        match self {
            TelegramReplyGranularity::Summary => "summary",
            TelegramReplyGranularity::Standard => "standard",
            TelegramReplyGranularity::Full => "full",
        }
    }

    /// 接受英文值与中文别名；Telegram 命令与管理 API 共用。
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "summary" | "quiet" | "摘要" | "摘要回复" => Some(TelegramReplyGranularity::Summary),
            "standard" | "normal" | "标准" | "标准回复" => Some(TelegramReplyGranularity::Standard),
            "full" | "all" | "完整" | "完整回复" => Some(TelegramReplyGranularity::Full),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TelegramConfig {
    pub enabled: bool,
    pub account_id: String,
    #[serde(alias = "bot_token")]
    pub bot_token: String,
    pub display_name: String,
    #[serde(alias = "mention_only")]
    pub mention_only: bool,
    #[serde(alias = "allowed_chat_ids")]
    pub allowed_chat_ids: Vec<String>,
    pub project_groups: Vec<TelegramProjectGroupConfig>,
    pub reply_granularity: TelegramReplyGranularity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct TelegramProjectGroupConfig {
    pub chat_id: String,
    pub project_name: String,
    pub cwd: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WechatConfig {
    pub enabled: bool,
    pub account_id: String,
    pub bot_token: String,
    pub display_name: String,
    pub base_url: String,
    pub user_id: String,
    pub bot_type: String,
    pub allowed_user_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WecomConfig {
    pub enabled: bool,
    pub account_id: String,
    pub bot_id: String,
    pub secret: String,
    pub display_name: String,
    pub websocket_url: String,
    pub allowed_user_ids: Vec<String>,
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
pub struct LoggingConfig {
    pub diagnostic: bool,
    pub max_mb: u64,
    pub retention_days: u64,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            bind: DEFAULT_BIND.to_string(),
            local_connection_mode: LocalConnectionMode::default(),
            outbound_proxy: OutboundProxyConfig::default(),
            language: None,
            theme: None,
            state_path: PathBuf::from("mochiport-state.json"),
            logging: LoggingConfig::default(),
            legacy_feishu: FeishuConfig::default(),
            legacy_telegram: TelegramConfig::default(),
            legacy_wechat: WechatConfig::default(),
            legacy_wecom: WecomConfig::default(),
            feishu_accounts: Vec::new(),
            telegram_accounts: Vec::new(),
            wechat_accounts: Vec::new(),
            wecom_accounts: Vec::new(),
            bridge: BridgeConfig::default(),
            ai_gateway: crate::ai_gateway::config::AiGatewayConfig::default(),
            pending_v2_save: false,
        }
    }
}

impl Default for FeishuConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            account_id: String::new(),
            app_id: String::new(),
            app_secret: String::new(),
            display_name: String::new(),
            mention_only: true,
            allowed_open_ids: Vec::new(),
            allowed_chat_ids: Vec::new(),
        }
    }
}

impl Default for TelegramConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            account_id: String::new(),
            bot_token: String::new(),
            display_name: String::new(),
            mention_only: false,
            allowed_chat_ids: Vec::new(),
            project_groups: Vec::new(),
            reply_granularity: TelegramReplyGranularity::default(),
        }
    }
}

impl Default for WechatConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            account_id: "wechat".to_string(),
            bot_token: String::new(),
            display_name: String::new(),
            base_url: String::new(),
            user_id: String::new(),
            bot_type: "3".to_string(),
            allowed_user_ids: Vec::new(),
        }
    }
}

impl Default for WecomConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            account_id: "wecom".to_string(),
            bot_id: String::new(),
            secret: String::new(),
            display_name: "企业微信机器人".to_string(),
            websocket_url: "wss://openws.work.weixin.qq.com".to_string(),
            allowed_user_ids: Vec::new(),
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

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            diagnostic: cfg!(debug_assertions),
            max_mb: 20,
            retention_days: 7,
        }
    }
}

impl AppConfig {
    pub fn apply_platform_defaults(&mut self) -> bool {
        let mut changed = false;
        if self.bind == LEGACY_DEFAULT_BIND {
            self.bind = DEFAULT_BIND.to_string();
            changed = true;
        }

        if self.migrate_legacy_im_accounts() || std::mem::take(&mut self.pending_v2_save) {
            changed = true;
        }

        changed
    }

    pub fn remote_control_base_url(&self) -> String {
        self.remote_control_base_url_for_mode(self.local_connection_mode)
    }

    pub fn local_listen_port(&self) -> Option<u16> {
        self.bind
            .parse::<SocketAddr>()
            .ok()
            .map(|address| address.port())
    }

    pub fn remote_control_base_url_for_mode(&self, mode: LocalConnectionMode) -> String {
        let host_port = self
            .bind
            .parse::<SocketAddr>()
            .ok()
            .map(|addr| {
                let host = if addr.ip().is_loopback() || addr.ip().is_unspecified() {
                    match mode {
                        LocalConnectionMode::Standard => "127.0.0.1".to_string(),
                        LocalConnectionMode::VpnCompatible => "localhost".to_string(),
                    }
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
        let mut document: toml::Value = toml::from_str(&raw)
            .with_context(|| format!("failed to parse config {}", path.display()))?;
        let legacy_fields_present = document.as_table().is_some_and(|table| {
            ["feishu", "telegram", "wechat", "wecom"]
                .iter()
                .any(|key| table.contains_key(*key))
        });
        let skipped = take_unsupported_providers(&mut document);
        if !skipped.is_empty() {
            tracing::warn!(
                count = skipped.len(),
                "unsupported gateway channels skipped; original configuration retained on disk"
            );
        }
        let mut config: Self = document
            .try_into()
            .with_context(|| format!("failed to parse config {}", path.display()))?;
        let migrated = config.migrate_legacy_im_accounts();
        config.pending_v2_save = legacy_fields_present || migrated;
        Ok(config)
    }

    pub fn save(&self, path: &PathBuf) -> anyhow::Result<()> {
        let mut document = toml::Value::try_from(self)?;
        // The GUI only edits supported providers. Preserve unknown entries from
        // disk when it saves, including fields introduced by a newer version.
        if path.exists() {
            let existing = std::fs::read_to_string(path)
                .with_context(|| format!("failed to read config {}", path.display()))?;
            let mut existing: toml::Value = toml::from_str(&existing)
                .with_context(|| format!("failed to parse config {}", path.display()))?;
            let unsupported = take_unsupported_providers(&mut existing);
            if !unsupported.is_empty() {
                document["aiGateway"]["providers"]
                    .as_array_mut()
                    .expect("serialized providers must be an array")
                    .extend(unsupported);
            }
        }
        let raw = toml::to_string_pretty(&document)?;
        let parent = path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config directory {}", parent.display()))?;

        let mut temporary = tempfile::Builder::new()
            .prefix(".mochiport-config.")
            .tempfile_in(parent)
            .with_context(|| format!("failed to prepare config file {}", path.display()))?;
        temporary
            .write_all(raw.as_bytes())
            .and_then(|_| temporary.as_file().sync_all())
            .with_context(|| format!("failed to write config {}", path.display()))?;
        temporary
            .persist(path)
            .map_err(|error| error.error)
            .with_context(|| format!("failed to replace config {}", path.display()))?;
        Ok(())
    }

    fn migrate_legacy_im_accounts(&mut self) -> bool {
        let mut changed = false;
        if self.feishu_accounts.is_empty() && self.legacy_feishu != FeishuConfig::default() {
            let mut account = self.legacy_feishu.clone();
            if account.account_id.trim().is_empty() {
                account.account_id = non_empty(&self.bridge.account_id)
                    .or_else(|| non_empty(&account.app_id))
                    .unwrap_or_else(|| "default".to_string());
            }
            self.feishu_accounts.push(account);
            changed = true;
        }
        if self.telegram_accounts.is_empty() && self.legacy_telegram != TelegramConfig::default() {
            let mut account = self.legacy_telegram.clone();
            if account.account_id.trim().is_empty() {
                account.account_id = "telegram".to_string();
            }
            self.telegram_accounts.push(account);
            changed = true;
        }
        if self.wechat_accounts.is_empty() && self.legacy_wechat != WechatConfig::default() {
            let mut account = self.legacy_wechat.clone();
            if account.account_id.trim().is_empty() {
                account.account_id = "wechat".to_string();
            }
            self.wechat_accounts.push(account);
            changed = true;
        }
        if self.wecom_accounts.is_empty() && self.legacy_wecom != WecomConfig::default() {
            let mut account = self.legacy_wecom.clone();
            if account.account_id.trim().is_empty() {
                account.account_id = "wecom".to_string();
            }
            self.wecom_accounts.push(account);
            changed = true;
        }
        self.legacy_feishu = FeishuConfig::default();
        self.legacy_telegram = TelegramConfig::default();
        self.legacy_wechat = WechatConfig::default();
        self.legacy_wecom = WecomConfig::default();
        changed |= self.normalize_im_account_ids();
        changed
    }

    /// Keep the account identity used by configuration, management APIs, and
    /// the running bridge identical. Older array-shaped configurations could
    /// leave an empty or padded `accountId`; that value is especially
    /// dangerous because the API clients have platform fallbacks while the
    /// registry uses the raw string as its map key.
    fn normalize_im_account_ids(&mut self) -> bool {
        let feishu_fallback = non_empty(&self.bridge.account_id);
        let mut changed = false;
        changed |= normalize_account_ids(
            &mut self.feishu_accounts,
            |account| {
                feishu_fallback
                    .clone()
                    .or_else(|| non_empty(&account.app_id))
                    .unwrap_or_else(|| "default".to_string())
            },
            |account| account.account_id.as_str(),
            |account, value| account.account_id = value,
        );
        changed |= normalize_account_ids(
            &mut self.telegram_accounts,
            |_| "telegram".to_string(),
            |account| account.account_id.as_str(),
            |account, value| account.account_id = value,
        );
        changed |= normalize_account_ids(
            &mut self.wechat_accounts,
            |_| "wechat".to_string(),
            |account| account.account_id.as_str(),
            |account, value| account.account_id = value,
        );
        changed |= normalize_account_ids(
            &mut self.wecom_accounts,
            |_| "wecom".to_string(),
            |account| account.account_id.as_str(),
            |account, value| account.account_id = value,
        );
        changed
    }

    pub fn feishu_account(&self, account_id: &str) -> Option<FeishuConfig> {
        find_account(&self.feishu_accounts, account_id, |account| {
            account.account_id.as_str()
        })
    }

    pub fn telegram_account(&self, account_id: &str) -> Option<TelegramConfig> {
        find_account(&self.telegram_accounts, account_id, |account| {
            account.account_id.as_str()
        })
    }

    /// 事件分发用的热查询：按账号返回回复颗粒度，未知账号回落到默认档。
    pub fn telegram_reply_granularity(&self, account_id: &str) -> TelegramReplyGranularity {
        self.telegram_accounts
            .iter()
            .find(|account| account.account_id == account_id)
            .map(|account| account.reply_granularity)
            .unwrap_or_default()
    }

    pub fn wechat_account(&self, account_id: &str) -> Option<WechatConfig> {
        find_account(&self.wechat_accounts, account_id, |account| {
            account.account_id.as_str()
        })
    }

    pub fn wecom_account(&self, account_id: &str) -> Option<WecomConfig> {
        find_account(&self.wecom_accounts, account_id, |account| {
            account.account_id.as_str()
        })
    }

    pub fn has_im_account(&self, platform: &str, account_id: &str) -> bool {
        let account_id = account_id.trim();
        if account_id.is_empty() {
            return false;
        }
        match ImPlatformKind::from_key(platform) {
            Some(ImPlatformKind::Feishu) => self
                .feishu_accounts
                .iter()
                .any(|account| account.account_id.trim() == account_id),
            Some(ImPlatformKind::Telegram) => self
                .telegram_accounts
                .iter()
                .any(|account| account.account_id.trim() == account_id),
            Some(ImPlatformKind::Wechat) => self
                .wechat_accounts
                .iter()
                .any(|account| account.account_id.trim() == account_id),
            Some(ImPlatformKind::Wecom) => self
                .wecom_accounts
                .iter()
                .any(|account| account.account_id.trim() == account_id),
            _ => false,
        }
    }

    pub fn upsert_feishu_account(&mut self, account: FeishuConfig) {
        upsert_account(&mut self.feishu_accounts, account, |account| {
            account.account_id.as_str()
        });
    }

    pub fn upsert_telegram_account(&mut self, account: TelegramConfig) {
        upsert_account(&mut self.telegram_accounts, account, |account| {
            account.account_id.as_str()
        });
    }

    pub fn upsert_wechat_account(&mut self, account: WechatConfig) {
        upsert_account(&mut self.wechat_accounts, account, |account| {
            account.account_id.as_str()
        });
    }

    pub fn upsert_wecom_account(&mut self, account: WecomConfig) {
        upsert_account(&mut self.wecom_accounts, account, |account| {
            account.account_id.as_str()
        });
    }

    pub fn remove_im_account(&mut self, platform: &str, account_id: &str) -> bool {
        let account_id = account_id.trim();
        if account_id.is_empty() {
            return false;
        }
        match ImPlatformKind::from_key(platform) {
            Some(ImPlatformKind::Feishu) => {
                remove_account(&mut self.feishu_accounts, account_id, |account| {
                    account.account_id.as_str()
                })
            }
            Some(ImPlatformKind::Telegram) => {
                remove_account(&mut self.telegram_accounts, account_id, |account| {
                    account.account_id.as_str()
                })
            }
            Some(ImPlatformKind::Wechat) => {
                remove_account(&mut self.wechat_accounts, account_id, |account| {
                    account.account_id.as_str()
                })
            }
            Some(ImPlatformKind::Wecom) => {
                remove_account(&mut self.wecom_accounts, account_id, |account| {
                    account.account_id.as_str()
                })
            }
            _ => false,
        }
    }

    pub fn set_im_account_enabled(
        &mut self,
        platform: &str,
        account_id: &str,
        enabled: bool,
    ) -> bool {
        let account_id = account_id.trim();
        if account_id.is_empty() {
            return false;
        }
        match ImPlatformKind::from_key(platform) {
            Some(ImPlatformKind::Feishu) => set_account_enabled(
                &mut self.feishu_accounts,
                account_id,
                enabled,
                |account| account.account_id.as_str(),
                |account| &mut account.enabled,
            ),
            Some(ImPlatformKind::Telegram) => set_account_enabled(
                &mut self.telegram_accounts,
                account_id,
                enabled,
                |account| account.account_id.as_str(),
                |account| &mut account.enabled,
            ),
            Some(ImPlatformKind::Wechat) => set_account_enabled(
                &mut self.wechat_accounts,
                account_id,
                enabled,
                |account| account.account_id.as_str(),
                |account| &mut account.enabled,
            ),
            Some(ImPlatformKind::Wecom) => set_account_enabled(
                &mut self.wecom_accounts,
                account_id,
                enabled,
                |account| account.account_id.as_str(),
                |account| &mut account.enabled,
            ),
            _ => false,
        }
    }

    pub fn ensure_telegram_allowed_chat_id(
        &mut self,
        account_id: &str,
        chat_id: &str,
    ) -> TelegramChatAllowResult {
        let account_id = account_id.trim();
        let chat_id = chat_id.trim();
        if account_id.is_empty() || chat_id.is_empty() {
            return TelegramChatAllowResult::AccountNotFound;
        }
        let Some(account) = self.telegram_accounts.iter_mut().find(|account| {
            account.account_id.trim() == account_id
                || (account.account_id.trim().is_empty() && account_id == "telegram")
        }) else {
            return TelegramChatAllowResult::AccountNotFound;
        };
        ensure_telegram_chat_id_on_account(account, chat_id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TelegramChatAllowResult {
    Allowed,
    Bound,
    Denied,
    AccountNotFound,
}

impl TelegramChatAllowResult {
    pub fn should_save(self) -> bool {
        matches!(self, Self::Bound)
    }
}

fn ensure_telegram_chat_id_on_account(
    account: &mut TelegramConfig,
    chat_id: &str,
) -> TelegramChatAllowResult {
    if account
        .allowed_chat_ids
        .iter()
        .any(|allowed| allowed.trim() == chat_id)
    {
        return TelegramChatAllowResult::Allowed;
    }
    if !account.allowed_chat_ids.is_empty() {
        return TelegramChatAllowResult::Denied;
    }
    account.allowed_chat_ids.push(chat_id.to_string());
    TelegramChatAllowResult::Bound
}

impl FeishuConfig {
    pub fn is_configured(&self) -> bool {
        !self.app_id.trim().is_empty() && !self.app_secret.trim().is_empty()
    }

    pub fn is_active(&self) -> bool {
        self.enabled && self.is_configured()
    }
}

impl TelegramConfig {
    pub fn is_configured(&self) -> bool {
        !self.bot_token.trim().is_empty()
    }

    pub fn project_group_for_chat(&self, chat_id: &str) -> Option<TelegramProjectGroupConfig> {
        let chat_id = chat_id.trim();
        self.project_groups
            .iter()
            .find(|group| group.chat_id.trim() == chat_id && !group.cwd.trim().is_empty())
            .cloned()
    }

    pub fn is_active(&self) -> bool {
        self.enabled && self.is_configured()
    }
}

impl WechatConfig {
    pub fn is_configured(&self) -> bool {
        !self.bot_token.trim().is_empty()
    }

    pub fn is_active(&self) -> bool {
        self.enabled && self.is_configured()
    }
}

impl WecomConfig {
    pub fn is_configured(&self) -> bool {
        !self.bot_id.trim().is_empty() && !self.secret.trim().is_empty()
    }

    pub fn is_active(&self) -> bool {
        self.enabled && self.is_configured()
    }
}

fn upsert_account<T>(accounts: &mut Vec<T>, account: T, account_id: impl Fn(&T) -> &str) {
    let id = account_id(&account).trim().to_string();
    if let Some(existing) = accounts
        .iter_mut()
        .find(|existing| account_id(existing).trim() == id)
    {
        *existing = account;
    } else {
        accounts.push(account);
    }
}

fn remove_account<T>(
    accounts: &mut Vec<T>,
    account_id: &str,
    get_account_id: impl Fn(&T) -> &str,
) -> bool {
    let before = accounts.len();
    accounts.retain(|account| get_account_id(account).trim() != account_id);
    accounts.len() != before
}

fn set_account_enabled<T>(
    accounts: &mut [T],
    account_id: &str,
    enabled: bool,
    get_account_id: impl Fn(&T) -> &str,
    get_enabled: impl Fn(&mut T) -> &mut bool,
) -> bool {
    let Some(account) = accounts
        .iter_mut()
        .find(|account| get_account_id(account).trim() == account_id)
    else {
        return false;
    };
    *get_enabled(account) = enabled;
    true
}

fn find_account<T: Clone>(
    accounts: &[T],
    account_id: &str,
    get_account_id: impl Fn(&T) -> &str,
) -> Option<T> {
    let account_id = account_id.trim();
    accounts
        .iter()
        .find(|account| get_account_id(account).trim() == account_id)
        .cloned()
}

fn take_unsupported_providers(document: &mut toml::Value) -> Vec<toml::Value> {
    let Some(providers) = document
        .get_mut("aiGateway")
        .and_then(|gateway| gateway.get_mut("providers"))
        .and_then(toml::Value::as_array_mut)
    else {
        return Vec::new();
    };
    let mut unsupported = Vec::new();
    providers.retain(|provider| {
        let unknown = provider
            .get("providerType")
            .and_then(toml::Value::as_str)
            .is_some_and(|kind| {
                serde_json::from_value::<crate::ai_gateway::config::ProviderType>(
                    serde_json::Value::String(kind.to_owned()),
                )
                .is_err()
            });
        if unknown {
            unsupported.push(provider.clone());
        }
        !unknown
    });
    unsupported
}

fn non_empty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn normalize_account_ids<T>(
    accounts: &mut [T],
    fallback: impl Fn(&T) -> String,
    get_account_id: impl Fn(&T) -> &str,
    set_account_id: impl Fn(&mut T, String),
) -> bool {
    // Reserve every explicit ID first. This prevents an earlier generated
    // fallback from colliding with an explicit ID that appears later in the
    // file.
    let mut used = accounts
        .iter()
        .filter_map(|account| non_empty(get_account_id(account)))
        .collect::<HashSet<_>>();
    let mut changed = false;
    for account in accounts.iter_mut() {
        let raw = get_account_id(account).to_string();
        let trimmed = raw.trim().to_string();
        let normalized = if trimmed.is_empty() {
            let candidate = fallback(account);
            unique_account_id(candidate, &used)
        } else {
            trimmed
        };
        if raw != normalized {
            set_account_id(account, normalized.clone());
            changed = true;
        }
        used.insert(normalized);
    }
    changed
}

fn unique_account_id(candidate: String, used: &HashSet<String>) -> String {
    if !used.contains(&candidate) {
        return candidate;
    }
    for suffix in 2.. {
        let next = format!("{candidate}-{suffix}");
        if !used.contains(&next) {
            return next;
        }
    }
    unreachable!("account id suffix search is bounded by the address space")
}

#[cfg(test)]
mod tests {
    use super::{
        AppConfig, FeishuConfig, OutboundProxyMode, TelegramChatAllowResult, TelegramConfig,
        TelegramReplyGranularity,
    };

    #[test]
    fn telegram_account_without_granularity_defaults_to_full() {
        let payload = r#"{
            "enabled": true,
            "accountId": "tg-1",
            "botToken": "123:abc"
        }"#;
        let account: TelegramConfig = serde_json::from_str(payload).unwrap();
        assert_eq!(account.reply_granularity, TelegramReplyGranularity::Standard);
    }

    #[test]
    fn reply_granularity_parse_accepts_english_and_chinese_aliases() {
        assert_eq!(
            TelegramReplyGranularity::parse("summary"),
            Some(TelegramReplyGranularity::Summary)
        );
        assert_eq!(
            TelegramReplyGranularity::parse("标准回复"),
            Some(TelegramReplyGranularity::Standard)
        );
        assert_eq!(
            TelegramReplyGranularity::parse("完整回复"),
            Some(TelegramReplyGranularity::Full)
        );
        assert_eq!(TelegramReplyGranularity::parse("nonsense"), None);
    }

    #[test]
    fn unsupported_channels_survive_config_save_without_entering_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let raw = r#"
[[aiGateway.providers]]
name = "future"
providerType = "gemini_generate_content"
enabled = true
apiKey = "test-key"
models = ["gemini-test"]
[aiGateway.providers.futureOptions]
flag = true
[[aiGateway.providers]]
name = "working"
providerType = "open_ai_responses"
models = ["gpt-test"]
"#;
        std::fs::write(&path, raw).unwrap();
        let original: toml::Value = toml::from_str(raw).unwrap();
        let mut config = AppConfig::load_or_default(&path).unwrap();
        assert_eq!(config.ai_gateway.providers.len(), 1);
        assert_eq!(config.ai_gateway.providers[0].name, "working");
        assert!(config.ai_gateway.select_provider("gemini-test").is_none());
        config.ai_gateway.providers[0].name = "edited".into();
        for _ in 0..2 {
            config.save(&path).unwrap();
            let saved: toml::Value =
                toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            let providers = saved["aiGateway"]["providers"].as_array().unwrap();
            assert_eq!(providers.len(), 2);
            assert_eq!(providers[0]["name"].as_str(), Some("edited"));
            assert_eq!(providers[1], original["aiGateway"]["providers"][0]);
            config = AppConfig::load_or_default(&path).unwrap();
            assert_eq!(config.ai_gateway.providers.len(), 1);
        }
    }

    #[test]
    fn unsupported_channels_do_not_hide_invalid_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        for raw in [
            "[[aiGateway.providers]]\nproviderType = 123",
            "[[aiGateway.providers]]\nproviderType = 'open_ai_responses'\nenabled = 'bad'",
            "[[aiGateway.providers]]\nproviderType = 'gemini_generate_content'\ninvalid = [",
        ] {
            std::fs::write(&path, raw).unwrap();
            assert!(AppConfig::load_or_default(&path).is_err());
        }
    }

    #[test]
    fn save_atomically_replaces_an_existing_config() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("config.toml");
        let mut original = AppConfig::default();
        original.language = Some("en-US".to_string());
        original.save(&path).expect("save original config");

        let mut replacement = original.clone();
        replacement.language = Some("zh-CN".to_string());
        replacement.save(&path).expect("replace config");

        let loaded = AppConfig::load_or_default(&path).expect("load replacement config");
        assert_eq!(loaded.language.as_deref(), Some("zh-CN"));
        let temporary_files = std::fs::read_dir(temp.path())
            .expect("read config directory")
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".mochiport-config.")
            })
            .count();
        assert_eq!(temporary_files, 0);
    }

    #[test]
    fn missing_outbound_proxy_config_defaults_to_system() {
        let config: AppConfig = toml::from_str("bind = '127.0.0.1:3847'").unwrap();
        assert_eq!(config.outbound_proxy.mode, OutboundProxyMode::System);
        assert!(config.outbound_proxy.url.is_empty());
    }

    #[test]
    fn legacy_fast_startup_setting_is_ignored_and_not_written_back() {
        let config: AppConfig =
            toml::from_str("bind = '127.0.0.1:3847'\ncodexAppFastStartup = true\n").unwrap();
        let serialized = toml::to_string(&config).unwrap();
        assert!(!serialized.contains("codexAppFastStartup"));
    }

    #[test]
    fn telegram_empty_allowlist_binds_first_private_chat() {
        let mut config = AppConfig::default();
        config.telegram_accounts.push(TelegramConfig {
            account_id: "tg_1".to_string(),
            bot_token: "token".to_string(),
            ..TelegramConfig::default()
        });

        assert_eq!(
            config.ensure_telegram_allowed_chat_id("tg_1", "123"),
            TelegramChatAllowResult::Bound
        );
        assert_eq!(
            config.telegram_accounts[0].allowed_chat_ids,
            vec!["123".to_string()]
        );
        assert_eq!(
            config.ensure_telegram_allowed_chat_id("tg_1", "123"),
            TelegramChatAllowResult::Allowed
        );
        assert_eq!(
            config.ensure_telegram_allowed_chat_id("tg_1", "456"),
            TelegramChatAllowResult::Denied
        );
    }

    #[test]
    fn telegram_project_groups_round_trip_through_toml() {
        let mut config: AppConfig = toml::from_str(
            r#"
                [telegram]
                botToken = "token"
                projectGroups = [
                  { chatId = "-100", projectName = "MochiPort", cwd = "/tmp/mochiport" }
                ]
            "#,
        )
        .expect("project group config");
        assert!(config.migrate_legacy_im_accounts());
        let group = config
            .telegram_accounts
            .first()
            .expect("migrated telegram account")
            .project_group_for_chat(" -100 ")
            .expect("configured project group");
        assert_eq!(group.project_name, "MochiPort");
        assert_eq!(group.cwd, "/tmp/mochiport");
    }

    #[test]
    fn telegram_legacy_default_account_can_bind_first_chat() {
        let mut config: AppConfig = toml::from_str(
            r#"
                [telegram]
                botToken = "token"
            "#,
        )
        .expect("legacy telegram config");
        assert!(config.migrate_legacy_im_accounts());

        assert_eq!(
            config.ensure_telegram_allowed_chat_id("telegram", "123"),
            TelegramChatAllowResult::Bound
        );
        assert_eq!(
            config.telegram_accounts[0].allowed_chat_ids,
            vec!["123".to_string()]
        );
    }

    #[test]
    fn wecom_legacy_account_migrates_and_is_resolved() {
        let mut config: AppConfig = toml::from_str(
            r#"
                [wecom]
                botId = "bot-1"
                secret = "secret-1"
            "#,
        )
        .expect("legacy wecom config");
        assert!(config.migrate_legacy_im_accounts());
        let account = config.wecom_account("wecom").expect("wecom account");
        assert_eq!(account.bot_id, "bot-1");
        assert!(account.is_active());
    }

    #[test]
    fn feishu_legacy_account_keeps_the_same_id_after_migration() {
        let mut config: AppConfig = toml::from_str(
            r#"
                [feishu]
                appId = "cli-app"
                appSecret = "secret"

                [bridge]
                accountId = "legacy-bridge"
            "#,
        )
        .expect("legacy feishu config");
        assert!(config.migrate_legacy_im_accounts());
        assert!(config.has_im_account("feishu", "legacy-bridge"));
        assert!(config.set_im_account_enabled("feishu", "legacy-bridge", false));
        assert!(config.remove_im_account("feishu", "legacy-bridge"));
    }

    #[test]
    fn legacy_singletons_are_deserialize_only_and_migrate_once() {
        let mut config: AppConfig = toml::from_str(
            r#"
                [telegram]
                botToken = "legacy-token"
            "#,
        )
        .expect("legacy config");

        assert!(config.migrate_legacy_im_accounts());
        assert!(!config.migrate_legacy_im_accounts());
        assert_eq!(config.telegram_accounts.len(), 1);
        let serialized = toml::to_string(&config).expect("serialize v2 config");
        assert!(!serialized.contains("[telegram]"));
        assert!(serialized.contains("[[telegramAccounts]]"));
    }

    #[test]
    fn array_account_ids_are_normalized_before_runtime_use() {
        let mut config: AppConfig = toml::from_str(
            r#"
                [bridge]
                accountId = "bridge-account"

                [[feishuAccounts]]
                accountId = "  "
                appId = "cli-first"
                appSecret = "secret-first"

                [[feishuAccounts]]
                accountId = ""
                appId = "cli-second"
                appSecret = "secret-second"

                [[telegramAccounts]]
                accountId = "  "
                botToken = "token-first"

                [[telegramAccounts]]
                accountId = ""
                botToken = "token-second"

                [[wechatAccounts]]
                accountId = "  "
                botToken = "wechat-token"

                [[wecomAccounts]]
                accountId = ""
                botId = "wecom-bot"
                secret = "wecom-secret"
            "#,
        )
        .expect("array account config");

        assert!(config.apply_platform_defaults());
        assert_eq!(
            config
                .feishu_accounts
                .iter()
                .map(|account| account.account_id.as_str())
                .collect::<Vec<_>>(),
            ["bridge-account", "bridge-account-2"]
        );
        assert_eq!(
            config
                .telegram_accounts
                .iter()
                .map(|account| account.account_id.as_str())
                .collect::<Vec<_>>(),
            ["telegram", "telegram-2"]
        );
        assert_eq!(config.wechat_accounts[0].account_id, "wechat");
        assert_eq!(config.wecom_accounts[0].account_id, "wecom");
        assert!(!config.apply_platform_defaults());
    }

    #[test]
    fn generated_account_ids_skip_later_explicit_ids() {
        let mut config: AppConfig = toml::from_str(
            r#"
                [[telegramAccounts]]
                accountId = ""
                botToken = "token-first"

                [[telegramAccounts]]
                accountId = "telegram"
                botToken = "token-second"
            "#,
        )
        .expect("array account config");

        assert!(config.apply_platform_defaults());
        assert_eq!(config.telegram_accounts[0].account_id, "telegram-2");
        assert_eq!(config.telegram_accounts[1].account_id, "telegram");
    }

    #[test]
    fn normalized_array_account_ids_are_persisted_on_startup() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("config.toml");
        let mut source = AppConfig::default();
        source.bridge.account_id.clear();
        source.feishu_accounts = vec![FeishuConfig {
            account_id: String::new(),
            app_id: "feishu-app".to_string(),
            app_secret: "feishu-secret".to_string(),
            ..FeishuConfig::default()
        }];
        source.telegram_accounts = vec![TelegramConfig {
            account_id: "  ".to_string(),
            bot_token: "telegram-token".to_string(),
            ..TelegramConfig::default()
        }];
        source.save(&path).expect("save source config");
        let mut loaded = AppConfig::load_or_default(&path).expect("load config");

        assert_eq!(loaded.feishu_accounts[0].account_id, "feishu-app");
        assert_eq!(loaded.telegram_accounts[0].account_id, "telegram");
        assert!(loaded.apply_platform_defaults());
        loaded.save(&path).expect("persist normalized config");

        let persisted = std::fs::read_to_string(&path).expect("read persisted config");
        assert!(persisted.contains("accountId = \"feishu-app\""));
        assert!(persisted.contains("accountId = \"telegram\""));
        let mut reloaded = AppConfig::load_or_default(&path).expect("reload config");
        assert_eq!(reloaded.feishu_accounts[0].account_id, "feishu-app");
        assert_eq!(reloaded.telegram_accounts[0].account_id, "telegram");
        assert!(!reloaded.apply_platform_defaults());
    }
}
