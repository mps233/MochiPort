//! Locating rollout files and accumulating usage from them.
//!
//! Codex writes `<root>/<year>/<month>/<day>/rollout-<timestamp>-<uuid>.jsonl`.
//! Scanning is incremental: a caller keeps the byte offset returned by the
//! previous scan and only appended records are read the next time.

use std::{
    collections::BTreeMap,
    fs::File,
    io::{BufRead, BufReader, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use super::log::{
    RateLimits, RolloutEvent, SessionMeta, ThreadSettings, TokenUsage, epoch_minute, hour_of_day,
    parse_record,
};

const ROLLOUT_PREFIX: &str = "rollout-";
const ROLLOUT_SUFFIX: &str = ".jsonl";

/// Live sessions, laid out as `<sessions>/<year>/<month>/<day>/rollout-*.jsonl`.
const SESSIONS_DIR: &str = "sessions";

/// Sessions Codex has archived. Flat: `<archived_sessions>/rollout-*.jsonl`.
const ARCHIVED_SESSIONS_DIR: &str = "archived_sessions";

/// Model label for samples that predate any model information.
///
/// Matches the clients' `state.currentModel.isEmpty ? "codex" : currentModel`.
const DEFAULT_CODEX_MODEL: &str = "codex";

/// Provider label for sessions whose logs carry no provider.
///
/// Matches the clients' `source` default and the `DEFAULT 'legacy'` in the macOS
/// history schema.
const LEGACY_PROVIDER: &str = "legacy";

/// Usage accumulated from one rollout file.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct SessionScan {
    pub meta: SessionMeta,
    pub usage: TokenUsage,
    pub rate_limits: RateLimits,
    pub context_window: Option<u64>,
    /// Timestamp of the most recent record that carried a quota window, used to
    /// pick the newest sample across sessions.
    pub quota_observed_at: Option<String>,
    /// Quota readings keyed by `(minute, kind)` for the recent window.
    ///
    /// The clients' short-horizon depletion estimate works from a minute-level
    /// percentage series (`percentHistory`, trimmed to the last 60 minutes), so a
    /// per-day history alone cannot reproduce it, and the samples have to carry
    /// the absolute minute for the same reason the token buckets do.
    pub quota_minutes: BTreeMap<(i64, QuotaKind), f64>,
    /// Quota observations keyed by `(day, kind)`, keeping the last reading of
    /// each day.
    ///
    /// The clients persist one percentage per day and window kind (their table is
    /// keyed by `(day, service, kind)` and written with `INSERT OR REPLACE`), and
    /// they read that history to estimate weekly depletion. Keeping only the
    /// latest reading overall — as this used to — made that history impossible to
    /// reproduce from the daemon.
    pub quota_history: BTreeMap<(String, QuotaKind), f64>,
    /// The model/provider in effect when the last record was read. Codex can
    /// change these mid-session, so this is a snapshot, not a constant.
    pub settings: Option<ThreadSettings>,
    /// Usage per hour of day (0-23), for the HUD's recent-activity series.
    pub hourly: BTreeMap<u8, TokenUsage>,
    /// Replay identities already counted in this file.
    ///
    /// Resume and subagent rollouts copy earlier `token_count` records into
    /// another file, so the same turn can appear more than once. Records carrying
    /// both a session id and a cumulative snapshot are matched on
    /// (session, model, per-turn counters, cumulative counters) and only the first
    /// occurrence is counted — the same rule both clients implement.
    seen_replays: std::collections::BTreeSet<ReplayKey>,
    /// Usage per absolute minute (minutes since the Unix epoch).
    ///
    /// The clients' live rates work over absolute minutes: `activeBaselineRate`
    /// averages the last 24 hours, which crosses midnight, so a minute-of-day
    /// index would merge the same clock minute on different days.
    pub minutely: BTreeMap<i64, TokenUsage>,
    /// Usage by model, attributed using the settings in effect when each sample
    /// was recorded. Codex re-emits `thread_settings_applied` on every change,
    /// so attribution follows the timeline rather than the session's first or
    /// last setting.
    pub models: BTreeMap<String, TokenUsage>,
    /// Usage by provider (`model_provider_id`), attributed the same way.
    pub providers: BTreeMap<String, TokenUsage>,
    /// The last `turn_context` seen, used to fill in a model or working
    /// directory that `session_meta` / `thread_settings_applied` did not carry.
    pub turn_context: Option<super::log::TurnContext>,
    /// Usage keyed by `(model, provider)` so callers can rebuild a cross-product
    /// breakdown without re-reading the log. The macOS history database stores
    /// rows per (day, service, source, model, project), so single-dimension
    /// totals are not enough: the combinations are what it replaces.
    pub by_model_provider: BTreeMap<(String, String), TokenUsage>,
    /// How many `token_count` records contributed to `usage`.
    pub samples: usize,
}

/// Usage rolled up to one calendar day.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DailyUsage {
    pub date: String,
    pub usage: TokenUsage,
    pub sessions: usize,
}

/// Every directory holding rollout files, live sessions first.
///
/// Codex moves finished sessions to `archived_sessions`, which the desktop
/// clients scan as a second root. Ignoring it dropped 54 local sessions
/// (348M tokens) from every summary, and because a history rebuild *replaces*
/// the day's rows, those days would have been deleted rather than merely
/// missing.
///
/// The two trees have different layouts: `sessions` nests `YYYY/MM/DD`, while
/// `archived_sessions` is flat. Both are returned and the caller derives each
/// file's day from whichever it has.
pub(crate) fn sessions_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = std::env::var_os("CODEX_HOME") {
        push_codex_roots(&mut roots, Path::new(&home));
    }
    // `CODEX_HOME` replaces the default location rather than adding to it, which
    // is how Codex itself resolves it.
    if roots.is_empty()
        && let Some(home) = std::env::var_os("HOME")
    {
        push_codex_roots(&mut roots, &PathBuf::from(home).join(".codex"));
    }
    roots
}

fn push_codex_roots(roots: &mut Vec<PathBuf>, codex_home: &Path) {
    for name in [SESSIONS_DIR, ARCHIVED_SESSIONS_DIR] {
        let candidate = codex_home.join(name);
        if candidate.is_dir() {
            roots.push(candidate);
        }
    }
}

