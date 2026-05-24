use serde::Serialize;
use tokio::sync::Mutex;

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImDesktopBinding {
    pub thread_id: Option<String>,
    pub thread_name: Option<String>,
    pub cwd: Option<String>,
    pub pending_plan_implement_turn_id: Option<String>,
    pub selected_model: Option<String>,
    pub selected_effort: Option<String>,
}

#[derive(Default)]
pub struct ImDesktopState {
    pub(crate) inner: Mutex<ImDesktopBinding>,
}
