use std::{
    collections::HashSet,
    env,
    ffi::OsString,
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    time::Duration,
};

use anyhow::{Context, Result, anyhow};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

use crate::config::AppConfig;

const DISABLE_ENV: &str = "CODEX_REMOTE_DISABLE";
const REAL_CODEX_ENV: &str = "CODEX_REMOTE_REAL_CODEX";
const SHIM_MODE_ENV: &str = "CODEX_REMOTE_SHIM";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShimStatusResponse {
    pub available: bool,
    pub enabled: bool,
    pub relay_url: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShimInstallReport {
    pub shim_path: PathBuf,
    pub bin_dir: PathBuf,
    pub real_codex_path: PathBuf,
    pub path_update: PathUpdate,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShimUninstallReport {
    pub shim_path: PathBuf,
    pub bin_dir: PathBuf,
    pub removed_shim: bool,
    pub path_update: PathUpdate,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexCandidate {
    pub path: PathBuf,
    pub source: String,
    pub confidence: u8,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ShimSessionRequest {
    cwd: String,
    upstream_ws_url: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShimSessionResponse {
    ok: bool,
    relay_url: String,
    upstream_ws_url: String,
    error: Option<String>,
}

pub fn install_shim(
    config: &mut AppConfig,
    config_path: &Path,
    real_codex: Option<PathBuf>,
    bin_dir: Option<PathBuf>,
) -> Result<ShimInstallReport> {
    let bin_dir = bin_dir
        .or_else(|| {
            (!config.shim.bin_dir.as_os_str().is_empty()).then(|| config.shim.bin_dir.clone())
        })
        .unwrap_or_else(|| AppConfig::default().shim.bin_dir);
    let bin_dir = absolutize(bin_dir)?;
    std::fs::create_dir_all(&bin_dir)
        .with_context(|| format!("failed to create shim directory {}", bin_dir.display()))?;

    let configured_real_codex = config
        .shim
        .real_codex_path
        .clone()
        .filter(|path| !path.as_os_str().is_empty());
    let real_codex = match real_codex {
        Some(path) => absolutize(path)?,
        None => match configured_real_codex {
            Some(path) => {
                let path = absolutize(path)?;
                if path.exists() && !looks_like_our_shim(&path) {
                    path
                } else {
                    find_real_codex(&bin_dir)?
                }
            }
            None => find_real_codex(&bin_dir)?,
        },
    };
    if !real_codex.exists() {
        anyhow::bail!("real codex path does not exist: {}", real_codex.display());
    }
    if looks_like_our_shim(&real_codex) {
        anyhow::bail!(
            "selected codex path points to codex-remote shim, not real Codex: {}",
            real_codex.display()
        );
    }

    let exe = env::current_exe().context("failed to resolve codex-remote executable")?;
    let shim = bin_dir.join("codex.cmd");
    let posix_shim = bin_dir.join("codex");
    let config_arg = config_path.display().to_string();
    let script = format!(
        "@echo off\r\n\"{}\" --config \"{}\" shim -- %*\r\n",
        exe.display(),
        config_arg
    );
    std::fs::write(&shim, script)
        .with_context(|| format!("failed to write shim {}", shim.display()))?;
    write_posix_shim(&posix_shim, config_path)?;

    config.shim.bin_dir = bin_dir.clone();
    config.shim.real_codex_path = Some(real_codex.clone());
    config.save(&config_path.to_path_buf())?;

    let path_update = ensure_user_path_prepend(&bin_dir)?;
    match path_update {
        PathUpdate::Added => {
            println!("added shim directory to user PATH");
            println!("restart the terminal for PATH changes to take effect");
        }
        PathUpdate::AlreadyPresent => println!("shim directory is already in user PATH"),
        PathUpdate::Unsupported => {
            println!("add this shim directory before official codex in PATH:")
        }
    }
    match ensure_shell_profile_prepend(&bin_dir)? {
        PathUpdate::Added => println!("added shim directory to shell profile"),
        PathUpdate::AlreadyPresent => println!("shim directory is already in shell profile"),
        PathUpdate::Unsupported => {}
    }
    println!("installed shim: {}", shim.display());
    println!("real codex: {}", real_codex.display());
    println!("{}", bin_dir.display());
    Ok(ShimInstallReport {
        shim_path: shim,
        bin_dir,
        real_codex_path: real_codex,
        path_update,
    })
}

pub fn uninstall_shim(config: &AppConfig) -> Result<ShimUninstallReport> {
    let shim = config.shim.bin_dir.join("codex.cmd");
    let posix_shim = config.shim.bin_dir.join("codex");
    let removed_shim = shim.exists() || posix_shim.exists();
    if removed_shim {
        if shim.exists() {
            std::fs::remove_file(&shim)
                .with_context(|| format!("failed to remove shim {}", shim.display()))?;
            println!("removed shim: {}", shim.display());
        }
        if posix_shim.exists() {
            std::fs::remove_file(&posix_shim)
                .with_context(|| format!("failed to remove shim {}", posix_shim.display()))?;
            println!("removed shim: {}", posix_shim.display());
        }
    } else {
        println!("shim not found: {}", shim.display());
    }
    let path_update = remove_user_path_entry(&config.shim.bin_dir)?;
    match path_update {
        PathUpdate::Added => println!("removed shim directory from user PATH"),
        PathUpdate::AlreadyPresent => println!("shim directory was not present in user PATH"),
        PathUpdate::Unsupported => {
            println!("remove this shim directory from PATH if you added it manually:")
        }
    }
    match remove_shell_profile_prepend(&config.shim.bin_dir)? {
        PathUpdate::Added => println!("removed shim directory from shell profile"),
        PathUpdate::AlreadyPresent => {}
        PathUpdate::Unsupported => {}
    }
    Ok(ShimUninstallReport {
        shim_path: shim,
        bin_dir: config.shim.bin_dir.clone(),
        removed_shim,
        path_update,
    })
}

pub fn shim_path(config: &AppConfig) -> PathBuf {
    config.shim.bin_dir.join("codex.cmd")
}

pub fn user_path_contains_dir(dir: &Path) -> Result<Option<bool>> {
    user_path_contains_dir_impl(dir)
}

pub fn discover_codex_candidates(shim_bin_dir: &Path) -> Vec<CodexCandidate> {
    let mut candidates = Vec::new();
    let mut seen = HashSet::new();
    collect_path_candidates(shim_bin_dir, &mut candidates, &mut seen);
    collect_command_candidates(shim_bin_dir, &mut candidates, &mut seen);
    collect_known_install_candidates(shim_bin_dir, &mut candidates, &mut seen);
    candidates.sort_by(|a, b| {
        b.confidence
            .cmp(&a.confidence)
            .then_with(|| a.path.cmp(&b.path))
    });
    candidates
}

pub async fn set_enabled(config_path: &Path, enabled: bool) -> Result<()> {
    let mut config = AppConfig::load_or_default(&config_path.to_path_buf())?;
    config.bridge.enabled = enabled;
    config.save(&config_path.to_path_buf())?;
    let _ = notify_daemon_enabled(&config, enabled).await;
    println!(
        "codex-remote bridge {}",
        if enabled { "enabled" } else { "disabled" }
    );
    Ok(())
}

pub async fn print_status(config: &AppConfig) -> Result<()> {
    println!(
        "bridge: {}",
        if config.bridge.enabled {
            "enabled"
        } else {
            "disabled"
        }
    );
    println!("shim dir: {}", config.shim.bin_dir.to_string_lossy());
    println!(
        "real codex: {}",
        config
            .shim
            .real_codex_path
            .as_ref()
            .map(|path| path.to_string_lossy().to_string())
            .unwrap_or_else(|| "<not configured>".to_string())
    );
    let status = query_daemon_status(config).await;
    match status {
        Ok(status) => {
            println!(
                "daemon: {} ({})",
                if status.available {
                    "available"
                } else {
                    "unavailable"
                },
                status.reason.unwrap_or_else(|| status.relay_url)
            );
        }
        Err(err) => println!("daemon: unavailable ({err})"),
    }
    Ok(())
}

pub async fn run_shim(config: &AppConfig, args: Vec<String>) -> Result<i32> {
    let real_codex = real_codex_path(config)?;
    notify_daemon_event(
        config,
        "info",
        "shim_invoked",
        format!("args={args:?} real_codex={}", real_codex.display()),
    )
    .await;
    if should_bypass(&args) {
        notify_daemon_event(config, "info", "shim_passthrough", "bypass command").await;
        return run_codex_passthrough(&real_codex, args);
    }
    if env_truthy(DISABLE_ENV) || !config.bridge.enabled {
        notify_daemon_event(
            config,
            "info",
            "shim_passthrough",
            "disabled by env or bridge config",
        )
        .await;
        return run_codex_passthrough(&real_codex, args);
    }

    if config.feishu.app_id.trim().is_empty() || config.feishu.app_secret.trim().is_empty() {
        notify_daemon_event(
            config,
            "info",
            "shim_passthrough",
            "Feishu is not configured",
        )
        .await;
        return run_codex_passthrough(&real_codex, args);
    }

    match query_daemon_status(config).await {
        Ok(status) if status.available && status.enabled => status,
        Ok(status) => {
            notify_daemon_event(
                config,
                "warn",
                "shim_passthrough",
                format!("daemon unavailable: {:?}", status.reason),
            )
            .await;
            return run_codex_passthrough(&real_codex, args);
        }
        Err(err) => {
            notify_daemon_event(
                config,
                "warn",
                "shim_passthrough",
                format!("daemon status failed: {err}"),
            )
            .await;
            return run_codex_passthrough(&real_codex, args);
        }
    };

    let cwd = env::current_dir().context("failed to resolve current directory")?;
    let upstream_addr = reserve_local_addr()?;
    let upstream_url = format!("ws://{upstream_addr}");
    let session = match register_session(config, &cwd, &upstream_url).await {
        Ok(session) if session.ok => session,
        Ok(session) => {
            notify_daemon_event(
                config,
                "warn",
                "shim_passthrough",
                format!("daemon refused session: {:?}", session.error),
            )
            .await;
            return run_codex_passthrough(&real_codex, args);
        }
        Err(err) => {
            notify_daemon_event(
                config,
                "warn",
                "shim_passthrough",
                format!("register session failed: {err}"),
            )
            .await;
            return run_codex_passthrough(&real_codex, args);
        }
    };
    notify_daemon_event(
        config,
        "info",
        "shim_app_server_start",
        format!("cwd={} upstream={}", cwd.display(), session.upstream_ws_url),
    )
    .await;

    let mut app_server = Command::new(&real_codex)
        .arg("app-server")
        .arg("--listen")
        .arg(&session.upstream_ws_url)
        .current_dir(&cwd)
        .env(SHIM_MODE_ENV, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| {
            format!(
                "failed to start codex app-server from {}",
                real_codex.display()
            )
        })?;

    if let Err(err) = wait_ready(&session.upstream_ws_url).await {
        let _ = app_server.kill();
        notify_daemon_event(
            config,
            "warn",
            "shim_passthrough",
            format!("app-server not ready: {err}"),
        )
        .await;
        return run_codex_passthrough(&real_codex, args);
    }
    notify_daemon_event(
        config,
        "info",
        "shim_app_server_ready",
        session.upstream_ws_url.clone(),
    )
    .await;

    let mut remote_args = Vec::with_capacity(args.len() + 2);
    remote_args.push("--remote".to_string());
    remote_args.push(session.relay_url);
    remote_args.extend(args);
    notify_daemon_event(
        config,
        "info",
        "shim_tui_start",
        format!("real_codex={} args={remote_args:?}", real_codex.display()),
    )
    .await;

    let exit = run_codex_status(&real_codex, remote_args)?;
    notify_daemon_event(
        config,
        "info",
        "shim_tui_exit",
        format!("exit={}", exit_code(exit)),
    )
    .await;
    let _ = app_server.kill();
    let _ = app_server.wait();
    Ok(exit_code(exit))
}

fn should_bypass(args: &[String]) -> bool {
    if env_truthy(SHIM_MODE_ENV) {
        return true;
    }
    matches!(
        args.first().map(String::as_str),
        Some("app-server")
            | Some("exec")
            | Some("review")
            | Some("login")
            | Some("logout")
            | Some("doctor")
            | Some("auth")
            | Some("mcp")
            | Some("mcp-server")
            | Some("plugin")
            | Some("app")
            | Some("update")
            | Some("cloud")
            | Some("sandbox")
            | Some("debug")
            | Some("execpolicy")
            | Some("apply")
            | Some("responses-api-proxy")
            | Some("stdio-to-uds")
            | Some("exec-server")
            | Some("features")
            | Some("remote-control")
            | Some("proto")
            | Some("completion")
            | Some("--help")
            | Some("-h")
            | Some("--version")
            | Some("-V")
    ) || args.iter().any(|arg| arg == "--remote")
}

fn run_codex_passthrough(real_codex: &Path, args: Vec<String>) -> Result<i32> {
    let status = run_codex_status(real_codex, args)?;
    Ok(exit_code(status))
}

fn run_codex_status(real_codex: &Path, args: Vec<String>) -> Result<ExitStatus> {
    Command::new(real_codex)
        .args(args)
        .env(SHIM_MODE_ENV, "1")
        .status()
        .with_context(|| format!("failed to run real codex {}", real_codex.display()))
}

fn exit_code(status: ExitStatus) -> i32 {
    status.code().unwrap_or(1)
}

async fn query_daemon_status(config: &AppConfig) -> Result<ShimStatusResponse> {
    let url = format!("http://{}/api/shim/status", config.bind);
    let response = reqwest::Client::new()
        .get(url)
        .timeout(Duration::from_millis(700))
        .send()
        .await?;
    if response.status() != StatusCode::OK {
        anyhow::bail!("daemon returned {}", response.status());
    }
    response
        .json::<ShimStatusResponse>()
        .await
        .map_err(Into::into)
}

async fn register_session(
    config: &AppConfig,
    cwd: &Path,
    upstream_ws_url: &str,
) -> Result<ShimSessionResponse> {
    let url = format!("http://{}/api/shim/session", config.bind);
    let response = reqwest::Client::new()
        .post(url)
        .json(&ShimSessionRequest {
            cwd: cwd.to_string_lossy().to_string(),
            upstream_ws_url: upstream_ws_url.to_string(),
        })
        .timeout(Duration::from_millis(900))
        .send()
        .await?;
    if response.status() != StatusCode::OK {
        anyhow::bail!("daemon returned {}", response.status());
    }
    let session = response.json::<ShimSessionResponse>().await?;
    if !session.ok {
        anyhow::bail!(
            "{}",
            session
                .error
                .unwrap_or_else(|| "daemon refused shim session".to_string())
        );
    }
    Ok(session)
}

async fn notify_daemon_enabled(config: &AppConfig, enabled: bool) -> Result<()> {
    let url = format!("http://{}/api/shim/enabled", config.bind);
    reqwest::Client::new()
        .post(url)
        .json(&serde_json::json!({ "enabled": enabled }))
        .timeout(Duration::from_millis(700))
        .send()
        .await?;
    Ok(())
}

async fn notify_daemon_event(
    config: &AppConfig,
    level: &str,
    kind: &str,
    message: impl Into<String>,
) {
    let url = format!("http://{}/api/shim/event", config.bind);
    let _ = reqwest::Client::new()
        .post(url)
        .json(&serde_json::json!({
            "level": level,
            "kind": kind,
            "message": message.into(),
        }))
        .timeout(Duration::from_millis(700))
        .send()
        .await;
}

async fn wait_ready(upstream_ws_url: &str) -> Result<()> {
    let health_url = upstream_ws_url
        .replacen("ws://", "http://", 1)
        .replacen("wss://", "https://", 1)
        + "/readyz";
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        if tokio::time::Instant::now() > deadline {
            anyhow::bail!("timed out waiting for app-server {health_url}");
        }
        if let Ok(response) = client
            .get(&health_url)
            .timeout(Duration::from_millis(250))
            .send()
            .await
            && response.status().is_success()
        {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn reserve_local_addr() -> Result<String> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?;
    drop(listener);
    Ok(addr.to_string())
}

fn real_codex_path(config: &AppConfig) -> Result<PathBuf> {
    if let Some(path) = env::var_os(REAL_CODEX_ENV).map(PathBuf::from)
        && !path.as_os_str().is_empty()
    {
        return Ok(path);
    }
    config
        .shim
        .real_codex_path
        .clone()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| {
            anyhow!("real codex path is not configured; run `codex-remote install-shim`")
        })
}

fn find_real_codex(shim_bin_dir: &Path) -> Result<PathBuf> {
    discover_codex_candidates(shim_bin_dir)
        .into_iter()
        .next()
        .map(|candidate| candidate.path)
        .ok_or_else(|| {
            anyhow!(
                "could not find real Codex automatically; install Codex first or set real Codex path in advanced settings"
            )
        })
}

fn collect_path_candidates(
    shim_bin_dir: &Path,
    candidates: &mut Vec<CodexCandidate>,
    seen: &mut HashSet<String>,
) {
    for dir in env::split_paths(&env::var_os("PATH").unwrap_or_default()) {
        add_candidates_from_dir(shim_bin_dir, &dir, "PATH", 100, candidates, seen);
    }
    #[cfg(windows)]
    if let Ok(path) = read_user_path() {
        for dir in split_path_value(&path) {
            add_candidates_from_dir(
                shim_bin_dir,
                &dir,
                "Windows user PATH",
                95,
                candidates,
                seen,
            );
        }
    }
}

fn collect_command_candidates(
    shim_bin_dir: &Path,
    candidates: &mut Vec<CodexCandidate>,
    seen: &mut HashSet<String>,
) {
    if let Some(prefix) = command_path("npm", &["prefix", "-g"]) {
        add_node_prefix_candidates(shim_bin_dir, &prefix, "npm prefix -g", 90, candidates, seen);
    }
    if let Some(root) = command_path("npm", &["root", "-g"]) {
        add_node_root_candidates(shim_bin_dir, &root, "npm root -g", 82, candidates, seen);
    }
    if let Some(dir) = command_path("pnpm", &["bin", "-g"]) {
        add_candidates_from_dir(shim_bin_dir, &dir, "pnpm bin -g", 88, candidates, seen);
    }
    if let Some(dir) = command_path("yarn", &["global", "bin"]) {
        add_candidates_from_dir(shim_bin_dir, &dir, "yarn global bin", 84, candidates, seen);
    }
    if let Some(prefix) = command_path("brew", &["--prefix"]) {
        add_candidates_from_dir(
            shim_bin_dir,
            &prefix.join("bin"),
            "Homebrew",
            84,
            candidates,
            seen,
        );
    }
}

fn collect_known_install_candidates(
    shim_bin_dir: &Path,
    candidates: &mut Vec<CodexCandidate>,
    seen: &mut HashSet<String>,
) {
    for name in ["NVM_SYMLINK", "FNM_MULTISHELL_PATH", "PNPM_HOME"] {
        if let Some(dir) = env_path(name) {
            add_candidates_from_dir(shim_bin_dir, &dir, name, 86, candidates, seen);
        }
    }
    if let Some(dir) = env_path("VOLTA_HOME") {
        add_candidates_from_dir(
            shim_bin_dir,
            &dir.join("bin"),
            "VOLTA_HOME",
            86,
            candidates,
            seen,
        );
    }
    if let Some(dir) = env_path("NVM_HOME") {
        add_nvm_candidates(shim_bin_dir, &dir, "NVM_HOME", 84, candidates, seen);
    }
    if let Some(dir) = env_path("ASDF_DATA_DIR") {
        add_candidates_from_dir(
            shim_bin_dir,
            &dir.join("shims"),
            "ASDF_DATA_DIR",
            80,
            candidates,
            seen,
        );
        add_versioned_bin_dirs(
            shim_bin_dir,
            &dir.join("installs").join("nodejs"),
            "ASDF nodejs",
            78,
            candidates,
            seen,
        );
    }
    if let Some(dir) = env_path("MISE_DATA_DIR") {
        add_candidates_from_dir(
            shim_bin_dir,
            &dir.join("shims"),
            "MISE_DATA_DIR",
            80,
            candidates,
            seen,
        );
        add_versioned_bin_dirs(
            shim_bin_dir,
            &dir.join("installs").join("node"),
            "MISE node",
            78,
            candidates,
            seen,
        );
    }

    if let Some(home) = home_dir() {
        add_candidates_from_dir(
            shim_bin_dir,
            &home.join(".volta").join("bin"),
            "Volta",
            84,
            candidates,
            seen,
        );
        add_candidates_from_dir(
            shim_bin_dir,
            &home.join(".asdf").join("shims"),
            "asdf",
            78,
            candidates,
            seen,
        );
        add_candidates_from_dir(
            shim_bin_dir,
            &home.join(".local").join("share").join("mise").join("shims"),
            "mise",
            78,
            candidates,
            seen,
        );
        add_candidates_from_dir(
            shim_bin_dir,
            &home.join(".bun").join("bin"),
            "Bun",
            72,
            candidates,
            seen,
        );
        add_nvm_candidates(
            shim_bin_dir,
            &home.join(".nvm"),
            "nvm",
            84,
            candidates,
            seen,
        );
        add_fnm_candidates(
            shim_bin_dir,
            &home.join(".fnm"),
            "fnm",
            82,
            candidates,
            seen,
        );
        add_versioned_bin_dirs(
            shim_bin_dir,
            &home.join(".asdf").join("installs").join("nodejs"),
            "asdf nodejs",
            76,
            candidates,
            seen,
        );
        add_versioned_bin_dirs(
            shim_bin_dir,
            &home
                .join(".local")
                .join("share")
                .join("mise")
                .join("installs")
                .join("node"),
            "mise node",
            76,
            candidates,
            seen,
        );

        #[cfg(windows)]
        {
            add_candidates_from_dir(
                shim_bin_dir,
                &home.join("scoop").join("shims"),
                "Scoop",
                82,
                candidates,
                seen,
            );
            add_candidates_from_dir(
                shim_bin_dir,
                &home.join("AppData").join("Roaming").join("npm"),
                "npm user bin",
                82,
                candidates,
                seen,
            );
            add_candidates_from_dir(
                shim_bin_dir,
                &home.join("AppData").join("Local").join("pnpm"),
                "pnpm user bin",
                80,
                candidates,
                seen,
            );
        }
    }

    #[cfg(windows)]
    {
        for name in [
            "APPDATA",
            "LOCALAPPDATA",
            "ProgramFiles",
            "ProgramFiles(x86)",
        ] {
            if let Some(dir) = env_path(name) {
                match name {
                    "APPDATA" => {
                        add_candidates_from_dir(
                            shim_bin_dir,
                            &dir.join("npm"),
                            name,
                            82,
                            candidates,
                            seen,
                        );
                    }
                    "LOCALAPPDATA" => {
                        add_candidates_from_dir(
                            shim_bin_dir,
                            &dir.join("pnpm"),
                            name,
                            80,
                            candidates,
                            seen,
                        );
                        add_candidates_from_dir(
                            shim_bin_dir,
                            &dir.join("Volta").join("bin"),
                            name,
                            80,
                            candidates,
                            seen,
                        );
                    }
                    _ => {
                        add_candidates_from_dir(
                            shim_bin_dir,
                            &dir.join("nodejs"),
                            name,
                            76,
                            candidates,
                            seen,
                        );
                    }
                }
            }
        }
        for dir in [
            PathBuf::from(r"C:\Program Files\nodejs"),
            PathBuf::from(r"C:\Program Files (x86)\nodejs"),
            PathBuf::from(r"C:\nvm4w\nodejs"),
        ] {
            add_candidates_from_dir(
                shim_bin_dir,
                &dir,
                "common Windows node dir",
                74,
                candidates,
                seen,
            );
        }
    }

    #[cfg(not(windows))]
    {
        for dir in [
            PathBuf::from("/usr/local/bin"),
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/usr/bin"),
        ] {
            add_candidates_from_dir(shim_bin_dir, &dir, "system bin", 74, candidates, seen);
        }
    }
}

fn add_node_prefix_candidates(
    shim_bin_dir: &Path,
    prefix: &Path,
    source: &str,
    confidence: u8,
    candidates: &mut Vec<CodexCandidate>,
    seen: &mut HashSet<String>,
) {
    if cfg!(windows) {
        add_candidates_from_dir(shim_bin_dir, prefix, source, confidence, candidates, seen);
    } else {
        add_candidates_from_dir(
            shim_bin_dir,
            &prefix.join("bin"),
            source,
            confidence,
            candidates,
            seen,
        );
    }
}

fn add_node_root_candidates(
    shim_bin_dir: &Path,
    root: &Path,
    source: &str,
    confidence: u8,
    candidates: &mut Vec<CodexCandidate>,
    seen: &mut HashSet<String>,
) {
    if let Some(parent) = root.parent() {
        add_candidates_from_dir(shim_bin_dir, parent, source, confidence, candidates, seen);
        if !cfg!(windows)
            && parent.file_name().and_then(|value| value.to_str()) == Some("lib")
            && let Some(prefix) = parent.parent()
        {
            add_candidates_from_dir(
                shim_bin_dir,
                &prefix.join("bin"),
                source,
                confidence,
                candidates,
                seen,
            );
        }
    }
}

fn add_nvm_candidates(
    shim_bin_dir: &Path,
    root: &Path,
    source: &str,
    confidence: u8,
    candidates: &mut Vec<CodexCandidate>,
    seen: &mut HashSet<String>,
) {
    add_candidates_from_dir(shim_bin_dir, root, source, confidence, candidates, seen);
    add_versioned_bin_dirs(
        shim_bin_dir,
        &root.join("versions").join("node"),
        source,
        confidence,
        candidates,
        seen,
    );
    add_versioned_dirs(
        shim_bin_dir,
        root,
        source,
        confidence.saturating_sub(4),
        candidates,
        seen,
    );
}

fn add_fnm_candidates(
    shim_bin_dir: &Path,
    root: &Path,
    source: &str,
    confidence: u8,
    candidates: &mut Vec<CodexCandidate>,
    seen: &mut HashSet<String>,
) {
    let versions = root.join("node-versions");
    if let Ok(entries) = std::fs::read_dir(&versions) {
        for entry in entries.flatten() {
            let path = entry.path();
            add_candidates_from_dir(
                shim_bin_dir,
                &path.join("installation").join("bin"),
                source,
                confidence,
                candidates,
                seen,
            );
            add_candidates_from_dir(
                shim_bin_dir,
                &path.join("bin"),
                source,
                confidence.saturating_sub(4),
                candidates,
                seen,
            );
        }
    }
}

fn add_versioned_bin_dirs(
    shim_bin_dir: &Path,
    root: &Path,
    source: &str,
    confidence: u8,
    candidates: &mut Vec<CodexCandidate>,
    seen: &mut HashSet<String>,
) {
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            add_candidates_from_dir(
                shim_bin_dir,
                &entry.path().join("bin"),
                source,
                confidence,
                candidates,
                seen,
            );
        }
    }
}

fn add_versioned_dirs(
    shim_bin_dir: &Path,
    root: &Path,
    source: &str,
    confidence: u8,
    candidates: &mut Vec<CodexCandidate>,
    seen: &mut HashSet<String>,
) {
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                add_candidates_from_dir(shim_bin_dir, &path, source, confidence, candidates, seen);
            }
        }
    }
}

