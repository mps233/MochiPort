//! Usage summaries served to the clients.
//!
//! A full scan of the local Codex sessions tree takes tens of seconds, so the
//! summary is always narrowed to a window of recent days and memoised for a
//! short interval. Days are discovered by reading the `<year>/<month>/<day>`
//! directory names, which sort correctly as strings and avoid pulling in a
//! calendar dependency.

use std::{
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use super::{
    cost, log,
    log::TokenUsage,
    scan::{self, SessionScan, accumulate},
};

/// Days included when a caller does not ask for a specific window.
pub(crate) const DEFAULT_WINDOW_DAYS: u32 = 7;
/// Upper bound on the window, to keep one request from walking everything.
pub(crate) const MAX_WINDOW_DAYS: u32 = 90;
/// Window used by the history rebuild, which by definition wants everything on
/// disk rather than a recent slice. Kept separate from `MAX_WINDOW_DAYS` so the
/// interactive endpoint stays bounded while the rebuild is still complete.
pub(crate) const HISTORY_WINDOW_DAYS: u32 = 3650;

const CACHE_TTL: Duration = Duration::from_secs(30);

/// How much minute-level history the clients can use.
///
/// macOS trims its minute buckets to 48h and derives its 24h baseline from them;
/// Windows averages a 24h baseline and 3/10-minute rate windows. 48h covers both
/// with margin, and it lets a 24h window cross midnight.
///
/// The series used to cover the whole requested window, so a 105-day refresh
/// shipped tens of thousands of buckets (measured: 27547 distinct minutes) every
/// 30 seconds while the clients discarded all but the last day.
const MINUTE_WINDOW_MINUTES: i64 = 48 * 60;

/// The client's `ServiceID` has exactly one case today, so breakdown rows carry
/// it as a constant instead of parsing a service name out of every record.
const CODEX_SERVICE: &str = "codex";

/// Usage attributed to one calendar day.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DayUsage {
    pub date: String,
    pub usage: TokenUsage,
    pub sessions: u32,
}

/// Usage for one hour of one day, for the recent-activity series.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HourUsage {
    pub date: String,
    pub hour: u8,
    pub usage: TokenUsage,
    pub sessions: u32,
}

/// Usage attributed to one project (the last segment of the session's `cwd`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectUsage {
    pub project: String,
    pub usage: TokenUsage,
    pub sessions: u32,
}

/// Usage attributed to one model. Attribution follows the settings in effect
/// when each sample was recorded, so a session that switched models splits here.
// `f64` cost means this cannot derive `Eq`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ModelUsage {
    pub model: String,
    pub usage: TokenUsage,
    pub sessions: u32,
    /// API-equivalent estimate in US dollars, for the same window as `usage`.
    pub cost_usd: f64,
}

/// Usage attributed to one provider (`model_provider_id`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProviderUsage {
    pub provider: String,
    pub usage: TokenUsage,
    pub sessions: u32,
}

/// One row of the cross-product breakdown.
///
/// Mirrors the macOS history database's primary key
/// `(day, service, source, model, project)`: that database replaces a row rather
/// than adding to it, so callers rebuilding from the daemon need the complete
/// combination, not per-dimension totals.
// `f64` cost means this cannot derive `Eq`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BreakdownRow {
    pub day: String,
    /// Always `codex` today: the client's `ServiceID` has a single case, so the
    /// value is constant rather than parsed from the log.
    pub service: String,
    /// `model_provider_id`, i.e. `ai-gateway` / `custom` / `sub2api`.
    pub source: String,
    pub model: String,
    pub project: String,
    pub usage: TokenUsage,
    /// Cost for this row, priced with its own row's model. Clients sum these per
    /// day or per week instead of re-deriving cost from a local event stream.
    pub cost_usd: f64,
}

/// One minute-level quota reading.
///
/// The clients keep a 60-minute series of these to estimate short-horizon
/// depletion, so the daemon exposes the same window rather than a day-level
/// summary.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct QuotaPoint {
    /// Minutes since the Unix epoch.
    pub minute: i64,
    /// `primary` or `secondary`, i.e. the record position.
    pub kind: &'static str,
    pub used_percent: f64,
}

/// How much minute-level quota history to keep, matching the clients'
/// `percentHistory` trim window.
const QUOTA_MINUTE_WINDOW: i64 = 60;

/// One day's last observed percentage for one quota window.
///
/// Mirrors the clients' `percent_snapshots` table, whose key is
/// `(day, service, kind)` and which stores a single reading per day.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct QuotaHistoryPoint {
    pub day: String,
    /// `primary` or `secondary`, i.e. the record position.
    pub kind: &'static str,
    pub used_percent: f64,
}

/// Token totals for one absolute minute, which is the unit the clients' live
/// rates are computed over. Minutes without samples are omitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MinuteBucket {
    /// Minutes since the Unix epoch.
    pub minute: i64,
    pub usage: TokenUsage,
}

