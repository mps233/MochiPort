use std::{collections::HashMap, path::Path};

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PersistedState {
    pub wechat: WechatPersistedState,
    pub im_thread_bindings: HashMap<String, String>,
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
}
