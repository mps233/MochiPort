mod app_state;
mod bridge;
mod chain_log;
mod cli;
mod codex;
mod codex_app_config;
mod config;
mod im;
mod im_runtime;
mod remote_control_backend;
mod store;
mod types;
mod web;

use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::Context;
use serde_json::Value;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

use crate::{
    app_state::AppState,
    cli::{Cli, Command},
    config::AppConfig,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse()?;
    let config_path = config_path_from_cli(cli.config_path.clone());
    let mut config = AppConfig::load_or_default(&config_path)?;
    normalize_config_paths(&mut config, &config_path);
    let log_path = init_logging(&config)?;
    tracing::info!(
        target: "codex_remote::logging",
        path = %log_path.display(),
        "codex-remote chain log initialized"
    );
    if !config_path.exists() {
        config.save(&config_path)?;
    }

    match cli.command {
        Command::Daemon => run_daemon(config_path, config).await,
        Command::On => set_bridge_enabled(&config_path, true).await,
        Command::Off => set_bridge_enabled(&config_path, false).await,
        Command::Status => print_status(&config).await,
        Command::ConfigureCodexApp {
            codex_home,
            provider_name,
            provider_base_url,
            provider_key,
            model,
        } => {
            let backend_url = config.remote_control_base_url();
            let report = codex_app_config::configure_codex_app(
                codex_app_config::ConfigureCodexAppOptions {
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
                },
            )?;
            let gui_api_base = codex_app_config::configure_gui_api_base_url(&backend_url)?;
            println!("Codex App configured:");
            println!("  codex home: {}", report.codex_home.display());
            println!("  config: {}", report.config_path.display());
            println!("  auth: {}", report.auth_path.display());
            println!("  chatgpt_base_url: {}", report.backend_url);
            println!(
                "  CODEX_API_BASE_URL for GUI launch: {}",
                gui_api_base.value.as_deref().unwrap_or("<unset>")
            );
            Ok(())
        }
    }
}

async fn run_daemon(config_path: PathBuf, config: AppConfig) -> anyhow::Result<()> {
    let bind = config.bind.clone();
    let chain_log_path = chain_log_path(&config);
    let state = AppState::new(config_path, config);
    {
        let config = state.config.lock().await;
        state
            .push_event(
                "info",
                "config_loaded",
                format!(
                    "config={} state={}",
                    state.config_path.display(),
                    config.state_path.display()
                ),
            )
            .await;
    }
    state
        .push_event(
            "info",
            "chain_log_ready",
            format!("path={}", chain_log_path.display()),
        )
        .await;
    if state.config.lock().await.bridge.enabled {
        let bridge_state = state.clone();
        let bridge_handle = tokio::spawn(async move {
            bridge::start_bridge(bridge_state).await;
        });
        *state.bridge_task.lock().await = Some(bridge_handle);
    } else {
        state
            .push_event("warn", "bridge_disabled", "bridge disabled by config")
            .await;
    }
    let app = web::router(state).layer(TraceLayer::new_for_http());
    let addr: SocketAddr = bind
        .parse()
        .with_context(|| format!("invalid bind address `{bind}`"))?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("codex-remote web: http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

fn config_path_from_cli(path: Option<PathBuf>) -> PathBuf {
    if let Some(path) = path {
        return absolutize(path);
    }

    inferred_repo_config_from_target_exe()
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .map(|cwd| cwd.join("config.toml"))
                .filter(|path| path.exists())
        })
        .unwrap_or_else(|| absolutize(PathBuf::from("config.toml")))
}

fn inferred_repo_config_from_target_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let profile_dir = exe.parent()?;
    let target_dir = profile_dir.parent()?;
    if target_dir.file_name().and_then(|value| value.to_str()) != Some("target") {
        return None;
    }
    let profile = profile_dir.file_name().and_then(|value| value.to_str())?;
    if profile != "debug" && profile != "release" {
        return None;
    }
    let config = target_dir.parent()?.join("config.toml");
    config.exists().then_some(config)
}

fn normalize_config_paths(config: &mut AppConfig, config_path: &Path) {
    let base = config_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    if config.state_path.is_relative() {
        config.state_path = base.join(&config.state_path);
    }
}

fn init_logging(config: &AppConfig) -> anyhow::Result<PathBuf> {
    let path = chain_log_path(config);
    crate::chain_log::init(&path)?;

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("codex_remote=info".parse()?))
        .with_ansi(false)
        .init();
    Ok(path)
}

fn chain_log_path(config: &AppConfig) -> PathBuf {
    log_dir_from_config(config).join("codex-remote-chain.log")
}

fn log_dir_from_config(config: &AppConfig) -> PathBuf {
    config
        .state_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("logs")
}

async fn set_bridge_enabled(config_path: &Path, enabled: bool) -> anyhow::Result<()> {
    let mut config = AppConfig::load_or_default(&config_path.to_path_buf())?;
    config.bridge.enabled = enabled;
    config.save(&config_path.to_path_buf())?;
    let _ = notify_daemon_bridge(&config, enabled).await;
    println!(
        "codex-remote Feishu bridge {}",
        if enabled { "enabled" } else { "disabled" }
    );
    Ok(())
}

async fn print_status(config: &AppConfig) -> anyhow::Result<()> {
    println!(
        "Feishu bridge: {}",
        if config.bridge.enabled {
            "enabled"
        } else {
            "disabled"
        }
    );
    println!(
        "remote-control backend: {}",
        config.remote_control_base_url()
    );
    let status = query_daemon_backend_status(config).await;
    match status {
        Ok(status) => {
            let reason = status
                .get("reason")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .or_else(|| {
                    status
                        .get("remoteControlBaseUrl")
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty())
                })
                .unwrap_or("ok");
            let available = status
                .get("available")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            println!(
                "daemon: {} ({reason})",
                if available {
                    "available"
                } else {
                    "unavailable"
                }
            );
        }
        Err(err) => println!("daemon: unavailable ({err})"),
    }
    Ok(())
}

async fn notify_daemon_bridge(config: &AppConfig, enabled: bool) -> anyhow::Result<()> {
    let action = if enabled { "start" } else { "stop" };
    let url = format!("http://{}/api/bridge/{action}", config.bind);
    reqwest::Client::new()
        .post(url)
        .timeout(Duration::from_millis(700))
        .send()
        .await?;
    Ok(())
}

async fn query_daemon_backend_status(config: &AppConfig) -> anyhow::Result<Value> {
    let url = format!("http://{}/api/remote-control/backend-status", config.bind);
    let response = reqwest::Client::new()
        .get(url)
        .timeout(Duration::from_millis(700))
        .send()
        .await?;
    if !response.status().is_success() {
        anyhow::bail!("daemon returned {}", response.status());
    }
    response.json::<Value>().await.map_err(Into::into)
}

fn absolutize(path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(&path))
            .unwrap_or_else(|_| path)
    }
}
