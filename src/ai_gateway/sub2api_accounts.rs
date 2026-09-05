//! Sub2API administrator integration for account-pool metrics and scheduling.
//!
//! The administrator key never leaves the daemon. Sub2API's regular account
//! API keeps upstream credentials redacted; when the deployment supports it,
//! the official admin backup export (`GET /api/v1/admin/accounts/data`) is
//! read once per refresh cycle to obtain them for balance probing. Those
//! upstream keys exist only in memory for the duration of a probe: they are
//! never logged, persisted, or included in any response.

use std::{
    collections::{HashMap, HashSet},
    sync::{Mutex, OnceLock},
    time::Duration,
};

use futures_util::StreamExt;
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
// Direct upstream balance probing (stock Sub2API has no batch usage probe).
// The cache keeps the per-request fan-out infrequent; a successful probe is
// trusted longer than a failed one so flaky upstreams retry without pinning
// the account pool request to their timeout.
const UPSTREAM_PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const UPSTREAM_PROBE_CONCURRENCY: usize = 8;
const UPSTREAM_PROBE_BUDGET: Duration = Duration::from_secs(8);
const BALANCE_CACHE_SUCCESS_TTL_MS: u64 = 600_000;
const BALANCE_CACHE_FAILURE_TTL_MS: u64 = 120_000;
// One API style relays report an effectively unbounded hard limit when no
// explicit quota is configured on the account.
const ONE_API_UNLIMITED_HARD_LIMIT_USD: f64 = 100_000_000.0;
// Site display names change essentially never; probe one request per unique
// site root and trust a successful answer for a day.
const SITE_NAME_PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const SITE_NAME_PROBE_CONCURRENCY: usize = 8;
const SITE_NAME_PROBE_BUDGET: Duration = Duration::from_secs(8);
const SITE_NAME_SUCCESS_TTL_MS: u64 = 24 * 60 * 60 * 1000;
// A cold-start burst can transiently fail slow sites (budget cut, TLS under
// load); retry soon so a first-fetch failure does not stick for an hour.
const SITE_NAME_FAILURE_TTL_MS: u64 = 600_000;
const SITE_NAME_MAX_LEN: usize = 64;
// Some deployments pack their whole public config (model pricing, etc.) into
// the settings endpoint; the probe only needs the site_name field.
const SITE_NAME_RESPONSE_LIMIT: usize = 8 * 1024 * 1024;
/// Template site names carry no identity: a deployment that has not
/// customized its name is better labeled by its domain.
const SITE_NAME_TEMPLATE_DEFAULTS: [&str; 3] = ["ai gateway", "sub2api", "ai api gateway"];
const HTML_TITLE_TEMPLATE_SUFFIX: &str = "ai api gateway";

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum Sub2ApiAccountPoolError {
    #[error("Sub2API 管理密钥无效")]
    Unauthorized,
    #[error("Sub2API 管理密钥没有账号管理权限")]
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
    /// Display name the upstream site reports for itself (Sub2API public
    /// settings or an One API status endpoint). None when the site does not
    /// expose one or only exposes a template default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site_name: Option<String>,
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
/// MochiPort Provider. The matching key remains inside the daemon.
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

/// Toggle whether an upstream account participates in scheduling. This is the
/// only write operation the Sub2API integration performs, and it mirrors the
/// official admin endpoint the Sub2API panel itself uses.
pub async fn set_account_schedulable(
    client: &reqwest::Client,
    base_url: &str,
    admin_api_key: &str,
    account_id: i64,
    schedulable: bool,
) -> Result<(), Sub2ApiAccountPoolError> {
    tokio::time::timeout(
        TOTAL_REQUEST_TIMEOUT,
        async {
            let root = provider_api_root(base_url);
            let envelope: ApiEnvelope<Value> = send_json(
                client
                    .post(format!(
                        "{root}/api/v1/admin/accounts/{account_id}/schedulable"
                    ))
                    .header("x-api-key", sensitive_header(admin_api_key)?)
                    .json(&serde_json::json!({ "schedulable": schedulable })),
            )
            .await?;
            if envelope.code != 0 {
                return Err(classify_api_message(&envelope.message));
            }
            Ok(())
        },
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

    // Balance probing and site-name discovery are independent fan-outs against
    // the same upstreams; run them together so a cold cache keeps the whole
    // pool request inside the client-visible budget.
    let usage_probe = async {
        if api_key_ids.is_empty() {
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
                    // Stock Sub2API builds never expose the batch probe. Fall back
                    // to the official admin backup export: read upstream
                    // credentials once (in memory only) and probe each upstream
                    // directly.
                    match probe_balances_via_export(client, base_url, admin_api_key, &accounts).await {
                        Ok(results) => Some(results),
                        Err(Sub2ApiAccountPoolError::Unsupported) => {
                            warnings.push("balance_export_unavailable");
                            None
                        }
                        Err(Sub2ApiAccountPoolError::Forbidden) => {
                            warnings.push("balance_export_forbidden");
                            None
                        }
                        Err(_) => {
                            warnings.push("usage_probe_failed");
                            Some(HashMap::new())
                        }
                    }
                }
                Err(_) => {
                    warnings.push("usage_probe_failed");
                    Some(HashMap::new())
                }
            }
        }
    };
    let (usage_results, site_names) = tokio::join!(usage_probe, probe_site_names(client, &accounts));

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
            normalize_account(
                account,
                billing.as_ref(),
                balance,
                usage_results.is_none(),
                &site_names,
            )
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
    send_json_limited(request, RESPONSE_LIMIT).await
}

