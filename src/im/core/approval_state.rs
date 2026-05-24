use std::collections::HashMap;

use tauri::{AppHandle, Manager};
use tokio::sync::Mutex;

use super::PendingApprovalRequest;

#[derive(Default)]
pub struct ImApprovalState {
    pub(crate) inner: Mutex<HashMap<String, Vec<PendingApprovalRequest>>>,
}

pub async fn push_pending_approval<R: tauri::Runtime>(
    app: &AppHandle<R>,
    route_key: &str,
    pending: PendingApprovalRequest,
) {
    let Some(state) = app.try_state::<ImApprovalState>() else {
        return;
    };
    let mut inner = state.inner.lock().await;
    inner
        .entry(route_key.to_string())
        .or_default()
        .push(pending);
}

pub async fn first_pending_approval<R: tauri::Runtime>(
    app: &AppHandle<R>,
    route_key: &str,
) -> Option<PendingApprovalRequest> {
    let state = app.try_state::<ImApprovalState>()?;
    let inner = state.inner.lock().await;
    inner
        .get(route_key)
        .and_then(|items| items.first().cloned())
}

pub async fn pop_pending_approval<R: tauri::Runtime>(
    app: &AppHandle<R>,
    route_key: &str,
) -> Option<(PendingApprovalRequest, Option<PendingApprovalRequest>)> {
    let state = app.try_state::<ImApprovalState>()?;
    let mut inner = state.inner.lock().await;
    let items = inner.get_mut(route_key)?;
    if items.is_empty() {
        return None;
    }
    let current = items.remove(0);
    let next = items.first().cloned();
    if items.is_empty() {
        inner.remove(route_key);
    }
    Some((current, next))
}

pub async fn approval_request_ids_for_route<R: tauri::Runtime>(
    app: &AppHandle<R>,
    route_key: &str,
) -> Vec<String> {
    let Some(state) = app.try_state::<ImApprovalState>() else {
        return Vec::new();
    };
    let inner = state.inner.lock().await;
    inner
        .get(route_key)
        .into_iter()
        .flat_map(|items| items.iter().map(|item| item.request_id.clone()))
        .collect()
}

pub async fn clear_approval_runtime<R: tauri::Runtime>(app: &AppHandle<R>) {
    let Some(state) = app.try_state::<ImApprovalState>() else {
        return;
    };
    let mut inner = state.inner.lock().await;
    inner.clear();
}
