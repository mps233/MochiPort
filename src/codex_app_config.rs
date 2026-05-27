use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow};
use base64::Engine;
use serde::Serialize;
use serde_json::json;

const DEFAULT_PROVIDER_NAME: &str = "codex";
const DEFAULT_MODEL: &str = "gpt-5.5";
const DEFAULT_REASONING_EFFORT: &str = "xhigh";

#[derive(Debug, Clone)]
pub struct ConfigureCodexAppOptions {
    pub codex_home: Option<PathBuf>,
    pub backend_url: String,
    pub account_id: String,
    pub user_id: String,
    pub email: String,
    pub plan_type: String,
    pub provider_name: Option<String>,
    pub provider_base_url: Option<String>,
    pub provider_key: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ConfigureCodexAppReport {
    pub codex_home: PathBuf,
    pub config_path: PathBuf,
    pub auth_path: PathBuf,
    pub backend_url: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UninstallCodexAppReport {
    pub codex_home: PathBuf,
    pub config_path: PathBuf,
    pub auth_path: PathBuf,
    pub removed_chatgpt_base_url: bool,
    pub removed_model_provider: bool,
    pub removed_auth: bool,
    pub gui_api_base: CodexAppGuiApiBaseStatus,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexAppGuiApiBaseStatus {
    pub supported: bool,
    pub configured: bool,
    pub expected: String,
    pub value: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexAppConfigStatus {
    pub codex_home: PathBuf,
    pub config_path: PathBuf,
    pub auth_path: PathBuf,
    pub configured: bool,
    pub config_ok: bool,
    pub auth_ok: bool,
    pub config_error: Option<String>,
    pub auth_error: Option<String>,
    pub gui_api_base: CodexAppGuiApiBaseStatus,
}

pub fn configure_codex_app(options: ConfigureCodexAppOptions) -> Result<ConfigureCodexAppReport> {
    let codex_home = options
        .codex_home
        .clone()
        .unwrap_or_else(default_codex_home);
    std::fs::create_dir_all(&codex_home)
        .with_context(|| format!("failed to create Codex home {}", codex_home.display()))?;

    let config_path = codex_home.join("config.toml");
    write_config_toml(&config_path, &options)?;

    let auth_path = codex_home.join("auth.json");
    write_auth_json(&auth_path, &options)?;

    Ok(ConfigureCodexAppReport {
        codex_home,
        config_path,
        auth_path,
        backend_url: options.backend_url,
    })
}

pub fn uninstall_codex_app(
    codex_home: Option<PathBuf>,
    backend_url: &str,
) -> Result<UninstallCodexAppReport> {
    let codex_home = codex_home.unwrap_or_else(default_codex_home);
    let config_path = codex_home.join("config.toml");
    let auth_path = codex_home.join("auth.json");

    let (removed_chatgpt_base_url, removed_model_provider) =
        uninstall_config_toml(&config_path, backend_url)?;
    let removed_auth = uninstall_auth_json(&auth_path)?;
    let gui_api_base = inspect_gui_api_base_url(backend_url);

    Ok(UninstallCodexAppReport {
        codex_home,
        config_path,
        auth_path,
        removed_chatgpt_base_url,
        removed_model_provider,
        removed_auth,
        gui_api_base,
    })
}

pub fn inspect_codex_app_config(
    codex_home: Option<PathBuf>,
    backend_url: &str,
) -> CodexAppConfigStatus {
    let codex_home = codex_home.unwrap_or_else(default_codex_home);
    let config_path = codex_home.join("config.toml");
    let auth_path = codex_home.join("auth.json");

    let (config_ok, config_error) = inspect_config_toml(&config_path, backend_url);
    let (auth_ok, auth_error) = inspect_auth_json(&auth_path);

    let gui_api_base = inspect_gui_api_base_url(backend_url);

    CodexAppConfigStatus {
        codex_home,
        config_path,
        auth_path,
        configured: config_ok && auth_ok,
        config_ok,
        auth_ok,
        config_error,
        auth_error,
        gui_api_base,
    }
}

pub fn inspect_gui_api_base_url(backend_url: &str) -> CodexAppGuiApiBaseStatus {
    #[cfg(target_os = "macos")]
    {
        match Command::new("launchctl")
            .arg("getenv")
            .arg("CODEX_API_BASE_URL")
            .output()
        {
            Ok(output) if output.status.success() => {
                let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
                let value = (!value.is_empty()).then_some(value);
                CodexAppGuiApiBaseStatus {
                    supported: true,
                    configured: value.as_deref() == Some(backend_url),
                    expected: backend_url.to_string(),
                    value,
                    error: None,
                }
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                CodexAppGuiApiBaseStatus {
                    supported: true,
                    configured: false,
                    expected: backend_url.to_string(),
                    value: None,
                    error: Some(if stderr.is_empty() {
                        output.status.to_string()
                    } else {
                        stderr
                    }),
                }
            }
            Err(err) => CodexAppGuiApiBaseStatus {
                supported: true,
                configured: false,
                expected: backend_url.to_string(),
                value: None,
                error: Some(err.to_string()),
            },
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        CodexAppGuiApiBaseStatus {
            supported: false,
            configured: false,
            expected: backend_url.to_string(),
            value: None,
            error: Some(
                "CODEX_API_BASE_URL one-click setup is only implemented for macOS launchctl"
                    .to_string(),
            ),
        }
    }
}

fn inspect_config_toml(path: &Path, backend_url: &str) -> (bool, Option<String>) {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return (false, Some("config.toml not found".to_string()));
        }
        Err(err) => return (false, Some(err.to_string())),
    };
    let doc = match raw.parse::<toml_edit::DocumentMut>() {
        Ok(doc) => doc,
        Err(err) => return (false, Some(err.to_string())),
    };
    let actual = doc
        .get("chatgpt_base_url")
        .and_then(|item| item.as_str())
        .map(str::trim);
    if actual == Some(backend_url) {
        (true, None)
    } else {
        (
            false,
            Some(format!(
                "chatgpt_base_url is {}, expected {backend_url}",
                actual.unwrap_or("<missing>")
            )),
        )
    }
}

fn inspect_auth_json(path: &Path) -> (bool, Option<String>) {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return (false, Some("auth.json not found".to_string()));
        }
        Err(err) => return (false, Some(err.to_string())),
    };
    let auth = match serde_json::from_str::<serde_json::Value>(&raw) {
        Ok(auth) => auth,
        Err(err) => return (false, Some(err.to_string())),
    };
    if auth.get("auth_mode").and_then(|value| value.as_str()) == Some("chatgptAuthTokens") {
        (true, None)
    } else {
        (
            false,
            Some("auth_mode is not chatgptAuthTokens".to_string()),
        )
    }
}

fn write_config_toml(path: &Path, options: &ConfigureCodexAppOptions) -> Result<()> {
    let mut doc = if path.exists() {
        parse_existing_config_toml(path)?
    } else {
        toml_edit::DocumentMut::new()
    };

    doc["chatgpt_base_url"] = toml_edit::value(&options.backend_url);

    let provider_base_url = non_empty(options.provider_base_url.as_deref());
    let provider_key = non_empty(options.provider_key.as_deref());
    let model = non_empty(options.model.as_deref());
    let provider_config_requested =
        provider_base_url.is_some() || provider_key.is_some() || model.is_some();

    if provider_config_requested {
        let provider_name = provider_name(options.provider_name.as_deref())?;
        let model = model.unwrap_or(DEFAULT_MODEL);

        doc["model_provider"] = toml_edit::value(provider_name.as_str());
        doc["model"] = toml_edit::value(model);
        doc["review_model"] = toml_edit::value(model);
        doc["model_reasoning_effort"] = toml_edit::value(DEFAULT_REASONING_EFFORT);
        doc["disable_response_storage"] = toml_edit::value(true);
        doc["network_access"] = toml_edit::value("enabled");
        doc["windows_wsl_setup_acknowledged"] = toml_edit::value(true);

        let provider = provider_table_mut(&mut doc, provider_name.as_str());
        provider["name"] = toml_edit::value(provider_name.as_str());
        provider["wire_api"] = toml_edit::value("responses");
        provider["requires_openai_auth"] = toml_edit::value(true);

        if let Some(provider_base_url) = provider_base_url {
            provider["base_url"] = toml_edit::value(provider_base_url);
        }
        if let Some(provider_key) = provider_key {
            provider["experimental_bearer_token"] = toml_edit::value(provider_key);
        }
    }

    backup_existing(path)?;
    std::fs::write(path, doc.to_string())
        .with_context(|| format!("failed to write {}", path.display()))
}

fn uninstall_config_toml(path: &Path, backend_url: &str) -> Result<(bool, bool)> {
    if !path.exists() {
        return Ok((false, false));
    }
    let mut doc = parse_existing_config_toml(path)?;

    let removed_chatgpt_base_url = doc
        .get("chatgpt_base_url")
        .and_then(|item| item.as_str())
        .map(str::trim)
        == Some(backend_url);
    if removed_chatgpt_base_url {
        doc.remove("chatgpt_base_url");
    }

    let removed_model_provider = doc
        .get("model_provider")
        .and_then(|item| item.as_str())
        .map(str::trim)
        .map(|value| value == DEFAULT_PROVIDER_NAME || value == "llmx")
        .unwrap_or(false);
    if removed_model_provider {
        doc.remove("model_provider");
    }

    if removed_chatgpt_base_url || removed_model_provider {
        backup_existing(path)?;
        std::fs::write(path, doc.to_string())
            .with_context(|| format!("failed to write {}", path.display()))?;
    }

    Ok((removed_chatgpt_base_url, removed_model_provider))
}

fn uninstall_auth_json(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let auth = serde_json::from_str::<serde_json::Value>(&raw)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    if auth.get("auth_mode").and_then(|value| value.as_str()) != Some("chatgptAuthTokens") {
        return Ok(false);
    }

    backup_existing(path)?;
    std::fs::remove_file(path).with_context(|| format!("failed to remove {}", path.display()))?;
    Ok(true)
}

fn parse_existing_config_toml(path: &Path) -> Result<toml_edit::DocumentMut> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    match raw.parse::<toml_edit::DocumentMut>() {
        Ok(doc) => Ok(doc),
        Err(err) => {
            let repaired = dedupe_duplicate_key_lines(&raw);
            if repaired == raw {
                return Err(err).with_context(|| format!("failed to parse {}", path.display()));
            }
            repaired.parse::<toml_edit::DocumentMut>().with_context(|| {
                format!(
                    "failed to parse {} after duplicate-key repair; original error: {err}",
                    path.display()
                )
            })
        }
    }
}

