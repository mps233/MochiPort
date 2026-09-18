//! Management API: local Codex usage.
//!
//! Serves the usage the daemon derives from the local Codex rollout logs. The
//! clients display this instead of parsing those logs themselves; see
//! docs/codex-usage-unification.zh-CN.md.

use axum::{
    Json,
    extract::{Query, State},
    response::IntoResponse,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::app_state::SharedState;
use crate::usage::{
    log::TokenUsage,
    service::{self, Summary},
};

/// Token counters for one day or for the whole window.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct UsageTotals {
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub cache_write_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_output_tokens: u64,
    pub total_tokens: u64,
}

/// Usage for one calendar day.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct UsageDayEntry {
    pub date: String,
    pub sessions: u32,
    pub totals: UsageTotals,
}

/// Usage for one hour of one day, for the recent-activity series.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct UsageHourEntry {
    pub date: String,
    pub hour: u8,
    pub sessions: u32,
    pub totals: UsageTotals,
}

/// Usage attributed to one project.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct UsageProjectEntry {
    pub project: String,
    pub sessions: u32,
    pub totals: UsageTotals,
}

/// Usage attributed to one model.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct UsageModelEntry {
    pub model: String,
    pub sessions: u32,
    pub totals: UsageTotals,
    /// API-equivalent estimate in US dollars. Mirrors the client calculation it
    /// replaces: cached input is billed once, at the cached rate.
    pub cost_usd: f64,
}

/// Usage attributed to one provider.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct UsageProviderEntry {
    pub provider: String,
    pub sessions: u32,
    pub totals: UsageTotals,
}

/// One row of the cross-product breakdown, keyed the same way as the client's
/// history database: `(day, service, source, model, project)`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct UsageBreakdownRow {
    pub day: String,
    pub service: String,
    pub source: String,
    pub model: String,
    pub project: String,
    pub totals: UsageTotals,
    /// Cost for this row, priced with its own model.
    pub cost_usd: f64,
}

/// One quota window as reported by Codex.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct UsageQuotaWindow {
    /// Percentage of the window already consumed.
    pub used_percent: f64,
    /// Window length in minutes, as reported by Codex. Callers should classify
    /// windows by this rather than by `primary`/`secondary` position: measured
    /// records carry a 30-day window under `primary`.
    pub window_minutes: Option<u32>,
    /// Absolute reset time, in milliseconds since the Unix epoch. Absent when
    /// Codex reported a relative reset without a usable record timestamp.
    pub resets_at_ms: Option<i64>,
}

/// Latest quota windows. Absent entirely when the logs carried none, which is
/// the normal state for sessions routed through the AI gateway.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct UsageQuotaSnapshot {
    /// The record's `primary` window. Classify by `windowMinutes`, not by this
    /// position: measured records carry a 30-day window here.
    pub primary: Option<UsageQuotaWindow>,
    /// The record's `secondary` window. Usually absent; classify by
    /// `windowMinutes` for the same reason as `primary`.
    pub secondary: Option<UsageQuotaWindow>,
    pub plan_type: Option<String>,
    pub observed_at_ms: Option<i64>,
}

/// One minute-level quota reading.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct UsageQuotaPoint {
    /// Minutes since the Unix epoch.
    pub minute: i64,
    /// `primary` or `secondary`, i.e. the record position.
    pub kind: String,
    pub used_percent: f64,
}

/// One day's last observed percentage for one quota window.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct UsageQuotaHistoryPoint {
    pub day: String,
    /// `primary` or `secondary`, i.e. the record position rather than a fixed
    /// duration (measured records carry a 30-day window under `primary`).
    pub kind: String,
    pub used_percent: f64,
}

/// Token totals for one minute of day.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct UsageMinuteBucket {
    /// Minutes since the Unix epoch, so a 24-hour window can cross midnight.
    pub minute: i64,
    pub totals: UsageTotals,
}

