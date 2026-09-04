use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use futures_util::FutureExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::panic::AssertUnwindSafe;

use crate::{
    ai_gateway::catalog::configured_models_response_with_etag,
    app_state::{LifecycleAdmissionPermit, SharedState},
    codex_app_config::{self, ConfigureCodexAppOptions},
    codex_app_enhanced,
    config::LocalConnectionMode,
    remote_control_backend,
};

use super::masked_url;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct EnhancedLaunchOperationRequest {
    request_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ConfigureCodexAppRequest {
    codex_home: Option<String>,
    connection_mode: Option<LocalConnectionMode>,
    provider_name: Option<String>,
    provider_base_url: Option<String>,
    provider_key: Option<String>,
    activate: Option<bool>,
    supports_websockets: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DeleteCodexAppProviderRequest {
    provider_name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SetCodexAppProviderWebSocketRequest {
    provider_name: String,
    enabled: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CodexAppSessionsResponse {
    ok: bool,
    threads: Vec<ManageCodexSession>,
    providers: Vec<String>,
    total: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ManageCodexSession {
    id: String,
    preview: String,
    model_provider: String,
    updated_at: i64,
    path: Option<String>,
    name: Option<String>,
    cwd: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ManageCodexAppStatus {
    codex_home: String,
    configured: bool,
    config_ok: bool,
    auth_ok: bool,
    provider_ok: bool,
    config_error: Option<String>,
    auth_error: Option<String>,
    gui_configured: bool,
    gui_error: Option<String>,
    remote_control_supported: bool,
    remote_control_configured: bool,
    remote_control_error: Option<String>,
    providers: Vec<ManageCodexAppProviderStatus>,
    image_generation_enabled: bool,
    connection_mode: LocalConnectionMode,
    provider_mode: codex_app_config::CodexProviderMode,
    provider_mode_message: String,
    active_provider: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ManageCodexAppProviderStatus {
    name: String,
    base_url: Option<String>,
    secret_set: bool,
    requires_openai_auth: bool,
    supports_websockets: bool,
}

fn codex_app_mutation_conflict() -> (StatusCode, Json<Value>) {
    (
        StatusCode::CONFLICT,
        Json(json!({
            "ok": false,
            "error": "另一个 Codex App 写操作正在进行，请稍后重试",
        })),
    )
}

pub(super) async fn configure_codex_app(
    State(state): State<SharedState>,
    payload: Option<Json<ConfigureCodexAppRequest>>,
) -> impl IntoResponse {
    let Ok(_mutation) = state.codex_app_mutations.try_lock() else {
        return codex_app_mutation_conflict();
    };
    let request = payload.map(|Json(value)| value);
    let config = state.config.lock().await.clone();
    let codex_home = request
        .as_ref()
        .and_then(|value| value.codex_home.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from);
    let provider_base_url = request
        .as_ref()
        .and_then(|value| value.provider_base_url.clone())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let provider_name = request
        .as_ref()
        .and_then(|value| value.provider_name.clone())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let provider_key = request
        .as_ref()
        .and_then(|value| value.provider_key.clone())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let activate_provider = request
        .as_ref()
        .and_then(|value| value.activate)
        .unwrap_or(true);
    let provider_supports_websockets = request.as_ref().and_then(|value| value.supports_websockets);
    let connection_mode = request
        .as_ref()
        .and_then(|value| value.connection_mode)
        .unwrap_or(config.local_connection_mode);

    let backend_url = config.remote_control_base_url();
    state
        .push_event(
            "info",
            "codex_app_configure_start",
            format!(
                "provider={} activate_provider={}",
                provider_name.as_deref().unwrap_or_default(),
                activate_provider
            ),
        )
        .await;
    match codex_app_config::configure_codex_app(ConfigureCodexAppOptions {
        codex_home,
        backend_url: backend_url.clone(),
        connection_mode,
        provider_name,
        provider_base_url,
        provider_key,
        activate_provider,
        provider_supports_websockets,
    }) {
        Ok(report) => {
            let gui_api_base = codex_app_config::inspect_gui_api_base_url(&backend_url);
            let remote_control_switch = report.remote_control_switch.clone();
            state
                .push_event(
                    "info",
                    "codex_app_configured",
                    format!(
                        "codex_home={} config={} auth={} gui_api_base={} remote_control_switch={}",
                        report.codex_home.display(),
                        report.config_path.display(),
                        report.auth_path.display(),
                        gui_api_base.value.as_deref().unwrap_or_default(),
                        remote_control_switch.configured
                    ),
                )
                .await;
            (
                StatusCode::OK,
                Json(json!({
                    "ok": true,
                    "codexHome": report.codex_home.to_string_lossy().to_string(),
                    "configPath": report.config_path.to_string_lossy().to_string(),
                    "authPath": report.auth_path.to_string_lossy().to_string(),
                    "backendUrl": report.backend_url,
                    "guiApiBase": gui_api_base,
                    "remoteControlSwitch": remote_control_switch,
                })),
            )
        }
        Err(err) => {
            state
                .push_event("error", "codex_app_configure_failed", err.to_string())
                .await;
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "ok": false, "error": err.to_string() })),
            )
        }
    }
}

pub(super) async fn set_codex_app_provider_websocket(
    State(state): State<SharedState>,
    Json(request): Json<SetCodexAppProviderWebSocketRequest>,
) -> impl IntoResponse {
    let Ok(_mutation) = state.codex_app_mutations.try_lock() else {
        return codex_app_mutation_conflict();
    };
    let provider_name = request.provider_name.trim();
    if provider_name.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": "provider_name is required" })),
        );
    }

    let config = state.config.lock().await.clone();
    let backend_url = config.remote_control_base_url();
    match codex_app_config::set_codex_app_provider_websocket(None, provider_name, request.enabled) {
        Ok(config_path) => {
            let status = codex_app_config::inspect_codex_app_config_for_mode(None, &backend_url);
            state
                .push_event(
                    "info",
                    "codex_app_provider_websocket_set",
                    format!(
                        "config={} provider={} supports_websockets={}",
                        config_path.display(),
                        provider_name,
                        request.enabled
                    ),
                )
                .await;
            (
                StatusCode::OK,
                Json(
                    json!({ "ok": true, "configPath": config_path.to_string_lossy().to_string(), "status": status }),
                ),
            )
        }
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "error": err.to_string() })),
        ),
    }
}