fn dedupe_duplicate_key_lines(raw: &str) -> String {
    let mut seen = HashMap::<String, HashSet<String>>::new();
    let mut current_table = String::new();
    let mut repaired = String::with_capacity(raw.len());

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            current_table = trimmed.to_string();
            repaired.push_str(line);
            repaired.push('\n');
            continue;
        }

        if let Some(key) = assignment_key(trimmed) {
            let table_seen = seen.entry(current_table.clone()).or_default();
            if !table_seen.insert(key.to_string()) {
                continue;
            }
        }

        repaired.push_str(line);
        repaired.push('\n');
    }

    repaired.trim_end_matches('\n').to_string()
}

fn assignment_key(trimmed: &str) -> Option<&str> {
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    let (key, _) = trimmed.split_once('=')?;
    let key = key.trim();
    if key.is_empty() || key.starts_with('[') || key.contains(char::is_whitespace) {
        None
    } else {
        Some(key)
    }
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn provider_table_mut<'a>(
    doc: &'a mut toml_edit::DocumentMut,
    provider_name: &str,
) -> &'a mut toml_edit::Table {
    if doc
        .get("model_providers")
        .is_none_or(|item| !item.is_table())
    {
        let mut table = toml_edit::Table::new();
        table.set_implicit(true);
        doc["model_providers"] = toml_edit::Item::Table(table);
    }
    let providers = doc["model_providers"]
        .as_table_mut()
        .expect("model_providers should be a table");
    if providers.is_empty() {
        providers.set_implicit(true);
    }
    if providers
        .get(provider_name)
        .is_none_or(|item| !item.is_table())
    {
        providers.insert(
            provider_name,
            toml_edit::Item::Table(toml_edit::Table::new()),
        );
    }
    providers
        .get_mut(provider_name)
        .expect("provider table should exist")
        .as_table_mut()
        .expect("provider should be a table")
}