async fn send_json_limited<T: DeserializeOwned>(
    request: reqwest::RequestBuilder,
    response_limit: usize,
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
        .is_some_and(|length| length > response_limit as u64)
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
        if body.len().saturating_add(chunk.len()) > response_limit {
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

type BalanceCacheKey = (String, i64);

struct CachedBalance {
    result: ProbeResult,
    probed_at_ms: u64,
    ok: bool,
}

static UPSTREAM_BALANCE_CACHE: OnceLock<Mutex<HashMap<BalanceCacheKey, CachedBalance>>> =
    OnceLock::new();

fn balance_cache() -> &'static Mutex<HashMap<BalanceCacheKey, CachedBalance>> {
    UPSTREAM_BALANCE_CACHE.get_or_init(Mutex::default)
}

fn cached_balance(root: &str, account_id: i64, now_ms: u64) -> Option<ProbeResult> {
    let cache = balance_cache().lock().expect("balance cache poisoned");
    let entry = cache.get(&(root.to_string(), account_id))?;
    let ttl = if entry.ok {
        BALANCE_CACHE_SUCCESS_TTL_MS
    } else {
        BALANCE_CACHE_FAILURE_TTL_MS
    };
    (now_ms.saturating_sub(entry.probed_at_ms) < ttl).then(|| entry.result.clone())
}

fn cache_insert_balance(root: &str, account_id: i64, ok: bool, probed_at_ms: u64, result: &ProbeResult) {
    let mut cache = balance_cache().lock().expect("balance cache poisoned");
    cache.insert(
        (root.to_string(), account_id),
        CachedBalance {
            result: result.clone(),
            probed_at_ms,
            ok,
        },
    );
}

/// Stock Sub2API builds never expose the batch usage probe, so balances are
/// probed directly against each upstream. Credentials come from the official
/// admin backup export and live only inside this call; results are cached per
/// upstream root so the fan-out stays infrequent across pool refreshes.
async fn probe_balances_via_export(
    client: &reqwest::Client,
    base_url: &str,
    admin_api_key: &str,
    accounts: &[AdminAccount],
) -> Result<HashMap<i64, ProbeResult>, Sub2ApiAccountPoolError> {
    let root = provider_api_root(base_url);
    let export = fetch_credential_export(client, &root, admin_api_key).await?;
    let mut credentials = match_upstream_credentials(&export, accounts);
    let now_ms = unix_time_ms();
    let mut results = HashMap::new();
    credentials.retain(|(id, credential)| match cached_balance(&credential.root, *id, now_ms) {
        Some(cached) => {
            results.insert(*id, cached);
            false
        }
        None => true,
    });
    if !credentials.is_empty() {
        let live_ids: HashSet<i64> = accounts.iter().map(|account| account.id).collect();
        let mut probes = futures_util::stream::iter(credentials)
            .map(|(id, credential)| async move {
                let snapshot = tokio::time::timeout(
                    UPSTREAM_PROBE_TIMEOUT,
                    probe_upstream_balance(client, &credential.root, &credential.api_key),
                )
                .await
                .unwrap_or_else(|_| upstream_snapshot("failed", "", Value::Null));
                (id, credential, snapshot)
            })
            .buffer_unordered(UPSTREAM_PROBE_CONCURRENCY);
        // Bound the whole fan-out so the account-pool request stays responsive;
        // upstreams that miss the budget simply retry on the next refresh.
        let _ = tokio::time::timeout(UPSTREAM_PROBE_BUDGET, async {
            while let Some((id, credential, snapshot)) = probes.next().await {
                let ok = snapshot.status == "ok";
                let result = ProbeResult {
                    account_id: id,
                    snapshot: Some(snapshot),
                    error: String::new(),
                };
                cache_insert_balance(&credential.root, id, ok, now_ms, &result);
                results.insert(id, result);
            }
        })
        .await;
        // Drop cache entries for accounts that disappeared from the pool so a
        // long-running daemon does not accumulate stale upstream state.
        let mut cache = balance_cache().lock().expect("balance cache poisoned");
        cache.retain(|(cached_root, id), _| cached_root == &root && live_ids.contains(id));
    }
    Ok(results)
}

async fn fetch_credential_export(
    client: &reqwest::Client,
    root: &str,
    admin_api_key: &str,
) -> Result<Vec<DataExportAccount>, Sub2ApiAccountPoolError> {
    let envelope: ApiEnvelope<DataExportPage> = send_json(
        client
            .get(format!("{root}/api/v1/admin/accounts/data"))
            .header("x-api-key", sensitive_header(admin_api_key)?),
    )
    .await?;
    if envelope.code != 0 {
        return Err(classify_api_message(&envelope.message));
    }
    Ok(envelope.data.accounts)
}

#[derive(Deserialize)]
struct DataExportPage {
    #[serde(default)]
    accounts: Vec<DataExportAccount>,
}

/// Raw admin-backup account entry. Deliberately excludes `Debug`/`Clone`:
/// `credentials` carries the upstream API key in plaintext.
#[derive(Deserialize)]
struct DataExportAccount {
    #[serde(default)]
    name: String,
    #[serde(default)]
    platform: String,
    #[serde(rename = "type", default)]
    account_type: String,
    #[serde(default)]
    credentials: DataExportCredentials,
}

#[derive(Deserialize, Default)]
struct DataExportCredentials {
    #[serde(default)]
    api_key: String,
    #[serde(default)]
    base_url: String,
}

/// A matched upstream credential. The key exists only between the backup
/// export and its single probe request; the struct intentionally has no
/// `Debug` derive so the key can never end up in a log line by accident.
struct UpstreamCredential {
    api_key: String,
    root: String,
}

/// Pair admin-list account ids with raw credentials from the backup export.
/// Matching is by platform + type + trimmed name + sanitized base URL; entries
/// that cannot be matched stay unprobed rather than guessing.
fn match_upstream_credentials(
    export: &[DataExportAccount],
    accounts: &[AdminAccount],
) -> Vec<(i64, UpstreamCredential)> {
    let mut by_key: HashMap<(String, String, String, String), &DataExportAccount> = HashMap::new();
    for entry in export {
        if let Some(key) = export_match_key(entry) {
            by_key.insert(key, entry);
        }
    }
    let mut matched = Vec::new();
    for account in accounts {
        if !account.account_type.eq_ignore_ascii_case("apikey") {
            continue;
        }
        let Some(key) = admin_match_key(account) else {
            continue;
        };
        let Some(entry) = by_key.get(&key) else {
            continue;
        };
        let api_key = entry.credentials.api_key.trim();
        if api_key.is_empty() || entry.credentials.base_url.trim().is_empty() {
            continue;
        }
        matched.push((
            account.id,
            UpstreamCredential {
                api_key: api_key.to_string(),
                root: provider_api_root(&entry.credentials.base_url),
            },
        ));
    }
    matched
}

fn admin_match_key(account: &AdminAccount) -> Option<(String, String, String, String)> {
    Some((
        account.platform.to_ascii_lowercase(),
        account.account_type.to_ascii_lowercase(),
        account.name.trim().to_string(),
        sanitized_site_url(&account.credentials)?,
    ))
}

fn export_match_key(entry: &DataExportAccount) -> Option<(String, String, String, String)> {
    Some((
        entry.platform.to_ascii_lowercase(),
        entry.account_type.to_ascii_lowercase(),
        entry.name.trim().to_string(),
        sanitize_site_url_str(&entry.credentials.base_url)?,
    ))
}

/// Probe one upstream with its own credential: the Sub2API-compatible
/// `/v1/usage` snapshot first, then the One API billing pair. Only derived
/// balance fields survive; the credential is dropped with this call.
async fn probe_upstream_balance(
    client: &reqwest::Client,
    root: &str,
    api_key: &str,
) -> ProbeSnapshot {
    let authorization = match bearer_header(api_key) {
        Ok(header) => header,
        Err(_) => return upstream_snapshot("failed", "unauthorized", Value::Null),
    };
    match send_json::<Value>(
        client
            .get(format!("{root}/v1/usage"))
            .header("Authorization", authorization),
    )
    .await
    {
        Ok(value) => {
            let Some(data) = upstream_usage_snapshot(&value) else {
                return upstream_snapshot("failed", "invalid_response", Value::Null);
            };
            upstream_snapshot("ok", "", data)
        }
        Err(Sub2ApiAccountPoolError::Unsupported) => {
            one_api_balance_snapshot(client, root, api_key).await
        }
        Err(error) => probe_error_snapshot(error),
    }
}

/// One API style relays answer the OpenAI billing pair instead of
/// `/v1/usage`: the subscription declares the granted limit in USD and the
/// usage endpoint reports spent cents.
async fn one_api_balance_snapshot(
    client: &reqwest::Client,
    root: &str,
    api_key: &str,
) -> ProbeSnapshot {
    let authorization = match bearer_header(api_key) {
        Ok(header) => header,
        Err(_) => return upstream_snapshot("failed", "unauthorized", Value::Null),
    };
    let subscription = match send_json::<Value>(
        client
            .get(format!("{root}/v1/dashboard/billing/subscription"))
            .header("Authorization", authorization),
    )
    .await
    {
        Ok(value) => value,
        Err(error) => return probe_error_snapshot(error),
    };
    let Some(hard_limit) = finite_field(&subscription, "hard_limit_usd")
        .or_else(|| finite_field(&subscription, "system_hard_limit_usd"))
    else {
        return upstream_snapshot("failed", "invalid_response", Value::Null);
    };
    let authorization = match bearer_header(api_key) {
        Ok(header) => header,
        Err(_) => return upstream_snapshot("failed", "unauthorized", Value::Null),
    };
    let used_usd = match send_json::<Value>(
        client
            .get(format!("{root}/v1/dashboard/billing/usage"))
            .header("Authorization", authorization),
    )
    .await
    {
        Ok(value) => finite_field(&value, "total_usage").map(|cents| cents / 100.0).unwrap_or(0.0),
        Err(Sub2ApiAccountPoolError::Unsupported) => 0.0,
        Err(error) => return probe_error_snapshot(error),
    };
    upstream_snapshot(
        "ok",
        "",
        one_api_balance_data(hard_limit, Some(used_usd)),
    )
}

/// Pure derivation of One API balance fields: remaining in USD with the
/// "no explicit quota" hard limit reported as unlimited.
fn one_api_balance_data(hard_limit_usd: f64, used_usd: Option<f64>) -> Value {
    let remaining = (hard_limit_usd - used_usd.unwrap_or(0.0)).max(0.0);
    serde_json::json!({
        "remaining": remaining,
        "unlimited": hard_limit_usd >= ONE_API_UNLIMITED_HARD_LIMIT_USD,
        "unit": "USD",
    })
}

/// Normalize a Sub2API-compatible `/v1/usage` payload into the snapshot data
/// consumed by `normalize_balance`. Mirrors the upstream project's own usage
/// probe semantics, including the `remaining: -1` unlimited rule that does
/// not apply to wallet balances.
fn upstream_usage_snapshot(payload: &Value) -> Option<Value> {
    let mode = payload
        .get("mode")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|mode| !mode.is_empty());
    if let Some(mode) = mode
        && mode != "quota_limited"
        && mode != "unrestricted"
    {
        return None;
    }
    let account_valid = payload
        .get("isValid")
        .and_then(Value::as_bool)
        .or_else(|| payload.get("is_active").and_then(Value::as_bool))?;
    let balance = finite_field(payload, "balance");
    let quota_remaining = payload
        .get("quota")
        .and_then(|quota| finite_field(quota, "remaining"));
    let remaining = balance
        .or_else(|| finite_field(payload, "remaining"))
        .or(quota_remaining);
    if mode.is_none() && remaining.is_none() {
        return None;
    }
    if remaining.is_some_and(|amount| balance.is_none() && amount < -1.0) {
        return None;
    }
    let plan_name = text_field(payload, "planName").or_else(|| text_field(payload, "plan_name"));
    let unlimited = balance.is_none()
        && quota_remaining.is_none()
        && remaining == Some(-1.0)
        && !is_wallet_plan(plan_name.as_deref());

    let mut data = serde_json::Map::new();
    data.insert("unlimited".to_string(), Value::Bool(unlimited));
    data.insert("account_valid".to_string(), Value::Bool(account_valid));
    if let Some(mode) = mode {
        data.insert("mode".to_string(), Value::String(mode.to_string()));
    }
    if let Some(status) = text_field(payload, "status") {
        data.insert("account_status".to_string(), Value::String(status));
    }
    if let Some(plan_name) = plan_name {
        data.insert("plan_name".to_string(), Value::String(plan_name));
    }
    if let Some(remaining) = remaining {
        data.insert("remaining".to_string(), serde_json::json!(remaining));
    }
    let unit = text_field(payload, "unit").unwrap_or_else(|| "USD".to_string());
    data.insert("unit".to_string(), Value::String(unit));
    Some(Value::Object(data))
}

