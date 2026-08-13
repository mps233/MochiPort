use std::{net::SocketAddr, path::PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SafeRelaunchShutdownMode {
    Guarded,
    Signal,
}

impl SafeRelaunchShutdownMode {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Guarded => "guarded",
            Self::Signal => "signal",
        }
    }

    fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "guarded" => Ok(Self::Guarded),
            "signal" => Ok(Self::Signal),
            _ => anyhow::bail!("--shutdown-mode requires `guarded` or `signal`"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Gui,
    Daemon,
    On,
    Off,
    Status,
    ConfigureCodexApp {
        codex_home: Option<PathBuf>,
        provider_name: Option<String>,
        provider_base_url: Option<String>,
        provider_key: Option<String>,
        model: Option<String>,
    },
    UninstallCodexApp {
        codex_home: Option<PathBuf>,
    },
    SafeRelaunchHelper {
        bundle_path: PathBuf,
        expected_bundle_identifier: String,
        expected_version: String,
        expected_build: String,
        daemon_pid: u32,
        daemon_instance_id: String,
        old_executable_path: PathBuf,
        gui_pid: Option<u32>,
        bind_addr: SocketAddr,
        log_path: PathBuf,
        config_path: PathBuf,
        start_delay_ms: u64,
        shutdown_mode: SafeRelaunchShutdownMode,
    },
}

#[derive(Debug, Clone)]
pub struct Cli {
    pub config_path: Option<PathBuf>,
    pub command: Command,
}

impl Cli {
    pub fn parse() -> anyhow::Result<Self> {
        let mut config_path = None;
        let mut remaining = Vec::new();
        let args = std::env::args().skip(1).collect::<Vec<_>>();
        let mut index = 0;

        while index < args.len() {
            match args[index].as_str() {
                "--config" | "-c" => {
                    let Some(path) = args.get(index + 1) else {
                        anyhow::bail!("{} requires a path", args[index]);
                    };
                    config_path = Some(PathBuf::from(path));
                    index += 2;
                }
                _ => {
                    remaining.extend_from_slice(&args[index..]);
                    break;
                }
            }
        }

        let command = match remaining.first().map(String::as_str) {
            None => default_command(),
            Some("gui") => Command::Gui,
            Some("daemon") | Some("run") => Command::Daemon,
            Some("on") => Command::On,
            Some("off") => Command::Off,
            Some("status") => Command::Status,
            Some("configure-codex-app") => parse_configure_codex_app(&remaining[1..])?,
            Some("uninstall-codex-app") => parse_uninstall_codex_app(&remaining[1..])?,
            Some("safe-relaunch-helper") => parse_safe_relaunch_helper(&remaining[1..])?,
            Some("install-shim") | Some("uninstall-shim") | Some("shim") => anyhow::bail!(
                "CLI shim support has been removed. Use `threadrelay configure-codex-app` and Codex App remote-control instead."
            ),
            Some("-h") | Some("--help") | Some("help") => {
                print_help();
                std::process::exit(0);
            }
            Some(other) => anyhow::bail!("unknown command `{other}`. Run `threadrelay help`."),
        };

        Ok(Self {
            config_path,
            command,
        })
    }
}

fn default_command() -> Command {
    if cfg!(feature = "gui") {
        Command::Gui
    } else {
        Command::Daemon
    }
}

fn parse_configure_codex_app(args: &[String]) -> anyhow::Result<Command> {
    let mut codex_home = None;
    let mut provider_name = None;
    let mut provider_base_url = None;
    let mut provider_key = None;
    let mut model = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--codex-home" => {
                let Some(value) = iter.next() else {
                    anyhow::bail!("--codex-home requires a path");
                };
                codex_home = Some(PathBuf::from(value));
            }
            "--provider-name" => {
                let Some(value) = iter.next() else {
                    anyhow::bail!("--provider-name requires a name");
                };
                provider_name = Some(value.to_string());
            }
            "--provider-base-url" => {
                let Some(value) = iter.next() else {
                    anyhow::bail!("--provider-base-url requires a URL");
                };
                provider_base_url = Some(value.to_string());
            }
            "--provider-key" => {
                let Some(value) = iter.next() else {
                    anyhow::bail!("--provider-key requires a token");
                };
                provider_key = Some(value.to_string());
            }
            "--model" => {
                let Some(value) = iter.next() else {
                    anyhow::bail!("--model requires a model name");
                };
                model = Some(value.to_string());
            }
            other => anyhow::bail!("unknown configure-codex-app argument `{other}`"),
        }
    }
    Ok(Command::ConfigureCodexApp {
        codex_home,
        provider_name,
        provider_base_url,
        provider_key,
        model,
    })
}

fn parse_uninstall_codex_app(args: &[String]) -> anyhow::Result<Command> {
    let mut codex_home = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--codex-home" => {
                let Some(value) = iter.next() else {
                    anyhow::bail!("--codex-home requires a path");
                };
                codex_home = Some(PathBuf::from(value));
            }
            other => anyhow::bail!("unknown uninstall-codex-app argument `{other}`"),
        }
    }
    Ok(Command::UninstallCodexApp { codex_home })
}

