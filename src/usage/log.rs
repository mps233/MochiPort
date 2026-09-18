//! Codex rollout log parsing.
//!
//! Codex writes one JSON record per line under
//! `~/.codex/sessions/<year>/<month>/<day>/rollout-*.jsonl`. Only two record
//! kinds matter for usage accounting: `session_meta` (which carries `cwd`, so
//! usage can be attributed to a project) and `event_msg` / `token_count`
//! (which carries the token counters and, for official accounts, the rate
//! limit windows).
//!
//! Everything else in the file is ignored, and unknown or malformed records are
//! skipped rather than treated as failures: these files are written by another
//! process and are only ever read incrementally.

use serde::{Deserialize, Serialize};

/// Token counters as reported by Codex.
///
/// `total_token_usage` is cumulative for the session while `last_token_usage`
/// covers the most recent turn; both use this shape.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TokenUsage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub cached_input_tokens: u64,
    #[serde(default)]
    pub cache_write_input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub reasoning_output_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
}

/// One quota window as reported by Codex.
///
/// Both windows may be absent: sessions routed through the AI gateway report
/// every window as null, which is normal and must not discard the usage sample.
///
/// The window's *duration* is carried by the record itself (`window_minutes`),
/// which is what callers should use to tell the windows apart. Naming them after
/// `primary`/`secondary` is not reliable: measured records report `primary` with
/// `window_minutes` of 43200 (30 days) as well as the expected short windows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct RateLimitWindow {
    /// Percentage of the window already consumed.
    pub used_percent: f64,
    /// Window length in minutes, as reported by Codex.
    pub window_minutes: Option<u32>,
    /// Absolute reset time, in milliseconds since the Unix epoch.
    ///
    /// Codex has shipped three shapes: an absolute epoch second `resets_at`, a
    /// relative `resets_in_seconds`, and `resets_at: null` when the window has no
    /// scheduled reset. The relative form is resolved against the record
    /// timestamp at parse time so callers only ever see absolute milliseconds.
    pub resets_at_ms: Option<i64>,
}

/// Quota windows attached to a `token_count` record.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct RateLimits {
    #[serde(default)]
    pub limit_id: Option<String>,
    /// The record's `primary` window. Classify by `window_minutes` rather than
    /// this position: measured records report 43200 (30 days) here.
    #[serde(default)]
    pub primary: Option<RateLimitWindow>,
    /// The record's `secondary` window, usually absent.
    #[serde(default)]
    pub secondary: Option<RateLimitWindow>,
    #[serde(default)]
    pub plan_type: Option<String>,
    #[serde(default)]
    pub rate_limit_reached_type: Option<String>,
}

impl RateLimits {
    /// Whether either window carries usable numbers.
    pub(crate) fn has_window(&self) -> bool {
        self.primary.is_some() || self.secondary.is_some()
    }
}

/// Raw wire shape, before the relative/absolute reset forms are normalised.
#[derive(Deserialize)]
struct RawRateLimits {
    #[serde(default)]
    limit_id: Option<String>,
    #[serde(default)]
    primary: Option<RawRateLimitWindow>,
    #[serde(default)]
    secondary: Option<RawRateLimitWindow>,
    #[serde(default)]
    plan_type: Option<String>,
    #[serde(default)]
    rate_limit_reached_type: Option<String>,
}

#[derive(Deserialize)]
struct RawRateLimitWindow {
    #[serde(default)]
    used_percent: Option<f64>,
    #[serde(default)]
    window_minutes: Option<u32>,
    #[serde(default)]
    resets_at: Option<f64>,
    #[serde(default)]
    resets_in_seconds: Option<f64>,
}

impl RawRateLimitWindow {
    /// Normalises one window against the record's own timestamp.
    ///
    /// `record_timestamp_ms` is only needed for the relative encoding; when it
    /// is unknown the relative form cannot be resolved and the reset time is
    /// dropped rather than guessed.
    fn normalize(self, record_timestamp_ms: Option<i64>) -> Option<RateLimitWindow> {
        // A window without `used_percent` carries nothing to display.
        let used_percent = self.used_percent?;
        let resets_at_ms = if let Some(epoch_seconds) = self.resets_at {
            Some((epoch_seconds * 1000.0) as i64)
        } else {
            match (self.resets_in_seconds, record_timestamp_ms) {
                (Some(seconds), Some(now_ms)) => Some(now_ms + (seconds * 1000.0) as i64),
                _ => None,
            }
        };
        Some(RateLimitWindow {
            used_percent,
            window_minutes: self.window_minutes,
            resets_at_ms,
        })
    }
}