/// Sub2API wallets report `-1` after overdrawing; only a subscription-style
/// `-1` without a wallet plan means unlimited.
fn is_wallet_plan(plan_name: Option<&str>) -> bool {
    let Some(plan_name) = plan_name else {
        return false;
    };
    let normalized = plan_name.trim().to_lowercase();
    normalized == "wallet balance" || normalized == "钱包余额"
}

fn upstream_snapshot(status: &'static str, last_error: &str, data: Value) -> ProbeSnapshot {
    ProbeSnapshot {
        status: status.to_string(),
        data,
        received_at: Some(rfc3339_now()),
        fresh_until: None,
        last_error: last_error.to_string(),
    }
}

fn probe_error_snapshot(error: Sub2ApiAccountPoolError) -> ProbeSnapshot {
    match error {
        Sub2ApiAccountPoolError::Unauthorized => {
            upstream_snapshot("failed", "unauthorized", Value::Null)
        }
        Sub2ApiAccountPoolError::Forbidden => upstream_snapshot("failed", "forbidden", Value::Null),
        Sub2ApiAccountPoolError::InvalidResponse => {
            upstream_snapshot("failed", "invalid_response", Value::Null)
        }
        Sub2ApiAccountPoolError::Unsupported | Sub2ApiAccountPoolError::TemporarilyUnavailable => {
            upstream_snapshot("failed", "", Value::Null)
        }
    }
}

