use std::{
    env,
    net::IpAddr,
    sync::{OnceLock, RwLock},
    time::Duration,
};

use anyhow::{Context, Result, anyhow};
use reqwest::Client;

use crate::config::{OutboundProxyConfig, OutboundProxyMode};

static GLOBAL_CLIENT: OnceLock<RwLock<Client>> = OnceLock::new();

pub fn init(config: &OutboundProxyConfig, local_port: Option<u16>) -> Result<()> {
    let client = build_client(config, local_port)?;
    if let Some(lock) = GLOBAL_CLIENT.get() {
        *lock
            .write()
            .map_err(|_| anyhow!("outbound HTTP client lock is poisoned"))? = client;
    } else {
        let _ = GLOBAL_CLIENT.set(RwLock::new(client));
    }

    tracing::info!(
        target: "codexhub::network",
        mode = ?config.mode,
        proxy = %masked_proxy_url(config),
        "outbound HTTP client initialized"
    );
    Ok(())
}

pub fn get() -> Client {
    GLOBAL_CLIENT
        .get()
        .and_then(|lock| lock.read().ok())
        .map(|client| client.clone())
        .unwrap_or_else(Client::new)
}

#[cfg(test)]
pub fn validate(config: &OutboundProxyConfig) -> Result<()> {
    build_client(config, None).map(|_| ())
}

pub fn validate_for_local_port(
    config: &OutboundProxyConfig,
    local_port: Option<u16>,
) -> Result<()> {
    build_client(config, local_port).map(|_| ())
}

pub fn build_client(config: &OutboundProxyConfig, local_port: Option<u16>) -> Result<Client> {
    apply_async_proxy(
        Client::builder()
            .pool_max_idle_per_host(10)
            .tcp_keepalive(Duration::from_secs(60))
            .connect_timeout(Duration::from_secs(30)),
        config,
        local_port,
    )?
    .build()
    .context("failed to build outbound HTTP client")
}

pub fn apply_async_proxy(
    builder: reqwest::ClientBuilder,
    config: &OutboundProxyConfig,
    local_port: Option<u16>,
) -> Result<reqwest::ClientBuilder> {
    match config.mode {
        OutboundProxyMode::System => {
            if local_port.is_some_and(system_proxy_points_to_local_server) {
                tracing::warn!(
                    target: "codexhub::network",
                    local_port,
                    "system proxy points to CodexHub; disabling it to avoid proxy recursion"
                );
                Ok(builder.no_proxy())
            } else {
                Ok(builder)
            }
        }
        OutboundProxyMode::Direct => Ok(builder.no_proxy()),
        OutboundProxyMode::Custom => {
            reject_local_proxy_recursion(config, local_port)?;
            let proxy = custom_proxy(config)?;
            Ok(builder.proxy(proxy))
        }
    }
}

#[cfg(feature = "gui")]
pub fn apply_blocking_proxy(
    builder: reqwest::blocking::ClientBuilder,
    config: &OutboundProxyConfig,
    local_port: Option<u16>,
) -> Result<reqwest::blocking::ClientBuilder> {
    match config.mode {
        OutboundProxyMode::System => {
            if local_port.is_some_and(system_proxy_points_to_local_server) {
                Ok(builder.no_proxy())
            } else {
                Ok(builder)
            }
        }
        OutboundProxyMode::Direct => Ok(builder.no_proxy()),
        OutboundProxyMode::Custom => {
            reject_local_proxy_recursion(config, local_port)?;
            let proxy = custom_proxy(config)?;
            Ok(builder.proxy(proxy))
        }
    }
}

fn custom_proxy(config: &OutboundProxyConfig) -> Result<reqwest::Proxy> {
    let value = config.url.trim();
    if value.is_empty() {
        return Err(anyhow!("custom outbound proxy URL is empty"));
    }
    let parsed = url::Url::parse(value).context("invalid custom outbound proxy URL")?;
    if !matches!(parsed.scheme(), "http" | "https" | "socks5" | "socks5h") {
        return Err(anyhow!(
            "unsupported outbound proxy scheme `{}`; use http, https, socks5, or socks5h",
            parsed.scheme()
        ));
    }
    reqwest::Proxy::all(value).context("invalid custom outbound proxy URL")
}

