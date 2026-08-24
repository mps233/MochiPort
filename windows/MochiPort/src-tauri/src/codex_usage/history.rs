use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use chrono::{Days, NaiveDate};
use serde::{Deserialize, Serialize};

const HISTORY_SCHEMA_VERSION: u32 = 1;
const HISTORY_FILE_NAME: &str = "codex-usage-history-v1.json";

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PersistedUsageHistory {
    schema_version: u32,
    #[serde(default)]
    daily_tokens: BTreeMap<String, u64>,
    #[serde(default)]
    observed_daily_tokens: BTreeMap<String, u64>,
    /// Per-source high-water observations keep a temporarily missing log
    /// from looking like a reset. The source path is stable across refreshes
    /// and a genuinely new path is still allowed to contribute its totals.
    #[serde(default)]
    source_daily_tokens: BTreeMap<String, BTreeMap<String, u64>>,
    #[serde(default)]
    source_observations_initialized: bool,
    #[serde(default)]
    quota_percent_snapshots: BTreeMap<String, BTreeMap<String, f64>>,
}

/// Long-lived aggregates that cannot be reconstructed from the bounded
/// in-memory event tail after old Codex session logs are rotated away.
pub(super) struct UsageHistoryStore {
    path: Option<PathBuf>,
    data: PersistedUsageHistory,
    needs_full_backfill: bool,
    dirty: bool,
}

impl Default for UsageHistoryStore {
    fn default() -> Self {
        #[cfg(test)]
        {
            Self::in_memory()
        }
        #[cfg(not(test))]
        {
            Self::load(default_history_path())
        }
    }
}

impl UsageHistoryStore {
    #[cfg(test)]
    pub(super) fn at(path: PathBuf) -> Self {
        Self::load(Some(path))
    }

    #[cfg(test)]
    fn in_memory() -> Self {
        Self {
            path: None,
            data: PersistedUsageHistory {
                schema_version: HISTORY_SCHEMA_VERSION,
                ..PersistedUsageHistory::default()
            },
            needs_full_backfill: true,
            dirty: false,
        }
    }

    fn load(path: Option<PathBuf>) -> Self {
        let loaded = path.as_deref().and_then(load_history_file);
        match loaded {
            Some(mut data) if data.schema_version == HISTORY_SCHEMA_VERSION => {
                if data.observed_daily_tokens.is_empty() && !data.daily_tokens.is_empty() {
                    data.observed_daily_tokens = data.daily_tokens.clone();
                }
                Self {
                    path,
                    data,
                    needs_full_backfill: false,
                    dirty: false,
                }
            }
            _ => Self {
                path,
                data: PersistedUsageHistory {
                    schema_version: HISTORY_SCHEMA_VERSION,
                    ..PersistedUsageHistory::default()
                },
                needs_full_backfill: true,
                dirty: false,
            },
        }
    }

    pub(super) fn needs_full_backfill(&self) -> bool {
        self.needs_full_backfill
    }

    /// Replace complete UTC-day aggregates. The first pass covers every
    /// discoverable JSONL file; later passes replace only the recent range,
    /// preserving older rows after their source files age out or rotate away.
    pub(super) fn replace_daily_tokens(
        &mut self,
        observed: &BTreeMap<NaiveDate, u64>,
        observed_sources: &BTreeMap<String, BTreeMap<NaiveDate, u64>>,
        replace_from: NaiveDate,
        full_backfill: bool,
        backfill_complete: bool,
    ) {
        if full_backfill {
            let next = observed
                .iter()
                .map(|(day, tokens)| (day_key(*day), *tokens))
                .collect::<BTreeMap<_, _>>();
            let next_sources = observed_sources
                .iter()
                .map(|(source, days)| {
                    (
                        source.clone(),
                        days.iter()
                            .map(|(day, tokens)| (day_key(*day), *tokens))
                            .collect::<BTreeMap<_, _>>(),
                    )
                })
                .collect::<BTreeMap<_, _>>();
            if next != self.data.daily_tokens
                || next != self.data.observed_daily_tokens
                || next_sources != self.data.source_daily_tokens
            {
                self.data.daily_tokens = next.clone();
                self.data.observed_daily_tokens = next;
                self.data.source_daily_tokens = next_sources;
                self.dirty = true;
            }
            if backfill_complete && !self.data.source_observations_initialized {
                self.data.source_observations_initialized = true;
                self.dirty = true;
            }
        } else {
            if self.data.source_observations_initialized {
                self.apply_source_deltas(observed, observed_sources, replace_from);
            } else {
                self.migrate_aggregate_observations(observed, observed_sources, replace_from);
            }
        }
        if !full_backfill || backfill_complete {
            self.needs_full_backfill = false;
        }
    }

