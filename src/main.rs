// These lints describe stable API and callback shapes in the current
// cross-platform implementation. Keep them visible in review without
// requiring behavior-changing signature refactors just to satisfy Clippy.
#![allow(
    clippy::enum_variant_names,
    clippy::explicit_counter_loop,
    clippy::field_reassign_with_default,
    clippy::large_enum_variant,
    clippy::redundant_locals,
    clippy::result_large_err,
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::while_let_loop
)]

mod ai_gateway;
mod app_state;
mod bridge;
mod chain_log;
mod cli;
mod codex;
mod codex_app_config;
mod codex_app_enhanced;
mod config;
mod daemon_process;
mod im;
mod im_runtime;
mod manage_api;
mod outbound_http;
mod remote_control_backend;
mod storage_migration;
mod store;
mod types;
mod version;
mod vscode_extension_patch;
mod web;

use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result};
use axum::Router;
use serde_json::Value;
use tokio::{net::TcpListener, sync::watch};
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

use crate::{
    app_state::AppState,
    cli::{Cli, Command},
    config::AppConfig,
};

// Isolated fault tests set this explicitly; normal launch configurations never do.
const SKIP_DESKTOP_INTEGRATION_ENV: &str = "MOCHIPORT_SKIP_DESKTOP_INTEGRATION";

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse()?;
    if matches!(cli.command, Command::Version) {
        println!(
            "mochiport {} (build {})",
            version::PRODUCT_VERSION,
            version::BUILD_NUMBER
        );
        return Ok(());
    }
    if let Command::MigrateStorage { source } = &cli.command {
        if cli.config_path.is_some() {
            anyhow::bail!(
                "--config is not accepted by migrate-storage; set MOCHIPORT_HOME to choose the destination storage directory"
            );
        }
        let report = storage_migration::migrate_storage(source.clone())?;
        if report.migrated {
            println!(
                "MochiPort storage migrated atomically from {} to {}",
                report.source_directory.display(),
                report.destination_directory.display()
            );
            println!("Migration manifest: {}", report.manifest_path.display());
        } else {
            println!(
                "MochiPort storage was already migrated from {} to {}",
                report.source_directory.display(),
                report.destination_directory.display()
            );
            println!("Migration manifest: {}", report.manifest_path.display());
        }
        return Ok(());
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async_main(cli))
}

async fn async_main(cli: Cli) -> anyhow::Result<()> {
    let config_path = config_path_from_cli(cli.config_path.clone());
    let mut config = AppConfig::load_or_default(&config_path)?;
    let should_save_config = !config_path.exists() || config.apply_platform_defaults();
    config::normalize_config_paths(&mut config, &config_path);
    let log_path = init_logging(&config)?;
    tracing::info!(
        target: "mochiport::logging",
        path = %log_path.display(),
        "MochiPort chain log initialized"
    );
    if should_save_config {
        config.save(&config_path)?;
    }

    match cli.command {
        Command::Daemon => run_daemon(config_path, config).await,
        Command::On => set_bridge_enabled(&config_path, true).await,
        Command::Off => set_bridge_enabled(&config_path, false).await,
        Command::Status => print_status(&config).await,
        Command::InspectVsCodeRemoteControl => {
            print_vscode_remote_control_report(vscode_extension_patch::inspect_remote_control()?);
            Ok(())
        }
        Command::EnableVsCodeLegacyPatch => {
            print_vscode_remote_control_report(
                vscode_extension_patch::enable_legacy_patch_fallback()?,
            );
            Ok(())
        }
        Command::RestoreVsCodeLegacyPatch => {
            print_vscode_remote_control_report(
                vscode_extension_patch::restore_legacy_patch_fallback()?,
            );
            Ok(())
        }
        Command::ConfigureCodexApp {
            codex_home,
            provider_name,
            provider_base_url,
            provider_key,
            model: _,
        } => {
            let backend_url = config.remote_control_base_url();
            let report = codex_app_config::configure_codex_app(
                codex_app_config::ConfigureCodexAppOptions {
                    codex_home,
                    backend_url: backend_url.clone(),
                    connection_mode: config.local_connection_mode,
                    provider_name,
                    provider_base_url,
                    provider_key,
                    activate_provider: true,
                    provider_supports_websockets: None,
                },
            )?;
            println!("Codex App configured:");
            println!("  codex home: {}", report.codex_home.display());
            println!("  config: {}", report.config_path.display());
            println!("  auth: {}", report.auth_path.display());
            println!("  chatgpt_base_url: {}", report.backend_url);
            println!(
                "  remote_control switch: {}",
                if report.remote_control_switch.configured {
                    "enabled"
                } else {
                    "not enabled"
                }
            );
            Ok(())
        }
        Command::UninstallCodexApp { codex_home } => {
            let backend_url = config.remote_control_base_url();
            let report = codex_app_config::uninstall_codex_app(codex_home, &backend_url)?;
            println!("Codex App local remote-control config removed:");
            println!("  codex home: {}", report.codex_home.display());
            println!("  config: {}", report.config_path.display());
            println!("  auth: {}", report.auth_path.display());
            println!(
                "  removed chatgpt_base_url: {}",
                report.removed_chatgpt_base_url
            );
            println!(
                "  removed model_provider: {}",
                report.removed_model_provider
            );
            println!("  removed local auth: {}", report.removed_auth);
            println!(
                "  Codex App GUI backend: {}",
                report.gui_api_base.value.as_deref().unwrap_or("<unset>")
            );
            Ok(())
        }
        Command::MigrateStorage { .. } => {
            unreachable!("storage migration is handled before runtime creation")
        }
        Command::Version => unreachable!("version command is handled before runtime creation"),
    }
}

