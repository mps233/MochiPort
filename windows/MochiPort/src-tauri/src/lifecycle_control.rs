use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use tauri::State;
use uuid::Uuid;

#[cfg(windows)]
use sha2::{Digest, Sha256};
#[cfg(windows)]
use std::{
    fs::File,
    io::Read,
    process::{Command, Stdio},
};

use super::{ActiveDaemonLocator, NativeResponse};

const INSTALLATION_ID_FILE: &str = "windows-installation.json";
const PROCESS_START_GRACE_BEFORE_MS: u64 = 2_000;
const PROCESS_START_GRACE_AFTER_MS: u64 = 120_000;
const REPLACEMENT_WAIT: Duration = Duration::from_secs(30);
const REPLACEMENT_POLL: Duration = Duration::from_millis(350);
const REPLACEMENT_STABLE_PROBES: usize = 2;
const OLD_PROCESS_EXIT_WAIT: Duration = Duration::from_secs(15);

#[derive(Debug)]
pub(crate) struct LifecycleCoordinator {
    installation_id: String,
    operation_in_progress: AtomicBool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PersistedInstallationIdentity {
    installation_id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LifecycleSnapshot {
    service: LifecycleService,
    executable: String,
    executable_sha256: Option<String>,
    config_path: String,
    bind: String,
    runtime: LifecycleRuntime,
    protected_work_items: ProtectedWorkItems,
    management: LifecycleManagement,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct LifecycleService {
    service: String,
    api_major: u16,
    ready: bool,
    instance_id: String,
    pid: u32,
    started_at_ms: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct LifecycleRuntime {
    state: String,
    product_version: String,
    build_number: Option<u64>,
    api_major: u16,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct ProtectedWorkItems {
    ai_gateway_requests: usize,
    codex_turns: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    enhanced_launches: Option<usize>,
    im_streams: usize,
    pending_approvals: usize,
    remote_control_requests: usize,
    total: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct LifecycleManagement {
    state: String,
    mode: String,
    can_control: bool,
    installation_id: Option<String>,
    lease_generation: Option<u64>,
    lease_expires_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    management_token_generation: Option<u64>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct DaemonIdentityProof {
    pid: u32,
    started_at_ms: u64,
    executable: String,
    executable_sha256: String,
    bind: String,
}

#[derive(Debug)]
struct ObservedProcess {
    pid: u32,
    created_at_ms: u64,
    executable: String,
    executable_sha256: String,
}

#[derive(Debug)]
struct ProcessExitWaiter {
    #[cfg(windows)]
    handle: isize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LifecycleLeaseRequest<'a> {
    installation_id: &'a str,
    daemon_instance_id: &'a str,
    daemon_identity: &'a DaemonIdentityProof,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LifecycleRestartRequest<'a> {
    installation_id: &'a str,
    daemon_instance_id: &'a str,
    lease_generation: u64,
    force: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LifecycleTakeoverRequest<'a> {
    installation_id: &'a str,
    daemon_instance_id: &'a str,
    expected_lease_generation: u64,
    expected_management_token_generation: u64,
    request_id: &'a str,
    force: bool,
    daemon_identity: &'a DaemonIdentityProof,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LifecycleCredentialRotateRequest<'a> {
    installation_id: &'a str,
    daemon_instance_id: &'a str,
    lease_generation: u64,
    expected_management_token_generation: u64,
    request_id: &'a str,
    reason: &'static str,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CredentialMutationResponse {
    ok: bool,
    rotated: bool,
    request_id: String,
    management_token_generation: u64,
}

#[derive(Debug, Deserialize)]
struct RestartResponse {
    ok: bool,
    state: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LeaseOperation {
    Claim,
    Renew,
}

impl LeaseOperation {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "claim" => Ok(Self::Claim),
            "renew" => Ok(Self::Renew),
            _ => Err("后台服务租约操作无效".to_string()),
        }
    }

    fn path(self) -> &'static str {
        match self {
            Self::Claim => "api/v1/manage/lifecycle/lease/claim",
            Self::Renew => "api/v1/manage/lifecycle/lease/renew",
        }
    }
}

impl LifecycleCoordinator {
    pub(crate) fn load(data_directory: &Path) -> Result<Self, String> {
        fs::create_dir_all(data_directory)
            .map_err(|error| format!("无法创建 MochiPort 本地数据目录：{error}"))?;
        let path = data_directory.join(INSTALLATION_ID_FILE);
        let installation_id = if path.exists() {
            read_installation_id(&path)?
        } else {
            create_installation_id(&path)?
        };
        Ok(Self {
            installation_id,
            operation_in_progress: AtomicBool::new(false),
        })
    }

    fn begin_operation(&self) -> Result<LifecycleOperationGuard<'_>, String> {
        self.operation_in_progress
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| "另一项后台服务管理操作正在进行中".to_string())?;
        Ok(LifecycleOperationGuard {
            flag: &self.operation_in_progress,
        })
    }
}

struct LifecycleOperationGuard<'a> {
    flag: &'a AtomicBool,
}

impl Drop for LifecycleOperationGuard<'_> {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::Release);
    }
}

fn read_installation_id(path: &Path) -> Result<String, String> {
    let contents = fs::read(path).map_err(|error| format!("无法读取 Windows 安装身份：{error}"))?;
    let persisted: PersistedInstallationIdentity = serde_json::from_slice(&contents)
        .map_err(|_| "Windows 安装身份文件格式无效".to_string())?;
    let parsed = Uuid::parse_str(&persisted.installation_id)
        .map_err(|_| "Windows 安装身份文件格式无效".to_string())?;
    Ok(parsed.hyphenated().to_string())
}

fn create_installation_id(path: &Path) -> Result<String, String> {
    let installation_id = Uuid::new_v4().hyphenated().to_string();
    let payload = serde_json::to_vec(&PersistedInstallationIdentity {
        installation_id: installation_id.clone(),
    })
    .map_err(|error| format!("无法编码 Windows 安装身份：{error}"))?;
    let parent = path
        .parent()
        .ok_or_else(|| "Windows 安装身份路径无效".to_string())?;
    let temporary = parent.join(format!(
        ".{INSTALLATION_ID_FILE}.{}.tmp",
        Uuid::new_v4().simple()
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|error| format!("无法创建 Windows 安装身份：{error}"))?;
        file.write_all(&payload)
            .and_then(|_| file.write_all(b"\n"))
            .and_then(|_| file.sync_all())
            .map_err(|error| format!("无法保存 Windows 安装身份：{error}"))?;
        match fs::rename(&temporary, path) {
            Ok(()) => Ok(installation_id.clone()),
            Err(_error) if path.exists() => read_installation_id(path),
            Err(error) => Err(format!("无法提交 Windows 安装身份：{error}")),
        }
    })();
    let _ = fs::remove_file(temporary);
    result
}

fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

fn normalize_windows_path(value: &str) -> String {
    let mut normalized = value.trim().replace('/', "\\");
    if let Some(without_prefix) = normalized.strip_prefix("\\\\?\\UNC\\") {
        normalized = format!("\\\\{without_prefix}");
    } else if let Some(without_prefix) = normalized.strip_prefix("\\\\?\\") {
        normalized = without_prefix.to_string();
    }
    while normalized.len() > 3 && normalized.ends_with('\\') {
        normalized.pop();
    }
    normalized.to_lowercase()
}

fn paths_match(left: &str, right: &str) -> bool {
    !left.trim().is_empty()
        && !right.trim().is_empty()
        && normalize_windows_path(left) == normalize_windows_path(right)
}

fn is_absolute_windows_path(value: &str) -> bool {
    let normalized = value.trim().replace('/', "\\");
    let bytes = normalized.as_bytes();
    (bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\')
        || normalized.starts_with("\\\\")
}

fn parent_path(value: &str) -> Option<String> {
    let normalized = normalize_windows_path(value);
    normalized
        .rsplit_once('\\')
        .map(|(parent, _)| parent.to_string())
}

fn file_name(value: &str) -> Option<String> {
    normalize_windows_path(value)
        .rsplit_once('\\')
        .map(|(_, name)| name.to_string())
}

fn loopback_endpoint(value: &str) -> Option<(u16, bool)> {
    if let Ok(address) = value.parse::<std::net::SocketAddr>() {
        return address.ip().is_loopback().then_some((address.port(), true));
    }
    let (host, port) = value.rsplit_once(':')?;
    let port = port.parse::<u16>().ok()?;
    matches!(
        host.trim_matches(['[', ']']),
        "localhost" | "127.0.0.1" | "::1"
    )
    .then_some((port, true))
}

fn locator_matches_bind(locator: &ActiveDaemonLocator, bind: &str) -> bool {
    let Some((bind_port, _)) = loopback_endpoint(bind) else {
        return false;
    };
    let Ok(url) = url::Url::parse(&locator.base_url) else {
        return false;
    };
    url.scheme() == "http"
        && url.port() == Some(bind_port)
        && matches!(url.host_str(), Some("127.0.0.1" | "::1" | "localhost"))
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
}

fn verify_observation(
    lifecycle: &LifecycleSnapshot,
    locator: &ActiveDaemonLocator,
    observed: &ObservedProcess,
) -> Result<DaemonIdentityProof, String> {
    if lifecycle.service.service != "mochiport"
        || lifecycle.service.api_major != 1
        || !lifecycle.service.ready
        || lifecycle.service.instance_id.trim().is_empty()
        || lifecycle.service.pid == 0
        || lifecycle.runtime.api_major != 1
        || !is_absolute_windows_path(&lifecycle.executable)
        || !is_absolute_windows_path(&lifecycle.config_path)
    {
        return Err("后台服务协议身份无效，未申请管理权".to_string());
    }
    if locator.service != lifecycle.service.service
        || locator.api_major != lifecycle.service.api_major
        || locator.instance_id != lifecycle.service.instance_id
        || locator.pid != lifecycle.service.pid
        || locator.started_at_ms != lifecycle.service.started_at_ms
        || !locator_matches_bind(locator, &lifecycle.bind)
    {
        return Err("后台服务定位器与生命周期身份不一致".to_string());
    }
    let control_file_is_known = file_name(&locator.control_file).as_deref()
        == Some("mochiport-control.json")
        && parent_path(&locator.control_file) == parent_path(&lifecycle.config_path);
    if !control_file_is_known {
        return Err("后台服务管理凭据路径不可信".to_string());
    }
    if observed.pid != lifecycle.service.pid {
        return Err("后台服务 PID 已变化，请刷新后重试".to_string());
    }
    let earliest = observed
        .created_at_ms
        .saturating_sub(PROCESS_START_GRACE_BEFORE_MS);
    let latest = observed
        .created_at_ms
        .saturating_add(PROCESS_START_GRACE_AFTER_MS);
    if !(earliest..=latest).contains(&lifecycle.service.started_at_ms) {
        return Err("后台服务启动时间与 Windows 进程不一致".to_string());
    }
    if !paths_match(&lifecycle.executable, &observed.executable) {
        return Err("后台服务可执行路径与 Windows 进程不一致".to_string());
    }
    let Some(expected_sha256) = lifecycle.executable_sha256.as_deref() else {
        return Err("后台服务未提供可执行文件 SHA-256，不能申请管理权".to_string());
    };
    if expected_sha256.len() != 64
        || !expected_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        || !expected_sha256.eq_ignore_ascii_case(&observed.executable_sha256)
    {
        return Err("后台服务可执行文件 SHA-256 校验失败".to_string());
    }

    Ok(DaemonIdentityProof {
        pid: lifecycle.service.pid,
        started_at_ms: lifecycle.service.started_at_ms,
        // The daemon compares this field with its lifecycle response exactly.
        // The independently observed process path was already normalized and
        // matched above.
        executable: lifecycle.executable.clone(),
        executable_sha256: observed.executable_sha256.clone(),
        bind: lifecycle.bind.clone(),
    })
}

#[cfg(windows)]
fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file =
        File::open(path).map_err(|error| format!("无法读取后台服务可执行文件：{error}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("无法校验后台服务可执行文件：{error}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(windows)]
fn observe_process(pid: u32) -> Result<ObservedProcess, String> {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, FILETIME},
        System::Threading::{
            GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
            QueryFullProcessImageNameW,
        },
    };

    struct HandleGuard(windows_sys::Win32::Foundation::HANDLE);
    impl Drop for HandleGuard {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return Err("无法打开后台服务 Windows 进程".to_string());
    }
    let handle = HandleGuard(handle);
    let mut length = 32_768_u32;
    let mut path = vec![0_u16; length as usize];
    if unsafe { QueryFullProcessImageNameW(handle.0, 0, path.as_mut_ptr(), &mut length) } == 0 {
        return Err("无法读取后台服务 Windows 进程路径".to_string());
    }
    path.truncate(length as usize);
    let executable =
        String::from_utf16(&path).map_err(|_| "后台服务 Windows 进程路径无效".to_string())?;

    let mut created = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut exited = created;
    let mut kernel = created;
    let mut user = created;
    if unsafe { GetProcessTimes(handle.0, &mut created, &mut exited, &mut kernel, &mut user) } == 0
    {
        return Err("无法读取后台服务 Windows 启动时间".to_string());
    }
    let windows_ticks = ((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64;
    let created_at_ms = windows_ticks
        .checked_div(10_000)
        .and_then(|milliseconds| milliseconds.checked_sub(11_644_473_600_000))
        .ok_or_else(|| "后台服务 Windows 启动时间无效".to_string())?;
    let executable_sha256 = sha256_file(Path::new(&executable))?;
    Ok(ObservedProcess {
        pid,
        created_at_ms,
        executable,
        executable_sha256,
    })
}

#[cfg(windows)]
impl ProcessExitWaiter {
    fn open(pid: u32) -> Result<Self, String> {
        use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE};
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        if handle.is_null() {
            return Err("无法持有当前后台服务进程句柄".to_string());
        }
        Ok(Self {
            handle: handle as isize,
        })
    }

    async fn wait(&self, timeout: Duration) -> Result<(), String> {
        use windows_sys::Win32::{
            Foundation::{WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT},
            System::Threading::WaitForSingleObject,
        };
        let deadline = Instant::now() + timeout;
        loop {
            let result = unsafe {
                WaitForSingleObject(self.handle as windows_sys::Win32::Foundation::HANDLE, 0)
            };
            match result {
                WAIT_OBJECT_0 => return Ok(()),
                WAIT_TIMEOUT if Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                WAIT_TIMEOUT => return Err("后台服务未在安全退出期限内停止".to_string()),
                WAIT_FAILED => return Err("无法等待后台服务 Windows 进程退出".to_string()),
                _ => return Err("后台服务 Windows 进程返回未知等待状态".to_string()),
            }
        }
    }
}

#[cfg(windows)]
impl Drop for ProcessExitWaiter {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(
                self.handle as windows_sys::Win32::Foundation::HANDLE,
            );
        }
    }
}