    /// Apply only positive deltas for sources that are present in this scan.
    /// Missing sources retain their last high-water value, so a transiently
    /// unavailable file cannot cause its old tokens to be counted twice when
    /// it returns on a later refresh.
    fn apply_source_deltas(
        &mut self,
        observed: &BTreeMap<NaiveDate, u64>,
        observed_sources: &BTreeMap<String, BTreeMap<NaiveDate, u64>>,
        replace_from: NaiveDate,
    ) {
        for (source, source_days) in observed_sources {
            let baseline = self
                .data
                .source_daily_tokens
                .entry(source.clone())
                .or_default();
            for (day, current) in source_days {
                if *day < replace_from {
                    continue;
                }
                let key = day_key(*day);
                let previous = baseline.get(&key).copied();
                match previous {
                    Some(previous) if *current > previous => {
                        let total = self.data.daily_tokens.entry(key.clone()).or_default();
                        *total = total.saturating_add(*current - previous);
                        baseline.insert(key, *current);
                        self.dirty = true;
                    }
                    Some(_) => {
                        // A decrease is a rewrite/partial view of the same
                        // source, not evidence that previously seen tokens
                        // should be removed or re-counted.
                    }
                    None => {
                        let total = self.data.daily_tokens.entry(key.clone()).or_default();
                        *total = total.saturating_add(*current);
                        baseline.insert(key, *current);
                        self.dirty = true;
                    }
                }
            }
        }

        // Keep the legacy aggregate as a high-water diagnostic and as a
        // migration fallback. It must never move backwards when a source is
        // absent or temporarily shorter than its previous observation.
        for (day, current) in observed {
            if *day < replace_from {
                continue;
            }
            let key = day_key(*day);
            let previous = self
                .data
                .observed_daily_tokens
                .get(&key)
                .copied()
                .unwrap_or_default();
            if *current > previous {
                self.data.observed_daily_tokens.insert(key, *current);
                self.dirty = true;
            }
        }
    }

    /// Histories written before per-source observations existed only have an
    /// aggregate high-water mark. Consume that delta once, then seed the
    /// source map so subsequent refreshes are source-aware.
    fn migrate_aggregate_observations(
        &mut self,
        observed: &BTreeMap<NaiveDate, u64>,
        observed_sources: &BTreeMap<String, BTreeMap<NaiveDate, u64>>,
        replace_from: NaiveDate,
    ) {
        let cutoff = day_key(replace_from);
        let mut days = self
            .data
            .observed_daily_tokens
            .keys()
            .filter(|day| *day >= &cutoff)
            .cloned()
            .collect::<HashSet<_>>();
        days.extend(
            observed
                .keys()
                .filter(|day| **day >= replace_from)
                .map(|day| day_key(*day)),
        );
        for day in days {
            let current = parse_day(&day)
                .and_then(|parsed| observed.get(&parsed).copied())
                .unwrap_or_default();
            let previous = self
                .data
                .observed_daily_tokens
                .get(&day)
                .copied()
                .unwrap_or_default();
            if current > previous {
                let total = self.data.daily_tokens.entry(day.clone()).or_default();
                *total = total.saturating_add(current - previous);
                self.data.observed_daily_tokens.insert(day, current);
                self.dirty = true;
            }
        }

        for (source, source_days) in observed_sources {
            let baseline = self
                .data
                .source_daily_tokens
                .entry(source.clone())
                .or_default();
            for (day, current) in source_days {
                let key = day_key(*day);
                let previous = baseline.get(&key).copied().unwrap_or_default();
                if *current > previous {
                    baseline.insert(key, *current);
                    self.dirty = true;
                }
            }
        }
        // If the old aggregate is being migrated while no source is visible,
        // keep the migration path alive. A source can be temporarily absent
        // from the bounded scan and should not be treated as a brand-new
        // source with a full duplicate total when it returns.
        if observed_sources.values().any(|days| !days.is_empty()) {
            self.data.source_observations_initialized = true;
        }
        self.dirty = true;
    }