fn bearer_header(api_key: &str) -> Result<HeaderValue, Sub2ApiAccountPoolError> {
    let key = api_key.trim();
    if key.is_empty() {
        return Err(Sub2ApiAccountPoolError::Unauthorized);
    }
    let mut header = HeaderValue::from_str(&format!("Bearer {key}"))
        .map_err(|_| Sub2ApiAccountPoolError::Unauthorized)?;
    header.set_sensitive(true);
    Ok(header)
}

type SiteNameCacheKey = String;

struct CachedSiteName {
    name: Option<String>,
    probed_at_ms: u64,
}

static SITE_NAME_CACHE: OnceLock<Mutex<HashMap<SiteNameCacheKey, CachedSiteName>>> = OnceLock::new();

fn site_name_cache() -> &'static Mutex<HashMap<SiteNameCacheKey, CachedSiteName>> {
    SITE_NAME_CACHE.get_or_init(Mutex::default)
}

fn cached_site_name(root: &str, now_ms: u64) -> Option<String> {
    let cache = site_name_cache().lock().expect("site name cache poisoned");
    let entry = cache.get(root)?;
    let ttl = if entry.name.is_some() {
        SITE_NAME_SUCCESS_TTL_MS
    } else {
        SITE_NAME_FAILURE_TTL_MS
    };
    (now_ms.saturating_sub(entry.probed_at_ms) < ttl)
        .then(|| entry.name.clone())
        .flatten()
}

/// Discover the display name each unique upstream site reports for itself.
/// Site names are static in practice, so a successful probe is trusted for a
/// day and a failed one retries after an hour, all per site root.
async fn probe_site_names(
    client: &reqwest::Client,
    accounts: &[AdminAccount],
) -> HashMap<String, String> {
    let mut roots: Vec<String> = Vec::new();
    for account in accounts {
        if let Some(site_url) = sanitized_site_url(&account.credentials) {
            let root = provider_api_root(&site_url);
            if !root.is_empty() && !roots.contains(&root) {
                roots.push(root);
            }
        }
    }
    let now_ms = unix_time_ms();
    let mut names = HashMap::new();
    let mut pending: Vec<String> = Vec::new();
    for root in roots {
        match cached_site_name(&root, now_ms) {
            Some(name) => {
                names.insert(root, name);
            }
            None => pending.push(root),
        }
    }
    if pending.is_empty() {
        return names;
    }

    let mut probes = futures_util::stream::iter(pending)
        .map(|root| async move {
            let name = tokio::time::timeout(SITE_NAME_PROBE_TIMEOUT, resolve_site_name(client, &root))
                .await
                .unwrap_or(None);
            (root, name)
        })
        .buffer_unordered(SITE_NAME_PROBE_CONCURRENCY);
    let _ = tokio::time::timeout(SITE_NAME_PROBE_BUDGET, async {
        while let Some((root, name)) = probes.next().await {
            let mut cache = site_name_cache().lock().expect("site name cache poisoned");
            cache.insert(
                root.clone(),
                CachedSiteName {
                    name: name.clone(),
                    probed_at_ms: now_ms,
                },
            );
            drop(cache);
            if let Some(name) = name {
                names.insert(root, name);
            }
        }
    })
    .await;
    names
}

