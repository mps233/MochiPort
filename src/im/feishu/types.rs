use serde::{Deserialize, Serialize};
use std::time::Instant;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeishuSettings {
    pub app_id: Option<String>,
    pub app_secret: Option<String>,
    pub verification_token: Option<String>,
    pub encrypt_key: Option<String>,
    pub mention_only: Option<bool>,
    pub sync_tool_calls: Option<bool>,
    pub sync_reasoning: Option<bool>,
    pub sync_debug_events: Option<bool>,
}

impl Default for FeishuSettings {
    fn default() -> Self {
        Self {
            app_id: None,
            app_secret: None,
            verification_token: None,
            encrypt_key: None,
            mention_only: Some(true),
            sync_tool_calls: Some(false),
            sync_reasoning: Some(false),
            sync_debug_events: Some(false),
        }
    }
}

impl FeishuSettings {
    pub fn from_app_config(config: &crate::config::FeishuConfig) -> Self {
        Self {
            app_id: non_empty(config.app_id.clone()),
            app_secret: non_empty(config.app_secret.clone()),
            verification_token: None,
            encrypt_key: None,
            mention_only: Some(config.mention_only),
            sync_tool_calls: Some(false),
            sync_reasoning: Some(false),
            sync_debug_events: Some(false),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FeishuStreamingCardState {
    pub account_id: String,
    pub chat_id: String,
    pub receive_id_type: String,
    pub receive_id: String,
    pub kind: String,
    pub message_id: Option<String>,
    pub card_id: Option<String>,
    pub sequence: u64,
    pub text: String,
    pub sent_text: String,
    pub completed: bool,
    pub sending: bool,
    pub dirty: bool,
    pub last_sent_at: Option<Instant>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct FeishuUserInputOption {
    pub label: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct FeishuUserInputQuestion {
    pub id: String,
    pub header: String,
    pub question: String,
    pub is_other: bool,
    pub is_secret: bool,
    pub options: Option<Vec<FeishuUserInputOption>>,
}

fn non_empty(value: String) -> Option<String> {
    let value = value.trim().to_string();
    (!value.is_empty()).then_some(value)
}
