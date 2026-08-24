use std::{
    collections::{BTreeMap, HashMap, HashSet},
    env, fs,
    io::{BufRead, BufReader, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use chrono::{DateTime, Days, Local, NaiveDate, Utc};
use serde::Serialize;
use serde_json::Value;

mod history;

use history::UsageHistoryStore;

const TREND_DAYS: u64 = 7;
const PROJECT_DAYS: i64 = 7;
const HISTORY_DAYS: u64 = 105;
const RECENT_DAYS: u64 = HISTORY_DAYS + 1;
const ACTIVITY_WINDOW_MINUTES: i64 = 3;
const BURN_WINDOW_MINUTES: i64 = 10;
const QUOTA_HISTORY_MINUTES: i64 = 60;
const MAX_DIRECTORY_DEPTH: usize = 8;
const MAX_FILES: usize = 20_000;
const MAX_BYTES_PER_FILE_PER_REFRESH: u64 = 128 * 1024 * 1024;
const MAX_JSON_LINE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Default)]
pub struct CodexUsageState {
    collector: Arc<Mutex<UsageCollector>>,
}

#[derive(Debug, Clone)]
struct UsageEvent {
    timestamp: DateTime<chrono::FixedOffset>,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    project: Option<String>,
}

#[derive(Debug, Clone)]
struct QuotaSample {
    timestamp_ms: i64,
    windows: Vec<CodexQuotaWindow>,
}

impl UsageEvent {
    fn total_tokens(&self) -> u64 {
        self.input_tokens
            .saturating_add(self.output_tokens)
            .saturating_add(self.cache_read_tokens)
    }

    fn estimated_cost_usd(&self) -> f64 {
        // Same API-equivalent Codex fallback rates as the macOS dashboard.
        // Codex subscriptions are flat-rate; this remains an estimate only.
        (self.input_tokens as f64 * 1.5
            + self.output_tokens as f64 * 12.0
            + self.cache_read_tokens as f64 * 0.15)
            / 1_000_000.0
    }
}

#[derive(Default)]
struct FileUsage {
    offset: u64,
    length: u64,
    project: Option<String>,
    events: Vec<UsageEvent>,
    utc_daily_tokens: BTreeMap<NaiveDate, u64>,
    latest_quota: Option<QuotaSample>,
    quota_history: Vec<QuotaSample>,
}