fn add_candidates_from_dir(
    shim_bin_dir: &Path,
    dir: &Path,
    source: &str,
    confidence: u8,
    candidates: &mut Vec<CodexCandidate>,
    seen: &mut HashSet<String>,
) {
    if !dir.is_dir() || same_path(dir, shim_bin_dir) {
        return;
    }
    for name in codex_exe_names() {
        add_candidate(
            shim_bin_dir,
            dir.join(name),
            source,
            confidence,
            candidates,
            seen,
        );
    }
}

fn add_candidate(
    shim_bin_dir: &Path,
    path: PathBuf,
    source: &str,
    confidence: u8,
    candidates: &mut Vec<CodexCandidate>,
    seen: &mut HashSet<String>,
) {
    if !path.is_file() || looks_like_our_shim(&path) {
        return;
    }
    if path
        .parent()
        .is_some_and(|parent| same_path(parent, shim_bin_dir))
    {
        return;
    }
    let key = canonical_key(&path);
    if !seen.insert(key) {
        return;
    }
    candidates.push(CodexCandidate {
        path,
        source: source.to_string(),
        confidence,
    });
}

fn command_path(program: &str, args: &[&str]) -> Option<PathBuf> {
    let output = Command::new(program).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout);
    let first_line = value.lines().map(str::trim).find(|line| !line.is_empty())?;
    Some(PathBuf::from(first_line))
}