fn parse_safe_relaunch_helper(args: &[String]) -> anyhow::Result<Command> {
    let mut bundle_path = None;
    let mut expected_bundle_identifier = None;
    let mut expected_version = None;
    let mut expected_build = None;
    let mut daemon_pid = None;
    let mut daemon_instance_id = None;
    let mut old_executable_path = None;
    let mut gui_pid = None;
    let mut bind_addr = None;
    let mut log_path = None;
    let mut config_path = None;
    let mut start_delay_ms = None;
    let mut shutdown_mode = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let value = match arg.as_str() {
            "--bundle-path"
            | "--expected-bundle-identifier"
            | "--expected-version"
            | "--expected-build"
            | "--daemon-pid"
            | "--daemon-instance-id"
            | "--old-executable-path"
            | "--gui-pid"
            | "--bind-address"
            | "--log-path"
            | "--config-path"
            | "--start-delay-ms"
            | "--shutdown-mode" => iter
                .next()
                .ok_or_else(|| anyhow::anyhow!("{arg} requires a value"))?,
            other => anyhow::bail!("unknown safe-relaunch-helper argument `{other}`"),
        };
        match arg.as_str() {
            "--bundle-path" => bundle_path = Some(PathBuf::from(value)),
            "--expected-bundle-identifier" => expected_bundle_identifier = Some(value.to_string()),
            "--expected-version" => expected_version = Some(value.to_string()),
            "--expected-build" => expected_build = Some(value.to_string()),
            "--daemon-pid" => daemon_pid = Some(parse_positive_u32(arg, value)?),
            "--daemon-instance-id" => daemon_instance_id = Some(value.to_string()),
            "--old-executable-path" => old_executable_path = Some(PathBuf::from(value)),
            "--gui-pid" => gui_pid = Some(parse_positive_u32(arg, value)?),
            "--bind-address" => {
                let parsed = value.parse::<SocketAddr>().map_err(|_| {
                    anyhow::anyhow!("--bind-address requires an IP address and TCP port")
                })?;
                if parsed.port() == 0 || !parsed.ip().is_loopback() {
                    anyhow::bail!("--bind-address requires a loopback address and TCP port");
                }
                bind_addr = Some(parsed);
            }
            "--log-path" => log_path = Some(PathBuf::from(value)),
            "--config-path" => config_path = Some(PathBuf::from(value)),
            "--start-delay-ms" => {
                let parsed = value
                    .parse::<u64>()
                    .map_err(|_| anyhow::anyhow!("--start-delay-ms requires milliseconds"))?;
                if parsed > 120_000 {
                    anyhow::bail!("--start-delay-ms may not exceed 120000");
                }
                start_delay_ms = Some(parsed);
            }
            "--shutdown-mode" => {
                shutdown_mode = Some(SafeRelaunchShutdownMode::parse(value)?);
            }
            _ => unreachable!("safe relaunch argument validated above"),
        }
    }

    let required_string = |value: Option<String>, name: &str| -> anyhow::Result<String> {
        value
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("{name} is required"))
    };
    let shutdown_mode =
        shutdown_mode.ok_or_else(|| anyhow::anyhow!("--shutdown-mode is required"))?;
    if shutdown_mode == SafeRelaunchShutdownMode::Signal && gui_pid.is_none() {
        anyhow::bail!("--gui-pid is required when --shutdown-mode is signal");
    }

    Ok(Command::SafeRelaunchHelper {
        bundle_path: bundle_path.ok_or_else(|| anyhow::anyhow!("--bundle-path is required"))?,
        expected_bundle_identifier: required_string(
            expected_bundle_identifier,
            "--expected-bundle-identifier",
        )?,
        expected_version: required_string(expected_version, "--expected-version")?,
        expected_build: required_string(expected_build, "--expected-build")?,
        daemon_pid: daemon_pid.ok_or_else(|| anyhow::anyhow!("--daemon-pid is required"))?,
        daemon_instance_id: required_string(daemon_instance_id, "--daemon-instance-id")?,
        old_executable_path: old_executable_path
            .ok_or_else(|| anyhow::anyhow!("--old-executable-path is required"))?,
        gui_pid,
        bind_addr: bind_addr.ok_or_else(|| anyhow::anyhow!("--bind-address is required"))?,
        log_path: log_path.ok_or_else(|| anyhow::anyhow!("--log-path is required"))?,
        config_path: config_path.ok_or_else(|| anyhow::anyhow!("--config-path is required"))?,
        start_delay_ms: start_delay_ms
            .ok_or_else(|| anyhow::anyhow!("--start-delay-ms is required"))?,
        shutdown_mode,
    })
}