fn reject_local_proxy_recursion(
    config: &OutboundProxyConfig,
    local_port: Option<u16>,
) -> Result<()> {
    if local_port.is_some_and(|port| proxy_points_to_loopback_port(config.url.trim(), port)) {
        return Err(anyhow!(
            "custom outbound proxy points to CodexHub's own local port"
        ));
    }
    Ok(())
}

fn system_proxy_points_to_local_server(local_port: u16) -> bool {
    const KEYS: [&str; 6] = [
        "HTTP_PROXY",
        "http_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
    ];

    KEYS.iter()
        .filter_map(|key| env::var(key).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .any(|value| proxy_points_to_loopback_port(&value, local_port))
}

fn proxy_points_to_loopback_port(value: &str, local_port: u16) -> bool {
    let value = value.trim();
    if let Some(bracketed) = value.strip_prefix('[')
        && let Some((host, port)) = bracketed.split_once("]:")
        && port.parse::<u16>().ok() == Some(local_port)
    {
        return host
            .parse::<IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false);
    }
    let parsed = url::Url::parse(value)
        .ok()
        .filter(|url| url.host_str().is_some())
        .or_else(|| url::Url::parse(&format!("http://{value}")).ok());
    parsed.is_some_and(|url| {
        url.port() == Some(local_port)
            && url.host_str().is_some_and(|host| {
                host.eq_ignore_ascii_case("localhost")
                    || host
                        .parse::<IpAddr>()
                        .map(|ip| ip.is_loopback())
                        .unwrap_or(false)
            })
    })
}

pub fn masked_proxy_url(config: &OutboundProxyConfig) -> String {
    if config.mode != OutboundProxyMode::Custom {
        return "<none>".to_string();
    }
    let value = config.url.trim();
    let Ok(parsed) = url::Url::parse(value) else {
        return "<invalid>".to_string();
    };
    let host = parsed.host_str().unwrap_or("?");
    match parsed.port() {
        Some(port) => format!("{}://{host}:{port}", parsed.scheme()),
        None => format!("{}://{host}", parsed.scheme()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_supported_proxy_modes() {
        validate(&OutboundProxyConfig::default()).unwrap();
        validate(&OutboundProxyConfig {
            mode: OutboundProxyMode::Direct,
            url: String::new(),
        })
        .unwrap();
        validate(&OutboundProxyConfig {
            mode: OutboundProxyMode::Custom,
            url: "socks5://127.0.0.1:1080".to_string(),
        })
        .unwrap();
    }

    #[test]
    fn rejects_empty_or_unsupported_custom_proxy() {
        assert!(
            validate(&OutboundProxyConfig {
                mode: OutboundProxyMode::Custom,
                url: String::new(),
            })
            .is_err()
        );
        assert!(
            validate(&OutboundProxyConfig {
                mode: OutboundProxyMode::Custom,
                url: "ftp://127.0.0.1:2121".to_string(),
            })
            .is_err()
        );
    }

    #[test]
    fn rejects_custom_proxy_pointing_to_codexhub() {
        assert!(
            validate_for_local_port(
                &OutboundProxyConfig {
                    mode: OutboundProxyMode::Custom,
                    url: "http://127.0.0.1:3847".to_string(),
                },
                Some(3847),
            )
            .is_err()
        );
    }

    #[test]
    fn loopback_detection_requires_the_codexhub_port() {
        assert!(proxy_points_to_loopback_port("http://127.0.0.1:3847", 3847));
        assert!(proxy_points_to_loopback_port(
            "socks5://localhost:3847",
            3847
        ));
        assert!(proxy_points_to_loopback_port("[::1]:3847", 3847));
        assert!(!proxy_points_to_loopback_port(
            "http://127.0.0.1:7890",
            3847
        ));
        assert!(!proxy_points_to_loopback_port(
            "http://192.168.1.10:3847",
            3847
        ));
    }

    #[test]
    fn masked_url_removes_credentials() {
        assert_eq!(
            masked_proxy_url(&OutboundProxyConfig {
                mode: OutboundProxyMode::Custom,
                url: "http://user:secret@127.0.0.1:7890".to_string(),
            }),
            "http://127.0.0.1:7890"
        );
    }
}