impl RawRateLimits {
    fn normalize(self, record_timestamp_ms: Option<i64>) -> RateLimits {
        RateLimits {
            limit_id: self.limit_id,
            primary: self.primary.and_then(|w| w.normalize(record_timestamp_ms)),
            secondary: self
                .secondary
                .and_then(|w| w.normalize(record_timestamp_ms)),
            plan_type: self.plan_type,
            rate_limit_reached_type: self.rate_limit_reached_type,
        }
    }
}

/// Parses an RFC3339 timestamp into milliseconds since the Unix epoch.
///
/// Only the forms Codex writes are handled (`...Z` and `+HH:MM`); anything else
/// yields `None`, which callers treat as "timestamp unknown".
pub(crate) fn timestamp_ms(timestamp: &str) -> Option<i64> {
    let bytes = timestamp.as_bytes();
    if bytes.len() < 19 {
        return None;
    }
    let number = |range: std::ops::Range<usize>| -> Option<i64> {
        timestamp.get(range)?.parse::<i64>().ok()
    };
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }

    // Days since the Unix epoch, by way of the civil-from-days algorithm.
    let adjusted_year = year - i64::from(month <= 2);
    let era = if adjusted_year >= 0 {
        adjusted_year
    } else {
        adjusted_year - 399
    } / 400;
    let year_of_era = adjusted_year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;

    let mut seconds = days * 86_400 + hour * 3_600 + minute * 60 + second;

    // Apply the declared offset, if any, so the result is true UTC.
    if let Some(rest) = timestamp.get(19..) {
        let sign = rest.as_bytes().first().copied();
        if matches!(sign, Some(b'+') | Some(b'-')) {
            let offset_hours = rest.get(1..3)?.parse::<i64>().ok()?;
            let offset_minutes = rest.get(4..6)?.parse::<i64>().ok()?;
            let magnitude = offset_hours * 3_600 + offset_minutes * 60;
            seconds += if sign == Some(b'+') {
                -magnitude
            } else {
                magnitude
            };
        }
    }

    Some(seconds * 1000)
}

/// Identity and working directory of one rollout file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SessionMeta {
    pub session_id: Option<String>,
    pub thread_id: Option<String>,
    pub cwd: Option<String>,
    pub originator: Option<String>,
    pub started_at: Option<String>,
    /// The provider Codex recorded for the session, from
    /// `payload.model_provider`.
    ///
    /// This is the same field the desktop clients read for their `source`
    /// dimension, and it is present on every local session. `thread_settings_applied`
    /// carries a *different* provider id (`ai-gateway`, `custom`) that changes
    /// mid-session, so it is used first when present and this is the fallback;
    /// without it about a sixth of all usage had no provider at all and was
    /// dropped from the cross-product breakdown.
    pub provider: Option<String>,
}

/// The model and provider in effect for a session, from
/// `event_msg` / `thread_settings_applied`.
///
/// Codex emits this again whenever the settings change, so one session can carry
/// several of them. Usage samples therefore have to be attributed to the
/// settings in effect when they were recorded, not to the first or last one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ThreadSettings {
    pub model: Option<String>,
    pub provider: Option<String>,
}

/// Per-turn context, from the top-level `turn_context` records.
///
/// This is the second source of the model, and the broader one: a session that
/// never re-emits `thread_settings_applied` (about 30% of the local sessions)
/// still carries `turn_context` for every turn. It also declares `cwd` and the
/// writer's `timezone`, neither of which the directory layout provides.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TurnContext {
    pub model: Option<String>,
    pub cwd: Option<String>,
    pub timezone: Option<String>,
}

