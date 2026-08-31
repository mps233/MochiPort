//! API Key 级别的余额与计费倍率探测。
//!
//! 目前支持 Sub2API 和 New API。调用方只会拿到结构化结果和脱敏状态，
//! 上游响应正文、请求 URL 与 Bearer 凭据均不会进入返回值。

use std::time::Duration;

use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::config::provider_api_root;

pub const PROVIDER_USAGE_FETCH_TIMEOUT: Duration = Duration::from_secs(15);
const PROVIDER_USAGE_RESPONSE_LIMIT: usize = 256 * 1024;
// New API reports quota points. Its public conversion is 500,000 points per
// USD, so normalize the value before handing it to the rest of the app.
const NEW_API_QUOTA_POINTS_PER_USD: f64 = 500_000.0;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityStatus {
    Available,
    Unsupported,
    Unauthorized,
    Forbidden,
    TemporarilyUnavailable,
    InvalidResponse,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsageSnapshot {
    pub source: &'static str,
    pub balance_status: CapabilityStatus,
    pub billing_status: CapabilityStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining: Option<f64>,
    pub unlimited: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub balance_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_valid: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub today_cost: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub today_actual_cost: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_rate_multiplier: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_rate_multiplier: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_rate_multiplier: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_rate_multiplier: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peak_rate_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peak_start: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peak_end: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peak_rate_multiplier: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub applied_peak_multiplier: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<String>,
}

impl ProviderUsageSnapshot {
    pub fn any_available(&self) -> bool {
        self.balance_status == CapabilityStatus::Available
            || self.billing_status == CapabilityStatus::Available
    }
}

#[derive(Debug)]
struct BalanceInfo {
    remaining: Option<f64>,
    unlimited: bool,
    unit: String,
    mode: Option<String>,
    plan_name: Option<String>,
    account_valid: Option<bool>,
    account_status: Option<String>,
    today_cost: Option<f64>,
    today_actual_cost: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct BillingInfo {
    object: String,
    schema_version: u32,
    billing_scope: String,
    group_rate_multiplier: f64,
    user_rate_multiplier: Option<f64>,
    resolved_rate_multiplier: f64,
    peak_rate_enabled: bool,
    peak_start: Option<String>,
    peak_end: Option<String>,
    peak_rate_multiplier: Option<f64>,
    applied_peak_multiplier: Option<f64>,
    effective_rate_multiplier: f64,
    timezone: Option<String>,
    observed_at: String,
}

enum Probe<T> {
    Available(T),
    Unavailable(CapabilityStatus),
}

impl<T> Probe<T> {
    fn status(&self) -> CapabilityStatus {
        match self {
            Self::Available(_) => CapabilityStatus::Available,
            Self::Unavailable(status) => *status,
        }
    }
}

pub async fn fetch_provider_usage(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
) -> ProviderUsageSnapshot {
    let root = provider_api_root(base_url);
    let usage_url = format!("{root}/v1/usage");
    let billing_url = format!("{root}/v1/sub2api/billing");
    let new_api_usage_url = format!("{root}/api/usage/token");
    let (balance, billing, new_api_balance) = tokio::join!(
        fetch_balance(client, &usage_url, api_key),
        fetch_billing(client, &billing_url, api_key),
        fetch_new_api_balance(client, &new_api_usage_url, api_key),
    );

    // Prefer the established Sub2API snapshot when it is available. New API
    // is a fallback for gateways whose compatible `/v1/usage` endpoint is not
    // exposed. Do not merge the two protocols: their quota units and billing
    // semantics are different.
    let use_new_api = matches!(&balance, Probe::Unavailable(_))
        && matches!(&new_api_balance, Probe::Available(_));
    let mut snapshot = ProviderUsageSnapshot {
        source: if use_new_api { "new_api" } else { "sub2api" },
        balance_status: balance.status(),
        billing_status: billing.status(),
        remaining: None,
        unlimited: false,
        unit: None,
        balance_mode: None,
        plan_name: None,
        account_valid: None,
        account_status: None,
        today_cost: None,
        today_actual_cost: None,
        group_rate_multiplier: None,
        user_rate_multiplier: None,
        resolved_rate_multiplier: None,
        effective_rate_multiplier: None,
        peak_rate_enabled: None,
        peak_start: None,
        peak_end: None,
        peak_rate_multiplier: None,
        applied_peak_multiplier: None,
        timezone: None,
        observed_at: None,
    };

    if use_new_api {
        snapshot.balance_status = CapabilityStatus::Available;
        // New API exposes token quota but has no equivalent current-rate
        // endpoint. Keep this explicit so the UI can say "未提供".
        snapshot.billing_status = CapabilityStatus::Unsupported;
        if let Probe::Available(balance) = new_api_balance {
            snapshot.remaining = balance.remaining;
            snapshot.unlimited = balance.unlimited;
            snapshot.unit = Some(balance.unit);
            snapshot.balance_mode = balance.mode;
            snapshot.plan_name = balance.plan_name;
            snapshot.account_valid = balance.account_valid;
            snapshot.account_status = balance.account_status;
            snapshot.today_cost = balance.today_cost;
            snapshot.today_actual_cost = balance.today_actual_cost;
        }
    } else if let Probe::Available(balance) = balance {
        snapshot.remaining = balance.remaining;
        snapshot.unlimited = balance.unlimited;
        snapshot.unit = Some(balance.unit);
        snapshot.balance_mode = balance.mode;
        snapshot.plan_name = balance.plan_name;
        snapshot.account_valid = balance.account_valid;
        snapshot.account_status = balance.account_status;
        snapshot.today_cost = balance.today_cost;
        snapshot.today_actual_cost = balance.today_actual_cost;
    }
    if !use_new_api && let Probe::Available(billing) = billing {
        snapshot.group_rate_multiplier = Some(billing.group_rate_multiplier);
        snapshot.user_rate_multiplier = billing.user_rate_multiplier;
        snapshot.resolved_rate_multiplier = Some(billing.resolved_rate_multiplier);
        snapshot.effective_rate_multiplier = Some(billing.effective_rate_multiplier);
        snapshot.peak_rate_enabled = Some(billing.peak_rate_enabled);
        snapshot.peak_start = billing.peak_start;
        snapshot.peak_end = billing.peak_end;
        snapshot.peak_rate_multiplier = billing.peak_rate_multiplier;
        snapshot.applied_peak_multiplier = billing.applied_peak_multiplier;
        snapshot.timezone = billing.timezone;
        snapshot.observed_at = Some(billing.observed_at);
    }
    snapshot
}

async fn fetch_balance(client: &reqwest::Client, url: &str, api_key: &str) -> Probe<BalanceInfo> {
    let value = match fetch_json(client, url, api_key).await {
        Ok(value) => value,
        Err(status) => return Probe::Unavailable(status),
    };
    match parse_balance(&value) {
        Some(balance) => Probe::Available(balance),
        None => Probe::Unavailable(CapabilityStatus::InvalidResponse),
    }
}

async fn fetch_new_api_balance(
    client: &reqwest::Client,
    url: &str,
    api_key: &str,
) -> Probe<BalanceInfo> {
    let value = match fetch_json(client, url, api_key).await {
        Ok(value) => value,
        Err(status) => return Probe::Unavailable(status),
    };
    match parse_new_api_balance(&value) {
        Some(balance) => Probe::Available(balance),
        None => Probe::Unavailable(CapabilityStatus::InvalidResponse),
    }
}

async fn fetch_billing(client: &reqwest::Client, url: &str, api_key: &str) -> Probe<BillingInfo> {
    let value = match fetch_json(client, url, api_key).await {
        Ok(value) => value,
        Err(status) => return Probe::Unavailable(status),
    };
    match serde_json::from_value::<BillingInfo>(value) {
        Ok(info) if valid_billing(&info) => Probe::Available(info),
        _ => Probe::Unavailable(CapabilityStatus::InvalidResponse),
    }
}

async fn fetch_json(
    client: &reqwest::Client,
    url: &str,
    api_key: &str,
) -> Result<Value, CapabilityStatus> {
    let response = client
        .get(url)
        .timeout(PROVIDER_USAGE_FETCH_TIMEOUT)
        // `bearer_auth` marks the header as sensitive in reqwest's request
        // representation in addition to keeping it out of our own result.
        .bearer_auth(api_key.trim())
        .send()
        .await
        .map_err(|_| CapabilityStatus::TemporarilyUnavailable)?;
    let status = response.status();
    if !status.is_success() {
        return Err(classify_http_status(status));
    }
    let mut response = response;
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| CapabilityStatus::TemporarilyUnavailable)?
    {
        if body.len().saturating_add(chunk.len()) > PROVIDER_USAGE_RESPONSE_LIMIT {
            return Err(CapabilityStatus::InvalidResponse);
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| CapabilityStatus::InvalidResponse)
}

fn classify_http_status(status: StatusCode) -> CapabilityStatus {
    match status {
        StatusCode::UNAUTHORIZED => CapabilityStatus::Unauthorized,
        StatusCode::FORBIDDEN => CapabilityStatus::Forbidden,
        StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED => CapabilityStatus::Unsupported,
        status if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() => {
            CapabilityStatus::TemporarilyUnavailable
        }
        _ => CapabilityStatus::InvalidResponse,
    }
}

fn parse_balance(value: &Value) -> Option<BalanceInfo> {
    let optional_string = |key: &str, max_chars: usize| {
        value
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .filter(|text| text.chars().count() <= max_chars)
            .map(str::to_string)
    };
    let mode = optional_string("mode", 64);
    // Some compatible gateways expose a wallet snapshot without Sub2API's
    // optional `mode` field. When present, keep validating the known values;
    // when absent, the finite balance fields below are sufficient evidence.
    if mode
        .as_deref()
        .is_some_and(|mode| !matches!(mode, "quota_limited" | "unrestricted"))
    {
        return None;
    }
    let reported_valid = value
        .get("isValid")
        .and_then(Value::as_bool)
        .or_else(|| value.get("is_active").and_then(Value::as_bool))?;

    let wallet_balance = value.get("balance").and_then(Value::as_f64);
    let raw_remaining = wallet_balance.or_else(|| {
        value
            .get("remaining")
            .and_then(Value::as_f64)
            .or_else(|| value.pointer("/quota/remaining").and_then(Value::as_f64))
    });
    if mode.is_none() && raw_remaining.is_none() {
        return None;
    }
    if raw_remaining
        .is_some_and(|amount| !amount.is_finite() || (wallet_balance.is_none() && amount < -1.0))
    {
        return None;
    }
    // Sub2API uses `remaining: -1` for an unlimited subscription, while a
    // wallet can legitimately expose the same (or a lower) value via
    // `balance` after overdrawing. Only the former means unlimited.
    let unlimited = wallet_balance.is_none() && raw_remaining == Some(-1.0);
    let unit = value
        .get("unit")
        .and_then(Value::as_str)
        .or_else(|| value.pointer("/quota/unit").and_then(Value::as_str))
        .unwrap_or("USD")
        .trim();
    if unit.is_empty() || unit.chars().count() > 16 {
        return None;
    }
    let account_status = optional_string("status", 32).or_else(|| {
        value
            .get("is_active")
            .and_then(Value::as_bool)
            .map(|active| if active { "active" } else { "inactive" }.to_string())
    });
    let account_valid = match account_status.as_deref() {
        Some("active") => Some(reported_valid),
        Some("disabled" | "inactive" | "quota_exhausted" | "expired") => Some(false),
        _ => Some(reported_valid),
    };
    let today_cost = value
        .pointer("/usage/today/cost")
        .and_then(Value::as_f64)
        .filter(|amount| amount.is_finite() && *amount >= 0.0);
    let today_actual_cost = value
        .pointer("/usage/today/actual_cost")
        .and_then(Value::as_f64)
        .filter(|amount| amount.is_finite() && *amount >= 0.0);
    Some(BalanceInfo {
        remaining: raw_remaining.filter(|_| !unlimited),
        unlimited,
        unit: unit.to_string(),
        mode,
        plan_name: optional_string("planName", 120),
        account_valid,
        account_status,
        today_cost,
        today_actual_cost,
    })
}

fn parse_new_api_balance(value: &Value) -> Option<BalanceInfo> {
    let data = value.get("data")?;
    let unlimited = data.get("unlimited_quota").and_then(Value::as_bool)?;
    let available_points = data.get("total_available").and_then(json_f64);
    let remaining = if unlimited {
        None
    } else {
        let points = available_points?;
        if points < 0.0 {
            return None;
        }
        Some(points / NEW_API_QUOTA_POINTS_PER_USD)
    };
    let plan_name = data
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty() && name.chars().count() <= 120)
        .map(str::to_string);

    Some(BalanceInfo {
        remaining,
        unlimited,
        unit: "USD".to_string(),
        mode: Some(if unlimited {
            "unrestricted".to_string()
        } else {
            "quota_limited".to_string()
        }),
        plan_name,
        account_valid: Some(true),
        account_status: Some("active".to_string()),
        today_cost: None,
        today_actual_cost: None,
    })
}

fn json_f64(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.trim().parse::<f64>().ok())
        .filter(|value| value.is_finite())
}