    /// Same-day observations replace the prior percentage, mirroring the
    /// macOS percent_snapshots primary key of (day, service, kind).
    pub(super) fn record_quota_percent(&mut self, day: NaiveDate, kind: &str, percent: f64) {
        if !percent.is_finite() {
            return;
        }
        let percent = percent.clamp(0.0, 100.0);
        let snapshots = self
            .data
            .quota_percent_snapshots
            .entry(day_key(day))
            .or_default();
        if snapshots.get(kind).copied() != Some(percent) {
            snapshots.insert(kind.to_string(), percent);
            self.dirty = true;
        }
    }

    pub(super) fn streak_days(&self, ending_on: NaiveDate) -> u64 {
        let active = self
            .data
            .daily_tokens
            .iter()
            .filter_map(|(day, tokens)| (*tokens > 0).then(|| parse_day(day)).flatten())
            .collect::<HashSet<_>>();
        consecutive_active_days(&active, ending_on)
    }

    pub(super) fn previous_best_daily_tokens(&self, excluding: NaiveDate) -> Option<u64> {
        let excluded = day_key(excluding);
        self.data
            .daily_tokens
            .iter()
            .filter(|(day, _)| *day != &excluded)
            .map(|(_, tokens)| *tokens)
            .max()
    }

    /// Average positive adjacent daily deltas, normalized by the actual day
    /// gap. Resets (delta <= 0) and sub-half-day pairs are discarded.
    pub(super) fn weekly_daily_rate(
        &self,
        kind: &str,
        ending_on: NaiveDate,
        days: u64,
    ) -> Option<f64> {
        let first = ending_on
            .checked_sub_days(Days::new(days.saturating_sub(1)))
            .unwrap_or(ending_on);
        let snapshots = self
            .data
            .quota_percent_snapshots
            .iter()
            .filter_map(|(day, by_kind)| {
                let day = parse_day(day)?;
                (day >= first && day <= ending_on).then_some((day, *by_kind.get(kind)?))
            })
            .collect::<Vec<_>>();
        let mut positives = Vec::new();
        for pair in snapshots.windows(2) {
            let day_gap = pair[1].0.signed_duration_since(pair[0].0).num_days() as f64;
            if day_gap < 0.5 {
                continue;
            }
            let delta = pair[1].1 - pair[0].1;
            if delta > 0.0 {
                positives.push(delta / day_gap);
            }
        }
        (!positives.is_empty()).then(|| positives.iter().sum::<f64>() / positives.len() as f64)
    }

    pub(super) fn persist_if_needed(&mut self) -> Result<(), String> {
        if !self.dirty {
            return Ok(());
        }
        let Some(path) = self.path.as_deref() else {
            self.dirty = false;
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!("无法创建 Codex 用量历史目录 {}：{error}", parent.display())
            })?;
        }
        let payload = serde_json::to_vec(&self.data)
            .map_err(|error| format!("无法编码 Codex 用量历史：{error}"))?;
        atomic_write(path, &payload)?;
        self.dirty = false;
        Ok(())
    }
}

#[cfg(not(test))]
fn default_history_path() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("APPDATA"))
        .map(PathBuf::from)
        .map(|root| root.join("MochiPort").join(HISTORY_FILE_NAME))
}

fn load_history_file(path: &Path) -> Option<PersistedUsageHistory> {
    let primary = fs::read(path).ok().and_then(|bytes| decode_history(&bytes));
    primary.or_else(|| {
        fs::read(backup_path(path))
            .ok()
            .and_then(|bytes| decode_history(&bytes))
    })
}

fn decode_history(bytes: &[u8]) -> Option<PersistedUsageHistory> {
    serde_json::from_slice::<PersistedUsageHistory>(bytes)
        .ok()
        .filter(|history| history.schema_version == HISTORY_SCHEMA_VERSION)
}