/// One `token_count` record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct TokenCount {
    /// Usage for the turn this record reports: `last_token_usage`, falling back
    /// to `total_token_usage` when a writer omits the per-turn counters.
    ///
    /// Both existing clients sum these per-turn values. That is the figure that
    /// matches what Codex actually consumes: the cumulative counters count the
    /// whole context again on every turn, so summing them is not meaningful and
    /// reading only the last one runs orders of magnitude high.
    pub usage: TokenUsage,
    /// Session-cumulative counters, kept for context and quota estimation.
    pub cumulative: Option<TokenUsage>,
    pub model_context_window: Option<u64>,
    pub rate_limits: RateLimits,
    /// The record's own timestamp, kept verbatim (RFC3339) so that time bucketing
    /// needs no calendar dependency.
    pub timestamp: Option<String>,
}

/// The records this module understands.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) enum RolloutEvent {
    SessionMeta(SessionMeta),
    TokenCount(TokenCount),
    ThreadSettings(ThreadSettings),
    TurnContext(TurnContext),
}

#[derive(Deserialize)]
struct RawRecord {
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default, rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    payload: Option<serde_json::Value>,
    /// Older clients put the provider at the top level rather than in `payload`.
    #[serde(default)]
    model_provider: Option<String>,
}

#[derive(Deserialize)]
struct TokenCountPayload {
    info: TokenCountInfo,
    #[serde(default)]
    rate_limits: Option<RawRateLimits>,
}

#[derive(Deserialize)]
struct TokenCountInfo {
    #[serde(default)]
    total_token_usage: Option<TokenUsage>,
    #[serde(default)]
    last_token_usage: Option<TokenUsage>,
    #[serde(default)]
    model_context_window: Option<u64>,
}

#[derive(Deserialize)]
struct SessionMetaPayload {
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    originator: Option<String>,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    model_provider: Option<String>,
    /// A few older clients put the provider under `provider` instead.
    #[serde(default)]
    provider: Option<String>,
}

/// Parses one line of a rollout file.
///
/// Returns `None` for blank lines, records this module does not model, and
/// records whose payload does not match the documented shape. Callers treat a
/// `None` as "nothing to account for", not as corruption.
pub(crate) fn parse_record(line: &str) -> Option<RolloutEvent> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    let record: RawRecord = serde_json::from_str(trimmed).ok()?;
    let payload = record.payload?;
    match record.kind.as_deref()? {
        "session_meta" => {
            let meta: SessionMetaPayload = serde_json::from_value(payload).ok()?;
            Some(RolloutEvent::SessionMeta(SessionMeta {
                session_id: meta.session_id,
                thread_id: meta.id,
                cwd: meta.cwd,
                originator: meta.originator,
                // `model_provider` is what Codex writes today; the other two are
                // the shapes the clients also accept for older logs.
                provider: meta
                    .model_provider
                    .or(meta.provider)
                    .or_else(|| {
                        record
                            .model_provider
                            .as_deref()
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                            .map(str::to_owned)
                    })
                    .filter(|value| !value.trim().is_empty()),
                started_at: meta.timestamp.or(record.timestamp),
            }))
        }
        "event_msg" => match payload.get("type").and_then(|value| value.as_str())? {
            "token_count" => {
                let token_count: TokenCountPayload = serde_json::from_value(payload).ok()?;
                let cumulative = token_count.info.total_token_usage;
                let usage = token_count.info.last_token_usage.or(cumulative)?;
                // The relative reset form needs the record's own timestamp.
                let record_ms = record.timestamp.as_deref().and_then(timestamp_ms);
                Some(RolloutEvent::TokenCount(TokenCount {
                    usage,
                    cumulative,
                    model_context_window: token_count.info.model_context_window,
                    rate_limits: token_count
                        .rate_limits
                        .map(|raw| raw.normalize(record_ms))
                        .unwrap_or_default(),
                    timestamp: record.timestamp,
                }))
            }
            "thread_settings_applied" => {
                let settings = payload.get("thread_settings")?;
                let model = settings
                    .get("model")
                    .and_then(|value| value.as_str())
                    .map(str::to_owned);
                let provider = settings
                    .get("model_provider_id")
                    .and_then(|value| value.as_str())
                    .map(str::to_owned);
                if model.is_none() && provider.is_none() {
                    return None;
                }
                Some(RolloutEvent::ThreadSettings(ThreadSettings {
                    model,
                    provider,
                }))
            }
            _ => None,
        },
        "turn_context" => {
            let model = payload
                .get("model")
                .and_then(|value| value.as_str())
                .filter(|model| !model.is_empty())
                .map(str::to_owned);
            let cwd = payload
                .get("cwd")
                .and_then(|value| value.as_str())
                .map(str::to_owned);
            let timezone = payload
                .get("timezone")
                .and_then(|value| value.as_str())
                .map(str::to_owned);
            if model.is_none() && cwd.is_none() {
                return None;
            }
            Some(RolloutEvent::TurnContext(TurnContext {
                model,
                cwd,
                timezone,
            }))
        }
        _ => None,
    }
}

