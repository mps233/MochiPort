use std::{
    collections::{BTreeMap, HashMap, HashSet},
    env, fs,
    io::{BufRead, BufReader, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use chrono::{DateTime, Days, Local, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};
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
const TAIL_FINGERPRINT_BYTES: usize = 16;
const LEGACY_PROVIDER: &str = "legacy";
const DEFAULT_CODEX_MODEL: &str = "codex";

#[derive(Clone, Default)]
pub struct CodexUsageState {
    collector: Arc<Mutex<UsageCollector>>,
}

#[derive(Debug, Clone)]
struct UsageEvent {
    timestamp: DateTime<chrono::FixedOffset>,
    session_id: String,
    model: String,
    /// Codex reports cached input as a subset of input tokens.
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    /// The headline usage unit reported by Codex. This is deliberately not
    /// reconstructed from uncached input because compaction records can carry
    /// input/output = 0 while still reporting a non-zero total.
    total_tokens: u64,
    cumulative: Option<CumulativeUsage>,
    // Retained for future source grouping. This is a Codex configuration name,
    // not a billing identity; the current dashboard keeps one provider-neutral
    // aggregate.
    #[allow(dead_code)]
    provider: String,
    project: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct CumulativeUsage {
    input: u64,
    cached: u64,
    output: u64,
    reasoning: u64,
    total: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TokenSnapshot {
    input: u64,
    output: u64,
    cached: u64,
    total: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ReplayKey {
    session_id: String,
    model: String,
    input: u64,
    output: u64,
    cached: u64,
    total: u64,
    cumulative: CumulativeUsage,
}

#[derive(Debug, Clone)]
struct QuotaSample {
    timestamp_ms: i64,
    windows: Vec<CodexQuotaWindow>,
}

impl UsageEvent {
    fn context_tokens(&self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }

    fn estimated_cost_usd(&self) -> f64 {
        self.estimated_cost_usd_with(codex_pricing(&self.model))
    }

    fn estimated_cost_usd_with(&self, pricing: CodexPricing) -> f64 {
        let uncached_input = self.input_tokens.saturating_sub(self.cache_read_tokens);
        (uncached_input as f64 * pricing.input
            + self.output_tokens as f64 * pricing.output
            + self.cache_read_tokens as f64 * pricing.cached_input)
            / 1_000_000.0
    }

    fn replay_key(&self) -> Option<ReplayKey> {
        Some(ReplayKey {
            session_id: self.session_id.clone(),
            model: self.model.clone(),
            input: self.input_tokens,
            output: self.output_tokens,
            cached: self.cache_read_tokens,
            total: self.total_tokens,
            cumulative: self.cumulative?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct CodexPricing {
    input: f64,
    output: f64,
    cached_input: f64,
}

#[derive(Debug, Deserialize)]
struct PricingConfig {
    // AI Token Monitor decodes the complete pricing file before using any one
    // provider. Requiring Claude here preserves that all-or-nothing fallback
    // behavior while MochiPort only consumes the Codex table.
    #[serde(rename = "claude")]
    _claude: ProviderConfig,
    codex: ProviderConfig,
    #[serde(default, rename = "opencode")]
    _opencode: Option<ProviderConfig>,
    #[serde(default, rename = "kimi")]
    _kimi: Option<ProviderConfig>,
    #[serde(default, rename = "glm")]
    _glm: Option<ProviderConfig>,
    #[serde(default, rename = "grok")]
    _grok: Option<ProviderConfig>,
}

#[derive(Debug, Deserialize)]
struct ProviderConfig {
    default: String,
    models: Vec<PricingEntry>,
}

#[derive(Debug, Deserialize)]
struct PricingEntry {
    #[serde(rename = "match")]
    match_pattern: String,
    #[serde(default, rename = "label")]
    _label: String,
    input: f64,
    output: f64,
    #[serde(default, rename = "cache_read")]
    _cache_read: f64,
    #[serde(default, rename = "cache_write")]
    _cache_write: f64,
    #[serde(default, rename = "cache_write_1h")]
    _cache_write_1h: f64,
    #[serde(default)]
    cached_input: f64,
    #[serde(default)]
    scheduled: Vec<ScheduledPrice>,
    #[serde(default, rename = "high_context")]
    _high_context: Option<HighContextTier>,
}

#[derive(Debug, Deserialize)]
struct ScheduledPrice {
    from: String,
    #[serde(default)]
    input: f64,
    #[serde(default)]
    output: f64,
    #[serde(default, rename = "cache_read")]
    _cache_read: f64,
    #[serde(default, rename = "cache_write")]
    _cache_write: f64,
    #[serde(default, rename = "cache_write_1h")]
    _cache_write_1h: f64,
    #[serde(default)]
    cached_input: f64,
    #[serde(default, rename = "high_context")]
    _high_context: Option<HighContextTier>,
}

#[derive(Debug, Deserialize)]
struct HighContextTier {
    #[serde(rename = "threshold_tokens")]
    _threshold_tokens: u64,
    #[serde(default, rename = "input")]
    _input: f64,
    #[serde(default, rename = "output")]
    _output: f64,
    #[serde(default, rename = "cached_input")]
    _cached_input: f64,
}

static USER_CODEX_PRICING: OnceLock<Option<ProviderConfig>> = OnceLock::new();

impl CodexPricing {
    const fn new(input: f64, output: f64, cached_input: f64) -> Self {
        Self {
            input,
            output,
            cached_input,
        }
    }
}

// AI Token Monitor v0.20.5 Codex pricing. Matching is intentionally ordered:
// the first canonicalized substring wins, so specific variants must precede
// their broader families.
const CODEX_PRICING: &[(&str, CodexPricing)] = &[
    ("gpt-5.6-sol", CodexPricing::new(5.00, 30.00, 0.50)),
    ("gpt-5.6-terra", CodexPricing::new(2.50, 15.00, 0.25)),
    ("gpt-5.6-luna", CodexPricing::new(1.00, 6.00, 0.10)),
    ("gpt-5.6", CodexPricing::new(5.00, 30.00, 0.50)),
    ("gpt-5.5-pro", CodexPricing::new(30.00, 180.00, 0.0)),
    ("gpt-5.5", CodexPricing::new(5.00, 30.00, 0.50)),
    ("gpt-5.4-pro", CodexPricing::new(30.00, 180.00, 0.0)),
    ("gpt-5.4-nano", CodexPricing::new(0.20, 1.25, 0.02)),
    ("gpt-5.4-mini", CodexPricing::new(0.75, 4.50, 0.075)),
    ("gpt-5.4", CodexPricing::new(2.50, 15.00, 0.25)),
    ("gpt-5.3-codex", CodexPricing::new(1.75, 14.00, 0.175)),
    ("gpt-5.3", CodexPricing::new(1.75, 14.00, 0.175)),
    ("gpt-5.2-codex", CodexPricing::new(1.75, 14.00, 0.175)),
    ("gpt-5.2", CodexPricing::new(1.25, 10.00, 0.125)),
    ("gpt-5.1-codex-max", CodexPricing::new(1.25, 10.00, 0.125)),
    ("gpt-5.1-codex-mini", CodexPricing::new(0.25, 2.00, 0.025)),
    ("gpt-5.1-codex", CodexPricing::new(1.25, 10.00, 0.125)),
    ("gpt-5.1", CodexPricing::new(0.625, 5.00, 0.125)),
    ("gpt-5-codex", CodexPricing::new(1.25, 10.00, 0.125)),
    ("gpt-5-mini", CodexPricing::new(0.125, 1.00, 0.025)),
    ("gpt-5-nano", CodexPricing::new(0.05, 0.40, 0.005)),
    ("gpt-5", CodexPricing::new(1.25, 10.00, 0.125)),
    ("gpt-4.1-mini", CodexPricing::new(0.40, 1.60, 0.10)),
    ("gpt-4.1", CodexPricing::new(2.00, 8.00, 0.50)),
    ("o4-mini", CodexPricing::new(1.10, 4.40, 0.55)),
    ("o3", CodexPricing::new(0.40, 1.60, 0.20)),
    ("codex-mini", CodexPricing::new(1.50, 6.00, 0.025)),
];

const DEFAULT_CODEX_PRICING: CodexPricing = CodexPricing::new(2.50, 15.00, 0.25);
const MODEL_VENDOR_PREFIXES: &[&str] = &[
    "anthropic",
    "openai",
    "azure",
    "bedrock",
    "vertex",
    "google",
    "xai",
    "moonshot",
    "zhipu",
    "kiro",
    "litellm",
    "omniroute",
    "openrouter",
];

fn canonical_model(model: &str) -> String {
    model
        .chars()
        .map(|character| match character {
            '.' | '_' => '-',
            other => other.to_ascii_lowercase(),
        })
        .collect()
}

fn normalize_model_id(model: &str) -> String {
    let canonical = canonical_model(model.trim());
    let without_path = canonical.rsplit('/').next().unwrap_or(&canonical);
    for vendor in MODEL_VENDOR_PREFIXES {
        if let Some(rest) = without_path.strip_prefix(&format!("{vendor}-")) {
            if names_model_family(rest) {
                return rest.to_string();
            }
        }
    }
    without_path.to_string()
}

fn names_model_family(model: &str) -> bool {
    const FAMILIES: &[&str] = &[
        "claude", "opus", "sonnet", "haiku", "fable", "mythos", "gpt", "codex", "o3-", "o4-",
        "grok", "kimi", "moonshot", "glm", "gemini", "deepseek", "qwen", "llama", "mistral",
    ];
    !model.is_empty() && FAMILIES.iter().any(|family| model.contains(family))
}

fn embedded_codex_pricing(model: &str) -> CodexPricing {
    let canonical = canonical_model(model);
    CODEX_PRICING
        .iter()
        .find_map(|(pattern, pricing)| {
            canonical
                .contains(&canonical_model(pattern))
                .then_some(*pricing)
        })
        .unwrap_or(DEFAULT_CODEX_PRICING)
}

fn decode_user_pricing(contents: &str) -> Result<ProviderConfig, serde_json::Error> {
    serde_json::from_str::<PricingConfig>(contents).map(|config| config.codex)
}

fn resolve_pricing(
    provider: &ProviderConfig,
    model: &str,
    today_utc: &str,
) -> Option<CodexPricing> {
    let canonical = canonical_model(model);
    let entry = provider
        .models
        .iter()
        .find(|entry| canonical.contains(&canonical_model(&entry.match_pattern)))
        .or_else(|| {
            provider
                .models
                .iter()
                .find(|entry| entry.match_pattern == provider.default)
        })
        .or_else(|| provider.models.first())?;

    let mut pricing = CodexPricing::new(entry.input, entry.output, entry.cached_input);
    if let Some(scheduled) = entry
        .scheduled
        .iter()
        .filter(|scheduled| scheduled.from.as_str() <= today_utc)
        .max_by(|left, right| left.from.cmp(&right.from))
    {
        if scheduled.input > 0.0 {
            pricing.input = scheduled.input;
        }
        if scheduled.output > 0.0 {
            pricing.output = scheduled.output;
        }
        if scheduled.cached_input > 0.0 {
            pricing.cached_input = scheduled.cached_input;
        }
    }
    Some(pricing)
}

fn user_pricing_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".claude").join("pricing.json"))
}

fn load_user_codex_pricing() -> Option<ProviderConfig> {
    let path = user_pricing_path()?;
    let contents = fs::read_to_string(&path).ok()?;
    match decode_user_pricing(&contents) {
        Ok(provider) => {
            eprintln!("[PRICING] Loaded Codex prices from {}", path.display());
            Some(provider)
        }
        Err(error) => {
            eprintln!(
                "[PRICING] Ignoring invalid {} and using embedded Codex prices: {error}",
                path.display()
            );
            None
        }
    }
}

fn today_utc() -> String {
    Utc::now().format("%Y-%m-%d").to_string()
}

fn codex_pricing(model: &str) -> CodexPricing {
    let today = today_utc();
    USER_CODEX_PRICING
        .get_or_init(load_user_codex_pricing)
        .as_ref()
        .and_then(|provider| resolve_pricing(provider, model, &today))
        .unwrap_or_else(|| embedded_codex_pricing(model))
}

#[cfg(test)]
fn pricing_from_user_contents(
    contents: Option<&str>,
    model: &str,
    today_utc: &str,
) -> CodexPricing {
    contents
        .and_then(|contents| decode_user_pricing(contents).ok())
        .and_then(|provider| resolve_pricing(&provider, model, today_utc))
        .unwrap_or_else(|| embedded_codex_pricing(model))
}

fn default_session_id(path: &Path) -> String {
    path.file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("codex-session")
        .to_string()
}

struct FileUsage {
    offset: u64,
    length: u64,
    modified: SystemTime,
    tail_fingerprint: Vec<u8>,
    session_id: String,
    current_model: String,
    previous_snapshot: Option<TokenSnapshot>,
    path_day: Option<NaiveDate>,
    provider: String,
    project: Option<String>,
    events: Vec<UsageEvent>,
    latest_quota: Option<QuotaSample>,
    quota_history: Vec<QuotaSample>,
}

impl FileUsage {
    fn new(path: &Path) -> Self {
        Self {
            offset: 0,
            length: 0,
            modified: UNIX_EPOCH,
            tail_fingerprint: Vec::new(),
            session_id: default_session_id(path),
            current_model: String::new(),
            previous_snapshot: None,
            path_day: extract_day_from_path(path),
            provider: LEGACY_PROVIDER.to_string(),
            project: None,
            events: Vec::new(),
            latest_quota: None,
            quota_history: Vec::new(),
        }
    }
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
    /// Sum of Codex's reported per-turn `total_tokens` values.
    today_tokens: u64,
    /// Input + output diagnostic; cached input is already included in input.
    today_context_tokens: u64,
    today_cache_read_tokens: u64,
    today_cache_creation_tokens: u64,
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
        let root = codex_data_root();
        let mut collector = collector
            .lock()
            .map_err(|_| "Codex 用量采集状态不可用".to_string())?;
        collector.collect(&root, Local::now())
    })
    .await
    .map_err(|error| format!("Codex 用量采集任务失败：{error}"))?
}

fn codex_data_root() -> PathBuf {
    if let Some(home) = env::var_os("CODEX_HOME").filter(|value| !value.is_empty()) {
        return PathBuf::from(home);
    }
    env::var_os("USERPROFILE")
        .or_else(|| env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".codex")
}

impl UsageCollector {
    fn collect(&mut self, root: &Path, now: DateTime<Local>) -> Result<CodexUsageSnapshot, String> {
        let updated_at_ms = system_time_ms(SystemTime::now());
        let scan_roots = codex_log_roots(root);
        if scan_roots.is_empty() {
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
        for scan_root in &scan_roots {
            collect_recent_jsonl(scan_root, cutoff_time, 0, file_limit, &mut discovered)?;
            if file_limit.is_some_and(|limit| discovered.len() >= limit) {
                break;
            }
        }
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
            let modified = metadata.modified().unwrap_or(UNIX_EPOCH);
            let state = self
                .files
                .entry(path.clone())
                .or_insert_with(|| FileUsage::new(path));
            let known_file = state.modified != UNIX_EPOCH;
            let rewritten = known_file
                && (length < state.length
                    || (length == state.length && modified != state.modified)
                    || (length > state.length && !tail_fingerprint_matches(path, state)));
            if rewritten || length < state.offset {
                reset_file_usage(path, state);
            }
            state.length = length;
            state.modified = modified;
            // The first durable-history seed must consume every complete line
            // in each file. Subsequent refreshes intentionally stay bounded
            // so a large active rollout cannot monopolize the collector.
            let byte_limit = (!full_backfill).then_some(MAX_BYTES_PER_FILE_PER_REFRESH);
            read_increment(path, state, byte_limit)?;
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
        // AI Token Monitor converts each event timestamp to the user's local
        // timezone before assigning it to a calendar day. Keep every daily
        // aggregate, including durable history and streaks, on that same
        // local-day boundary.
        let local_today = today;
        let last_week_start = local_today
            .checked_sub_days(Days::new(7))
            .unwrap_or(local_today);
        let previous_week_start = last_week_start
            .checked_sub_days(Days::new(7))
            .unwrap_or(last_week_start);
        let earliest_history_day = today
            .checked_sub_days(Days::new(HISTORY_DAYS - 1))
            .unwrap_or(today);
        let persisted_replace_from = local_today
            .checked_sub_days(Days::new(RECENT_DAYS - 1))
            .unwrap_or(local_today);
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
        let mut today_context_tokens = 0_u64;
        let mut today_cache_read_tokens = 0_u64;
        let today_cache_creation_tokens = 0_u64;
        let mut today_requests = 0_usize;
        let mut yesterday_tokens = 0_u64;
        let mut yesterday_cost_usd = 0_f64;
        let mut yesterday_projects = HashMap::<String, u64>::new();
        let mut activity_tokens = 0_u64;
        let mut burn_tokens = 0_u64;
        let mut estimated_cost_usd = 0_f64;
        let mut daily = BTreeMap::<NaiveDate, u64>::new();
        let mut observed_local_daily = BTreeMap::<NaiveDate, u64>::new();
        let mut observed_source_daily = BTreeMap::<String, BTreeMap<NaiveDate, u64>>::new();
        let mut projects = HashMap::<String, u64>::new();
        let mut seven_day_projects = HashMap::<String, u64>::new();
        let mut last_week_tokens = 0_u64;
        let mut last_week_cost_usd = 0_f64;
        let mut previous_week_tokens = 0_u64;
        let mut last_week_projects = HashMap::<String, u64>::new();
        let mut last_activity_at_ms = None;
        let mut active_minutes = HashMap::<i64, u64>::new();

        // Resume/subagent rollouts can replay earlier token_count records into
        // another file. Match AI Token Monitor: only events with a cumulative
        // snapshot can be collapsed, using session + normalized model + per-turn
        // four-tuple + cumulative five-tuple, and keep the earliest occurrence
        // globally. Events without a cumulative snapshot cannot be safely
        // identified as replays and pass through unchanged.
        let mut replayed = HashMap::<ReplayKey, UsageEvent>::new();
        let mut passthrough = Vec::<UsageEvent>::new();
        for state in self.files.values() {
            for event in &state.events {
                if let Some(key) = event.replay_key() {
                    match replayed.get(&key) {
                        Some(kept) if kept.timestamp <= event.timestamp => {}
                        _ => {
                            replayed.insert(key, event.clone());
                        }
                    }
                } else {
                    passthrough.push(event.clone());
                }
            }
        }
        let mut deduplicated_events = passthrough;
        deduplicated_events.extend(replayed.into_values());

        for event in &deduplicated_events {
            let local_day = event.timestamp.with_timezone(&Local).date_naive();
            let source_days = observed_source_daily
                // A rollout can move from sessions to archived_sessions. The
                // session id remains stable, unlike its path, so persistence
                // does not reinterpret that move as a brand-new source.
                .entry(format!("session:{}", event.session_id))
                .or_default();
            let source_total = source_days.entry(local_day).or_default();
            *source_total = source_total.saturating_add(event.total_tokens);
            if full_backfill || local_day >= persisted_replace_from {
                let total = observed_local_daily.entry(local_day).or_default();
                *total = total.saturating_add(event.total_tokens);
            }
        }

        for event in &deduplicated_events {
            let timestamp_ms = event.timestamp.timestamp_millis();
            let local_day = event.timestamp.with_timezone(&Local).date_naive();
            let tokens = event.total_tokens;
            // Durable totals above intentionally see the complete first-pass
            // history. Dashboard aggregates stay bounded.
            if timestamp_ms < history_cutoff {
                continue;
            }
            if local_day >= last_week_start && local_day < local_today {
                last_week_tokens = last_week_tokens.saturating_add(tokens);
                last_week_cost_usd += event.estimated_cost_usd();
                if let Some(project) = event.project.as_ref() {
                    let total = last_week_projects.entry(project.clone()).or_default();
                    *total = total.saturating_add(tokens);
                }
            } else if local_day >= previous_week_start && local_day < last_week_start {
                previous_week_tokens = previous_week_tokens.saturating_add(tokens);
            }
            if local_day >= earliest_history_day && local_day <= today {
                let total = daily.entry(local_day).or_default();
                *total = total.saturating_add(tokens);
            }
            if local_day == today {
                today_tokens = today_tokens.saturating_add(tokens);
                today_context_tokens = today_context_tokens.saturating_add(event.context_tokens());
                today_cache_read_tokens =
                    today_cache_read_tokens.saturating_add(event.cache_read_tokens);
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
                last_activity_at_ms.map_or(timestamp_ms, |current: i64| current.max(timestamp_ms)),
            );
        }

        if !full_backfill || full_backfill_complete {
            for state in self.files.values_mut() {
                state
                    .events
                    .retain(|event| event.timestamp.timestamp_millis() >= history_cutoff);
            }
        }

        self.history.replace_daily_tokens(
            &observed_local_daily,
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
        let streak_days = self.history.streak_days(local_today);
        let previous_best_daily_tokens = self.history.previous_best_daily_tokens(local_today);
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
                .record_quota_percent(local_today, &window.kind, window.used_percent);
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
                    .weekly_daily_rate(&window.kind, local_today, 8)
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
            today_context_tokens,
            today_cache_read_tokens,
            today_cache_creation_tokens,
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
        today_context_tokens: 0,
        today_cache_read_tokens: 0,
        today_cache_creation_tokens: 0,
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

fn codex_log_roots(codex_root: &Path) -> Vec<PathBuf> {
    let roots = ["sessions", "archived_sessions"]
        .into_iter()
        .map(|directory| codex_root.join(directory))
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    #[cfg(test)]
    if roots.is_empty() && codex_root.is_dir() {
        return vec![codex_root.to_path_buf()];
    }
    roots
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

fn reset_file_usage(path: &Path, state: &mut FileUsage) {
    state.offset = 0;
    state.tail_fingerprint.clear();
    state.session_id = default_session_id(path);
    state.current_model.clear();
    state.previous_snapshot = None;
    state.project = None;
    state.provider = LEGACY_PROVIDER.to_string();
    state.events.clear();
    state.latest_quota = None;
    state.quota_history.clear();
}

fn tail_fingerprint_matches(path: &Path, state: &FileUsage) -> bool {
    use std::io::Read;

    if state.tail_fingerprint.is_empty() {
        return state.offset == 0;
    }
    let fingerprint_length = state.tail_fingerprint.len() as u64;
    if state.offset < fingerprint_length {
        return false;
    }
    let Ok(mut file) = fs::File::open(path) else {
        return false;
    };
    if file
        .seek(SeekFrom::Start(state.offset - fingerprint_length))
        .is_err()
    {
        return false;
    }
    let mut observed = vec![0; state.tail_fingerprint.len()];
    file.read_exact(&mut observed).is_ok() && observed == state.tail_fingerprint
}

fn read_increment(
    path: &Path,
    state: &mut FileUsage,
    byte_limit: Option<u64>,
) -> Result<(), String> {
    let file = fs::File::open(path)
        .map_err(|error| format!("无法读取 Codex 会话日志 {}：{error}", path.display()))?;
    let mut reader = BufReader::new(file);
    reader
        .seek(SeekFrom::Start(state.offset))
        .map_err(|error| format!("无法定位 Codex 会话日志 {}：{error}", path.display()))?;
    let initial_offset = state.offset;
    let mut consumed = 0_u64;
    let mut line = Vec::new();
    while byte_limit.map_or(true, |limit| consumed < limit) {
        line.clear();
        let bytes = reader
            .read_until(b'\n', &mut line)
            .map_err(|error| format!("无法解析 Codex 会话日志 {}：{error}", path.display()))?;
        if bytes == 0 || line.last() != Some(&b'\n') {
            break;
        }
        consumed = consumed.saturating_add(bytes as u64);
        state.offset = initial_offset.saturating_add(consumed);
        let fingerprint_length = line.len().min(TAIL_FINGERPRINT_BYTES);
        state.tail_fingerprint = line[line.len() - fingerprint_length..].to_vec();
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
        if let Some(session_id) = value
            .pointer("/payload/id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            state.session_id = session_id.to_string();
        }
        state.provider = value
            .pointer("/payload/model_provider")
            .and_then(Value::as_str)
            .or_else(|| value.pointer("/payload/provider").and_then(Value::as_str))
            .or_else(|| value.get("model_provider").and_then(Value::as_str))
            .or_else(|| value.get("provider").and_then(Value::as_str))
            .map(str::trim)
            .filter(|provider| !provider.is_empty())
            .unwrap_or(LEGACY_PROVIDER)
            .to_string();
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
    if value.get("type").and_then(Value::as_str) == Some("turn_context") {
        if let Some(model) = value
            .pointer("/payload/model")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            state.current_model = normalize_model_id(model);
        }
        return;
    }
    if value.pointer("/payload/type").and_then(Value::as_str) != Some("token_count") {
        return;
    }
    let Some(timestamp) = resolve_event_timestamp(&value, state.path_day) else {
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
    let Some(info) = value
        .pointer("/payload/info")
        .filter(|info| !info.is_null())
    else {
        return;
    };
    let Some(usage) = info
        .get("last_token_usage")
        .or_else(|| info.get("total_token_usage"))
    else {
        return;
    };
    let input_tokens = nonnegative_u64(usage.get("input_tokens"));
    let cache_read_tokens = nonnegative_u64(usage.get("cached_input_tokens"));
    let output_tokens = nonnegative_u64(usage.get("output_tokens"));
    let total_tokens = usage
        .get("total_tokens")
        .map(|value| nonnegative_u64(Some(value)))
        .unwrap_or_else(|| input_tokens.saturating_add(output_tokens));
    let snapshot = TokenSnapshot {
        input: input_tokens,
        output: output_tokens,
        cached: cache_read_tokens,
        total: total_tokens,
    };
    if state.previous_snapshot == Some(snapshot) {
        return;
    }
    state.previous_snapshot = Some(snapshot);
    if input_tokens == 0 && output_tokens == 0 && cache_read_tokens == 0 && total_tokens == 0 {
        return;
    }
    let event = UsageEvent {
        timestamp,
        session_id: state.session_id.clone(),
        model: if state.current_model.is_empty() {
            DEFAULT_CODEX_MODEL.to_string()
        } else {
            state.current_model.clone()
        },
        input_tokens,
        output_tokens,
        cache_read_tokens,
        total_tokens,
        cumulative: parse_cumulative_usage(info.get("total_token_usage")),
        provider: state.provider.clone(),
        project: state.project.clone(),
    };
    state.events.push(event);
}

fn parse_cumulative_usage(usage: Option<&Value>) -> Option<CumulativeUsage> {
    let usage = usage?;
    Some(CumulativeUsage {
        input: nonnegative_u64(usage.get("input_tokens")),
        cached: nonnegative_u64(usage.get("cached_input_tokens")),
        output: nonnegative_u64(usage.get("output_tokens")),
        reasoning: nonnegative_u64(usage.get("reasoning_output_tokens")),
        total: nonnegative_u64(usage.get("total_tokens")),
    })
}

fn resolve_event_timestamp(
    value: &Value,
    path_day: Option<NaiveDate>,
) -> Option<DateTime<chrono::FixedOffset>> {
    if let Some(timestamp) = value.get("timestamp").and_then(Value::as_str) {
        if let Ok(parsed) = DateTime::parse_from_rfc3339(timestamp) {
            return Some(parsed);
        }
        if let Some(day) = timestamp
            .get(..10)
            .and_then(|prefix| NaiveDate::parse_from_str(prefix, "%Y-%m-%d").ok())
        {
            return local_start_of_day(day);
        }
    }
    path_day.and_then(local_start_of_day)
}

fn local_start_of_day(day: NaiveDate) -> Option<DateTime<chrono::FixedOffset>> {
    let local = Local
        .from_local_datetime(&day.and_hms_opt(0, 0, 0)?)
        .earliest()?;
    Some(local.fixed_offset())
}

fn extract_day_from_path(path: &Path) -> Option<NaiveDate> {
    let components = path
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect::<Vec<_>>();
    components.windows(4).find_map(|window| {
        ((window[0] == "sessions" || window[0] == "archived_sessions")
            && window[1].len() == 4
            && window[2].len() == 2
            && window[3].len() == 2)
            .then(|| format!("{}-{}-{}", window[1], window[2], window[3]))
            .and_then(|day| NaiveDate::parse_from_str(&day, "%Y-%m-%d").ok())
    })
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

    fn token_line_with_totals_at(
        timestamp: DateTime<Utc>,
        last: TokenSnapshot,
        cumulative: Option<CumulativeUsage>,
    ) -> String {
        let mut info = serde_json::json!({
            "last_token_usage": {
                "input_tokens": last.input,
                "cached_input_tokens": last.cached,
                "output_tokens": last.output,
                "total_tokens": last.total,
            }
        });
        if let Some(cumulative) = cumulative {
            info["total_token_usage"] = serde_json::json!({
                "input_tokens": cumulative.input,
                "cached_input_tokens": cumulative.cached,
                "output_tokens": cumulative.output,
                "reasoning_output_tokens": cumulative.reasoning,
                "total_tokens": cumulative.total,
            });
        }
        serde_json::json!({
            "timestamp": timestamp.to_rfc3339(),
            "type": "event_msg",
            "payload": { "type": "token_count", "info": info }
        })
        .to_string()
    }

    fn turn_context_line(model: &str) -> String {
        serde_json::json!({
            "type": "turn_context",
            "payload": { "model": model },
        })
        .to_string()
    }

    fn session_meta_line(cwd: &str, provider: Option<&str>) -> String {
        let mut payload = serde_json::json!({ "id": "session-test", "cwd": cwd });
        if let Some(provider) = provider {
            payload["model_provider"] = serde_json::json!(provider);
        }
        serde_json::json!({
            "type": "session_meta",
            "payload": payload,
        })
        .to_string()
    }

    #[test]
    fn reported_total_and_context_keep_codex_input_semantics() {
        let event = UsageEvent {
            timestamp: Utc::now().fixed_offset(),
            session_id: "session-test".to_string(),
            model: "gpt-5-6-sol".to_string(),
            input_tokens: 100,
            output_tokens: 20,
            cache_read_tokens: 40,
            total_tokens: 120,
            cumulative: None,
            provider: "legacy".to_string(),
            project: None,
        };
        assert_eq!(event.context_tokens(), 120);
        assert_eq!(event.total_tokens, 120);
        assert!(
            (event.estimated_cost_usd_with(CodexPricing::new(5.0, 30.0, 0.5)) - 0.00092).abs()
                < 1e-12
        );
    }

    #[test]
    fn parser_prefers_reported_last_total_and_only_skips_consecutive_snapshots() {
        let mut state = FileUsage::new(Path::new("rollout-test.jsonl"));
        parse_line(
            session_meta_line("C:\\Code\\MochiPort", None).as_bytes(),
            &mut state,
        );
        parse_line(
            turn_context_line("OpenAI/GPT-5.6_SOL").as_bytes(),
            &mut state,
        );
        let cumulative = CumulativeUsage {
            input: 1_000,
            cached: 900,
            output: 50,
            reasoning: 10,
            total: 1_050,
        };
        let compaction = token_line_with_totals_at(
            Utc::now(),
            TokenSnapshot {
                input: 0,
                output: 0,
                cached: 0,
                total: 14_880,
            },
            Some(cumulative),
        );
        parse_line(compaction.as_bytes(), &mut state);
        parse_line(compaction.as_bytes(), &mut state);
        parse_line(token_line(1, 0, 0).as_bytes(), &mut state);
        parse_line(compaction.as_bytes(), &mut state);

        assert_eq!(state.events.len(), 3);
        assert_eq!(state.events[0].total_tokens, 14_880);
        assert_eq!(state.events[0].input_tokens, 0);
        assert_eq!(state.events[0].model, "gpt-5-6-sol");
        assert_eq!(state.events[0].session_id, "session-test");
        assert_eq!(state.events[0].cumulative, Some(cumulative));
        assert_eq!(state.events[2].total_tokens, 14_880);
    }

    #[test]
    fn token_usage_falls_back_to_cumulative_and_then_input_plus_output() {
        let mut state = FileUsage::new(Path::new("rollout-test.jsonl"));
        let cumulative_only = serde_json::json!({
            "timestamp": Utc::now().to_rfc3339(),
            "type": "event_msg",
            "payload": { "type": "token_count", "info": {
                "total_token_usage": {
                    "input_tokens": 200,
                    "cached_input_tokens": 10,
                    "output_tokens": 100,
                    "total_tokens": 321
                }
            }}
        });
        parse_line(cumulative_only.to_string().as_bytes(), &mut state);
        assert_eq!(state.events[0].total_tokens, 321);

        let missing_total = token_line(20, 2, 5);
        parse_line(missing_total.as_bytes(), &mut state);
        assert_eq!(state.events[1].total_tokens, 25);
    }

    #[test]
    fn pricing_and_model_normalization_match_ai_token_monitor_v0205() {
        assert_eq!(CODEX_PRICING.len(), 27);
        assert_eq!(normalize_model_id("OpenAI/GPT-5.6_SOL"), "gpt-5-6-sol");
        assert_eq!(normalize_model_id("anthropic-gpt-5.6-sol"), "gpt-5-6-sol");
        let sol = embedded_codex_pricing("gpt-5-6-sol");
        assert_eq!((sol.input, sol.output, sol.cached_input), (5.0, 30.0, 0.5));
        let mini = embedded_codex_pricing("gpt-5.1-codex-mini");
        assert_eq!(
            (mini.input, mini.output, mini.cached_input),
            (0.25, 2.0, 0.025)
        );
        let unknown = embedded_codex_pricing("some-future-model");
        assert_eq!(
            (unknown.input, unknown.output, unknown.cached_input),
            (2.5, 15.0, 0.25),
        );
    }

    fn pricing_config(codex: Value) -> String {
        serde_json::json!({
            "claude": {
                "default": "sonnet",
                "models": [
                    { "match": "sonnet", "input": 3.0, "output": 15.0 }
                ]
            },
            "codex": codex,
        })
        .to_string()
    }

    #[test]
    fn user_pricing_uses_first_canonical_substring_match() {
        let contents = pricing_config(serde_json::json!({
            "default": "fallback",
            "models": [
                {
                    "match": "gpt-5",
                    "input": 9.0,
                    "output": 90.0,
                    "cached_input": 0.9
                },
                {
                    "match": "gpt-5.6-sol",
                    "input": 5.0,
                    "output": 30.0,
                    "cached_input": 0.5
                },
                {
                    "match": "fallback",
                    "input": 2.0,
                    "output": 20.0,
                    "cached_input": 0.2
                }
            ]
        }));
        let provider = decode_user_pricing(&contents).expect("decode complete pricing config");
        assert_eq!(
            resolve_pricing(&provider, "OpenAI/GPT-5.6_SOL", "2026-08-25"),
            Some(CodexPricing::new(9.0, 90.0, 0.9))
        );
    }

    #[test]
    fn user_pricing_unknown_model_uses_exact_default_then_first_entry() {
        let exact_default = pricing_config(serde_json::json!({
            "default": "Fallback.Raw",
            "models": [
                { "match": "first", "input": 1.0, "output": 10.0 },
                {
                    "match": "Fallback.Raw",
                    "input": 2.0,
                    "output": 20.0,
                    "cached_input": 0.2
                }
            ]
        }));
        let provider = decode_user_pricing(&exact_default).expect("decode exact default config");
        assert_eq!(
            resolve_pricing(&provider, "unknown-model", "2026-08-25"),
            Some(CodexPricing::new(2.0, 20.0, 0.2))
        );

        let missing_default = pricing_config(serde_json::json!({
            "default": "missing",
            "models": [
                { "match": "first", "input": 1.0, "output": 10.0 },
                { "match": "second", "input": 2.0, "output": 20.0 }
            ]
        }));
        let provider =
            decode_user_pricing(&missing_default).expect("decode missing default config");
        assert_eq!(
            resolve_pricing(&provider, "unknown-model", "2026-08-25"),
            Some(CodexPricing::new(1.0, 10.0, 0.0))
        );
    }

    #[test]
    fn user_pricing_applies_latest_effective_schedule_over_base_fields() {
        let contents = pricing_config(serde_json::json!({
            "default": "scheduled-model",
            "models": [{
                "match": "scheduled-model",
                "input": 1.0,
                "output": 2.0,
                "cached_input": 0.1,
                "scheduled": [
                    {
                        "from": "2026-08-20",
                        "input": 3.0,
                        "output": 4.0,
                        "cached_input": 0.3
                    },
                    {
                        "from": "2026-08-24",
                        "input": 0.0,
                        "output": 8.0,
                        "cached_input": 0.8
                    },
                    {
                        "from": "2026-09-01",
                        "input": 9.0,
                        "output": 10.0,
                        "cached_input": 0.9
                    }
                ]
            }]
        }));
        let provider = decode_user_pricing(&contents).expect("decode scheduled pricing config");
        assert_eq!(
            resolve_pricing(&provider, "scheduled-model", "2026-08-19"),
            Some(CodexPricing::new(1.0, 2.0, 0.1))
        );
        assert_eq!(
            resolve_pricing(&provider, "scheduled-model", "2026-08-25"),
            Some(CodexPricing::new(1.0, 8.0, 0.8))
        );
        assert_eq!(
            resolve_pricing(&provider, "scheduled-model", "2026-09-01"),
            Some(CodexPricing::new(9.0, 10.0, 0.9))
        );
    }

    #[test]
    fn invalid_complete_user_pricing_falls_back_to_embedded_v0205() {
        let missing_claude = serde_json::json!({
            "codex": {
                "default": "gpt-5.6-sol",
                "models": [{
                    "match": "gpt-5.6-sol",
                    "input": 99.0,
                    "output": 99.0
                }]
            }
        })
        .to_string();
        assert_eq!(
            pricing_from_user_contents(Some(&missing_claude), "gpt-5.6-sol", "2026-08-25"),
            CodexPricing::new(5.0, 30.0, 0.5)
        );

        let malformed_model = pricing_config(serde_json::json!({
            "default": "gpt-5.6-sol",
            "models": [{ "match": "gpt-5.6-sol", "input": 99.0 }]
        }));
        assert_eq!(
            pricing_from_user_contents(Some(&malformed_model), "gpt-5.6-sol", "2026-08-25"),
            CodexPricing::new(5.0, 30.0, 0.5)
        );

        let mut malformed_optional_provider: Value =
            serde_json::from_str(&pricing_config(serde_json::json!({
                "default": "gpt-5.6-sol",
                "models": [{
                    "match": "gpt-5.6-sol",
                    "input": 99.0,
                    "output": 99.0
                }]
            })))
            .expect("decode test fixture");
        malformed_optional_provider["kimi"] = serde_json::json!({
            "default": 42,
            "models": []
        });
        assert_eq!(
            pricing_from_user_contents(
                Some(&malformed_optional_provider.to_string()),
                "gpt-5.6-sol",
                "2026-08-25"
            ),
            CodexPricing::new(5.0, 30.0, 0.5)
        );
    }

    #[test]
    fn parser_preserves_session_provider_and_falls_back_to_legacy() {
        for (provider, expected) in [
            (Some("ai-gateway"), "ai-gateway"),
            (Some("custom"), "custom"),
            (None, LEGACY_PROVIDER),
            (Some("  "), LEGACY_PROVIDER),
        ] {
            let mut state = FileUsage::new(Path::new("session.jsonl"));
            parse_line(
                session_meta_line("C:\\Code\\MochiPort", provider).as_bytes(),
                &mut state,
            );
            parse_line(token_line(100, 40, 20).as_bytes(), &mut state);
            assert_eq!(state.provider, expected);
            assert_eq!(state.events[0].provider, expected);
        }
    }

    #[test]
    fn collector_retains_provider_for_each_session_file() {
        let root = temporary_root();
        let fixtures = [
            ("gateway", Some("ai-gateway"), "ai-gateway"),
            ("custom", Some("custom"), "custom"),
            ("legacy", None, LEGACY_PROVIDER),
        ];
        for (name, provider, _) in fixtures {
            let path = root.join(format!("{name}.jsonl"));
            fs::write(
                &path,
                format!(
                    "{}\n{}\n",
                    session_meta_line(&format!("C:\\Code\\{name}"), provider),
                    token_line(100, 40, 20),
                ),
            )
            .expect("write provider fixture");
        }

        let mut collector = UsageCollector::default();
        collector
            .collect(&root, Local::now())
            .expect("collect provider fixtures");
        for (name, _, expected) in fixtures {
            let state = collector
                .files
                .get(&root.join(format!("{name}.jsonl")))
                .expect("provider file state");
            assert_eq!(state.provider, expected);
            assert_eq!(state.events[0].provider, expected);
        }

        fs::remove_dir_all(root).expect("remove provider fixture root");
    }

    #[test]
    fn collector_collapses_only_cumulative_cross_rollout_replays() {
        let root = temporary_root();
        let now = Utc::now();
        let cumulative = CumulativeUsage {
            input: 1_000,
            cached: 900,
            output: 50,
            reasoning: 10,
            total: 1_050,
        };
        let replay = token_line_with_totals_at(
            now,
            TokenSnapshot {
                input: 100,
                output: 20,
                cached: 80,
                total: 120,
            },
            Some(cumulative),
        );
        let compaction = token_line_with_totals_at(
            now,
            TokenSnapshot {
                input: 0,
                output: 0,
                cached: 0,
                total: 14_880,
            },
            Some(cumulative),
        );
        let no_cumulative = token_line_with_totals_at(
            now,
            TokenSnapshot {
                input: 40,
                output: 10,
                cached: 0,
                total: 50,
            },
            None,
        );
        let prefix = format!(
            "{}\n{}\n",
            session_meta_line("C:\\Code\\MochiPort", None),
            turn_context_line("gpt-5.6-sol"),
        );
        fs::write(
            root.join("rollout-a.jsonl"),
            format!("{prefix}{replay}\n{compaction}\n{no_cumulative}\n"),
        )
        .expect("write first replay fixture");
        fs::write(
            root.join("rollout-b.jsonl"),
            format!("{prefix}{replay}\n{no_cumulative}\n"),
        )
        .expect("write second replay fixture");

        let mut collector = UsageCollector::default();
        let snapshot = collector
            .collect(&root, now.with_timezone(&Local))
            .expect("collect replay fixtures");

        assert_eq!(snapshot.today_tokens, 15_100);
        assert_eq!(snapshot.today_requests, 4);
        fs::remove_dir_all(root).expect("remove replay fixtures");
    }

    #[test]
    fn collector_collapses_nonconsecutive_matching_snapshots_inside_one_rollout() {
        let root = temporary_root();
        let now = Utc::now();
        let cumulative = CumulativeUsage {
            input: 1_000,
            cached: 900,
            output: 50,
            reasoning: 10,
            total: 1_050,
        };
        let compaction = token_line_with_totals_at(
            now,
            TokenSnapshot {
                input: 0,
                output: 0,
                cached: 0,
                total: 14_880,
            },
            Some(cumulative),
        );
        let separator = token_line_with_totals_at(
            now,
            TokenSnapshot {
                input: 1,
                output: 0,
                cached: 0,
                total: 1,
            },
            Some(cumulative),
        );
        fs::write(
            root.join("rollout.jsonl"),
            format!("{compaction}\n{separator}\n{compaction}\n"),
        )
        .expect("write nonconsecutive fixture");

        let mut collector = UsageCollector::default();
        let snapshot = collector
            .collect(&root, now.with_timezone(&Local))
            .expect("collect nonconsecutive fixture");
        // A cumulative snapshot is a replay discriminator even when the same
        // record is separated by another line in the same rollout. Keep the
        // earliest one globally, matching AI Token Monitor's HashMap reducer.
        assert_eq!(snapshot.today_tokens, 14_881);
        assert_eq!(snapshot.today_requests, 2);

        fs::remove_dir_all(root).expect("remove nonconsecutive fixture");
    }

    #[test]
    fn collector_scans_sessions_and_archived_sessions() {
        let root = temporary_root();
        let sessions = root.join("sessions");
        let archived = root.join("archived_sessions");
        fs::create_dir_all(&sessions).expect("create sessions fixture");
        fs::create_dir_all(&archived).expect("create archived fixture");
        fs::write(
            sessions.join("current.jsonl"),
            format!("{}\n", token_line(100, 0, 0)),
        )
        .expect("write sessions fixture");
        fs::write(
            archived.join("archived.jsonl"),
            format!("{}\n", token_line(200, 0, 0)),
        )
        .expect("write archived fixture");

        let mut collector = UsageCollector::default();
        let snapshot = collector
            .collect(&root, Local::now())
            .expect("collect both Codex roots");
        assert_eq!(snapshot.scanned_files, 2);
        assert_eq!(snapshot.today_tokens, 300);

        fs::remove_dir_all(root).expect("remove Codex roots fixture");
    }

    #[test]
    fn moving_a_session_to_archive_does_not_duplicate_durable_history() {
        let root = temporary_root();
        let sessions = root.join("sessions");
        let archived = root.join("archived_sessions");
        fs::create_dir_all(&sessions).expect("create sessions fixture");
        fs::create_dir_all(&archived).expect("create archived fixture");
        let history_path = root.join("durable-history-v3.json");
        let session_path = sessions.join("rollout.jsonl");
        let yesterday = Utc::now() - chrono::Duration::days(1);
        fs::write(
            &session_path,
            format!(
                "{}\n{}\n",
                session_meta_line("C:\\Code\\MochiPort", None),
                token_line_at(yesterday, 200, 0, 0),
            ),
        )
        .expect("write session before archive");

        let mut initial = UsageCollector {
            files: HashMap::new(),
            history: UsageHistoryStore::at(history_path.clone()),
        };
        let initial_snapshot = initial
            .collect(&root, Local::now())
            .expect("collect session before archive");
        assert_eq!(initial_snapshot.previous_best_daily_tokens, Some(200));
        drop(initial);

        fs::rename(&session_path, archived.join("rollout.jsonl"))
            .expect("move session into archive");
        let mut restarted = UsageCollector {
            files: HashMap::new(),
            history: UsageHistoryStore::at(history_path),
        };
        let archived_snapshot = restarted
            .collect(&root, Local::now())
            .expect("collect archived session");
        assert_eq!(archived_snapshot.previous_best_daily_tokens, Some(200));

        fs::remove_dir_all(root).expect("remove archive move fixture");
    }

    #[test]
    fn timestamp_fallback_uses_prefix_then_rollout_path_day() {
        let value = serde_json::json!({ "timestamp": "2026-03-27 not-rfc3339" });
        let prefix = resolve_event_timestamp(&value, None).expect("timestamp date prefix");
        assert_eq!(
            prefix.with_timezone(&Local).date_naive().to_string(),
            "2026-03-27"
        );

        let path = Path::new("C:/Users/Mia/.codex/archived_sessions/2026/03/26/rollout.jsonl");
        let path_day = extract_day_from_path(path).expect("archived path day");
        let missing = resolve_event_timestamp(&serde_json::json!({}), Some(path_day))
            .expect("path date fallback");
        assert_eq!(
            missing.with_timezone(&Local).date_naive().to_string(),
            "2026-03-26"
        );
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
        // AI Token Monitor uses the reported total. With no total_tokens field,
        // Codex's input + output fallback is 100 + 20 = 120; cached input is
        // already a subset of input and is not added again or subtracted.
        assert_eq!(first.today_tokens, 120);
        assert_eq!(first.today_context_tokens, 120);
        assert_eq!(first.today_cache_read_tokens, 40);
        assert_eq!(first.today_cache_creation_tokens, 0);
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

        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&log)
            .expect("reopen fixture log");
        writeln!(file, "{incomplete}").expect("append duplicate across refresh boundary");
        drop(file);
        let duplicate_tail = collector
            .collect(&root, Local::now())
            .expect("collect duplicate tail snapshot");
        assert_eq!(duplicate_tail.today_tokens, 180);
        assert_eq!(duplicate_tail.today_requests, 2);

        fs::remove_dir_all(root).expect("remove temporary usage root");
    }

    #[test]
    fn read_increment_distinguishes_bounded_refresh_from_full_backfill() {
        let root = temporary_root();
        let log = root.join("session.jsonl");
        let first = token_line(100, 0, 10);
        let second = token_line(200, 0, 20);
        let third = token_line(300, 0, 30);
        fs::write(&log, format!("{first}\n{second}\n{third}\n"))
            .expect("write bounded read fixture");

        let mut state = FileUsage::new(&log);
        read_increment(&log, &mut state, Some(1)).expect("bounded read");
        let file_length = fs::metadata(&log).expect("read fixture metadata").len();
        assert!(state.offset < file_length);
        assert_eq!(state.events.len(), 1);

        read_increment(&log, &mut state, None).expect("full backfill read");
        assert_eq!(state.offset, file_length);
        assert_eq!(state.events.len(), 3);

        let partial = token_line(400, 0, 40);
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&log)
            .expect("open partial read fixture");
        write!(file, "{partial}").expect("append partial line");
        drop(file);
        read_increment(&log, &mut state, None).expect("ignore partial tail");
        assert_eq!(state.events.len(), 3);

        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&log)
            .expect("reopen partial read fixture");
        writeln!(file).expect("complete partial line");
        drop(file);
        read_increment(&log, &mut state, None).expect("resume completed tail");
        assert_eq!(state.events.len(), 4);

        fs::remove_dir_all(root).expect("remove bounded read fixture");
    }

    #[test]
    fn collector_replaces_same_size_rewrites_instead_of_double_counting() {
        let root = temporary_root();
        let log = root.join("session.jsonl");
        fs::write(&log, format!("{}\n", token_line(100, 0, 0))).expect("write original fixture");
        let mut collector = UsageCollector::default();
        let original = collector
            .collect(&root, Local::now())
            .expect("collect original fixture");
        assert_eq!(original.today_tokens, 100);

        std::thread::sleep(Duration::from_millis(5));
        fs::write(&log, format!("{}\n", token_line(200, 0, 0)))
            .expect("rewrite fixture with same length");
        let rewritten = collector
            .collect(&root, Local::now())
            .expect("collect rewritten fixture");
        assert_eq!(rewritten.today_tokens, 200);
        assert_eq!(rewritten.today_requests, 1);

        fs::remove_dir_all(root).expect("remove rewritten fixture");
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
    fn collector_publishes_local_streak_and_weekly_report_fields() {
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
        assert!((snapshot.yesterday_cost_usd - 0.0005).abs() < f64::EPSILON);
        assert_eq!(snapshot.yesterday_top_project.as_deref(), Some("MochiPort"));
        let weekly = snapshot.weekly_report.expect("weekly report");
        assert_eq!(weekly.last_week_tokens, 900);
        assert!((weekly.last_week_cost_usd - 0.00225).abs() < f64::EPSILON);
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
                token_line_at(now - chrono::Duration::days(1), 101, 0, 0),
                token_line_at(now, 102, 0, 0),
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
