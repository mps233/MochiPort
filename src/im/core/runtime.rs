use std::collections::{HashMap, HashSet};

use tauri::{AppHandle, Manager};
use tokio::sync::Mutex;

use super::desktop::ImDesktopBinding;

#[derive(Debug, Clone, Default)]
pub struct ImThreadRuntimeState {
    pub current_turn_id: Option<String>,
    pub current_turn_origin: Option<String>,
    pub last_status: Option<String>,
}

#[derive(Default)]
pub(crate) struct ImRuntimeInner {
    pub(crate) thread_runtime_by_id: HashMap<String, ImThreadRuntimeState>,
    pub(crate) turn_origin_by_id: HashMap<String, String>,
    pub(crate) turn_mode_by_id: HashMap<String, String>,
    pub(crate) turns_with_plan: HashSet<String>,
}

#[derive(Default)]
pub struct ImRuntimeState {
    pub(crate) inner: Mutex<ImRuntimeInner>,
}

pub async fn resolve_turn_origin<R: tauri::Runtime>(
    app: &AppHandle<R>,
    thread_id: &str,
    turn_id: Option<&str>,
) -> Option<String> {
    if let (Some(state), Some(turn_id)) = (app.try_state::<ImRuntimeState>(), turn_id) {
        let inner = state.inner.lock().await;
        if let Some(origin) = inner.turn_origin_by_id.get(turn_id).cloned() {
            return Some(origin);
        }
    }
    get_thread_runtime_state(app, thread_id)
        .await
        .current_turn_origin
}

pub async fn active_desktop_thread_id<R: tauri::Runtime>(app: &AppHandle<R>) -> Option<String> {
    let state = app.try_state::<super::desktop::ImDesktopState>()?;
    let inner = state.inner.lock().await;
    inner.thread_id.clone()
}

pub async fn desktop_binding_snapshot<R: tauri::Runtime>(app: &AppHandle<R>) -> ImDesktopBinding {
    let Some(state) = app.try_state::<super::desktop::ImDesktopState>() else {
        return ImDesktopBinding::default();
    };
    let inner = state.inner.lock().await;
    inner.clone()
}

pub async fn active_desktop_turn_busy<R: tauri::Runtime>(app: &AppHandle<R>) -> bool {
    let Some(thread_id) = active_desktop_thread_id(app).await else {
        return false;
    };
    thread_has_in_progress_turn(app, &thread_id).await
}

pub async fn is_active_desktop_thread<R: tauri::Runtime>(
    app: &AppHandle<R>,
    thread_id: &str,
) -> bool {
    active_desktop_thread_id(app).await.as_deref() == Some(thread_id)
}

pub async fn get_thread_runtime_state<R: tauri::Runtime>(
    app: &AppHandle<R>,
    thread_id: &str,
) -> ImThreadRuntimeState {
    let Some(state) = app.try_state::<ImRuntimeState>() else {
        return ImThreadRuntimeState::default();
    };
    let inner = state.inner.lock().await;
    inner
        .thread_runtime_by_id
        .get(thread_id)
        .cloned()
        .unwrap_or_default()
}

pub async fn thread_has_in_progress_turn<R: tauri::Runtime>(
    app: &AppHandle<R>,
    thread_id: &str,
) -> bool {
    let runtime = get_thread_runtime_state(app, thread_id).await;
    runtime.current_turn_id.is_some()
        || matches!(
            runtime.last_status.as_deref(),
            Some("running") | Some("in_progress") | Some("inProgress")
        )
}

pub async fn update_thread_runtime_state<R: tauri::Runtime>(
    app: &AppHandle<R>,
    thread_id: &str,
    f: impl FnOnce(&mut ImThreadRuntimeState),
) {
    let Some(state) = app.try_state::<ImRuntimeState>() else {
        return;
    };
    let mut inner = state.inner.lock().await;
    let runtime = inner
        .thread_runtime_by_id
        .entry(thread_id.to_string())
        .or_default();
    f(runtime);
}

pub async fn mark_turn_started<R: tauri::Runtime>(
    app: &AppHandle<R>,
    thread_id: &str,
    turn_id: &str,
) {
    update_thread_runtime_state(app, thread_id, |runtime| {
        runtime.current_turn_id = Some(turn_id.to_string());
        runtime.last_status = Some("running".to_string());
    })
    .await;
    if let Some(origin) = resolve_turn_origin(app, thread_id, Some(turn_id)).await {
        update_thread_runtime_state(app, thread_id, |runtime| {
            if runtime.current_turn_id.as_deref() == Some(turn_id) {
                runtime.current_turn_origin = Some(origin);
            }
        })
        .await;
    }
}