fn valid_billing(info: &BillingInfo) -> bool {
    let strings_valid = !info.observed_at.trim().is_empty()
        && info.observed_at.chars().count() <= 128
        && info
            .peak_start
            .as_deref()
            .is_none_or(|value| !value.trim().is_empty() && value.chars().count() <= 32)
        && info
            .peak_end
            .as_deref()
            .is_none_or(|value| !value.trim().is_empty() && value.chars().count() <= 32)
        && info
            .timezone
            .as_deref()
            .is_none_or(|value| !value.trim().is_empty() && value.chars().count() <= 128);
    let peak_fields_valid = if info.peak_rate_enabled {
        info.peak_start.is_some()
            && info.peak_end.is_some()
            && info.peak_rate_multiplier.is_some()
            && info.applied_peak_multiplier.is_some()
            && info.timezone.is_some()
    } else {
        true
    };
    let applied_peak = info.applied_peak_multiplier.unwrap_or(1.0);
    let expected_effective = info.resolved_rate_multiplier * applied_peak;
    let tolerance = expected_effective
        .abs()
        .max(info.effective_rate_multiplier.abs())
        .mul_add(1e-9, 1e-12);
    let effective_matches = expected_effective.is_finite()
        && (expected_effective - info.effective_rate_multiplier).abs() <= tolerance;

    info.object == "sub2api.key_billing"
        && info.schema_version == 1
        && info.billing_scope == "token"
        && strings_valid
        && peak_fields_valid
        && effective_matches
        && [
            info.group_rate_multiplier,
            info.resolved_rate_multiplier,
            info.effective_rate_multiplier,
        ]
        .into_iter()
        .chain(info.user_rate_multiplier)
        .chain(info.peak_rate_multiplier)
        .chain(info.applied_peak_multiplier)
        .all(|value| value.is_finite() && value >= 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn balance_parses_wallet_quota_and_unlimited_shapes() {
        let wallet = parse_balance(&json!({
            "mode": "unrestricted",
            "remaining": 12.5,
            "balance": 12.5,
            "unit": "USD",
            "planName": "钱包余额",
            "isValid": true,
            "usage": { "today": { "cost": 0.12, "actual_cost": 0.09 } }
        }))
        .expect("wallet balance");
        assert_eq!(wallet.remaining, Some(12.5));
        assert!(!wallet.unlimited);
        assert_eq!(wallet.mode.as_deref(), Some("unrestricted"));
        assert_eq!(wallet.today_cost, Some(0.12));
        assert_eq!(wallet.today_actual_cost, Some(0.09));

        let quota = parse_balance(&json!({
            "mode": "quota_limited",
            "isValid": true,
            "quota": { "remaining": 8.25, "unit": "CNY" }
        }))
        .expect("quota balance");
        assert_eq!(quota.remaining, Some(8.25));
        assert_eq!(quota.unit, "CNY");

        let unlimited = parse_balance(&json!({
            "mode": "unrestricted",
            "isValid": true,
            "remaining": -1,
            "unit": "USD"
        }))
        .expect("unlimited balance");
        assert!(unlimited.unlimited);
        assert_eq!(unlimited.remaining, None);

        let rate_only = parse_balance(&json!({
            "mode": "quota_limited",
            "isValid": true,
            "status": "active",
            "rate_limits": [{ "window": "5h", "remaining": 100 }]
        }))
        .expect("rate-only key");
        assert_eq!(rate_only.remaining, None);
        assert_eq!(rate_only.account_valid, Some(true));
        assert_eq!(rate_only.account_status.as_deref(), Some("active"));

        let subscription_without_snapshot = parse_balance(&json!({
            "mode": "unrestricted",
            "isValid": true,
            "unit": "USD",
            "planName": "Subscription"
        }))
        .expect("subscription without remaining snapshot");
        assert_eq!(subscription_without_snapshot.remaining, None);

        let overdrawn_wallet = parse_balance(&json!({
            "mode": "unrestricted",
            "isValid": true,
            "remaining": -2.5,
            "balance": -2.5,
            "unit": "USD",
            "planName": "钱包余额"
        }))
        .expect("overdrawn wallet");
        assert_eq!(overdrawn_wallet.remaining, Some(-2.5));
        assert!(!overdrawn_wallet.unlimited);
    }

    #[test]
    fn balance_parses_shenwenai_wallet_snapshot_without_mode() {
        let balance = parse_balance(&json!({
            "is_active": true,
            "isValid": true,
            "planName": "ShenwenAI",
            "unit": "USD",
            "total": 59.6667,
            "used": 7.27338899,
            "remaining": 52.39331101,
            "balance": 52.39331101
        }))
        .expect("ShenwenAI wallet balance");

        assert_eq!(balance.remaining, Some(52.39331101));
        assert!(!balance.unlimited);
        assert_eq!(balance.unit, "USD");
        assert_eq!(balance.mode, None);
        assert_eq!(balance.plan_name.as_deref(), Some("ShenwenAI"));
        assert_eq!(balance.account_valid, Some(true));
        assert_eq!(balance.account_status.as_deref(), Some("active"));
    }

    #[test]
    fn new_api_usage_converts_quota_points_to_usd() {
        let balance = parse_new_api_balance(&json!({
            "data": {
                "total_granted": 2_000_000,
                "total_used": 750_000,
                "total_available": 1_250_000,
                "unlimited_quota": false,
                "name": "New API Key"
            }
        }))
        .expect("New API usage");

        assert_eq!(balance.remaining, Some(2.5));
        assert!(!balance.unlimited);
        assert_eq!(balance.unit, "USD");
        assert_eq!(balance.mode.as_deref(), Some("quota_limited"));
        assert_eq!(balance.plan_name.as_deref(), Some("New API Key"));
        assert_eq!(balance.account_valid, Some(true));
        assert_eq!(balance.account_status.as_deref(), Some("active"));

        let unlimited = parse_new_api_balance(&json!({
            "data": {
                "unlimited_quota": true,
                "name": "Unlimited"
            }
        }))
        .expect("unlimited New API usage");
        assert!(unlimited.unlimited);
        assert_eq!(unlimited.remaining, None);
    }

    #[test]
    fn new_api_usage_rejects_missing_or_negative_quota() {
        assert!(
            parse_new_api_balance(&json!({
                "data": { "unlimited_quota": false }
            }))
            .is_none()
        );
        assert!(
            parse_new_api_balance(&json!({
                "data": { "unlimited_quota": false, "total_available": -1 }
            }))
            .is_none()
        );
        assert!(
            parse_new_api_balance(&json!({
                "data": { "unlimited_quota": "false", "total_available": 1 }
            }))
            .is_none()
        );
    }

    #[test]
    fn balance_rejects_missing_or_invalid_amounts() {
        assert!(parse_balance(&json!({ "unit": "USD" })).is_none());
        assert!(parse_balance(&json!({ "remaining": -2, "unit": "USD" })).is_none());
        assert!(parse_balance(&json!({ "remaining": 1, "unit": "" })).is_none());
        assert!(parse_balance(&json!({ "mode": "future", "remaining": 1 })).is_none());
    }

    #[test]
    fn balance_normalizes_non_active_key_statuses() {
        for status in ["disabled", "inactive", "quota_exhausted", "expired"] {
            let balance = parse_balance(&json!({
                "mode": "quota_limited",
                "isValid": true,
                "status": status,
                "remaining": 0,
                "unit": "USD"
            }))
            .expect("known key status");
            assert_eq!(balance.account_status.as_deref(), Some(status));
            assert_eq!(balance.account_valid, Some(false));
        }
    }

    #[test]
    fn billing_requires_supported_sub2api_token_schema() {
        let value = json!({
            "object": "sub2api.key_billing",
            "schema_version": 1,
            "billing_scope": "token",
            "group_rate_multiplier": 0.8,
            "resolved_rate_multiplier": 0.6,
            "peak_rate_enabled": true,
            "peak_start": "08:00",
            "peak_end": "12:00",
            "peak_rate_multiplier": 1.5,
            "applied_peak_multiplier": 1.5,
            "effective_rate_multiplier": 0.9,
            "timezone": "Asia/Shanghai",
            "observed_at": "2026-08-15T00:00:00Z",
            "future_field": true
        });
        let info: BillingInfo = serde_json::from_value(value.clone()).expect("billing info");
        assert!(valid_billing(&info));

        let mut unsupported = value;
        unsupported["schema_version"] = json!(2);
        let info: BillingInfo = serde_json::from_value(unsupported).expect("future schema");
        assert!(!valid_billing(&info));
    }

    #[test]
    fn http_statuses_map_to_stable_capability_states() {
        assert_eq!(
            classify_http_status(StatusCode::UNAUTHORIZED),
            CapabilityStatus::Unauthorized
        );
        assert_eq!(
            classify_http_status(StatusCode::FORBIDDEN),
            CapabilityStatus::Forbidden
        );
        assert_eq!(
            classify_http_status(StatusCode::NOT_FOUND),
            CapabilityStatus::Unsupported
        );
        assert_eq!(
            classify_http_status(StatusCode::SERVICE_UNAVAILABLE),
            CapabilityStatus::TemporarilyUnavailable
        );
    }
}