/// The most recent quota window reported by Codex, if any.
///
/// Sessions routed through the AI gateway report every window as null, so this
/// is frequently absent — a normal state, not an error.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct QuotaSnapshot {
    /// The record's `primary` window. Classify by `window_minutes`, which
    /// measured records set to 43200 (30 days) — not the 5-hour window the
    /// position suggests.
    pub primary: Option<log::RateLimitWindow>,
    /// The record's `secondary` window, usually absent.
    pub secondary: Option<log::RateLimitWindow>,
    pub plan_type: Option<String>,
    /// When the sample this snapshot came from was recorded.
    pub observed_at_ms: Option<i64>,
}

/// Usage across the requested window, newest day first.
// `ModelUsage` and `BreakdownRow` both carry `f64` costs, so this cannot derive
// `Eq` either.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Summary {
    pub window_days: u32,
    pub sessions: u32,
    pub usage: TokenUsage,
    pub days: Vec<DayUsage>,
    /// Newest hour first.
    pub hours: Vec<HourUsage>,
    /// Highest usage first.
    pub projects: Vec<ProjectUsage>,
    /// Highest usage first.
    pub models: Vec<ModelUsage>,
    /// Highest usage first.
    pub providers: Vec<ProviderUsage>,
    /// Cross-product rows, mirroring the client history database's key.
    pub breakdown: Vec<BreakdownRow>,
    /// Latest quota windows, when the logs contained any.
    pub quota: Option<QuotaSnapshot>,
    /// Per-minute totals for the trailing day, oldest first.
    pub minutes: Vec<MinuteBucket>,
    /// Last quota reading per day and window, oldest day first.
    pub quota_history: Vec<QuotaHistoryPoint>,
    /// Minute-level quota readings over the trailing window, oldest first.
    pub quota_minutes: Vec<QuotaPoint>,
    /// `None` when no local Codex installation was found.
    pub sessions_root: Option<String>,
    pub generated_at_ms: u64,
}

struct CacheEntry {
    window_days: u32,
    generated: Instant,
    summary: Summary,
}

fn cache() -> &'static Mutex<Option<CacheEntry>> {
    static CACHE: OnceLock<Mutex<Option<CacheEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// Clamps a requested window to the supported range.
pub(crate) fn normalize_window_days(requested: Option<u32>) -> u32 {
    match requested {
        Some(days) if days > 0 => days.min(MAX_WINDOW_DAYS),
        _ => DEFAULT_WINDOW_DAYS,
    }
}

/// Returns the history summary used to rebuild or refresh a client's history.
///
/// This deliberately bypasses the interactive window cap: replacing the history
/// database requires every day on disk, and a partial rebuild would delete the
/// days it did not return (the client writes rows with `INSERT OR REPLACE`).
///
/// `days` narrows the window for callers that only need a recent slice — the
/// Windows client keeps 105 days — so a periodic refresh does not have to walk
/// the entire sessions tree. `None` means "everything on disk".
///
/// The window is clamped to `HISTORY_WINDOW_DAYS` and then handed to
/// `summarize`, which must not clamp it again: routing through the interactive
/// entry point would narrow every history request to `MAX_WINDOW_DAYS` and make
/// both the 105-day refresh and the rebuild silently return 90 days, deleting
/// the older history the rebuild is meant to preserve.
pub(crate) fn history_summary(days: Option<u32>) -> Summary {
    summarize(history_window_days(days))
}

/// Resolves the history window, which is bounded by `HISTORY_WINDOW_DAYS`
/// rather than the interactive `MAX_WINDOW_DAYS`.
///
/// Kept separate from `normalize_window_days` so the two ceilings cannot be
/// confused: the history window must stay wider than the interactive one, and
/// `None` means "everything on disk" rather than "the default recent slice".
pub(crate) fn history_window_days(days: Option<u32>) -> u32 {
    match days {
        Some(days) if days > 0 => days.min(HISTORY_WINDOW_DAYS),
        _ => HISTORY_WINDOW_DAYS,
    }
}

/// Returns the usage summary for the last `window_days` days.
///
/// The interactive endpoint is bounded by `MAX_WINDOW_DAYS`; history callers go
/// through `history_summary` instead.
pub(crate) fn summary(window_days: u32) -> Summary {
    summarize(normalize_window_days(Some(window_days)))
}

/// Computes and briefly memoises the summary for an already-clamped window.
///
/// Both entry points apply their own ceiling before calling this, so the window
/// is taken as final here. Clamping a second time is what previously collapsed
/// the history window onto the interactive cap.
fn summarize(window_days: u32) -> Summary {
    if let Ok(guard) = cache().lock()
        && let Some(entry) = guard.as_ref()
        && entry.window_days == window_days
        && entry.generated.elapsed() < CACHE_TTL
    {
        return entry.summary.clone();
    }

    let summary = compute(window_days);

    if let Ok(mut guard) = cache().lock() {
        *guard = Some(CacheEntry {
            window_days,
            generated: Instant::now(),
            summary: summary.clone(),
        });
    }
    summary
}