fn atomic_write(path: &Path, payload: &[u8]) -> Result<(), String> {
    let temporary = temporary_path(path);
    let backup = backup_path(path);
    fs::write(&temporary, payload).map_err(|error| {
        format!(
            "无法写入 Codex 用量历史临时文件 {}：{error}",
            temporary.display()
        )
    })?;

    let had_primary = path.is_file();
    let primary_was_valid = had_primary && primary_history_is_valid(path);
    if had_primary {
        if primary_was_valid {
            if backup.exists() {
                fs::remove_file(&backup).map_err(|error| {
                    format!("无法清理 Codex 用量历史备份 {}：{error}", backup.display())
                })?;
            }
            fs::rename(path, &backup)
                .map_err(|error| format!("无法轮换 Codex 用量历史 {}：{error}", path.display()))?;
        } else {
            // A valid backup may be the only surviving long-lived history.
            // Remove the corrupt primary without replacing that backup.
            fs::remove_file(path).map_err(|error| {
                format!("无法清理损坏的 Codex 用量历史 {}：{error}", path.display())
            })?;
        }
    }

    if let Err(error) = fs::rename(&temporary, path) {
        if primary_was_valid {
            let _ = fs::rename(&backup, path);
        }
        return Err(format!(
            "无法提交 Codex 用量历史 {}：{error}",
            path.display()
        ));
    }
    // Keep one known-good generation. If the primary is later truncated or
    // corrupted, startup can recover from this backup without discarding the
    // long-lived streak/record/quota history.
    Ok(())
}

fn primary_history_is_valid(path: &Path) -> bool {
    fs::read(path)
        .ok()
        .and_then(|bytes| decode_history(&bytes))
        .is_some()
}

fn temporary_path(path: &Path) -> PathBuf {
    path.with_extension("json.tmp")
}

fn backup_path(path: &Path) -> PathBuf {
    path.with_extension("json.bak")
}

fn day_key(day: NaiveDate) -> String {
    day.format("%Y-%m-%d").to_string()
}

fn parse_day(day: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()
}