/// Local Codex usage across the requested window.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct UsageSummaryResponse {
    /// Milliseconds since the Unix epoch, so clients can label their snapshot.
    pub generated_at_ms: u64,
    pub window_days: u32,
    pub sessions: u32,
    pub totals: UsageTotals,
    // These collections grow as the daemon learns new dimensions. `serde(default)`
    // keeps a newer client able to decode an older daemon's response instead of
    // failing on a missing key.
    /// Newest day first; empty when no local Codex data was found.
    #[serde(default)]
    pub days: Vec<UsageDayEntry>,
    /// Newest hour first, for the recent-activity series.
    #[serde(default)]
    pub hours: Vec<UsageHourEntry>,
    /// Highest usage first; sessions without a recorded working directory are
    /// omitted rather than bucketed as "unknown".
    #[serde(default)]
    pub projects: Vec<UsageProjectEntry>,
    /// Highest usage first. A session that switched models contributes to each
    /// model it actually used.
    #[serde(default)]
    pub models: Vec<UsageModelEntry>,
    /// Highest usage first.
    #[serde(default)]
    pub providers: Vec<UsageProviderEntry>,
    /// Cross-product rows, ordered by day, model, source, then project.
    #[serde(default)]
    pub breakdown: Vec<UsageBreakdownRow>,
    /// Estimated cost across the window, in US dollars. Summed per model so each
    /// model's own rate applies.
    ///
    /// `serde(default)` like the collections above: this field was added after the
    /// first release of the endpoint, and a newer client must still be able to
    /// decode an older daemon's response.
    #[serde(default)]
    pub estimated_cost_usd: f64,
    /// Latest quota windows, or absent when the logs contained none.
    #[serde(default)]
    pub quota: Option<UsageQuotaSnapshot>,
    /// Per-minute totals over the trailing window the clients use (48 hours at
    /// most), oldest minute first. Clients derive live rates (`tokensPerMinute`,
    /// baseline) from these.
    #[serde(default)]
    pub minutes: Vec<UsageMinuteBucket>,
    /// Last quota reading per day and window, oldest day first. This is the
    /// history clients persist to estimate weekly depletion.
    #[serde(default)]
    pub quota_history: Vec<UsageQuotaHistoryPoint>,
    /// Minute-level quota readings over the trailing hour, oldest first. Clients
    /// estimate short-horizon depletion from this series.
    #[serde(default)]
    pub quota_minutes: Vec<UsageQuotaPoint>,
    /// `None` when this machine has no local Codex installation to read.
    pub sessions_root: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct UsageSummaryQuery {
    /// Window size in days; clamped to the supported range.
    pub days: Option<u32>,
}

/// Full-history usage, for clients rebuilding their local history database.
///
/// Separate from `/usage/summary` because it intentionally ignores the
/// interactive window cap: a rebuild must see every day on disk, since the
/// client replaces rows rather than merging them.
pub(super) async fn usage_history(
    State(_state): State<SharedState>,
    Query(query): Query<UsageHistoryQuery>,
) -> impl IntoResponse {
    Json(snapshot(service::history_summary(query.days)))
}

#[derive(Debug, Deserialize)]
pub(super) struct UsageHistoryQuery {
    /// Optional window; omit for everything on disk.
    pub days: Option<u32>,
}

pub(super) async fn usage_summary(
    State(_state): State<SharedState>,
    Query(query): Query<UsageSummaryQuery>,
) -> impl IntoResponse {
    let window = service::normalize_window_days(query.days);
    Json(snapshot(service::summary(window)))
}

fn quota_window(window: crate::usage::log::RateLimitWindow) -> UsageQuotaWindow {
    UsageQuotaWindow {
        used_percent: window.used_percent,
        window_minutes: window.window_minutes,
        resets_at_ms: window.resets_at_ms,
    }
}

fn totals(usage: TokenUsage) -> UsageTotals {
    UsageTotals {
        input_tokens: usage.input_tokens,
        cached_input_tokens: usage.cached_input_tokens,
        cache_write_input_tokens: usage.cache_write_input_tokens,
        output_tokens: usage.output_tokens,
        reasoning_output_tokens: usage.reasoning_output_tokens,
        total_tokens: usage.total_tokens,
    }
}