#[derive(Default)]
struct UsageCollector {
    files: HashMap<PathBuf, FileUsage>,
    history: UsageHistoryStore,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageDay {
    day: String,
    tokens: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageProject {
    project: String,
    tokens: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexQuotaWindow {
    kind: String,
    used_percent: f64,
    resets_at_ms: Option<i64>,
    depletion_eta_ms: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexWeeklyReport {
    last_week_tokens: u64,
    last_week_cost_usd: f64,
    previous_week_tokens: u64,
    last_week_top_project: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexUsageSnapshot {
    available: bool,
    source_directory: String,
    scanned_files: usize,
    today_tokens: u64,
    today_requests: usize,
    yesterday_tokens: u64,
    yesterday_cost_usd: f64,
    yesterday_top_project: Option<String>,
    tokens_per_minute: f64,
    burn_rate_tokens_per_minute: f64,
    active_baseline_tokens_per_minute: f64,
    estimated_cost_usd: f64,
    quota_windows: Vec<CodexQuotaWindow>,
    seven_day: Vec<UsageDay>,
    daily_usage: Vec<UsageDay>,
    seven_day_projects: Vec<UsageProject>,
    top_project: Option<String>,
    streak_days: u64,
    previous_best_daily_tokens: Option<u64>,
    weekly_report: Option<CodexWeeklyReport>,
    last_activity_at_ms: Option<i64>,
    updated_at_ms: u64,
}

pub async fn snapshot(state: &CodexUsageState) -> Result<CodexUsageSnapshot, String> {
    let collector = Arc::clone(&state.collector);
    tauri::async_runtime::spawn_blocking(move || {
        let root = codex_sessions_root();
        let mut collector = collector
            .lock()
            .map_err(|_| "Codex 用量采集状态不可用".to_string())?;
        collector.collect(&root, Local::now())
    })
    .await
    .map_err(|error| format!("Codex 用量采集任务失败：{error}"))?
}

fn codex_sessions_root() -> PathBuf {
    if let Some(home) = env::var_os("CODEX_HOME").filter(|value| !value.is_empty()) {
        return PathBuf::from(home).join("sessions");
    }
    env::var_os("USERPROFILE")
        .or_else(|| env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".codex")
        .join("sessions")
}

impl UsageCollector {
    fn collect(&mut self, root: &Path, now: DateTime<Local>) -> Result<CodexUsageSnapshot, String> {
        let updated_at_ms = system_time_ms(SystemTime::now());
        if !root.is_dir() {
            self.files.clear();
            return Ok(empty_snapshot(root, updated_at_ms));
        }

        // Seed the durable daily history from every discoverable session log
        // once. Later launches need only the bounded recent tail because old
        // UTC-day totals have already been persisted independently of logs.
        let full_backfill = self.history.needs_full_backfill();
        let cutoff_time = if full_backfill {
            UNIX_EPOCH
        } else {
            SystemTime::now()
                .checked_sub(Duration::from_secs(RECENT_DAYS * 24 * 60 * 60))
                .unwrap_or(UNIX_EPOCH)
        };
        let mut discovered = Vec::new();
        // The initial durable-history seed must inspect every discoverable
        // session file. Normal refreshes remain bounded to avoid monopolizing
        // the collector on very large or damaged profiles.
        let file_limit = (!full_backfill).then_some(MAX_FILES);
        collect_recent_jsonl(root, cutoff_time, 0, file_limit, &mut discovered)?;
        discovered.sort();
        if let Some(limit) = file_limit {
            discovered.truncate(limit);
        }
        let active = discovered.iter().cloned().collect::<HashSet<_>>();
        self.files.retain(|path, _| active.contains(path));

        for path in &discovered {
            let Ok(metadata) = fs::metadata(path) else {
                continue;
            };
            let length = metadata.len();
            let state = self.files.entry(path.clone()).or_default();
            if length < state.offset || (length == state.offset && length != state.length) {
                state.offset = 0;
                state.events.clear();
                state.utc_daily_tokens.clear();
                state.project = None;
                state.latest_quota = None;
                state.quota_history.clear();
            }
            state.length = length;
            read_increment(path, state)?;
        }

        let full_backfill_complete = !full_backfill
            || discovered.iter().all(|path| {
                self.files
                    .get(path)
                    .is_some_and(|state| state.offset >= state.length)
            });

        self.build_snapshot(
            root,
            now,
            updated_at_ms,
            full_backfill,
            full_backfill_complete,
        )
    }

    fn build_snapshot(
        &mut self,
        root: &Path,
        now: DateTime<Local>,
        updated_at_ms: u64,
        full_backfill: bool,
        full_backfill_complete: bool,
    ) -> Result<CodexUsageSnapshot, String> {
        let today = now.date_naive();
        let yesterday = today.checked_sub_days(Days::new(1)).unwrap_or(today);
        let utc_today = now.with_timezone(&Utc).date_naive();
        let last_week_start = utc_today
            .checked_sub_days(Days::new(7))
            .unwrap_or(utc_today);
        let previous_week_start = last_week_start
            .checked_sub_days(Days::new(7))
            .unwrap_or(last_week_start);
        let earliest_history_day = today
            .checked_sub_days(Days::new(HISTORY_DAYS - 1))
            .unwrap_or(today);
        let persisted_replace_from = utc_today
            .checked_sub_days(Days::new(RECENT_DAYS - 1))
            .unwrap_or(utc_today);
        let activity_cutoff = now
            .timestamp_millis()
            .saturating_sub(ACTIVITY_WINDOW_MINUTES * 60 * 1000);
        let burn_cutoff = now
            .timestamp_millis()
            .saturating_sub(BURN_WINDOW_MINUTES * 60 * 1000);
        let baseline_cutoff = now.timestamp_millis().saturating_sub(24 * 60 * 60 * 1000);
        let project_cutoff = now
            .timestamp_millis()
            .saturating_sub(PROJECT_DAYS * 24 * 60 * 60 * 1000);
        let history_cutoff = now
            .timestamp_millis()
            .saturating_sub((RECENT_DAYS * 86_400_000) as i64);
        let mut today_tokens = 0_u64;
        let mut today_requests = 0_usize;
        let mut yesterday_tokens = 0_u64;
        let mut yesterday_cost_usd = 0_f64;
        let mut yesterday_projects = HashMap::<String, u64>::new();
        let mut activity_tokens = 0_u64;
        let mut burn_tokens = 0_u64;
        let mut estimated_cost_usd = 0_f64;
        let mut daily = BTreeMap::<NaiveDate, u64>::new();
        let mut observed_utc_daily = BTreeMap::<NaiveDate, u64>::new();
        let mut observed_source_daily = BTreeMap::<String, BTreeMap<NaiveDate, u64>>::new();
        let mut projects = HashMap::<String, u64>::new();
        let mut seven_day_projects = HashMap::<String, u64>::new();
        let mut last_week_tokens = 0_u64;
        let mut last_week_cost_usd = 0_f64;
        let mut previous_week_tokens = 0_u64;
        let mut last_week_projects = HashMap::<String, u64>::new();
        let mut last_activity_at_ms = None;
        let mut active_minutes = HashMap::<i64, u64>::new();

        for (path, state) in self.files.iter_mut() {
            observed_source_daily.insert(
                path.to_string_lossy().into_owned(),
                state.utc_daily_tokens.clone(),
            );
            for (day, tokens) in &state.utc_daily_tokens {
                if full_backfill || *day >= persisted_replace_from {
                    let total = observed_utc_daily.entry(*day).or_default();
                    *total = total.saturating_add(*tokens);
                }
            }
            for event in &state.events {
                let timestamp_ms = event.timestamp.timestamp_millis();
                let local_day = event.timestamp.with_timezone(&Local).date_naive();
                let utc_day = event.timestamp.with_timezone(&Utc).date_naive();
                let tokens = event.total_tokens();
                // Durable UTC totals above intentionally see the complete
                // first-pass history. Dashboard aggregates stay bounded.
                if timestamp_ms < history_cutoff {
                    continue;
                }
                if utc_day >= last_week_start && utc_day < utc_today {
                    last_week_tokens = last_week_tokens.saturating_add(tokens);
                    last_week_cost_usd += event.estimated_cost_usd();
                    if let Some(project) = event.project.as_ref() {
                        let total = last_week_projects.entry(project.clone()).or_default();
                        *total = total.saturating_add(tokens);
                    }
                } else if utc_day >= previous_week_start && utc_day < last_week_start {
                    previous_week_tokens = previous_week_tokens.saturating_add(tokens);
                }
                if local_day >= earliest_history_day && local_day <= today {
                    let total = daily.entry(local_day).or_default();
                    *total = total.saturating_add(tokens);
                }
                if local_day == today {
                    today_tokens = today_tokens.saturating_add(tokens);
                    today_requests = today_requests.saturating_add(1);
                    estimated_cost_usd += event.estimated_cost_usd();
                    if let Some(project) = event.project.as_ref() {
                        let total = projects.entry(project.clone()).or_default();
                        *total = total.saturating_add(tokens);
                    }
                }
                if timestamp_ms >= project_cutoff && timestamp_ms <= now.timestamp_millis() {
                    if let Some(project) = event.project.as_ref() {
                        let total = seven_day_projects.entry(project.clone()).or_default();
                        *total = total.saturating_add(tokens);
                    }
                }
                if local_day == yesterday {
                    yesterday_tokens = yesterday_tokens.saturating_add(tokens);
                    yesterday_cost_usd += event.estimated_cost_usd();
                    if let Some(project) = event.project.as_ref() {
                        let total = yesterday_projects.entry(project.clone()).or_default();
                        *total = total.saturating_add(tokens);
                    }
                }
                if timestamp_ms >= activity_cutoff && timestamp_ms <= now.timestamp_millis() {
                    activity_tokens = activity_tokens.saturating_add(tokens);
                }
                if timestamp_ms >= burn_cutoff && timestamp_ms <= now.timestamp_millis() {
                    burn_tokens = burn_tokens.saturating_add(tokens);
                }
                if timestamp_ms >= baseline_cutoff && timestamp_ms <= now.timestamp_millis() {
                    let total = active_minutes.entry(timestamp_ms / 60_000).or_default();
                    *total = total.saturating_add(tokens);
                }
                last_activity_at_ms = Some(
                    last_activity_at_ms
                        .map_or(timestamp_ms, |current: i64| current.max(timestamp_ms)),
                );
            }
            state
                .events
                .retain(|event| event.timestamp.timestamp_millis() >= history_cutoff);
        }

        self.history.replace_daily_tokens(
            &observed_utc_daily,
            &observed_source_daily,
            persisted_replace_from,
            full_backfill,
            full_backfill_complete,
        );

        let seven_day = (0..TREND_DAYS)
            .rev()
            .filter_map(|offset| today.checked_sub_days(Days::new(offset)))
            .map(|day| UsageDay {
                day: day.format("%Y-%m-%d").to_string(),
                tokens: daily.get(&day).copied().unwrap_or_default(),
            })
            .collect();
        let daily_usage = (0..HISTORY_DAYS)
            .rev()
            .filter_map(|offset| today.checked_sub_days(Days::new(offset)))
            .map(|day| UsageDay {
                day: day.format("%Y-%m-%d").to_string(),
                tokens: daily.get(&day).copied().unwrap_or_default(),
            })
            .collect();
        let mut seven_day_projects = seven_day_projects
            .into_iter()
            .map(|(project, tokens)| UsageProject { project, tokens })
            .collect::<Vec<_>>();
        seven_day_projects.sort_by(|left, right| {
            right
                .tokens
                .cmp(&left.tokens)
                .then_with(|| left.project.cmp(&right.project))
        });
        let top_project = projects
            .into_iter()
            .max_by(|left, right| left.1.cmp(&right.1).then_with(|| right.0.cmp(&left.0)))
            .map(|entry| entry.0);
        let yesterday_top_project = yesterday_projects
            .into_iter()
            .max_by(|left, right| left.1.cmp(&right.1).then_with(|| right.0.cmp(&left.0)))
            .map(|entry| entry.0);
        let streak_days = self.history.streak_days(utc_today);
        let previous_best_daily_tokens = self.history.previous_best_daily_tokens(utc_today);
        let weekly_report = (last_week_tokens > 0).then(|| CodexWeeklyReport {
            last_week_tokens,
            last_week_cost_usd,
            previous_week_tokens,
            last_week_top_project: last_week_projects
                .into_iter()
                .max_by(|left, right| left.1.cmp(&right.1).then_with(|| right.0.cmp(&left.0)))
                .map(|entry| entry.0),
        });
        let active_baseline_tokens_per_minute = if active_minutes.is_empty() {
            0.0
        } else {
            active_minutes.values().copied().sum::<u64>() as f64 / active_minutes.len() as f64
        };
        let mut quota_windows = self
            .files
            .values()
            .filter_map(|state| state.latest_quota.as_ref())
            .max_by_key(|sample| sample.timestamp_ms)
            .map(|sample| sample.windows.clone())
            .unwrap_or_default();
        for window in &quota_windows {
            self.history
                .record_quota_percent(utc_today, &window.kind, window.used_percent);
        }

        let quota_history_cutoff = now
            .timestamp_millis()
            .saturating_sub(QUOTA_HISTORY_MINUTES * 60 * 1000);
        let mut percent_history = HashMap::<String, BTreeMap<i64, f64>>::new();
        for sample in self.files.values().flat_map(|state| &state.quota_history) {
            if sample.timestamp_ms < quota_history_cutoff
                || sample.timestamp_ms > now.timestamp_millis()
            {
                continue;
            }
            for window in &sample.windows {
                percent_history
                    .entry(window.kind.clone())
                    .or_default()
                    .insert(sample.timestamp_ms, window.used_percent);
            }
        }
        for window in &mut quota_windows {
            window.depletion_eta_ms = match window.kind.as_str() {
                "session5h" => percent_history.get(&window.kind).and_then(|samples| {
                    estimate_session_depletion(
                        &samples
                            .iter()
                            .map(|(timestamp, percent)| (*timestamp, *percent))
                            .collect::<Vec<_>>(),
                        window.resets_at_ms,
                        now.timestamp_millis(),
                    )
                }),
                "weekly" => self
                    .history
                    .weekly_daily_rate(&window.kind, utc_today, 8)
                    .and_then(|rate| {
                        estimate_weekly_depletion(
                            window.used_percent,
                            rate,
                            window.resets_at_ms,
                            now.timestamp_millis(),
                        )
                    }),
                _ => None,
            };
        }
        // History is an enhancement, not a reason to hide an otherwise valid
        // live snapshot if the local profile directory is temporarily locked.
        if !full_backfill || full_backfill_complete {
            let _ = self.history.persist_if_needed();
        }

        Ok(CodexUsageSnapshot {
            available: true,
            source_directory: root.to_string_lossy().into_owned(),
            scanned_files: self.files.len(),
            today_tokens,
            today_requests,
            yesterday_tokens,
            yesterday_cost_usd,
            yesterday_top_project,
            tokens_per_minute: activity_tokens as f64 / ACTIVITY_WINDOW_MINUTES as f64,
            burn_rate_tokens_per_minute: burn_tokens as f64 / BURN_WINDOW_MINUTES as f64,
            active_baseline_tokens_per_minute,
            estimated_cost_usd,
            quota_windows,
            seven_day,
            daily_usage,
            seven_day_projects,
            top_project,
            streak_days,
            previous_best_daily_tokens,
            weekly_report,
            last_activity_at_ms,
            updated_at_ms,
        })
    }
}

/// Least-squares 5-hour quota regression, matching the macOS estimator:
/// at least three samples, at least five minutes of span, and a slope above
/// 0.05 percentage points per minute. Only pre-reset depletion is published.
fn estimate_session_depletion(
    samples: &[(i64, f64)],
    resets_at_ms: Option<i64>,
    now_ms: i64,
) -> Option<i64> {
    let reset = resets_at_ms.filter(|reset| *reset > now_ms)?;
    if samples.len() < 3 {
        return None;
    }
    let mut sorted = samples
        .iter()
        .copied()
        .filter(|(_, percent)| percent.is_finite())
        .collect::<Vec<_>>();
    if sorted.len() < 3 {
        return None;
    }
    sorted.sort_by_key(|sample| sample.0);
    let first_timestamp = sorted.first()?.0;
    let last_timestamp = sorted.last()?.0;
    if last_timestamp.saturating_sub(first_timestamp) < 5 * 60 * 1000 {
        return None;
    }

    let xs = sorted
        .iter()
        .map(|sample| (sample.0 - first_timestamp) as f64 / 60_000.0)
        .collect::<Vec<_>>();
    let ys = sorted.iter().map(|sample| sample.1).collect::<Vec<_>>();
    let count = sorted.len() as f64;
    let sum_x = xs.iter().sum::<f64>();
    let sum_y = ys.iter().sum::<f64>();
    let sum_xy = xs.iter().zip(&ys).map(|(x, y)| x * y).sum::<f64>();
    let sum_xx = xs.iter().map(|x| x * x).sum::<f64>();
    let denominator = count * sum_xx - sum_x * sum_x;
    if !denominator.is_finite() || denominator.abs() < f64::EPSILON {
        return None;
    }
    let slope = (count * sum_xy - sum_x * sum_y) / denominator;
    if !slope.is_finite() || slope <= 0.05 {
        return None;
    }

    let remaining = 100.0 - *ys.last()?;
    if !remaining.is_finite() || remaining <= 0.0 {
        return None;
    }
    let eta = now_ms as f64 + (remaining / slope) * 60_000.0;
    if !eta.is_finite() || eta <= now_ms as f64 || eta >= reset as f64 {
        return None;
    }
    Some(eta.round() as i64)
}

fn estimate_weekly_depletion(
    current_percent: f64,
    rate_percent_per_day: f64,
    resets_at_ms: Option<i64>,
    now_ms: i64,
) -> Option<i64> {
    let reset = resets_at_ms.filter(|reset| *reset > now_ms)?;
    if !current_percent.is_finite()
        || !rate_percent_per_day.is_finite()
        || rate_percent_per_day <= 0.5
    {
        return None;
    }
    let remaining = 100.0 - current_percent;
    if remaining <= 0.0 {
        return None;
    }
    let eta = now_ms as f64 + (remaining / rate_percent_per_day) * 86_400_000.0;
    if !eta.is_finite() || eta <= now_ms as f64 || eta >= reset as f64 {
        return None;
    }
    Some(eta.round() as i64)
}

fn empty_snapshot(root: &Path, updated_at_ms: u64) -> CodexUsageSnapshot {
    let today = Local::now().date_naive();
    CodexUsageSnapshot {
        available: false,
        source_directory: root.to_string_lossy().into_owned(),
        scanned_files: 0,
        today_tokens: 0,
        today_requests: 0,
        yesterday_tokens: 0,
        yesterday_cost_usd: 0.0,
        yesterday_top_project: None,
        tokens_per_minute: 0.0,
        burn_rate_tokens_per_minute: 0.0,
        active_baseline_tokens_per_minute: 0.0,
        estimated_cost_usd: 0.0,
        quota_windows: Vec::new(),
        seven_day: (0..TREND_DAYS)
            .rev()
            .filter_map(|offset| today.checked_sub_days(Days::new(offset)))
            .map(|day| UsageDay {
                day: day.format("%Y-%m-%d").to_string(),
                tokens: 0,
            })
            .collect(),
        daily_usage: (0..HISTORY_DAYS)
            .rev()
            .filter_map(|offset| today.checked_sub_days(Days::new(offset)))
            .map(|day| UsageDay {
                day: day.format("%Y-%m-%d").to_string(),
                tokens: 0,
            })
            .collect(),
        seven_day_projects: Vec::new(),
        top_project: None,
        streak_days: 0,
        previous_best_daily_tokens: None,
        weekly_report: None,
        last_activity_at_ms: None,
        updated_at_ms,
    }
}

fn collect_recent_jsonl(
    directory: &Path,
    cutoff: SystemTime,
    depth: usize,
    file_limit: Option<usize>,
    output: &mut Vec<PathBuf>,
) -> Result<(), String> {
    if depth > MAX_DIRECTORY_DEPTH || file_limit.is_some_and(|limit| output.len() >= limit) {
        return Ok(());
    }
    let entries = fs::read_dir(directory)
        .map_err(|error| format!("无法读取 Codex 会话目录 {}：{error}", directory.display()))?;
    for entry in entries.flatten() {
        if file_limit.is_some_and(|limit| output.len() >= limit) {
            break;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            collect_recent_jsonl(&path, cutoff, depth + 1, file_limit, output)?;
            continue;
        }
        if !file_type.is_file()
            || path.extension().and_then(|value| value.to_str()) != Some("jsonl")
        {
            continue;
        }
        let is_recent = entry
            .metadata()
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .is_some_and(|modified| modified >= cutoff);
        if is_recent {
            output.push(path);
        }
    }
    Ok(())
}

fn read_increment(path: &Path, state: &mut FileUsage) -> Result<(), String> {
    let file = fs::File::open(path)
        .map_err(|error| format!("无法读取 Codex 会话日志 {}：{error}", path.display()))?;
    let mut reader = BufReader::new(file);
    reader
        .seek(SeekFrom::Start(state.offset))
        .map_err(|error| format!("无法定位 Codex 会话日志 {}：{error}", path.display()))?;
    let initial_offset = state.offset;
    let mut consumed = 0_u64;
    let mut line = Vec::new();
    while consumed < MAX_BYTES_PER_FILE_PER_REFRESH {
        line.clear();
        let bytes = reader
            .read_until(b'\n', &mut line)
            .map_err(|error| format!("无法解析 Codex 会话日志 {}：{error}", path.display()))?;
        if bytes == 0 || line.last() != Some(&b'\n') {
            break;
        }
        consumed = consumed.saturating_add(bytes as u64);
        state.offset = initial_offset.saturating_add(consumed);
        if line.len() > MAX_JSON_LINE_BYTES {
            continue;
        }
        if line.last() == Some(&b'\n') {
            line.pop();
        }
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        parse_line(&line, state);
    }
    Ok(())
}

fn parse_line(line: &[u8], state: &mut FileUsage) {
    let Ok(value) = serde_json::from_slice::<Value>(line) else {
        return;
    };
    if value.get("type").and_then(Value::as_str) == Some("session_meta") {
        if let Some(cwd) = value
            .pointer("/payload/cwd")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            state.project = project_name(cwd);
        }
        return;
    }
    if value.pointer("/payload/type").and_then(Value::as_str) != Some("token_count") {
        return;
    }
    let Some(timestamp) = value
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
    else {
        return;
    };
    let quota_windows = parse_quota_windows(&value, timestamp.timestamp_millis());
    if !quota_windows.is_empty() {
        let sample = QuotaSample {
            timestamp_ms: timestamp.timestamp_millis(),
            windows: quota_windows,
        };
        state.latest_quota = Some(sample.clone());
        state.quota_history.push(sample);
        let newest = state
            .quota_history
            .iter()
            .map(|sample| sample.timestamp_ms)
            .max()
            .unwrap_or(timestamp.timestamp_millis());
        let cutoff = newest.saturating_sub(QUOTA_HISTORY_MINUTES * 60 * 1000);
        state
            .quota_history
            .retain(|sample| sample.timestamp_ms >= cutoff);
    }
    let Some(usage) = value.pointer("/payload/info/last_token_usage") else {
        return;
    };
    let input_total = nonnegative_u64(usage.get("input_tokens"));
    let cache_read_tokens = nonnegative_u64(usage.get("cached_input_tokens"));
    let input_tokens = input_total.saturating_sub(cache_read_tokens);
    let output_tokens = nonnegative_u64(usage.get("output_tokens"));
    let event = UsageEvent {
        timestamp,
        input_tokens,
        output_tokens,
        cache_read_tokens,
        project: state.project.clone(),
    };
    let utc_day = event.timestamp.with_timezone(&Utc).date_naive();
    let total = state.utc_daily_tokens.entry(utc_day).or_default();
    *total = total.saturating_add(event.total_tokens());
    state.events.push(event);
}

fn parse_quota_windows(value: &Value, timestamp_ms: i64) -> Vec<CodexQuotaWindow> {
    [("primary", "session5h"), ("secondary", "weekly")]
        .into_iter()
        .filter_map(|(source, kind)| {
            let window = value.pointer(&format!("/payload/rate_limits/{source}"))?;
            let used_percent = finite_f64(window.get("used_percent"))?;
            let resets_at_ms = finite_f64(window.get("resets_at"))
                .map(|seconds| (seconds * 1_000.0).round() as i64)
                .or_else(|| {
                    finite_f64(window.get("resets_in_seconds")).map(|seconds| {
                        timestamp_ms.saturating_add((seconds * 1_000.0).round() as i64)
                    })
                });
            Some(CodexQuotaWindow {
                kind: kind.to_string(),
                used_percent: used_percent.clamp(0.0, 100.0),
                resets_at_ms,
                depletion_eta_ms: None,
            })
        })
        .collect()
}

fn finite_f64(value: Option<&Value>) -> Option<f64> {
    let number = value?.as_f64()?;
    number.is_finite().then_some(number)
}

fn nonnegative_u64(value: Option<&Value>) -> u64 {
    value
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_i64().and_then(|number| u64::try_from(number).ok()))
                .or_else(|| {
                    value
                        .as_f64()
                        .filter(|number| number.is_finite() && *number >= 0.0)
                        .map(|number| number as u64)
                })
        })
        .unwrap_or_default()
}

fn project_name(cwd: &str) -> Option<String> {
    let normalized = cwd.trim_end_matches(['/', '\\']);
    normalized
        .rsplit(['/', '\\'])
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn system_time_ms(value: SystemTime) -> u64 {
    value
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::{
        io::Write,
        sync::atomic::{AtomicU64, Ordering},
    };

    use chrono::Utc;

    use super::*;

    static NEXT_TEMPORARY_ROOT: AtomicU64 = AtomicU64::new(0);

    fn temporary_root() -> PathBuf {
        let root = env::temp_dir().join(format!(
            "mochiport-codex-usage-{}-{}-{}",
            std::process::id(),
            system_time_ms(SystemTime::now()),
            NEXT_TEMPORARY_ROOT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir_all(&root).expect("create temporary usage root");
        root
    }

    fn token_line(input: u64, cached: u64, output: u64) -> String {
        token_line_at(Utc::now(), input, cached, output)
    }

    fn token_line_at(timestamp: DateTime<Utc>, input: u64, cached: u64, output: u64) -> String {
        serde_json::json!({
            "timestamp": timestamp.to_rfc3339(),
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": { "last_token_usage": {
                    "input_tokens": input,
                    "cached_input_tokens": cached,
                    "output_tokens": output
                }}
            }
        })
        .to_string()
    }

    fn quota_line(timestamp: DateTime<Utc>, primary: f64, weekly: f64) -> String {
        serde_json::json!({
            "timestamp": timestamp.to_rfc3339(),
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": null,
                "rate_limits": {
                    "primary": { "used_percent": primary, "resets_in_seconds": 3600 },
                    "secondary": { "used_percent": weekly, "resets_at": timestamp.timestamp() + 86_400 }
                }
            }
        })
        .to_string()
    }

