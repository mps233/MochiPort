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
    app_state::{
        FeishuWsState, ImAccountRuntimeState, RemoteControlSourceKind, SharedState, TelegramState,
        WechatState, im_account_key,
    },
    chain_log, codex_app_config,
    config::AppConfig,
    manage_api, remote_control_backend,
    types::ImPlatformKind,
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
        .route("/lifecycle", get(manage_lifecycle))
        .route("/dashboard", get(manage_dashboard))
        .route("/log-directory", get(manage_log_directory))
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

async fn manage_lifecycle(State(state): State<SharedState>) -> Json<manage_api::LifecycleResponse> {
    Json(manage_api::lifecycle_snapshot(&state).await)
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
    // Keep the original v1 aggregate fields during the additive schema
    // transition. Older clients ignore the newer nested sections, while
    // existing clients continue to receive the fields they already decode.
    codex_app_configured: bool,
    im_account_count: usize,
    connected_im_account_count: usize,
    execution_clients: ManageExecutionClients,
    message_channels: ManageMessageChannels,
    ai_gateway_enabled: bool,
    ai_gateway_provider_count: usize,
    request_logging_enabled: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ManageExecutionClients {
    codex_app: ManageExecutionClient,
    vscode: ManageExecutionClient,
    cli: ManageExecutionClient,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ManageExecutionClient {
    configured: bool,
    connected: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ManageMessageChannels {
    telegram: ManageMessageChannel,
    feishu: ManageMessageChannel,
    wechat: ManageMessageChannel,
    wecom: ManageMessageChannel,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ManageMessageChannel {
    account_count: usize,
    connected_account_count: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ManageLogDirectoryResponse {
    directory: String,
    instance_id: String,
}

fn remote_source_status(
    remote: &remote_control_backend::RemoteControlStatusResponse,
    source_kind: RemoteControlSourceKind,
) -> (bool, bool) {
    remote_source_status_from(
        remote
            .connections
            .iter()
            .map(|connection| (connection.source_kind, connection.healthy)),
        source_kind,
    )
}

fn remote_source_status_from(
    connections: impl IntoIterator<Item = (RemoteControlSourceKind, bool)>,
    source_kind: RemoteControlSourceKind,
) -> (bool, bool) {
    let mut configured = false;
    let mut connected = false;
    for (connection_source, healthy) in connections {
        if connection_source != source_kind {
            continue;
        }
        configured = true;
        connected |= healthy;
    }
    (configured, connected)
}

fn im_account_counts<T>(
    platform: ImPlatformKind,
    accounts: &[T],
    account_id: impl Fn(&T) -> &str,
    runtime: &std::collections::HashMap<String, ImAccountRuntimeState>,
) -> (usize, usize) {
    let connected = accounts
        .iter()
        .filter(|account| {
            runtime
                .get(&im_account_key(platform, account_id(account)))
                .is_some_and(|state| state.connected)
        })
        .count();
    (accounts.len(), connected)
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
    let (_, codex_app_connected) = remote_source_status(&remote, RemoteControlSourceKind::CodexApp);
    let (vscode_configured, vscode_connected) =
        remote_source_status(&remote, RemoteControlSourceKind::Vscode);
    let (cli_configured, cli_connected) =
        remote_source_status(&remote, RemoteControlSourceKind::Cli);
    let config = state.config.lock().await.clone();
    let feishu_accounts = config.effective_feishu_accounts();
    let telegram_accounts = config.effective_telegram_accounts();
    let wechat_accounts = config.effective_wechat_accounts();
    let wecom_accounts = config.effective_wecom_accounts();
    let runtime = state.im_accounts.lock().await;
    let (feishu_account_count, feishu_connected_account_count) = im_account_counts(
        ImPlatformKind::Feishu,
        &feishu_accounts,
        |account| account.account_id.as_str(),
        &runtime,
    );
    let (telegram_account_count, telegram_connected_account_count) = im_account_counts(
        ImPlatformKind::Telegram,
        &telegram_accounts,
        |account| account.account_id.as_str(),
        &runtime,
    );
    let (wechat_account_count, wechat_connected_account_count) = im_account_counts(
        ImPlatformKind::Wechat,
        &wechat_accounts,
        |account| account.account_id.as_str(),
        &runtime,
    );
    let (wecom_account_count, wecom_connected_account_count) = im_account_counts(
        ImPlatformKind::Wecom,
        &wecom_accounts,
        |account| account.account_id.as_str(),
        &runtime,
    );
    let legacy_im_account_count = runtime.len();
    let legacy_connected_im_account_count =
        runtime.values().filter(|account| account.connected).count();
    drop(runtime);
    Json(ManageDashboardResponse {
        service,
        bridge_running,
        remote_control_connected: remote.connected,
        remote_control_healthy: remote.healthy,
        codex_app_configured: codex_app.configured,
        im_account_count: legacy_im_account_count,
        connected_im_account_count: legacy_connected_im_account_count,
        execution_clients: ManageExecutionClients {
            codex_app: ManageExecutionClient {
                configured: codex_app.configured,
                connected: codex_app_connected,
            },
            vscode: ManageExecutionClient {
                configured: vscode_configured,
                connected: vscode_connected,
            },
            cli: ManageExecutionClient {
                configured: cli_configured,
                connected: cli_connected,
            },
        },
        message_channels: ManageMessageChannels {
            telegram: ManageMessageChannel {
                account_count: telegram_account_count,
                connected_account_count: telegram_connected_account_count,
            },
            feishu: ManageMessageChannel {
                account_count: feishu_account_count,
                connected_account_count: feishu_connected_account_count,
            },
            wechat: ManageMessageChannel {
                account_count: wechat_account_count,
                connected_account_count: wechat_connected_account_count,
            },
            wecom: ManageMessageChannel {
                account_count: wecom_account_count,
                connected_account_count: wecom_connected_account_count,
            },
        },
        ai_gateway_enabled: config.ai_gateway.enabled,
        ai_gateway_provider_count: config.ai_gateway.providers.len(),
        request_logging_enabled: config.ai_gateway.request_logging_enabled,
    })
}

async fn manage_log_directory(
    State(state): State<SharedState>,
) -> Json<ManageLogDirectoryResponse> {
    let config = state.config.lock().await;
    let directory = config
        .state_path
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("logs");
    Json(ManageLogDirectoryResponse {
        directory: directory.to_string_lossy().into_owned(),
        instance_id: state.daemon_identity.instance_id.clone(),
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
    use std::collections::HashMap;
    use tempfile::TempDir;
    use tower::ServiceExt;

    use crate::{
        ai_gateway::config::ProviderConfig,
        app_state::{AppState, ImAccountRuntimeState, RemoteControlServerConnection},
        config::{FeishuConfig, TelegramConfig, WechatConfig, WecomConfig},
        daemon_process::DaemonIdentity,
        types::ImPlatformKind,
    };

    const CANARY_PROVIDER_KEY: &str = "canary-provider-key-must-not-leak";
    const CANARY_PROVIDER_URL: &str = "https://canary-provider.invalid/v1";
    const CANARY_MODELS_URL: &str = "https://canary-provider.invalid/v1/models";
    const CANARY_MODEL: &str = "canary-model-must-not-leak";
    const CANARY_STATE_PATH: &str = "canary-private-state-path-must-not-leak.json";

    fn management_test_state() -> (SharedState, TempDir, String) {
        let temp = tempfile::tempdir().expect("tempdir");
        let config_path = temp.path().join("user-domain/config.toml");
        let mut config = AppConfig::default();
        config.state_path = temp.path().join(CANARY_STATE_PATH);
        config.feishu_accounts = vec![FeishuConfig {
            account_id: "canary-feishu-account-must-not-leak".to_string(),
            app_id: "canary-feishu-app-id-must-not-leak".to_string(),
            app_secret: "canary-feishu-secret-must-not-leak".to_string(),
            display_name: "canary-feishu-name-must-not-leak".to_string(),
            ..FeishuConfig::default()
        }];
        config.telegram_accounts = vec![
            TelegramConfig {
                account_id: "canary-telegram-connected-must-not-leak".to_string(),
                bot_token: "canary-telegram-token-one-must-not-leak".to_string(),
                display_name: "canary-telegram-name-one-must-not-leak".to_string(),
                ..TelegramConfig::default()
            },
            TelegramConfig {
                account_id: "canary-telegram-offline-must-not-leak".to_string(),
                bot_token: "canary-telegram-token-two-must-not-leak".to_string(),
                display_name: "canary-telegram-name-two-must-not-leak".to_string(),
                ..TelegramConfig::default()
            },
        ];
        config.wechat_accounts = vec![WechatConfig {
            account_id: "canary-wechat-account-must-not-leak".to_string(),
            bot_token: "canary-wechat-token-must-not-leak".to_string(),
            display_name: "canary-wechat-name-must-not-leak".to_string(),
            base_url: "https://canary-wechat.invalid".to_string(),
            user_id: "canary-wechat-user-must-not-leak".to_string(),
            ..WechatConfig::default()
        }];
        config.wecom_accounts = vec![WecomConfig {
            account_id: "canary-wecom-account-must-not-leak".to_string(),
            bot_id: "canary-wecom-bot-id-must-not-leak".to_string(),
            secret: "canary-wecom-secret-must-not-leak".to_string(),
            display_name: "canary-wecom-name-must-not-leak".to_string(),
            websocket_url: "wss://canary-wecom.invalid".to_string(),
            ..WecomConfig::default()
        }];
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
        manage_api::ensure_management_token(&config_path).expect("create management control file");
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
        (state, temp, token)
    }

    fn management_test_router() -> (Router, TempDir, String) {
        let (state, temp, token) = management_test_state();
        (router(state), temp, token)
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
        let (state, temp, token) = management_test_state();
        {
            let mut runtime = state.im_accounts.lock().await;
            let mut telegram = ImAccountRuntimeState::new(
                ImPlatformKind::Telegram,
                "canary-telegram-connected-must-not-leak",
            );
            telegram.connected = true;
            runtime.insert(
                im_account_key(
                    ImPlatformKind::Telegram,
                    "canary-telegram-connected-must-not-leak",
                ),
                telegram,
            );
            let mut wechat = ImAccountRuntimeState::new(
                ImPlatformKind::Wechat,
                "canary-wechat-account-must-not-leak",
            );
            wechat.connected = true;
            runtime.insert(
                im_account_key(
                    ImPlatformKind::Wechat,
                    "canary-wechat-account-must-not-leak",
                ),
                wechat,
            );
            let mut orphan = ImAccountRuntimeState::new(
                ImPlatformKind::Wecom,
                "canary-orphan-runtime-must-not-leak",
            );
            orphan.connected = true;
            runtime.insert(
                im_account_key(ImPlatformKind::Wecom, "canary-orphan-runtime-must-not-leak"),
                orphan,
            );
        }
        {
            let mut remote = state.remote_control.inner.lock().await;
            let (vscode_tx, _vscode_rx) = tokio::sync::mpsc::unbounded_channel();
            remote.connections.insert(
                "canary-vscode-connection-must-not-leak".to_string(),
                test_remote_connection(
                    "canary-vscode-connection-must-not-leak",
                    RemoteControlSourceKind::Vscode,
                    true,
                    Some(vscode_tx),
                ),
            );
            let (cli_tx, _cli_rx) = tokio::sync::mpsc::unbounded_channel();
            remote.connections.insert(
                "canary-cli-connection-must-not-leak".to_string(),
                test_remote_connection(
                    "canary-cli-connection-must-not-leak",
                    RemoteControlSourceKind::Cli,
                    false,
                    Some(cli_tx),
                ),
            );
        }
        let app = router(state);
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
                "executionClients",
                "imAccountCount",
                "messageChannels",
                "remoteControlConnected",
                "remoteControlHealthy",
                "requestLoggingEnabled",
                "service",
            ]
        );

        let execution_clients = object
            .get("executionClients")
            .and_then(Value::as_object)
            .expect("execution clients object");
        assert_exact_keys(execution_clients, &["cli", "codexApp", "vscode"]);
        for client in ["cli", "codexApp", "vscode"] {
            let status = execution_clients[client]
                .as_object()
                .expect("execution client status object");
            assert_exact_keys(status, &["configured", "connected"]);
        }
        assert_eq!(execution_clients["codexApp"]["connected"], json!(false));
        assert_eq!(execution_clients["vscode"]["configured"], json!(true));
        assert_eq!(execution_clients["vscode"]["connected"], json!(true));
        assert_eq!(execution_clients["cli"]["configured"], json!(true));
        assert_eq!(execution_clients["cli"]["connected"], json!(false));
        assert_eq!(object["codexAppConfigured"], json!(true));
        assert_eq!(object["imAccountCount"], json!(3));
        assert_eq!(object["connectedImAccountCount"], json!(3));

        let message_channels = object
            .get("messageChannels")
            .and_then(Value::as_object)
            .expect("message channels object");
        assert_exact_keys(message_channels, &["feishu", "telegram", "wechat", "wecom"]);
        for platform in ["feishu", "telegram", "wechat", "wecom"] {
            let status = message_channels[platform]
                .as_object()
                .expect("message channel status object");
            assert_exact_keys(status, &["accountCount", "connectedAccountCount"]);
        }
        assert_eq!(message_channels["feishu"]["accountCount"], json!(1));
        assert_eq!(
            message_channels["feishu"]["connectedAccountCount"],
            json!(0)
        );
        assert_eq!(message_channels["telegram"]["accountCount"], json!(2));
        assert_eq!(
            message_channels["telegram"]["connectedAccountCount"],
            json!(1)
        );
        assert_eq!(message_channels["wechat"]["accountCount"], json!(1));
        assert_eq!(
            message_channels["wechat"]["connectedAccountCount"],
            json!(1)
        );
        assert_eq!(message_channels["wecom"]["accountCount"], json!(1));
        assert_eq!(message_channels["wecom"]["connectedAccountCount"], json!(0));

        let service = object
            .get("service")
            .and_then(Value::as_object)
            .expect("service status object");
        assert_exact_keys(
            service,
            &[
                "apiMajor",
                "instanceId",
                "pid",
                "ready",
                "service",
                "startedAtMs",
            ],
        );

        let encoded = serde_json::to_string(&dashboard).expect("serialize dashboard");
        for secret in [
            CANARY_PROVIDER_KEY,
            CANARY_PROVIDER_URL,
            CANARY_MODELS_URL,
            CANARY_MODEL,
            CANARY_STATE_PATH,
            "canary-provider-name-must-not-leak",
            "canary-feishu",
            "canary-telegram",
            "canary-wechat",
            "canary-wecom",
            "canary-orphan",
            "canary-vscode",
            "canary-cli",
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
            "accountId",
            "displayName",
            "appId",
            "appSecret",
            "botId",
            "botToken",
            "secret",
            "websocketUrl",
            "userId",
            "allowedOpenIds",
            "allowedChatIds",
            "allowedUserIds",
            "lastError",
        ] {
            assert!(
                !contains_json_key(&dashboard, forbidden_field),
                "dashboard exposed field {forbidden_field}"
            );
        }
    }

    #[tokio::test]
    async fn manage_log_directory_follows_the_normalized_state_path() {
        let (state, temp, token) = management_test_state();
        let expected = state
            .config
            .lock()
            .await
            .state_path
            .parent()
            .expect("state directory")
            .join("logs")
            .to_string_lossy()
            .into_owned();
        let instance_id = state.daemon_identity.instance_id.clone();
        let app = router(state);

        let unauthorized = route_response(app.clone(), "/api/v1/manage/log-directory", None).await;
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

        let response = route_response(app, "/api/v1/manage/log-directory", Some(&token)).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response_json(response).await,
            json!({
                "directory": expected,
                "instanceId": instance_id,
            })
        );
        drop(temp);
    }

    #[tokio::test]
    async fn lifecycle_route_is_authenticated_and_reports_read_only_snapshot() {
        let (state, temp, token) = management_test_state();
        {
            let mut runtime = state.runtime.lock().await;
            runtime
                .starting_turn_by_thread
                .insert("thread-starting".to_string());
            runtime
                .current_turn_by_thread
                .insert("thread-running".to_string(), "turn-1".to_string());
            runtime
                .pending_approval_request_keys
                .insert("approval-1".to_string());
        }
        {
            let mut remote = state.remote_control.inner.lock().await;
            let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
            let mut connection = test_remote_connection(
                "lifecycle-connection",
                RemoteControlSourceKind::CodexApp,
                true,
                Some(tx),
            );
            connection.clients.insert(
                "client".to_string(),
                crate::app_state::RemoteControlClientState {
                    client_id: "client".to_string(),
                    stream_id: "stream".to_string(),
                    initialized: true,
                    next_seq_id: 1,
                    pending: std::collections::HashMap::from([(
                        "request".to_string(),
                        crate::app_state::PendingRemoteRequest {
                            connection_epoch: 1,
                            method: "turn/start".to_string(),
                            thread_id: None,
                            track_thread_active: true,
                            response_tx: tokio::sync::oneshot::channel().0,
                            message: json!({}),
                            envelopes: Vec::new(),
                        },
                    )]),
                    current_thread_id: None,
                    current_turn_id: None,
                    last_app_ping_at_ms: None,
                    last_app_pong_at_ms: None,
                    last_app_pong_status: None,
                    last_initialize_sent_at_ms: None,
                    recovery_attempt: 0,
                    recovery_started_at_ms: None,
                },
            );
            remote
                .connections
                .insert(connection.connection_id.clone(), connection);
        }

        let app = router(state.clone());
        let unauthorized = route_response(app.clone(), "/api/v1/manage/lifecycle", None).await;
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

        let response = route_response(app, "/api/v1/manage/lifecycle", Some(&token)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let lifecycle = response_json(response).await;
        assert_exact_keys(
            lifecycle.as_object().expect("lifecycle object"),
            &[
                "bind",
                "configPath",
                "executable",
                "management",
                "protectedWorkItems",
                "runtime",
                "service",
            ],
        );
        assert_eq!(lifecycle["service"]["service"], json!("threadrelay"));
        assert_eq!(lifecycle["service"]["apiMajor"], json!(1));
        assert_eq!(lifecycle["runtime"]["state"], json!("active"));
        assert_eq!(
            lifecycle["runtime"]["productVersion"],
            json!(env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(lifecycle["management"]["state"], json!("unmanaged"));
        assert_eq!(lifecycle["management"]["mode"], json!("readOnly"));
        assert_eq!(lifecycle["management"]["canControl"], json!(false));
        assert_eq!(lifecycle["protectedWorkItems"]["codexTurns"], json!(2));
        assert_eq!(
            lifecycle["protectedWorkItems"]["pendingApprovals"],
            json!(1)
        );
        assert_eq!(
            lifecycle["protectedWorkItems"]["remoteControlRequests"],
            json!(1)
        );
        assert_eq!(lifecycle["protectedWorkItems"]["total"], json!(4),);
        assert_eq!(
            lifecycle["configPath"],
            json!(
                temp.path()
                    .join("user-domain/config.toml")
                    .to_string_lossy()
            ),
        );
        assert!(
            lifecycle["executable"]
                .as_str()
                .is_some_and(|path| !path.is_empty())
        );
    }

    #[tokio::test]
    async fn lifecycle_route_reports_an_empty_protected_work_set() {
        let (state, _temp, token) = management_test_state();
        let app = router(state);

        let response = route_response(app, "/api/v1/manage/lifecycle", Some(&token)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let lifecycle = response_json(response).await;
        let protected = lifecycle["protectedWorkItems"]
            .as_object()
            .expect("protected work item object");
        assert_exact_keys(
            protected,
            &[
                "aiGatewayRequests",
                "codexTurns",
                "imStreams",
                "pendingApprovals",
                "remoteControlRequests",
                "total",
            ],
        );
        assert_eq!(
            lifecycle["protectedWorkItems"],
            json!({
                "aiGatewayRequests": 0,
                "codexTurns": 0,
                "imStreams": 0,
                "pendingApprovals": 0,
                "remoteControlRequests": 0,
                "total": 0,
            })
        );
    }

    fn assert_exact_keys(object: &serde_json::Map<String, Value>, expected: &[&str]) {
        let mut keys = object.keys().map(String::as_str).collect::<Vec<_>>();
        keys.sort_unstable();
        let mut expected = expected.to_vec();
        expected.sort_unstable();
        assert_eq!(keys, expected);
    }

    fn test_remote_connection(
        id: &str,
        source_kind: RemoteControlSourceKind,
        initialized: bool,
        outbound_tx: Option<
            tokio::sync::mpsc::UnboundedSender<crate::remote_control_backend::OutboundWsMessage>,
        >,
    ) -> RemoteControlServerConnection {
        RemoteControlServerConnection {
            connection_id: id.to_string(),
            connection_epoch: 1,
            default_client_key: "default".to_string(),
            connected: true,
            initialized,
            source_kind,
            user_agent: Some(format!("canary-user-agent-{id}-must-not-leak")),
            server_id: Some(format!("canary-server-{id}-must-not-leak")),
            environment_id: None,
            server_name: None,
            installation_id: None,
            account_id: None,
            subscribe_cursor: None,
            outbound_tx,
            connected_at_ms: Some(1),
            last_ws_inbound_at_ms: Some(1),
            last_ws_ping_at_ms: None,
            last_ws_pong_at_ms: None,
            last_error: Some(format!("canary-error-{id}-must-not-leak")),
            clients: HashMap::new(),
            stream_diagnostics: HashMap::new(),
        }
    }

    #[test]
    fn execution_client_status_separates_discovery_from_healthy_connection() {
        assert_eq!(
            remote_source_status_from([], RemoteControlSourceKind::Vscode),
            (false, false)
        );
        assert_eq!(
            remote_source_status_from(
                [(RemoteControlSourceKind::Vscode, false)],
                RemoteControlSourceKind::Vscode,
            ),
            (true, false)
        );
        assert_eq!(
            remote_source_status_from(
                [
                    (RemoteControlSourceKind::CodexApp, true),
                    (RemoteControlSourceKind::Vscode, true),
                ],
                RemoteControlSourceKind::Vscode,
            ),
            (true, true)
        );
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
