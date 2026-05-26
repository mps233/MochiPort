use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{Html, IntoResponse},
    routing::{get, post},
};
use qrcode::{QrCode, render::svg};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    app_state::{FeishuWsState, SharedState},
    bridge,
    codex_app_config::{self, ConfigureCodexAppOptions},
    config::AppConfig,
    im::feishu::{FeishuApi, FeishuSettings},
};
use crate::{remote_control_backend, shim};

pub fn router(state: SharedState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/api/status", get(status))
        .route("/api/config", get(get_config).post(save_config))
        .route("/api/codex-app/configure", post(configure_codex_app))
        .route("/api/codex-app/status", get(codex_app_status))
        .route("/api/bridge/start", post(start_bridge))
        .route("/api/bridge/stop", post(stop_bridge))
        .route("/api/shim/status", get(shim_status))
        .route("/api/shim/enabled", post(shim_enabled))
        .route("/api/shim/install", post(shim_install))
        .route("/api/shim/uninstall", post(shim_uninstall))
        .route("/api/shim/candidates", get(shim_candidates))
        .route("/api/shim/event", post(shim_event))
        .route("/api/feishu/onboard/start", post(feishu_onboard_start))
        .route("/api/feishu/onboard/poll", post(feishu_onboard_poll))
        .route("/backend-api/plugins/list", get(plugin_legacy_list))
        .route("/backend-api/plugins/featured", get(plugin_legacy_featured))
        .route("/backend-api/ps/plugins/list", get(plugin_catalog_page))
        .route(
            "/backend-api/ps/plugins/workspace/shared",
            get(plugin_catalog_page),
        )
        .route(
            "/backend-api/ps/plugins/installed",
            get(plugin_installed_page),
        )
        .route("/api/events", get(events))
        .merge(remote_control_backend::router())
        .with_state(state)
}

async fn index() -> Html<&'static str> {
    Html(include_str!("web/index.html"))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StatusResponse {
    running: bool,
    bind: String,
    state_path: String,
    feishu_ws: FeishuWsState,
}

async fn status(State(state): State<SharedState>) -> Json<StatusResponse> {
    let running = state
        .bridge_task
        .lock()
        .await
        .as_ref()
        .map(|handle| !handle.is_finished())
        .unwrap_or(false);
    let config = state.config.lock().await;
    let feishu_ws = state.feishu_ws.lock().await.clone();
    Json(StatusResponse {
        running,
        bind: config.bind.clone(),
        state_path: config.state_path.to_string_lossy().to_string(),
        feishu_ws,
    })
}

async fn get_config(State(state): State<SharedState>) -> Json<AppConfig> {
    Json(state.config.lock().await.clone())
}

async fn save_config(
    State(state): State<SharedState>,
    Json(config): Json<AppConfig>,
) -> impl IntoResponse {
    if let Err(err) = config.save(&state.config_path) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": err.to_string() })),
        );
    }
    *state.config.lock().await = config;
    state
        .push_event("info", "config_saved", "configuration saved")
        .await;
    (StatusCode::OK, Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfigureCodexAppRequest {
    codex_home: Option<String>,
    provider_name: Option<String>,
    provider_base_url: Option<String>,
    provider_key: Option<String>,
    model: Option<String>,
}

async fn configure_codex_app(
    State(state): State<SharedState>,
    payload: Option<Json<ConfigureCodexAppRequest>>,
) -> impl IntoResponse {
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
    let model = request
        .as_ref()
        .and_then(|value| value.model.clone())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    let backend_url = shim::remote_control_base_url(&config);
    match codex_app_config::configure_codex_app(ConfigureCodexAppOptions {
        codex_home,
        backend_url: backend_url.clone(),
        account_id: "acct_codex_remote_local".to_string(),
        user_id: "user_codex_remote_local".to_string(),
        email: "codex-remote-local@example.local".to_string(),
        plan_type: "pro".to_string(),
        provider_name,
        provider_base_url,
        provider_key,
        model,
    }) {
        Ok(report) => {
            let gui_api_base = match codex_app_config::configure_gui_api_base_url(&backend_url) {
                Ok(status) => status,
                Err(err) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({
                            "ok": false,
                            "error": format!("Codex App config/auth 已写入，但设置双击启动 API base 失败：{err}")
                        })),
                    );
                }
            };
            state
                .push_event(
                    "info",
                    "codex_app_configured",
                    format!(
                        "codex_home={} config={} auth={} gui_api_base={}",
                        report.codex_home.display(),
                        report.config_path.display(),
                        report.auth_path.display(),
                        gui_api_base.value.as_deref().unwrap_or_default()
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
                })),
            )
        }
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "error": err.to_string() })),
        ),
    }
}

