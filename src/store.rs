use std::{collections::HashMap, path::Path};

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PersistedState {
    pub wechat: WechatPersistedState,
    pub im_thread_bindings: HashMap<String, String>,
    pub telegram_topic_binding_states: HashMap<String, TelegramTopicBindingState>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TelegramTopicBindingState {
    pub thread_id: String,
    /// The latest Codex title observed for this bound session.
    pub codex_title: String,
    pub topic_name: String,
    /// Names at the last successful two-way synchronization. Keeping both
    /// sides lets reconciliation tell which side changed since that point.
    pub last_synced_codex_title: String,
    pub last_synced_topic_name: String,
    pub codex_state: String,
    pub telegram_state: String,
    pub archived_at_ms: Option<u128>,
    pub missing_at_ms: Option<u128>,
    /// Bridge generation that last committed a Codex lifecycle state. This is
    /// reset when persisted bindings are restored after a daemon restart.
    pub lifecycle_generation: u64,
    /// Monotonic binding-local lifecycle intent. Unlike the bridge generation,
    /// this distinguishes archive/unarchive/rearchive races in one generation.
    pub lifecycle_revision: u64,
    pub last_checked_at_ms: u128,
}

impl Default for TelegramTopicBindingState {
    fn default() -> Self {
        Self {
            thread_id: String::new(),
            codex_title: String::new(),
            topic_name: String::new(),
            last_synced_codex_title: String::new(),
            last_synced_topic_name: String::new(),
            codex_state: "active".to_string(),
            telegram_state: "open".to_string(),
            archived_at_ms: None,
            missing_at_ms: None,
            lifecycle_generation: 0,
            lifecycle_revision: 0,
            last_checked_at_ms: 0,
        }
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WechatPersistedState {
    pub sync_buf_by_account: HashMap<String, String>,
    pub context_tokens: HashMap<String, String>,
    pub context_token_captured_at_ms: HashMap<String, u128>,
}

impl PersistedState {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let raw = serde_json::to_string_pretty(self)?;
        std::fs::write(path, raw)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::PersistedState;

    #[test]
    fn legacy_state_without_im_thread_bindings_uses_empty_default() {
        let state: PersistedState = serde_json::from_str(
            r#"{
                "wechat": {
                    "syncBufByAccount": {"wechat": "cursor"}
                }
            }"#,
        )
        .expect("legacy state");

        assert!(state.im_thread_bindings.is_empty());
        assert_eq!(
            state.wechat.sync_buf_by_account.get("wechat"),
            Some(&"cursor".to_string())
        );
    }

    #[test]
    fn im_thread_bindings_round_trip_with_camel_case_key() {
        let mut state = PersistedState::default();
        state
            .im_thread_bindings
            .insert("telegram:bot:42".to_string(), "thread-42".to_string());

        let raw = serde_json::to_string(&state).expect("serialize state");
        assert!(raw.contains("imThreadBindings"));

        let restored: PersistedState = serde_json::from_str(&raw).expect("restore state");
        assert_eq!(
            restored.im_thread_bindings.get("telegram:bot:42"),
            Some(&"thread-42".to_string())
        );
    }

    #[test]
    fn topic_binding_states_are_optional_for_legacy_state() {
        let state: PersistedState =
            serde_json::from_str(r#"{"imThreadBindings":{"telegram:bot:42":"thread-42"}}"#)
                .expect("legacy state");
        assert!(state.telegram_topic_binding_states.is_empty());
    }

    #[test]
    fn topic_binding_names_are_optional_for_older_bindings() {
        let state: PersistedState = serde_json::from_str(
            r#"{"telegramTopicBindingStates":{"telegram:bot:-100|topic=7":{"threadId":"t-7","topicName":"旧标题"}}}"#,
        )
        .expect("older topic binding");
        let binding = state
            .telegram_topic_binding_states
            .get("telegram:bot:-100|topic=7")
            .expect("binding");
        assert_eq!(binding.thread_id, "t-7");
        assert_eq!(binding.topic_name, "旧标题");
        assert!(binding.last_synced_codex_title.is_empty());
        assert_eq!(binding.lifecycle_revision, 0);
    }
}
