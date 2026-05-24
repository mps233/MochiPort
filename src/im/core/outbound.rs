use std::collections::HashMap;

use tauri::{AppHandle, Manager};
use tokio::sync::Mutex;

#[derive(Default)]
pub struct ImOutboundState {
    pub(crate) inner: Mutex<ImOutboundStore>,
}

#[derive(Default)]
pub(crate) struct ImOutboundStore {
    pub(crate) last_sent_text_by_route: HashMap<String, String>,
}

pub async fn should_skip_duplicate_text<R: tauri::Runtime>(
    app: &AppHandle<R>,
    route_key: &str,
    text: &str,
) -> bool {
    let Some(state) = app.try_state::<ImOutboundState>() else {
        return false;
    };
    let inner = state.inner.lock().await;
    inner
        .last_sent_text_by_route
        .get(route_key)
        .map(|last| last == text)
        .unwrap_or(false)
}

pub async fn remember_sent_text<R: tauri::Runtime>(
    app: &AppHandle<R>,
    route_key: &str,
    text: &str,
) {
    let Some(state) = app.try_state::<ImOutboundState>() else {
        return;
    };
    let mut inner = state.inner.lock().await;
    inner
        .last_sent_text_by_route
        .insert(route_key.to_string(), text.to_string());
}

pub async fn clear_outbound_runtime<R: tauri::Runtime>(app: &AppHandle<R>) {
    let Some(state) = app.try_state::<ImOutboundState>() else {
        return;
    };
    let mut inner = state.inner.lock().await;
    inner.last_sent_text_by_route.clear();
}