pub(super) async fn delete_codex_app_provider(
    State(state): State<SharedState>,
    Json(request): Json<DeleteCodexAppProviderRequest>,
) -> impl IntoResponse {
    let Ok(_mutation) = state.codex_app_mutations.try_lock() else {
        return codex_app_mutation_conflict();
    };
    let config = state.config.lock().await.clone();
    let backend_url = config.remote_control_base_url();
    match codex_app_config::delete_codex_app_provider(None, request.provider_name.trim()) {
        Ok(config_path) => {
            let status = codex_app_config::inspect_codex_app_config_for_mode(None, &backend_url);
            state
                .push_event(
                    "info",
                    "codex_app_provider_deleted",
                    format!(
                        "config={} provider={}",
                        config_path.display(),
                        request.provider_name.trim()
                    ),
                )
                .await;
            (
                StatusCode::OK,
                Json(
                    json!({ "ok": true, "configPath": config_path.to_string_lossy().to_string(), "status": status }),
                ),
            )
        }
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "error": err.to_string() })),
        ),
    }
}

pub(super) async fn uninstall_codex_app(State(state): State<SharedState>) -> impl IntoResponse {
    let Ok(_mutation) = state.codex_app_mutations.try_lock() else {
        return codex_app_mutation_conflict();
    };
    let config = state.config.lock().await.clone();
    let backend_url = config.remote_control_base_url();

    match codex_app_config::uninstall_codex_app(None, &backend_url) {
        Ok(report) => {
            state
                .push_event(
                    "info",
                    "codex_app_uninstalled",
                    format!(
                        "codex_home={} config={} auth={} removed_chatgpt_base_url={} removed_model_provider={} removed_auth={} gui_api_base={}",
                        report.codex_home.display(),
                        report.config_path.display(),
                        report.auth_path.display(),
                        report.removed_chatgpt_base_url,
                        report.removed_model_provider,
                        report.removed_auth,
                        report.gui_api_base.value.as_deref().unwrap_or_default()
                    ),
                )
                .await;
            (
                StatusCode::OK,
                Json(json!({ "ok": true, "report": report, "requiresCodexRestart": true })),
            )
        }
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "error": err.to_string() })),
        ),
    }
}

