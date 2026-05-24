use serde::Serialize;
use tauri::{AppHandle, Manager};
use tokio::sync::Mutex;

use super::types::ImChannelKind;

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImActiveBinding {
    pub channel: Option<ImChannelKind>,
    pub account_id: Option<String>,
}

#[derive(Default)]
pub struct ImActiveState {
    pub(crate) inner: Mutex<ImActiveBinding>,
}

pub async fn current_active_binding<R: tauri::Runtime>(
    app: &AppHandle<R>,
) -> Option<ImActiveBinding> {
    let Some(state) = app.try_state::<ImActiveState>() else {
        return None;
    };
    let inner = state.inner.lock().await;
    Some(inner.clone())
}

pub async fn current_active_kind<R: tauri::Runtime>(app: &AppHandle<R>) -> Option<ImChannelKind> {
    current_active_binding(app)
        .await
        .and_then(|binding| binding.channel)
}

pub async fn set_active_binding<R: tauri::Runtime>(
    app: &AppHandle<R>,
    channel: Option<ImChannelKind>,
    account_id: Option<String>,
) {
    let Some(state) = app.try_state::<ImActiveState>() else {
        return;
    };
    let mut inner = state.inner.lock().await;
    inner.channel = channel;
    inner.account_id = account_id;
}

pub async fn clear_active_binding<R: tauri::Runtime>(app: &AppHandle<R>) {
    set_active_binding(app, None, None).await;
}

pub async fn is_active_account<R: tauri::Runtime>(
    app: &AppHandle<R>,
    kind: ImChannelKind,
    account_id: &str,
) -> bool {
    let Some(binding) = current_active_binding(app).await else {
        return false;
    };
    binding.channel == Some(kind) && binding.account_id.as_deref() == Some(account_id)
}