fn snapshot(summary: Summary) -> UsageSummaryResponse {
    // Filled in while the per-model entries are built.
    let estimated_cost = summary.models.iter().map(|model| model.cost_usd).sum();
    UsageSummaryResponse {
        generated_at_ms: summary.generated_at_ms,
        window_days: summary.window_days,
        sessions: summary.sessions,
        totals: totals(summary.usage),
        days: summary
            .days
            .into_iter()
            .map(|day| UsageDayEntry {
                date: day.date,
                sessions: day.sessions,
                totals: totals(day.usage),
            })
            .collect(),
        hours: summary
            .hours
            .into_iter()
            .map(|hour| UsageHourEntry {
                date: hour.date,
                hour: hour.hour,
                sessions: hour.sessions,
                totals: totals(hour.usage),
            })
            .collect(),
        projects: summary
            .projects
            .into_iter()
            .map(|project| UsageProjectEntry {
                project: project.project,
                sessions: project.sessions,
                totals: totals(project.usage),
            })
            .collect(),
        models: {
            let models: Vec<UsageModelEntry> = summary
                .models
                .into_iter()
                .map(|model| UsageModelEntry {
                    model: model.model,
                    sessions: model.sessions,
                    totals: totals(model.usage),
                    cost_usd: model.cost_usd,
                })
                .collect();
            models
        },
        providers: summary
            .providers
            .into_iter()
            .map(|provider| UsageProviderEntry {
                provider: provider.provider,
                sessions: provider.sessions,
                totals: totals(provider.usage),
            })
            .collect(),
        estimated_cost_usd: estimated_cost,
        minutes: summary
            .minutes
            .into_iter()
            .map(|bucket| UsageMinuteBucket {
                minute: bucket.minute,
                totals: totals(bucket.usage),
            })
            .collect(),
        quota_history: summary
            .quota_history
            .into_iter()
            .map(|point| UsageQuotaHistoryPoint {
                day: point.day,
                kind: point.kind.to_owned(),
                used_percent: point.used_percent,
            })
            .collect(),
        quota_minutes: summary
            .quota_minutes
            .into_iter()
            .map(|point| UsageQuotaPoint {
                minute: point.minute,
                kind: point.kind.to_owned(),
                used_percent: point.used_percent,
            })
            .collect(),
        quota: summary.quota.map(|quota| UsageQuotaSnapshot {
            primary: quota.primary.map(quota_window),
            secondary: quota.secondary.map(quota_window),
            plan_type: quota.plan_type,
            observed_at_ms: quota.observed_at_ms,
        }),
        breakdown: summary
            .breakdown
            .into_iter()
            .map(|row| UsageBreakdownRow {
                day: row.day,
                service: row.service,
                source: row.source,
                model: row.model,
                project: row.project,
                totals: totals(row.usage),
                cost_usd: row.cost_usd,
            })
            .collect(),
        sessions_root: summary.sessions_root,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_maps_totals_and_days() {
        let summary = Summary {
            window_days: 7,
            sessions: 2,
            usage: TokenUsage {
                input_tokens: 30,
                cached_input_tokens: 5,
                cache_write_input_tokens: 1,
                output_tokens: 6,
                reasoning_output_tokens: 2,
                total_tokens: 37,
            },
            days: vec![service::DayUsage {
                date: "2026-08-25".to_owned(),
                usage: TokenUsage {
                    input_tokens: 30,
                    total_tokens: 37,
                    ..TokenUsage::default()
                },
                sessions: 2,
            }],
            hours: vec![service::HourUsage {
                date: "2026-08-25".to_owned(),
                hour: 2,
                usage: TokenUsage {
                    input_tokens: 30,
                    total_tokens: 37,
                    ..TokenUsage::default()
                },
                sessions: 2,
            }],
            projects: vec![service::ProjectUsage {
                project: "codexhub".to_owned(),
                usage: TokenUsage {
                    input_tokens: 30,
                    total_tokens: 37,
                    ..TokenUsage::default()
                },
                sessions: 2,
            }],
            models: vec![service::ModelUsage {
                model: "gpt-5.6-sol".to_owned(),
                usage: TokenUsage {
                    input_tokens: 30,
                    total_tokens: 37,
                    ..TokenUsage::default()
                },
                sessions: 2,
                cost_usd: 0.0,
            }],
            providers: vec![service::ProviderUsage {
                provider: "ai-gateway".to_owned(),
                usage: TokenUsage {
                    input_tokens: 30,
                    total_tokens: 37,
                    ..TokenUsage::default()
                },
                sessions: 2,
            }],
            quota: None,
            minutes: Vec::new(),
            quota_history: Vec::new(),
            quota_minutes: Vec::new(),
            breakdown: vec![service::BreakdownRow {
                day: "2026-08-25".to_owned(),
                service: "codex".to_owned(),
                source: "ai-gateway".to_owned(),
                model: "gpt-5.6-sol".to_owned(),
                project: "codexhub".to_owned(),
                usage: TokenUsage {
                    input_tokens: 30,
                    total_tokens: 37,
                    ..TokenUsage::default()
                },
                cost_usd: 0.0,
            }],
            sessions_root: Some("/home/example/.codex/sessions".to_owned()),
            generated_at_ms: 1_700_000_000_000,
        };

        let response = snapshot(summary);
        assert_eq!(response.window_days, 7);
        assert_eq!(response.sessions, 2);
        assert_eq!(response.totals.total_tokens, 37);
        assert_eq!(response.totals.cache_write_input_tokens, 1);
        assert_eq!(response.days.len(), 1);
        assert_eq!(response.days[0].date, "2026-08-25");
        assert_eq!(response.days[0].totals.input_tokens, 30);
        assert_eq!(response.hours.len(), 1);
        assert_eq!(response.hours[0].hour, 2);
        assert_eq!(response.hours[0].date, "2026-08-25");
        assert_eq!(response.projects.len(), 1);
        assert_eq!(response.projects[0].project, "codexhub");
        assert_eq!(response.models.len(), 1);
        assert_eq!(response.models[0].model, "gpt-5.6-sol");
        assert_eq!(response.providers[0].provider, "ai-gateway");
        assert_eq!(response.breakdown.len(), 1);
        assert_eq!(response.breakdown[0].service, "codex");
        assert_eq!(response.breakdown[0].model, "gpt-5.6-sol");
        assert_eq!(response.breakdown[0].project, "codexhub");
    }

    #[test]
    fn response_serialises_with_camel_case_keys() {
        let summary = Summary {
            window_days: 7,
            sessions: 0,
            usage: TokenUsage::default(),
            days: Vec::new(),
            hours: Vec::new(),
            projects: Vec::new(),
            models: Vec::new(),
            providers: Vec::new(),
            breakdown: Vec::new(),
            quota: None,
            minutes: Vec::new(),
            quota_history: Vec::new(),
            quota_minutes: Vec::new(),
            sessions_root: None,
            generated_at_ms: 42,
        };
        let rendered = serde_json::to_value(snapshot(summary)).expect("serialise");
        assert!(rendered.get("generatedAtMs").is_some());
        assert!(rendered.get("windowDays").is_some());
        assert_eq!(
            rendered.get("generatedAtMs").and_then(|v| v.as_u64()),
            Some(42)
        );
    }
}