pub(super) async fn refresh_codex_app_models(
    State(state): State<SharedState>,
) -> impl IntoResponse {
    let cache_removed = match codex_app_config::clear_codex_models_cache(None) {
        Ok(removed) => removed,
        Err(err) => {
            state
                .push_event(
                    "warn",
                    "codex_app_models_cache_clear_failed",
                    err.to_string(),
                )
                .await;
            false
        }
    };

    let model_list_result = remote_control_backend::model_list_for_client(
        &state,
        remote_control_backend::default_remote_client_key(),
        true,
        Some(200),
    )
    .await;

    match model_list_result {
        Ok(value) => {
            let count = value
                .get("data")
                .and_then(|value| value.as_array())
                .map(Vec::len)
                .unwrap_or(0);
            state
                .push_event(
                    "info",
                    "codex_app_models_refreshed",
                    format!("cache_removed={cache_removed} count={count}"),
                )
                .await;
            Json(
                json!({ "ok": true, "cacheRemoved": cache_removed, "modelListRefreshed": true, "count": count }),
            )
        }
        Err(err) => {
            state
                .push_event(
                    "warn",
                    "codex_app_models_refresh_skipped",
                    format!("cache_removed={cache_removed} err={err}"),
                )
                .await;
            Json(
                json!({ "ok": true, "cacheRemoved": cache_removed, "modelListRefreshed": false, "error": err.to_string() }),
            )
        }
    }
}

pub(super) async fn launch_codex_app_enhanced(
    State(state): State<SharedState>,
) -> impl IntoResponse {
    let mutation = match state.codex_app_mutations.clone().try_lock_owned() {
        Ok(mutation) => mutation,
        Err(_) => return codex_app_mutation_conflict(),
    };
    let request_id = format!("legacy-{}", uuid::Uuid::new_v4().as_simple());
    match state
        .enhanced_launch_operations
        .begin(request_id.clone(), &state.lifecycle_admission)
    {
        codex_app_enhanced::EnhancedLaunchOperationBegin::Started {
            control,
            lifecycle_permit,
            ..
        } => {
            spawn_codex_app_enhanced_operation(
                state.clone(),
                request_id.clone(),
                control,
                lifecycle_permit,
                mutation,
            );
            match state
                .enhanced_launch_operations
                .wait_for_terminal(&request_id)
                .await
            {
                Some(operation) => legacy_enhanced_operation_response(operation),
                None => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "ok": false, "error": "增强启动状态丢失" })),
                ),
            }
        }
        codex_app_enhanced::EnhancedLaunchOperationBegin::Existing(operation) => {
            legacy_enhanced_operation_response(operation)
        }
        codex_app_enhanced::EnhancedLaunchOperationBegin::Conflict(operation) => (
            StatusCode::CONFLICT,
            Json(json!({
                "ok": false,
                "error": "已有增强启动正在进行",
                "operation": operation,
            })),
        ),
        codex_app_enhanced::EnhancedLaunchOperationBegin::LifecycleUnavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "ok": false,
                "error": "后台服务正在切换或关闭，暂时不能启动增强模式",
            })),
        ),
    }
}

