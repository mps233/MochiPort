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
mod manage_workspace;
mod oauth;
mod onboarding;
pub(crate) mod plugins;

pub async fn start_bridge_if_ready(state: &SharedState, event_message: &'static str) -> bool {
    im_api::start_bridge_task(state, im_api::BridgeStartMode::KeepExisting, event_message).await
}

/// Stop message polling/streaming before a lifecycle restart is committed.
/// Kept at the web boundary so the management API does not reach into the
/// private IM route implementation directly.
pub(crate) async fn stop_bridge_for_lifecycle_shutdown(state: &SharedState) {
    im_api::stop_bridge_task(state).await;
}

pub fn router(state: SharedState) -> Router {
    // Initialize the shared user-domain credential before serving requests.
    // Errors are surfaced as a protected-route 500 without exposing secrets;
    // legacy routes remain available during the migration window.
    let _ = manage_api::ensure_management_token(&state.config_path);
    let manage_routes = Router::new()
        .route("/status", get(manage_api::status))
        .route("/lifecycle", get(manage_lifecycle))
        .route(
            "/lifecycle/lease/claim",
            post(manage_api::claim_lifecycle_lease),
        )
        .route(
            "/lifecycle/lease/renew",
            post(manage_api::renew_lifecycle_lease),
        )
        .route(
            "/lifecycle/lease/release",
            post(manage_api::release_lifecycle_lease),
        )
        .route("/lifecycle/restart", post(manage_api::restart_lifecycle))
        .route("/dashboard", get(manage_dashboard))
        .route("/log-directory", get(manage_log_directory))
        .route("/codex/status", get(codex_app::manage_codex_app_status))
        .route("/codex/configure", post(codex_app::configure_codex_app))
        .route(
            "/codex/repair",
            post(codex_app::repair_codex_app_gui_environment),
        )
        .route("/codex/uninstall", post(codex_app::uninstall_codex_app))
        .route(
            "/codex/models/refresh",
            post(codex_app::refresh_codex_app_models),
        )
        .route(
            "/codex/models/catalog",
            get(manage_workspace::codex_models_catalog),
        )
        .route(
            "/codex/enhanced/preflight",
            get(codex_app::codex_app_enhanced_preflight),
        )
        .route(
            "/codex/enhanced/launch",
            post(codex_app::launch_codex_app_enhanced),
        )
        .route("/sessions", get(codex_app::codex_app_sessions))
        .route(
            "/sessions/provider",
            post(codex_app::move_managed_codex_app_session_provider),
        )
        .route("/gateway", get(manage_workspace::gateway))
        .route("/gateway/settings", post(manage_workspace::update_gateway))
        .route("/gateway/provider", post(manage_workspace::upsert_provider))
        .route(
            "/gateway/provider/delete",
            post(manage_workspace::delete_provider),
        )
        .route(
            "/gateway/provider/models/fetch",
            post(manage_workspace::fetch_provider_models),
        )
        .route(
            "/gateway/provider-templates",
            get(manage_workspace::provider_templates),
        )
        .route("/settings", get(manage_workspace::settings))
        .route("/settings", post(manage_workspace::update_settings))
        .route("/request-logs", get(manage_workspace::request_logs))
        .route(
            "/request-logs/clear",
            post(manage_workspace::clear_request_logs),
        )
        .route(
            "/request-logs/clear-old",
            post(manage_workspace::clear_old_request_logs),
        )
        .route(
            "/request-logs/{id}",
            get(manage_workspace::request_log_detail),
        )
        // IM account management is exposed under the authenticated, versioned
        // namespace.  Keep the legacy /api/im/* routes below during the
        // migration window, but make new clients use this surface.
        .route("/im/accounts", get(im_api::manage_im_accounts))
        .route("/im/account/enabled", post(im_api::set_im_account_enabled))
        .route("/im/account/delete", post(im_api::delete_im_account))
        .route(
            "/im/account/telegram",
            post(im_api::manage_configure_telegram_account),
        )
        .route(
            "/im/account/feishu",
            post(onboarding::manage_configure_feishu_account),
        )
        // Scan-based onboarding shares the legacy handlers wherever their
        // responses are already secret-free; only the Feishu poll needs the
        // sanitized manage variant because the legacy response echoes the raw
        // registration payload (including the issued app secret).
        .route(
            "/im/onboarding/feishu/start",
            post(onboarding::feishu_onboard_start),
        )
        .route(
            "/im/onboarding/feishu/poll",
            post(onboarding::manage_feishu_onboard_poll),
        )
        .route(
            "/im/onboarding/wechat/start",
            post(onboarding::wechat_onboard_start),
        )
        .route(
            "/im/onboarding/wechat/poll",
            post(onboarding::wechat_onboard_poll),
        )
        .route(
            "/im/onboarding/wecom/start",
            post(onboarding::wecom_onboard_start),
        )
        .route(
            "/im/onboarding/wecom/poll",
            post(onboarding::wecom_onboard_poll),
        )
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            manage_api::require_bearer,
        ));

    // Legacy GUI paths remain available during migration, but write-capable
    // IM operations share the same management credential as the versioned
    // API. This prevents the compatibility surface from bypassing bearer
    // authentication while the Rust GUI transitions to `/api/v1/manage/*`.
    let legacy_im_manage_routes = Router::new()
        .route("/api/bridge/start", post(im_api::start_bridge))
        .route("/api/bridge/stop", post(im_api::stop_bridge))
        .route(
            "/api/im-channel/enabled",
            post(im_api::set_im_channel_enabled),
        )
        .route(
            "/api/im/account/enabled",
            post(im_api::set_im_account_enabled),
        )
        .route("/api/im/account/delete", post(im_api::delete_im_account))
        .route(
            "/api/feishu/onboard/start",
            post(onboarding::feishu_onboard_start),
        )
        .route(
            "/api/feishu/onboard/poll",
            post(onboarding::feishu_onboard_poll),
        )
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
        .route(
            "/api/wecom/onboard/start",
            post(onboarding::wecom_onboard_start),
        )
        .route(
            "/api/wecom/onboard/poll",
            post(onboarding::wecom_onboard_poll),
        )
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            manage_api::require_bearer,
        ));

    // The compatibility shutdown endpoints predate the versioned management
    // API. Keep them available for older launchers, but require the same
    // local bearer credential so they cannot bypass lifecycle ownership.
    let legacy_shutdown_routes = Router::new()
        .route("/api/shutdown", post(shutdown))
        .route("/api/shutdown/instance", post(shutdown_instance))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            manage_api::require_bearer,
        ));

    Router::new()
        .route("/healthz", get(manage_api::healthz))
        .nest("/api/v1/manage", manage_routes)
        .merge(legacy_im_manage_routes)
        .route("/oauth/authorize", get(oauth::oauth_authorize))
        .route("/oauth/token", post(oauth::oauth_token))
        .route("/api/status", get(status))
        .route("/api/gui/dashboard", get(gui_dashboard))
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
        .route("/api/im/accounts", get(im_api::im_accounts))
        .route(
            "/api/remote-control/backend-status",
            get(remote_control_backend_status),
        )
        .route("/api/feishu/bot", get(im_api::feishu_bot_status))
        .route("/api/telegram/bot", get(im_api::telegram_bot_status))
        .route("/api/wechat/bot", get(im_api::wechat_bot_status))
        .route("/api/wecom/bot", get(im_api::wecom_bot_status))
        .route("/api/events", get(events))
        .merge(plugins::router())
        .merge(remote_control_backend::router())
        .merge(legacy_shutdown_routes)
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
    match manage_api::request_shutdown_with_drain(state, false, event_message).await {
        manage_api::LifecycleShutdownResult::Accepted => (
            StatusCode::OK,
            Json(json!({ "ok": true, "accepted": true })),
        ),
        manage_api::LifecycleShutdownResult::NotRunning => (
            StatusCode::OK,
            Json(json!({ "ok": true, "accepted": false })),
        ),
        manage_api::LifecycleShutdownResult::AlreadyInProgress => (
            StatusCode::CONFLICT,
            Json(json!({
                "ok": false,
                "accepted": false,
                "state": "draining",
                "error": "后台服务正在关闭或重启，请稍后重试。",
            })),
        ),
        manage_api::LifecycleShutdownResult::ProtectedWork(protected_work_items) => (
            StatusCode::CONFLICT,
            Json(json!({
                "ok": false,
                "accepted": false,
                "state": "active",
                "error": format!(
                    "后台服务仍有 {} 项受保护任务，已取消关闭。",
                    protected_work_items.total
                ),
                "protectedWorkItems": protected_work_items,
            })),
        ),
        manage_api::LifecycleShutdownResult::LeaseRejected(_) => (
            StatusCode::CONFLICT,
            Json(json!({
                "ok": false,
                "accepted": false,
                "error": "当前后台服务管理租约已失效。",
            })),
        ),
    }
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

    fn management_state_with_config(mut config: AppConfig) -> (SharedState, TempDir, String) {
        let temp = tempfile::tempdir().expect("tempdir");
        let config_path = temp.path().join("user-domain/config.toml");
        config.state_path = temp.path().join(CANARY_STATE_PATH);
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

    fn management_test_state() -> (SharedState, TempDir, String) {
        let mut config = AppConfig::default();
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
        management_state_with_config(config)
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
        request_response(app, Method::GET, path, bearer, None).await
    }

    async fn request_response(
        app: Router,
        method: Method,
        path: &str,
        bearer: Option<&str>,
        body: Option<&str>,
    ) -> axum::response::Response {
        let mut builder = Request::builder().method(method).uri(path);
        if let Some(token) = bearer {
            builder = builder.header(AUTHORIZATION, format!("Bearer {token}"));
        }
        let body = if let Some(body) = body {
            builder = builder.header("content-type", "application/json");
            Body::from(body.to_string())
        } else {
            Body::empty()
        };
        app.oneshot(builder.body(body).expect("request"))
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
    async fn disabled_ai_gateway_rejects_model_requests_before_routing() {
        let (state, _temp, _token) = management_state_with_config(AppConfig::default());
        let app = router(state);
        let response = request_response(
            app,
            Method::POST,
            "/ai-gateway/v1/responses",
            None,
            Some(r#"{"model":"unconfigured-model","input":[]}"#),
        )
        .await;

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "gateway_disabled");
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
    async fn legacy_shutdown_routes_require_the_shared_bearer_credential() {
        let (app, _temp, token) = management_test_router();
        let missing =
            request_response(app.clone(), Method::POST, "/api/shutdown", None, None).await;
        assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);

        let wrong = request_response(
            app.clone(),
            Method::POST,
            "/api/shutdown/instance",
            Some("wrong-management-token"),
            Some(r#"{"daemonInstanceId":"stale"}"#),
        )
        .await;
        assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);

        let accepted =
            request_response(app, Method::POST, "/api/shutdown", Some(&token), None).await;
        assert_eq!(accepted.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn manage_workspace_routes_require_the_shared_bearer_credential() {
        let (app, _temp, _token) = management_test_router();
        for path in [
            "/api/v1/manage/codex/status",
            "/api/v1/manage/sessions",
            "/api/v1/manage/gateway",
            "/api/v1/manage/settings",
            "/api/v1/manage/request-logs",
        ] {
            let response = route_response(app.clone(), path, None).await;
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "path={path}");
        }
    }

    #[tokio::test]
    async fn managed_session_move_rejects_gui_supplied_rollout_paths() {
        let (app, _temp, token) = management_test_router();
        let response = request_response(
            app,
            Method::POST,
            "/api/v1/manage/sessions/provider",
            Some(&token),
            Some(
                r#"{"threadId":"thread-canary","rolloutPath":"/tmp/canary.jsonl","targetProvider":"openai"}"#,
            ),
        )
        .await;

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn manage_gateway_and_settings_expose_state_without_secrets() {
        let mut config = AppConfig::default();
        config.outbound_proxy = crate::config::OutboundProxyConfig {
            mode: crate::config::OutboundProxyMode::Custom,
            url: "http://proxy-user:proxy-secret@127.0.0.1:7890".to_string(),
        };
        config.ai_gateway.providers = vec![ProviderConfig {
            name: "primary".to_string(),
            base_url: "https://provider-user:provider-password@provider.example/v1".to_string(),
            api_key: CANARY_PROVIDER_KEY.to_string(),
            models: vec!["model-a".to_string()],
            ..ProviderConfig::default()
        }];
        let (state, _temp, token) = management_state_with_config(config);
        let app = router(state);

        let gateway = route_response(app.clone(), "/api/v1/manage/gateway", Some(&token)).await;
        assert_eq!(gateway.status(), StatusCode::OK);
        let gateway = response_json(gateway).await;
        let gateway_text = gateway.to_string();
        assert_eq!(gateway["providers"][0]["name"], "primary");
        assert_eq!(gateway["providers"][0]["secretSet"], true);
        assert!(!gateway_text.contains(CANARY_PROVIDER_KEY));
        assert!(!gateway_text.contains("provider-password"));
        assert!(!gateway_text.contains("provider-user"));
        assert!(!gateway_text.contains("\"apiKey\""));

        let settings = route_response(app, "/api/v1/manage/settings", Some(&token)).await;
        assert_eq!(settings.status(), StatusCode::OK);
        let settings = response_json(settings).await;
        assert_eq!(settings["outboundProxy"]["url"], "http://127.0.0.1:7890");
        assert_eq!(settings["outboundProxy"]["credentialSet"], true);
        assert!(!settings.to_string().contains("proxy-secret"));
    }

    #[tokio::test]
    async fn manage_gateway_mutations_persist_and_keep_api_keys_write_only() {
        let (state, _temp, token) = management_state_with_config(AppConfig::default());
        let app = router(state);
        let key = "write-only-provider-key";
        let body = json!({
            "name": "primary",
            "enabled": true,
            "providerType": "open_ai_responses",
            "baseUrl": "https://provider.example/v1",
            "models": ["model-a", "model-a", " model-b "],
            "modelAliases": {},
            "weight": 100,
            "timeoutSecs": 60,
            "apiKey": key,
        })
        .to_string();
        let response = request_response(
            app.clone(),
            Method::POST,
            "/api/v1/manage/gateway/provider",
            Some(&token),
            Some(&body),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let response = response_json(response).await;
        assert_eq!(response["gateway"]["providers"][0]["secretSet"], true);
        assert_eq!(
            response["gateway"]["providers"][0]["models"],
            json!(["model-a", "model-b"])
        );
        assert!(!response.to_string().contains(key));

        let gateway = route_response(app, "/api/v1/manage/gateway", Some(&token)).await;
        let gateway = response_json(gateway).await;
        assert_eq!(gateway["providers"][0]["secretSet"], true);
        assert!(!gateway.to_string().contains(key));
    }

    #[tokio::test]
    async fn manage_gateway_edit_preserves_masked_legacy_provider_urls() {
        let legacy_base =
            "https://provider-user:provider-password@provider.example/v1?api_key=canary";
        let legacy_models =
            "https://models-user:models-password@provider.example/v1/models?token=canary";
        let mut config = AppConfig::default();
        config.ai_gateway.providers = vec![ProviderConfig {
            name: "primary".to_string(),
            base_url: legacy_base.to_string(),
            models_url: Some(legacy_models.to_string()),
            models: vec!["model-a".to_string()],
            ..ProviderConfig::default()
        }];
        let (state, _temp, token) = management_state_with_config(config);
        let inspect_state = state.clone();
        let app = router(state);
        let body = json!({
            "originalName": "primary",
            "name": "primary",
            "enabled": true,
            "providerType": "open_ai_responses",
            "baseUrl": "https://provider.example/v1",
            "modelsUrl": "https://provider.example/v1/models",
            "models": ["model-a"],
            "modelAliases": {},
            "weight": 200,
            "timeoutSecs": 60,
        })
        .to_string();

        let response = request_response(
            app,
            Method::POST,
            "/api/v1/manage/gateway/provider",
            Some(&token),
            Some(&body),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        let config = inspect_state.config.lock().await;
        let provider = &config.ai_gateway.providers[0];
        assert_eq!(provider.base_url, legacy_base);
        assert_eq!(provider.models_url.as_deref(), Some(legacy_models));
        assert_eq!(provider.weight, 200);
    }

    #[tokio::test]
    async fn manage_provider_templates_route_serves_static_camel_case_templates() {
        let (app, _temp, token) = management_test_router();

        let missing = route_response(
            app.clone(),
            "/api/v1/manage/gateway/provider-templates",
            None,
        )
        .await;
        assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);

        let response = route_response(
            app,
            "/api/v1/manage/gateway/provider-templates",
            Some(&token),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let payload = response_json(response).await;
        assert_exact_keys(
            payload.as_object().expect("templates response object"),
            &["templates"],
        );
        let templates = payload["templates"].as_array().expect("templates array");
        assert!(!templates.is_empty());
        for template in templates {
            let object = template.as_object().expect("template object");
            for key in ["id", "displayName", "providerType", "baseUrl", "models"] {
                assert!(object.contains_key(key), "template missing {key}");
            }
            assert!(
                object.keys().all(|key| !key.contains('_')),
                "template keys must be camelCase: {template}"
            );
        }

        let template_by_id = |id: &str| {
            templates
                .iter()
                .find(|template| template["id"] == id)
                .unwrap_or_else(|| panic!("missing template {id}"))
        };
        let openai = template_by_id("openai");
        assert_eq!(openai["displayName"], "OpenAI");
        assert_eq!(openai["providerType"], "open_ai_responses");
        assert_eq!(openai["baseUrl"], "https://api.openai.com/v1");
        assert_eq!(openai["models"], json!([]));

        let anthropic = template_by_id("anthropic");
        assert_eq!(anthropic["providerType"], "anthropic_messages");
        assert_eq!(anthropic["compatibility"], "anthropic");
        assert_eq!(anthropic["baseUrl"], "https://api.anthropic.com/v1");

        let deepseek_responses = template_by_id("deepseek-responses");
        assert_eq!(deepseek_responses["providerType"], "deepseek_responses");
        assert_eq!(deepseek_responses["models"], json!(["deepseek-v4-flash"]));

        let glm = template_by_id("glm");
        assert_eq!(glm["providerType"], "anthropic_messages");
        assert_eq!(glm["compatibility"], "glm_anthropic");
        assert_eq!(glm["baseUrl"], "https://open.bigmodel.cn/api/anthropic");
        assert_eq!(
            glm["modelsUrl"],
            "https://open.bigmodel.cn/api/paas/v4/models"
        );

        // 模板是静态数据：用户已配置的 canary provider、模型与密钥绝不能混入。
        let encoded = payload.to_string();
        for canary in [
            CANARY_PROVIDER_KEY,
            CANARY_PROVIDER_URL,
            CANARY_MODELS_URL,
            CANARY_MODEL,
            "canary-provider-name-must-not-leak",
        ] {
            assert!(!encoded.contains(canary), "templates leaked {canary}");
        }
        for forbidden_field in ["apiKey", "secretSet", "enabled", "weight", "timeoutSecs"] {
            assert!(
                !contains_json_key(&payload, forbidden_field),
                "templates exposed field {forbidden_field}"
            );
        }
    }

    fn seed_request_log(state: &SharedState, request_id: &str, created_at_ms: i64) {
        seed_request_log_with_fields(
            state,
            request_id,
            "test-model",
            "test",
            "open_ai_responses",
            "completed",
            created_at_ms,
        );
    }

    fn seed_request_log_with_fields(
        state: &SharedState,
        request_id: &str,
        model_id: &str,
        channel: &str,
        provider_type: &str,
        status: &str,
        created_at_ms: i64,
    ) -> i64 {
        use crate::ai_gateway::request_log::{LogUsage, RequestLogRecord};
        state
            .ai_gateway_request_logs
            .insert_record(&RequestLogRecord {
                request_id: request_id.to_string(),
                model_id: model_id.to_string(),
                stream: false,
                channel: channel.to_string(),
                provider_type: provider_type.to_string(),
                status: status.to_string(),
                usage: LogUsage::default(),
                cost_usd: None,
                latency_ms: None,
                ttft_ms: None,
                created_at_ms,
                error_message: None,
                request_headers_json: None,
                request_json: None,
                upstream_request_body_bytes: None,
                upstream_request_headers_json: None,
                upstream_request_json: None,
                upstream_response_sse: None,
                response_json: None,
            })
            .expect("seed request log")
    }

    #[tokio::test]
    async fn manage_request_logs_supports_keyset_sort_filters_and_literal_search() {
        let (state, _temp, token) = management_test_state();
        let same_time = 1_754_000_000_000;
        for request_id in ["same-a", "same-b", "same-c"] {
            seed_request_log_with_fields(
                &state,
                request_id,
                "Model-A",
                "Primary",
                "open_ai_responses",
                "Completed",
                same_time,
            );
        }
        seed_request_log_with_fields(
            &state,
            "literal%_request",
            "Model-X",
            "Primary",
            "anthropic_messages",
            "Failed",
            same_time + 1,
        );
        seed_request_log_with_fields(
            &state,
            "literalXXrequest",
            "Model-X",
            "Primary",
            "anthropic_messages",
            "Failed",
            same_time + 2,
        );
        let app = router(state);

        // The legacy `limit` query remains valid while the response gains page metadata.
        let first = route_response(
            app.clone(),
            "/api/v1/manage/request-logs?limit=2&status=completed&channel=PRIMARY&modelId=model-a",
            Some(&token),
        )
        .await;
        assert_eq!(first.status(), StatusCode::OK);
        let first = response_json(first).await;
        assert_eq!(first["logs"].as_array().unwrap().len(), 2);
        assert_eq!(first["hasMore"], true);
        let cursor = first["nextCursor"].as_str().expect("next cursor");
        assert!(!cursor.contains('='));
        let first_ids = first["logs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|log| log["id"].as_i64().unwrap())
            .collect::<Vec<_>>();

        let second_path = format!(
            "/api/v1/manage/request-logs?limit=2&status=COMPLETED&channel=primary&modelId=MODEL-A&cursor={cursor}"
        );
        let second = route_response(app.clone(), &second_path, Some(&token)).await;
        assert_eq!(second.status(), StatusCode::OK);
        let second = response_json(second).await;
        assert_eq!(second["logs"].as_array().unwrap().len(), 1);
        assert_eq!(second["hasMore"], false);
        assert!(second["nextCursor"].is_null());
        let second_id = second["logs"][0]["id"].as_i64().unwrap();
        assert!(!first_ids.contains(&second_id));

        let oldest = route_response(
            app.clone(),
            "/api/v1/manage/request-logs?limit=3&status=completed&sort=oldest",
            Some(&token),
        )
        .await;
        assert_eq!(oldest.status(), StatusCode::OK);
        let oldest = response_json(oldest).await;
        let oldest_ids = oldest["logs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|log| log["id"].as_i64().unwrap())
            .collect::<Vec<_>>();
        assert!(oldest_ids.windows(2).all(|ids| ids[0] < ids[1]));

        let literal = route_response(
            app,
            "/api/v1/manage/request-logs?query=%25_&status=failed&channel=PRIMARY&modelId=model-x",
            Some(&token),
        )
        .await;
        assert_eq!(literal.status(), StatusCode::OK);
        let literal = response_json(literal).await;
        assert_eq!(literal["logs"].as_array().unwrap().len(), 1);
        assert_eq!(literal["logs"][0]["requestId"], "literal%_request");
    }

    #[tokio::test]
    async fn manage_request_logs_rejects_invalid_cursor_and_sort_without_internal_details() {
        let (app, _temp, token) = management_test_router();
        for (path, expected_error) in [
            (
                "/api/v1/manage/request-logs?cursor=not-a-valid-cursor",
                "invalid request log cursor",
            ),
            (
                "/api/v1/manage/request-logs?sort=descending",
                "unsupported request log sort",
            ),
        ] {
            let response = route_response(app.clone(), path, Some(&token)).await;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            assert_eq!(
                response_json(response).await,
                json!({ "ok": false, "error": expected_error })
            );
        }
    }

    #[tokio::test]
    async fn manage_request_logs_clear_old_deletes_only_expired_logs() {
        let (state, _temp, token) = management_test_state();
        let now = crate::ai_gateway::request_log::now_ms();
        seed_request_log(&state, "ten-day-old-log", now - 10 * 86_400_000);
        seed_request_log(&state, "recent-log", now);
        let app = router(state.clone());

        let unauthorized = request_response(
            app.clone(),
            Method::POST,
            "/api/v1/manage/request-logs/clear-old",
            None,
            Some(r#"{"days":3}"#),
        )
        .await;
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

        // 无请求体时 days 缺省为 3：删除 10 天前的日志。
        let response = request_response(
            app.clone(),
            Method::POST,
            "/api/v1/manage/request-logs/clear-old",
            Some(&token),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response_json(response).await,
            json!({ "ok": true, "deleted": 1 })
        );

        // 显式 days=1 只清理超过 1 天的日志，近期日志保留。
        seed_request_log(&state, "two-day-old-log", now - 2 * 86_400_000);
        let response = request_response(
            app.clone(),
            Method::POST,
            "/api/v1/manage/request-logs/clear-old",
            Some(&token),
            Some(r#"{"days":1}"#),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response_json(response).await,
            json!({ "ok": true, "deleted": 1 })
        );

        let logs = route_response(app, "/api/v1/manage/request-logs", Some(&token)).await;
        let logs = response_json(logs).await;
        let logs = logs["logs"].as_array().expect("logs array");
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0]["requestId"], "recent-log");
    }

    #[tokio::test]
    async fn manage_provider_models_fetch_uses_stored_key_and_parses_models() {
        use std::sync::{Arc, Mutex};

        let recorded_auth: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let mock_auth = recorded_auth.clone();
        let mock = Router::new().route(
            "/v1/models",
            axum::routing::get(move |headers: axum::http::HeaderMap| {
                let recorded = mock_auth.clone();
                async move {
                    recorded.lock().expect("record auth header").push(
                        headers
                            .get(AUTHORIZATION)
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string(),
                    );
                    Json(json!({
                        "data": [
                            { "id": "model-b" },
                            { "id": "model-a" },
                            { "id": "model-b" }
                        ]
                    }))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock models server");
        let address = listener.local_addr().expect("mock models server address");
        tokio::spawn(async move {
            axum::serve(listener, mock)
                .await
                .expect("serve mock models endpoint");
        });

        let (app, _temp, token) = management_test_router();
        let body = json!({
            "providerName": "canary-provider-name-must-not-leak",
            "baseUrl": format!("http://{address}/v1"),
            "providerType": "open_ai_responses",
        })
        .to_string();
        let response = request_response(
            app,
            Method::POST,
            "/api/v1/manage/gateway/provider/models/fetch",
            Some(&token),
            Some(&body),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let payload = response_json(response).await;
        assert_eq!(payload["ok"], json!(true));
        assert_eq!(payload["models"], json!(["model-b", "model-a"]));
        assert_eq!(
            payload["attempts"],
            json!([{ "url": format!("http://{address}/v1/models"), "status": 200 }])
        );

        // providerName 命中现有 provider 时使用已存 key，但响应绝不回显它。
        assert_eq!(
            recorded_auth.lock().expect("read recorded auth").as_slice(),
            [format!("Bearer {CANARY_PROVIDER_KEY}")]
        );
        assert!(!payload.to_string().contains(CANARY_PROVIDER_KEY));
    }

    #[tokio::test]
    async fn manage_provider_models_fetch_reports_every_failed_attempt() {
        let mock = Router::new().fallback(|| async { (StatusCode::NOT_FOUND, "e".repeat(600)) });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind failing models server");
        let address = listener
            .local_addr()
            .expect("failing models server address");
        tokio::spawn(async move {
            axum::serve(listener, mock)
                .await
                .expect("serve failing models endpoint");
        });

        let (app, _temp, token) = management_test_router();

        let unauthorized = request_response(
            app.clone(),
            Method::POST,
            "/api/v1/manage/gateway/provider/models/fetch",
            None,
            Some(r#"{"baseUrl":"https://api.example.com/v1","providerType":"open_ai_responses"}"#),
        )
        .await;
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

        let body = json!({
            "baseUrl": format!("http://{address}/api"),
            "providerType": "deepseek_responses",
        })
        .to_string();
        let response = request_response(
            app,
            Method::POST,
            "/api/v1/manage/gateway/provider/models/fetch",
            Some(&token),
            Some(&body),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let payload = response_json(response).await;
        assert_eq!(payload["ok"], json!(false));
        assert_eq!(payload["models"], json!([]));
        let attempts = payload["attempts"].as_array().expect("attempts array");
        assert_eq!(attempts.len(), 2);
        assert_eq!(attempts[0]["url"], format!("http://{address}/api/models"));
        assert_eq!(
            attempts[1]["url"],
            format!("http://{address}/api/v1/models")
        );
        for attempt in attempts {
            assert_eq!(attempt["status"], json!(404));
            let preview = attempt["preview"].as_str().expect("attempt preview");
            assert_eq!(preview.chars().count(), 240);
            assert!(preview.chars().all(|ch| ch == 'e'));
            assert!(attempt.get("error").is_none());
        }
    }

    #[tokio::test]
    async fn manage_codex_models_catalog_lists_visible_catalog_models() {
        let (app, _temp, token) = management_test_router();

        let missing =
            route_response(app.clone(), "/api/v1/manage/codex/models/catalog", None).await;
        assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);

        let response =
            route_response(app, "/api/v1/manage/codex/models/catalog", Some(&token)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let payload = response_json(response).await;
        assert_exact_keys(
            payload.as_object().expect("catalog response object"),
            &["models"],
        );
        let models = payload["models"].as_array().expect("models array");
        assert!(!models.is_empty());
        for model in models {
            let object = model.as_object().expect("catalog model object");
            assert_exact_keys(object, &["displayName", "id"]);
            assert!(model["id"].as_str().is_some_and(|id| !id.is_empty()));
            assert!(
                model["displayName"]
                    .as_str()
                    .is_some_and(|name| !name.is_empty())
            );
        }
        let ids = models
            .iter()
            .filter_map(|model| model["id"].as_str())
            .collect::<Vec<_>>();
        assert!(ids.contains(&"gpt-5.5"), "catalog missing gpt-5.5: {ids:?}");
        // catalog 是内置静态目录，用户配置的 canary 模型不能出现。
        assert!(!payload.to_string().contains(CANARY_MODEL));
    }

    #[tokio::test]
    async fn manage_im_accounts_route_is_authenticated_and_exposes_no_secrets() {
        let (app, temp, token) = management_test_router();

        let missing = route_response(app.clone(), "/api/v1/manage/im/accounts", None).await;
        assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);

        let response = route_response(app, "/api/v1/manage/im/accounts", Some(&token)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let payload = response_json(response).await;
        let object = payload.as_object().expect("accounts response object");
        assert_exact_keys(object, &["accounts", "service"]);

        let service = object
            .get("service")
            .and_then(Value::as_object)
            .expect("service object");
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

        let accounts = object
            .get("accounts")
            .and_then(Value::as_array)
            .expect("accounts array");
        assert_eq!(accounts.len(), 5);
        for account in accounts {
            let account = account.as_object().expect("account object");
            assert_exact_keys(
                account,
                &[
                    "accountId",
                    "configured",
                    "connected",
                    "connecting",
                    "displayName",
                    "enabled",
                    "lastError",
                    "lastEventAtMs",
                    "lastInboundAtMs",
                    "platform",
                    "polling",
                    "secretSet",
                ],
            );
        }

        let encoded = serde_json::to_string(&payload).expect("serialize accounts response");
        for secret in [
            "canary-feishu-secret-must-not-leak",
            "canary-telegram-token-one-must-not-leak",
            "canary-telegram-token-two-must-not-leak",
            "canary-wechat-token-must-not-leak",
            "canary-wecom-secret-must-not-leak",
        ] {
            assert!(
                !encoded.contains(secret),
                "accounts response leaked {secret}"
            );
        }
        drop(temp);
    }

    #[tokio::test]
    async fn manage_im_mutation_routes_require_the_shared_bearer_credential() {
        let (app, _temp, token) = management_test_router();
        let body = r#"{"platform":"telegram","accountId":"missing","enabled":false}"#;

        let missing_enabled = request_response(
            app.clone(),
            Method::POST,
            "/api/v1/manage/im/account/enabled",
            None,
            Some(body),
        )
        .await;
        assert_eq!(missing_enabled.status(), StatusCode::UNAUTHORIZED);

        let missing_delete = request_response(
            app.clone(),
            Method::POST,
            "/api/v1/manage/im/account/delete",
            None,
            Some(r#"{"platform":"telegram","accountId":"missing"}"#),
        )
        .await;
        assert_eq!(missing_delete.status(), StatusCode::UNAUTHORIZED);

        let wrong_enabled = request_response(
            app.clone(),
            Method::POST,
            "/api/v1/manage/im/account/enabled",
            Some("wrong-management-token"),
            Some(body),
        )
        .await;
        assert_eq!(wrong_enabled.status(), StatusCode::UNAUTHORIZED);

        let authorized_missing = request_response(
            app,
            Method::POST,
            "/api/v1/manage/im/account/enabled",
            Some(&token),
            Some(body),
        )
        .await;
        assert_eq!(authorized_missing.status(), StatusCode::NOT_FOUND);

        let (unknown_app, _unknown_temp, unknown_token) = management_test_router();
        let unknown_platform = request_response(
            unknown_app,
            Method::POST,
            "/api/v1/manage/im/account/delete",
            Some(&unknown_token),
            Some(r#"{"platform":"matrix","accountId":"x"}"#),
        )
        .await;
        assert_eq!(unknown_platform.status(), StatusCode::BAD_REQUEST);

        let (legacy_app, legacy_temp, legacy_token) = management_test_router();
        let legacy_body = r#"{"platform":"telegram","accountId":"legacy-missing"}"#;
        let legacy_missing = request_response(
            legacy_app,
            Method::POST,
            "/api/v1/manage/im/account/delete",
            Some(&legacy_token),
            Some(legacy_body),
        )
        .await;
        assert_eq!(legacy_missing.status(), StatusCode::NOT_FOUND);
        let persisted = std::fs::read_to_string(legacy_temp.path().join("user-domain/config.toml"));
        assert!(
            persisted.is_err(),
            "failed delete should not persist migration"
        );
    }

    #[tokio::test]
    async fn manage_im_mutations_reach_legacy_singletons_and_do_not_resurrect_them() {
        // A feishu legacy singleton synthesizes its account id from the legacy
        // bridge account id; that id must stay usable across migration.
        let mut feishu_config = AppConfig::default();
        feishu_config.feishu.app_id = "legacy-feishu-app".to_string();
        feishu_config.feishu.app_secret = "legacy-feishu-secret".to_string();
        feishu_config.bridge.account_id = "legacy-bridge-id".to_string();
        let (feishu_state, _feishu_temp, feishu_token) =
            management_state_with_config(feishu_config);
        let feishu_app = router(feishu_state);

        let toggled = request_response(
            feishu_app.clone(),
            Method::POST,
            "/api/v1/manage/im/account/enabled",
            Some(&feishu_token),
            Some(r#"{"platform":"feishu","accountId":"legacy-bridge-id","enabled":false}"#),
        )
        .await;
        assert_eq!(toggled.status(), StatusCode::OK);
        let accounts = response_json(
            route_response(
                feishu_app,
                "/api/v1/manage/im/accounts",
                Some(&feishu_token),
            )
            .await,
        )
        .await;
        let accounts = accounts["accounts"].as_array().expect("accounts array");
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0]["platform"], "feishu");
        assert_eq!(accounts[0]["accountId"], "legacy-bridge-id");
        assert_eq!(accounts[0]["enabled"], false);

        // Deleting a legacy wechat/wecom singleton must clear the singleton
        // fields too, otherwise the effective view resurrects the account on
        // the next read.
        let mut wechat_config = AppConfig::default();
        wechat_config.wechat.bot_token = "legacy-wechat-token".to_string();
        let (wechat_state, _wechat_temp, wechat_token) =
            management_state_with_config(wechat_config);
        let wechat_app = router(wechat_state);

        let deleted = request_response(
            wechat_app.clone(),
            Method::POST,
            "/api/v1/manage/im/account/delete",
            Some(&wechat_token),
            Some(r#"{"platform":"wechat","accountId":"wechat"}"#),
        )
        .await;
        assert_eq!(deleted.status(), StatusCode::OK);
        let accounts = response_json(
            route_response(
                wechat_app,
                "/api/v1/manage/im/accounts",
                Some(&wechat_token),
            )
            .await,
        )
        .await;
        assert_eq!(
            accounts["accounts"]
                .as_array()
                .expect("accounts array")
                .len(),
            0,
            "deleted legacy wechat singleton must not resurrect"
        );

        let mut wecom_config = AppConfig::default();
        wecom_config.wecom.bot_id = "legacy-wecom-bot".to_string();
        wecom_config.wecom.secret = "legacy-wecom-secret".to_string();
        let (wecom_state, _wecom_temp, wecom_token) = management_state_with_config(wecom_config);
        let wecom_app = router(wecom_state);

        let deleted = request_response(
            wecom_app.clone(),
            Method::POST,
            "/api/v1/manage/im/account/delete",
            Some(&wecom_token),
            Some(r#"{"platform":"wecom","accountId":"wecom"}"#),
        )
        .await;
        assert_eq!(deleted.status(), StatusCode::OK);
        let accounts = response_json(
            route_response(wecom_app, "/api/v1/manage/im/accounts", Some(&wecom_token)).await,
        )
        .await;
        assert_eq!(
            accounts["accounts"]
                .as_array()
                .expect("accounts array")
                .len(),
            0,
            "deleted legacy wecom singleton must not resurrect"
        );
    }

    #[tokio::test]
    async fn manage_im_onboarding_routes_require_bearer_and_validate_input_before_network() {
        let (app, _temp, token) = management_test_router();

        // Every onboarding route is inside the bearer layer; unauthenticated
        // requests are rejected by the middleware before any handler runs.
        for (path, body) in [
            (
                "/api/v1/manage/im/account/feishu",
                r#"{"appId":"a","appSecret":"b"}"#,
            ),
            ("/api/v1/manage/im/onboarding/feishu/start", "{}"),
            (
                "/api/v1/manage/im/onboarding/feishu/poll",
                r#"{"deviceCode":"x"}"#,
            ),
            ("/api/v1/manage/im/onboarding/wechat/start", "{}"),
            (
                "/api/v1/manage/im/onboarding/wechat/poll",
                r#"{"sessionKey":"x"}"#,
            ),
            ("/api/v1/manage/im/onboarding/wecom/start", "{}"),
            (
                "/api/v1/manage/im/onboarding/wecom/poll",
                r#"{"sessionKey":"x"}"#,
            ),
        ] {
            let response =
                request_response(app.clone(), Method::POST, path, None, Some(body)).await;
            assert_eq!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "expected 401 for {path}"
            );
        }

        // Input validation runs before any upstream network call, so these
        // authorized failure paths stay hermetic.
        let missing_secret = request_response(
            app.clone(),
            Method::POST,
            "/api/v1/manage/im/account/feishu",
            Some(&token),
            Some(r#"{"appId":"cli-app"}"#),
        )
        .await;
        assert_eq!(missing_secret.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response_json(missing_secret).await,
            json!({ "ok": false, "error": "missing appSecret" })
        );

        let masked_secret = request_response(
            app.clone(),
            Method::POST,
            "/api/v1/manage/im/account/feishu",
            Some(&token),
            Some(r#"{"appId":"cli-app","appSecret":"******"}"#),
        )
        .await;
        assert_eq!(masked_secret.status(), StatusCode::BAD_REQUEST);

        let missing_device_code = request_response(
            app.clone(),
            Method::POST,
            "/api/v1/manage/im/onboarding/feishu/poll",
            Some(&token),
            Some("{}"),
        )
        .await;
        assert_eq!(missing_device_code.status(), StatusCode::BAD_REQUEST);

        let stale_wechat_session = request_response(
            app.clone(),
            Method::POST,
            "/api/v1/manage/im/onboarding/wechat/poll",
            Some(&token),
            Some(r#"{"sessionKey":"wechat-onboard-unknown"}"#),
        )
        .await;
        assert_eq!(stale_wechat_session.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response_json(stale_wechat_session).await,
            json!({ "done": false, "error": "missing_session" })
        );

        let stale_wecom_session = request_response(
            app,
            Method::POST,
            "/api/v1/manage/im/onboarding/wecom/poll",
            Some(&token),
            Some(r#"{"sessionKey":"wecom-onboard-unknown"}"#),
        )
        .await;
        assert_eq!(stale_wecom_session.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response_json(stale_wecom_session).await,
            json!({ "done": false, "error": "missing_session" })
        );
    }

    #[tokio::test]
    async fn legacy_im_mutation_routes_cannot_bypass_management_bearer() {
        let (app, _temp, _token) = management_test_router();

        for (path, body) in [
            ("/api/bridge/start", "{}"),
            ("/api/bridge/stop", "{}"),
            ("/api/im-channel/enabled", r#"{"enabled":true}"#),
            (
                "/api/im/account/enabled",
                r#"{"platform":"telegram","accountId":"tg","enabled":false}"#,
            ),
            (
                "/api/im/account/delete",
                r#"{"platform":"telegram","accountId":"tg"}"#,
            ),
            ("/api/feishu/onboard/start", "{}"),
            ("/api/feishu/onboard/poll", r#"{"deviceCode":"x"}"#),
            ("/api/telegram/configure", r#"{"botToken":"x"}"#),
            ("/api/wechat/onboard/start", "{}"),
            ("/api/wechat/onboard/poll", r#"{"sessionKey":"x"}"#),
            ("/api/wecom/onboard/start", "{}"),
            ("/api/wecom/onboard/poll", r#"{"sessionKey":"x"}"#),
        ] {
            let response =
                request_response(app.clone(), Method::POST, path, None, Some(body)).await;
            assert_eq!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "expected 401 for legacy route {path}"
            );
        }
    }

    #[tokio::test]
    async fn manage_im_telegram_route_requires_bearer_and_validates_the_token_shape() {
        let (app, _temp, token) = management_test_router();
        let body = r#"{"botToken":"12345:fixture-token"}"#;

        let missing = request_response(
            app.clone(),
            Method::POST,
            "/api/v1/manage/im/account/telegram",
            None,
            Some(body),
        )
        .await;
        assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);

        let empty_token = request_response(
            app.clone(),
            Method::POST,
            "/api/v1/manage/im/account/telegram",
            Some(&token),
            Some(r#"{"botToken":"   "}"#),
        )
        .await;
        assert_eq!(empty_token.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response_json(empty_token).await,
            json!({ "ok": false, "error": "missing botToken" })
        );

        // Masked placeholder values from status screens must never be
        // accepted as a real credential.
        let masked_token = request_response(
            app,
            Method::POST,
            "/api/v1/manage/im/account/telegram",
            Some(&token),
            Some(r#"{"botToken":"********"}"#),
        )
        .await;
        assert_eq!(masked_token.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response_json(masked_token).await,
            json!({ "ok": false, "error": "missing botToken" })
        );
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
        assert!(
            object["codexAppConfigured"].is_boolean(),
            "Codex configuration status must stay aggregate-only"
        );
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
                            lifecycle_permit: None,
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
        assert_eq!(
            lifecycle["runtime"]["buildNumber"],
            json!(crate::version::build_number())
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

    #[tokio::test]
    async fn lifecycle_lease_claims_and_restart_respects_protected_work() {
        let (state, _temp, token) = management_test_state();
        let instance_id = state.daemon_identity.instance_id.clone();
        {
            let mut runtime = state.runtime.lock().await;
            runtime
                .current_turn_by_thread
                .insert("protected-thread".to_string(), "protected-turn".to_string());
        }
        let app = router(state.clone());
        let body = json!({
            "installationId": "swiftui-test-installation",
            "daemonInstanceId": instance_id,
        })
        .to_string();

        let before_claim = request_response(
            app.clone(),
            Method::POST,
            "/api/v1/manage/lifecycle/restart",
            Some(&token),
            Some(&body),
        )
        .await;
        assert_eq!(before_claim.status(), StatusCode::CONFLICT);

        let claim = request_response(
            app.clone(),
            Method::POST,
            "/api/v1/manage/lifecycle/lease/claim",
            Some(&token),
            Some(&body),
        )
        .await;
        assert_eq!(claim.status(), StatusCode::OK);
        let claimed = response_json(claim).await;
        assert_eq!(claimed["management"]["state"], json!("managed"));
        assert_eq!(
            claimed["management"]["installationId"],
            json!("swiftui-test-installation")
        );

        let restart = request_response(
            app,
            Method::POST,
            "/api/v1/manage/lifecycle/restart",
            Some(&token),
            Some(&body),
        )
        .await;
        assert_eq!(restart.status(), StatusCode::CONFLICT);
        let restart_payload = response_json(restart).await;
        assert!(
            restart_payload["error"]
                .as_str()
                .is_some_and(|message| message.contains("受保护任务"))
        );
        assert_eq!(restart_payload["protectedWorkItems"]["total"], json!(1));
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

    #[tokio::test]
    async fn guarded_shutdown_refuses_protected_work_without_signalling() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut config = AppConfig::default();
        config.state_path = temp.path().join("state.json");
        let identity = DaemonIdentity::new();
        let instance_id = identity.instance_id.clone();
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel();
        let state = AppState::new(
            temp.path().join("config.toml"),
            config,
            Some(shutdown_tx),
            Some(identity),
        );
        state
            .runtime
            .lock()
            .await
            .current_turn_by_thread
            .insert("protected-thread".to_string(), "protected-turn".to_string());

        let response = shutdown_instance(
            State(state.clone()),
            Json(InstanceShutdownRequest {
                daemon_instance_id: instance_id,
            }),
        )
        .await
        .into_response();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let payload = response_json(response).await;
        assert_eq!(payload["protectedWorkItems"]["total"], json!(1));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut shutdown_rx)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn lifecycle_restart_commits_shutdown_once_when_idle() {
        let (state, _temp, token) = management_test_state();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        *state.shutdown_tx.lock().await = Some(shutdown_tx);
        let instance_id = state.daemon_identity.instance_id.clone();
        let body = json!({
            "installationId": "swiftui-test-installation",
            "daemonInstanceId": instance_id,
        })
        .to_string();
        let app = router(state.clone());

        let claim = request_response(
            app.clone(),
            Method::POST,
            "/api/v1/manage/lifecycle/lease/claim",
            Some(&token),
            Some(&body),
        )
        .await;
        assert_eq!(claim.status(), StatusCode::OK);

        let restart = request_response(
            app,
            Method::POST,
            "/api/v1/manage/lifecycle/restart",
            Some(&token),
            Some(&body),
        )
        .await;
        assert_eq!(restart.status(), StatusCode::OK);
        assert_eq!(response_json(restart).await["state"], json!("restarting"));
        tokio::time::timeout(std::time::Duration::from_secs(1), shutdown_rx)
            .await
            .expect("shutdown signal timeout")
            .expect("shutdown signal sender");
        assert_eq!(
            state.lifecycle_admission.state(),
            crate::app_state::LifecycleAdmissionState::ShutdownCommitted
        );
    }

    #[tokio::test]
    async fn lifecycle_restart_rejects_stale_lease_generation() {
        let (state, _temp, token) = management_test_state();
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel();
        *state.shutdown_tx.lock().await = Some(shutdown_tx);
        let instance_id = state.daemon_identity.instance_id.clone();
        let claim_body = json!({
            "installationId": "swiftui-test-installation",
            "daemonInstanceId": instance_id,
        })
        .to_string();
        let app = router(state.clone());

        let first_claim = request_response(
            app.clone(),
            Method::POST,
            "/api/v1/manage/lifecycle/lease/claim",
            Some(&token),
            Some(&claim_body),
        )
        .await;
        assert_eq!(first_claim.status(), StatusCode::OK);
        let first_generation = response_json(first_claim).await["management"]["leaseGeneration"]
            .as_u64()
            .expect("first lease generation");

        let release = request_response(
            app.clone(),
            Method::POST,
            "/api/v1/manage/lifecycle/lease/release",
            Some(&token),
            Some(&claim_body),
        )
        .await;
        assert_eq!(release.status(), StatusCode::OK);

        let second_claim = request_response(
            app.clone(),
            Method::POST,
            "/api/v1/manage/lifecycle/lease/claim",
            Some(&token),
            Some(&claim_body),
        )
        .await;
        assert_eq!(second_claim.status(), StatusCode::OK);
        let second_generation = response_json(second_claim).await["management"]["leaseGeneration"]
            .as_u64()
            .expect("second lease generation");
        assert!(second_generation > first_generation);

        let stale_body = json!({
            "installationId": "swiftui-test-installation",
            "daemonInstanceId": instance_id,
            "leaseGeneration": first_generation,
        })
        .to_string();
        let restart = request_response(
            app,
            Method::POST,
            "/api/v1/manage/lifecycle/restart",
            Some(&token),
            Some(&stale_body),
        )
        .await;
        assert_eq!(restart.status(), StatusCode::CONFLICT);
        assert!(
            response_json(restart).await["error"]
                .as_str()
                .is_some_and(|message| message.contains("换代"))
        );
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut shutdown_rx)
                .await
                .is_err()
        );
    }
}