#[cfg(not(windows))]
impl ProcessExitWaiter {
    fn open(_pid: u32) -> Result<Self, String> {
        Err("后台服务进程退出等待仅在 Windows 上可用".to_string())
    }

    async fn wait(&self, _timeout: Duration) -> Result<(), String> {
        Err("后台服务进程退出等待仅在 Windows 上可用".to_string())
    }
}

#[cfg(not(windows))]
fn observe_process(_pid: u32) -> Result<ObservedProcess, String> {
    Err("后台服务原生身份校验仅在 Windows 上可用".to_string())
}

fn verify_native_identity(lifecycle: &LifecycleSnapshot) -> Result<DaemonIdentityProof, String> {
    let locator = super::read_locator().ok_or_else(|| "未找到可信的后台服务定位器".to_string())?;
    let observed = observe_process(lifecycle.service.pid)?;
    verify_observation(lifecycle, &locator, &observed)
}

#[cfg(any(windows, test))]
fn relaunch_arguments(config_path: &str) -> [&str; 3] {
    ["--config", config_path, "daemon"]
}

#[cfg(windows)]
fn relaunch_verified_daemon(
    lifecycle: &LifecycleSnapshot,
    expected_sha256: &str,
) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS};

    if !is_absolute_windows_path(&lifecycle.executable)
        || !is_absolute_windows_path(&lifecycle.config_path)
    {
        return Err("后台服务重启路径不是 Windows 绝对路径".to_string());
    }
    let executable = Path::new(&lifecycle.executable);
    let current_sha256 = sha256_file(executable)?;
    if !current_sha256.eq_ignore_ascii_case(expected_sha256) {
        return Err("后台服务可执行文件在退出后发生变化，已取消重新启动".to_string());
    }
    let mut command = Command::new(executable);
    command
        .args(relaunch_arguments(&lifecycle.config_path))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    command
        .spawn()
        .map_err(|error| format!("无法从已核验路径重新启动后台服务：{error}"))?;
    Ok(())
}