fn provider_name(value: Option<&str>) -> Result<String> {
    let provider_name = non_empty(value).unwrap_or(DEFAULT_PROVIDER_NAME);
    if provider_name
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        Ok(provider_name.to_string())
    } else {
        Err(anyhow!(
            "provider name `{provider_name}` can only contain ASCII letters, numbers, `_`, and `-`"
        ))
    }
}

fn write_auth_json(path: &Path, options: &ConfigureCodexAppOptions) -> Result<()> {
    let jwt = local_chatgpt_jwt(options)?;
    let auth = json!({
        "auth_mode": "chatgptAuthTokens",
        "OPENAI_API_KEY": null,
        "tokens": {
            "id_token": jwt,
            "access_token": jwt,
            "refresh_token": "",
            "account_id": options.account_id,
        },
        "last_refresh": rfc3339_now(),
    });
    let raw = serde_json::to_string_pretty(&auth)?;
    backup_existing(path)?;
    std::fs::write(path, format!("{raw}\n"))
        .with_context(|| format!("failed to write {}", path.display()))
}

fn backup_existing(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let file_name = path
        .file_name()
        .ok_or_else(|| anyhow!("invalid backup path {}", path.display()))?
        .to_string_lossy();
    let backup = path.with_file_name(format!("{file_name}.bak"));
    std::fs::copy(path, &backup).with_context(|| {
        format!(
            "failed to backup existing {} to {}",
            path.display(),
            backup.display()
        )
    })?;
    Ok(())
}