fn compute(window_days: u32) -> Summary {
    let generated_at_ms = now_ms();
    let roots = scan::sessions_roots();
    if roots.is_empty() {
        return Summary {
            window_days,
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
            generated_at_ms,
        };
    }

    // Live sessions and the archive both count: a rebuild replaces the day's
    // rows, so a session left out of the scan is history the client loses.
    let scans = scan_roots(&roots, window_days);
    let sessions: u32 = scans.len().min(u32::MAX as usize) as u32;
    let mut usage = TokenUsage::default();
    for (_, scan) in &scans {
        accumulate(&mut usage, scan.usage);
    }

    let days = scan::aggregate_by_day(&scans)
        .into_iter()
        .map(|day| DayUsage {
            date: day.date,
            usage: day.usage,
            sessions: day.sessions.min(u32::MAX as usize) as u32,
        })
        .collect();

    Summary {
        window_days,
        sessions,
        usage,
        days,
        hours: aggregate_hours(&scans),
        projects: aggregate_projects(&scans),
        models: aggregate_models(&scans),
        providers: aggregate_providers(&scans),
        breakdown: aggregate_breakdown(&scans),
        quota: latest_quota(&scans),
        minutes: aggregate_minutes(&scans),
        quota_history: aggregate_quota_history(&scans),
        quota_minutes: aggregate_quota_minutes(&scans),
        // The live tree, which is the one a caller would look at; the archive is
        // an implementation detail of "read every rollout on disk".
        sessions_root: roots
            .first()
            .map(|root| root.to_string_lossy().into_owned()),
        generated_at_ms,
    }
}

/// Rolls the per-session hour buckets up, newest hour first.
fn aggregate_hours(scans: &[(String, SessionScan)]) -> Vec<HourUsage> {
    let mut buckets: std::collections::BTreeMap<(String, u8), HourUsage> =
        std::collections::BTreeMap::new();
    for (date, scan) in scans {
        for (hour, usage) in &scan.hourly {
            let entry = buckets
                .entry((date.clone(), *hour))
                .or_insert_with(|| HourUsage {
                    date: date.clone(),
                    hour: *hour,
                    usage: TokenUsage::default(),
                    sessions: 0,
                });
            accumulate(&mut entry.usage, *usage);
            entry.sessions += 1;
        }
    }
    let mut collected: Vec<HourUsage> = buckets.into_values().collect();
    collected.sort_by(|left, right| {
        right
            .date
            .cmp(&left.date)
            .then_with(|| right.hour.cmp(&left.hour))
    });
    collected
}

/// Builds the cross-product rows the client history database is keyed by.
///
/// A session contributes one row per `(model, provider)` combination actually
/// used, and every session contributes: `model` and `provider` fall back to the
/// labels the clients use, and a session with no working directory is stored
/// under the empty project name, which is what their history databases do too
/// (`project: event.project ?? ""`).
///
/// Nothing is dropped here on purpose. These rows are a *replacement* for the
/// days they cover, so any session omitted is usage the client's history loses.
fn aggregate_breakdown(scans: &[(String, SessionScan)]) -> Vec<BreakdownRow> {
    let mut rows: std::collections::BTreeMap<(String, String, String, String), BreakdownRow> =
        std::collections::BTreeMap::new();
    for (day, scan) in scans {
        // A session with no recorded `cwd` still counts; the clients key it under
        // an empty project rather than dropping the day's usage.
        let project = scan.project().unwrap_or_default();
        for ((model, provider), usage) in &scan.by_model_provider {
            let key = (
                day.clone(),
                model.clone(),
                provider.clone(),
                project.to_owned(),
            );
            let row = rows.entry(key).or_insert_with(|| BreakdownRow {
                day: day.clone(),
                service: CODEX_SERVICE.to_owned(),
                source: provider.clone(),
                model: model.clone(),
                project: project.to_owned(),
                usage: TokenUsage::default(),
                cost_usd: 0.0,
            });
            accumulate(&mut row.usage, *usage);
        }
    }
    let mut collected: Vec<BreakdownRow> = rows.into_values().collect();
    for row in &mut collected {
        row.cost_usd = cost::cost_of(&row.model, row.usage);
    }
    collected.sort_by(|left, right| {
        left.day
            .cmp(&right.day)
            .then_with(|| left.model.cmp(&right.model))
            .then_with(|| left.source.cmp(&right.source))
            .then_with(|| left.project.cmp(&right.project))
    });
    collected
}