fn env_path(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("USERPROFILE")
        .or_else(|| env::var_os("HOME"))
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn canonical_key(path: &Path) -> String {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let value = path.to_string_lossy().to_string();
    if cfg!(windows) {
        value.to_ascii_lowercase()
    } else {
        value
    }
}

fn looks_like_our_shim(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .map(|content| content.contains("codex-remote") && content.contains(" shim "))
        .unwrap_or(false)
}

fn write_posix_shim(path: &Path, config_path: &Path) -> Result<()> {
    let exe =
        posix_shell_path(&env::current_exe().context("failed to resolve codex-remote executable")?);
    let config_path = posix_shell_path(&absolutize(config_path.to_path_buf())?);
    let script = format!(
        r#"#!/usr/bin/env sh
exec "{exe}" --config "{config_path}" shim -- "$@"
"#
    );
    std::fs::write(path, script)
        .with_context(|| format!("failed to write shim {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(path)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

fn posix_shell_path(path: &Path) -> String {
    let raw = path.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        let bytes = raw.as_bytes();
        if bytes.len() >= 3 && bytes[1] == b':' && bytes[2] == b'/' {
            let drive = (bytes[0] as char).to_ascii_lowercase();
            return format!("/{drive}/{}", &raw[3..]);
        }
    }
    raw
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PathUpdate {
    Added,
    AlreadyPresent,
    Unsupported,
}

#[cfg(windows)]
const _: Option<PathUpdate> = Some(PathUpdate::Unsupported);

#[cfg(windows)]
fn ensure_user_path_prepend(dir: &Path) -> Result<PathUpdate> {
    let current = read_user_path()?;
    let entries = split_path_value(&current);
    if entries.iter().any(|entry| same_path(entry, dir)) {
        return Ok(PathUpdate::AlreadyPresent);
    }
    let mut updated = Vec::with_capacity(entries.len() + 1);
    updated.push(dir.to_path_buf());
    updated.extend(entries);
    write_user_path(&join_path_value(&updated)?)?;
    Ok(PathUpdate::Added)
}

#[cfg(not(windows))]
fn ensure_user_path_prepend(_dir: &Path) -> Result<PathUpdate> {
    Ok(PathUpdate::Unsupported)
}

#[cfg(windows)]
fn remove_user_path_entry(dir: &Path) -> Result<PathUpdate> {
    let current = read_user_path()?;
    let entries = split_path_value(&current);
    let filtered = entries
        .iter()
        .filter(|entry| !same_path(entry, dir))
        .cloned()
        .collect::<Vec<_>>();
    if filtered.len() == entries.len() {
        return Ok(PathUpdate::AlreadyPresent);
    }
    write_user_path(&join_path_value(&filtered)?)?;
    Ok(PathUpdate::Added)
}

#[cfg(not(windows))]
fn remove_user_path_entry(_dir: &Path) -> Result<PathUpdate> {
    Ok(PathUpdate::Unsupported)
}

const SHELL_PROFILE_BEGIN: &str = "# >>> codex-remote shim >>>";
const SHELL_PROFILE_END: &str = "# <<< codex-remote shim <<<";

fn ensure_shell_profile_prepend(dir: &Path) -> Result<PathUpdate> {
    let Some(home) = home_dir() else {
        return Ok(PathUpdate::Unsupported);
    };
    let shell_dir = posix_shell_path(dir);
    let shell_shim = posix_shell_path(&dir.join("codex"));
    let block = format!(
        "{SHELL_PROFILE_BEGIN}\ncase \":$PATH:\" in\n  *\":{shell_dir}:\"*) ;;\n  *) export PATH=\"{shell_dir}:$PATH\" ;;\nesac\nhash -r 2>/dev/null || true\ncodex() {{\n  \"{shell_shim}\" \"$@\"\n}}\n{SHELL_PROFILE_END}\n"
    );
    let mut changed = false;
    for profile in shell_profile_paths(&home) {
        let existing = std::fs::read_to_string(&profile).unwrap_or_default();
        let cleaned = remove_shell_profile_block(&existing);
        if cleaned != existing || !existing.contains(&block) {
            let mut next = cleaned;
            if !next.is_empty() && !next.ends_with('\n') {
                next.push('\n');
            }
            next.push_str(&block);
            std::fs::write(&profile, next)
                .with_context(|| format!("failed to update shell profile {}", profile.display()))?;
            changed = true;
        }
    }
    Ok(if changed {
        PathUpdate::Added
    } else {
        PathUpdate::AlreadyPresent
    })
}

fn remove_shell_profile_prepend(dir: &Path) -> Result<PathUpdate> {
    let Some(home) = home_dir() else {
        return Ok(PathUpdate::Unsupported);
    };
    let mut changed = false;
    for profile in shell_profile_paths(&home) {
        if !profile.exists() {
            continue;
        }
        let existing = std::fs::read_to_string(&profile)
            .with_context(|| format!("failed to read shell profile {}", profile.display()))?;
        let cleaned = remove_shell_profile_block(&existing);
        let legacy = format!("export PATH=\"{}:$PATH\"", posix_shell_path(dir));
        let cleaned = cleaned.replace(&legacy, "");
        if cleaned != existing {
            std::fs::write(&profile, cleaned)
                .with_context(|| format!("failed to update shell profile {}", profile.display()))?;
            changed = true;
        }
    }
    Ok(if changed {
        PathUpdate::Added
    } else {
        PathUpdate::AlreadyPresent
    })
}

fn shell_profile_paths(home: &Path) -> Vec<PathBuf> {
    if cfg!(target_os = "macos") {
        vec![
            home.join(".zshrc"),
            home.join(".zprofile"),
            home.join(".bashrc"),
            home.join(".bash_profile"),
        ]
    } else {
        vec![
            home.join(".zshrc"),
            home.join(".bashrc"),
            home.join(".bash_profile"),
        ]
    }
}

fn remove_shell_profile_block(content: &str) -> String {
    let mut output = String::new();
    let mut skipping = false;
    for line in content.lines() {
        if line.trim() == SHELL_PROFILE_BEGIN {
            skipping = true;
            continue;
        }
        if skipping {
            if line.trim() == SHELL_PROFILE_END {
                skipping = false;
            }
            continue;
        }
        output.push_str(line);
        output.push('\n');
    }
    output
}

#[cfg(windows)]
fn user_path_contains_dir_impl(dir: &Path) -> Result<Option<bool>> {
    let current = read_user_path()?;
    Ok(Some(
        split_path_value(&current)
            .iter()
            .any(|entry| same_path(entry, dir)),
    ))
}

#[cfg(not(windows))]
fn user_path_contains_dir_impl(_dir: &Path) -> Result<Option<bool>> {
    Ok(None)
}

#[cfg(windows)]
fn read_user_path() -> Result<String> {
    let output = Command::new("reg")
        .args(["query", "HKCU\\Environment", "/v", "Path"])
        .output()
        .context("failed to query user PATH from registry")?;
    if !output.status.success() {
        return Ok(String::new());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let trimmed = line.trim_start();
        if !trimmed.to_ascii_lowercase().starts_with("path    reg_") {
            continue;
        }
        let parts = trimmed.split_whitespace().collect::<Vec<_>>();
        if parts.len() >= 3 {
            return Ok(parts[2..].join(" "));
        }
    }
    Ok(String::new())
}

#[cfg(windows)]
fn write_user_path(value: &str) -> Result<()> {
    let status = Command::new("reg")
        .args([
            "add",
            "HKCU\\Environment",
            "/v",
            "Path",
            "/t",
            "REG_EXPAND_SZ",
            "/d",
            value,
            "/f",
        ])
        .status()
        .context("failed to update user PATH in registry")?;
    if !status.success() {
        anyhow::bail!("failed to update user PATH in registry");
    }
    Ok(())
}

#[cfg(windows)]
fn split_path_value(value: &str) -> Vec<PathBuf> {
    value
        .split(';')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(PathBuf::from)
        .collect()
}

#[cfg(windows)]
fn join_path_value(entries: &[PathBuf]) -> Result<String> {
    Ok(env::join_paths(entries)?.to_string_lossy().to_string())
}

fn codex_exe_names() -> &'static [&'static str] {
    if cfg!(windows) {
        &["codex.exe", "codex.cmd", "codex.bat"]
    } else {
        &["codex"]
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    let a = a.canonicalize().unwrap_or_else(|_| a.to_path_buf());
    let b = b.canonicalize().unwrap_or_else(|_| b.to_path_buf());
    if cfg!(windows) {
        a.to_string_lossy()
            .eq_ignore_ascii_case(&b.to_string_lossy())
    } else {
        a == b
    }
}

fn env_truthy(name: &str) -> bool {
    env::var_os(name)
        .map(|value| {
            let value = value.to_string_lossy();
            value == "1" || value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("yes")
        })
        .unwrap_or(false)
}

fn absolutize(path: PathBuf) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(env::current_dir()?.join(path))
    }
}

#[allow(dead_code)]
fn os_string(value: &str) -> OsString {
    OsString::from(value)
}