async fn codex_app_status(
    State(state): State<SharedState>,
) -> Json<codex_app_config::CodexAppConfigStatus> {
    let config = state.config.lock().await.clone();
    Json(codex_app_config::inspect_codex_app_config(
        None,
        &shim::remote_control_base_url(&config),
    ))
}

async fn start_bridge(State(state): State<SharedState>) -> impl IntoResponse {
    {
        let mut config = state.config.lock().await;
        config.bridge.enabled = true;
        if let Err(err) = config.save(&state.config_path) {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "ok": false, "error": err.to_string() })),
            );
        }
    }
    let mut task = state.bridge_task.lock().await;
    if task
        .as_ref()
        .map(|handle| !handle.is_finished())
        .unwrap_or(false)
    {
        return (StatusCode::OK, Json(json!({ "ok": true, "running": true })));
    }
    if task
        .as_ref()
        .map(|handle| handle.is_finished())
        .unwrap_or(false)
    {
        *task = None;
    }
    let bridge_state = state.clone();
    *task = Some(tokio::spawn(async move {
        bridge::start_bridge(bridge_state).await;
    }));
    state
        .push_event("info", "bridge_start_requested", "bridge start requested")
        .await;
    (StatusCode::OK, Json(json!({ "ok": true, "running": true })))
}

async fn stop_bridge(State(state): State<SharedState>) -> impl IntoResponse {
    {
        let mut config = state.config.lock().await;
        config.bridge.enabled = false;
        if let Err(err) = config.save(&state.config_path) {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "ok": false, "error": err.to_string() })),
            );
        }
    }
    stop_bridge_task(&state).await;
    (
        StatusCode::OK,
        Json(json!({ "ok": true, "running": false })),
    )
}