    #[test]
    fn collector_is_incremental_and_waits_for_complete_lines() {
        let root = temporary_root();
        let log = root.join("session.jsonl");
        let meta = serde_json::json!({
            "type": "session_meta",
            "payload": { "cwd": "C:\\Code\\MochiPort" }
        });
        fs::write(&log, format!("{meta}\n{}\n", token_line(100, 40, 20)))
            .expect("write fixture log");

        let mut collector = UsageCollector::default();
        let first = collector
            .collect(&root, Local::now())
            .expect("collect first snapshot");
        assert_eq!(first.today_tokens, 120);
        assert_eq!(first.today_requests, 1);
        assert_eq!(first.top_project.as_deref(), Some("MochiPort"));
        assert_eq!(first.seven_day.len(), TREND_DAYS as usize);
        assert_eq!(first.daily_usage.len(), HISTORY_DAYS as usize);
        assert_eq!(first.daily_usage.last().map(|day| day.tokens), Some(120));
        assert_eq!(first.seven_day_projects.len(), 1);
        assert_eq!(first.seven_day_projects[0].project, "MochiPort");
        assert_eq!(first.seven_day_projects[0].tokens, 120);

        let incomplete = token_line(50, 10, 10);
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&log)
            .expect("open fixture log");
        write!(file, "{incomplete}").expect("append incomplete line");
        let unchanged = collector
            .collect(&root, Local::now())
            .expect("collect incomplete snapshot");
        assert_eq!(unchanged.today_tokens, 120);