async fn run_daemon(config_path: PathBuf, config: AppConfig) -> anyhow::Result<()> {
    let daemon_identity = daemon_process::DaemonIdentity::new();
    let _daemon_lock = daemon_process::DaemonInstanceLock::acquire(&config_path, &daemon_identity)?;
    let desktop_integration_enabled =
        !environment_switch_enabled(std::env::var_os(SKIP_DESKTOP_INTEGRATION_ENV).as_deref());
    let bind = config.bind.clone();
    outbound_http::init(&config.outbound_proxy, config.local_listen_port())?;
    let chain_log_path = chain_log_path(&config);
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let (server_shutdown_tx, server_shutdown_rx) = watch::channel(false);
    let state = AppState::new(
        config_path,
        config,
        Some(shutdown_tx),
        Some(daemon_identity),
    );
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
    // Daemon startup must not mutate Codex-owned files. Explicit management
    // operations remain responsible for setup, repair, and legacy migration.
    if desktop_integration_enabled {
        let backend_url = state.config.lock().await.remote_control_base_url();
        tracing::info!(target: "mochiport::startup", "inspecting Codex App environment without mutation");
        let result = tokio::task::spawn_blocking(move || {
            codex_app_config::inspect_gui_api_base_url(&backend_url)
        })
        .await;

        match result {
            Ok(gui_api_base) => {
                tracing::info!(
                    target: "mochiport::startup",
                    configured = gui_api_base.configured,
                    "Codex App environment inspection finished"
                );
                state
                    .push_event(
                        "info",
                        "codex_app_direct_api_environment_checked",
                        format!(
                            "configured={} value={} error={}",
                            gui_api_base.configured,
                            gui_api_base.value.as_deref().unwrap_or_default(),
                            gui_api_base.error.as_deref().unwrap_or_default()
                        ),
                    )
                    .await;
            }
            Err(error) => {
                tracing::warn!(
                    target: "mochiport::startup",
                    error = %error,
                    "Codex App environment synchronization worker failed"
                );
                state
                    .push_event(
                        "warn",
                        "codex_app_direct_api_environment_check_failed",
                        format!("error={error}"),
                    )
                    .await;
            }
        }
    } else {
        state
            .push_event(
                "info",
                "desktop_integration_skipped",
                format!("requested_by={SKIP_DESKTOP_INTEGRATION_ENV}"),
            )
            .await;
    }

    let app = web::router(state.clone()).layer(TraceLayer::new_for_http());
    let addr: SocketAddr = bind
        .parse()
        .with_context(|| format!("invalid bind address `{bind}`"))?;
    tracing::info!(target: "mochiport::startup", addr = %addr, "binding local service");
    let listener = TcpListener::bind(addr).await?;
    let advertised_addr = if addr.ip().is_unspecified() {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), addr.port())
    } else {
        addr
    };
    let _active_daemon_locator = manage_api::publish_active_daemon_locator(
        &state.config_path,
        &state.daemon_identity,
        &format!("http://{advertised_addr}"),
    )?;
    println!("mochiport web: http://{addr}");
    tracing::info!(target: "mochiport::startup", addr = %addr, "local service listener ready");

    tokio::spawn(run_daemon_startup_tasks(
        state.clone(),
        desktop_integration_enabled,
    ));

    let companion = compatible_loopback_addr(addr);
    let mut companion_tasks = Vec::new();
    if let Some(companion_addr) = companion {
        match TcpListener::bind(companion_addr).await {
            Ok(companion_listener) => {
                println!("mochiport web: http://{companion_addr}");
                companion_tasks.push(tokio::spawn(serve_http(
                    companion_listener,
                    app.clone(),
                    server_shutdown_rx.clone(),
                )));
            }
            Err(err) => {
                tracing::warn!(
                    target: "mochiport::server",
                    addr = %companion_addr,
                    error = %err,
                    "compatible loopback listener unavailable"
                );
            }
        }
    }
    let shutdown_task_tx = server_shutdown_tx.clone();
    tokio::spawn(async move {
        let _ = shutdown_rx.await;
        let _ = shutdown_task_tx.send(true);
    });

    let primary_result = serve_http(listener, app, server_shutdown_rx).await;
    let _ = server_shutdown_tx.send(true);
    for task in companion_tasks {
        match task.await {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                tracing::warn!(
                    target: "mochiport::server",
                    error = %err,
                    "compatible loopback server stopped with error"
                );
            }
            Err(err) => {
                tracing::warn!(
                    target: "mochiport::server",
                    error = %err,
                    "compatible loopback server task failed"
                );
            }
        }
    }
    primary_result?;
    Ok(())
}

