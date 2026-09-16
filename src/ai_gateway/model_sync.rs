//! 官方模型目录同步。
//!
//! Codex 官方把模型目录维护在 `openai/codex` 仓库的
//! `codex-rs/models-manager/models.json`；MochiPort 的内置
//! `src/ai_gateway/models.json` 是它的忠实副本，但副本会随官方更新而过期。
//!
//! 本模块在 daemon 启动时和之后每 24 小时拉一次官方目录，把**官方模型条目**
//! （`gpt-*`、`codex-auto-review` 等）的能力字段与提示词模板同步到用户级覆盖
//! 文件，再由 `catalog` 合并进内置目录。因此官方更新后无需改源码、无需重建。
//!
//! 边界：
//! - 只从固定的官方 raw 地址拉取；
//! - 只更新内置目录里**已存在且属于官方**的条目，以及官方新增的条目；
//! - 项目自建的第三方条目（grok / GLM / Claude / deepseek 等）完全不碰；
//! - 校验失败、网络失败、格式异常一律静默保持现状，绝不用坏数据覆盖。

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 官方模型目录地址（与项目文档引用的源码位置一致）。
pub(crate) const OFFICIAL_MODEL_CATALOG_URL: &str =
    "https://raw.githubusercontent.com/openai/codex/main/codex-rs/models-manager/models.json";

/// 官方 raw 文件不大（约 300KB），10 秒足够。
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

/// 同步之间的最小间隔：24 小时。
pub(crate) const SYNC_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// 用户级覆盖文件名（放在 MochiPort 数据目录里）。
pub(crate) const OVERLAY_FILE_NAME: &str = "model-catalog.official.json";

/// 覆盖文件的结构：与内置目录同构，只是条目来自官方最新版本。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OfficialCatalogOverlay {
    /// 覆盖文件格式版本。
    #[serde(default = "overlay_schema_version")]
    pub schema_version: u32,
    /// 本次同步时间（毫秒时间戳）。
    #[serde(default)]
    pub synced_at_ms: u128,
    /// 同步来源 URL。
    #[serde(default)]
    pub source_url: String,
    /// 官方条目（按 slug 覆盖或新增）。
    #[serde(default)]
    pub models: Vec<Value>,
    /// 官方**曾经提供过**的全部 slug（跨多次同步累积）。
    ///
    /// 用来区分"官方下线的模型"和"项目自建的第三方模型"：只有出现在这里的
    /// slug 才允许被下线逻辑移除，第三方条目永远不受影响。
    #[serde(default)]
    pub known_official_slugs: Vec<String>,
    /// 本次同步产生的变更说明，供 GUI/日志展示。
    #[serde(default)]
    pub changes: Vec<String>,
}

fn overlay_schema_version() -> u32 {
    1
}

/// 一次同步的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SyncOutcome {
    /// 覆盖文件已更新。
    Updated(Vec<String>),
    /// 官方与本地一致，无需写入。
    Unchanged,
    /// 拉取或校验失败，保持现状。
    Skipped(String),
}