        writeln!(file).expect("complete fixture line");
        drop(file);
        let updated = collector
            .collect(&root, Local::now())
            .expect("collect updated snapshot");
        assert_eq!(updated.today_tokens, 180);
        assert_eq!(updated.today_requests, 2);
        let repeated = collector
            .collect(&root, Local::now())
            .expect("collect repeated snapshot");
        assert_eq!(repeated.today_tokens, 180);

        fs::remove_dir_all(root).expect("remove temporary usage root");
    }

    #[test]
    fn full_backfill_discovers_all_jsonl_files_while_refreshes_are_bounded() {
        let root = temporary_root();
        for index in 0..3 {
            fs::write(root.join(format!("session-{index}.jsonl")), "{}\n")
                .expect("write discovery fixture");
        }

        let mut full = Vec::new();
        collect_recent_jsonl(&root, UNIX_EPOCH, 0, None, &mut full)
            .expect("discover full backfill files");
        assert_eq!(full.len(), 3);

        let mut bounded = Vec::new();
        collect_recent_jsonl(&root, UNIX_EPOCH, 0, Some(2), &mut bounded)
            .expect("discover bounded refresh files");
        assert_eq!(bounded.len(), 2);

        fs::remove_dir_all(root).expect("remove temporary usage root");
    }

    #[test]
    fn collector_publishes_the_latest_absolute_and_relative_quota_windows() {
        let root = temporary_root();
        let log = root.join("session.jsonl");
        let older = Utc::now() - chrono::Duration::minutes(2);
        let latest = Utc::now();
        fs::write(
            &log,
            format!(
                "{}\n{}\n",
                quota_line(older, 18.0, 34.0),
                quota_line(latest, 42.5, 61.25),
            ),
        )
        .expect("write quota fixture");

        let mut collector = UsageCollector::default();
        let snapshot = collector
            .collect(&root, Local::now())
            .expect("collect quota snapshot");
        assert_eq!(snapshot.quota_windows.len(), 2);
        assert_eq!(snapshot.quota_windows[0].kind, "session5h");
        assert_eq!(snapshot.quota_windows[0].used_percent, 42.5);
        assert_eq!(
            snapshot.quota_windows[0].resets_at_ms,
            Some(latest.timestamp_millis() + 3_600_000),
        );
        assert_eq!(snapshot.quota_windows[1].kind, "weekly");
        assert_eq!(snapshot.quota_windows[1].used_percent, 61.25);

        fs::remove_dir_all(root).expect("remove temporary usage root");
    }

    #[test]
    fn collector_separates_activity_and_burn_windows_and_regresses_session_depletion() {
        let root = temporary_root();
        let log = root.join("session.jsonl");
        let now = Utc::now();
        fs::write(
            &log,
            format!(
                "{}\n{}\n{}\n{}\n{}\n",
                quota_line(now - chrono::Duration::minutes(10), 50.0, 20.0),
                quota_line(now - chrono::Duration::minutes(5), 60.0, 20.0),
                quota_line(now, 70.0, 20.0),
                token_line_at(now - chrono::Duration::minutes(5), 400, 0, 0),
                token_line_at(now - chrono::Duration::minutes(2), 600, 0, 0),
            ),
        )
        .expect("write burn and depletion fixture");

        let mut collector = UsageCollector::default();
        let snapshot = collector
            .collect(&root, now.with_timezone(&Local))
            .expect("collect burn and depletion snapshot");
        assert!((snapshot.tokens_per_minute - 200.0).abs() < f64::EPSILON);
        assert!((snapshot.burn_rate_tokens_per_minute - 100.0).abs() < f64::EPSILON);
        let session = snapshot
            .quota_windows
            .iter()
            .find(|window| window.kind == "session5h")
            .expect("session quota window");
        assert_eq!(
            session.depletion_eta_ms,
            Some(now.timestamp_millis() + 15 * 60_000),
        );

        fs::remove_dir_all(root).expect("remove temporary usage root");
    }

    #[test]
    fn depletion_estimators_reject_sparse_slow_and_post_reset_forecasts() {
        let now_ms = 1_800_000_i64;
        assert_eq!(
            estimate_session_depletion(
                &[(0, 20.0), (300_000, 25.0)],
                Some(now_ms + 3_600_000),
                now_ms,
            ),
            None,
        );
        assert_eq!(
            estimate_session_depletion(
                &[(0, 20.0), (300_000, 20.2), (600_000, 20.4)],
                Some(now_ms + 86_400_000),
                now_ms,
            ),
            None,
        );
        assert_eq!(
            estimate_session_depletion(
                &[(0, 20.0), (300_000, 25.0), (600_000, 30.0)],
                Some(now_ms + 30 * 60_000),
                now_ms,
            ),
            None,
        );
        assert_eq!(
            estimate_weekly_depletion(60.0, 0.5, Some(now_ms + 8 * 86_400_000), now_ms),
            None,
        );
        assert_eq!(
            estimate_weekly_depletion(60.0, 10.0, Some(now_ms + 5 * 86_400_000), now_ms),
            Some(now_ms + 4 * 86_400_000),
        );
    }

    #[test]
    fn collector_publishes_utc_streak_and_weekly_report_fields() {
        let root = temporary_root();
        let now = Utc::now();
        let current_log = root.join("current.jsonl");
        let current_meta = serde_json::json!({
            "type": "session_meta",
            "payload": { "cwd": "C:\\Code\\MochiPort" }
        });
        fs::write(
            &current_log,
            format!(
                "{current_meta}\n{}\n{}\n{}\n{}\n",
                token_line_at(now, 100, 0, 0),
                token_line_at(now - chrono::Duration::days(1), 200, 0, 0),
                token_line_at(now - chrono::Duration::days(2), 300, 0, 0),
                token_line_at(now - chrono::Duration::days(4), 400, 0, 0),
            ),
        )
        .expect("write current usage fixture");

        let previous_log = root.join("previous.jsonl");
        let previous_meta = serde_json::json!({
            "type": "session_meta",
            "payload": { "cwd": "C:\\Code\\Legacy" }
        });
        fs::write(
            &previous_log,
            format!(
                "{previous_meta}\n{}\n",
                token_line_at(now - chrono::Duration::days(8), 100, 0, 0),
            ),
        )
        .expect("write previous usage fixture");

        let mut collector = UsageCollector::default();
        let snapshot = collector
            .collect(&root, now.with_timezone(&Local))
            .expect("collect summary snapshot");

        assert_eq!(snapshot.streak_days, 3);
        assert_eq!(snapshot.yesterday_tokens, 200);
        assert!((snapshot.yesterday_cost_usd - 0.0003).abs() < f64::EPSILON);
        assert_eq!(snapshot.yesterday_top_project.as_deref(), Some("MochiPort"));
        let weekly = snapshot.weekly_report.expect("weekly report");
        assert_eq!(weekly.last_week_tokens, 900);
        assert!((weekly.last_week_cost_usd - 0.00135).abs() < f64::EPSILON);
        assert_eq!(weekly.previous_week_tokens, 100);
        assert_eq!(weekly.last_week_top_project.as_deref(), Some("MochiPort"));

        fs::remove_dir_all(root).expect("remove temporary usage root");
    }

    #[test]
    fn collector_keeps_full_record_and_streak_after_source_log_rotation() {
        let root = temporary_root();
        let history_path = root.join("durable-usage-history.json");
        let log = root.join("historic-session.jsonl");
        let now = Utc::now();
        fs::write(
            &log,
            format!(
                "{}\n{}\n{}\n{}\n",
                token_line_at(now - chrono::Duration::days(500), 10_000, 0, 0),
                token_line_at(now - chrono::Duration::days(2), 100, 0, 0),
                token_line_at(now - chrono::Duration::days(1), 100, 0, 0),
                token_line_at(now, 100, 0, 0),
            ),
        )
        .expect("write historic usage fixture");

        let mut first_collector = UsageCollector {
            files: HashMap::new(),
            history: UsageHistoryStore::at(history_path.clone()),
        };
        let first = first_collector
            .collect(&root, now.with_timezone(&Local))
            .expect("collect full usage backfill");
        assert_eq!(first.streak_days, 3);
        assert_eq!(first.previous_best_daily_tokens, Some(10_000));

        fs::remove_file(log).expect("rotate old session log");
        fs::write(
            root.join("current-session.jsonl"),
            format!("{}\n", token_line_at(now, 150, 0, 0)),
        )
        .expect("write current usage fixture");
        let mut restarted_collector = UsageCollector {
            files: HashMap::new(),
            history: UsageHistoryStore::at(history_path),
        };
        let restarted = restarted_collector
            .collect(&root, now.with_timezone(&Local))
            .expect("collect after source rotation");
        assert_eq!(restarted.streak_days, 3);
        assert_eq!(restarted.previous_best_daily_tokens, Some(10_000));

        fs::remove_dir_all(root).expect("remove temporary usage root");
    }

    #[test]
    fn collector_does_not_duplicate_a_source_that_disappears_and_returns() {
        let root = temporary_root();
        let history_path = root.join("durable-usage-history.json");
        let log = root.join("session.jsonl");
        let now = Utc::now();
        let historical = now - chrono::Duration::days(1);
        let line = token_line_at(historical, 200, 0, 0);
        fs::write(&log, format!("{line}\n")).expect("write source fixture");

        let mut first = UsageCollector {
            files: HashMap::new(),
            history: UsageHistoryStore::at(history_path.clone()),
        };
        let initial = first
            .collect(&root, now.with_timezone(&Local))
            .expect("collect initial source");
        assert_eq!(initial.previous_best_daily_tokens, Some(200));
        drop(first);

        fs::remove_file(&log).expect("temporarily remove source fixture");
        let mut missing = UsageCollector {
            files: HashMap::new(),
            history: UsageHistoryStore::at(history_path.clone()),
        };
        missing
            .collect(&root, now.with_timezone(&Local))
            .expect("collect with missing source");
        drop(missing);

        fs::write(&log, format!("{line}\n")).expect("restore source fixture");
        let mut returned = UsageCollector {
            files: HashMap::new(),
            history: UsageHistoryStore::at(history_path.clone()),
        };
        let returned_snapshot = returned
            .collect(&root, now.with_timezone(&Local))
            .expect("collect returned source");
        assert_eq!(returned_snapshot.previous_best_daily_tokens, Some(200));

        fs::OpenOptions::new()
            .append(true)
            .open(&log)
            .expect("open source fixture")
            .write_all(format!("{}\n", token_line_at(historical, 50, 0, 0)).as_bytes())
            .expect("append source activity");
        let appended = returned
            .collect(&root, now.with_timezone(&Local))
            .expect("collect appended source");
        assert_eq!(appended.previous_best_daily_tokens, Some(250));

        fs::remove_dir_all(root).expect("remove temporary usage root");
    }
}
