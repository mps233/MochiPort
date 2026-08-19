//! Read-only Sub2API administrator integration for account-pool metrics.
//!
//! The administrator key never leaves the daemon. Upstream account credentials
//! are not returned by Sub2API's account API and are never requested here.

use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

use reqwest::{StatusCode, header::HeaderValue};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use thiserror::Error;

use super::config::provider_api_root;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const TOTAL_REQUEST_TIMEOUT: Duration = Duration::from_secs(45);
const RESPONSE_LIMIT: usize = 2 * 1024 * 1024;
const ACCOUNT_PAGE_SIZE: usize = 1000;
const MAX_ACCOUNTS: usize = 10_000;
// The endpoint is requested with a page size of 1,000; this keeps the total
// request time bounded alongside MAX_ACCOUNTS instead of trusting `pages`.
const MAX_ACCOUNT_PAGES: usize = 10;
const USAGE_PAGE_SIZE: usize = 100;
const MAX_USAGE_PAGES: usize = 10;
const PROBE_BATCH_SIZE: usize = 20;

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum Sub2ApiAccountPoolError {
    #[error("Sub2API 管理密钥无效")]
    Unauthorized,
    #[error("Sub2API 管理密钥没有账号读取权限")]
    Forbidden,
    #[error("当前 Sub2API 版本不支持账号池接口")]
    Unsupported,
    #[error("Sub2API 暂时不可用")]
    TemporarilyUnavailable,
    #[error("Sub2API 返回了无法识别的数据")]
    InvalidResponse,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AccountPoolSnapshot {
    pub source: &'static str,
    pub fetched_at_ms: u64,
    pub accounts: Vec<AccountPoolAccount>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<&'static str>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AccountPoolAccount {
    pub id: i64,
    pub name: String,
    /// Sanitized upstream base URL used to group channels in the GUI. It is
    /// derived from Sub2API's non-sensitive credentials metadata; userinfo,
    /// query strings, and fragments are removed before it crosses the API.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site_url: Option<String>,
    pub platform: String,
    pub account_type: String,
    pub status: String,
    pub schedulable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_rate_multiplier: Option<f64>,
    pub upstream_billing: AccountBillingSnapshot,
    pub upstream_balance: AccountBalanceSnapshot,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AccountBillingSnapshot {
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_rate_multiplier: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_rate_multiplier: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fresh_until: Option<String>,
    pub stale: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AccountBalanceSnapshot {
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining: Option<f64>,
    pub unlimited: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_valid: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<String>,
}

/// Most recent Sub2API usage record whose API key exactly matches a saved
/// ThreadRelay Provider. The matching key remains inside the daemon.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RecentProviderAccountSnapshot {
    pub account_id: i64,
    pub account_name: String,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
struct ApiEnvelope<T> {
    code: i64,
    #[serde(default)]
    message: String,
    data: T,
}

#[derive(Debug, Deserialize)]
struct AccountPage {
    #[serde(default)]
    items: Vec<AdminAccount>,
    #[serde(default)]
    pages: usize,
}

#[derive(Debug, Deserialize)]
struct UsagePage {
    #[serde(default)]
    items: Vec<AdminUsageRecord>,
    #[serde(default)]
    pages: usize,
}

#[derive(Debug, Deserialize)]
struct AdminUsageRecord {
    account_id: Option<i64>,
    account: Option<UsageAccount>,
    api_key: Option<UsageApiKey>,
    #[serde(default)]
    created_at: String,
}

#[derive(Debug, Deserialize)]
struct UsageAccount {
    id: i64,
    #[serde(default)]
    name: String,
}

#[derive(Debug, Deserialize)]
struct UsageApiKey {
    #[serde(default)]
    key: String,
}

#[derive(Debug, Clone, Deserialize)]
struct AdminAccount {
    id: i64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    platform: String,
    #[serde(rename = "type", default)]
    account_type: String,
    #[serde(default)]
    status: String,
    #[serde(default = "default_true")]
    schedulable: bool,
    rate_multiplier: Option<f64>,
    #[serde(default)]
    credentials: Value,
    #[serde(default)]
    extra: Value,
}

#[derive(Debug, Deserialize)]
struct ProbeBatch {
    #[serde(default)]
    results: Vec<ProbeResult>,
}

#[derive(Debug, Clone, Deserialize)]
struct ProbeResult {
    account_id: i64,
    snapshot: Option<ProbeSnapshot>,
    #[serde(default)]
    error: String,
}

#[derive(Debug, Clone, Deserialize)]
struct ProbeSnapshot {
    #[serde(default)]
    status: String,
    #[serde(default)]
    data: Value,
    received_at: Option<String>,
    fresh_until: Option<String>,
    #[serde(default)]
    last_error: String,
}

fn default_true() -> bool {
    true
}

pub async fn validate_admin_connection(
    client: &reqwest::Client,
    base_url: &str,
    admin_api_key: &str,
) -> Result<(), Sub2ApiAccountPoolError> {
    let _ = tokio::time::timeout(
        TOTAL_REQUEST_TIMEOUT,
        fetch_accounts(client, base_url, admin_api_key),
    )
    .await
    .map_err(|_| Sub2ApiAccountPoolError::TemporarilyUnavailable)??;
    Ok(())
}

pub async fn fetch_account_pool(
    client: &reqwest::Client,
    base_url: &str,
    admin_api_key: &str,
    force_billing_refresh: bool,
) -> Result<AccountPoolSnapshot, Sub2ApiAccountPoolError> {
    tokio::time::timeout(
        TOTAL_REQUEST_TIMEOUT,
        fetch_account_pool_inner(client, base_url, admin_api_key, force_billing_refresh),
    )
    .await
    .map_err(|_| Sub2ApiAccountPoolError::TemporarilyUnavailable)?
}

pub async fn fetch_recent_provider_account(
    client: &reqwest::Client,
    base_url: &str,
    admin_api_key: &str,
    provider_api_key: &str,
) -> Result<Option<RecentProviderAccountSnapshot>, Sub2ApiAccountPoolError> {
    tokio::time::timeout(
        TOTAL_REQUEST_TIMEOUT,
        fetch_recent_provider_account_inner(client, base_url, admin_api_key, provider_api_key),
    )
    .await
    .map_err(|_| Sub2ApiAccountPoolError::TemporarilyUnavailable)?
}

async fn fetch_recent_provider_account_inner(
    client: &reqwest::Client,
    base_url: &str,
    admin_api_key: &str,
    provider_api_key: &str,
) -> Result<Option<RecentProviderAccountSnapshot>, Sub2ApiAccountPoolError> {
    let provider_api_key = provider_api_key.trim();
    if provider_api_key.is_empty() {
        return Err(Sub2ApiAccountPoolError::Unauthorized);
    }

    let root = provider_api_root(base_url);
    for page in 1..=MAX_USAGE_PAGES {
        let url = format!(
            "{root}/api/v1/admin/usage?page={page}&page_size={USAGE_PAGE_SIZE}&sort_by=created_at&sort_order=desc"
        );
        let envelope: ApiEnvelope<UsagePage> = send_json(
            client
                .get(url)
                .header("x-api-key", sensitive_header(admin_api_key)?),
        )
        .await?;
        if envelope.code != 0 {
            return Err(classify_api_message(&envelope.message));
        }

        let UsagePage { items, pages } = envelope.data;
        if items.len() > USAGE_PAGE_SIZE
            || (pages == 0 && !items.is_empty())
            || (pages > 0 && page > pages)
        {
            return Err(Sub2ApiAccountPoolError::InvalidResponse);
        }
        if pages == 0 {
            return Ok(None);
        }

        for record in items {
            let Some(api_key) = record.api_key.as_ref() else {
                continue;
            };
            if api_key.key.trim() != provider_api_key {
                continue;
            }
            return normalize_recent_provider_account(record).map(Some);
        }

        if page >= pages {
            break;
        }
    }
    Ok(None)
}

async fn fetch_account_pool_inner(
    client: &reqwest::Client,
    base_url: &str,
    admin_api_key: &str,
    force_billing_refresh: bool,
) -> Result<AccountPoolSnapshot, Sub2ApiAccountPoolError> {
    let mut accounts = fetch_accounts(client, base_url, admin_api_key).await?;
    let mut warnings = Vec::new();
    let api_key_ids = account_ids_for_upstream_probe(&accounts);

    let refreshed_billing = if force_billing_refresh && !api_key_ids.is_empty() {
        match fetch_probe_batches(
            client,
            base_url,
            admin_api_key,
            "/api/v1/admin/accounts/upstream-billing-probe/batch",
            &api_key_ids,
        )
        .await
        {
            Ok(results) => {
                // Billing synchronization may have changed the local account
                // multiplier and persisted a new snapshot.
                if let Ok(latest) = fetch_accounts(client, base_url, admin_api_key).await {
                    accounts = latest;
                }
                probe_map(results)
            }
            Err(_) => {
                warnings.push("billing_refresh_failed");
                HashMap::new()
            }
        }
    } else {
        HashMap::new()
    };

    let usage_results = if api_key_ids.is_empty() {
        Some(HashMap::new())
    } else {
        match fetch_probe_batches(
            client,
            base_url,
            admin_api_key,
            "/api/v1/admin/accounts/upstream-usage-probe/batch",
            &api_key_ids,
        )
        .await
        {
            Ok(results) => Some(probe_map(results)),
            Err(Sub2ApiAccountPoolError::Unsupported) => {
                warnings.push("usage_probe_not_exposed");
                None
            }
            Err(_) => {
                warnings.push("usage_probe_failed");
                Some(HashMap::new())
            }
        }
    };

    let normalized = accounts
        .into_iter()
        .map(|account| {
            let cached_billing = account
                .extra
                .get("upstream_billing_probe")
                .cloned()
                .and_then(|value| serde_json::from_value::<ProbeSnapshot>(value).ok());
            let billing = refreshed_billing
                .get(&account.id)
                .cloned()
                .or(cached_billing.as_ref().map(|snapshot| ProbeResult {
                    account_id: account.id,
                    snapshot: Some(snapshot.clone()),
                    error: String::new(),
                }));
            let balance = usage_results
                .as_ref()
                .and_then(|results| results.get(&account.id));
            normalize_account(account, billing.as_ref(), balance, usage_results.is_none())
        })
        .collect();

    Ok(AccountPoolSnapshot {
        source: "sub2api_admin",
        fetched_at_ms: unix_time_ms(),
        accounts: normalized,
        warnings,
    })
}

async fn fetch_accounts(
    client: &reqwest::Client,
    base_url: &str,
    admin_api_key: &str,
) -> Result<Vec<AdminAccount>, Sub2ApiAccountPoolError> {
    let root = provider_api_root(base_url);
    let mut accounts = Vec::new();
    let mut account_ids = HashSet::new();
    let mut page = 1_usize;
    loop {
        let url = format!(
            "{root}/api/v1/admin/accounts?page={page}&page_size={ACCOUNT_PAGE_SIZE}&sort_by=name&sort_order=asc"
        );
        let envelope: ApiEnvelope<AccountPage> = send_json(
            client
                .get(url)
                .header("x-api-key", sensitive_header(admin_api_key)?),
        )
        .await?;
        if envelope.code != 0 {
            return Err(classify_api_message(&envelope.message));
        }
        let AccountPage { items, pages } = envelope.data;
        if pages == 0 {
            if page == 1 && items.is_empty() {
                break;
            }
            return Err(Sub2ApiAccountPoolError::InvalidResponse);
        }
        if pages > MAX_ACCOUNT_PAGES || page > pages || (page < pages && items.is_empty()) {
            return Err(Sub2ApiAccountPoolError::InvalidResponse);
        }
        if accounts.len().saturating_add(items.len()) > MAX_ACCOUNTS {
            return Err(Sub2ApiAccountPoolError::InvalidResponse);
        }
        for account in items {
            if !account_ids.insert(account.id) {
                return Err(Sub2ApiAccountPoolError::InvalidResponse);
            }
            accounts.push(account);
        }
        if page >= pages {
            break;
        }
        page += 1;
    }
    Ok(accounts)
}

async fn fetch_probe_batches(
    client: &reqwest::Client,
    base_url: &str,
    admin_api_key: &str,
    path: &str,
    account_ids: &[i64],
) -> Result<Vec<ProbeResult>, Sub2ApiAccountPoolError> {
    let root = provider_api_root(base_url);
    let mut results = Vec::with_capacity(account_ids.len());
    for chunk in account_ids.chunks(PROBE_BATCH_SIZE) {
        let envelope: ApiEnvelope<ProbeBatch> = send_json(
            client
                .post(format!("{root}{path}"))
                .header("x-api-key", sensitive_header(admin_api_key)?)
                .json(&serde_json::json!({ "account_ids": chunk })),
        )
        .await?;
        if envelope.code != 0 {
            return Err(classify_api_message(&envelope.message));
        }
        results.extend(envelope.data.results);
    }
    Ok(results)
}

async fn send_json<T: DeserializeOwned>(
    request: reqwest::RequestBuilder,
) -> Result<T, Sub2ApiAccountPoolError> {
    let response = request
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|_| Sub2ApiAccountPoolError::TemporarilyUnavailable)?;
    let status = response.status();
    if !status.is_success() {
        return Err(classify_http_status(status));
    }
    if response
        .content_length()
        .is_some_and(|length| length > RESPONSE_LIMIT as u64)
    {
        return Err(Sub2ApiAccountPoolError::InvalidResponse);
    }
    let mut response = response;
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| Sub2ApiAccountPoolError::TemporarilyUnavailable)?
    {
        if body.len().saturating_add(chunk.len()) > RESPONSE_LIMIT {
            return Err(Sub2ApiAccountPoolError::InvalidResponse);
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| Sub2ApiAccountPoolError::InvalidResponse)
}

fn sensitive_header(value: &str) -> Result<HeaderValue, Sub2ApiAccountPoolError> {
    let mut header =
        HeaderValue::from_str(value.trim()).map_err(|_| Sub2ApiAccountPoolError::Unauthorized)?;
    if header.is_empty() {
        return Err(Sub2ApiAccountPoolError::Unauthorized);
    }
    header.set_sensitive(true);
    Ok(header)
}

fn classify_http_status(status: StatusCode) -> Sub2ApiAccountPoolError {
    match status {
        StatusCode::UNAUTHORIZED => Sub2ApiAccountPoolError::Unauthorized,
        StatusCode::FORBIDDEN => Sub2ApiAccountPoolError::Forbidden,
        StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED => {
            Sub2ApiAccountPoolError::Unsupported
        }
        StatusCode::REQUEST_TIMEOUT
        | StatusCode::TOO_MANY_REQUESTS
        | StatusCode::BAD_GATEWAY
        | StatusCode::SERVICE_UNAVAILABLE
        | StatusCode::GATEWAY_TIMEOUT => Sub2ApiAccountPoolError::TemporarilyUnavailable,
        _ => Sub2ApiAccountPoolError::InvalidResponse,
    }
}

fn classify_api_message(message: &str) -> Sub2ApiAccountPoolError {
    let message = message.to_ascii_lowercase();
    if message.contains("unauthor") || message.contains("invalid admin") {
        Sub2ApiAccountPoolError::Unauthorized
    } else if message.contains("forbidden") || message.contains("permission") {
        Sub2ApiAccountPoolError::Forbidden
    } else {
        Sub2ApiAccountPoolError::InvalidResponse
    }
}

fn account_ids_for_upstream_probe(accounts: &[AdminAccount]) -> Vec<i64> {
    accounts
        .iter()
        .filter(|account| account.account_type.eq_ignore_ascii_case("apikey"))
        .map(|account| account.id)
        .collect()
}

fn probe_map(results: Vec<ProbeResult>) -> HashMap<i64, ProbeResult> {
    results
        .into_iter()
        .map(|result| (result.account_id, result))
        .collect()
}

fn normalize_recent_provider_account(
    record: AdminUsageRecord,
) -> Result<RecentProviderAccountSnapshot, Sub2ApiAccountPoolError> {
    let nested_id = record.account.as_ref().map(|account| account.id);
    if record.account_id.is_some() && nested_id.is_some() && record.account_id != nested_id {
        return Err(Sub2ApiAccountPoolError::InvalidResponse);
    }
    let account_id = record
        .account_id
        .or(nested_id)
        .filter(|id| *id > 0)
        .ok_or(Sub2ApiAccountPoolError::InvalidResponse)?;
    let account_name = record
        .account
        .and_then(|account| nonempty(account.name))
        .unwrap_or_else(|| format!("账号 {account_id}"));
    let created_at = record.created_at.trim().to_string();
    if account_name.chars().count() > 256
        || created_at.is_empty()
        || created_at.chars().count() > 128
    {
        return Err(Sub2ApiAccountPoolError::InvalidResponse);
    }
    Ok(RecentProviderAccountSnapshot {
        account_id,
        account_name,
        created_at,
    })
}

fn normalize_account(
    account: AdminAccount,
    billing_result: Option<&ProbeResult>,
    usage_result: Option<&ProbeResult>,
    usage_not_exposed: bool,
) -> AccountPoolAccount {
    let is_api_key = account.account_type.eq_ignore_ascii_case("apikey");
    AccountPoolAccount {
        id: account.id,
        name: nonempty(account.name).unwrap_or_else(|| format!("账号 {}", account.id)),
        site_url: sanitized_site_url(&account.credentials),
        platform: account.platform,
        account_type: account.account_type,
        status: account.status,
        schedulable: account.schedulable,
        local_rate_multiplier: account
            .rate_multiplier
            .filter(|value| value.is_finite() && *value >= 0.0),
        upstream_billing: normalize_billing(billing_result, is_api_key),
        upstream_balance: normalize_balance(usage_result, is_api_key, usage_not_exposed),
    }
}

/// Keep only a safe, stable URL for presentation/grouping. Sub2API redacts
/// credential values, but a custom base URL can still contain userinfo or
/// tracking components that should never be echoed by ThreadRelay.
fn sanitized_site_url(credentials: &Value) -> Option<String> {
    let raw = credentials
        .get("base_url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let mut parsed = url::Url::parse(raw).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return None;
    }
    let _ = parsed.set_username("");
    let _ = parsed.set_password(None);
    parsed.set_query(None);
    parsed.set_fragment(None);
    let normalized = parsed.as_str().trim_end_matches('/').to_string();
    (!normalized.is_empty()).then_some(normalized)
}

fn normalize_billing(result: Option<&ProbeResult>, applicable: bool) -> AccountBillingSnapshot {
    if !applicable {
        return empty_billing("not_applicable");
    }
    let Some(result) = result else {
        return empty_billing("not_exposed");
    };
    let Some(snapshot) = result.snapshot.as_ref() else {
        return empty_billing("temporarily_unavailable");
    };
    let resolved_rate_multiplier =
        nonnegative_finite_field(&snapshot.data, "resolved_rate_multiplier");
    let effective_rate_multiplier =
        nonnegative_finite_field(&snapshot.data, "effective_rate_multiplier");
    let mut state = probe_snapshot_state(snapshot, &result.error);
    if state == "available"
        && (resolved_rate_multiplier.is_none() || effective_rate_multiplier.is_none())
    {
        state = "invalid_response";
    }
    AccountBillingSnapshot {
        state,
        resolved_rate_multiplier,
        effective_rate_multiplier,
        observed_at: text_field(&snapshot.data, "observed_at")
            .or_else(|| snapshot.received_at.clone()),
        fresh_until: snapshot.fresh_until.clone(),
        stale: state != "available",
    }
}

fn normalize_balance(
    result: Option<&ProbeResult>,
    applicable: bool,
    usage_not_exposed: bool,
) -> AccountBalanceSnapshot {
    if !applicable {
        return empty_balance("not_applicable");
    }
    if usage_not_exposed {
        return empty_balance("not_exposed");
    }
    let Some(result) = result else {
        return empty_balance("temporarily_unavailable");
    };
    let Some(snapshot) = result.snapshot.as_ref() else {
        return empty_balance("temporarily_unavailable");
    };
    let remaining = finite_field(&snapshot.data, "remaining");
    let unlimited = snapshot
        .data
        .get("unlimited")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut state = probe_snapshot_state(snapshot, &result.error);
    if state == "available" && remaining.is_none() && !unlimited {
        state = "invalid_response";
    }
    AccountBalanceSnapshot {
        state,
        remaining,
        unlimited,
        unit: text_field(&snapshot.data, "unit"),
        mode: text_field(&snapshot.data, "mode"),
        plan_name: text_field(&snapshot.data, "plan_name"),
        account_valid: snapshot.data.get("account_valid").and_then(Value::as_bool),
        account_status: text_field(&snapshot.data, "account_status"),
        observed_at: snapshot.received_at.clone(),
    }
}

fn probe_snapshot_state(snapshot: &ProbeSnapshot, result_error: &str) -> &'static str {
    if !result_error.trim().is_empty() {
        return "temporarily_unavailable";
    }
    match snapshot.status.as_str() {
        "ok" => "available",
        "unsupported" => "unsupported",
        "failed" => match snapshot.last_error.as_str() {
            "unauthorized" => "unauthorized",
            "forbidden" => "forbidden",
            "invalid_response" => "invalid_response",
            "unsupported" => "unsupported",
            _ => "temporarily_unavailable",
        },
        _ => "invalid_response",
    }
}

fn empty_billing(state: &'static str) -> AccountBillingSnapshot {
    AccountBillingSnapshot {
        state,
        resolved_rate_multiplier: None,
        effective_rate_multiplier: None,
        observed_at: None,
        fresh_until: None,
        stale: false,
    }
}

fn empty_balance(state: &'static str) -> AccountBalanceSnapshot {
    AccountBalanceSnapshot {
        state,
        remaining: None,
        unlimited: false,
        unit: None,
        mode: None,
        plan_name: None,
        account_valid: None,
        account_status: None,
        observed_at: None,
    }
}

fn finite_field(value: &Value, key: &str) -> Option<f64> {
    value
        .get(key)
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
}

fn nonnegative_finite_field(value: &Value, key: &str) -> Option<f64> {
    finite_field(value, key).filter(|value| *value >= 0.0)
}

fn text_field(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).and_then(|value| {
        let value = value.trim();
        (!value.is_empty()).then(|| value.chars().take(256).collect())
    })
}

