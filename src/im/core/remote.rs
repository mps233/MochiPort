use serde::Serialize;
use tauri::{AppHandle, Manager};
use tokio::sync::Mutex;

use super::types::ImChannelKind;

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImRemoteBinding {
    // Snapshot of the currently active IM route observed by core.
    // This is not a thread binding; core uses it to connect a desktop-selected
    // thread with the active remote route when the user switches context on desktop.
    pub channel: Option<ImChannelKind>,
    pub account_id: Option<String>,
    pub route_id: Option<String>,
    pub route_hint: Option<String>,
}

#[derive(Default)]
pub struct ImRemoteState {
    pub(crate) inner: Mutex<ImRemoteBinding>,
}

pub async fn set_remote_binding<R: tauri::Runtime>(
    app: &AppHandle<R>,
    channel: Option<ImChannelKind>,
    account_id: Option<String>,
    route_id: Option<String>,
    route_hint: Option<String>,
) {
    let Some(state) = app.try_state::<ImRemoteState>() else {
        return;
    };
    let mut inner = state.inner.lock().await;
    inner.channel = channel;
    inner.account_id = account_id;
    inner.route_id = route_id;
    inner.route_hint = route_hint;
}

pub async fn clear_remote_binding<R: tauri::Runtime>(app: &AppHandle<R>) {
    set_remote_binding(app, None, None, None, None).await;
}

pub async fn set_remote_binding_for_route<R: tauri::Runtime>(
    app: &AppHandle<R>,
    channel: ImChannelKind,
    account_id: impl Into<String>,
    route_id: impl Into<String>,
    route_hint: impl Into<String>,
) {
    set_remote_binding(
        app,
        Some(channel),
        Some(account_id.into()),
        Some(route_id.into()),
        Some(route_hint.into()),
    )
    .await;
}

pub async fn current_remote_binding<R: tauri::Runtime>(app: &AppHandle<R>) -> ImRemoteBinding {
    let Some(state) = app.try_state::<ImRemoteState>() else {
        return ImRemoteBinding::default();
    };
    let binding = state.inner.lock().await.clone();
    binding
}