fn local_chatgpt_jwt(options: &ConfigureCodexAppOptions) -> Result<String> {
    let now = unix_now()?;
    let exp = now + 10 * 365 * 24 * 60 * 60;
    let payload = json!({
        "iss": "https://auth.openai.com",
        "aud": ["https://api.openai.com/v1"],
        "iat": now,
        "nbf": now,
        "exp": exp,
        "sub": format!("local|{}", options.user_id),
        "email": options.email,
        "email_verified": true,
        "https://api.openai.com/profile": {
            "email": options.email,
            "email_verified": true,
        },
        "https://api.openai.com/auth": {
            "chatgpt_account_id": options.account_id,
            "chatgpt_account_user_id": format!("{}__{}", options.user_id, options.account_id),
            "chatgpt_plan_type": options.plan_type,
            "chatgpt_user_id": options.user_id,
            "user_id": options.user_id,
            "chatgpt_account_is_fedramp": false,
            "localhost": true,
            "groups": [],
            "organizations": [{
                "id": options.account_id,
                "is_default": true,
                "role": "owner",
                "title": "Codex Remote Local",
            }]
        },
        "scp": [
            "openid",
            "profile",
            "email",
            "offline_access",
            "api.connectors.read",
            "api.connectors.invoke"
        ],
    });

    Ok(format!(
        "{}.{}.{}",
        b64url_json(&json!({ "alg": "none", "typ": "JWT" }))?,
        b64url_json(&payload)?,
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"sig")
    ))
}

fn b64url_json(value: &serde_json::Value) -> Result<String> {
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(value)?))
}

fn unix_now() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| anyhow!("system time is before UNIX epoch: {err}"))?
        .as_secs())
}

fn rfc3339_now() -> String {
    match unix_now() {
        Ok(now) => format_rfc3339_utc(now),
        Err(_) => "1970-01-01T00:00:00Z".to_string(),
    }
}