/// Resolve one site's self-reported display name: the Sub2API public settings
/// first, then the One API status endpoint, then the panel HTML title. Names
/// matching template defaults are treated as absent so the GUI keeps labeling
/// those sites by their domain.
async fn resolve_site_name(client: &reqwest::Client, root: &str) -> Option<String> {
    if let Ok(envelope) = send_json_limited::<ApiEnvelope<Value>>(
        client
            .get(format!("{root}/api/v1/settings/public"))
            .header("Accept", "application/json"),
        SITE_NAME_RESPONSE_LIMIT,
    )
    .await
        && envelope.code == 0
        && let Some(name) = parse_public_site_name(&envelope.data)
    {
        return Some(name);
    }
    if let Ok(value) = send_json_limited::<Value>(
        client
            .get(format!("{root}/api/status"))
            .header("Accept", "application/json"),
        SITE_NAME_RESPONSE_LIMIT,
    )
    .await
        && let Some(name) = parse_status_site_name(&value)
    {
        return Some(name);
    }
    fetch_html_site_title(client, root).await
}

fn parse_public_site_name(value: &Value) -> Option<String> {
    clean_site_name(value.get("site_name")?.as_str()?)
}

fn parse_status_site_name(value: &Value) -> Option<String> {
    clean_site_name(value.pointer("/data/system_name")?.as_str()?)
}

fn clean_site_name(raw: &str) -> Option<String> {
    let name = raw.trim();
    if name.is_empty() || name.chars().count() > SITE_NAME_MAX_LEN {
        return None;
    }
    if SITE_NAME_TEMPLATE_DEFAULTS.contains(&name.to_lowercase().as_str()) {
        return None;
    }
    Some(name.to_string())
}

/// Last resort: read the panel page title. Sub2API templates render
/// "<custom name> - AI API Gateway"; keep the custom prefix when present.
async fn fetch_html_site_title(client: &reqwest::Client, root: &str) -> Option<String> {
    let response = client.get(root).header("Accept", "text/html").send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let body = response.bytes().await.ok()?;
    if body.len() > SITE_NAME_RESPONSE_LIMIT {
        return None;
    }
    let text = String::from_utf8_lossy(&body);
    let lowered = text.to_lowercase();
    let start = lowered.find("<title>")? + "<title>".len();
    let end = lowered[start..].find("</title>")? + start;
    let title = text[start..end].trim();
    if let Some(position) = title.rfind(" - ") {
        let suffix = title[position + " - ".len()..].trim();
        if suffix.eq_ignore_ascii_case(HTML_TITLE_TEMPLATE_SUFFIX) && position > 0 {
            return clean_site_name(title[..position].trim());
        }
    }
    clean_site_name(title)
}

/// RFC 3339 UTC stamp for balance observations, avoiding a time crate the
/// same way the auth-file stamp in `codex_app_config` does.
fn rfc3339_now() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = (seconds / 86_400) as i64;
    let seconds_of_day = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        seconds_of_day / 3_600,
        (seconds_of_day % 3_600) / 60,
        seconds_of_day % 60
    )
}