/// Whether a path looks like a rollout file rather than some other artifact in
/// the sessions tree.
pub(crate) fn is_rollout_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with(ROLLOUT_PREFIX) && name.ends_with(ROLLOUT_SUFFIX))
}

/// The calendar day a rollout file belongs to, from its path.
///
/// Live sessions sit in a `<year>/<month>/<day>` directory, which is the pairing
/// the service layer already buckets by, so that is preferred. Archived sessions
/// are flat and fall back to the `rollout-<YYYY-MM-DD>T…` file name — the same
/// date, since Codex names the file after the session's start (verified on this
/// machine: 400 live files, zero disagreements between the two).
///
/// Returns `None` when neither carries a usable date, so the caller can fall
/// back to the records' own timestamps rather than inventing a day.
pub(crate) fn rollout_day_from_path(path: &Path) -> Option<String> {
    if let Some(directory) = path.parent()
        && let Some(day) = directory.file_name().and_then(|name| name.to_str())
        && let Some(month) = directory
            .parent()
            .and_then(|parent| parent.file_name())
            .and_then(|name| name.to_str())
        && let Some(year) = directory
            .parent()
            .and_then(|parent| parent.parent())
            .and_then(|parent| parent.file_name())
            .and_then(|name| name.to_str())
        && let Some(date) = session_date_from_timestamp(&format!("{year}-{month}-{day}"))
    {
        return Some(date);
    }
    let name = path.file_name()?.to_str()?;
    let date = name.strip_prefix(ROLLOUT_PREFIX)?.get(0..10)?;
    session_date_from_timestamp(date)
}

/// Identity of a usage record that can be recognised as a replay.
///
/// Resume and subagent rollouts copy earlier `token_count` records into another
/// file, so the same turn appears twice. Both clients collapse these by matching
/// the session, model, the per-turn four-tuple and the cumulative five-tuple, and
/// keep the earliest occurrence globally. Doing it here keeps one implementation
/// instead of two.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ReplayKey {
    session_id: String,
    model: String,
    input: u64,
    cached: u64,
    output: u64,
    total: u64,
    cumulative: (u64, u64, u64, u64, u64),
}

impl ReplayKey {
    /// `None` when the record cannot be safely identified as a replay.
    ///
    /// Without a session id or a cumulative snapshot there is nothing to match
    /// on, and treating such records as replays would drop real usage.
    fn of(
        session_id: Option<&str>,
        model: Option<&str>,
        count: &super::log::TokenCount,
    ) -> Option<Self> {
        let session_id = session_id?;
        let cumulative = count.cumulative?;
        Some(ReplayKey {
            session_id: session_id.to_owned(),
            model: model.unwrap_or_default().to_owned(),
            input: count.usage.input_tokens,
            cached: count.usage.cached_input_tokens,
            output: count.usage.output_tokens,
            total: count.usage.total_tokens,
            cumulative: (
                cumulative.input_tokens,
                cumulative.cached_input_tokens,
                cumulative.output_tokens,
                cumulative.reasoning_output_tokens,
                cumulative.total_tokens,
            ),
        })
    }
}

/// Which quota window a reading belongs to.
///
/// Named after the record position rather than a fixed duration: measured
/// records carry a 30-day window under `primary`, so callers classify by
/// `window_minutes` when they need the duration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum QuotaKind {
    Primary,
    Secondary,
}

/// Extracts `YYYY-MM-DD` from a Codex timestamp, matching the clients' local-day
/// bucketing.
pub(crate) fn session_date_from_timestamp(timestamp: &str) -> Option<String> {
    let date = timestamp.get(0..10)?;
    let bytes = date.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    if !bytes
        .iter()
        .enumerate()
        .all(|(index, byte)| index == 4 || index == 7 || byte.is_ascii_digit())
    {
        return None;
    }
    Some(date.to_owned())
}

/// Last path segment of a working directory, i.e. the project name.
pub(crate) fn project_name(cwd: &str) -> Option<&str> {
    cwd.trim_end_matches('/')
        .rsplit('/')
        .find(|segment| !segment.is_empty())
}

impl SessionScan {
    /// The model in effect: an explicit settings change wins, otherwise the
    /// per-turn context. Sessions that never re-emit settings still attribute.
    pub(crate) fn effective_model(&self) -> Option<&str> {
        self.settings
            .as_ref()
            .and_then(|settings| settings.model.as_deref())
            .or_else(|| {
                self.turn_context
                    .as_ref()
                    .and_then(|context| context.model.as_deref())
            })
    }

    /// The model to attribute a sample to, using the clients' fallback.
    ///
    /// Both clients record `"codex"` when a sample predates any model information
    /// (the first turns of a session, before its first `turn_context`). Returning
    /// `None` there would drop those samples from the breakdown, and the client
    /// history database is keyed by model, so an unlabelled sample cannot be
    /// written at all.
    pub(crate) fn attributed_model(&self) -> &str {
        self.effective_model().unwrap_or(DEFAULT_CODEX_MODEL)
    }

    /// The provider to attribute a sample to, using the clients' fallback.
    ///
    /// The provider is resolved in two steps. `thread_settings_applied` carries
    /// the precise provider in effect at the sample's position in the timeline
    /// (`ai-gateway`, `custom`, …) and wins when present. Otherwise the session's
    /// `session_meta.model_provider` applies, which is what the desktop clients
    /// read for their `source` column and is present on essentially every local
    /// session. `"legacy"` is the clients' final fallback for older logs and
    /// matches the `DEFAULT 'legacy'` in their schema.
    ///
    /// Resolving this eagerly is what keeps the breakdown complete: gating on the
    /// settings event alone dropped every sample from a session that never emitted
    /// one, losing about a sixth of all usage.
    pub(crate) fn attributed_provider(&self) -> &str {
        self.settings
            .as_ref()
            .and_then(|settings| settings.provider.as_deref())
            .or(self.meta.provider.as_deref())
            .unwrap_or(LEGACY_PROVIDER)
    }

    /// Project the session ran in, derived from the session's working directory.
    /// Falls back to the per-turn context when `session_meta` omitted it.
    pub(crate) fn project(&self) -> Option<&str> {
        self.meta
            .cwd
            .as_deref()
            .or_else(|| {
                self.turn_context
                    .as_ref()
                    .and_then(|context| context.cwd.as_deref())
            })
            .and_then(project_name)
    }
}

