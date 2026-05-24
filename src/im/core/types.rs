use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImChannelKind {
    Wechat,
    Feishu,
    Telegram,
}

impl ImChannelKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Wechat => "wechat",
            Self::Feishu => "feishu",
            Self::Telegram => "telegram",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImChatType {
    Direct,
    Group,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboundAttachment {
    pub kind: String,
    pub name: Option<String>,
    pub mime_type: Option<String>,
    pub text_hint: Option<String>,
    pub local_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboundMessage {
    pub channel: ImChannelKind,
    pub account_id: String,
    pub sender_id: String,
    pub chat_id: String,
    pub chat_type: ImChatType,
    pub message_id: String,
    pub text: String,
    pub mentioned: bool,
    #[serde(default)]
    pub attachments: Vec<InboundAttachment>,
}

impl InboundMessage {
    pub fn conversation_key(&self) -> String {
        format!(
            "{}:{}:{}",
            self.channel.as_str(),
            self.account_id,
            self.chat_id
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboundAttachment {
    pub kind: String,
    pub name: Option<String>,
    pub mime_type: Option<String>,
    pub local_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImDesktopAttachmentInput {
    pub kind: String,
    pub name: Option<String>,
    pub mime_type: Option<String>,
    pub local_path: Option<String>,
    pub data_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImCodexUserMessageInput {
    pub thread_id: String,
    pub text: Option<String>,
    #[serde(default)]
    pub attachments: Vec<ImDesktopAttachmentInput>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboundMessage {
    pub channel: ImChannelKind,
    pub account_id: String,
    pub chat_id: String,
    pub reply_to_message_id: Option<String>,
    pub text: String,
    #[serde(default)]
    pub attachments: Vec<OutboundAttachment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingApprovalRequest {
    pub request_id: String,
    pub request_kind: String,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ImThreadRouteMode {
    NoActiveThread,
    ActiveThreadBusy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImPendingThreadRoute {
    pub request_id: String,
    pub mode: ImThreadRouteMode,
    pub workspace: Option<String>,
    pub current_thread_id: Option<String>,
    pub current_message_id: Option<String>,
    pub selected_action: Option<String>,
    pub current_page: usize,
    pub current_cursor: Option<String>,
    pub next_cursor: Option<String>,
    pub prev_cursors: Vec<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImThreadListEntry {
    pub thread_id: String,
    pub title: String,
    pub summary: Option<String>,
    pub updated_at_secs: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImTurnTerminalState {
    Completed,
    Interrupted,
    Failed,
}

impl ImTurnTerminalState {
    pub fn marker_text(self) -> &'static str {
        match self {
            Self::Completed => "状态：已完成 ✅",
            Self::Interrupted => "状态：已中断 ⏹️",
            Self::Failed => "状态：执行失败 ❌",
        }
    }
}