fn consecutive_active_days(active_days: &HashSet<NaiveDate>, ending_on: NaiveDate) -> u64 {
    let mut streak = 0_u64;
    let mut cursor = ending_on;
    while active_days.contains(&cursor) {
        streak = streak.saturating_add(1);
        let Some(previous) = cursor.checked_sub_days(Days::new(1)) else {
            break;
        };
        cursor = previous;
    }
    streak
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_round_trips_full_streak_record_and_weekly_percentages() {
        let root = std::env::temp_dir().join(format!(
            "mochiport-usage-history-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
        ));
        let path = root.join(HISTORY_FILE_NAME);
        let today = NaiveDate::from_ymd_opt(2026, 8, 24).expect("valid day");
        let yesterday = today.checked_sub_days(Days::new(1)).expect("yesterday");
        let two_days_ago = today.checked_sub_days(Days::new(2)).expect("two days ago");
        let historic_record = today
            .checked_sub_days(Days::new(500))
            .expect("historic day");

        let mut store = UsageHistoryStore::at(path.clone());
        assert!(store.needs_full_backfill());
        store.replace_daily_tokens(
            &BTreeMap::from([
                (historic_record, 900),
                (two_days_ago, 100),
                (yesterday, 200),
                (today, 300),
            ]),
            &BTreeMap::from([(
                "source-a".to_string(),
                BTreeMap::from([
                    (historic_record, 900),
                    (two_days_ago, 100),
                    (yesterday, 200),
                    (today, 300),
                ]),
            )]),
            historic_record,
            true,
            true,
        );
        store.record_quota_percent(two_days_ago, "weekly", 10.0);
        store.record_quota_percent(yesterday, "weekly", 20.0);
        store.record_quota_percent(today, "weekly", 35.0);
        store.persist_if_needed().expect("persist usage history");

        let loaded = UsageHistoryStore::at(path);
        assert!(!loaded.needs_full_backfill());
        assert_eq!(loaded.streak_days(today), 3);
        assert_eq!(loaded.previous_best_daily_tokens(today), Some(900));
        assert_eq!(loaded.weekly_daily_rate("weekly", today, 8), Some(12.5));

        fs::remove_dir_all(root).expect("remove temporary usage history");
    }

    #[test]
    fn recent_replacement_preserves_old_and_rotated_history() {
        let today = NaiveDate::from_ymd_opt(2026, 8, 24).expect("valid day");
        let old = today.checked_sub_days(Days::new(400)).expect("old day");
        let yesterday = today.checked_sub_days(Days::new(1)).expect("yesterday");
        let mut store = UsageHistoryStore::in_memory();
        store.replace_daily_tokens(
            &BTreeMap::from([(old, 1_000), (yesterday, 200), (today, 300)]),
            &BTreeMap::from([(
                "source-a".to_string(),
                BTreeMap::from([(old, 1_000), (yesterday, 200), (today, 300)]),
            )]),
            old,
            true,
            true,
        );

        store.replace_daily_tokens(
            &BTreeMap::from([(today, 350)]),
            &BTreeMap::from([("source-a".to_string(), BTreeMap::from([(today, 350)]))]),
            yesterday,
            false,
            true,
        );
        assert_eq!(store.previous_best_daily_tokens(today), Some(1_000));
        assert_eq!(store.streak_days(today), 2);
        assert_eq!(store.data.daily_tokens.get(&day_key(today)), Some(&350));
        assert_eq!(store.data.daily_tokens.get(&day_key(yesterday)), Some(&200));

        // The old source disappears temporarily. Its baseline is retained,
        // so returning with the same totals does not duplicate the day.
        store.replace_daily_tokens(&BTreeMap::new(), &BTreeMap::new(), yesterday, false, true);
        store.replace_daily_tokens(
            &BTreeMap::from([(today, 350)]),
            &BTreeMap::from([("source-a".to_string(), BTreeMap::from([(today, 350)]))]),
            yesterday,
            false,
            true,
        );
        assert_eq!(store.data.daily_tokens.get(&day_key(today)), Some(&350));

        // A genuinely new source/path contributes its current high-water
        // total once, without disturbing the rotated older record.
        store.replace_daily_tokens(
            &BTreeMap::from([(yesterday, 50), (today, 375)]),
            &BTreeMap::from([(
                "source-b".to_string(),
                BTreeMap::from([(yesterday, 50), (today, 25)]),
            )]),
            yesterday,
            false,
            true,
        );
        assert_eq!(store.data.daily_tokens.get(&day_key(yesterday)), Some(&250));
        assert_eq!(store.data.daily_tokens.get(&day_key(today)), Some(&375));
        assert_eq!(store.streak_days(today), 2);
    }

    #[test]
    fn corrupted_primary_falls_back_to_the_previous_atomic_generation() {
        let root = std::env::temp_dir().join(format!(
            "mochiport-usage-history-fallback-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
        ));
        let path = root.join(HISTORY_FILE_NAME);
        let today = NaiveDate::from_ymd_opt(2026, 8, 24).expect("valid day");

        let mut store = UsageHistoryStore::at(path.clone());
        store.replace_daily_tokens(
            &BTreeMap::from([(today, 100)]),
            &BTreeMap::new(),
            today,
            true,
            true,
        );
        store.persist_if_needed().expect("persist first generation");
        store.replace_daily_tokens(
            &BTreeMap::from([(today, 200)]),
            &BTreeMap::new(),
            today,
            true,
            true,
        );
        store
            .persist_if_needed()
            .expect("persist second generation");
        fs::write(&path, b"{not valid json").expect("corrupt primary generation");

        let mut recovered = UsageHistoryStore::at(path.clone());
        assert_eq!(
            recovered.previous_best_daily_tokens(
                today.checked_add_days(Days::new(1)).expect("next day")
            ),
            Some(100)
        );
        recovered.record_quota_percent(today, "weekly", 42.0);
        recovered
            .persist_if_needed()
            .expect("repair primary without overwriting valid backup");
        let repaired = UsageHistoryStore::at(path);
        assert_eq!(
            repaired.previous_best_daily_tokens(
                today.checked_add_days(Days::new(1)).expect("next day")
            ),
            Some(100)
        );

        fs::remove_dir_all(root).expect("remove temporary usage history");
    }
}
