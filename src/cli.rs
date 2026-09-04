use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Version,
    Daemon,
    On,
    Off,
    Status,
    MigrateStorage {
        source: Option<PathBuf>,
    },
    InspectVsCodeRemoteControl,
    EnableVsCodeLegacyPatch,
    RestoreVsCodeLegacyPatch,
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
}

#[derive(Debug, Clone)]
pub struct Cli {
    pub config_path: Option<PathBuf>,
    pub command: Command,
}

impl Cli {
    pub fn parse() -> anyhow::Result<Self> {
        let args = std::env::args().skip(1).collect::<Vec<_>>();
        Self::parse_arguments(&args)
    }

    fn parse_arguments(args: &[String]) -> anyhow::Result<Self> {
        let mut config_path = None;
        let mut remaining = Vec::new();
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
            Some("-V") | Some("--version") | Some("version") => Command::Version,
            Some("daemon") | Some("run") => Command::Daemon,
            Some("on") => Command::On,
            Some("off") => Command::Off,
            Some("status") => Command::Status,
            Some("migrate-storage") => parse_migrate_storage(&remaining[1..])?,
            Some("vscode-remote-control") => parse_vscode_remote_control(&remaining[1..])?,
            Some("configure-codex-app") => parse_configure_codex_app(&remaining[1..])?,
            Some("uninstall-codex-app") => parse_uninstall_codex_app(&remaining[1..])?,
            Some("install-shim") | Some("uninstall-shim") | Some("shim") => anyhow::bail!(
                "CLI shim support has been removed. Use `mochiport configure-codex-app` and Codex App remote-control instead."
            ),
            Some("-h") | Some("--help") | Some("help") => {
                print_help();
                std::process::exit(0);
            }
            Some(other) => anyhow::bail!("unknown command `{other}`. Run `mochiport help`."),
        };

        Ok(Self {
            config_path,
            command,
        })
    }
}

fn parse_vscode_remote_control(args: &[String]) -> anyhow::Result<Command> {
    match args.first().map(String::as_str) {
        None | Some("status") => Ok(Command::InspectVsCodeRemoteControl),
        Some("patch-fallback") if args.len() == 1 => Ok(Command::EnableVsCodeLegacyPatch),
        Some("restore-fallback") if args.len() == 1 => Ok(Command::RestoreVsCodeLegacyPatch),
        Some(command) => anyhow::bail!(
            "unknown vscode-remote-control command `{command}`; use status, patch-fallback, or restore-fallback"
        ),
    }
}

fn parse_migrate_storage(args: &[String]) -> anyhow::Result<Command> {
    let mut source = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--from" => {
                let Some(path) = iter.next() else {
                    anyhow::bail!("--from requires a legacy storage directory");
                };
                if source.replace(PathBuf::from(path)).is_some() {
                    anyhow::bail!("--from may only be specified once");
                }
            }
            other => anyhow::bail!("unknown migrate-storage argument `{other}`"),
        }
    }
    Ok(Command::MigrateStorage { source })
}

fn default_command() -> Command {
    Command::Daemon
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

pub fn print_help() {
    println!(
        r#"mochiport

Usage:
  mochiport --version
  mochiport [--config PATH] daemon
  mochiport [--config PATH] on
  mochiport [--config PATH] off
  mochiport [--config PATH] status
  mochiport migrate-storage [--from LEGACY_STORAGE_PATH]
  mochiport vscode-remote-control [status|patch-fallback|restore-fallback]
  mochiport [--config PATH] configure-codex-app [--codex-home PATH] [--provider-name NAME] [--provider-base-url URL] [--provider-key TOKEN] [--model MODEL]
  mochiport [--config PATH] uninstall-codex-app [--codex-home PATH]

Default command is daemon.
"#
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_explicit_storage_migration_source() {
        let parsed = Cli::parse_arguments(&[
            "migrate-storage".to_string(),
            "--from".to_string(),
            "/fixture/ThreadRelay".to_string(),
        ])
        .expect("parse migrate-storage command");
        assert_eq!(
            parsed.command,
            Command::MigrateStorage {
                source: Some(PathBuf::from("/fixture/ThreadRelay")),
            }
        );
    }

    #[test]
    fn storage_migration_rejects_unknown_options() {
        let error = Cli::parse_arguments(&[
            "migrate-storage".to_string(),
            "--target".to_string(),
            "/fixture/MochiPort".to_string(),
        ])
        .expect_err("unknown migration option must fail")
        .to_string();
        assert!(error.contains("unknown migrate-storage argument"));
    }

    #[test]
    fn vscode_patch_requires_an_explicit_fallback_command() {
        let status = Cli::parse_arguments(&["vscode-remote-control".to_string()])
            .expect("parse status command");
        assert_eq!(status.command, Command::InspectVsCodeRemoteControl);

        let patch = Cli::parse_arguments(&[
            "vscode-remote-control".to_string(),
            "patch-fallback".to_string(),
        ])
        .expect("parse explicit patch command");
        assert_eq!(patch.command, Command::EnableVsCodeLegacyPatch);
    }
}