async fn stop_bridge_task(state: &SharedState) {
    let mut task = state.bridge_task.lock().await;
    if let Some(handle) = task.take() {
        handle.abort();
    }
    state.runtime.lock().await.invalidate_bridge_generation();
    {
        let mut ws = state.feishu_ws.lock().await;
        ws.connecting = false;
        ws.connected = false;
    }
    state
        .push_event("warn", "bridge_stopped", "bridge task aborted")
        .await;
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ShimStatusResponse {
    available: bool,
    enabled: bool,
    remote_control_base_url: String,
    remote_control_connected: bool,
    remote_control_initialized: bool,
    current_thread_id: Option<String>,
    shim_path: String,
    shim_dir: String,
    shim_installed: bool,
    path_configured: Option<bool>,
    real_codex_path: Option<String>,
    codex_candidates: Vec<shim::CodexCandidate>,
    feishu_configured: bool,
    reason: Option<String>,
}

async fn shim_status(State(state): State<SharedState>) -> Json<ShimStatusResponse> {
    let config = state.config.lock().await.clone();
    let remote = remote_control_backend::status_snapshot(&state).await;
    let shim_path = shim::shim_path(&config);
    let shim_installed = shim_path.exists();
    let path_configured = shim::user_path_contains_dir(&config.shim.bin_dir)
        .ok()
        .flatten();
    let feishu_configured =
        !config.feishu.app_id.trim().is_empty() && !config.feishu.app_secret.trim().is_empty();
    let codex_candidates = if config
        .shim
        .real_codex_path
        .as_ref()
        .is_some_and(|path| path.exists())
    {
        Vec::new()
    } else {
        shim::discover_codex_candidates(&config.shim.bin_dir)
            .into_iter()
            .take(8)
            .collect()
    };
    let reason = if !config.bridge.enabled {
        Some("bridge disabled".to_string())
    } else if !shim_installed {
        Some("shim is not installed".to_string())
    } else if !feishu_configured {
        Some("Feishu is not configured".to_string())
    } else {
        None
    };
    Json(ShimStatusResponse {
        available: config.bridge.enabled && shim_installed && feishu_configured,
        enabled: config.bridge.enabled,
        remote_control_base_url: shim::remote_control_base_url(&config),
        remote_control_connected: remote.connected,
        remote_control_initialized: remote.initialized,
        current_thread_id: remote.current_thread_id,
        shim_path: shim_path.to_string_lossy().to_string(),
        shim_dir: config.shim.bin_dir.to_string_lossy().to_string(),
        shim_installed,
        path_configured,
        real_codex_path: config
            .shim
            .real_codex_path
            .as_ref()
            .map(|path| path.to_string_lossy().to_string()),
        codex_candidates,
        feishu_configured,
        reason,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShimEnabledRequest {
    enabled: bool,
}

async fn shim_enabled(
    State(state): State<SharedState>,
    Json(request): Json<ShimEnabledRequest>,
) -> impl IntoResponse {
    {
        let mut config = state.config.lock().await;
        config.bridge.enabled = request.enabled;
        if let Err(err) = config.save(&state.config_path) {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "ok": false, "error": err.to_string() })),
            );
        }
    }
    if request.enabled {
        let mut task = state.bridge_task.lock().await;
        if task
            .as_ref()
            .map(|handle| handle.is_finished())
            .unwrap_or(false)
        {
            *task = None;
        }
        if task.is_none() {
            let bridge_state = state.clone();
            *task = Some(tokio::spawn(async move {
                bridge::start_bridge(bridge_state).await;
            }));
        }
        state
            .push_event("info", "bridge_enabled", "bridge enabled")
            .await;
    } else {
        stop_bridge_task(&state).await;
        state
            .push_event("warn", "bridge_disabled", "bridge disabled")
            .await;
    }
    (
        StatusCode::OK,
        Json(json!({ "ok": true, "enabled": request.enabled })),
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShimInstallRequest {
    real_codex_path: Option<String>,
    bin_dir: Option<String>,
}

async fn shim_install(
    State(state): State<SharedState>,
    payload: Option<Json<ShimInstallRequest>>,
) -> impl IntoResponse {
    let request = payload.map(|Json(value)| value);
    let real_codex = request
        .as_ref()
        .and_then(|value| value.real_codex_path.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from);
    let bin_dir = request
        .as_ref()
        .and_then(|value| value.bin_dir.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from);

    let result = {
        let mut config = state.config.lock().await;
        shim::install_shim(&mut config, &state.config_path, real_codex, bin_dir)
    };
    match result {
        Ok(report) => {
            let response = json!({
                "ok": true,
                "shimPath": report.shim_path.to_string_lossy().to_string(),
                "shimDir": report.bin_dir.to_string_lossy().to_string(),
                "realCodexPath": report.real_codex_path.to_string_lossy().to_string(),
                "pathUpdate": report.path_update,
            });
            state
                .push_event(
                    "info",
                    "shim_installed",
                    format!(
                        "shim={} real_codex={}",
                        report.shim_path.display(),
                        report.real_codex_path.display()
                    ),
                )
                .await;
            (StatusCode::OK, Json(response))
        }
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "error": err.to_string() })),
        ),
    }
}

async fn shim_uninstall(State(state): State<SharedState>) -> impl IntoResponse {
    let config = state.config.lock().await.clone();
    match shim::uninstall_shim(&config) {
        Ok(report) => {
            let response = json!({
                "ok": true,
                "shimPath": report.shim_path.to_string_lossy().to_string(),
                "shimDir": report.bin_dir.to_string_lossy().to_string(),
                "removedShim": report.removed_shim,
                "pathUpdate": report.path_update,
            });
            state
                .push_event(
                    "warn",
                    "shim_uninstalled",
                    format!("shim={}", report.shim_path.display()),
                )
                .await;
            (StatusCode::OK, Json(response))
        }
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "error": err.to_string() })),
        ),
    }
}

