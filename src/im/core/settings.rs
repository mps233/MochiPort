use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::workspace_state::WorkspaceState;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImCommonSettings {
    pub approval_policy: Option<String>,
    pub approvals_reviewer: Option<String>,
    pub sandbox_mode: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub route_tag: Option<String>,
    pub sync_tool_calls: Option<bool>,
    pub sync_reasoning: Option<bool>,
    pub sync_debug_events: Option<bool>,
}

pub trait ImSyncSettings {
    fn sync_tool_calls(&self) -> Option<bool>;
    fn sync_reasoning(&self) -> Option<bool>;
    fn sync_debug_events(&self) -> Option<bool>;
}

impl ImSyncSettings for ImCommonSettings {
    fn sync_tool_calls(&self) -> Option<bool> {
        self.sync_tool_calls
    }

    fn sync_reasoning(&self) -> Option<bool> {
        self.sync_reasoning
    }

    fn sync_debug_events(&self) -> Option<bool> {
        self.sync_debug_events
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ImSyncMessageKind {
    ToolCall,
    Reasoning,
    DebugEvent,
}

pub fn should_sync_message_kind(settings: &impl ImSyncSettings, kind: ImSyncMessageKind) -> bool {
    match kind {
        ImSyncMessageKind::ToolCall => settings.sync_tool_calls().unwrap_or(false),
        ImSyncMessageKind::Reasoning => settings.sync_reasoning().unwrap_or(false),
        ImSyncMessageKind::DebugEvent => settings.sync_debug_events().unwrap_or(false),
    }
}

impl Default for ImCommonSettings {
    fn default() -> Self {
        Self {
            approval_policy: Some("on-request".to_string()),
            approvals_reviewer: Some("user".to_string()),
            sandbox_mode: Some("workspace-write".to_string()),
            model: None,
            reasoning_effort: None,
            route_tag: None,
            sync_tool_calls: Some(false),
            sync_reasoning: Some(false),
            sync_debug_events: Some(false),
        }
    }
}

pub fn thread_sandbox_for_settings(
    approval_policy: Option<&str>,
    sandbox_mode: Option<&str>,
) -> serde_json::Value {
    match sandbox_mode.unwrap_or_default() {
        "danger-full-access" => serde_json::json!("danger-full-access"),
        "workspace-write" => serde_json::json!("workspace-write"),
        _ => match approval_policy.unwrap_or_default() {
            "never" => serde_json::json!("danger-full-access"),
            "on-request" | "untrusted" => serde_json::json!("workspace-write"),
            _ => serde_json::json!("read-only"),
        },
    }
}

pub async fn resolve_workspace_cwd<R: tauri::Runtime>(app: &AppHandle<R>) -> Option<String> {
    let Some(state) = app.try_state::<WorkspaceState>() else {
        return None;
    };
    let current = state.current.read().await.clone();
    current.filter(|v| !v.trim().is_empty())
}
