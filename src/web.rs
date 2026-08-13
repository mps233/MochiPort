use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::{
        Request, StatusCode,
        header::{CACHE_CONTROL, EXPIRES, HeaderValue, PRAGMA},
    },
    middleware::{self, Next},
    response::IntoResponse,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    app_state::{FeishuWsState, ImAccountRuntimeState, SharedState, TelegramState, WechatState},
    chain_log, codex_app_config,
    config::AppConfig,
    manage_api, remote_control_backend,
};

mod codex_app;
mod im_api;
mod oauth;
mod onboarding;
pub(crate) mod plugins;

pub async fn start_bridge_if_ready(state: &SharedState, event_message: &'static str) -> bool {
    im_api::start_bridge_task(state, im_api::BridgeStartMode::KeepExisting, event_message).await
}

pub fn router(state: SharedState) -> Router {
    // Initialize the shared user-domain credential before serving requests.
    // Errors are surfaced as a protected-route 500 without exposing secrets;
    // legacy routes remain available during the migration window.
    let _ = manage_api::ensure_management_token(&state.config_path);
    let manage_routes = Router::new()
        .route("/status", get(manage_api::status))
        .route("/dashboard", get(manage_dashboard))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            manage_api::require_bearer,
        ));

    Router::new()
        .route("/healthz", get(manage_api::healthz))
        .nest("/api/v1/manage", manage_routes)
        .route("/oauth/authorize", get(oauth::oauth_authorize))
        .route("/oauth/token", post(oauth::oauth_token))
        .route("/api/status", get(status))
        .route("/api/gui/dashboard", get(gui_dashboard))
        .route("/api/shutdown", post(shutdown))
        .route("/api/shutdown/instance", post(shutdown_instance))
        .route(
            "/api/update/safe-relaunch",
            get(crate::safe_relaunch::status).post(crate::safe_relaunch::register),
        )
        .route("/api/config", get(get_config).post(save_config))
        .route(
            "/api/codex-app/configure",
            post(codex_app::configure_codex_app),
        )
        .route(
            "/api/codex-app/provider/websocket",
            post(codex_app::set_codex_app_provider_websocket),
        )
        .route(
            "/api/codex-app/provider/delete",
            post(codex_app::delete_codex_app_provider),
        )
        .route(
            "/api/codex-app/repair-gui-environment",
            post(codex_app::repair_codex_app_gui_environment),
        )
        .route(
            "/api/codex-app/uninstall",
            post(codex_app::uninstall_codex_app),
        )
        .route("/api/codex-app/status", get(codex_app::codex_app_status))
        .route(
            "/api/codex-app/models/refresh",
            post(codex_app::refresh_codex_app_models),
        )
        .route(
            "/api/codex-app/enhanced-launch",
            post(codex_app::launch_codex_app_enhanced),
        )
        .route(
            "/api/codex-app/enhanced-launch/preflight",
            get(codex_app::codex_app_enhanced_preflight),
        )
        .route(
            "/api/codex-app/sessions",
            get(codex_app::codex_app_sessions),
        )
        .route(
            "/api/codex-app/session/provider",
            post(codex_app::move_codex_app_session_provider),
        )
        .route("/api/bridge/start", post(im_api::start_bridge))
        .route("/api/bridge/stop", post(im_api::stop_bridge))
        .route(
            "/api/im-channel/enabled",
            post(im_api::set_im_channel_enabled),
        )
        .route("/api/im/accounts", get(im_api::im_accounts))
        .route(
            "/api/im/account/enabled",
            post(im_api::set_im_account_enabled),
        )
        .route("/api/im/account/delete", post(im_api::delete_im_account))
        .route(
            "/api/remote-control/backend-status",
            get(remote_control_backend_status),
        )
        .route(
            "/api/feishu/onboard/start",
            post(onboarding::feishu_onboard_start),
        )
        .route(
            "/api/feishu/onboard/poll",
            post(onboarding::feishu_onboard_poll),
        )
        .route("/api/feishu/bot", get(im_api::feishu_bot_status))
        .route("/api/telegram/bot", get(im_api::telegram_bot_status))
        .route(
            "/api/telegram/configure",
            post(im_api::configure_telegram_bot),
        )
        .route(
            "/api/wechat/onboard/start",
            post(onboarding::wechat_onboard_start),
        )
        .route(
            "/api/wechat/onboard/poll",
            post(onboarding::wechat_onboard_poll),
        )
        .route("/api/wechat/bot", get(im_api::wechat_bot_status))
        .route(
            "/api/wecom/onboard/start",
            post(onboarding::wecom_onboard_start),
        )
        .route(
            "/api/wecom/onboard/poll",
            post(onboarding::wecom_onboard_poll),
        )
        .route("/api/wecom/bot", get(im_api::wecom_bot_status))
        .route("/api/events", get(events))
        .merge(plugins::router())
        .merge(remote_control_backend::router())
        .nest("/ai-gateway", crate::ai_gateway::router())
        .layer(middleware::from_fn(access_log))
        .with_state(state)
}

