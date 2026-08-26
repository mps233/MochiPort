mod codex_usage;
mod lifecycle_control;

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use reqwest::{Client, Method};
use serde::{Deserialize, Serialize};
use tauri::Manager;
use tauri_plugin_opener::OpenerExt;
use url::Url;

use codex_usage::{CodexUsageSnapshot, CodexUsageState};
use lifecycle_control::LifecycleCoordinator;

use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(windows)]
use tauri::{
    menu::{Menu, MenuItemBuilder},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};

const FALLBACK_BASE_URL: &str = "http://127.0.0.1:3847";
const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const LATEST_RELEASE_API: &str = "https://api.github.com/repos/mps233/mochiport/releases/latest";
const RELEASE_PATH_PREFIX: &str = "/mps233/mochiport/releases/";
const MAX_UPDATE_RESPONSE_BYTES: usize = 512 * 1024;

#[cfg(windows)]
const TRAY_SHOW_ID: &str = "show-main-window";
#[cfg(windows)]
const TRAY_QUIT_ID: &str = "quit-application";

// Tauri keeps the target-triple suffix on external binaries inside the
// packaged resources. Keep the unsuffixed name as a portable-ZIP fallback.
#[cfg(windows)]
const DAEMON_RESOURCE_NAMES: &[&str] = &[
    "mochiport-daemon-x86_64-pc-windows-msvc.exe",
    "mochiport-daemon-aarch64-pc-windows-msvc.exe",
    "mochiport-daemon.exe",
];
#[cfg(not(windows))]
const DAEMON_RESOURCE_NAMES: &[&str] = &["mochiport-daemon"];