fn nonempty(value: String) -> Option<String> {
    let value = value.trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn unix_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        extract::Query,
        http::{HeaderMap, StatusCode, header::LOCATION},
        routing::get,
    };
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    async fn spawn_server(app: Router) -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock Sub2API server");
        let address = listener.local_addr().expect("mock Sub2API address");
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve mock Sub2API endpoint");
        });
        address
    }

    #[test]
    fn normalization_keeps_local_and_upstream_rates_separate() {
        let account = AdminAccount {
            id: 7,
            name: "primary".to_string(),
            platform: "openai".to_string(),
            account_type: "apikey".to_string(),
            status: "active".to_string(),
            schedulable: true,
            rate_multiplier: Some(1.0),
            credentials: serde_json::json!({
                "base_url": "https://relay.example.test/v1/?tracking=removed",
            }),
            extra: Value::Null,
        };
        let billing = ProbeResult {
            account_id: 7,
            snapshot: Some(ProbeSnapshot {
                status: "ok".to_string(),
                data: serde_json::json!({
                    "resolved_rate_multiplier": 0.06,
                    "effective_rate_multiplier": 0.09,
                }),
                received_at: Some("2026-08-15T00:00:00Z".to_string()),
                fresh_until: None,
                last_error: String::new(),
            }),
            error: String::new(),
        };

        let normalized = normalize_account(account, Some(&billing), None, true);

        assert_eq!(normalized.local_rate_multiplier, Some(1.0));
        assert_eq!(
            normalized.site_url.as_deref(),
            Some("https://relay.example.test/v1")
        );
        assert_eq!(
            normalized.upstream_billing.resolved_rate_multiplier,
            Some(0.06)
        );
        assert_eq!(
            normalized.upstream_billing.effective_rate_multiplier,
            Some(0.09)
        );
        assert_eq!(normalized.upstream_balance.state, "not_exposed");
    }

    #[test]
    fn site_url_removes_credentials_and_tracking_components() {
        let credentials = serde_json::json!({
            "base_url": "https://user:password@example.test/v1/?token=secret#fragment",
        });
        assert_eq!(
            sanitized_site_url(&credentials).as_deref(),
            Some("https://example.test/v1")
        );
        assert_eq!(sanitized_site_url(&serde_json::json!({})), None);
    }

    #[test]
    fn failed_snapshot_can_keep_stale_values_without_becoming_available() {
        let result = ProbeResult {
            account_id: 1,
            snapshot: Some(ProbeSnapshot {
                status: "failed".to_string(),
                data: serde_json::json!({"effective_rate_multiplier": 0.12}),
                received_at: Some("2026-08-15T00:00:00Z".to_string()),
                fresh_until: Some("2026-08-15T01:00:00Z".to_string()),
                last_error: "http_error".to_string(),
            }),
            error: String::new(),
        };

        let normalized = normalize_billing(Some(&result), true);

        assert_eq!(normalized.state, "temporarily_unavailable");
        assert_eq!(normalized.effective_rate_multiplier, Some(0.12));
        assert!(normalized.stale);
    }

    #[test]
    fn available_billing_requires_complete_nonnegative_multipliers() {
        for data in [
            serde_json::json!({"resolved_rate_multiplier": 0.5}),
            serde_json::json!({
                "resolved_rate_multiplier": -0.5,
                "effective_rate_multiplier": 0.75,
            }),
        ] {
            let result = ProbeResult {
                account_id: 1,
                snapshot: Some(ProbeSnapshot {
                    status: "ok".to_string(),
                    data,
                    received_at: None,
                    fresh_until: None,
                    last_error: String::new(),
                }),
                error: String::new(),
            };

            let normalized = normalize_billing(Some(&result), true);

            assert_eq!(normalized.state, "invalid_response");
            assert!(normalized.stale);
        }
    }

    #[test]
    fn available_balance_requires_value_or_unlimited_and_accepts_negative_balance() {
        let balance = |data| ProbeResult {
            account_id: 1,
            snapshot: Some(ProbeSnapshot {
                status: "ok".to_string(),
                data,
                received_at: None,
                fresh_until: None,
                last_error: String::new(),
            }),
            error: String::new(),
        };

        let negative = balance(serde_json::json!({"remaining": -12.5, "unlimited": false}));
        let normalized = normalize_balance(Some(&negative), true, false);
        assert_eq!(normalized.state, "available");
        assert_eq!(normalized.remaining, Some(-12.5));

        let missing = balance(serde_json::json!({"unlimited": false}));
        assert_eq!(
            normalize_balance(Some(&missing), true, false).state,
            "invalid_response"
        );

        let unlimited = balance(serde_json::json!({"unlimited": true}));
        assert_eq!(
            normalize_balance(Some(&unlimited), true, false).state,
            "available"
        );
    }

    #[tokio::test]
    async fn account_pagination_rejects_excessive_pages_and_accounts() {
        let excessive_pages = Router::new().route(
            "/api/v1/admin/accounts",
            get(|| async {
                Json(serde_json::json!({
                    "code": 0,
                    "message": "",
                    "data": { "items": [], "pages": MAX_ACCOUNT_PAGES + 1 },
                }))
            }),
        );
        let address = spawn_server(excessive_pages).await;
        let client = reqwest::Client::new();
        assert!(matches!(
            fetch_accounts(&client, &format!("http://{address}"), "admin-key").await,
            Err(Sub2ApiAccountPoolError::InvalidResponse)
        ));

        let items = (0..=MAX_ACCOUNTS)
            .map(|id| serde_json::json!({ "id": id }))
            .collect::<Vec<_>>();
        let excessive_accounts = Router::new().route(
            "/api/v1/admin/accounts",
            get(move || {
                let items = items.clone();
                async move {
                    Json(serde_json::json!({
                        "code": 0,
                        "message": "",
                        "data": { "items": items, "pages": 1 },
                    }))
                }
            }),
        );
        let address = spawn_server(excessive_accounts).await;
        assert!(matches!(
            fetch_accounts(&client, &format!("http://{address}"), "admin-key").await,
            Err(Sub2ApiAccountPoolError::InvalidResponse)
        ));
    }

    #[tokio::test]
    async fn recent_provider_account_paginates_and_matches_the_full_key() {
        const ADMIN_KEY: &str = "admin-key-must-not-leak";
        const PROVIDER_KEY: &str = "provider-key-must-not-leak";
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
        let app = Router::new().route(
            "/api/v1/admin/usage",
            get(
                move |headers: HeaderMap, Query(query): Query<HashMap<String, String>>| {
                    let recorded = recorded.clone();
                    async move {
                        let page = query
                            .get("page")
                            .and_then(|value| value.parse::<usize>().ok())
                            .expect("usage page");
                        recorded.lock().expect("record usage request").push((
                            page,
                            query.get("page_size").cloned(),
                            headers
                                .get("x-api-key")
                                .and_then(|value| value.to_str().ok())
                                .unwrap_or_default()
                                .to_string(),
                        ));
                        let item = if page == 1 {
                            serde_json::json!({
                                "account_id": 1,
                                "account": { "id": 1, "name": "other" },
                                "api_key": { "key": "different-provider-key" },
                                "created_at": "2026-08-16T23:59:00+08:00"
                            })
                        } else {
                            serde_json::json!({
                                "account_id": 2,
                                "account": { "id": 2, "name": "mdkj" },
                                "api_key": { "key": PROVIDER_KEY },
                                "created_at": "2026-08-17T00:00:00+08:00"
                            })
                        };
                        Json(serde_json::json!({
                            "code": 0,
                            "message": "success",
                            "data": { "items": [item], "pages": 3 }
                        }))
                    }
                },
            ),
        );
        let address = spawn_server(app).await;
        let account = fetch_recent_provider_account(
            &reqwest::Client::new(),
            &format!("http://{address}"),
            ADMIN_KEY,
            PROVIDER_KEY,
        )
        .await
        .expect("recent account request")
        .expect("matching account");

        assert_eq!(
            account,
            RecentProviderAccountSnapshot {
                account_id: 2,
                account_name: "mdkj".to_string(),
                created_at: "2026-08-17T00:00:00+08:00".to_string(),
            }
        );
        assert_eq!(
            requests.lock().expect("read usage requests").as_slice(),
            &[
                (1, Some(USAGE_PAGE_SIZE.to_string()), ADMIN_KEY.to_string()),
                (2, Some(USAGE_PAGE_SIZE.to_string()), ADMIN_KEY.to_string()),
            ]
        );
    }

    #[tokio::test]
    async fn recent_provider_account_scan_is_bounded() {
        let requests = Arc::new(AtomicUsize::new(0));
        let recorded = requests.clone();
        let app = Router::new().route(
            "/api/v1/admin/usage",
            get(move || {
                let recorded = recorded.clone();
                async move {
                    recorded.fetch_add(1, Ordering::SeqCst);
                    Json(serde_json::json!({
                        "code": 0,
                        "message": "success",
                        "data": {
                            "items": [{
                                "account_id": 1,
                                "account": { "id": 1, "name": "other" },
                                "api_key": { "key": "different-provider-key" },
                                "created_at": "2026-08-17T00:00:00+08:00"
                            }],
                            "pages": MAX_USAGE_PAGES + 100
                        }
                    }))
                }
            }),
        );
        let address = spawn_server(app).await;
        let account = fetch_recent_provider_account(
            &reqwest::Client::new(),
            &format!("http://{address}"),
            "admin-key",
            "missing-provider-key",
        )
        .await
        .expect("bounded usage scan");

        assert_eq!(account, None);
        assert_eq!(requests.load(Ordering::SeqCst), MAX_USAGE_PAGES);
    }

    #[tokio::test]
    async fn sensitive_client_does_not_forward_admin_key_across_redirects() {
        let leaked_headers = Arc::new(AtomicUsize::new(0));
        let target_headers = leaked_headers.clone();
        let target = Router::new().route(
            "/leak",
            get(move |headers: HeaderMap| {
                let leaked_headers = target_headers.clone();
                async move {
                    if headers.contains_key("x-api-key") {
                        leaked_headers.fetch_add(1, Ordering::SeqCst);
                    }
                    Json(serde_json::json!({}))
                }
            }),
        );
        let target_address = spawn_server(target).await;
        let location = format!("http://{target_address}/leak");
        let redirect = Router::new().route(
            "/api/v1/admin/accounts",
            get(move || {
                let location = location.clone();
                async move { (StatusCode::FOUND, [(LOCATION, location)], "") }
            }),
        );
        let redirect_address = spawn_server(redirect).await;
        let client = crate::outbound_http::build_sensitive_client(
            &crate::config::OutboundProxyConfig::default(),
            None,
        )
        .expect("no-redirect client");

        assert_eq!(
            validate_admin_connection(
                &client,
                &format!("http://{redirect_address}"),
                "redirect-canary-admin-key",
            )
            .await,
            Err(Sub2ApiAccountPoolError::InvalidResponse)
        );
        assert_eq!(leaked_headers.load(Ordering::SeqCst), 0);
    }
}