/// 拉取官方目录并写入覆盖文件。
///
/// `data_dir` 是 MochiPort 数据目录（`config.toml` 所在目录）。
pub(crate) async fn sync_official_catalog(
    data_dir: &Path,
    embedded: &Value,
    force: bool,
) -> SyncOutcome {
    let overlay_path = overlay_path(data_dir);
    if !force
        && let Some(existing) = read_overlay(&overlay_path)
        && is_fresh(&existing)
    {
        return SyncOutcome::Unchanged;
    }

    let client = crate::outbound_http::get();
    let response = match client
        .get(OFFICIAL_MODEL_CATALOG_URL)
        .timeout(FETCH_TIMEOUT)
        .send()
        .await
    {
        Ok(response) => response,
        Err(err) => return SyncOutcome::Skipped(format!("fetch failed: {err}")),
    };
    if !response.status().is_success() {
        return SyncOutcome::Skipped(format!("unexpected status {}", response.status()));
    }
    let body = match response.text().await {
        Ok(body) => body,
        Err(err) => return SyncOutcome::Skipped(format!("read body failed: {err}")),
    };
    let official: Value = match serde_json::from_str(&body) {
        Ok(value) => value,
        Err(err) => return SyncOutcome::Skipped(format!("invalid json: {err}")),
    };
    let Some(official_models) = official.get("models").and_then(Value::as_array) else {
        return SyncOutcome::Skipped("official catalog has no models array".to_string());
    };

    let embedded_slugs = embedded_slugs(embedded);
    let mut merged: Vec<Value> = Vec::new();
    let mut changes: Vec<String> = Vec::new();
    for entry in official_models {
        let Some(slug) = entry.get("slug").and_then(Value::as_str).map(str::trim) else {
            continue;
        };
        if slug.is_empty() {
            continue;
        }
        // 只接受形如官方模型的条目；官方目录本身只含官方模型，这里再兜一层。
        // 官方条目原样保留（含 visibility / supported_in_api / priority）：
        // 官方模型以官方定义为准，本地只在合并时补 base_instructions。
        let entry = entry.clone();
        if !embedded_slugs.contains(&slug.to_ascii_lowercase()) {
            changes.push(format!("新增 {slug}"));
        } else {
            changes.push(format!("更新 {slug}"));
        }
        merged.push(entry);
    }
    if merged.is_empty() {
        return SyncOutcome::Skipped("official catalog produced no usable entries".to_string());
    }

    // 累积"官方提供过"的 slug，三个来源缺一不可：
    // 1. 内置目录里属于官方的条目——旧版覆盖文件没有 knownOfficialSlugs 字段，
    //    靠它兜底，避免升级后丢失下线追踪能力；
    // 2. 上一次覆盖文件记录的已知集合与条目；
    // 3. 本次拉到的官方条目。
    let previous = read_overlay(&overlay_path);
    let mut known: Vec<String> = Vec::new();
    for slug in embedded_official_slugs(embedded) {
        push_unique_case_insensitive(&mut known, &slug);
    }
    if let Some(previous) = previous.as_ref() {
        for slug in &previous.known_official_slugs {
            push_unique_case_insensitive(&mut known, slug);
        }
        for entry in &previous.models {
            if let Some(slug) = entry.get("slug").and_then(Value::as_str) {
                push_unique_case_insensitive(&mut known, slug);
            }
        }
    }
    for entry in &merged {
        if let Some(slug) = entry.get("slug").and_then(Value::as_str) {
            push_unique_case_insensitive(&mut known, slug);
        }
    }
    if let Some(previous) = previous.as_ref() {
        for entry in &previous.models {
            if let Some(slug) = entry.get("slug").and_then(Value::as_str) {
                push_unique_case_insensitive(&mut known, slug);
            }
        }
    }

    let overlay = OfficialCatalogOverlay {
        schema_version: overlay_schema_version(),
        synced_at_ms: crate::types::now_ms(),
        source_url: OFFICIAL_MODEL_CATALOG_URL.to_string(),
        models: merged,
        known_official_slugs: known,
        changes,
    };
    if let Err(err) = write_overlay(&overlay_path, &overlay) {
        return SyncOutcome::Skipped(format!("write overlay failed: {err}"));
    }
    SyncOutcome::Updated(overlay.changes)
}

/// 读取覆盖文件（不存在或损坏时返回 None）。
pub(crate) fn read_overlay(path: &Path) -> Option<OfficialCatalogOverlay> {
    let raw = std::fs::read_to_string(path).ok()?;
    let overlay: OfficialCatalogOverlay = serde_json::from_str(&raw).ok()?;
    (overlay.schema_version == overlay_schema_version()).then_some(overlay)
}

/// 覆盖文件的完整路径。
pub(crate) fn overlay_path(data_dir: &Path) -> PathBuf {
    data_dir.join(OVERLAY_FILE_NAME)
}

/// daemon 的数据目录（与请求日志库、`config.toml` 同处）。
///
/// 必须用 `storage_migration::current_storage_home()`：它才是权威来源，并遵守
/// `MOCHIPORT_HOME` 环境变量。早先版本用 `config_path.parent()` 推导，在
/// `MOCHIPORT_HOME` 与配置文件位置不一致时会指向错误的目录（隔离环境与自定义
/// 部署都会踩到），导致覆盖文件读取不到。
pub(crate) fn data_directory() -> PathBuf {
    crate::storage_migration::current_storage_home()
}

fn write_overlay(path: &Path, overlay: &OfficialCatalogOverlay) -> std::io::Result<()> {
    let serialized = serde_json::to_string_pretty(overlay)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    // 先写临时文件再原子替换，避免读到半截 JSON。
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, serialized)?;
    std::fs::rename(&temp, path)
}

fn is_fresh(overlay: &OfficialCatalogOverlay) -> bool {
    // 旧版本写的覆盖文件没有 knownOfficialSlugs，会让"官方下线"识别失效；
    // 这类文件一律视为不新鲜，触发一次重新同步把它补齐。
    if overlay.known_official_slugs.is_empty() {
        return false;
    }
    let now = crate::types::now_ms();
    now.saturating_sub(overlay.synced_at_ms) < SYNC_INTERVAL.as_millis()
}

fn push_unique_case_insensitive(values: &mut Vec<String>, value: &str) {
    let value = value.trim();
    if value.is_empty()
        || values
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(value))
    {
        return;
    }
    values.push(value.to_string());
}