#[derive(Default)]
struct NativeLifecycle {
    #[cfg(windows)]
    quitting: AtomicBool,
    quit_on_close: AtomicBool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActiveDaemonLocator {
    service: String,
    api_major: u16,
    instance_id: String,
    pid: u32,
    started_at_ms: u64,
    base_url: String,
    control_file: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagementControlFile {
    management_token: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct HealthResponse {
    service: String,
    api_major: u16,
    ready: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagementStatusResponse {
    service: String,
    api_major: u16,
    ready: bool,
    instance_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyStatusResponse {
    service: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ManagementCredential {
    token: String,
    expected_instance_id: Option<String>,
}

#[derive(Debug)]
struct ManagementEndpoint {
    base_url: Url,
    locator_credential: Option<ManagementCredential>,
}

struct ManagementConnection {
    endpoints: Vec<ManagementEndpoint>,
    fallback_credentials: Vec<ManagementCredential>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EndpointProbe {
    CompatibleV1,
    OccupiedOrIncompatible,
    Offline,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NativeResponse {
    pub(crate) status: u16,
    pub(crate) body: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DaemonLaunchResult {
    started: bool,
    executable: Option<String>,
    message: String,
}

#[derive(Debug, Deserialize)]
struct GitHubReleaseResponse {
    tag_name: String,
    html_url: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateCheckResult {
    current_version: String,
    latest_version: String,
    update_available: bool,
    release_url: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LogDirectoryResponse {
    directory: String,
    instance_id: String,
}

fn user_data_root() -> Option<PathBuf> {
    env::var_os("LOCALAPPDATA")
        .or_else(|| env::var_os("APPDATA"))
        .map(PathBuf::from)
}

fn configured_home_directories() -> Vec<PathBuf> {
    let mut directories = ["MOCHIPORT_HOME", "THREADRELAY_HOME", "CODEXHUB_HOME"]
        .iter()
        .filter_map(|key| env::var_os(key).map(PathBuf::from))
        .collect::<Vec<_>>();
    if let Some(root) = user_data_root() {
        directories.extend(["MochiPort", "ThreadRelay", "CodexHub"].map(|name| root.join(name)));
    }
    directories
}

fn validate_loopback_base_url(raw: &str) -> Option<Url> {
    let mut url = Url::parse(raw).ok()?;
    if url.scheme() != "http"
        || url.username() != ""
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.host_str(), Some("127.0.0.1" | "::1" | "localhost"))
        || url.port().is_none()
    {
        return None;
    }
    url.set_path("/");
    Some(url)
}

fn read_locator() -> Option<ActiveDaemonLocator> {
    let path = user_data_root()?
        .join("MochiPort")
        .join("mochiport-active-daemon.json");
    let locator: ActiveDaemonLocator = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    if locator.service != "threadrelay"
        || locator.api_major != 1
        || locator.instance_id.is_empty()
        || locator.pid == 0
        || locator.started_at_ms == 0
        || validate_loopback_base_url(&locator.base_url).is_none()
        || locator.control_file.is_empty()
    {
        return None;
    }
    Some(locator)
}

fn load_control_token(path: &Path) -> Option<String> {
    let control: ManagementControlFile = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    let token = control.management_token;
    if token.is_empty()
        || token.len() > 256
        || token.trim() != token
        || token.chars().any(char::is_whitespace)
    {
        return None;
    }
    Some(token)
}

fn ordered_endpoints(
    locator_url: Option<Url>,
    locator_credential: Option<ManagementCredential>,
) -> Vec<ManagementEndpoint> {
    let fallback = validate_loopback_base_url(FALLBACK_BASE_URL)
        .expect("static fallback management URL is valid");
    let mut endpoints = Vec::new();
    if let Some(base_url) = locator_url {
        endpoints.push(ManagementEndpoint {
            base_url,
            locator_credential,
        });
    }
    if !endpoints
        .iter()
        .any(|endpoint| endpoint.base_url == fallback)
    {
        endpoints.push(ManagementEndpoint {
            base_url: fallback,
            // A locator credential is scoped to the locator endpoint and its
            // advertised instance. The fallback endpoint only uses control
            // files discovered through the configured home directories.
            locator_credential: None,
        });
    }
    endpoints
}

fn fallback_credentials(tokens: Vec<String>) -> Vec<ManagementCredential> {
    let mut credentials = Vec::new();
    for token in tokens {
        if credentials
            .iter()
            .any(|credential: &ManagementCredential| credential.token == token)
        {
            continue;
        }
        credentials.push(ManagementCredential {
            token,
            expected_instance_id: None,
        });
    }
    credentials
}

fn resolve_connection() -> ManagementConnection {
    let locator = read_locator();
    let locator_url = locator
        .as_ref()
        .and_then(|value| validate_loopback_base_url(&value.base_url));
    let locator_credential = locator.as_ref().and_then(|locator| {
        load_control_token(Path::new(&locator.control_file)).map(|token| ManagementCredential {
            token,
            expected_instance_id: Some(locator.instance_id.clone()),
        })
    });

    let mut fallback_paths = Vec::new();
    for directory in configured_home_directories() {
        fallback_paths.push(directory.join("mochiport-control.json"));
        fallback_paths.push(directory.join("threadrelay-control.json"));
    }

    let fallback_tokens = fallback_paths
        .iter()
        .filter_map(|path| load_control_token(path))
        .collect();
    ManagementConnection {
        endpoints: ordered_endpoints(locator_url, locator_credential),
        // Keep an unconstrained retry even when a fallback control file has
        // the same token as the locator. The locator instance may be stale
        // after daemon replacement.
        fallback_credentials: fallback_credentials(fallback_tokens),
    }
}

fn validated_relative_path(path: &str) -> Result<String, String> {
    let path = path.trim_start_matches('/');
    let raw_path = path.split_once('?').map_or(path, |(value, _)| value);
    if path.is_empty()
        || raw_path.contains('%')
        || raw_path.contains('\\')
        || raw_path.contains('\0')
        || raw_path
            .split('/')
            .any(|segment| matches!(segment, "." | ".."))
        || path.starts_with("http:")
        || path.starts_with("https:")
        || path.starts_with("//")
    {
        return Err("请求路径无效".to_string());
    }

    let validation_base =
        Url::parse("http://127.0.0.1/").expect("static management validation URL is valid");
    let normalized = validation_base
        .join(path)
        .map_err(|_| "请求路径无效".to_string())?;
    if normalized.fragment().is_some()
        || normalized.scheme() != validation_base.scheme()
        || normalized.host_str() != validation_base.host_str()
    {
        return Err("请求路径无效".to_string());
    }
    let normalized_path = normalized.path().trim_start_matches('/');
    if normalized_path != "healthz"
        && normalized_path != "api/status"
        && !normalized_path.starts_with("api/v1/manage/")
    {
        return Err("只允许访问 MochiPort 管理 API".to_string());
    }

    let mut result = normalized_path.to_string();
    if let Some(query) = normalized.query() {
        result.push('?');
        result.push_str(query);
    }
    Ok(result)
}

async fn send_request_with_timeout(
    client: &Client,
    base_url: &Url,
    path: &str,
    method: &Method,
    body: Option<&str>,
    token: Option<&str>,
    timeout: Option<Duration>,
) -> Result<NativeResponse, String> {
    let url = base_url
        .join(path)
        .map_err(|_| "无法构造管理 API 地址".to_string())?;
    let mut request = client.request(method.clone(), url);
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    if let Some(timeout) = timeout {
        request = request.timeout(timeout);
    }
    if let Some(body) = body {
        request = request
            .header("content-type", "application/json")
            .body(body.to_owned());
    }
    let response = request
        .send()
        .await
        .map_err(|error| format!("无法连接本地服务：{error}"))?;
    let status = response.status().as_u16();
    let bytes = response
        .bytes()
        .await
        .map_err(|error| format!("无法读取本地服务响应：{error}"))?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err("本地服务响应过大".to_string());
    }
    let body = String::from_utf8_lossy(&bytes).into_owned();
    if token.is_some_and(|token| body.contains(token)) {
        return Err("本地服务响应包含管理凭据，已阻止传递到界面".to_string());
    }
    Ok(NativeResponse { status, body })
}

async fn send_request(
    client: &Client,
    base_url: &Url,
    path: &str,
    method: &Method,
    body: Option<&str>,
    token: Option<&str>,
) -> Result<NativeResponse, String> {
    send_request_with_timeout(client, base_url, path, method, body, token, None).await
}

async fn send_probe_request(
    client: &Client,
    base_url: &Url,
    path: &str,
    token: Option<&str>,
) -> Result<NativeResponse, String> {
    send_request_with_timeout(
        client,
        base_url,
        path,
        &Method::GET,
        None,
        token,
        Some(Duration::from_secs(3)),
    )
    .await
}

fn compatible_service(service: &str) -> bool {
    matches!(service, "threadrelay" | "codexhub")
}

fn parse_legacy_identity(response: &NativeResponse) -> Option<LegacyStatusResponse> {
    if response.status != 200 {
        return None;
    }
    serde_json::from_str::<LegacyStatusResponse>(&response.body)
        .ok()
        .filter(|status| compatible_service(&status.service))
}

fn is_ready_mochiport_health(response: &NativeResponse) -> bool {
    response.status == 200
        && serde_json::from_str::<HealthResponse>(&response.body).is_ok_and(|health| {
            health.service == "threadrelay" && health.api_major == 1 && health.ready
        })
}

fn management_status_matches(
    response: &NativeResponse,
    expected_instance_id: Option<&str>,
) -> bool {
    response.status == 200
        && serde_json::from_str::<ManagementStatusResponse>(&response.body).is_ok_and(|status| {
            status.service == "threadrelay"
                && status.api_major == 1
                && status.ready
                && expected_instance_id.is_none_or(|expected| status.instance_id == expected)
        })
}

fn classify_endpoint_probe(
    health: Option<&NativeResponse>,
    legacy: Option<&NativeResponse>,
) -> EndpointProbe {
    if health.is_some_and(is_ready_mochiport_health) {
        EndpointProbe::CompatibleV1
    } else if health.is_none() && legacy.is_none() {
        EndpointProbe::Offline
    } else {
        // Any HTTP response proves that something owns the endpoint. This
        // includes legacy ThreadRelay and newer-but-incompatible management
        // APIs, neither of which may be bypassed to control another daemon.
        EndpointProbe::OccupiedOrIncompatible
    }
}

fn endpoint_allows_fallback(probe: EndpointProbe) -> bool {
    probe == EndpointProbe::Offline
}

async fn credential_matches_instance(
    client: &Client,
    base_url: &Url,
    credential: &ManagementCredential,
) -> bool {
    let Ok(response) = send_probe_request(
        client,
        base_url,
        "api/v1/manage/status",
        Some(&credential.token),
    )
    .await
    else {
        return false;
    };
    management_status_matches(&response, credential.expected_instance_id.as_deref())
}

async fn health_with_legacy_fallback(
    client: &Client,
    base_url: &Url,
) -> Result<NativeResponse, String> {
    let health = send_probe_request(client, base_url, "healthz", None).await?;
    if health.status != 404 {
        return Ok(health);
    }

    let legacy = match send_probe_request(client, base_url, "api/status", None).await {
        Ok(response) => response,
        Err(_) => return Ok(health),
    };
    if parse_legacy_identity(&legacy).is_none() {
        return Ok(legacy);
    }

    let body = serde_json::to_string(&HealthResponse {
        service: "threadrelay".to_string(),
        // API major zero is the bridge compatibility signal used by the
        // frontend. It keeps the legacy daemon visible without claiming the
        // authenticated v1 management surface exists.
        api_major: 0,
        ready: true,
    })
    .map_err(|error| format!("无法编码旧版后台服务状态：{error}"))?;
    Ok(NativeResponse { status: 200, body })
}

async fn management_request_inner(
    path: String,
    method: String,
    body: Option<String>,
    native_lifecycle: bool,
) -> Result<NativeResponse, String> {
    let path = validated_relative_path(&path)?;
    let method = match method.as_str() {
        "GET" => Method::GET,
        "POST" => Method::POST,
        _ => return Err("只允许 GET 和 POST 管理请求".to_string()),
    };
    let lifecycle_mutation = method == Method::POST && path.starts_with("api/v1/manage/lifecycle/");
    if lifecycle_mutation && !native_lifecycle {
        return Err("后台服务生命周期操作只能通过 Windows 原生身份核验流程执行".to_string());
    }
    if native_lifecycle
        && !matches!(
            path.as_str(),
            "api/v1/manage/lifecycle/lease/claim"
                | "api/v1/manage/lifecycle/lease/renew"
                | "api/v1/manage/lifecycle/lease/takeover"
                | "api/v1/manage/lifecycle/credential/rotate"
                | "api/v1/manage/lifecycle/restart"
        )
    {
        return Err("Windows 原生生命周期流程不允许该操作".to_string());
    }
    if body
        .as_ref()
        .is_some_and(|value| value.len() > 2 * 1024 * 1024)
    {
        return Err("管理请求内容过大".to_string());
    }

    let client = Client::builder()
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(300))
        .no_proxy()
        .build()
        .map_err(|error| format!("无法建立本地连接：{error}"))?;
    let connection = resolve_connection();

    if path == "healthz" || path == "api/status" {
        if method != Method::GET || body.is_some() {
            return Err("健康检查只允许无请求体的 GET 请求".to_string());
        }
        return public_request_with_fallback(&client, &connection.endpoints, &path).await;
    }
    if connection.fallback_credentials.is_empty()
        && connection
            .endpoints
            .iter()
            .all(|endpoint| endpoint.locator_credential.is_none())
    {
        return Err("未找到本地服务管理凭据。请先启动 MochiPort 后台服务。".to_string());
    }

    for endpoint in &connection.endpoints {
        let probe = probe_endpoint(&client, &endpoint.base_url).await;
        if endpoint_allows_fallback(probe) {
            continue;
        }
        if probe == EndpointProbe::OccupiedOrIncompatible {
            return Err(
                "检测到已有本地服务，但它不是已就绪的 MochiPort 管理 API；未发送管理凭据"
                    .to_string(),
            );
        }

        let mut attempted_credential = false;
        let mut last_response = None;
        for credential in endpoint
            .locator_credential
            .iter()
            .chain(connection.fallback_credentials.iter())
        {
            attempted_credential = true;
            if !credential_matches_instance(&client, &endpoint.base_url, credential).await {
                continue;
            }
            let response = send_request(
                &client,
                &endpoint.base_url,
                &path,
                &method,
                body.as_deref(),
                Some(&credential.token),
            )
            .await?;
            if response.status != 401 {
                return Ok(response);
            }
            last_response = Some(response);
        }
        if let Some(response) = last_response {
            return Ok(response);
        }
        return Err(if attempted_credential {
            "管理凭据与当前后台服务实例不匹配。请刷新服务状态后重试。".to_string()
        } else {
            "未找到当前后台服务的管理凭据。请刷新服务状态后重试。".to_string()
        });
    }
    Err("无法连接定位器或默认地址上的本地服务".to_string())
}

#[tauri::command]
async fn management_request(
    path: String,
    method: String,
    body: Option<String>,
) -> Result<NativeResponse, String> {
    management_request_inner(path, method, body, false).await
}

pub(crate) async fn native_lifecycle_post(
    path: &'static str,
    body: String,
) -> Result<NativeResponse, String> {
    management_request_inner(path.to_string(), "POST".to_string(), Some(body), true).await
}

pub(crate) async fn native_lifecycle_get() -> Result<NativeResponse, String> {
    management_request_inner(
        "api/v1/manage/lifecycle".to_string(),
        "GET".to_string(),
        None,
        false,
    )
    .await
}

fn daemon_candidates(app: &tauri::AppHandle) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(resource_dir) = app.path().resource_dir() {
        for name in DAEMON_RESOURCE_NAMES {
            candidates.push(resource_dir.join("binaries").join(name));
            candidates.push(resource_dir.join(name));
        }
    }
    if let Ok(current_exe) = env::current_exe() {
        if let Some(directory) = current_exe.parent() {
            for name in DAEMON_RESOURCE_NAMES {
                candidates.push(directory.join(name));
            }
        }
    }
    candidates
}

async fn probe_endpoint(client: &Client, base_url: &Url) -> EndpointProbe {
    let health = send_probe_request(client, base_url, "healthz", None).await;
    if health.as_ref().is_ok_and(is_ready_mochiport_health) {
        return EndpointProbe::CompatibleV1;
    }
    if let Ok(response) = &health {
        return classify_endpoint_probe(Some(response), None);
    }

    let legacy = send_probe_request(client, base_url, "api/status", None).await;
    classify_endpoint_probe(None, legacy.as_ref().ok())
}

fn candidate_base_urls() -> Vec<Url> {
    let locator_url = read_locator()
        .as_ref()
        .and_then(|locator| validate_loopback_base_url(&locator.base_url));
    ordered_endpoints(locator_url, None)
        .into_iter()
        .map(|endpoint| endpoint.base_url)
        .collect()
}

#[tauri::command]
async fn start_daemon(app: tauri::AppHandle) -> Result<DaemonLaunchResult, String> {
    let client = Client::builder()
        .connect_timeout(Duration::from_millis(600))
        .timeout(Duration::from_secs(2))
        .no_proxy()
        .build()
        .map_err(|error| format!("无法检查本地服务：{error}"))?;
    for base_url in candidate_base_urls() {
        match probe_endpoint(&client, &base_url).await {
            EndpointProbe::CompatibleV1 => {
                return Ok(DaemonLaunchResult {
                    started: false,
                    executable: None,
                    message: "后台服务已经在运行".to_string(),
                });
            }
            EndpointProbe::OccupiedOrIncompatible => {
                return Ok(DaemonLaunchResult {
                    started: false,
                    executable: None,
                    message: "检测到已有本地服务，但管理接口版本不兼容；未启动新的后台服务"
                        .to_string(),
                });
            }
            EndpointProbe::Offline => {}
        }
    }
    let executable = daemon_candidates(&app)
        .into_iter()
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| "应用内未找到 MochiPort 后台服务".to_string())?;

    let mut command = Command::new(&executable);
    command
        .arg("daemon")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS};
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    command
        .spawn()
        .map_err(|error| format!("无法启动后台服务：{error}"))?;
    Ok(DaemonLaunchResult {
        started: true,
        executable: Some(executable.to_string_lossy().into_owned()),
        message: "后台服务启动中".to_string(),
    })
}

async fn public_request_with_fallback(
    client: &Client,
    endpoints: &[ManagementEndpoint],
    path: &str,
) -> Result<NativeResponse, String> {
    let mut last_error = None;
    for endpoint in endpoints {
        let response = if path == "healthz" {
            health_with_legacy_fallback(client, &endpoint.base_url).await
        } else {
            send_probe_request(client, &endpoint.base_url, path, None).await
        };
        match response {
            Ok(response) => return Ok(response),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| "没有可用的本地服务地址".to_string()))
}

#[tauri::command]
fn set_close_behavior(
    behavior: String,
    lifecycle: tauri::State<'_, NativeLifecycle>,
) -> Result<(), String> {
    let quit_on_close = match behavior.as_str() {
        "tray" => false,
        "quit" => true,
        _ => return Err("关闭行为无效".to_string()),
    };
    lifecycle
        .quit_on_close
        .store(quit_on_close, Ordering::Release);
    Ok(())
}

#[tauri::command]
async fn codex_usage_snapshot(
    state: tauri::State<'_, CodexUsageState>,
) -> Result<CodexUsageSnapshot, String> {
    codex_usage::snapshot(state.inner()).await
}

#[tauri::command]
async fn open_log_directory(app: tauri::AppHandle) -> Result<(), String> {
    let response = management_request_inner(
        "api/v1/manage/log-directory".to_string(),
        "GET".to_string(),
        None,
        false,
    )
    .await?;
    if response.status != 200 {
        return Err(format!("读取日志目录失败：HTTP {}", response.status));
    }
    let response = serde_json::from_str::<LogDirectoryResponse>(&response.body)
        .map_err(|_| "日志目录响应格式无效".to_string())?;
    if response.instance_id.trim().is_empty() {
        return Err("日志目录响应缺少后台服务身份".to_string());
    }
    let directory = PathBuf::from(response.directory);
    if !directory.is_absolute() {
        return Err("后台服务返回了非绝对日志目录".to_string());
    }
    let directory = directory
        .canonicalize()
        .map_err(|error| format!("无法访问日志目录：{error}"))?;
    if !directory.is_dir() {
        return Err("日志目录不存在".to_string());
    }
    app.opener()
        .open_path(directory.to_string_lossy().into_owned(), None::<&str>)
        .map_err(|error| format!("无法打开日志目录：{error}"))
}

fn parsed_release_version(raw: &str) -> Option<Vec<u64>> {
    let raw = raw.trim().trim_start_matches(['v', 'V']);
    let core = raw.split_once(['-', '+']).map_or(raw, |(value, _)| value);
    if core.is_empty() {
        return None;
    }
    let components = core
        .split('.')
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    (!components.is_empty()).then_some(components)
}

fn release_is_newer(candidate: &str, current: &str) -> bool {
    let (Some(mut candidate), Some(mut current)) = (
        parsed_release_version(candidate),
        parsed_release_version(current),
    ) else {
        return false;
    };
    let count = candidate.len().max(current.len());
    candidate.resize(count, 0);
    current.resize(count, 0);
    candidate > current
}

fn validated_release_url(raw: &str) -> Option<String> {
    let url = Url::parse(raw).ok()?;
    (url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.host_str() == Some("github.com")
        && url.port().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && url
            .path()
            .to_ascii_lowercase()
            .starts_with(RELEASE_PATH_PREFIX))
    .then(|| url.into())
}

#[tauri::command]
fn open_release_page(app: tauri::AppHandle, url: String) -> Result<(), String> {
    let url =
        validated_release_url(&url).ok_or_else(|| "发布页面地址未通过安全校验".to_string())?;
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|error| format!("无法打开发布页面：{error}"))
}

#[tauri::command]
async fn check_for_updates() -> Result<UpdateCheckResult, String> {
    let current_version = env!("CARGO_PKG_VERSION").to_string();
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|error| format!("无法建立更新检查连接：{error}"))?;
    let response = client
        .get(LATEST_RELEASE_API)
        .header("accept", "application/vnd.github+json")
        .header("user-agent", format!("MochiPort/{current_version}"))
        .send()
        .await
        .map_err(|error| format!("无法连接更新服务：{error}"))?;
    if !response.status().is_success() {
        return Err(format!("更新服务返回 HTTP {}", response.status().as_u16()));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|error| format!("无法读取更新信息：{error}"))?;
    if bytes.len() > MAX_UPDATE_RESPONSE_BYTES {
        return Err("更新信息响应过大".to_string());
    }
    let release = serde_json::from_slice::<GitHubReleaseResponse>(&bytes)
        .map_err(|_| "更新信息格式无效".to_string())?;
    let release_url = validated_release_url(&release.html_url)
        .ok_or_else(|| "更新页面地址未通过安全校验".to_string())?;
    if parsed_release_version(&release.tag_name).is_none() {
        return Err("更新版本号格式无效".to_string());
    }
    Ok(UpdateCheckResult {
        update_available: release_is_newer(&release.tag_name, &current_version),
        latest_version: release.tag_name,
        current_version,
        release_url,
    })
}

#[cfg(windows)]
fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

#[cfg(any(windows, test))]
fn launch_requests_hidden(args: &[String]) -> bool {
    args.iter().any(|argument| argument == "--hidden")
}

#[cfg(windows)]
fn setup_native_lifecycle(app: &mut tauri::App) -> tauri::Result<()> {
    let show_item = MenuItemBuilder::with_id(TRAY_SHOW_ID, "显示 MochiPort").build(app)?;
    let quit_item = MenuItemBuilder::with_id(TRAY_QUIT_ID, "退出").build(app)?;
    let menu = Menu::with_items(app, &[&show_item, &quit_item])?;

    let mut tray = TrayIconBuilder::with_id("mochiport-tray")
        .menu(&menu)
        .tooltip("MochiPort")
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            TRAY_SHOW_ID => show_main_window(app),
            TRAY_QUIT_ID => {
                app.state::<NativeLifecycle>()
                    .quitting
                    .store(true, Ordering::Release);
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon().cloned() {
        tray = tray.icon(icon);
    }
    tray.build(app)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(status: u16, body: &str) -> NativeResponse {
        NativeResponse {
            status,
            body: body.to_string(),
        }
    }

    fn locator_credential() -> ManagementCredential {
        ManagementCredential {
            token: "locator-token".to_string(),
            expected_instance_id: Some("locator-instance".to_string()),
        }
    }

    #[test]
    fn endpoints_prefer_locator_then_fallback_and_scope_locator_credential() {
        let locator_url = validate_loopback_base_url("http://127.0.0.1:49321").unwrap();
        let endpoints = ordered_endpoints(Some(locator_url.clone()), Some(locator_credential()));

        assert_eq!(endpoints.len(), 2);
        assert_eq!(endpoints[0].base_url, locator_url);
        assert_eq!(endpoints[0].locator_credential, Some(locator_credential()));
        assert_eq!(endpoints[1].base_url.as_str(), "http://127.0.0.1:3847/");
        assert_eq!(endpoints[1].locator_credential, None);
    }

    #[test]
    fn endpoint_list_deduplicates_default_locator_without_losing_binding() {
        let locator_url = validate_loopback_base_url(FALLBACK_BASE_URL).unwrap();
        let endpoints = ordered_endpoints(Some(locator_url), Some(locator_credential()));

        assert_eq!(endpoints.len(), 1);
        assert_eq!(endpoints[0].locator_credential, Some(locator_credential()));
    }

    #[test]
    fn fallback_control_tokens_remain_unconstrained_and_deduplicated() {
        let credentials = fallback_credentials(vec![
            "shared-token".to_string(),
            "shared-token".to_string(),
            "current-token".to_string(),
        ]);

        assert_eq!(
            credentials,
            vec![
                ManagementCredential {
                    token: "shared-token".to_string(),
                    expected_instance_id: None,
                },
                ManagementCredential {
                    token: "current-token".to_string(),
                    expected_instance_id: None,
                },
            ]
        );
    }

    #[test]
    fn only_offline_endpoint_allows_fallback() {
        assert!(endpoint_allows_fallback(EndpointProbe::Offline));
        assert!(!endpoint_allows_fallback(EndpointProbe::CompatibleV1));
        assert!(!endpoint_allows_fallback(
            EndpointProbe::OccupiedOrIncompatible
        ));
    }

    #[test]
    fn probe_classification_does_not_bypass_active_legacy_or_incompatible_service() {
        let ready = response(
            200,
            r#"{"service":"threadrelay","apiMajor":1,"ready":true}"#,
        );
        let legacy = response(200, r#"{"service":"threadrelay"}"#);
        let incompatible = response(
            200,
            r#"{"service":"threadrelay","apiMajor":0,"ready":true}"#,
        );

        assert_eq!(
            classify_endpoint_probe(Some(&ready), None),
            EndpointProbe::CompatibleV1
        );
        assert_eq!(
            classify_endpoint_probe(None, Some(&legacy)),
            EndpointProbe::OccupiedOrIncompatible
        );
        assert_eq!(
            classify_endpoint_probe(Some(&incompatible), None),
            EndpointProbe::OccupiedOrIncompatible
        );
        assert_eq!(classify_endpoint_probe(None, None), EndpointProbe::Offline);
    }

    #[test]
    fn management_status_requires_ready_v1_mochiport_and_optional_instance_match() {
        let current = response(
            200,
            r#"{"service":"threadrelay","apiMajor":1,"ready":true,"instanceId":"current"}"#,
        );
        let wrong_service = response(
            200,
            r#"{"service":"other","apiMajor":1,"ready":true,"instanceId":"current"}"#,
        );
        let wrong_version = response(
            200,
            r#"{"service":"threadrelay","apiMajor":2,"ready":true,"instanceId":"current"}"#,
        );
        let not_ready = response(
            200,
            r#"{"service":"threadrelay","apiMajor":1,"ready":false,"instanceId":"current"}"#,
        );

        assert!(management_status_matches(&current, None));
        assert!(management_status_matches(&current, Some("current")));
        assert!(!management_status_matches(&current, Some("stale")));
        assert!(!management_status_matches(&wrong_service, None));
        assert!(!management_status_matches(&wrong_version, None));
        assert!(!management_status_matches(&not_ready, None));
    }

    #[test]
    fn management_path_rejects_encoded_or_normalized_traversal() {
        for path in [
            "api/v1/manage/%2e%2e/%2e%2e/api/shutdown",
            "api/v1/manage/../../../api/shutdown",
            "api/v1/manage/..\\..\\api\\shutdown",
            "api/v1/manage/status#fragment",
        ] {
            assert!(validated_relative_path(path).is_err(), "accepted {path}");
        }

        assert_eq!(
            validated_relative_path("/api/v1/manage/request-logs?query=hello%20world").as_deref(),
            Ok("api/v1/manage/request-logs?query=hello%20world")
        );
    }

    #[test]
    fn update_versions_and_release_urls_are_strictly_validated() {
        assert!(release_is_newer("v0.6.0", "0.5.3"));
        assert!(release_is_newer("0.5.3.1", "v0.5.3"));
        assert!(!release_is_newer("v0.5.3", "0.5.3"));
        assert!(!release_is_newer("not-a-version", "0.5.3"));
        assert_eq!(
            validated_release_url("https://github.com/mps233/mochiport/releases/tag/v0.6.0")
                .as_deref(),
            Some("https://github.com/mps233/mochiport/releases/tag/v0.6.0")
        );
        assert!(
            validated_release_url("https://evil.example/mps233/mochiport/releases/tag/v1")
                .is_none()
        );
        assert!(validated_release_url("https://github.com/other/repo/releases/tag/v1").is_none());
    }

    #[test]
    fn hidden_launch_flag_is_matched_as_a_distinct_argument() {
        assert!(launch_requests_hidden(&[
            "MochiPort.exe".to_string(),
            "--hidden".to_string(),
        ]));
        assert!(!launch_requests_hidden(&["MochiPort.exe".to_string()]));
        assert!(!launch_requests_hidden(&[
            "MochiPort.exe".to_string(),
            "--hidden-window".to_string(),
        ]));
    }

    #[test]
    fn packaged_sidecar_candidates_include_target_triple_and_portable_names() {
        assert!(
            DAEMON_RESOURCE_NAMES
                .iter()
                .any(|name| name.contains("x86_64-pc-windows-msvc") || *name == "mochiport-daemon")
        );
        assert!(
            DAEMON_RESOURCE_NAMES
                .iter()
                .any(|name| *name == "mochiport-daemon" || name.ends_with(".exe"))
        );
    }
}

pub fn run() {
    let builder = tauri::Builder::default().manage(CodexUsageState::default());
    // The single-instance plugin must be the first registered plugin. A normal
    // second launch restores the main window; an autostart-style hidden launch
    // leaves the existing window state unchanged.
    #[cfg(windows)]
    let builder = builder
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            if !launch_requests_hidden(&args) {
                show_main_window(app);
            }
        }))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--hidden"]),
        ))
        .plugin(tauri_plugin_notification::init())
        .manage(NativeLifecycle::default())
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let lifecycle = window.app_handle().state::<NativeLifecycle>();
                if !lifecycle.quitting.load(Ordering::Acquire) {
                    api.prevent_close();
                    if lifecycle.quit_on_close.load(Ordering::Acquire) {
                        lifecycle.quitting.store(true, Ordering::Release);
                        window.app_handle().exit(0);
                    } else {
                        let _ = window.hide();
                    }
                }
            }
        });

    builder
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            management_request,
            lifecycle_control::lifecycle_installation_id,
            lifecycle_control::lifecycle_lease,
            lifecycle_control::lifecycle_takeover,
            lifecycle_control::lifecycle_rotate_credential,
            lifecycle_control::lifecycle_safe_restart,
            start_daemon,
            set_close_behavior,
            codex_usage_snapshot,
            check_for_updates,
            open_log_directory,
            open_release_page
        ])
        .setup(|app| {
            let lifecycle_directory = app.path().app_local_data_dir()?;
            let lifecycle_coordinator =
                LifecycleCoordinator::load(&lifecycle_directory).map_err(std::io::Error::other)?;
            app.manage(lifecycle_coordinator);
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_shadow(true);
                #[cfg(windows)]
                if std::env::args_os().any(|argument| argument == "--hidden") {
                    let _ = window.hide();
                }
            }
            #[cfg(windows)]
            setup_native_lifecycle(app)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running MochiPort");
}