#[cfg(not(windows))]
fn relaunch_verified_daemon(
    _lifecycle: &LifecycleSnapshot,
    _expected_sha256: &str,
) -> Result<(), String> {
    Err("后台服务同路径重新启动仅在 Windows 上可用".to_string())
}

fn error_from_response(response: &NativeResponse, fallback: &str) -> String {
    serde_json::from_str::<serde_json::Value>(&response.body)
        .ok()
        .and_then(|value| {
            value
                .get("error")
                .and_then(|error| error.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| format!("{fallback}（HTTP {}）", response.status))
}

fn lifecycle_from_response(
    response: NativeResponse,
    operation: &str,
) -> Result<LifecycleSnapshot, String> {
    if !(200..300).contains(&response.status) {
        return Err(error_from_response(
            &response,
            &format!("后台服务{operation}失败"),
        ));
    }
    serde_json::from_str(&response.body).map_err(|_| format!("后台服务{operation}响应格式无效"))
}

async fn request_lease(
    coordinator: &LifecycleCoordinator,
    operation: LeaseOperation,
    lifecycle: &LifecycleSnapshot,
    allow_during_restart: bool,
) -> Result<LifecycleSnapshot, String> {
    if !allow_during_restart && coordinator.operation_in_progress.load(Ordering::Acquire) {
        return Err("后台服务管理操作期间不会修改管理租约".to_string());
    }
    let now = current_time_ms();
    let owner = lifecycle.management.installation_id.as_deref();
    let active = lifecycle
        .management
        .lease_expires_at_ms
        .is_some_and(|expiry| expiry > now);
    match operation {
        LeaseOperation::Claim => {
            if active && owner.is_some_and(|owner| owner != coordinator.installation_id) {
                return Err("后台服务已由其他安装管理，不会自动接管".to_string());
            }
        }
        LeaseOperation::Renew => {
            if !active
                || !lifecycle.management.can_control
                || owner != Some(coordinator.installation_id.as_str())
                || lifecycle.management.lease_generation.is_none()
            {
                return Err("当前 Windows 安装不持有有效的后台服务管理租约".to_string());
            }
        }
    }

    let identity = verify_native_identity(lifecycle)?;
    let body = serde_json::to_string(&LifecycleLeaseRequest {
        installation_id: &coordinator.installation_id,
        daemon_instance_id: &lifecycle.service.instance_id,
        daemon_identity: &identity,
    })
    .map_err(|error| format!("无法编码后台服务租约请求：{error}"))?;
    let response = super::native_lifecycle_post(operation.path(), body).await?;
    let result = lifecycle_from_response(response, "管理租约")?;
    if result.service.instance_id != lifecycle.service.instance_id
        || result.management.installation_id.as_deref()
            != Some(coordinator.installation_id.as_str())
        || !result.management.can_control
        || result.management.lease_generation.is_none()
    {
        return Err("后台服务返回的管理租约身份不一致".to_string());
    }
    Ok(result)
}

fn replacement_matches(previous: &LifecycleSnapshot, candidate: &LifecycleSnapshot) -> bool {
    candidate.service.ready
        && candidate.service.instance_id != previous.service.instance_id
        && candidate.runtime.build_number.is_some()
        && candidate.runtime.build_number == previous.runtime.build_number
        && candidate.runtime.product_version == previous.runtime.product_version
        && paths_match(&candidate.executable, &previous.executable)
        && candidate.bind == previous.bind
        && candidate
            .executable_sha256
            .as_deref()
            .zip(previous.executable_sha256.as_deref())
            .is_some_and(|(candidate, previous)| candidate.eq_ignore_ascii_case(previous))
}

async fn wait_for_verified_replacement(
    previous: &LifecycleSnapshot,
) -> Result<LifecycleSnapshot, String> {
    let deadline = Instant::now() + REPLACEMENT_WAIT;
    let mut stable: Option<(String, u32, u64)> = None;
    let mut stable_count = 0;
    while Instant::now() < deadline {
        tokio::time::sleep(REPLACEMENT_POLL).await;
        let Ok(response) = super::native_lifecycle_get().await else {
            stable = None;
            stable_count = 0;
            continue;
        };
        let Ok(candidate) = lifecycle_from_response(response, "重启恢复检查") else {
            stable = None;
            stable_count = 0;
            continue;
        };
        if !replacement_matches(previous, &candidate) || verify_native_identity(&candidate).is_err()
        {
            stable = None;
            stable_count = 0;
            continue;
        }
        let identity = (
            candidate.service.instance_id.clone(),
            candidate.service.pid,
            candidate.service.started_at_ms,
        );
        if stable.as_ref() == Some(&identity) {
            stable_count += 1;
        } else {
            stable = Some(identity);
            stable_count = 1;
        }
        if stable_count >= REPLACEMENT_STABLE_PROBES {
            return Ok(candidate);
        }
    }
    Err("后台服务未能以相同路径和构建在预期时间内恢复".to_string())
}

#[tauri::command]
pub(crate) fn lifecycle_installation_id(coordinator: State<'_, LifecycleCoordinator>) -> String {
    coordinator.installation_id.clone()
}

#[tauri::command]
pub(crate) async fn lifecycle_lease(
    operation: String,
    lifecycle: LifecycleSnapshot,
    coordinator: State<'_, LifecycleCoordinator>,
) -> Result<LifecycleSnapshot, String> {
    request_lease(
        coordinator.inner(),
        LeaseOperation::parse(&operation)?,
        &lifecycle,
        false,
    )
    .await
}

fn credential_mutation_from_response(
    response: NativeResponse,
    operation: &str,
    request_id: &str,
) -> Result<CredentialMutationResponse, String> {
    if !(200..300).contains(&response.status) {
        return Err(error_from_response(
            &response,
            &format!("后台服务{operation}失败"),
        ));
    }
    let mutation: CredentialMutationResponse = serde_json::from_str(&response.body)
        .map_err(|_| format!("后台服务{operation}响应格式无效"))?;
    let _ = mutation.rotated;
    if !mutation.ok || mutation.request_id != request_id || mutation.management_token_generation == 0 {
        return Err(format!("后台服务没有确认{operation}"));
    }
    Ok(mutation)
}

async fn validated_lifecycle_after_credential_mutation(
    coordinator: &LifecycleCoordinator,
    previous: &LifecycleSnapshot,
    mutation: &CredentialMutationResponse,
) -> Result<LifecycleSnapshot, String> {
    let response = super::native_lifecycle_get().await?;
    let refreshed = lifecycle_from_response(response, "管理状态复核")?;
    let lease_is_active = refreshed
        .management
        .lease_expires_at_ms
        .is_some_and(|expiry| expiry > current_time_ms());
    if refreshed.service.instance_id != previous.service.instance_id
        || !refreshed.management.can_control
        || refreshed.management.installation_id.as_deref()
            != Some(coordinator.installation_id.as_str())
        || refreshed.management.lease_generation.is_none()
        || !lease_is_active
        || refreshed.management.management_token_generation
            != Some(mutation.management_token_generation)
    {
        return Err("后台服务管理状态校验失败，请刷新后重试".to_string());
    }
    verify_native_identity(&refreshed)?;
    Ok(refreshed)
}

#[tauri::command]
pub(crate) async fn lifecycle_takeover(
    lifecycle: LifecycleSnapshot,
    coordinator: State<'_, LifecycleCoordinator>,
) -> Result<LifecycleSnapshot, String> {
    let coordinator = coordinator.inner();
    let _operation = coordinator.begin_operation()?;
    let owner = lifecycle.management.installation_id.as_deref();
    let conflicting_lease = owner.is_some_and(|owner| owner != coordinator.installation_id)
        && lifecycle
            .management
            .lease_expires_at_ms
            .is_none_or(|expiry| expiry > current_time_ms());
    if !conflicting_lease {
        return Err("确认后后台服务管理租约已变化，请刷新状态并重新确认".to_string());
    }
    let expected_lease_generation = lifecycle
        .management
        .lease_generation
        .ok_or_else(|| "后台服务管理租约缺少 generation".to_string())?;
    let expected_management_token_generation = lifecycle
        .management
        .management_token_generation
        .ok_or_else(|| "后台服务管理凭据缺少 generation".to_string())?;
    let identity = verify_native_identity(&lifecycle)?;
    let request_id = Uuid::new_v4().to_string();
    let body = serde_json::to_string(&LifecycleTakeoverRequest {
        installation_id: &coordinator.installation_id,
        daemon_instance_id: &lifecycle.service.instance_id,
        expected_lease_generation,
        expected_management_token_generation,
        request_id: &request_id,
        force: true,
        daemon_identity: &identity,
    })
    .map_err(|error| format!("无法编码后台服务接管请求：{error}"))?;
    let response = super::native_lifecycle_post(
        "api/v1/manage/lifecycle/lease/takeover",
        body,
    )
    .await?;
    let mutation = credential_mutation_from_response(response, "接管管理权", &request_id)?;
    validated_lifecycle_after_credential_mutation(coordinator, &lifecycle, &mutation).await
}

#[tauri::command]
pub(crate) async fn lifecycle_rotate_credential(
    lifecycle: LifecycleSnapshot,
    coordinator: State<'_, LifecycleCoordinator>,
) -> Result<LifecycleSnapshot, String> {
    let coordinator = coordinator.inner();
    let _operation = coordinator.begin_operation()?;
    let lease_is_active = lifecycle
        .management
        .lease_expires_at_ms
        .is_some_and(|expiry| expiry > current_time_ms());
    if !lifecycle.management.can_control
        || lifecycle.management.installation_id.as_deref()
            != Some(coordinator.installation_id.as_str())
        || !lease_is_active
    {
        return Err("确认后后台服务管理状态已变化，请刷新状态并重新确认".to_string());
    }
    let lease_generation = lifecycle
        .management
        .lease_generation
        .ok_or_else(|| "后台服务管理租约缺少 generation".to_string())?;
    let expected_management_token_generation = lifecycle
        .management
        .management_token_generation
        .ok_or_else(|| "后台服务管理凭据缺少 generation".to_string())?;
    verify_native_identity(&lifecycle)?;
    let request_id = Uuid::new_v4().to_string();
    let body = serde_json::to_string(&LifecycleCredentialRotateRequest {
        installation_id: &coordinator.installation_id,
        daemon_instance_id: &lifecycle.service.instance_id,
        lease_generation,
        expected_management_token_generation,
        request_id: &request_id,
        reason: "leakRecovery",
    })
    .map_err(|error| format!("无法编码后台服务凭据轮换请求：{error}"))?;
    let response = super::native_lifecycle_post(
        "api/v1/manage/lifecycle/credential/rotate",
        body,
    )
    .await?;
    let mutation = credential_mutation_from_response(response, "重新生成管理凭据", &request_id)?;
    validated_lifecycle_after_credential_mutation(coordinator, &lifecycle, &mutation).await
}

#[tauri::command]
pub(crate) async fn lifecycle_safe_restart(
    lifecycle: LifecycleSnapshot,
    coordinator: State<'_, LifecycleCoordinator>,
) -> Result<LifecycleSnapshot, String> {
    let coordinator = coordinator.inner();
    let _operation = coordinator.begin_operation()?;
    if lifecycle.runtime.build_number.is_none() {
        return Err("后台服务未提供构建号，不能验证同构建重启".to_string());
    }
    if lifecycle.protected_work_items.total != 0 {
        return Err(format!(
            "后台服务仍有 {} 项受保护任务，已取消重启",
            lifecycle.protected_work_items.total
        ));
    }
    let now = current_time_ms();
    if !lifecycle.management.can_control
        || lifecycle.management.installation_id.as_deref()
            != Some(coordinator.installation_id.as_str())
        || lifecycle
            .management
            .lease_expires_at_ms
            .is_none_or(|expiry| expiry <= now)
    {
        return Err("当前 Windows 安装不持有有效的后台服务管理租约".to_string());
    }
    let lease_generation = lifecycle
        .management
        .lease_generation
        .ok_or_else(|| "后台服务管理租约缺少 generation".to_string())?;
    // Re-observe the native process immediately before the destructive request.
    let identity = verify_native_identity(&lifecycle)?;
    let exit_waiter = ProcessExitWaiter::open(lifecycle.service.pid)?;
    let body = serde_json::to_string(&LifecycleRestartRequest {
        installation_id: &coordinator.installation_id,
        daemon_instance_id: &lifecycle.service.instance_id,
        lease_generation,
        force: false,
    })
    .map_err(|error| format!("无法编码后台服务重启请求：{error}"))?;
    let response = super::native_lifecycle_post("api/v1/manage/lifecycle/restart", body).await?;
    if !(200..300).contains(&response.status) {
        return Err(error_from_response(&response, "后台服务拒绝安全重启"));
    }
    let accepted: RestartResponse =
        serde_json::from_str(&response.body).map_err(|_| "后台服务重启响应格式无效".to_string())?;
    if !accepted.ok || accepted.state != "restarting" {
        return Err("后台服务没有接受安全重启请求".to_string());
    }

    exit_waiter.wait(OLD_PROCESS_EXIT_WAIT).await?;
    relaunch_verified_daemon(&lifecycle, &identity.executable_sha256)?;

    // This path never installs, stages, switches, kills, or selects the bundled
    // sidecar. It only starts the exact executable just observed from the old
    // process, with the old config, then verifies the replacement before claim.
    let replacement = wait_for_verified_replacement(&lifecycle).await?;
    request_lease(coordinator, LeaseOperation::Claim, &replacement, true).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lifecycle() -> LifecycleSnapshot {
        LifecycleSnapshot {
            service: LifecycleService {
                service: "mochiport".to_string(),
                api_major: 1,
                ready: true,
                instance_id: "daemon-a".to_string(),
                pid: 42,
                started_at_ms: 1_700_000_000_500,
            },
            executable: "C:/Program Files/MochiPort/mochiport-daemon.exe".to_string(),
            executable_sha256: Some("a".repeat(64)),
            config_path: "C:/Users/test/AppData/Local/MochiPort/config.toml".to_string(),
            bind: "127.0.0.1:3847".to_string(),
            runtime: LifecycleRuntime {
                state: "active".to_string(),
                product_version: "0.5.3".to_string(),
                build_number: Some(439),
                api_major: 1,
            },
            protected_work_items: ProtectedWorkItems {
                ai_gateway_requests: 0,
                codex_turns: 0,
                enhanced_launches: Some(0),
                im_streams: 0,
                pending_approvals: 0,
                remote_control_requests: 0,
                total: 0,
            },
            management: LifecycleManagement {
                state: "managed".to_string(),
                mode: "managed".to_string(),
                can_control: true,
                installation_id: Some("installation-a".to_string()),
                lease_generation: Some(7),
                lease_expires_at_ms: Some(u64::MAX),
                management_token_generation: Some(3),
            },
        }
    }

    fn locator(lifecycle: &LifecycleSnapshot) -> ActiveDaemonLocator {
        ActiveDaemonLocator {
            service: "mochiport".to_string(),
            api_major: 1,
            instance_id: lifecycle.service.instance_id.clone(),
            pid: lifecycle.service.pid,
            started_at_ms: lifecycle.service.started_at_ms,
            base_url: "http://127.0.0.1:3847".to_string(),
            control_file: "C:/Users/test/AppData/Local/MochiPort/mochiport-control.json"
                .to_string(),
        }
    }

    fn observed(lifecycle: &LifecycleSnapshot) -> ObservedProcess {
        ObservedProcess {
            pid: lifecycle.service.pid,
            created_at_ms: lifecycle.service.started_at_ms - 500,
            executable: "c:\\program files\\mochiport\\MOCHIPORT-DAEMON.EXE".to_string(),
            executable_sha256: "a".repeat(64),
        }
    }

    #[test]
    fn installation_identity_is_created_once_and_reused() {
        let directory = std::env::temp_dir().join(format!(
            "mochiport-lifecycle-identity-{}",
            Uuid::new_v4().simple()
        ));
        let first = LifecycleCoordinator::load(&directory).unwrap();
        let second = LifecycleCoordinator::load(&directory).unwrap();
        assert_eq!(first.installation_id, second.installation_id);
        assert!(Uuid::parse_str(&first.installation_id).is_ok());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn identity_verification_requires_pid_start_path_sha_bind_and_locator() {
        let lifecycle = lifecycle();
        let active_locator = locator(&lifecycle);
        let process = observed(&lifecycle);
        let proof = verify_observation(&lifecycle, &active_locator, &process).unwrap();
        assert_eq!(proof.pid, 42);
        assert_eq!(proof.executable_sha256, "a".repeat(64));

        let mut wrong = observed(&lifecycle);
        wrong.pid = 43;
        assert!(verify_observation(&lifecycle, &active_locator, &wrong).is_err());
        let mut wrong = observed(&lifecycle);
        wrong.created_at_ms -= PROCESS_START_GRACE_AFTER_MS + 1;
        assert!(verify_observation(&lifecycle, &active_locator, &wrong).is_err());
        let mut wrong = observed(&lifecycle);
        wrong.executable = "C:/Temp/mochiport-daemon.exe".to_string();
        assert!(verify_observation(&lifecycle, &active_locator, &wrong).is_err());
        let mut wrong = observed(&lifecycle);
        wrong.executable_sha256 = "b".repeat(64);
        assert!(verify_observation(&lifecycle, &active_locator, &wrong).is_err());
        let mut wrong_locator = locator(&lifecycle);
        wrong_locator.base_url = "http://127.0.0.1:9999".to_string();
        assert!(verify_observation(&lifecycle, &wrong_locator, &process).is_err());
    }

    #[test]
    fn replacement_must_be_new_and_keep_path_build_version_sha_and_bind() {
        let previous = lifecycle();
        let mut replacement = previous.clone();
        replacement.service.instance_id = "daemon-b".to_string();
        replacement.service.pid = 84;
        replacement.service.started_at_ms += 10_000;
        replacement.executable = "c:\\PROGRAM FILES\\MochiPort\\mochiport-daemon.exe".to_string();
        assert!(replacement_matches(&previous, &replacement));

        let mut wrong = replacement.clone();
        wrong.runtime.build_number = Some(440);
        assert!(!replacement_matches(&previous, &wrong));
        let mut wrong = replacement.clone();
        wrong.executable = "C:/Other/mochiport-daemon.exe".to_string();
        assert!(!replacement_matches(&previous, &wrong));
        let mut wrong = replacement.clone();
        wrong.executable_sha256 = Some("b".repeat(64));
        assert!(!replacement_matches(&previous, &wrong));
        assert!(!replacement_matches(&previous, &previous));
    }

    #[test]
    fn restart_payload_is_always_non_forced_and_fenced() {
        let request = LifecycleRestartRequest {
            installation_id: "installation-a",
            daemon_instance_id: "daemon-a",
            lease_generation: 7,
            force: false,
        };
        let json = serde_json::to_value(request).unwrap();
        assert_eq!(json["force"], false);
        assert_eq!(json["daemonInstanceId"], "daemon-a");
        assert_eq!(json["leaseGeneration"], 7);
        assert!(json.get("candidatePath").is_none());
        assert_eq!(
            relaunch_arguments("C:\\Users\\Test\\MochiPort\\config.toml"),
            [
                "--config",
                "C:\\Users\\Test\\MochiPort\\config.toml",
                "daemon",
            ]
        );
    }
}