/// Collects minute-level quota readings, newest window only.
///
/// Narrows to the trailing `QUOTA_MINUTE_WINDOW` minutes relative to the newest
/// reading seen, rather than to wall-clock "now": the daemon may be reading logs
/// that are hours old, and anchoring on the reader's clock would empty the series.
fn aggregate_quota_minutes(scans: &[(String, SessionScan)]) -> Vec<QuotaPoint> {
    let mut merged: std::collections::BTreeMap<(i64, scan::QuotaKind), f64> =
        std::collections::BTreeMap::new();
    for (_, session) in scans {
        for (key, percent) in &session.quota_minutes {
            let entry = merged.entry(*key).or_insert(*percent);
            // A later session may report a lower percentage (after a reset); the
            // series is a trend, so the day's peak is what should survive.
            if *percent > *entry {
                *entry = *percent;
            }
        }
    }

    let Some(newest) = merged.keys().map(|(minute, _)| *minute).max() else {
        return Vec::new();
    };
    // Strictly greater: the window spans `newest` and the 59 minutes before it,
    // which is 60 samples. An inclusive bound would keep 61.
    let cutoff = newest - QUOTA_MINUTE_WINDOW;
    merged
        .into_iter()
        .filter(|((minute, _), _)| *minute > cutoff)
        .map(|((minute, kind), used_percent)| QuotaPoint {
            minute,
            kind: match kind {
                scan::QuotaKind::Primary => "primary",
                scan::QuotaKind::Secondary => "secondary",
            },
            used_percent,
        })
        .collect()
}

/// Collapses per-session quota readings into one point per `(day, kind)`.
///
/// Keeps the highest reading seen for a day: sessions are scanned independently
/// and the clients store a single value per day, so a later session reporting a
/// lower percentage (for example after a reset) must not overwrite the peak the
/// same day reached. Sessions are ordered by path, not by time, so "last one
/// read" would not be stable.
fn aggregate_quota_history(scans: &[(String, SessionScan)]) -> Vec<QuotaHistoryPoint> {
    let mut points: std::collections::BTreeMap<(String, scan::QuotaKind), f64> =
        std::collections::BTreeMap::new();
    for (_, session) in scans {
        for ((day, kind), percent) in &session.quota_history {
            let entry = points.entry((day.clone(), *kind)).or_insert(*percent);
            if *percent > *entry {
                *entry = *percent;
            }
        }
    }
    points
        .into_iter()
        .map(|((day, kind), used_percent)| QuotaHistoryPoint {
            day,
            kind: match kind {
                scan::QuotaKind::Primary => "primary",
                scan::QuotaKind::Secondary => "secondary",
            },
            used_percent,
        })
        .collect()
}

/// Merges per-session minute buckets, oldest minute first.
///
/// Buckets are keyed by absolute minute, so sessions from different days merge
/// correctly and a 24-hour window can span midnight.
fn aggregate_minutes(scans: &[(String, SessionScan)]) -> Vec<MinuteBucket> {
    let mut merged: std::collections::BTreeMap<i64, TokenUsage> = std::collections::BTreeMap::new();
    for (_, scan) in scans {
        for (minute, usage) in &scan.minutely {
            accumulate(merged.entry(*minute).or_default(), *usage);
        }
    }
    // Trimmed to the trailing window the clients use, anchored to the newest
    // minute observed rather than to now: a machine that has not run Codex today
    // still gets its last active minutes instead of an empty series.
    let cutoff = merged
        .keys()
        .next_back()
        .map(|newest| newest - MINUTE_WINDOW_MINUTES);
    merged
        .into_iter()
        .filter(|(minute, _)| cutoff.is_none_or(|cutoff| *minute > cutoff))
        .map(|(minute, usage)| MinuteBucket { minute, usage })
        .collect()
}

/// Most recent quota windows across all scanned sessions.
///
/// Picks the sample with the newest observation time that actually carries a
/// window, rather than the newest sample overall: a gateway-routed session can be
/// the most recent one while reporting no windows at all.
fn latest_quota(scans: &[(String, SessionScan)]) -> Option<QuotaSnapshot> {
    let mut best: Option<&SessionScan> = None;
    let mut best_observed: Option<i64> = None;
    for (_, scan) in scans {
        if !scan.rate_limits.has_window() {
            continue;
        }
        let observed = scan
            .quota_observed_at
            .as_deref()
            .and_then(log::timestamp_ms);
        let is_newer = match (best_observed, observed) {
            (None, _) => best.is_none(),
            (Some(previous), Some(current)) => current > previous,
            // An undated sample never displaces a dated one.
            (Some(_), None) => false,
        };
        if is_newer {
            best = Some(scan);
            best_observed = observed;
        }
    }
    let scan = best?;
    Some(QuotaSnapshot {
        primary: scan.rate_limits.primary.clone(),
        secondary: scan.rate_limits.secondary.clone(),
        plan_type: scan.rate_limits.plan_type.clone(),
        observed_at_ms: best_observed,
    })
}

/// Rolls usage up per model, highest usage first, with the estimated cost.
fn aggregate_models(scans: &[(String, SessionScan)]) -> Vec<ModelUsage> {
    let mut grouped: std::collections::BTreeMap<String, ModelUsage> =
        std::collections::BTreeMap::new();
    for (_, scan) in scans {
        for (model, usage) in &scan.models {
            let entry = grouped.entry(model.clone()).or_insert_with(|| ModelUsage {
                model: model.clone(),
                usage: TokenUsage::default(),
                sessions: 0,
                cost_usd: 0.0,
            });
            accumulate(&mut entry.usage, *usage);
            entry.sessions += 1;
        }
    }
    let mut collected: Vec<ModelUsage> = grouped.into_values().collect();
    // Cost is derived from the accumulated usage so it stays consistent with the
    // token figures even after per-session attribution.
    for entry in &mut collected {
        entry.cost_usd = cost::cost_of(&entry.model, entry.usage);
    }
    sort_by_usage_desc(&mut collected, |entry| &entry.usage, |entry| &entry.model);
    collected
}