/// Reads a rollout file starting at `offset`.
///
/// Returns the accumulated usage together with the offset to pass in next time.
/// A trailing record without a newline is left unread: the writer may still be
/// appending to it.
pub(crate) fn scan_from(path: &Path, offset: u64) -> std::io::Result<(SessionScan, u64)> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    let start = offset.min(length);
    file.seek(SeekFrom::Start(start))?;

    let mut reader = BufReader::new(file);
    let mut scan = SessionScan::default();
    let mut line = String::new();
    let mut consumed = start;

    loop {
        line.clear();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            break;
        }
        if !line.ends_with('\n') {
            break;
        }
        consumed += read as u64;
        match parse_record(&line) {
            Some(RolloutEvent::SessionMeta(meta)) => scan.meta = merge_meta(scan.meta, meta),
            Some(RolloutEvent::TokenCount(count)) => {
                if let Some(key) = ReplayKey::of(
                    scan.meta.session_id.as_deref(),
                    scan.effective_model(),
                    &count,
                ) && !scan.seen_replays.insert(key)
                {
                    // A copy of a turn already counted in this file.
                    continue;
                }
                scan.samples += 1;
                // Per-turn counters are summed, matching the macOS and Windows
                // implementations; cumulative counters would overstate usage.
                accumulate(&mut scan.usage, count.usage);
                if let Some(hour) = count.timestamp.as_deref().and_then(hour_of_day) {
                    accumulate(scan.hourly.entry(hour).or_default(), count.usage);
                }
                if let Some(minute) = count.timestamp.as_deref().and_then(epoch_minute) {
                    accumulate(scan.minutely.entry(minute).or_default(), count.usage);
                }
                // Attribute to whatever settings were in effect for this sample.
                // Both axes resolve through a fallback so no sample is dropped:
                // the breakdown is what the client history database is keyed by,
                // and a sample with no model/provider cannot be written at all.
                let model = scan.attributed_model().to_owned();
                let provider = scan.attributed_provider().to_owned();
                accumulate(scan.models.entry(model.clone()).or_default(), count.usage);
                accumulate(
                    scan.providers.entry(provider.clone()).or_default(),
                    count.usage,
                );
                accumulate(
                    scan.by_model_provider.entry((model, provider)).or_default(),
                    count.usage,
                );
                scan.context_window = count.model_context_window.or(scan.context_window);
                if count.rate_limits.has_window() {
                    scan.quota_observed_at = count.timestamp.clone();
                    // Record per-day history before the latest-reading snapshot
                    // overwrites the previous value.
                    if let Some(day) = count
                        .timestamp
                        .as_deref()
                        .and_then(session_date_from_timestamp)
                    {
                        let minute = count.timestamp.as_deref().and_then(epoch_minute);
                        for (kind, window) in [
                            (QuotaKind::Primary, count.rate_limits.primary.as_ref()),
                            (QuotaKind::Secondary, count.rate_limits.secondary.as_ref()),
                        ] {
                            if let Some(window) = window {
                                scan.quota_history
                                    .insert((day.clone(), kind), window.used_percent);
                                if let Some(minute) = minute {
                                    scan.quota_minutes
                                        .insert((minute, kind), window.used_percent);
                                }
                            }
                        }
                    }
                    scan.rate_limits = count.rate_limits;
                }
            }
            Some(RolloutEvent::ThreadSettings(settings)) => scan.settings = Some(settings),
            Some(RolloutEvent::TurnContext(context)) => {
                // Broader model source than `thread_settings_applied`, and it can
                // supply a working directory when `session_meta` did not.
                scan.turn_context = Some(context);
            }
            None => {}
        }
    }

    Ok((scan, consumed))
}

/// Adds one turn's counters into a running total.
pub(super) fn accumulate(total: &mut TokenUsage, addition: TokenUsage) {
    total.input_tokens = total.input_tokens.saturating_add(addition.input_tokens);
    total.cached_input_tokens = total
        .cached_input_tokens
        .saturating_add(addition.cached_input_tokens);
    total.cache_write_input_tokens = total
        .cache_write_input_tokens
        .saturating_add(addition.cache_write_input_tokens);
    total.output_tokens = total.output_tokens.saturating_add(addition.output_tokens);
    total.reasoning_output_tokens = total
        .reasoning_output_tokens
        .saturating_add(addition.reasoning_output_tokens);
    total.total_tokens = total.total_tokens.saturating_add(addition.total_tokens);
}

/// Fills gaps in `existing` from `update` without discarding what is known.
fn merge_meta(existing: SessionMeta, update: SessionMeta) -> SessionMeta {
    SessionMeta {
        session_id: update.session_id.or(existing.session_id),
        thread_id: update.thread_id.or(existing.thread_id),
        cwd: update.cwd.or(existing.cwd),
        originator: update.originator.or(existing.originator),
        provider: update.provider.or(existing.provider),
        started_at: existing.started_at.or(update.started_at),
    }
}

