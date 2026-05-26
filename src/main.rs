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
mod shim;
mod store;
mod types;
mod web;

use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};

use anyhow::Context;
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
        Command::On => shim::set_enabled(&config_path, true).await,
        Command::Off => shim::set_enabled(&config_path, false).await,
        Command::Status => shim::print_status(&config).await,
        Command::InstallShim {
            real_codex,
            bin_dir,
        } => {
            shim::install_shim(&mut config, &config_path, real_codex, bin_dir)?;
            Ok(())
        }
        Command::UninstallShim => {
            shim::uninstall_shim(&config)?;
            Ok(())
        }
        Command::ConfigureCodexApp {
            codex_home,
            provider_name,
            provider_base_url,
            provider_key,
            model,
        } => {
            let backend_url = shim::remote_control_base_url(&config);
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
        Command::Shim { args } => {
            let code = shim::run_shim(&config, args).await?;
            std::process::exit(code);
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
    if config.shim.bin_dir.is_relative() {
        config.shim.bin_dir = base.join(&config.shim.bin_dir);
    }
    if let Some(real_codex_path) = config.shim.real_codex_path.as_mut()
        && real_codex_path.is_relative()
    {
        *real_codex_path = base.join(&real_codex_path);
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
        .shim
        .bin_dir
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| config.shim.bin_dir.clone())
        .join("logs")
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