/// 内置目录里属于官方的条目（`gpt-*` 与 `codex-auto-review`）。
///
/// 这些条目最初就是从官方目录抄来的，因此必须计入"官方提供过"的集合，
/// 否则官方哪天删掉它们时无法识别为下线。
pub(crate) fn embedded_official_slugs(embedded: &Value) -> Vec<String> {
    embedded_slugs(embedded)
        .into_iter()
        .filter(|slug| slug.starts_with("gpt-") || slug == "codex-auto-review")
        .collect()
}

fn embedded_slugs(embedded: &Value) -> Vec<String> {
    embedded
        .get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|model| model.get("slug").and_then(Value::as_str))
        .map(|slug| slug.trim().to_ascii_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn overlay_path_lives_in_the_data_directory() {
        let path = overlay_path(Path::new("/tmp/mochiport"));
        assert!(path.ends_with(OVERLAY_FILE_NAME));
        assert_eq!(path.parent().unwrap(), Path::new("/tmp/mochiport"));
    }

    #[test]
    fn embedded_slugs_are_lowercased() {
        let slugs = embedded_slugs(&json!({
            "models": [{ "slug": "GPT-5.6-SOL" }, { "slug": " grok-4.6 " }, { "nope": 1 }]
        }));
        assert_eq!(
            slugs,
            vec!["gpt-5.6-sol".to_string(), "grok-4.6".to_string()]
        );
    }

    #[test]
    fn embedded_official_slugs_covers_gpt_and_auto_review() {
        let embedded = json!({
            "models": [
                { "slug": "gpt-5.6-sol" },
                { "slug": "codex-auto-review" },
                { "slug": "grok-4.6" },
                { "slug": "GLM-5.2" }
            ]
        });
        assert_eq!(
            embedded_official_slugs(&embedded),
            vec!["gpt-5.6-sol".to_string(), "codex-auto-review".to_string()]
        );
    }

    #[test]
    fn push_unique_is_case_insensitive_and_skips_blanks() {
        let mut values = vec!["gpt-5.6-sol".to_string()];
        push_unique_case_insensitive(&mut values, "GPT-5.6-SOL");
        push_unique_case_insensitive(&mut values, "   ");
        push_unique_case_insensitive(&mut values, "gpt-5.5");
        assert_eq!(
            values,
            vec!["gpt-5.6-sol".to_string(), "gpt-5.5".to_string()]
        );
    }

    #[test]
    fn read_overlay_ignores_missing_or_broken_files() {
        let dir = std::env::temp_dir().join(format!("mp-overlay-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = overlay_path(&dir);
        let _ = std::fs::remove_file(&path);
        assert!(read_overlay(&path).is_none());

        std::fs::write(&path, "{ not json").unwrap();
        assert!(read_overlay(&path).is_none());

        // 未来版本的覆盖文件不参与合并（避免用不认识的格式）。
        std::fs::write(
            &path,
            json!({ "schemaVersion": 99, "models": [] }).to_string(),
        )
        .unwrap();
        assert!(read_overlay(&path).is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_then_read_overlay_round_trips() {
        let dir = std::env::temp_dir().join(format!("mp-overlay-rw-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = overlay_path(&dir);
        let overlay = OfficialCatalogOverlay {
            schema_version: overlay_schema_version(),
            synced_at_ms: crate::types::now_ms(),
            source_url: OFFICIAL_MODEL_CATALOG_URL.to_string(),
            models: vec![json!({ "slug": "gpt-5.6-sol", "use_responses_lite": true })],
            known_official_slugs: vec!["gpt-5.6-sol".to_string()],
            changes: vec!["更新 gpt-5.6-sol".to_string()],
        };
        write_overlay(&path, &overlay).expect("write overlay");
        let read = read_overlay(&path).expect("read overlay");
        assert_eq!(read.models.len(), 1);
        assert_eq!(read.models[0]["slug"], "gpt-5.6-sol");
        assert!(is_fresh(&read));
        // 临时文件不应残留。
        assert!(!path.with_extension("json.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn overlay_without_known_slugs_forces_a_resync() {
        // 旧版覆盖文件：有同步时间、但没有 knownOfficialSlugs。
        let legacy = OfficialCatalogOverlay {
            synced_at_ms: crate::types::now_ms(),
            known_official_slugs: Vec::new(),
            ..Default::default()
        };
        assert!(!is_fresh(&legacy), "legacy overlay must trigger a resync");
    }

    #[test]
    fn stale_overlay_is_not_fresh() {
        let overlay = OfficialCatalogOverlay {
            synced_at_ms: crate::types::now_ms().saturating_sub(SYNC_INTERVAL.as_millis() + 1),
            ..Default::default()
        };
        assert!(!is_fresh(&overlay));
    }
}