fn compatible_loopback_addr(addr: SocketAddr) -> Option<SocketAddr> {
    let port = addr.port();
    match addr.ip() {
        IpAddr::V4(ip) if ip == Ipv4Addr::LOCALHOST => {
            Some(SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), port))
        }
        IpAddr::V6(ip) if ip == Ipv6Addr::LOCALHOST => {
            Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port))
        }
        _ => None,
    }
}

async fn serve_http(
    listener: TcpListener,
    app: Router,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<()> {
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            if *shutdown_rx.borrow() {
                return;
            }
            let _ = shutdown_rx.changed().await;
        })
        .await?;
    Ok(())
}

async fn run_daemon_startup_tasks(
    state: crate::app_state::SharedState,
    _desktop_integration_enabled: bool,
) {
    if state.config.lock().await.bridge.enabled {
        web::start_bridge_if_ready(&state, "bridge start requested during daemon startup").await;
    } else {
        state
            .push_event("warn", "bridge_disabled", "bridge disabled by config")
            .await;
    }
}

fn environment_switch_enabled(value: Option<&std::ffi::OsStr>) -> bool {
    value.and_then(std::ffi::OsStr::to_str) == Some("1")
}

fn config_path_from_cli(path: Option<PathBuf>) -> PathBuf {
    path.map(absolutize)
        .unwrap_or_else(storage_migration::current_config_path)
}

fn init_logging(config: &AppConfig) -> anyhow::Result<PathBuf> {
    let path = chain_log_path(config);
    crate::chain_log::init(
        &path,
        effective_chain_log_diagnostic(config),
        config.logging.max_mb.saturating_mul(1024 * 1024),
        config.logging.retention_days,
    )?;

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("mochiport=info".parse()?))
        .with_ansi(false)
        .init();
    Ok(path)
}

fn effective_chain_log_diagnostic(config: &AppConfig) -> bool {
    config.logging.diagnostic
}

fn chain_log_path(config: &AppConfig) -> PathBuf {
    log_dir_from_config(config).join("mochiport-chain.log")
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
    let config = AppConfig::load_or_default(&config_path.to_path_buf())?;
    notify_daemon_bridge(config_path, &config, enabled).await?;
    println!(
        "MochiPort Feishu bridge {}",
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

async fn notify_daemon_bridge(
    config_path: &Path,
    config: &AppConfig,
    enabled: bool,
) -> anyhow::Result<()> {
    let action = if enabled { "start" } else { "stop" };
    let token = manage_api::management_token(config_path)
        .context("failed to load local management credential for bridge control")?;
    let url = format!("http://{}/api/v1/manage/bridge/{action}", config.bind);
    let response = local_daemon_http_client()?
        .post(url)
        .bearer_auth(token)
        .timeout(Duration::from_millis(700))
        .send()
        .await
        .context("bridge management request failed")?;
    if !response.status().is_success() {
        let status = response.status();
        let detail = response
            .text()
            .await
            .unwrap_or_default()
            .chars()
            .take(512)
            .collect::<String>();
        if detail.trim().is_empty() {
            anyhow::bail!("bridge management API returned {status}");
        }
        anyhow::bail!("bridge management API returned {status}: {detail}");
    }
    Ok(())
}

async fn query_daemon_backend_status(config: &AppConfig) -> anyhow::Result<Value> {
    let url = format!("http://{}/api/remote-control/backend-status", config.bind);
    let response = local_daemon_http_client()?
        .get(url)
        .timeout(Duration::from_millis(700))
        .send()
        .await?;
    if !response.status().is_success() {
        anyhow::bail!("daemon returned {}", response.status());
    }
    response.json::<Value>().await.map_err(Into::into)
}

fn local_daemon_http_client() -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .context("failed to build local daemon HTTP client")
}

fn print_vscode_remote_control_report(report: vscode_extension_patch::VsCodeExtensionPatchReport) {
    println!("VS Code remote-control: {}", report.action);
    println!("  {}", report.message);
    if let Some(path) = report.extension_js {
        println!("  extension: {}", path.display());
    }
    if let Some(path) = report.backup_path {
        println!("  backup: {}", path.display());
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compatible_loopback_addr_pairs_ipv4_and_ipv6_localhost() {
        let ipv4 = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 3847);
        let ipv6 = SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 3847);

        assert_eq!(compatible_loopback_addr(ipv4), Some(ipv6));
        assert_eq!(compatible_loopback_addr(ipv6), Some(ipv4));
    }

    #[test]
    fn compatible_loopback_addr_ignores_non_loopback() {
        let public_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 3847);

        assert_eq!(compatible_loopback_addr(public_addr), None);
    }

    #[test]
    fn desktop_integration_skip_requires_an_explicit_one_value() {
        assert!(environment_switch_enabled(Some(std::ffi::OsStr::new("1"))));
        assert!(!environment_switch_enabled(Some(std::ffi::OsStr::new(
            "true"
        ))));
        assert!(!environment_switch_enabled(Some(std::ffi::OsStr::new("0"))));
        assert!(!environment_switch_enabled(None));
    }
}