fn parse_positive_u32(name: &str, value: &str) -> anyhow::Result<u32> {
    let parsed = value
        .parse::<u32>()
        .map_err(|_| anyhow::anyhow!("{name} requires a positive integer"))?;
    if parsed == 0 {
        anyhow::bail!("{name} requires a positive integer");
    }
    Ok(parsed)
}

pub fn print_help() {
    println!(
        r#"threadrelay

Usage:
  threadrelay [--config PATH] gui
  threadrelay [--config PATH] daemon
  threadrelay [--config PATH] on
  threadrelay [--config PATH] off
  threadrelay [--config PATH] status
  threadrelay [--config PATH] configure-codex-app [--codex-home PATH] [--provider-name NAME] [--provider-base-url URL] [--provider-key TOKEN] [--model MODEL]
  threadrelay [--config PATH] uninstall-codex-app [--codex-home PATH]

Default command is gui when built with the gui feature, otherwise daemon.
"#
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn safe_relaunch_helper_args(shutdown_mode: Option<&str>) -> Vec<String> {
        let mut args = vec![
            "--bundle-path",
            "/tmp/ThreadRelay build 330.app",
            "--expected-bundle-identifier",
            "io.github.mps233.threadrelay",
            "--expected-version",
            "0.4.20",
            "--expected-build",
            "330",
            "--daemon-pid",
            "200",
            "--daemon-instance-id",
            "instance-a",
            "--old-executable-path",
            "/tmp/ThreadRelay old.app/Contents/MacOS/ThreadRelay",
            "--gui-pid",
            "100",
            "--bind-address",
            "127.0.0.1:3847",
            "--log-path",
            "/tmp/ThreadRelay logs/relaunch.log",
            "--config-path",
            "/tmp/ThreadRelay config/config.toml",
            "--start-delay-ms",
            "350",
        ]
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
        if let Some(shutdown_mode) = shutdown_mode {
            args.push("--shutdown-mode".to_string());
            args.push(shutdown_mode.to_string());
        }
        args
    }

    #[test]
    fn parses_safe_relaunch_helper_paths_as_single_arguments() {
        let args = safe_relaunch_helper_args(Some("guarded"));

        let parsed = parse_safe_relaunch_helper(&args).expect("parse helper command");
        let Command::SafeRelaunchHelper {
            bundle_path,
            old_executable_path,
            gui_pid,
            bind_addr,
            log_path,
            config_path,
            start_delay_ms,
            shutdown_mode,
            ..
        } = parsed
        else {
            panic!("expected safe relaunch helper")
        };
        assert_eq!(bundle_path, PathBuf::from("/tmp/ThreadRelay build 330.app"));
        assert_eq!(
            old_executable_path,
            PathBuf::from("/tmp/ThreadRelay old.app/Contents/MacOS/ThreadRelay")
        );
        assert_eq!(gui_pid, Some(100));
        assert_eq!(bind_addr, "127.0.0.1:3847".parse().expect("bind address"));
        assert_eq!(
            log_path,
            PathBuf::from("/tmp/ThreadRelay logs/relaunch.log")
        );
        assert_eq!(
            config_path,
            PathBuf::from("/tmp/ThreadRelay config/config.toml")
        );
        assert_eq!(start_delay_ms, 350);
        assert_eq!(shutdown_mode, SafeRelaunchShutdownMode::Guarded);
    }

    #[test]
    fn parses_signal_shutdown_mode() {
        let parsed = parse_safe_relaunch_helper(&safe_relaunch_helper_args(Some("signal")))
            .expect("parse signal shutdown mode");
        let Command::SafeRelaunchHelper { shutdown_mode, .. } = parsed else {
            panic!("expected safe relaunch helper")
        };
        assert_eq!(shutdown_mode, SafeRelaunchShutdownMode::Signal);
    }

    #[test]
    fn requires_valid_safe_relaunch_shutdown_mode() {
        let missing = parse_safe_relaunch_helper(&safe_relaunch_helper_args(None))
            .expect_err("missing shutdown mode must fail")
            .to_string();
        assert!(missing.contains("--shutdown-mode is required"));

        let invalid = parse_safe_relaunch_helper(&safe_relaunch_helper_args(Some("force")))
            .expect_err("invalid shutdown mode must fail")
            .to_string();
        assert!(invalid.contains("`guarded` or `signal`"));
    }

    #[test]
    fn signal_shutdown_mode_requires_gui_pid() {
        let mut args = safe_relaunch_helper_args(Some("signal"));
        let gui_pid_index = args
            .iter()
            .position(|arg| arg == "--gui-pid")
            .expect("gui pid argument");
        args.drain(gui_pid_index..=gui_pid_index + 1);

        let error = parse_safe_relaunch_helper(&args)
            .expect_err("signal mode without gui pid must fail")
            .to_string();
        assert!(error.contains("--gui-pid is required"));
    }

    #[test]
    fn rejects_zero_process_ids_for_safe_relaunch_helper() {
        let error = parse_positive_u32("--daemon-pid", "0")
            .expect_err("zero pid must be rejected")
            .to_string();
        assert!(error.contains("positive integer"));
    }
}