pub async fn mark_turn_completed<R: tauri::Runtime>(
    app: &AppHandle<R>,
    thread_id: &str,
) -> Option<String> {
    let previous_origin = get_thread_runtime_state(app, thread_id)
        .await
        .current_turn_origin;
    update_thread_runtime_state(app, thread_id, |runtime| {
        runtime.current_turn_id = None;
        runtime.current_turn_origin = None;
        runtime.last_status = Some("completed".to_string());
    })
    .await;
    previous_origin
}

pub async fn mark_thread_status<R: tauri::Runtime>(
    app: &AppHandle<R>,
    thread_id: &str,
    status: &str,
    terminal: bool,
) {
    update_thread_runtime_state(app, thread_id, |runtime| {
        runtime.last_status = Some(status.to_string());
        if terminal {
            runtime.current_turn_id = None;
            runtime.current_turn_origin = None;
        }
    })
    .await;
}

pub async fn note_turn_origin<R: tauri::Runtime>(app: &AppHandle<R>, turn_id: &str, origin: &str) {
    if turn_id.trim().is_empty() || origin.trim().is_empty() {
        return;
    }
    if let Some(state) = app.try_state::<ImRuntimeState>() {
        let mut inner = state.inner.lock().await;
        inner
            .turn_origin_by_id
            .insert(turn_id.to_string(), origin.to_string());
    }
}

pub async fn note_turn_mode<R: tauri::Runtime>(app: &AppHandle<R>, turn_id: &str, mode: &str) {
    if turn_id.trim().is_empty() || mode.trim().is_empty() {
        return;
    }
    if let Some(state) = app.try_state::<ImRuntimeState>() {
        let mut inner = state.inner.lock().await;
        inner
            .turn_mode_by_id
            .insert(turn_id.to_string(), mode.to_string());
    }
}

pub async fn resolve_turn_mode<R: tauri::Runtime>(
    app: &AppHandle<R>,
    turn_id: &str,
) -> Option<String> {
    if turn_id.trim().is_empty() {
        return None;
    }
    let state = app.try_state::<ImRuntimeState>()?;
    let inner = state.inner.lock().await;
    inner.turn_mode_by_id.get(turn_id).cloned()
}

pub async fn clear_turn_origin<R: tauri::Runtime>(app: &AppHandle<R>, turn_id: &str) {
    if turn_id.trim().is_empty() {
        return;
    }
    if let Some(state) = app.try_state::<ImRuntimeState>() {
        let mut inner = state.inner.lock().await;
        inner.turn_origin_by_id.remove(turn_id);
    }
}

pub async fn clear_turn_mode<R: tauri::Runtime>(app: &AppHandle<R>, turn_id: &str) {
    if turn_id.trim().is_empty() {
        return;
    }
    if let Some(state) = app.try_state::<ImRuntimeState>() {
        let mut inner = state.inner.lock().await;
        inner.turn_mode_by_id.remove(turn_id);
    }
}

pub async fn mark_turn_has_plan<R: tauri::Runtime>(app: &AppHandle<R>, turn_id: &str) {
    if turn_id.trim().is_empty() {
        return;
    }
    if let Some(state) = app.try_state::<ImRuntimeState>() {
        let mut inner = state.inner.lock().await;
        inner.turns_with_plan.insert(turn_id.to_string());
    }
}

pub async fn turn_has_plan<R: tauri::Runtime>(app: &AppHandle<R>, turn_id: &str) -> bool {
    if turn_id.trim().is_empty() {
        return false;
    }
    let Some(state) = app.try_state::<ImRuntimeState>() else {
        return false;
    };
    let inner = state.inner.lock().await;
    inner.turns_with_plan.contains(turn_id)
}

pub async fn clear_turn_plan<R: tauri::Runtime>(app: &AppHandle<R>, turn_id: &str) {
    if turn_id.trim().is_empty() {
        return;
    }
    if let Some(state) = app.try_state::<ImRuntimeState>() {
        let mut inner = state.inner.lock().await;
        inner.turns_with_plan.remove(turn_id);
    }
}

pub async fn clear_all_runtime<R: tauri::Runtime>(app: &AppHandle<R>) {
    if let Some(state) = app.try_state::<ImRuntimeState>() {
        let mut inner = state.inner.lock().await;
        inner.thread_runtime_by_id.clear();
        inner.turn_origin_by_id.clear();
        inner.turn_mode_by_id.clear();
        inner.turns_with_plan.clear();
    }
}

pub async fn clear_channel_runtime<R: tauri::Runtime>(app: &AppHandle<R>) {
    super::approval_state::clear_approval_runtime(app).await;
    super::outbound::clear_outbound_runtime(app).await;
    super::remote::clear_remote_binding(app).await;
    super::active::clear_active_binding(app).await;
    clear_all_runtime(app).await;
}