async fn shim_candidates(State(state): State<SharedState>) -> Json<Vec<shim::CodexCandidate>> {
    let config = state.config.lock().await.clone();
    Json(shim::discover_codex_candidates(&config.shim.bin_dir))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShimEventRequest {
    level: Option<String>,
    kind: String,
    message: String,
}

async fn shim_event(
    State(state): State<SharedState>,
    Json(request): Json<ShimEventRequest>,
) -> impl IntoResponse {
    let level = request.level.as_deref().unwrap_or("info");
    state
        .push_event(level, &request.kind, request.message)
        .await;
    Json(json!({ "ok": true }))
}

async fn events(State(state): State<SharedState>) -> impl IntoResponse {
    let events = state.events.lock().await.clone();
    Json(events)
}

async fn plugin_legacy_list() -> Json<Vec<serde_json::Value>> {
    Json(Vec::new())
}

async fn plugin_legacy_featured() -> Json<Vec<serde_json::Value>> {
    Json(Vec::new())
}

async fn plugin_catalog_page() -> Json<serde_json::Value> {
    Json(json!({
        "plugins": [],
        "pagination": {
            "next_page_token": null
        }
    }))
}

async fn plugin_installed_page() -> Json<serde_json::Value> {
    Json(json!({
        "plugins": [],
        "pagination": {
            "next_page_token": null
        }
    }))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FeishuOnboardStartResponse {
    verification_uri: String,
    verification_uri_complete: String,
    device_code: String,
    expires_in: u64,
    interval: u64,
    qr_svg: String,
}

async fn feishu_onboard_start(State(state): State<SharedState>) -> impl IntoResponse {
    let settings = {
        let config = state.config.lock().await;
        FeishuSettings::from_app_config(&config.feishu)
    };
    let api = FeishuApi::new(settings);
    match api.start_app_registration().await {
        Ok(payload) => {
            let verification_uri = payload
                .get("verification_uri")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let verification_uri_complete = payload
                .get("verification_uri_complete")
                .and_then(|v| v.as_str())
                .or_else(|| payload.get("verification_uri").and_then(|v| v.as_str()))
                .unwrap_or_default()
                .to_string();
            let device_code = payload
                .get("device_code")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let expires_in = payload
                .get("expire_in")
                .or_else(|| payload.get("expires_in"))
                .and_then(|v| v.as_u64())
                .unwrap_or(600);
            let interval = payload
                .get("interval")
                .and_then(|v| v.as_u64())
                .unwrap_or(5);
            let qr_svg = build_qr_svg(&verification_uri_complete).unwrap_or_default();
            state
                .push_event("info", "feishu_onboard_started", "scan flow started")
                .await;
            (
                StatusCode::OK,
                Json(json!(FeishuOnboardStartResponse {
                    verification_uri,
                    verification_uri_complete,
                    device_code,
                    expires_in,
                    interval,
                    qr_svg,
                })),
            )
        }
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": err.to_string() })),
        ),
    }
}

async fn feishu_onboard_poll(
    State(state): State<SharedState>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    let Some(device_code) = payload.get("deviceCode").and_then(|v| v.as_str()) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "missing deviceCode" })),
        );
    };
    let settings = {
        let config = state.config.lock().await;
        FeishuSettings::from_app_config(&config.feishu)
    };
    let api = FeishuApi::new(settings);
    match api.poll_app_registration(device_code).await {
        Ok(result) => {
            let app_id = result
                .get("client_id")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let app_secret = result
                .get("client_secret")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let open_id = result
                .get("user_info")
                .and_then(|v| v.get("open_id"))
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let done = app_id.is_some() && app_secret.is_some();
            let mut display_name = None;
            if let (Some(app_id), Some(app_secret)) = (app_id.clone(), app_secret.clone()) {
                let feishu_config = {
                    let mut config = state.config.lock().await;
                    config.feishu.app_id = app_id.clone();
                    config.feishu.app_secret = app_secret;
                    if let Some(open_id) = open_id.clone()
                        && !config.feishu.allowed_open_ids.contains(&open_id)
                    {
                        config.feishu.allowed_open_ids.push(open_id);
                    }
                    if let Err(err) = config.save(&state.config_path) {
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(json!({ "error": err.to_string() })),
                        );
                    }
                    config.feishu.clone()
                };
                let api = FeishuApi::new(FeishuSettings::from_app_config(&feishu_config));
                display_name = api
                    .get_application_display_name(&app_id)
                    .await
                    .ok()
                    .flatten();
                state
                    .push_event(
                        "info",
                        "feishu_onboard_completed",
                        format!(
                            "app_id={app_id} open_id={}",
                            open_id.clone().unwrap_or_default()
                        ),
                    )
                    .await;
            }
            (
                StatusCode::OK,
                Json(json!({
                    "done": done,
                    "appId": app_id,
                    "openId": open_id,
                    "displayName": display_name,
                    "error": result.get("error").cloned(),
                    "errorDescription": result.get("error_description").cloned(),
                    "raw": result,
                })),
            )
        }
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": err.to_string() })),
        ),
    }
}

fn build_qr_svg(content: &str) -> anyhow::Result<String> {
    let code = QrCode::new(content.as_bytes())?;
    Ok(code
        .render::<svg::Color>()
        .min_dimensions(220, 220)
        .dark_color(svg::Color("#20242a"))
        .light_color(svg::Color("#ffffff"))
        .build())
}