/// Rolls usage up per provider, highest usage first.
fn aggregate_providers(scans: &[(String, SessionScan)]) -> Vec<ProviderUsage> {
    let mut grouped: std::collections::BTreeMap<String, ProviderUsage> =
        std::collections::BTreeMap::new();
    for (_, scan) in scans {
        for (provider, usage) in &scan.providers {
            let entry = grouped
                .entry(provider.clone())
                .or_insert_with(|| ProviderUsage {
                    provider: provider.clone(),
                    usage: TokenUsage::default(),
                    sessions: 0,
                });
            accumulate(&mut entry.usage, *usage);
            entry.sessions += 1;
        }
    }
    let mut collected: Vec<ProviderUsage> = grouped.into_values().collect();
    sort_by_usage_desc(
        &mut collected,
        |entry| &entry.usage,
        |entry| &entry.provider,
    );
    collected
}

/// Orders buckets by usage, breaking ties by name so output stays deterministic.
fn sort_by_usage_desc<T>(
    items: &mut [T],
    usage: impl Fn(&T) -> &TokenUsage,
    key: impl Fn(&T) -> &str,
) {
    items.sort_by(|left, right| {
        usage(right)
            .total_tokens
            .cmp(&usage(left).total_tokens)
            .then_with(|| key(left).cmp(key(right)))
    });
}

/// Rolls usage up per project, highest usage first. Sessions whose working
/// directory Codex did not record are omitted rather than bucketed as unknown.
fn aggregate_projects(scans: &[(String, SessionScan)]) -> Vec<ProjectUsage> {
    let mut buckets: std::collections::BTreeMap<String, ProjectUsage> =
        std::collections::BTreeMap::new();
    for (_, scan) in scans {
        let Some(project) = scan.project() else {
            continue;
        };
        let entry = buckets
            .entry(project.to_owned())
            .or_insert_with(|| ProjectUsage {
                project: project.to_owned(),
                usage: TokenUsage::default(),
                sessions: 0,
            });
        accumulate(&mut entry.usage, scan.usage);
        entry.sessions += 1;
    }
    let mut collected: Vec<ProjectUsage> = buckets.into_values().collect();
    collected.sort_by(|left, right| {
        right
            .usage
            .total_tokens
            .cmp(&left.usage.total_tokens)
            .then_with(|| left.project.cmp(&right.project))
    });
    collected
}

/// Scans every rollout file across `roots`, newest `window_days` first.
///
/// Each root is enumerated in the shape it actually has: the live tree nests
/// `<year>/<month>/<day>` directory pairs, while the archive is flat. The day is
/// resolved per file so both contribute to the same daily buckets.
fn scan_roots(roots: &[PathBuf], window_days: u32) -> Vec<(String, SessionScan)> {
    let mut scans = Vec::new();
    for root in roots {
        for (date, path) in rollout_files(root, window_days) {
            let Ok((scan, _)) = scan::scan_from(&path, 0) else {
                continue;
            };
            if scan.samples == 0 {
                continue;
            }
            // A flat archive file with no usable date in its name still has one
            // in its records; that is what the clients bucket by.
            let date = date
                .or_else(|| {
                    scan.meta
                        .started_at
                        .as_deref()
                        .and_then(scan::session_date_from_timestamp)
                })
                .unwrap_or_else(|| "unknown".to_owned());
            scans.push((date, scan));
        }
    }
    scans
}

/// Every rollout file in `root` that falls inside the newest `window_days`.
///
/// The window is applied by day directory when the tree has one. A flat archive
/// has no day directories to narrow, so every file is offered and the caller
/// buckets it by its own date; the file count there is small (54 locally).
fn rollout_files(root: &Path, window_days: u32) -> Vec<(Option<String>, PathBuf)> {
    let directories = newest_day_directories(root, window_days);
    if directories.is_empty() {
        // Flat layout: the root itself holds the rollout files.
        return sorted_rollout_files(root)
            .into_iter()
            .map(|path| (scan::rollout_day_from_path(&path), path))
            .collect();
    }
    let mut files = Vec::new();
    for directory in directories {
        // A day directory already names the day; the files inside it do not need
        // to be re-derived individually.
        let date = day_directory_date(&directory);
        for path in sorted_rollout_files(&directory) {
            files.push((date.clone(), path));
        }
    }
    files
}

/// Sorted rollout files directly inside `directory`.
fn sorted_rollout_files(directory: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| scan::is_rollout_file(path))
        .collect();
    // Deterministic order keeps aggregation stable between runs.
    files.sort();
    files
}

