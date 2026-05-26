use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Daemon,
    On,
    Off,
    Status,
    InstallShim {
        real_codex: Option<PathBuf>,
        bin_dir: Option<PathBuf>,
    },
    UninstallShim,
    ConfigureCodexApp {
        codex_home: Option<PathBuf>,
        provider_name: Option<String>,
        provider_base_url: Option<String>,
        provider_key: Option<String>,
        model: Option<String>,
    },
    Shim {
        args: Vec<String>,
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
            None => Command::Daemon,
            Some("daemon") | Some("run") => Command::Daemon,
            Some("on") => Command::On,
            Some("off") => Command::Off,
            Some("status") => Command::Status,
            Some("uninstall-shim") => Command::UninstallShim,
            Some("install-shim") => parse_install_shim(&remaining[1..])?,
            Some("configure-codex-app") => parse_configure_codex_app(&remaining[1..])?,
            Some("shim") => {
                let args = if remaining.get(1).map(String::as_str) == Some("--") {
                    remaining[2..].to_vec()
                } else {
                    remaining[1..].to_vec()
                };
                Command::Shim { args }
            }
            Some("-h") | Some("--help") | Some("help") => {
                print_help();
                std::process::exit(0);
            }
            Some(other) => anyhow::bail!("unknown command `{other}`. Run `codex-remote help`."),
        };

        Ok(Self {
            config_path,
            command,
        })
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

fn parse_install_shim(args: &[String]) -> anyhow::Result<Command> {
    let mut real_codex = None;
    let mut bin_dir = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--real-codex" => {
                let Some(value) = iter.next() else {
                    anyhow::bail!("--real-codex requires a path");
                };
                real_codex = Some(PathBuf::from(value));
            }
            "--bin-dir" => {
                let Some(value) = iter.next() else {
                    anyhow::bail!("--bin-dir requires a path");
                };
                bin_dir = Some(PathBuf::from(value));
            }
            other => anyhow::bail!("unknown install-shim argument `{other}`"),
        }
    }
    Ok(Command::InstallShim {
        real_codex,
        bin_dir,
    })
}

pub fn print_help() {
    println!(
        r#"codex-remote

Usage:
  codex-remote [--config PATH] daemon
  codex-remote [--config PATH] on
  codex-remote [--config PATH] off
  codex-remote [--config PATH] status
  codex-remote [--config PATH] install-shim [--real-codex PATH] [--bin-dir PATH]
  codex-remote [--config PATH] uninstall-shim
  codex-remote [--config PATH] configure-codex-app [--codex-home PATH] [--provider-name NAME] [--provider-base-url URL] [--provider-key TOKEN] [--model MODEL]
  codex-remote [--config PATH] shim -- [codex args...]

Default command is daemon.
"#
    );
}