fn format_rfc3339_utc(timestamp: u64) -> String {
    // Valid for normal contemporary timestamps. This avoids adding a time crate
    // just to stamp the local auth file.
    let days = (timestamp / 86_400) as i64;
    let seconds_of_day = timestamp % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn civil_from_days(days_since_unix_epoch: i64) -> (i32, u32, u32) {
    let z = days_since_unix_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if m <= 2 { 1 } else { 0 };
    (year as i32, m as u32, d as u32)
}

fn default_codex_home() -> PathBuf {
    // This helper configures the separately launched Codex App, not the
    // CODEX_HOME of the process that happens to run codex-remote.
    std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join(".codex"))
        .or_else(|| std::env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join(".codex")))
        .unwrap_or_else(|| PathBuf::from(".codex"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configure_codex_app_writes_provider_and_local_auth() {
        let codex_home = unique_temp_dir();
        let report = configure_codex_app(ConfigureCodexAppOptions {
            codex_home: Some(codex_home.clone()),
            backend_url: "http://127.0.0.1:3847/backend-api".to_string(),
            account_id: "acct_test".to_string(),
            user_id: "user_test".to_string(),
            email: "local@example.test".to_string(),
            plan_type: "pro".to_string(),
            provider_name: Some("codex".to_string()),
            provider_base_url: Some("https://api.example.invalid".to_string()),
            provider_key: Some("test-provider-key".to_string()),
            model: Some("gpt-5.5".to_string()),
        })
        .expect("configure codex app");

        let config = std::fs::read_to_string(report.config_path).expect("read config");
        assert!(config.contains("chatgpt_base_url = \"http://127.0.0.1:3847/backend-api\""));
        assert!(config.contains("model_provider = \"codex\""));
        assert!(config.contains("model = \"gpt-5.5\""));
        assert!(config.contains("review_model = \"gpt-5.5\""));
        assert!(config.contains("model_reasoning_effort = \"xhigh\""));
        assert!(config.contains("disable_response_storage = true"));
        assert!(config.contains("network_access = \"enabled\""));
        assert!(config.contains("windows_wsl_setup_acknowledged = true"));
        assert!(config.contains("[model_providers.codex]"));
        assert!(config.contains("base_url = \"https://api.example.invalid\""));
        assert!(config.contains("wire_api = \"responses\""));
        assert!(config.contains("requires_openai_auth = true"));
        assert!(config.contains("experimental_bearer_token = \"test-provider-key\""));

        let auth = std::fs::read_to_string(report.auth_path).expect("read auth");
        assert!(auth.contains("\"auth_mode\": \"chatgptAuthTokens\""));
        assert!(auth.contains("\"account_id\": \"acct_test\""));

        let _ = std::fs::remove_dir_all(codex_home);
    }

    #[test]
    fn rejects_invalid_provider_name() {
        let err = provider_name(Some("bad.provider")).expect_err("invalid provider name");
        assert!(err.to_string().contains("can only contain ASCII"));
    }

    #[test]
    fn configure_codex_app_repairs_duplicate_provider_key() {
        let codex_home = unique_temp_dir();
        let config_path = codex_home.join("config.toml");
        std::fs::write(
            &config_path,
            r#"model_provider = "codex"

[model_providers.codex]
name = "codex"
wire_api = "responses"
requires_openai_auth = true
base_url = "https://old.example"
requires_openai_auth = true
"#,
        )
        .expect("write invalid config");

        configure_codex_app(ConfigureCodexAppOptions {
            codex_home: Some(codex_home.clone()),
            backend_url: "http://127.0.0.1:3847/backend-api".to_string(),
            account_id: "acct_test".to_string(),
            user_id: "user_test".to_string(),
            email: "local@example.test".to_string(),
            plan_type: "pro".to_string(),
            provider_name: Some("codex".to_string()),
            provider_base_url: Some("https://api.example.invalid".to_string()),
            provider_key: Some("test-provider-key".to_string()),
            model: Some("gpt-5.5".to_string()),
        })
        .expect("configure codex app");

        let config = std::fs::read_to_string(config_path).expect("read config");
        assert_eq!(config.matches("requires_openai_auth = true").count(), 1);
        assert!(config.contains("base_url = \"https://api.example.invalid\""));

        let _ = std::fs::remove_dir_all(codex_home);
    }

    #[test]
    fn uninstall_codex_app_removes_local_routing_only() {
        let codex_home = unique_temp_dir();
        let config_path = codex_home.join("config.toml");
        let auth_path = codex_home.join("auth.json");
        std::fs::write(
            &config_path,
            r#"chatgpt_base_url = "http://127.0.0.1:3847/backend-api"
model_provider = "codex"
model = "gpt-5.5"

[model_providers.codex]
name = "codex"
base_url = "https://api.example.invalid"
"#,
        )
        .expect("write config");
        std::fs::write(
            &auth_path,
            r#"{
  "auth_mode": "chatgptAuthTokens",
  "tokens": {
    "account_id": "acct_test"
  }
}
"#,
        )
        .expect("write auth");

        let (removed_base_url, removed_model_provider) =
            uninstall_config_toml(&config_path, "http://127.0.0.1:3847/backend-api")
                .expect("uninstall config");
        let removed_auth = uninstall_auth_json(&auth_path).expect("uninstall auth");

        assert!(removed_base_url);
        assert!(removed_model_provider);
        assert!(removed_auth);

        let config = std::fs::read_to_string(&config_path).expect("read config");
        assert!(!config.contains("chatgpt_base_url"));
        assert!(!config.contains("model_provider = \"codex\""));
        assert!(config.contains("model = \"gpt-5.5\""));
        assert!(config.contains("[model_providers.codex]"));
        assert!(config.contains("base_url = \"https://api.example.invalid\""));
        assert!(!auth_path.exists());

        let _ = std::fs::remove_dir_all(codex_home);
    }

    fn unique_temp_dir() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after UNIX epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "codex-remote-test-{}-{}",
            std::process::id(),
            nanos
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn default_codex_home_ignores_process_codex_home() {
        let home = std::env::var_os("HOME").expect("HOME should exist for this test");
        let expected = PathBuf::from(home).join(".codex");

        let old_codex_home = std::env::var_os("CODEX_HOME");
        unsafe {
            std::env::set_var("CODEX_HOME", "/tmp/not-the-codex-app-home");
        }
        let actual = default_codex_home();
        match old_codex_home {
            Some(value) => unsafe {
                std::env::set_var("CODEX_HOME", value);
            },
            None => unsafe {
                std::env::remove_var("CODEX_HOME");
            },
        }

        assert_eq!(actual, expected);
    }
}