/// Reads `YYYY-MM-DD` out of a `<year>/<month>/<day>` directory path.
fn day_directory_date(directory: &Path) -> Option<String> {
    let day = directory.file_name()?.to_str()?;
    let month = directory.parent()?.file_name()?.to_str()?;
    let year = directory.parent()?.parent()?.file_name()?.to_str()?;
    Some(format!("{year}-{month}-{day}"))
}

/// Returns the newest day directories, deepest date last, capped at `limit`.
fn newest_day_directories(root: &Path, limit: u32) -> Vec<PathBuf> {
    let mut days: Vec<PathBuf> = Vec::new();
    for year in sorted_directories(root) {
        for month in sorted_directories(&year) {
            for day in sorted_directories(&month) {
                days.push(day);
            }
        }
    }
    // `<year>/<month>/<day>` sorts chronologically as plain strings.
    days.sort();
    let limit = limit.max(1) as usize;
    if days.len() > limit {
        days.drain(..days.len() - limit);
    }
    days
}

fn sorted_directories(parent: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut names: Vec<PathBuf> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .collect();
    names.sort();
    names
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn write_day(root: &Path, date: &str, totals: &[u64]) {
        let directory = root.join(date.replace('-', "/"));
        std::fs::create_dir_all(&directory).expect("create day directory");
        for (index, total) in totals.iter().enumerate() {
            let file = directory.join(format!("rollout-{date}T0{index}-00-00-x.jsonl"));
            let mut handle = std::fs::File::create(&file).expect("create rollout");
            writeln!(
                handle,
                r#"{{"type":"session_meta","payload":{{"session_id":"s{index}","cwd":"/p","originator":"Codex Desktop"}}}}"#
            )
            .expect("write meta");
            writeln!(
                handle,
                r#"{{"type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":{total},"output_tokens":1,"total_tokens":{}}}}}}}}}"#,
                total + 1
            )
            .expect("write token count");
            handle.flush().expect("flush");
        }
    }

    /// Writes one rollout file directly into `directory`, the flat archive shape.
    fn write_flat(root: &Path, date: &str, total: u64) {
        std::fs::create_dir_all(root).expect("create archive directory");
        let file = root.join(format!("rollout-{date}T01-00-00-x.jsonl"));
        let mut handle = std::fs::File::create(&file).expect("create rollout");
        writeln!(
            handle,
            r#"{{"type":"session_meta","payload":{{"session_id":"s-{date}","cwd":"/p"}}}}"#
        )
        .expect("write meta");
        writeln!(
            handle,
            r#"{{"type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":{total},"output_tokens":1,"total_tokens":{}}}}}}}}}"#,
            total + 1
        )
        .expect("write token count");
        handle.flush().expect("flush");
    }

    /// The archive is flat, so its files have no day directory to bucket by.
    ///
    /// Its day has to come from the `rollout-<date>T…` file name. Before this was
    /// handled the whole tree was skipped, dropping every archived session from
    /// the summary — and, for a rebuild, from the client's history.
    #[test]
    fn scans_a_flat_archive_root_with_dates_from_file_names() {
        let live = tempfile::tempdir().expect("live dir");
        write_day(live.path(), "2026-08-25", &[30]);

        let archive = tempfile::tempdir().expect("archive dir");
        write_flat(archive.path(), "2026-08-24", 20);
        write_flat(archive.path(), "2026-08-20", 10);

        let roots = vec![live.path().to_path_buf(), archive.path().to_path_buf()];
        let scans = scan_roots(&roots, 7);

        assert_eq!(scans.len(), 3, "all three sessions have to be scanned");
        let dates: Vec<&str> = scans.iter().map(|(date, _)| date.as_str()).collect();
        assert!(
            dates.contains(&"2026-08-25"),
            "live day kept its directory date"
        );
        assert!(
            dates.contains(&"2026-08-24"),
            "archived file used its file-name date"
        );
        assert!(dates.contains(&"2026-08-20"));
    }

    /// A file with no date in its path still lands in the right bucket.
    ///
    /// `session_meta` carries the session timestamp (all 54 local archived files
    /// have one), so the day is recoverable even when neither the directory nor
    /// the file name provides it.
    #[test]
    fn falls_back_to_the_records_timestamp_when_the_path_has_no_date() {
        let archive = tempfile::tempdir().expect("archive dir");
        // No date in the name, so only the records can place it.
        let file = archive.path().join("rollout-unknown-session.jsonl");
        let mut handle = std::fs::File::create(&file).expect("create rollout");
        writeln!(
            handle,
            r#"{{"timestamp":"2026-08-22T03:04:05.000Z","type":"session_meta","payload":{{"session_id":"s1","cwd":"/p"}}}}"#
        )
        .expect("write meta");
        writeln!(
            handle,
            r#"{{"timestamp":"2026-08-22T03:04:06.000Z","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":5,"output_tokens":1,"total_tokens":6}}}}}}}}"#
        )
        .expect("write token count");
        handle.flush().expect("flush");

        let roots = vec![archive.path().to_path_buf()];
        let scans = scan_roots(&roots, 7);

        assert_eq!(scans.len(), 1);
        assert_eq!(
            scans[0].0, "2026-08-22",
            "the record timestamp places the session when the path cannot"
        );
    }

    /// Day directories are picked by name, so an older day must not leak into a
    /// narrow window even though it sorts before the newest ones.
    #[test]
    fn window_keeps_only_the_newest_days() {
        let root = tempfile::tempdir().expect("temp dir");
        write_day(root.path(), "2026-08-20", &[10]);
        write_day(root.path(), "2026-08-24", &[20]);
        write_day(root.path(), "2026-08-25", &[30, 40]);

        let directories = newest_day_directories(root.path(), 2);
        assert_eq!(directories.len(), 2);
        assert!(directories[0].ends_with("2026/08/24"));
        assert!(directories[1].ends_with("2026/08/25"));
    }

    /// Sessions are scanned in path order, not time order, so the service layer
    /// must collapse a day deterministically: it keeps the day's peak, matching
    /// what the clients store (one percentage per day).
    #[test]
    fn quota_history_keeps_the_daily_peak_across_sessions() {
        let mut first = SessionScan::default();
        first
            .quota_history
            .insert(("2026-08-25".to_owned(), scan::QuotaKind::Primary), 42.0);
        let mut second = SessionScan::default();
        second
            .quota_history
            .insert(("2026-08-25".to_owned(), scan::QuotaKind::Primary), 7.0);
        let mut other_day = SessionScan::default();
        other_day
            .quota_history
            .insert(("2026-08-24".to_owned(), scan::QuotaKind::Primary), 11.0);

        let scans = vec![
            ("2026-08-25".to_owned(), second),
            ("2026-08-24".to_owned(), other_day),
            ("2026-08-25".to_owned(), first),
        ];

        let history = aggregate_quota_history(&scans);
        assert_eq!(history.len(), 2);
        // Oldest day first, and the peak survives regardless of scan order.
        assert_eq!(history[0].day, "2026-08-24");
        assert!((history[0].used_percent - 11.0).abs() < 1e-9);
        assert_eq!(history[1].day, "2026-08-25");
        assert!((history[1].used_percent - 42.0).abs() < 1e-9);
        assert_eq!(history[1].kind, "primary");
    }

    /// The series is narrowed to the trailing hour relative to the newest reading,
    /// not to wall-clock now: a daemon reading day-old logs must still return the
    /// samples it has.
    #[test]
    fn quota_minutes_keep_only_the_trailing_window() {
        let mut session = SessionScan::default();
        for minute in 0..200 {
            session
                .quota_minutes
                .insert((minute, scan::QuotaKind::Primary), 1.0);
        }
        let scans = vec![("2026-08-25".to_owned(), session)];

        let points = aggregate_quota_minutes(&scans);
        assert_eq!(points.len(), QUOTA_MINUTE_WINDOW as usize);
        // Newest reading is minute 199, so the window starts at 140.
        assert_eq!(points.first().map(|point| point.minute), Some(140));
        assert_eq!(points.last().map(|point| point.minute), Some(199));
        assert_eq!(points[0].kind, "primary");
    }

    /// A lower reading from a later session must not overwrite the peak: the
    /// series is a trend, and sessions are scanned in path order.
    #[test]
    fn quota_minutes_keep_the_peak_for_a_minute() {
        let mut high = SessionScan::default();
        high.quota_minutes
            .insert((100, scan::QuotaKind::Primary), 42.0);
        let mut low = SessionScan::default();
        low.quota_minutes
            .insert((100, scan::QuotaKind::Primary), 7.0);

        let scans = vec![
            ("2026-08-25".to_owned(), low),
            ("2026-08-25".to_owned(), high),
        ];
        let points = aggregate_quota_minutes(&scans);
        assert_eq!(points.len(), 1);
        assert!((points[0].used_percent - 42.0).abs() < 1e-9);
    }

    #[test]
    fn quota_minutes_are_empty_without_samples() {
        let scans = vec![("2026-08-25".to_owned(), SessionScan::default())];
        assert!(aggregate_quota_minutes(&scans).is_empty());
    }

    /// The minute series is trimmed to the trailing window the clients use.
    ///
    /// It used to cover the whole requested window, so a 105-day refresh shipped
    /// every minute on disk (measured: 27547 buckets) while the clients kept only
    /// the last day. The window is anchored to the newest minute present, so a
    /// daemon reading older logs still returns its most recent activity.
    #[test]
    fn minute_buckets_keep_only_the_trailing_window() {
        let mut session = SessionScan::default();
        // Two minutes past the window, then the newest minute.
        let oldest = 1_000_000;
        let newest = oldest + MINUTE_WINDOW_MINUTES + 5;
        session.minutely.insert(
            oldest,
            TokenUsage {
                total_tokens: 7,
                ..TokenUsage::default()
            },
        );
        session.minutely.insert(
            newest,
            TokenUsage {
                total_tokens: 11,
                ..TokenUsage::default()
            },
        );
        let scans = vec![("2026-08-25".to_owned(), session)];

        let buckets = aggregate_minutes(&scans);
        assert_eq!(buckets.len(), 1, "only the in-window minute survives");
        assert_eq!(buckets[0].minute, newest);
        assert_eq!(buckets[0].usage.total_tokens, 11);
        // Oldest first, which is what the clients iterate.
        let mut all = SessionScan::default();
        for offset in 0..MINUTE_WINDOW_MINUTES {
            all.minutely.insert(newest - offset, TokenUsage::default());
        }
        let scans = vec![("2026-08-25".to_owned(), all)];
        let buckets = aggregate_minutes(&scans);
        assert_eq!(buckets.len(), MINUTE_WINDOW_MINUTES as usize);
        assert!(
            buckets
                .windows(2)
                .all(|pair| pair[0].minute < pair[1].minute)
        );
    }

    #[test]
    fn minute_buckets_are_empty_without_samples() {
        let scans = vec![("2026-08-25".to_owned(), SessionScan::default())];
        assert!(aggregate_minutes(&scans).is_empty());
    }

    #[test]
    fn normalize_window_clamps_to_supported_range() {
        assert_eq!(normalize_window_days(None), DEFAULT_WINDOW_DAYS);
        assert_eq!(normalize_window_days(Some(0)), DEFAULT_WINDOW_DAYS);
        assert_eq!(normalize_window_days(Some(3)), 3);
        assert_eq!(normalize_window_days(Some(10_000)), MAX_WINDOW_DAYS);
    }

    /// The history window must not be collapsed onto the interactive cap.
    ///
    /// `history_summary` used to clamp to `HISTORY_WINDOW_DAYS` and then call
    /// `summary`, which clamped again to `MAX_WINDOW_DAYS`; both the 105-day
    /// refresh and the "everything on disk" rebuild therefore returned 90 days,
    /// and a rebuild deletes the days it did not get back.
    #[test]
    fn history_window_bypasses_the_interactive_cap() {
        assert_eq!(history_window_days(None), HISTORY_WINDOW_DAYS);
        assert_eq!(history_window_days(Some(0)), HISTORY_WINDOW_DAYS);
        assert!(
            HISTORY_WINDOW_DAYS > MAX_WINDOW_DAYS,
            "the history window has to stay wider than the interactive one"
        );

        // Windows keeps 105 days; that request must survive intact.
        assert_eq!(history_window_days(Some(105)), 105);
        assert_ne!(
            history_window_days(Some(105)),
            normalize_window_days(Some(105)),
            "105 days must not be narrowed to the interactive cap"
        );

        // Only the interactive ceiling is 90 days.
        assert_eq!(normalize_window_days(Some(105)), MAX_WINDOW_DAYS);

        // An over-wide history request is still bounded, but by its own limit.
        assert_eq!(history_window_days(Some(u32::MAX)), HISTORY_WINDOW_DAYS);
    }

    /// A missing installation is a normal state, not an error: the clients show
    /// "no local Codex data" rather than failing the request.
    #[test]
    fn breakdown_keeps_sessions_without_a_project() {
        // A session whose logs never recorded a working directory. The macOS
        // history database stores these under `project = ""` (its local rebuild
        // writes `event.project ?? ""`), so the daemon has to emit them too:
        // dropping the session would lose its usage from the rows a rebuild
        // replaces, and `windowed_projects` on the Windows side already skips
        // empty names when ranking.
        let mut no_project = SessionScan::default();
        no_project.samples = 1;
        no_project.usage.total_tokens = 100;
        no_project.meta.session_id = Some("no-cwd".to_owned());
        no_project
            .by_model_provider
            .insert(("codex".to_owned(), "legacy".to_owned()), no_project.usage);

        let scans = vec![("2026-08-25".to_owned(), no_project)];
        let rows = aggregate_breakdown(&scans);

        assert_eq!(rows.len(), 1, "the session still has to produce a row");
        assert_eq!(rows[0].project, "");
        assert_eq!(rows[0].model, "codex");
        assert_eq!(rows[0].source, "legacy");
        assert_eq!(rows[0].usage.total_tokens, 100);
    }

    /// A missing installation is a normal state, not an error: the clients show
    /// "no local Codex data" rather than failing the request.
    #[test]
    fn missing_installation_reports_no_root() {
        // Point CODEX_HOME at an empty directory so the lookup cannot succeed.
        let empty = tempfile::tempdir().expect("temp dir");
        let previous_codex_home = std::env::var_os("CODEX_HOME");
        let previous_home = std::env::var_os("HOME");
        // SAFETY: this test does not run concurrently with other tests that read
        // these variables, and both are restored before returning.
        unsafe {
            std::env::set_var("CODEX_HOME", empty.path());
            std::env::set_var("HOME", empty.path());
        }
        let summary = summary(DEFAULT_WINDOW_DAYS);
        unsafe {
            match previous_codex_home {
                Some(value) => std::env::set_var("CODEX_HOME", value),
                None => std::env::remove_var("CODEX_HOME"),
            }
            match previous_home {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
        }
        assert!(summary.sessions_root.is_none());
        assert_eq!(summary.sessions, 0);
        assert_eq!(summary.usage.total_tokens, 0);
    }
}