pub(super) async fn start_codex_app_enhanced_operation(
    State(state): State<SharedState>,
    Json(request): Json<EnhancedLaunchOperationRequest>,
) -> impl IntoResponse {
    let request_id = match normalized_enhanced_request_id(&request.request_id) {
        Ok(request_id) => request_id,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "ok": false, "error": error })),
            );
        }
    };
    let mutation = match state.codex_app_mutations.clone().try_lock_owned() {
        Ok(mutation) => mutation,
        Err(_) => {
            if let Some(operation) = state.enhanced_launch_operations.current() {
                if operation.request_id == request_id {
                    return (
                        StatusCode::OK,
                        Json(json!({ "ok": true, "operation": operation })),
                    );
                }
                if !operation.is_terminal() {
                    return (
                        StatusCode::CONFLICT,
                        Json(json!({
                            "ok": false,
                            "error": "已有其他增强启动正在进行",
                            "operation": operation,
                        })),
                    );
                }
            }
            return codex_app_mutation_conflict();
        }
    };
    match state
        .enhanced_launch_operations
        .begin(request_id.clone(), &state.lifecycle_admission)
    {
        codex_app_enhanced::EnhancedLaunchOperationBegin::Started {
            operation,
            control,
            lifecycle_permit,
        } => {
            spawn_codex_app_enhanced_operation(
                state,
                request_id,
                control,
                lifecycle_permit,
                mutation,
            );
            (
                StatusCode::ACCEPTED,
                Json(json!({ "ok": true, "operation": operation })),
            )
        }
        codex_app_enhanced::EnhancedLaunchOperationBegin::Existing(operation) => (
            StatusCode::OK,
            Json(json!({ "ok": true, "operation": operation })),
        ),
        codex_app_enhanced::EnhancedLaunchOperationBegin::Conflict(operation) => (
            StatusCode::CONFLICT,
            Json(json!({
                "ok": false,
                "error": "已有其他增强启动正在进行",
                "operation": operation,
            })),
        ),
        codex_app_enhanced::EnhancedLaunchOperationBegin::LifecycleUnavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "ok": false,
                "error": "后台服务正在切换或关闭，暂时不能启动增强模式",
            })),
        ),
    }
}

pub(super) async fn codex_app_enhanced_operation(
    State(state): State<SharedState>,
) -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(json!({
            "ok": true,
            "operation": state.enhanced_launch_operations.current(),
        })),
    )
}

pub(super) async fn cancel_codex_app_enhanced_operation(
    State(state): State<SharedState>,
    Json(request): Json<EnhancedLaunchOperationRequest>,
) -> impl IntoResponse {
    let request_id = match normalized_enhanced_request_id(&request.request_id) {
        Ok(request_id) => request_id,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "ok": false, "error": error })),
            );
        }
    };
    match state.enhanced_launch_operations.cancel(&request_id) {
        codex_app_enhanced::EnhancedLaunchCancelResult::Accepted(operation) => (
            StatusCode::ACCEPTED,
            Json(json!({ "ok": true, "operation": operation })),
        ),
        codex_app_enhanced::EnhancedLaunchCancelResult::Terminal(operation) => (
            StatusCode::OK,
            Json(json!({ "ok": true, "operation": operation })),
        ),
        codex_app_enhanced::EnhancedLaunchCancelResult::Conflict(operation) => {
            let error = if operation.request_id == request_id {
                "增强启动正在完成，已经无法取消"
            } else {
                "requestId 与当前增强启动不一致"
            };
            (
                StatusCode::CONFLICT,
                Json(json!({ "ok": false, "error": error, "operation": operation })),
            )
        }
        codex_app_enhanced::EnhancedLaunchCancelResult::NotFound => (
            StatusCode::NOT_FOUND,
            Json(json!({ "ok": false, "error": "没有可取消的增强启动" })),
        ),
    }
}

fn normalized_enhanced_request_id(request_id: &str) -> Result<String, &'static str> {
    let request_id = request_id.trim();
    if request_id.is_empty() {
        return Err("requestId 不能为空");
    }
    if request_id.len() > 128 || request_id.chars().any(char::is_control) {
        return Err("requestId 格式无效");
    }
    Ok(request_id.to_string())
}

async fn run_codex_app_enhanced_operation(
    state: SharedState,
    request_id: String,
    control: codex_app_enhanced::EnhancedLaunchControl,
    lifecycle_permit: LifecycleAdmissionPermit,
) {
    let _lifecycle_permit = lifecycle_permit;
    let operation_control = control.clone();
    let (models, backend_url) = {
        let config = state.config.lock().await;
        let (response, _) = configured_models_response_with_etag(&config.ai_gateway);
        let models = response["models"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|model| model.get("slug").and_then(serde_json::Value::as_str))
            .map(str::to_string)
            .collect::<Vec<_>>();
        (models, config.remote_control_base_url())
    };
    state
        .push_event(
            "info",
            "codex_app_enhanced_launch_start",
            format!("models={}", models.len()),
        )
        .await;
    let result =
        codex_app_enhanced::launch_and_inject_controlled(models, &backend_url, control).await;
    match result {
        Ok(report) => {
            let event_message = format!(
                "launched={} port={} models={} gates={} i18n={}",
                report.launched,
                report.port,
                report.available_models.len(),
                report.key_gates_enabled,
                report.i18n_enabled
            );
            state.enhanced_launch_operations.finish_success(
                &request_id,
                operation_control.cancellation(),
                report,
            );
            state
                .push_event("info", "codex_app_enhanced_launch_ready", event_message)
                .await;
        }
        Err(failure) => {
            let error = failure.error.clone();
            state.enhanced_launch_operations.finish_failure(
                &request_id,
                operation_control.cancellation(),
                failure,
            );
            state
                .push_event("error", "codex_app_enhanced_launch_failed", error)
                .await;
        }
    }
}