fn civil_from_days(days_since_unix_epoch: i64) -> (i32, u32, u32) {
    let z = days_since_unix_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 1_460) / 365;
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if m <= 2 { 1 } else { 0 };
    (year as i32, m as u32, d as u32)
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
    site_names: &HashMap<String, String>,
) -> AccountPoolAccount {
    let is_api_key = account.account_type.eq_ignore_ascii_case("apikey");
    let site_url = sanitized_site_url(&account.credentials);
    let site_name = site_url
        .as_deref()
        .map(provider_api_root)
        .and_then(|root| site_names.get(&root).cloned());
    AccountPoolAccount {
        id: account.id,
        name: nonempty(account.name).unwrap_or_else(|| format!("账号 {}", account.id)),
        site_url,
        site_name,
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
/// tracking components that should never be echoed by MochiPort.
fn sanitized_site_url(credentials: &Value) -> Option<String> {
    let raw = credentials
        .get("base_url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    sanitize_site_url_str(raw)
}

fn sanitize_site_url_str(raw: &str) -> Option<String> {
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
    let remaining = finite_field(&snapshot.data, "remaining")
        .or_else(|| finite_field(&snapshot.data, "balance"));
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
        plan_name: text_field(&snapshot.data, "plan_name")
            .or_else(|| text_field(&snapshot.data, "planName")),
        account_valid: snapshot
            .data
            .get("account_valid")
            .and_then(Value::as_bool)
            .or_else(|| snapshot.data.get("isValid").and_then(Value::as_bool))
            .or_else(|| snapshot.data.get("is_active").and_then(Value::as_bool)),
        account_status: text_field(&snapshot.data, "account_status")
            .or_else(|| text_field(&snapshot.data, "status"))
            .or_else(|| {
                snapshot
                    .data
                    .get("is_active")
                    .and_then(Value::as_bool)
                    .map(|active| if active { "active" } else { "inactive" }.to_string())
            }),
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
        routing::{get, post},
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

    #[tokio::test]
    async fn set_account_schedulable_posts_admin_key_and_boolean_body() {
        const ADMIN_KEY: &str = "admin-key-must-not-leak";
        let seen = Arc::new(Mutex::new(None::<(String, bool)>));
        let recorded = seen.clone();
        let app = Router::new().route(
            "/api/v1/admin/accounts/42/schedulable",
            post(move |headers: HeaderMap, Json(body): Json<Value>| {
                let recorded = recorded.clone();
                async move {
                    let key = headers
                        .get("x-api-key")
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string();
                    let schedulable = body
                        .get("schedulable")
                        .and_then(Value::as_bool)
                        .expect("schedulable body");
                    *recorded.lock().expect("record schedulable request") =
                        Some((key, schedulable));
                    Json(serde_json::json!({ "code": 0, "message": "success", "data": {} }))
                }
            }),
        );
        let address = spawn_server(app).await;

        set_account_schedulable(
            &reqwest::Client::new(),
            &format!("http://{address}"),
            ADMIN_KEY,
            42,
            false,
        )
        .await
        .expect("set account schedulable");

        assert_eq!(
            seen.lock().expect("read schedulable request").as_ref(),
            Some(&(ADMIN_KEY.to_string(), false))
        );
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

        let normalized = normalize_account(account, Some(&billing), None, true, &HashMap::new());

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

    #[test]
    fn available_balance_accepts_shenwenai_wallet_snapshot_aliases() {
        let result = ProbeResult {
            account_id: 1,
            snapshot: Some(ProbeSnapshot {
                status: "ok".to_string(),
                data: serde_json::json!({
                    "is_active": true,
                    "isValid": true,
                    "planName": "ShenwenAI",
                    "unit": "USD",
                    "total": 59.6667,
                    "used": 7.27338899,
                    "remaining": 52.39331101,
                    "balance": 52.39331101
                }),
                received_at: Some("2026-08-21T21:00:00+08:00".to_string()),
                fresh_until: None,
                last_error: String::new(),
            }),
            error: String::new(),
        };

        let normalized = normalize_balance(Some(&result), true, false);

        assert_eq!(normalized.state, "available");
        assert_eq!(normalized.remaining, Some(52.39331101));
        assert_eq!(normalized.unit.as_deref(), Some("USD"));
        assert_eq!(normalized.plan_name.as_deref(), Some("ShenwenAI"));
        assert_eq!(normalized.account_valid, Some(true));
        assert_eq!(normalized.account_status.as_deref(), Some("active"));
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

    #[test]
    fn upstream_usage_snapshot_parses_sub2api_payload() {
        let data = upstream_usage_snapshot(&serde_json::json!({
            "balance": 2.98696819,
            "isValid": true,
            "mode": "unrestricted",
            "planName": "sub",
            "unit": "USD",
        }))
        .expect("usable usage payload");

        assert_eq!(finite_field(&data, "remaining"), Some(2.98696819));
        assert_eq!(data["unlimited"], serde_json::json!(false));
        assert_eq!(data["account_valid"], serde_json::json!(true));
        assert_eq!(text_field(&data, "plan_name").as_deref(), Some("sub"));
        assert_eq!(text_field(&data, "unit").as_deref(), Some("USD"));
        let snapshot = ProbeSnapshot {
            status: "ok".to_string(),
            data,
            received_at: None,
            fresh_until: None,
            last_error: String::new(),
        };
        let result = ProbeResult {
            account_id: 9,
            snapshot: Some(snapshot),
            error: String::new(),
        };
        let normalized = normalize_balance(Some(&result), true, false);
        assert_eq!(normalized.state, "available");
        assert_eq!(normalized.remaining, Some(2.98696819));
        assert!(!normalized.unlimited);
        assert_eq!(normalized.plan_name.as_deref(), Some("sub"));
    }

    #[test]
    fn upstream_usage_snapshot_applies_minus_one_unlimited_rules() {
        let subscription = upstream_usage_snapshot(&serde_json::json!({
            "remaining": -1,
            "isValid": true,
            "mode": "unrestricted",
            "planName": "sub",
        }))
        .expect("subscription payload");
        assert_eq!(subscription["unlimited"], serde_json::json!(true));

        let wallet_plan = upstream_usage_snapshot(&serde_json::json!({
            "remaining": -1,
            "isValid": true,
            "mode": "unrestricted",
            "planName": "钱包余额",
        }))
        .expect("wallet plan payload");
        assert_eq!(wallet_plan["unlimited"], serde_json::json!(false));

        let overdrawn = upstream_usage_snapshot(&serde_json::json!({
            "balance": -1,
            "isValid": true,
            "mode": "unrestricted",
        }))
        .expect("wallet balance payload");
        assert_eq!(overdrawn["unlimited"], serde_json::json!(false));
    }

    #[test]
    fn upstream_usage_snapshot_rejects_unknown_modes_and_missing_evidence() {
        assert!(upstream_usage_snapshot(&serde_json::json!({
            "remaining": 5,
            "isValid": true,
            "mode": "subscription",
        }))
        .is_none());
        assert!(upstream_usage_snapshot(&serde_json::json!({ "remaining": 5 })).is_none());
        assert!(upstream_usage_snapshot(&serde_json::json!({ "mode": "unrestricted" })).is_none());
        assert!(upstream_usage_snapshot(&serde_json::json!({
            "remaining": -5,
            "isValid": true,
            "mode": "unrestricted",
        }))
        .is_none());
    }

    #[test]
    fn one_api_balance_data_derives_remaining_and_unlimited() {
        let unlimited = one_api_balance_data(100_000_000.0, Some(495.3648));
        assert_eq!(unlimited["unlimited"], serde_json::json!(true));
        assert_eq!(unlimited["unit"], serde_json::json!("USD"));

        let bounded = one_api_balance_data(20.0, Some(5.0));
        assert_eq!(finite_field(&bounded, "remaining"), Some(15.0));
        assert_eq!(bounded["unlimited"], serde_json::json!(false));

        let clamped = one_api_balance_data(2.0, Some(5.0));
        assert_eq!(finite_field(&clamped, "remaining"), Some(0.0));
    }

    #[test]
    fn match_upstream_credentials_pairs_admin_ids_with_export_entries() {
        let accounts = vec![AdminAccount {
            id: 30,
            name: "AtlasAPI".to_string(),
            platform: "openai".to_string(),
            account_type: "apikey".to_string(),
            status: "active".to_string(),
            schedulable: false,
            rate_multiplier: Some(1.0),
            credentials: serde_json::json!({
                "base_url": "https://api.aixoras.com/v1",
            }),
            extra: Value::Null,
        }];
        let export = vec![
            DataExportAccount {
                name: "AtlasAPI".to_string(),
                platform: "openai".to_string(),
                account_type: "apikey".to_string(),
                credentials: DataExportCredentials {
                    api_key: "sk-upstream".to_string(),
                    base_url: "https://api.aixoras.com/v1".to_string(),
                },
            },
            DataExportAccount {
                name: "wrong-name".to_string(),
                platform: "openai".to_string(),
                account_type: "apikey".to_string(),
                credentials: DataExportCredentials {
                    api_key: "sk-other".to_string(),
                    base_url: "https://api.aixoras.com/v1".to_string(),
                },
            },
        ];

        let matched = match_upstream_credentials(&export, &accounts);
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].0, 30);
        assert_eq!(matched[0].1.api_key, "sk-upstream");
        assert_eq!(matched[0].1.root, "https://api.aixoras.com");
    }

    #[test]
    fn balance_cache_honors_success_and_failure_ttls() {
        let root = "https://cache-ttl.test";
        let ok = ProbeResult {
            account_id: 1,
            snapshot: Some(ProbeSnapshot {
                status: "ok".to_string(),
                data: serde_json::json!({ "remaining": 1.0 }),
                received_at: None,
                fresh_until: None,
                last_error: String::new(),
            }),
            error: String::new(),
        };
        let failed = ProbeResult {
            account_id: 2,
            snapshot: Some(ProbeSnapshot {
                status: "failed".to_string(),
                data: Value::Null,
                received_at: None,
                fresh_until: None,
                last_error: String::new(),
            }),
            error: String::new(),
        };
        cache_insert_balance(root, 1, true, 1_000, &ok);
        cache_insert_balance(root, 2, false, 1_000, &failed);

        assert!(cached_balance(root, 1, 1_000 + BALANCE_CACHE_SUCCESS_TTL_MS - 1).is_some());
        assert!(cached_balance(root, 1, 1_000 + BALANCE_CACHE_SUCCESS_TTL_MS).is_none());
        assert!(cached_balance(root, 2, 1_000 + BALANCE_CACHE_FAILURE_TTL_MS - 1).is_some());
        assert!(cached_balance(root, 2, 1_000 + BALANCE_CACHE_FAILURE_TTL_MS).is_none());
        assert!(cached_balance("https://other.test", 1, 1_000).is_none());
    }

    #[tokio::test]
    async fn stock_pool_fetch_probes_upstreams_via_backup_export() {
        const ADMIN_KEY: &str = "admin-key-must-not-leak";
        const UPSTREAM_KEY: &str = "upstream-key-must-not-leak";
        let seen_authorization = Arc::new(Mutex::new(String::new()));
        let upstream_seen = seen_authorization.clone();
        let upstream = Router::new().route(
            "/v1/usage",
            get(move |headers: HeaderMap| {
                let upstream_seen = upstream_seen.clone();
                async move {
                    *upstream_seen.lock().expect("record upstream auth") = headers
                        .get("Authorization")
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string();
                    Json(serde_json::json!({
                        "balance": 2.99,
                        "isValid": true,
                        "mode": "unrestricted",
                        "planName": "sub",
                        "unit": "USD",
                    }))
                }
            }),
        );
        let upstream_address = spawn_server(upstream).await;
        let upstream_base_url = format!("http://{upstream_address}/v1");
        let export_base_url = upstream_base_url.clone();

        let sub2api = Router::new()
            .route(
                "/api/v1/admin/accounts",
                get(move || {
                    let base_url = upstream_base_url.clone();
                    async move {
                        Json(serde_json::json!({
                            "code": 0,
                            "message": "success",
                            "data": { "items": [{
                                "id": 9,
                                "name": "gateai",
                                "platform": "openai",
                                "type": "apikey",
                                "status": "active",
                                "schedulable": true,
                                "rate_multiplier": 1.0,
                                "credentials": { "base_url": base_url },
                                "extra": {},
                            }], "pages": 1 }
                        }))
                    }
                }),
            )
            .route(
                "/api/v1/admin/accounts/data",
                get(move || {
                    let base_url = export_base_url.clone();
                    async move {
                        Json(serde_json::json!({
                            "code": 0,
                            "message": "success",
                            "data": {
                                "exported_at": "2026-09-05T00:00:00Z",
                                "proxies": [],
                                "accounts": [{
                                    "name": "gateai",
                                    "platform": "openai",
                                    "type": "apikey",
                                    "credentials": {
                                        "api_key": UPSTREAM_KEY,
                                        "base_url": base_url,
                                    },
                                }],
                            }
                        }))
                    }
                }),
            );
        let address = spawn_server(sub2api).await;
        let snapshot = fetch_account_pool(
            &reqwest::Client::new(),
            &format!("http://{address}"),
            ADMIN_KEY,
            false,
        )
        .await
        .expect("account pool fetch");

        assert!(snapshot.warnings.is_empty());
        assert_eq!(snapshot.accounts.len(), 1);
        let balance = &snapshot.accounts[0].upstream_balance;
        assert_eq!(balance.state, "available");
        assert_eq!(balance.remaining, Some(2.99));
        assert!(!balance.unlimited);
        assert_eq!(balance.account_valid, Some(true));
        assert_eq!(balance.plan_name.as_deref(), Some("sub"));
        // The upstream saw only its own bearer credential, never the admin key.
        assert_eq!(
            seen_authorization.lock().expect("read upstream auth").as_str(),
            format!("Bearer {UPSTREAM_KEY}")
        );
    }

    #[tokio::test]
    async fn one_api_upstream_falls_back_to_the_billing_pair() {
        let upstream = Router::new()
            .route(
                "/v1/usage",
                get(|| async { (StatusCode::NOT_FOUND, "404 page not found") }),
            )
            .route(
                "/v1/dashboard/billing/subscription",
                get(|| async {
                    Json(serde_json::json!({
                        "object": "billing_subscription",
                        "hard_limit_usd": 20.0,
                    }))
                }),
            )
            .route(
                "/v1/dashboard/billing/usage",
                get(|| async {
                    Json(serde_json::json!({ "object": "list", "total_usage": 500.0 }))
                }),
            );
        let upstream_address = spawn_server(upstream).await;

        let sub2api = Router::new()
            .route(
                "/api/v1/admin/accounts",
                get(move || {
                    let base_url = format!("http://{upstream_address}");
                    async move {
                        Json(serde_json::json!({
                            "code": 0,
                            "message": "success",
                            "data": { "items": [{
                                "id": 4,
                                "name": "aixoras",
                                "platform": "openai",
                                "type": "apikey",
                                "status": "active",
                                "schedulable": true,
                                "rate_multiplier": 1.0,
                                "credentials": { "base_url": base_url },
                                "extra": {},
                            }], "pages": 1 }
                        }))
                    }
                }),
            )
            .route(
                "/api/v1/admin/accounts/data",
                get(move || {
                    let base_url = format!("http://{upstream_address}");
                    async move {
                        Json(serde_json::json!({
                            "code": 0,
                            "message": "success",
                            "data": {
                                "exported_at": "2026-09-05T00:00:00Z",
                                "proxies": [],
                                "accounts": [{
                                    "name": "aixoras",
                                    "platform": "openai",
                                    "type": "apikey",
                                    "credentials": {
                                        "api_key": "sk-one-api",
                                        "base_url": base_url,
                                    },
                                }],
                            }
                        }))
                    }
                }),
            );
        let address = spawn_server(sub2api).await;
        let snapshot = fetch_account_pool(
            &reqwest::Client::new(),
            &format!("http://{address}"),
            "admin-key",
            false,
        )
        .await
        .expect("account pool fetch");

        let balance = &snapshot.accounts[0].upstream_balance;
        assert_eq!(balance.state, "available");
        assert_eq!(balance.remaining, Some(15.0));
        assert!(!balance.unlimited);
        assert_eq!(balance.unit.as_deref(), Some("USD"));
    }


    #[test]
    fn clean_site_name_rejects_empty_template_and_oversized_names() {
        assert_eq!(
            clean_site_name("FastAI 模型"),
            Some("FastAI 模型".to_string())
        );
        assert_eq!(clean_site_name("  AtlasAPI  "), Some("AtlasAPI".to_string()));
        assert_eq!(clean_site_name(""), None);
        assert_eq!(clean_site_name("AI Gateway"), None);
        assert_eq!(clean_site_name("Sub2API"), None);
        assert_eq!(clean_site_name(&"长".repeat(SITE_NAME_MAX_LEN + 1)), None);
    }

    #[test]
    fn site_name_parsers_read_both_public_protocols() {
        let public = serde_json::json!({ "site_name": "FastAI 模型" });
        assert_eq!(
            parse_public_site_name(&public),
            Some("FastAI 模型".to_string())
        );

        let status = serde_json::json!({
            "data": { "system_name": "JuAI API" }
        });
        assert_eq!(parse_status_site_name(&status), Some("JuAI API".to_string()));

        assert_eq!(
            parse_public_site_name(&serde_json::json!({ "site_name": "Sub2API" })),
            None
        );
        assert_eq!(parse_status_site_name(&serde_json::json!({ "data": {} })), None);
    }

    #[tokio::test]
    async fn pool_accounts_carry_self_reported_site_names() {
        let sub2api_style = Router::new().route(
            "/api/v1/settings/public",
            get(|| async {
                Json(serde_json::json!({
                    "code": 0,
                    "message": "success",
                    "data": { "site_name": "FastAI 模型" }
                }))
            }),
        );
        let sub2api_address = spawn_server(sub2api_style).await;

        let one_api_style = Router::new()
            .route(
                "/api/v1/settings/public",
                get(|| async {
                    Json(serde_json::json!({
                        "code": 0,
                        "message": "success",
                        "data": { "site_name": "AI Gateway" }
                    }))
                }),
            )
            .route(
                "/api/status",
                get(|| async {
                    Json(serde_json::json!({
                        "data": { "system_name": "AI Gateway" }
                    }))
                }),
            );
        let one_api_address = spawn_server(one_api_style).await;

        let accounts = Router::new().route(
            "/api/v1/admin/accounts",
            get(move || {
                let sub2api_url = format!("http://{sub2api_address}");
                let one_api_url = format!("http://{one_api_address}");
                async move {
                    Json(serde_json::json!({
                        "code": 0,
                        "message": "success",
                        "data": { "items": [
                            {
                                "id": 1,
                                "name": "customized",
                                "platform": "openai",
                                "type": "apikey",
                                "credentials": { "base_url": sub2api_url }
                            },
                            {
                                "id": 2,
                                "name": "template",
                                "platform": "openai",
                                "type": "apikey",
                                "credentials": { "base_url": one_api_url }
                            }
                        ], "pages": 1 }
                    }))
                }
            }),
        );
        let address = spawn_server(accounts).await;

        let snapshot = fetch_account_pool(
            &reqwest::Client::new(),
            &format!("http://{address}"),
            "admin-key",
            false,
        )
        .await
        .expect("account pool fetch");

        assert_eq!(snapshot.accounts.len(), 2);
        assert_eq!(
            snapshot.accounts[0].site_name.as_deref(),
            Some("FastAI 模型")
        );
        // The second site only reports the template default, so no name is
        // published and the GUI keeps labeling it by its domain.
        assert_eq!(snapshot.accounts[1].site_name, None);
    }
}
