use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatType {
    Direct,
    Group,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboundAttachment {
    pub kind: String,
    pub name: Option<String>,
    pub mime_type: Option<String>,
    pub text_hint: Option<String>,
    pub local_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ThreadRouteDirection {
    Prev,
    Next,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum InboundAction {
    ThreadRouteChoice {
        request_id: String,
        action: String,
    },
    ThreadRouteCreateSubmit {
        request_id: String,
        cwd_choice: Option<String>,
        cwd_custom: Option<String>,
        model: Option<String>,
        effort: Option<String>,
    },
    ThreadRouteCreateDefault {
        request_id: String,
    },
    ThreadRouteResumeSelected {
        request_id: String,
        thread_id: String,
    },
    ThreadRouteListPage {
        request_id: String,
        direction: ThreadRouteDirection,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboundMessage {
    pub account_id: String,
    pub sender_id: String,
    pub chat_id: String,
    pub chat_type: ChatType,
    pub message_id: String,
    pub text: String,
    pub mentioned: bool,
    pub approval_request_key: Option<String>,
    #[serde(default)]
    pub action: Option<InboundAction>,
    #[serde(default)]
    pub card_message_id: Option<String>,
    #[serde(default)]
    pub attachments: Vec<InboundAttachment>,
}

impl InboundMessage {
    pub fn conversation_key(&self) -> String {
        format!("feishu:{}:{}", self.account_id, self.chat_id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventRecord {
    pub at_ms: u128,
    pub level: String,
    pub kind: String,
    pub message: String,
}

pub fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|v| v.as_millis())
        .unwrap_or_default()
}