fn spawn_codex_app_enhanced_operation(
    state: SharedState,
    request_id: String,
    control: codex_app_enhanced::EnhancedLaunchControl,
    lifecycle_permit: LifecycleAdmissionPermit,
    mutation: tokio::sync::OwnedMutexGuard<()>,
) {
    let manager = state.enhanced_launch_operations.clone();
    let panic_control = control.clone();
    let panic_request_id = request_id.clone();
    tokio::spawn(async move {
        let panicked = AssertUnwindSafe(run_codex_app_enhanced_operation(
            state.clone(),
            request_id,
            control,
            lifecycle_permit,
        ))
        .catch_unwind()
        .await
        .is_err();
        if panicked {
            manager.finish_failure(
                &panic_request_id,
                panic_control.cancellation(),
                codex_app_enhanced::EnhancedLaunchFailure {
                    error: "增强启动后台任务异常退出".to_string(),
                    recovery: Some(
                        "请检查 Codex App 状态；如接入异常，请先在设置中修复后重试".to_string(),
                    ),
                    cancelled: false,
                },
            );
            // Keep the mutation guard until the panic path has published a
            // terminal operation state, just like the normal worker path.
            drop(mutation);
            state
                .push_event(
                    "error",
                    "codex_app_enhanced_launch_worker_panicked",
                    "增强启动后台任务异常退出",
                )
                .await;
        } else {
            drop(mutation);
        }
    });
}

fn legacy_enhanced_operation_response(
    operation: codex_app_enhanced::EnhancedLaunchOperation,
) -> (StatusCode, Json<Value>) {
    match operation.phase {
        codex_app_enhanced::EnhancedLaunchOperationPhase::Ready => (
            StatusCode::OK,
            Json(json!({ "ok": true, "report": operation.report })),
        ),
        codex_app_enhanced::EnhancedLaunchOperationPhase::Cancelled => (
            StatusCode::CONFLICT,
            Json(json!({
                "ok": false,
                "error": operation.error.unwrap_or_else(|| "增强启动已取消".to_string()),
                "recovery": operation.recovery,
            })),
        ),
        codex_app_enhanced::EnhancedLaunchOperationPhase::Failed => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "ok": false,
                "error": operation.error.unwrap_or_else(|| "增强启动失败".to_string()),
                "recovery": operation.recovery,
            })),
        ),
        _ => (
            StatusCode::CONFLICT,
            Json(json!({
                "ok": false,
                "error": "增强启动仍在进行",
                "operation": operation,
            })),
        ),
    }
}

pub(super) async fn codex_app_enhanced_preflight() -> impl IntoResponse {
    match codex_app_enhanced::preflight().await {
        Ok(status) => (
            StatusCode::OK,
            Json(json!({ "ok": true, "status": status })),
        ),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "error": err.to_string() })),
        ),
    }
}

pub(super) async fn codex_app_sessions(State(state): State<SharedState>) -> impl IntoResponse {
    const PAGE_LIMIT: u32 = 100;
    const MAX_PAGES: usize = 20;
    let raw_threads = match remote_control_backend::session_history_threads(
        &state,
        remote_control_backend::default_remote_client_key(),
        PAGE_LIMIT,
        MAX_PAGES,
        false,
    )
    .await
    {
        Ok(threads) => threads,
        Err(err) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "ok": false, "error": err.to_string() })),
            );
        }
    };

    let threads = raw_threads
        .into_iter()
        .filter_map(normalize_managed_session)
        .collect::<Vec<_>>();
    let mut providers = threads
        .iter()
        .map(|thread| normalize_session_provider_name(&thread.model_provider))
        .collect::<Vec<_>>();
    providers.extend(
        codex_app_status_snapshot(&state)
            .await
            .providers
            .into_iter()
            .map(|provider| normalize_session_provider_name(&provider.name)),
    );
    providers.push("openai".to_string());
    providers.retain(|provider| !provider.trim().is_empty());
    providers.sort();
    providers.dedup();

    (
        StatusCode::OK,
        Json(json!(CodexAppSessionsResponse {
            ok: true,
            total: threads.len(),
            threads,
            providers,
        })),
    )
}