/// Extracts the minute since the Unix epoch from a Codex timestamp.
///
/// The clients' live rates work over absolute minutes — `activeBaselineRate`
/// averages the last 24 hours, which crosses midnight — so buckets have to carry
/// the day, not just a minute-of-day index.
pub(crate) fn epoch_minute(timestamp: &str) -> Option<i64> {
    timestamp_ms(timestamp).map(|ms| ms.div_euclid(60_000))
}

/// Extracts the hour of day from a Codex timestamp (`YYYY-MM-DDTHH:MM:SS...`).
///
/// Deliberately string-based: the clients want hour buckets, and a full
/// calendar conversion would pull in a date library for no added value.
pub(crate) fn hour_of_day(timestamp: &str) -> Option<u8> {
    let hour = timestamp.get(11..13)?.parse::<u8>().ok()?;
    (hour < 24).then_some(hour)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_hour_from_codex_timestamps() {
        assert_eq!(hour_of_day("2026-08-25T02:17:02.910Z"), Some(2));
        assert_eq!(hour_of_day("2026-08-25T23:59:59.999Z"), Some(23));
        assert_eq!(hour_of_day("2026-08-25T00:00:00Z"), Some(0));
    }

    #[test]
    fn extracts_absolute_minutes_across_day_boundaries() {
        let first = epoch_minute("2026-08-25T23:59:00Z").expect("timestamp");
        let second = epoch_minute("2026-08-26T00:01:00Z").expect("timestamp");
        // Two minutes apart even though the minute-of-day wrapped around.
        assert_eq!(second - first, 2);
        assert_eq!(epoch_minute("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(epoch_minute("1970-01-01T00:01:30Z"), Some(1));
        assert_eq!(epoch_minute("not-a-timestamp"), None);
    }

    #[test]
    fn rejects_timestamps_without_a_valid_hour() {
        assert_eq!(hour_of_day(""), None);
        assert_eq!(hour_of_day("2026-08-25"), None);
        assert_eq!(hour_of_day("2026-08-25T99:00:00Z"), None);
        assert_eq!(hour_of_day("not-a-timestamp-at-all"), None);
    }

    // Shapes mirror the records measured in ~/.codex/sessions.
    const SESSION_META: &str = r#"{"timestamp":"2026-08-25T02:16:45.551Z","ordinal":0,"type":"session_meta","payload":{"session_id":"01a03005-4b42-72d0-a5f4-8e666294aac7","id":"01a036b4-acb2-7fa1-b00b-d0d0a64a561b","timestamp":"2026-08-25T02:16:45.522Z","cwd":"/Users/example/project","originator":"Codex Desktop","cli_version":"0.5.11"}}"#;

    const TOKEN_COUNT: &str = r#"{"timestamp":"2026-08-25T02:17:02.910Z","ordinal":12,"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":24230,"cached_input_tokens":22272,"cache_write_input_tokens":0,"output_tokens":203,"reasoning_output_tokens":48,"total_tokens":24433},"last_token_usage":{"input_tokens":24230,"cached_input_tokens":22272,"cache_write_input_tokens":0,"output_tokens":203,"reasoning_output_tokens":48,"total_tokens":24433},"model_context_window":258400},"rate_limits":{"limit_id":"codex","limit_name":null,"primary":null,"secondary":null,"credits":null,"individual_limit":null,"spend_control_reached":null,"plan_type":null,"rate_limit_reached_type":null}}}"#;

    #[test]
    fn parses_session_meta_with_project_directory() {
        let event = parse_record(SESSION_META).expect("session_meta parses");
        let RolloutEvent::SessionMeta(meta) = event else {
            panic!("expected session_meta, got {event:?}");
        };
        assert_eq!(meta.cwd.as_deref(), Some("/Users/example/project"));
        assert_eq!(
            meta.thread_id.as_deref(),
            Some("01a036b4-acb2-7fa1-b00b-d0d0a64a561b")
        );
        assert_eq!(meta.originator.as_deref(), Some("Codex Desktop"));
    }

    #[test]
    fn parses_token_count_counters_and_context_window() {
        let event = parse_record(TOKEN_COUNT).expect("token_count parses");
        let RolloutEvent::TokenCount(count) = event else {
            panic!("expected token_count, got {event:?}");
        };
        assert_eq!(count.usage.input_tokens, 24230);
        assert_eq!(count.usage.cached_input_tokens, 22272);
        assert_eq!(count.usage.output_tokens, 203);
        assert_eq!(count.usage.reasoning_output_tokens, 48);
        assert_eq!(count.usage.total_tokens, 24433);
        assert_eq!(count.model_context_window, Some(258400));
        assert_eq!(count.rate_limits.limit_id.as_deref(), Some("codex"));
    }

    /// A gateway-routed session reports every rate limit field as null; that
    /// must not discard the usage sample.
    #[test]
    fn keeps_usage_when_rate_limits_are_entirely_null() {
        let event = parse_record(TOKEN_COUNT).expect("token_count parses");
        let RolloutEvent::TokenCount(count) = event else {
            panic!("expected token_count");
        };
        assert!(count.rate_limits.primary.is_none());
        assert!(count.rate_limits.plan_type.is_none());
        assert_eq!(count.usage.total_tokens, 24433);
    }

    const THREAD_SETTINGS: &str = r#"{"timestamp":"2026-08-25T02:16:52.020Z","ordinal":4,"type":"event_msg","payload":{"type":"thread_settings_applied","thread_settings":{"model":"gpt-5.6-sol","model_provider_id":"ai-gateway","approval_policy":"never"}}}"#;

    #[test]
    fn parses_thread_settings_model_and_provider() {
        let event = parse_record(THREAD_SETTINGS).expect("thread_settings_applied parses");
        let RolloutEvent::ThreadSettings(settings) = event else {
            panic!("expected thread settings, got {event:?}");
        };
        assert_eq!(settings.model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(settings.provider.as_deref(), Some("ai-gateway"));
    }

    /// Settings carrying neither field say nothing about attribution, so they
    /// are skipped rather than recorded as an empty change.
    #[test]
    fn skips_thread_settings_without_model_or_provider() {
        let line = r#"{"type":"event_msg","payload":{"type":"thread_settings_applied","thread_settings":{"approval_policy":"never"}}}"#;
        assert!(parse_record(line).is_none());
    }

    /// Measured shape: `turn_context` carries the model for every turn, so it
    /// covers sessions that never emit `thread_settings_applied`.
    const TURN_CONTEXT: &str = r#"{"timestamp":"2026-07-09T06:45:33.708Z","ordinal":4,"type":"turn_context","payload":{"turn_id":"019f459f-e170-7431-b186-17806e1d7501","cwd":"/Users/example/nih","workspace_roots":["/Users/example"],"current_date":"2026-07-09","timezone":"Asia/Shanghai","approval_policy":"never","model":"gpt-5.6-sol"}}"#;

    #[test]
    fn parses_turn_context_model_directory_and_timezone() {
        let event = parse_record(TURN_CONTEXT).expect("turn_context parses");
        let RolloutEvent::TurnContext(context) = event else {
            panic!("expected turn context, got {event:?}");
        };
        assert_eq!(context.model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(context.cwd.as_deref(), Some("/Users/example/nih"));
        // The writer declares its own timezone, so day attribution does not have
        // to guess at the reader's locale.
        assert_eq!(context.timezone.as_deref(), Some("Asia/Shanghai"));
    }

    #[test]
    fn skips_turn_context_without_model_or_directory() {
        let line = r#"{"type":"turn_context","payload":{"turn_id":"x"}}"#;
        assert!(parse_record(line).is_none());
    }

    /// Measured on this machine: a real record whose `primary` window is a
    /// 30-day window (`window_minutes: 43200`), not the 5-hour window the field
    /// name suggests. Copied verbatim from a rollout log.
    const REAL_QUOTA_RECORD: &str = r#"{"timestamp":"2026-07-09T16:46:45.966Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"output_tokens":10,"total_tokens":110}},"rate_limits":{"limit_id":"codex","limit_name":null,"primary":{"used_percent":5.0,"window_minutes":43200,"resets_at":1786207603},"secondary":null,"credits":{"has_credits":false,"unlimited":false,"balance":null},"individual_limit":null,"spend_control_reached":null,"plan_type":"free","rate_limit_reached_type":null}}}"#;

    #[test]
    fn probe_timestamp_presence() {
        let event = parse_record(REAL_QUOTA_RECORD).expect("parses");
        let RolloutEvent::TokenCount(count) = event else {
            panic!("expected")
        };
        eprintln!("timestamp = {:?}", count.timestamp);
        eprintln!("has_window = {}", count.rate_limits.has_window());
        eprintln!("primary = {:?}", count.rate_limits.primary);
    }

    #[test]
    fn parses_a_real_record_with_window_minutes() {
        let event = parse_record(REAL_QUOTA_RECORD).expect("real record parses");
        let RolloutEvent::TokenCount(count) = event else {
            panic!("expected token_count");
        };
        let primary = count.rate_limits.primary.clone().expect("primary window");
        assert!((primary.used_percent - 5.0).abs() < 1e-9);
        // The window length is the only reliable way to tell windows apart.
        assert_eq!(primary.window_minutes, Some(43_200));
        assert_eq!(primary.resets_at_ms, Some(1_786_207_603_000));
        // `secondary` was null in every measured record.
        assert!(count.rate_limits.secondary.is_none());
        assert_eq!(count.rate_limits.plan_type.as_deref(), Some("free"));
        assert!(count.rate_limits.has_window());
    }

    /// Also measured: a window may report `resets_at: null`, meaning no scheduled
    /// reset. That is not the same as a malformed record.
    #[test]
    fn accepts_a_window_without_a_scheduled_reset() {
        let line = r#"{"timestamp":"2026-07-30T03:22:21.588Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"output_tokens":1,"total_tokens":11}},"rate_limits":{"limit_id":"codex","primary":{"used_percent":2.0,"window_minutes":43800,"resets_at":null},"secondary":null,"plan_type":null}}}"#;
        let event = parse_record(line).expect("record parses");
        let RolloutEvent::TokenCount(count) = event else {
            panic!("expected token_count");
        };
        let primary = count.rate_limits.primary.expect("primary window");
        assert!((primary.used_percent - 2.0).abs() < 1e-9);
        assert_eq!(primary.window_minutes, Some(43_800));
        assert_eq!(primary.resets_at_ms, None);
    }

    /// Newer Codex writes absolute epoch seconds.
    #[test]
    fn parses_absolute_quota_windows() {
        let line = r#"{"timestamp":"2026-08-25T02:17:02.910Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"output_tokens":1,"total_tokens":11}},"rate_limits":{"limit_id":"codex","primary":{"used_percent":42.5,"resets_at":1787624166},"secondary":{"used_percent":7.0,"resets_at":1788228966},"plan_type":"plus"}}}"#;
        let event = parse_record(line).expect("token_count parses");
        let RolloutEvent::TokenCount(count) = event else {
            panic!("expected token_count");
        };
        let primary = count.rate_limits.primary.clone().expect("primary window");
        assert!((primary.used_percent - 42.5).abs() < 1e-9);
        assert_eq!(primary.resets_at_ms, Some(1_787_624_166_000));
        let secondary = count
            .rate_limits
            .secondary
            .clone()
            .expect("secondary window");
        assert_eq!(secondary.resets_at_ms, Some(1_788_228_966_000));
        assert_eq!(count.rate_limits.plan_type.as_deref(), Some("plus"));
        assert!(count.rate_limits.has_window());
    }

    /// Older Codex writes a relative reset, which is resolved against the
    /// record's own timestamp so callers only ever see an absolute value.
    #[test]
    fn resolves_relative_quota_resets_against_the_record_timestamp() {
        let line = r#"{"timestamp":"2026-08-25T02:17:02.910Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"output_tokens":1,"total_tokens":11}},"rate_limits":{"primary":{"used_percent":10.0,"resets_in_seconds":3600}}}}"#;
        let event = parse_record(line).expect("token_count parses");
        let RolloutEvent::TokenCount(count) = event else {
            panic!("expected token_count");
        };
        let record_ms = timestamp_ms("2026-08-25T02:17:02.910Z").expect("timestamp");
        let primary = count.rate_limits.primary.expect("primary window");
        assert_eq!(primary.resets_at_ms, Some(record_ms + 3_600_000));
    }

    /// A relative reset without a usable record timestamp is dropped rather
    /// than guessed at.
    #[test]
    fn drops_relative_resets_without_a_timestamp() {
        let line = r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"output_tokens":1,"total_tokens":11}},"rate_limits":{"primary":{"used_percent":10.0,"resets_in_seconds":3600}}}}"#;
        let event = parse_record(line).expect("token_count parses");
        let RolloutEvent::TokenCount(count) = event else {
            panic!("expected token_count");
        };
        let primary = count.rate_limits.primary.expect("primary window");
        assert_eq!(primary.used_percent, 10.0);
        assert_eq!(primary.resets_at_ms, None);
    }

    /// A window without `used_percent` carries nothing to display and is
    /// dropped, while the other window survives.
    #[test]
    fn drops_windows_without_a_percentage() {
        let line = r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"output_tokens":1,"total_tokens":11}},"rate_limits":{"primary":{"resets_at":1787624166},"secondary":{"used_percent":7.0,"resets_at":1788228966}}}}"#;
        let event = parse_record(line).expect("token_count parses");
        let RolloutEvent::TokenCount(count) = event else {
            panic!("expected token_count");
        };
        assert!(count.rate_limits.primary.is_none());
        assert!(count.rate_limits.secondary.is_some());
    }

    /// The gateway case: every window null. The usage sample must survive.
    #[test]
    fn keeps_usage_when_windows_are_null() {
        let event = parse_record(TOKEN_COUNT).expect("token_count parses");
        let RolloutEvent::TokenCount(count) = event else {
            panic!("expected token_count");
        };
        assert!(!count.rate_limits.has_window());
        assert_eq!(count.usage.total_tokens, 24433);
    }

    #[test]
    fn converts_rfc3339_timestamps_to_epoch_milliseconds() {
        assert_eq!(timestamp_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(timestamp_ms("1970-01-02T00:00:00Z"), Some(86_400_000));
        // A positive offset is behind UTC, so it maps to an earlier instant.
        assert_eq!(
            timestamp_ms("1970-01-01T08:00:00+08:00"),
            Some(0),
            "08:00+08:00 is 00:00Z"
        );
        assert_eq!(
            timestamp_ms("2026-08-25T02:17:02Z"),
            Some(1_787_624_222_000)
        );
        assert_eq!(timestamp_ms("not-a-timestamp"), None);
        assert_eq!(timestamp_ms(""), None);
        // Out-of-range components are rejected rather than silently rolled over.
        assert_eq!(timestamp_ms("2026-13-01T00:00:00Z"), None);
        assert_eq!(timestamp_ms("2026-08-25T25:00:00Z"), None);
    }

    #[test]
    fn skips_unknown_and_malformed_records() {
        assert!(parse_record("").is_none());
        assert!(parse_record("   ").is_none());
        assert!(parse_record("not json").is_none());
        assert!(
            parse_record(r#"{"type":"response_item","payload":{"type":"reasoning"}}"#).is_none()
        );
        assert!(
            parse_record(r#"{"type":"event_msg","payload":{"type":"item_completed"}}"#).is_none()
        );
        assert!(parse_record(r#"{"type":"session_meta"}"#).is_none());
    }

    /// Records written by other Codex versions may omit counters this build
    /// knows about; missing counters read as zero instead of failing.
    #[test]
    fn tolerates_missing_counters() {
        let line = r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"output_tokens":2,"total_tokens":12}}}}"#;
        let event = parse_record(line).expect("token_count parses");
        let RolloutEvent::TokenCount(count) = event else {
            panic!("expected token_count");
        };
        assert_eq!(count.usage.input_tokens, 10);
        assert_eq!(count.usage.cached_input_tokens, 0);
        assert_eq!(count.usage.reasoning_output_tokens, 0);
        assert!(count.model_context_window.is_none());
    }
}