async fn access_log(request: Request<Body>, next: Next) -> impl IntoResponse {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let started = std::time::Instant::now();
    let mut response = next.run(request).await;
    let status = response.status();
    let elapsed_ms = started.elapsed().as_millis();
    if path.starts_with("/backend-api/") || path.starts_with("/api/") {
        let headers = response.headers_mut();
        headers.insert(
            CACHE_CONTROL,
            HeaderValue::from_static("no-store, no-cache, max-age=0, must-revalidate"),
        );
        headers.insert(PRAGMA, HeaderValue::from_static("no-cache"));
        headers.insert(EXPIRES, HeaderValue::from_static("0"));
    }
    chain_log::write_line(format!(
        "[http] method={} path={} status={} elapsed_ms={}",
        method,
        path,
        status.as_u16(),
        elapsed_ms
    ));
    tracing::info!(
        target: "threadrelay::http",
        method = %method,
        path,
        status = status.as_u16(),
        elapsed_ms,
        "http request"
    );
    response
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StatusResponse {
    service: String,
    pid: u32,
    instance_id: String,
    started_at_ms: u64,
    running: bool,
    bind: String,
    local_connection_mode: crate::config::LocalConnectionMode,
    outbound_proxy_mode: crate::config::OutboundProxyMode,
    state_path: String,
    feishu_ws: FeishuWsState,
    telegram: TelegramState,
    wechat: WechatState,
    im_accounts: Vec<ImAccountRuntimeState>,
}

async fn status(State(state): State<SharedState>) -> Json<StatusResponse> {
    Json(status_snapshot(&state).await)
}

async fn status_snapshot(state: &SharedState) -> StatusResponse {
    let running = state
        .bridge_task
        .lock()
        .await
        .as_ref()
        .map(|handle| !handle.is_finished())
        .unwrap_or(false);
    let config = state.config.lock().await;
    let feishu_ws = state.feishu_ws.lock().await.clone();
    let telegram = state.telegram.lock().await.clone();
    let wechat = state.wechat.lock().await.clone();
    let im_accounts = state
        .im_accounts
        .lock()
        .await
        .values()
        .cloned()
        .collect::<Vec<_>>();
    StatusResponse {
        service: state.daemon_identity.service.clone(),
        pid: state.daemon_identity.pid,
        instance_id: state.daemon_identity.instance_id.clone(),
        started_at_ms: state.daemon_identity.started_at_ms,
        running,
        bind: config.bind.clone(),
        local_connection_mode: config.local_connection_mode,
        outbound_proxy_mode: config.outbound_proxy.mode,
        state_path: config.state_path.to_string_lossy().to_string(),
        feishu_ws,
        telegram,
        wechat,
        im_accounts,
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GuiDashboardResponse {
    status: StatusResponse,
    remote: remote_control_backend::RemoteControlStatusResponse,
    codex_app: codex_app_config::CodexAppConfigStatus,
    im_accounts: im_api::ImAccountsResponse,
    ai_gateway: crate::ai_gateway::config::AiGatewayConfig,
}

async fn gui_dashboard(State(state): State<SharedState>) -> Json<GuiDashboardResponse> {
    let status = status_snapshot(&state).await;
    let remote = remote_control_backend::status_snapshot(&state).await;
    let codex_app = codex_app::codex_app_status_snapshot(&state).await;
    let im_accounts = im_api::im_accounts_snapshot(&state).await;
    let ai_gateway = state.config.lock().await.ai_gateway.clone();
    Json(GuiDashboardResponse {
        status,
        remote,
        codex_app,
        im_accounts,
        ai_gateway,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ManageDashboardResponse {
    service: manage_api::ManageStatusResponse,
    bridge_running: bool,
    remote_control_connected: bool,
    remote_control_healthy: bool,
    codex_app_configured: bool,
    im_account_count: usize,
    connected_im_account_count: usize,
    ai_gateway_enabled: bool,
    ai_gateway_provider_count: usize,
    request_logging_enabled: bool,
}

async fn manage_dashboard(State(state): State<SharedState>) -> Json<ManageDashboardResponse> {
    let service = manage_api::status_snapshot(&state);
    let bridge_running = state
        .bridge_task
        .lock()
        .await
        .as_ref()
        .is_some_and(|handle| !handle.is_finished());
    let remote = remote_control_backend::status_snapshot(&state).await;
    let codex_app = codex_app::codex_app_status_snapshot(&state).await;
    let im_accounts = state.im_accounts.lock().await;
    let im_account_count = im_accounts.len();
    let connected_im_account_count = im_accounts
        .values()
        .filter(|account| account.connected)
        .count();
    drop(im_accounts);
    let config = state.config.lock().await;

    Json(ManageDashboardResponse {
        service,
        bridge_running,
        remote_control_connected: remote.connected,
        remote_control_healthy: remote.healthy,
        codex_app_configured: codex_app.configured,
        im_account_count,
        connected_im_account_count,
        ai_gateway_enabled: config.ai_gateway.enabled,
        ai_gateway_provider_count: config.ai_gateway.providers.len(),
        request_logging_enabled: config.ai_gateway.request_logging_enabled,
    })
}

async fn shutdown(State(state): State<SharedState>) -> impl IntoResponse {
    perform_shutdown(&state, "daemon shutdown requested").await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InstanceShutdownRequest {
    daemon_instance_id: String,
}

async fn shutdown_instance(
    State(state): State<SharedState>,
    Json(request): Json<InstanceShutdownRequest>,
) -> impl IntoResponse {
    if request.daemon_instance_id.trim() != state.daemon_identity.instance_id {
        return (
            StatusCode::CONFLICT,
            Json(json!({
                "ok": false,
                "error": "daemon instance id does not match the active service",
            })),
        );
    }
    perform_shutdown(&state, "instance-guarded daemon shutdown requested").await
}

async fn perform_shutdown(
    state: &SharedState,
    event_message: &'static str,
) -> (StatusCode, Json<serde_json::Value>) {
    state
        .push_event("warn", "shutdown_requested", event_message)
        .await;
    im_api::stop_bridge_task(&state).await;
    let accepted = state.request_shutdown().await;
    (
        StatusCode::OK,
        Json(json!({ "ok": true, "accepted": accepted })),
    )
}

async fn get_config(State(state): State<SharedState>) -> Json<AppConfig> {
    Json(state.config.lock().await.clone())
}

async fn save_config(
    State(state): State<SharedState>,
    Json(config): Json<AppConfig>,
) -> impl IntoResponse {
    if let Err(err) = crate::outbound_http::validate_for_local_port(
        &config.outbound_proxy,
        config.local_listen_port(),
    ) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": err.to_string() })),
        );
    }
    if let Err(err) = config.save(&state.config_path) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": err.to_string() })),
        );
    }
    if let Err(err) = crate::outbound_http::init(&config.outbound_proxy, config.local_listen_port())
    {
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RemoteControlBackendStatusResponse {
    available: bool,
    enabled: bool,
    remote_control_base_url: String,
    remote_control_connected: bool,
    remote_control_initialized: bool,
    server_name: Option<String>,
    environment_id: Option<String>,
    installation_id: Option<String>,
    current_thread_id: Option<String>,
    feishu_configured: bool,
    telegram_configured: bool,
    wechat_configured: bool,
    wecom_configured: bool,
    reason: Option<String>,
}

async fn remote_control_backend_status(
    State(state): State<SharedState>,
) -> Json<RemoteControlBackendStatusResponse> {
    let config = state.config.lock().await.clone();
    let remote = remote_control_backend::status_snapshot(&state).await;
    let feishu_configured = im_api::feishu_configured(&config);
    let telegram_configured = im_api::telegram_configured(&config);
    let wechat_configured = im_api::wechat_configured(&config);
    let wecom_configured = im_api::wecom_configured(&config);
    let im_configured = im_api::im_bridge_configured(&config);
    let reason = if !config.bridge.enabled {
        Some("bridge disabled".to_string())
    } else if !im_configured {
        Some("No enabled IM channel is configured".to_string())
    } else {
        None
    };
    Json(RemoteControlBackendStatusResponse {
        available: config.bridge.enabled && im_configured,
        enabled: config.bridge.enabled,
        remote_control_base_url: config.remote_control_base_url(),
        remote_control_connected: remote.connected,
        remote_control_initialized: remote.initialized,
        server_name: remote.server_name,
        environment_id: remote.environment_id,
        installation_id: remote.installation_id,
        current_thread_id: remote.current_thread_id,
        feishu_configured,
        telegram_configured,
        wechat_configured,
        wecom_configured,
        reason,
    })
}

async fn events(State(state): State<SharedState>) -> impl IntoResponse {
    let events = state.events.lock().await.clone();
    Json(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::to_bytes,
        http::{Method, header::AUTHORIZATION},
    };
    use serde_json::Value;
    use tempfile::TempDir;
    use tower::ServiceExt;

    use crate::{
        ai_gateway::config::ProviderConfig, app_state::AppState, daemon_process::DaemonIdentity,
    };

    const CANARY_PROVIDER_KEY: &str = "canary-provider-key-must-not-leak";
    const CANARY_PROVIDER_URL: &str = "https://canary-provider.invalid/v1";
    const CANARY_MODELS_URL: &str = "https://canary-provider.invalid/v1/models";
    const CANARY_MODEL: &str = "canary-model-must-not-leak";
    const CANARY_STATE_PATH: &str = "canary-private-state-path-must-not-leak.json";

    fn management_test_router() -> (Router, TempDir, String) {
        let temp = tempfile::tempdir().expect("tempdir");
        let config_path = temp.path().join("user-domain/config.toml");
        let mut config = AppConfig::default();
        config.state_path = temp.path().join(CANARY_STATE_PATH);
        config.ai_gateway.enabled = true;
        config.ai_gateway.request_logging_enabled = true;
        config.ai_gateway.providers = vec![ProviderConfig {
            name: "canary-provider-name-must-not-leak".to_string(),
            base_url: CANARY_PROVIDER_URL.to_string(),
            models_url: Some(CANARY_MODELS_URL.to_string()),
            api_key: CANARY_PROVIDER_KEY.to_string(),
            models: vec![CANARY_MODEL.to_string()],
            ..ProviderConfig::default()
        }];
        let state = AppState::new(config_path.clone(), config, None, None);
        let app = router(state);
        let control: Value = serde_json::from_slice(
            &std::fs::read(manage_api::control_file_path(&config_path))
                .expect("read management control file"),
        )
        .expect("parse management control file");
        let token = control
            .get("managementToken")
            .and_then(Value::as_str)
            .expect("management token")
            .to_string();
        (app, temp, token)
    }

    async fn route_response(
        app: Router,
        path: &str,
        bearer: Option<&str>,
    ) -> axum::response::Response {
        let mut builder = Request::builder().method(Method::GET).uri(path);
        if let Some(token) = bearer {
            builder = builder.header(AUTHORIZATION, format!("Bearer {token}"));
        }
        app.oneshot(builder.body(Body::empty()).expect("request"))
            .await
            .expect("route response")
    }

    async fn response_json(response: axum::response::Response) -> Value {
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body");
        serde_json::from_slice(&body).expect("JSON response")
    }

    #[tokio::test]
    async fn healthz_route_is_public_and_strictly_minimal() {
        let (app, _temp, _token) = management_test_router();
        let response = route_response(app, "/healthz", None).await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response_json(response).await,
            json!({
                "service": "threadrelay",
                "apiMajor": 1,
                "ready": true,
            })
        );
    }

    #[tokio::test]
    async fn manage_route_requires_the_shared_bearer_credential() {
        let (app, _temp, token) = management_test_router();

        let missing = route_response(app.clone(), "/api/v1/manage/status", None).await;
        assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);

        let wrong = route_response(
            app.clone(),
            "/api/v1/manage/status",
            Some("wrong-management-token"),
        )
        .await;
        assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);

        let valid = route_response(app, "/api/v1/manage/status", Some(&token)).await;
        assert_eq!(valid.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn manage_dashboard_exposes_only_aggregate_non_secret_state() {
        let (app, temp, token) = management_test_router();
        let response = route_response(app, "/api/v1/manage/dashboard", Some(&token)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let dashboard = response_json(response).await;

        let object = dashboard.as_object().expect("dashboard object");
        let mut keys = object.keys().map(String::as_str).collect::<Vec<_>>();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "aiGatewayEnabled",
                "aiGatewayProviderCount",
                "bridgeRunning",
                "codexAppConfigured",
                "connectedImAccountCount",
                "imAccountCount",
                "remoteControlConnected",
                "remoteControlHealthy",
                "requestLoggingEnabled",
                "service",
            ]
        );

        let service = object
            .get("service")
            .and_then(Value::as_object)
            .expect("service status object");
        let mut service_keys = service.keys().map(String::as_str).collect::<Vec<_>>();
        service_keys.sort_unstable();
        assert_eq!(
            service_keys,
            vec![
                "apiMajor",
                "instanceId",
                "pid",
                "ready",
                "service",
                "startedAtMs",
            ]
        );

        let encoded = serde_json::to_string(&dashboard).expect("serialize dashboard");
        for secret in [
            CANARY_PROVIDER_KEY,
            CANARY_PROVIDER_URL,
            CANARY_MODELS_URL,
            CANARY_MODEL,
            CANARY_STATE_PATH,
            "canary-provider-name-must-not-leak",
        ] {
            assert!(!encoded.contains(secret), "dashboard leaked {secret}");
        }
        let mut string_values = Vec::new();
        collect_json_strings(&dashboard, &mut string_values);
        assert!(
            string_values.iter().all(|value| !value.contains("://")),
            "dashboard exposed a URL: {string_values:?}"
        );
        let temp_path = temp.path().to_string_lossy();
        assert!(
            string_values
                .iter()
                .all(|value| !value.contains(temp_path.as_ref())),
            "dashboard exposed a local path: {string_values:?}"
        );
        for forbidden_field in [
            "apiKey",
            "baseUrl",
            "modelsUrl",
            "models",
            "model",
            "statePath",
            "configPath",
            "config",
            "providers",
        ] {
            assert!(
                !contains_json_key(&dashboard, forbidden_field),
                "dashboard exposed field {forbidden_field}"
            );
        }
    }

    fn collect_json_strings<'a>(value: &'a Value, output: &mut Vec<&'a str>) {
        match value {
            Value::String(value) => output.push(value),
            Value::Object(object) => {
                for value in object.values() {
                    collect_json_strings(value, output);
                }
            }
            Value::Array(values) => {
                for value in values {
                    collect_json_strings(value, output);
                }
            }
            _ => {}
        }
    }

    fn contains_json_key(value: &Value, expected: &str) -> bool {
        match value {
            Value::Object(object) => object
                .iter()
                .any(|(key, value)| key == expected || contains_json_key(value, expected)),
            Value::Array(values) => values
                .iter()
                .any(|value| contains_json_key(value, expected)),
            _ => false,
        }
    }

    #[tokio::test]
    async fn instance_guarded_shutdown_rejects_stale_daemon_before_accepting_current() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut config = AppConfig::default();
        config.state_path = temp.path().join("state.json");
        let identity = DaemonIdentity::new();
        let instance_id = identity.instance_id.clone();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        let state = AppState::new(
            temp.path().join("config.toml"),
            config,
            Some(shutdown_tx),
            Some(identity),
        );

        let stale = shutdown_instance(
            State(state.clone()),
            Json(InstanceShutdownRequest {
                daemon_instance_id: "stale-instance".to_string(),
            }),
        )
        .await
        .into_response();
        assert_eq!(stale.status(), StatusCode::CONFLICT);

        let accepted = shutdown_instance(
            State(state),
            Json(InstanceShutdownRequest {
                daemon_instance_id: instance_id,
            }),
        )
        .await
        .into_response();
        assert_eq!(accepted.status(), StatusCode::OK);
        tokio::time::timeout(std::time::Duration::from_secs(1), shutdown_rx)
            .await
            .expect("shutdown signal timeout")
            .expect("shutdown signal sender");
    }
}