fn normalize_managed_session(thread: serde_json::Value) -> Option<ManageCodexSession> {
    let id = first_session_string(&thread, &["id", "threadId"])?;
    let preview = first_session_string(&thread, &["preview"]).unwrap_or_default();
    let model_provider = first_session_string(&thread, &["modelProvider", "model_provider"])
        .map(|provider| normalize_session_provider_name(&provider))
        .unwrap_or_else(|| "openai".to_string());
    let updated_at = first_session_i64(&thread, &["updatedAt", "updated_at"]).unwrap_or_default();
    let path = first_session_string(&thread, &["path", "rolloutPath", "rollout_path"]);
    let name = first_session_string(&thread, &["name", "title"]);
    let cwd = thread.get("cwd").and_then(session_path_value);
    Some(ManageCodexSession {
        id,
        preview,
        model_provider,
        updated_at,
        path,
        name,
        cwd,
    })
}

fn normalize_session_provider_name(provider: &str) -> String {
    if provider.trim().eq_ignore_ascii_case("ai-gateway") {
        "MochiPort".to_string()
    } else {
        provider.to_string()
    }
}

fn session_path_value(value: &Value) -> Option<String> {
    if let Some(path) = value.as_str() {
        return non_empty_session_string(path);
    }
    if let Some(values) = value.as_array() {
        return values.iter().find_map(session_path_value);
    }
    let object = value.as_object()?;
    ["path", "value", "text", "uri"]
        .iter()
        .find_map(|key| object.get(*key).and_then(session_path_value))
}

fn non_empty_session_string(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn first_session_string(value: &serde_json::Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

fn first_session_i64(value: &serde_json::Value, keys: &[&str]) -> Option<i64> {
    keys.iter().find_map(|key| {
        let value = value.get(*key)?;
        value
            .as_i64()
            .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
            .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
    })
}

pub(super) async fn repair_codex_app_gui_environment(
    State(state): State<SharedState>,
) -> impl IntoResponse {
    let Ok(_mutation) = state.codex_app_mutations.try_lock() else {
        return codex_app_mutation_conflict();
    };
    let config = state.config.lock().await.clone();
    let backend_url = config.remote_control_base_url();
    let status = codex_app_config::inspect_codex_app_config_for_mode(None, &backend_url);
    if !status.config_ok || !status.auth_ok {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "Codex App local config is not ready; write config first",
            })),
        );
    }

    let remote_control_switch =
        match codex_app_config::enable_codex_app_remote_control_switch_for_backend(
            None,
            &backend_url,
        ) {
            Ok(status) => status,
            Err(err) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "ok": false,
                        "error": err.to_string(),
                    })),
                );
            }
        };
    let gui_api_base = codex_app_config::cleanup_gui_environment(&backend_url);
    state
        .push_event(
            "info",
            "codex_app_gui_environment_cleaned",
            format!(
                "gui_api_base={} login_issuer={} remote_control_switch={}",
                gui_api_base.value.as_deref().unwrap_or_default(),
                gui_api_base
                    .login_issuer_value
                    .as_deref()
                    .unwrap_or_default(),
                remote_control_switch.configured
            ),
        )
        .await;
    (
        StatusCode::OK,
        Json(json!({
            "ok": true,
            "guiApiBase": gui_api_base,
            "remoteControlSwitch": remote_control_switch,
        })),
    )
}