/// Rolls per-session scans up by calendar day, newest day first.
pub(crate) fn aggregate_by_day(scans: &[(String, SessionScan)]) -> Vec<DailyUsage> {
    let mut days: std::collections::BTreeMap<&str, DailyUsage> = std::collections::BTreeMap::new();
    for (date, scan) in scans {
        let entry = days.entry(date.as_str()).or_insert_with(|| DailyUsage {
            date: date.clone(),
            usage: TokenUsage::default(),
            sessions: 0,
        });
        entry.usage.input_tokens += scan.usage.input_tokens;
        entry.usage.cached_input_tokens += scan.usage.cached_input_tokens;
        entry.usage.cache_write_input_tokens += scan.usage.cache_write_input_tokens;
        entry.usage.output_tokens += scan.usage.output_tokens;
        entry.usage.reasoning_output_tokens += scan.usage.reasoning_output_tokens;
        entry.usage.total_tokens += scan.usage.total_tokens;
        entry.sessions += 1;
    }
    let mut collected: Vec<DailyUsage> = days.into_values().collect();
    collected.sort_by(|left, right| right.date.cmp(&left.date));
    collected
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    const SESSION_META_LINE: &str = r#"{"timestamp":"2026-08-25T02:16:45.551Z","ordinal":0,"type":"session_meta","payload":{"session_id":"s1","id":"t1","cwd":"/Users/example/project","originator":"Codex Desktop"}}"#;

    fn token_count_line(total: u64) -> String {
        format!(
            r#"{{"timestamp":"2026-08-25T02:17:02.910Z","ordinal":1,"type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":{total},"cached_input_tokens":0,"cache_write_input_tokens":0,"output_tokens":5,"reasoning_output_tokens":1,"total_tokens":{}}}}},"rate_limits":{{"limit_id":"codex","primary":null,"secondary":null}}}}}}"#,
            total + 6
        )
    }

    fn rollout(directory: &Path, day: &str, records: &[String]) -> PathBuf {
        let path = directory.join(day);
        std::fs::create_dir_all(&path).expect("create day directory");
        let file = path.join("rollout-2026-08-25T02-16-45-01a036b4.jsonl");
        let mut handle = File::create(&file).expect("create rollout file");
        for record in records {
            writeln!(handle, "{record}").expect("write record");
        }
        handle.flush().expect("flush");
        file
    }

    #[test]
    fn recognises_rollout_files_only() {
        assert!(is_rollout_file(Path::new(
            "/a/b/rollout-2026-08-25T02-16-45-x.jsonl"
        )));
        assert!(!is_rollout_file(Path::new("/a/b/session_index.jsonl")));
        assert!(!is_rollout_file(Path::new("/a/b/rollout-x.json")));
    }

    #[test]
    fn reads_meta_and_sums_per_turn_usage() {
        let directory = tempfile::tempdir().expect("temp dir");
        let file = rollout(
            directory.path(),
            "2026/08/25",
            &[
                SESSION_META_LINE.to_owned(),
                token_count_line(100),
                token_count_line(250),
            ],
        );

        let (scan, offset) = scan_from(&file, 0).expect("scan succeeds");
        assert_eq!(scan.meta.cwd.as_deref(), Some("/Users/example/project"));
        assert_eq!(scan.samples, 2);
        // Per-turn counters are summed across the two records.
        assert_eq!(scan.usage.input_tokens, 350);
        assert!(offset > 0);
    }

    /// Reading the same file twice must not double count.
    #[test]
    fn resuming_from_the_returned_offset_reads_nothing_new() {
        let directory = tempfile::tempdir().expect("temp dir");
        let file = rollout(directory.path(), "2026/08/25", &[token_count_line(250)]);

        let (_first, offset) = scan_from(&file, 0).expect("first scan");
        let (second, offset_after) = scan_from(&file, offset).expect("second scan");
        assert_eq!(second.samples, 0);
        assert_eq!(second.usage.total_tokens, 0);
        assert_eq!(offset, offset_after);
    }

    #[test]
    fn appended_records_are_picked_up_on_resume() {
        let directory = tempfile::tempdir().expect("temp dir");
        let file = rollout(directory.path(), "2026/08/25", &[token_count_line(250)]);
        let (_first, offset) = scan_from(&file, 0).expect("first scan");

        let mut handle = std::fs::OpenOptions::new()
            .append(true)
            .open(&file)
            .expect("open for append");
        writeln!(handle, "{}", token_count_line(900)).expect("append");
        handle.flush().expect("flush");

        let (second, _) = scan_from(&file, offset).expect("resume");
        assert_eq!(second.samples, 1);
        assert_eq!(second.usage.input_tokens, 900);
    }

    /// A partially written trailing record must not be parsed as a whole one.
    #[test]
    fn leaves_a_trailing_partial_record_for_the_next_scan() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("2026/08/25");
        std::fs::create_dir_all(&path).expect("create day directory");
        let file = path.join("rollout-2026-08-25T02-16-45-01a036b4.jsonl");
        std::fs::write(
            &file,
            format!(
                "{}\n{}",
                token_count_line(250),
                r#"{"type":"event_msg","payl"#
            ),
        )
        .expect("write file");

        let (scan, offset) = scan_from(&file, 0).expect("scan succeeds");
        assert_eq!(scan.samples, 1);
        assert_eq!(scan.usage.input_tokens, 250);
        assert!(offset < std::fs::metadata(&file).expect("metadata").len());
    }

    #[test]
    fn buckets_usage_by_hour_of_day() {
        let directory = tempfile::tempdir().expect("temp dir");
        let day = directory.path().join("2026/08/25");
        std::fs::create_dir_all(&day).expect("create day directory");
        let file = day.join("rollout-2026-08-25T02-16-45-x.jsonl");

        let record = |hour: &str, total: u64| {
            format!(
                r#"{{"timestamp":"2026-08-25T{hour}:00:00.000Z","type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{{"input_tokens":{total},"output_tokens":1,"total_tokens":{}}}}}}}}}"#,
                total + 1
            )
        };
        // A trailing newline matters: `scan_from` deliberately leaves a partial
        // final record for the next scan, so the last line would be skipped.
        std::fs::write(
            &file,
            format!(
                "{}\n{}\n{}\n",
                record("02", 10),
                record("02", 20),
                record("05", 30)
            ),
        )
        .expect("write file");

        let (scan, _) = scan_from(&file, 0).expect("scan succeeds");
        assert_eq!(scan.samples, 3);
        assert_eq!(scan.hourly.len(), 2);
        assert_eq!(scan.hourly[&2].input_tokens, 30);
        assert_eq!(scan.hourly[&5].input_tokens, 30);
    }

    /// A session that switches model mid-way must not attribute later usage to
    /// the earlier model.
    #[test]
    fn attributes_usage_to_the_settings_in_effect_at_the_time() {
        let directory = tempfile::tempdir().expect("temp dir");
        let day = directory.path().join("2026/08/25");
        std::fs::create_dir_all(&day).expect("create day directory");
        let file = day.join("rollout-2026-08-25T02-16-45-x.jsonl");

        let settings = |model: &str, provider: &str| {
            format!(
                r#"{{"type":"event_msg","payload":{{"type":"thread_settings_applied","thread_settings":{{"model":"{model}","model_provider_id":"{provider}"}}}}}}"#
            )
        };
        let token = |total: u64| {
            format!(
                r#"{{"timestamp":"2026-08-25T02:00:00.000Z","type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{{"input_tokens":{total},"output_tokens":1,"total_tokens":{}}}}}}}}}"#,
                total + 1
            )
        };

        std::fs::write(
            &file,
            format!(
                "{}\n{}\n{}\n{}\n",
                settings("model-a", "provider-x"),
                token(10),
                settings("model-b", "provider-y"),
                token(20)
            ),
        )
        .expect("write file");

        let (scan, _) = scan_from(&file, 0).expect("scan succeeds");
        assert_eq!(scan.samples, 2);
        assert_eq!(scan.models.len(), 2);
        assert_eq!(scan.models["model-a"].input_tokens, 10);
        assert_eq!(scan.models["model-b"].input_tokens, 20);
        assert_eq!(scan.providers["provider-x"].input_tokens, 10);
        assert_eq!(scan.providers["provider-y"].input_tokens, 20);
        // The session total is unaffected by attribution.
        assert_eq!(scan.usage.input_tokens, 30);
    }

    /// The cross product is what the macOS history database replaces rows by, so
    /// a switched session must produce two combinations, not one merged bucket.
    #[test]
    fn keeps_model_and_provider_combinations_separate() {
        let directory = tempfile::tempdir().expect("temp dir");
        let day = directory.path().join("2026/08/25");
        std::fs::create_dir_all(&day).expect("create day directory");
        let file = day.join("rollout-2026-08-25T02-16-45-x.jsonl");

        let settings = |model: &str, provider: &str| {
            format!(
                r#"{{"type":"event_msg","payload":{{"type":"thread_settings_applied","thread_settings":{{"model":"{model}","model_provider_id":"{provider}"}}}}}}"#
            )
        };
        let token = |total: u64| {
            format!(
                r#"{{"timestamp":"2026-08-25T02:00:00.000Z","type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{{"input_tokens":{total},"output_tokens":1,"total_tokens":{}}}}}}}}}"#,
                total + 1
            )
        };

        std::fs::write(
            &file,
            format!(
                "{}\n{}\n{}\n{}\n",
                settings("model-a", "provider-x"),
                token(10),
                settings("model-a", "provider-y"),
                token(20)
            ),
        )
        .expect("write file");

        let (scan, _) = scan_from(&file, 0).expect("scan succeeds");
        assert_eq!(scan.by_model_provider.len(), 2);
        assert_eq!(
            scan.by_model_provider[&("model-a".to_owned(), "provider-x".to_owned())].input_tokens,
            10
        );
        assert_eq!(
            scan.by_model_provider[&("model-a".to_owned(), "provider-y".to_owned())].input_tokens,
            20
        );
        // The model total still aggregates both providers.
        assert_eq!(scan.models["model-a"].input_tokens, 30);
    }

    /// Samples recorded before any settings event carry no attribution rather
    /// than being guessed at.
    /// A session that never emits settings still contributes to the breakdown.
    ///
    /// This used to leave the sample unattributed, which dropped it from
    /// `by_model_provider` — the only source of the cross-product rows the client
    /// history database is keyed by — losing roughly a sixth of all local usage.
    /// The clients label such samples `codex` / `legacy`, so the daemon does too.
    #[test]
    fn attributes_samples_without_settings_using_the_client_fallbacks() {
        let directory = tempfile::tempdir().expect("temp dir");
        let file = rollout(directory.path(), "2026/08/25", &[token_count_line(250)]);

        let (scan, _) = scan_from(&file, 0).expect("scan succeeds");
        assert_eq!(scan.samples, 1);
        assert_eq!(scan.attributed_model(), "codex");
        assert_eq!(scan.attributed_provider(), "legacy");
        assert_eq!(scan.models["codex"].total_tokens, 256);
        assert_eq!(scan.providers["legacy"].total_tokens, 256);
        assert_eq!(
            scan.by_model_provider[&("codex".to_owned(), "legacy".to_owned())].total_tokens,
            256
        );
    }

    /// `session_meta.model_provider` is the clients' `source`, and it covers the
    /// sessions that never emit a settings event.
    #[test]
    fn falls_back_to_the_session_provider_when_settings_never_arrive() {
        let directory = tempfile::tempdir().expect("temp dir");
        let meta = r#"{"timestamp":"2026-08-25T02:16:45.551Z","ordinal":0,"type":"session_meta","payload":{"session_id":"s1","cwd":"/Users/example/project","model_provider":"MochiPort"}}"#;
        let context = r#"{"timestamp":"2026-08-25T02:16:46.551Z","type":"turn_context","payload":{"model":"gpt-5.6-sol","cwd":"/Users/example/project"}}"#;
        let file = rollout(
            directory.path(),
            "2026/08/25",
            &[meta.to_owned(), context.to_owned(), token_count_line(250)],
        );

        let (scan, _) = scan_from(&file, 0).expect("scan succeeds");
        assert_eq!(scan.attributed_model(), "gpt-5.6-sol");
        assert_eq!(scan.attributed_provider(), "MochiPort");
        assert_eq!(
            scan.by_model_provider[&("gpt-5.6-sol".to_owned(), "MochiPort".to_owned())]
                .total_tokens,
            256
        );
    }

    /// The timeline provider from `thread_settings_applied` is more precise than
    /// the session-level one, so it still wins when both are present.
    #[test]
    fn prefers_the_timeline_provider_over_the_session_provider() {
        let directory = tempfile::tempdir().expect("temp dir");
        let meta = r#"{"timestamp":"2026-08-25T02:16:45.551Z","ordinal":0,"type":"session_meta","payload":{"session_id":"s1","cwd":"/p","model_provider":"MochiPort"}}"#;
        let settings = r#"{"type":"event_msg","payload":{"type":"thread_settings_applied","thread_settings":{"model":"m","model_provider_id":"ai-gateway"}}}"#;
        let file = rollout(
            directory.path(),
            "2026/08/25",
            &[meta.to_owned(), settings.to_owned(), token_count_line(250)],
        );

        let (scan, _) = scan_from(&file, 0).expect("scan succeeds");
        assert_eq!(scan.attributed_provider(), "ai-gateway");
        assert_eq!(scan.providers["ai-gateway"].total_tokens, 256);
        assert!(scan.providers.get("MochiPort").is_none());
    }

    /// Every sample must land in the breakdown, so its totals add up.
    #[test]
    fn breakdown_covers_every_counted_sample() {
        let directory = tempfile::tempdir().expect("temp dir");
        let settings = r#"{"type":"event_msg","payload":{"type":"thread_settings_applied","thread_settings":{"model":"m","model_provider_id":"ai-gateway"}}}"#;
        let file = rollout(
            directory.path(),
            "2026/08/25",
            &[
                // Before any settings event: needs the fallbacks.
                token_count_line(100),
                settings.to_owned(),
                // After it: attributed to the timeline provider.
                token_count_line(200),
            ],
        );

        let (scan, _) = scan_from(&file, 0).expect("scan succeeds");
        let breakdown: u64 = scan
            .by_model_provider
            .values()
            .map(|usage| usage.total_tokens)
            .sum();
        assert_eq!(
            breakdown, scan.usage.total_tokens,
            "breakdown must account for every sample, otherwise history rows go missing"
        );
    }

    #[test]
    fn buckets_usage_by_minute_of_day() {
        let directory = tempfile::tempdir().expect("temp dir");
        let day = directory.path().join("2026/08/25");
        std::fs::create_dir_all(&day).expect("create day directory");
        let file = day.join("rollout-2026-08-25T02-16-45-x.jsonl");

        let record = |timestamp: &str, total: u64| {
            format!(
                r#"{{"timestamp":"{timestamp}","type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{{"input_tokens":{total},"output_tokens":1,"total_tokens":{}}}}}}}}}"#,
                total + 1
            )
        };
        // Two samples in the same minute, one in the next.
        std::fs::write(
            &file,
            format!(
                "{}\n{}\n{}\n",
                record("2026-08-25T02:17:02.000Z", 10),
                record("2026-08-25T02:17:45.000Z", 20),
                record("2026-08-25T02:18:01.000Z", 30)
            ),
        )
        .expect("write file");

        let (scan, _) = scan_from(&file, 0).expect("scan succeeds");
        assert_eq!(scan.samples, 3);
        assert_eq!(scan.minutely.len(), 2);
        let first = epoch_minute("2026-08-25T02:17:00Z").expect("minute");
        let second = epoch_minute("2026-08-25T02:18:00Z").expect("minute");
        assert_eq!(scan.minutely[&first].input_tokens, 30);
        assert_eq!(scan.minutely[&second].input_tokens, 30);
        // The hour bucket still aggregates both minutes.
        assert_eq!(scan.hourly[&2].input_tokens, 60);
    }

    /// The clients store one percentage per day, so the daemon has to collapse a
    /// day's readings deterministically.
    #[test]
    fn probe_scan_quota() {
        let line = r#"{"timestamp":"2026-08-25T02:00:00Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}},"rate_limits":{"primary":{"used_percent":10.0,"window_minutes":300}}}}"#;
        let parsed = parse_record(line);
        eprintln!("parsed = {:?}", parsed.is_some());
        if let Some(RolloutEvent::TokenCount(count)) = parsed {
            eprintln!("has_window = {}", count.rate_limits.has_window());
            eprintln!("timestamp = {:?}", count.timestamp);
            eprintln!(
                "date = {:?}",
                count
                    .timestamp
                    .as_deref()
                    .and_then(session_date_from_timestamp)
            );
        }
    }

    #[test]
    fn keeps_the_last_quota_reading_within_a_file() {
        let directory = tempfile::tempdir().expect("temp dir");
        let day = directory.path().join("2026/08/25");
        std::fs::create_dir_all(&day).expect("create day directory");
        let file = day.join("rollout-2026-08-25T02-16-45-x.jsonl");

        // Built with serde_json rather than a format string: hand-escaping this
        // nested JSON produced an unbalanced brace and the records silently
        // failed to parse.
        let record = |timestamp: &str, percent: f64| {
            serde_json::json!({
                "timestamp": timestamp,
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "last_token_usage": {
                            "input_tokens": 1,
                            "output_tokens": 1,
                            "total_tokens": 2
                        }
                    },
                    "rate_limits": {
                        "primary": { "used_percent": percent, "window_minutes": 300 }
                    }
                }
            })
            .to_string()
        };
        // Rises, then falls back — the peak must survive.
        std::fs::write(
            &file,
            format!(
                "{}\n{}\n{}\n",
                record("2026-08-25T02:00:00Z", 10.0),
                record("2026-08-25T03:00:00Z", 42.0),
                record("2026-08-25T04:00:00Z", 7.0)
            ),
        )
        .expect("write file");

        let (scan, _) = scan_from(&file, 0).expect("scan succeeds");
        assert_eq!(scan.samples, 3);
        assert_eq!(scan.quota_history.len(), 1);
        // Within a single file the last record wins; collapsing a day across
        // sessions is the service layer's job, which keeps the day's peak.
        let key = ("2026-08-25".to_owned(), QuotaKind::Primary);
        assert!((scan.quota_history[&key] - 7.0).abs() < 1e-9);
    }

    /// Two days must produce two points, not one.
    #[test]
    fn separates_quota_history_by_day() {
        let directory = tempfile::tempdir().expect("temp dir");
        let mut files = Vec::new();
        for (day_path, timestamp, percent) in [
            ("2026/08/24", "2026-08-24T02:00:00Z", 10.0),
            ("2026/08/25", "2026-08-25T02:00:00Z", 20.0),
        ] {
            let day = directory.path().join(day_path);
            std::fs::create_dir_all(&day).expect("create day directory");
            let file = day.join("rollout-2026-08-25T02-16-45-x.jsonl");
            let line = serde_json::json!({
                "timestamp": timestamp,
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "last_token_usage": {
                            "input_tokens": 1,
                            "output_tokens": 1,
                            "total_tokens": 2
                        }
                    },
                    "rate_limits": {
                        "primary": { "used_percent": percent, "window_minutes": 300 }
                    }
                }
            })
            .to_string();
            // A trailing newline matters: `scan_from` leaves a partial final
            // record for the next scan, so an unterminated line is skipped.
            std::fs::write(&file, format!("{line}\n")).expect("write file");
            files.push(file);
        }

        let mut points = std::collections::BTreeMap::new();
        for file in &files {
            let (scan, _) = scan_from(file, 0).expect("scan succeeds");
            for (key, value) in &scan.quota_history {
                points.insert(key.clone(), *value);
            }
        }
        assert_eq!(points.len(), 2);
        assert!((points[&("2026-08-24".to_owned(), QuotaKind::Primary)] - 10.0).abs() < 1e-9);
        assert!((points[&("2026-08-25".to_owned(), QuotaKind::Primary)] - 20.0).abs() < 1e-9);
    }

    #[test]
    fn derives_dates_from_timestamps() {
        assert_eq!(
            session_date_from_timestamp("2026-08-25T02:17:02.910Z").as_deref(),
            Some("2026-08-25")
        );
        assert_eq!(
            session_date_from_timestamp("2026-08-25"),
            Some("2026-08-25".to_owned())
        );
        assert_eq!(session_date_from_timestamp("not-a-date"), None);
        assert_eq!(session_date_from_timestamp(""), None);
    }

    /// A replayed turn appears twice with identical counters; only the first
    /// occurrence counts.
    #[test]
    fn counts_a_replayed_turn_once() {
        let directory = tempfile::tempdir().expect("temp dir");
        let day = directory.path().join("2026/08/25");
        std::fs::create_dir_all(&day).expect("create day directory");
        let file = day.join("rollout-2026-08-25T02-16-45-x.jsonl");

        let meta = serde_json::json!({
            "timestamp": "2026-08-25T02:00:00Z",
            "type": "session_meta",
            "payload": { "session_id": "s1", "id": "t1", "cwd": "/p" }
        })
        .to_string();
        let turn = serde_json::json!({
            "timestamp": "2026-08-25T02:01:00Z",
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "last_token_usage": {
                        "input_tokens": 100, "cached_input_tokens": 40,
                        "output_tokens": 10, "total_tokens": 110
                    },
                    "total_token_usage": {
                        "input_tokens": 500, "cached_input_tokens": 200,
                        "output_tokens": 50, "total_tokens": 550
                    }
                }
            }
        })
        .to_string();
        // The same turn written twice, as a resume would.
        std::fs::write(&file, format!("{meta}\n{turn}\n{turn}\n")).expect("write file");

        let (scan, _) = scan_from(&file, 0).expect("scan succeeds");
        assert_eq!(scan.samples, 1, "the copy must not be counted");
        assert_eq!(scan.usage.input_tokens, 100);
    }

    /// Records without a cumulative snapshot cannot be identified as replays, so
    /// identical-looking turns must both count rather than risk dropping usage.
    #[test]
    fn counts_turns_that_cannot_be_identified_as_replays() {
        let directory = tempfile::tempdir().expect("temp dir");
        let day = directory.path().join("2026/08/25");
        std::fs::create_dir_all(&day).expect("create day directory");
        let file = day.join("rollout-2026-08-25T02-16-45-x.jsonl");

        let turn = serde_json::json!({
            "timestamp": "2026-08-25T02:01:00Z",
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "last_token_usage": {
                        "input_tokens": 100, "output_tokens": 10, "total_tokens": 110
                    }
                }
            }
        })
        .to_string();
        std::fs::write(&file, format!("{turn}\n{turn}\n")).expect("write file");

        let (scan, _) = scan_from(&file, 0).expect("scan succeeds");
        assert_eq!(
            scan.samples, 2,
            "no cumulative snapshot means no replay match"
        );
        assert_eq!(scan.usage.input_tokens, 200);
    }

    /// Two genuinely different turns in one session must both count even though
    /// they share a session id.
    #[test]
    fn keeps_distinct_turns_in_one_session() {
        let directory = tempfile::tempdir().expect("temp dir");
        let day = directory.path().join("2026/08/25");
        std::fs::create_dir_all(&day).expect("create day directory");
        let file = day.join("rollout-2026-08-25T02-16-45-x.jsonl");

        let meta = serde_json::json!({
            "timestamp": "2026-08-25T02:00:00Z",
            "type": "session_meta",
            "payload": { "session_id": "s1", "id": "t1", "cwd": "/p" }
        })
        .to_string();
        let turn = |input: u64, cumulative_input: u64| {
            serde_json::json!({
                "timestamp": "2026-08-25T02:01:00Z",
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "last_token_usage": {
                            "input_tokens": input, "output_tokens": 1, "total_tokens": input + 1
                        },
                        "total_token_usage": {
                            "input_tokens": cumulative_input, "output_tokens": 2,
                            "total_tokens": cumulative_input + 2
                        }
                    }
                }
            })
            .to_string()
        };
        std::fs::write(
            &file,
            format!("{meta}\n{}\n{}\n", turn(10, 10), turn(20, 30)),
        )
        .expect("write file");

        let (scan, _) = scan_from(&file, 0).expect("scan succeeds");
        assert_eq!(scan.samples, 2);
        assert_eq!(scan.usage.input_tokens, 30);
    }

    #[test]
    fn derives_the_project_from_the_session_directory() {
        let scan = SessionScan {
            meta: SessionMeta {
                cwd: Some("/Users/example/codexhub".to_owned()),
                ..SessionMeta::default()
            },
            ..SessionScan::default()
        };
        assert_eq!(scan.project(), Some("codexhub"));

        let trailing = SessionScan {
            meta: SessionMeta {
                cwd: Some("/Users/example/codexhub/".to_owned()),
                ..SessionMeta::default()
            },
            ..SessionScan::default()
        };
        assert_eq!(trailing.project(), Some("codexhub"));
        assert_eq!(SessionScan::default().project(), None);
    }

    #[test]
    fn aggregates_sessions_into_days_newest_first() {
        let scan = |total: u64| SessionScan {
            usage: TokenUsage {
                total_tokens: total,
                input_tokens: total,
                ..TokenUsage::default()
            },
            samples: 1,
            ..SessionScan::default()
        };
        let scans = vec![
            ("2026-08-24".to_owned(), scan(10)),
            ("2026-08-25".to_owned(), scan(20)),
            ("2026-08-25".to_owned(), scan(30)),
        ];

        let days = aggregate_by_day(&scans);
        assert_eq!(days.len(), 2);
        assert_eq!(days[0].date, "2026-08-25");
        assert_eq!(days[0].usage.total_tokens, 50);
        assert_eq!(days[0].sessions, 2);
        assert_eq!(days[1].usage.total_tokens, 10);
        assert_eq!(days[1].sessions, 1);
    }

    /// Reports how many local sessions can be attributed to a model and a
    /// project, which is what the daemon needs to rebuild the client history
    /// database. Sessions without attribution are counted, not guessed at.
    ///
    /// ```text
    /// cargo test --lib attribution_coverage -- --ignored --nocapture
    /// ```
    /// Every local rollout file, across the live tree and the flat archive.
    ///
    /// The manual cross-checks below run against the real Codex installation, so
    /// they have to see the same set the daemon scans. Walking only the live tree
    /// would under-report by exactly the sessions the archive holds.
    fn local_rollout_files() -> Vec<PathBuf> {
        let mut files = Vec::new();
        for root in sessions_roots() {
            collect_rollout_files(&root, &mut files);
        }
        files.sort();
        files
    }

    fn collect_rollout_files(directory: &Path, files: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                collect_rollout_files(&path, files);
            } else if is_rollout_file(&path) {
                files.push(path);
            }
        }
    }

    #[test]
    #[ignore]
    fn attribution_coverage() {
        let files = local_rollout_files();
        if files.is_empty() {
            eprintln!("no local Codex sessions directory found");
            return;
        }

        let mut total = 0usize;
        let mut with_model = 0usize;
        let mut with_project = 0usize;
        let mut with_settings = 0usize;
        let mut with_session_provider = 0usize;
        let mut with_path_date = 0usize;
        // Tokens, not sessions: the breakdown's completeness is what decides
        // whether a client history rebuild can drop usage.
        let mut tokens = 0u64;
        let mut breakdown_tokens = 0u64;
        for path in &files {
            let Ok((scan, _)) = scan_from(path, 0) else {
                continue;
            };
            if scan.samples == 0 {
                continue;
            }
            total += 1;
            if scan.effective_model().is_some() {
                with_model += 1;
            }
            if scan.project().is_some() {
                with_project += 1;
            }
            if scan.settings.is_some() {
                with_settings += 1;
            }
            if scan.meta.provider.is_some() {
                with_session_provider += 1;
            }
            if rollout_day_from_path(path).is_some() {
                with_path_date += 1;
            }
            tokens += scan.usage.total_tokens;
            breakdown_tokens += scan
                .by_model_provider
                .values()
                .map(|usage| usage.total_tokens)
                .sum::<u64>();
        }

        eprintln!("rollout files: {total}");
        eprintln!("  with a model:            {with_model}");
        eprintln!("  with a project:          {with_project}");
        eprintln!("  with settings:           {with_settings}");
        eprintln!("  with a session provider: {with_session_provider}");
        eprintln!("  with a date in the path: {with_path_date}");
        let coverage = if tokens == 0 {
            100.0
        } else {
            breakdown_tokens as f64 / tokens as f64 * 100.0
        };
        eprintln!("tokens total:     {tokens}");
        eprintln!("tokens breakdown: {breakdown_tokens} ({coverage:.2}%)");
        assert_eq!(
            breakdown_tokens, tokens,
            "every counted token has to reach the breakdown, or history rows go missing"
        );
        assert_eq!(
            with_path_date, total,
            "every rollout file has to yield a day, or its usage lands in the wrong bucket"
        );
    }

    /// Manual cross-check against the local Codex installation, which is the
    /// only place real rollout files exist:
    ///
    /// ```text
    /// cargo test --lib scan_local_sessions -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore]
    fn scan_local_sessions() {
        let files = local_rollout_files();
        if files.is_empty() {
            eprintln!("no local Codex sessions directory found");
            return;
        }

        let mut scans: Vec<(String, SessionScan)> = Vec::new();
        for path in &files {
            let Ok((scan, _)) = scan_from(path, 0) else {
                continue;
            };
            if scan.samples == 0 {
                continue;
            }
            // Prefer the path, which is what the service layer buckets by, and
            // fall back to the records' own timestamp.
            let date = rollout_day_from_path(path)
                .or_else(|| {
                    scan.meta
                        .started_at
                        .as_deref()
                        .and_then(session_date_from_timestamp)
                })
                .unwrap_or_else(|| "unknown".to_owned());
            scans.push((date, scan));
        }

        let days = aggregate_by_day(&scans);
        eprintln!(
            "scanned {} rollout sessions across {} days",
            scans.len(),
            days.len()
        );
        for day in days.iter().take(5) {
            eprintln!(
                "  {} sessions={} total_tokens={} input={} cached={} output={}",
                day.date,
                day.sessions,
                day.usage.total_tokens,
                day.usage.input_tokens,
                day.usage.cached_input_tokens,
                day.usage.output_tokens
            );
        }
    }

    /// Compares the two accumulation strategies the clients could use, across
    /// every rollout file on this machine:
    ///
    /// ```text
    /// cargo test --lib compare_accumulation -- --ignored --nocapture
    /// ```
    ///
    /// The daemon reads the last cumulative `total_token_usage` while the
    /// existing macOS engine sums the per-turn `last_token_usage`. If the two
    /// disagree on real data, the daemon has to adopt `last_token_usage` before
    /// the clients can switch over.
    #[test]
    #[ignore]
    fn compare_accumulation_strategies() {
        let files = local_rollout_files();
        if files.is_empty() {
            eprintln!("no local Codex sessions directory found");
            return;
        }

        let mut compared = 0usize;
        let mut differing = Vec::new();
        for path in &files {
            let Ok(text) = std::fs::read_to_string(path) else {
                continue;
            };
            let mut cumulative_only = TokenUsage::default();
            let mut summed = TokenUsage::default();
            let mut samples = 0usize;
            for line in text.lines() {
                let Some(RolloutEvent::TokenCount(count)) = parse_record(line) else {
                    continue;
                };
                samples += 1;
                // Wrong strategy: only the last session-cumulative
                // counter is read.
                if let Some(cumulative) = count.cumulative {
                    cumulative_only = cumulative;
                }
                // Right strategy: per-turn counters are summed.
                accumulate(&mut summed, count.usage);
            }
            if samples == 0 {
                continue;
            }
            compared += 1;
            if cumulative_only.total_tokens != summed.total_tokens {
                differing.push((
                    path.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    cumulative_only.total_tokens,
                    summed.total_tokens,
                ));
            }
        }

        eprintln!("compared {compared} sessions across both strategies");
        eprintln!("sessions where the totals differ: {}", differing.len());
        for (name, cumulative, summed) in differing.iter().take(5) {
            eprintln!("  {name}: cumulative={cumulative} summed={summed}");
        }
    }
}