pub(super) async fn switch_codex_app_to_direct_api_mode(
    State(state): State<SharedState>,
) -> impl IntoResponse {
    let Ok(_mutation) = state.codex_app_mutations.try_lock() else {
        return codex_app_mutation_conflict();
    };
    let config = state.config.lock().await.clone();
    let backend_url = config.remote_control_base_url();
    match codex_app_config::switch_codex_app_to_direct_api_mode(None, &backend_url) {
        Ok(report) => {
            state
                .push_event(
                    "info",
                    "codex_app_direct_api_mode_enabled",
                    format!("provider={}", report.active_provider),
                )
                .await;
            (
                StatusCode::OK,
                Json(json!({ "ok": true, "report": report })),
            )
        }
        Err(err) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": err.to_string() })),
        ),
    }
}

pub(super) async fn codex_app_status(
    State(state): State<SharedState>,
) -> Json<codex_app_config::CodexAppConfigStatus> {
    Json(codex_app_status_snapshot(&state).await)
}

pub(super) async fn manage_codex_app_status(
    State(state): State<SharedState>,
) -> Json<ManageCodexAppStatus> {
    let status = codex_app_status_snapshot(&state).await;
    Json(ManageCodexAppStatus {
        codex_home: status.codex_home.to_string_lossy().into_owned(),
        configured: status.configured,
        config_ok: status.config_ok,
        auth_ok: status.auth_ok,
        provider_ok: status.provider_ok,
        config_error: status
            .config_error
            .map(|_| "Codex configuration is invalid".to_string()),
        auth_error: status
            .auth_error
            .map(|_| "Codex authentication is not ready".to_string()),
        gui_configured: status.gui_api_base.configured
            && status.gui_api_base.login_issuer_configured,
        gui_error: status
            .gui_api_base
            .error
            .map(|_| "Codex GUI environment is not ready".to_string()),
        remote_control_supported: status.remote_control_switch.supported,
        remote_control_configured: status.remote_control_switch.configured,
        remote_control_error: status
            .remote_control_switch
            .error
            .map(|_| "Codex remote control is not ready".to_string()),
        providers: status
            .providers
            .into_iter()
            .map(|provider| ManageCodexAppProviderStatus {
                name: normalize_session_provider_name(&provider.name),
                base_url: provider.base_url.as_deref().map(masked_url),
                secret_set: provider
                    .key
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty()),
                requires_openai_auth: provider.requires_openai_auth,
                supports_websockets: provider.supports_websockets,
            })
            .collect(),
        image_generation_enabled: status.image_generation_enabled,
        connection_mode: status.connection_mode,
        provider_mode: status.provider_mode,
        provider_mode_message: status.provider_mode_message,
        active_provider: status
            .active_provider
            .map(|provider| normalize_session_provider_name(&provider)),
    })
}

pub(super) async fn codex_app_status_snapshot(
    state: &SharedState,
) -> codex_app_config::CodexAppConfigStatus {
    let config = state.config.lock().await.clone();
    codex_app_config::inspect_codex_app_config_for_mode(None, &config.remote_control_base_url())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_sessions_supply_defaults_for_legacy_thread_shapes() {
        let session =
            normalize_managed_session(json!({ "id": "legacy-thread" })).expect("legacy session");

        assert_eq!(session.id, "legacy-thread");
        assert_eq!(session.preview, "");
        assert_eq!(session.model_provider, "openai");
        assert_eq!(session.updated_at, 0);
        assert_eq!(session.path, None);
        assert_eq!(session.name, None);
        assert_eq!(session.cwd, None);
    }

    #[test]
    fn managed_sessions_normalize_workspace_shapes() {
        let session = normalize_managed_session(json!({
            "id": "workspace-thread",
            "cwd": { "path": "/Users/me/Projects/codexhub" }
        }))
        .expect("workspace session");

        assert_eq!(session.cwd.as_deref(), Some("/Users/me/Projects/codexhub"));

        let session = normalize_managed_session(json!({
            "id": "string-workspace-thread",
            "cwd": "/Users/me/Projects/other"
        }))
        .expect("string workspace session");
        assert_eq!(session.cwd.as_deref(), Some("/Users/me/Projects/other"));
    }

    #[test]
    fn provider_url_masking_never_returns_unparseable_input() {
        assert_eq!(masked_url("https://broken url?api_key=canary"), "<invalid>");
        assert_eq!(
            masked_url("https://user:password@provider.example/v1?api_key=canary"),
            "https://provider.example/v1"
        );
    }
}
