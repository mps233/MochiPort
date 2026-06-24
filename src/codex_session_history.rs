use std::{
    collections::BTreeMap,
    fs::FileTimes,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{chain_log, codex_app_config};

const AI_GATEWAY_PROVIDER_NAME: &str = "ai-gateway";
const MIGRATION_LOG_FILE: &str = "session-provider-moves.json";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionProviderMoveReport {
    pub thread_id: String,
    pub rollout_path: PathBuf,
    pub from_provider: Option<String>,
    pub to_provider: String,
    pub sqlite_updated: bool,
    pub migration_log_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionProviderMoveRecord {
    thread_id: String,
    rollout_path: PathBuf,
    original_provider: Option<String>,
    previous_provider: Option<String>,
    current_provider: String,
    updated_at_ms: i64,
}

pub fn move_thread_to_ai_gateway(
    codex_home: Option<PathBuf>,
    thread_id: &str,
    rollout_path: Option<PathBuf>,
) -> Result<SessionProviderMoveReport> {
    move_thread_to_provider(
        codex_home,
        thread_id,
        rollout_path,
        AI_GATEWAY_PROVIDER_NAME,
    )
}

pub fn move_thread_to_provider(
    codex_home: Option<PathBuf>,
    thread_id: &str,
    rollout_path: Option<PathBuf>,
    target_provider: &str,
) -> Result<SessionProviderMoveReport> {
    let thread_id = thread_id.trim();
    if thread_id.is_empty() {
        return Err(anyhow!("thread_id is required"));
    }
    let target_provider = target_provider.trim();
    if target_provider.is_empty() {
        return Err(anyhow!("target_provider is required"));
    }

    let codex_home = codex_home.unwrap_or_else(codex_app_config::default_codex_home);
    let rollout_path = resolve_rollout_path(&codex_home, thread_id, rollout_path)?;
    let from_provider = rewrite_rollout_provider(&rollout_path, thread_id, target_provider)?;
    let sqlite_updated = update_state_db_provider(&codex_home, thread_id, target_provider)?;
    let migration_log_path = record_provider_move(
        &codex_home,
        thread_id,
        &rollout_path,
        from_provider.clone(),
        target_provider,
    )?;

    chain_log::write_line(format!(
        "[codex_session_history] event=provider_move thread_id={} from={} to={} rollout={} sqlite_updated={}",
        thread_id,
        from_provider.as_deref().unwrap_or(""),
        target_provider,
        rollout_path.display(),
        sqlite_updated
    ));

    Ok(SessionProviderMoveReport {
        thread_id: thread_id.to_string(),
        rollout_path,
        from_provider,
        to_provider: target_provider.to_string(),
        sqlite_updated,
        migration_log_path,
    })
}

fn resolve_rollout_path(
    codex_home: &Path,
    thread_id: &str,
    rollout_path: Option<PathBuf>,
) -> Result<PathBuf> {
    if let Some(path) = rollout_path {
        if !path.exists() {
            return Err(anyhow!("rollout path does not exist: {}", path.display()));
        }
        return Ok(path);
    }

    if let Some(path) = find_rollout_path_in_state_db(codex_home, thread_id)? {
        if path.exists() {
            return Ok(path);
        }
    }

    Err(anyhow!(
        "rollout path is required because thread {} was not found in Codex state DB",
        thread_id
    ))
}

fn find_rollout_path_in_state_db(codex_home: &Path, thread_id: &str) -> Result<Option<PathBuf>> {
    let Some(db_path) = existing_state_db_path(codex_home) else {
        return Ok(None);
    };
    let conn = open_state_db_readonly(&db_path)?;
    let path = conn
        .query_row(
            "SELECT rollout_path FROM threads WHERE id = ?",
            params![thread_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .with_context(|| format!("failed to query {}", db_path.display()))?;
    Ok(path.map(PathBuf::from))
}

fn rewrite_rollout_provider(
    rollout_path: &Path,
    expected_thread_id: &str,
    target_provider: &str,
) -> Result<Option<String>> {
    let raw = std::fs::read_to_string(rollout_path)
        .with_context(|| format!("failed to read {}", rollout_path.display()))?;
    let original_times = file_times(rollout_path)?;
    let had_trailing_newline = raw.ends_with('\n');
    let mut lines = raw.lines().map(str::to_string).collect::<Vec<_>>();
    let first_line = lines
        .first_mut()
        .ok_or_else(|| anyhow!("rollout is empty: {}", rollout_path.display()))?;
    let mut value: Value = serde_json::from_str(first_line).with_context(|| {
        format!(
            "failed to parse first rollout line in {}",
            rollout_path.display()
        )
    })?;

    let actual_type = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if actual_type != "session_meta" {
        return Err(anyhow!(
            "first rollout line is not session_meta in {}",
            rollout_path.display()
        ));
    }
    let payload = value
        .get_mut("payload")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| anyhow!("first rollout line has no session_meta payload"))?;
    let actual_thread_id = payload
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("session_meta missing id in {}", rollout_path.display()))?;
    if actual_thread_id != expected_thread_id {
        return Err(anyhow!(
            "thread id mismatch for {}: expected {}, got {}",
            rollout_path.display(),
            expected_thread_id,
            actual_thread_id
        ));
    }

    let previous_provider = payload
        .get("model_provider")
        .and_then(Value::as_str)
        .map(str::to_string);
    payload.insert(
        "model_provider".to_string(),
        Value::String(target_provider.to_string()),
    );
    *first_line = serde_json::to_string(&value)?;

    let mut output = lines.join("\n");
    if had_trailing_newline {
        output.push('\n');
    }
    std::fs::write(rollout_path, output)
        .with_context(|| format!("failed to write {}", rollout_path.display()))?;
    restore_file_times(rollout_path, original_times)?;
    Ok(previous_provider)
}

fn file_times(path: &Path) -> Result<FileTimes> {
    let metadata =
        std::fs::metadata(path).with_context(|| format!("failed to stat {}", path.display()))?;
    let mut times = FileTimes::new();
    if let Ok(accessed) = metadata.accessed() {
        times = times.set_accessed(accessed);
    }
    if let Ok(modified) = metadata.modified() {
        times = times.set_modified(modified);
    }
    Ok(times)
}

fn restore_file_times(path: &Path, times: FileTimes) -> Result<()> {
    std::fs::File::options()
        .write(true)
        .open(path)
        .with_context(|| format!("failed to reopen {}", path.display()))?
        .set_times(times)
        .with_context(|| format!("failed to restore timestamps for {}", path.display()))
}

fn update_state_db_provider(
    codex_home: &Path,
    thread_id: &str,
    target_provider: &str,
) -> Result<bool> {
    let Some(db_path) = existing_state_db_path(codex_home) else {
        return Ok(false);
    };
    let conn = open_state_db_write(&db_path)?;
    let changed = conn
        .execute(
            "UPDATE threads SET model_provider = ? WHERE id = ?",
            params![target_provider, thread_id],
        )
        .with_context(|| format!("failed to update {}", db_path.display()))?;
    Ok(changed > 0)
}

fn record_provider_move(
    codex_home: &Path,
    thread_id: &str,
    rollout_path: &Path,
    previous_provider: Option<String>,
    target_provider: &str,
) -> Result<PathBuf> {
    let path = migration_log_path(codex_home);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let mut records = read_migration_records(&path)?;
    let existing = records.get(thread_id).cloned();
    let original_provider = existing
        .as_ref()
        .and_then(|record| record.original_provider.clone())
        .or(previous_provider.clone());
    records.insert(
        thread_id.to_string(),
        SessionProviderMoveRecord {
            thread_id: thread_id.to_string(),
            rollout_path: rollout_path.to_path_buf(),
            original_provider,
            previous_provider,
            current_provider: target_provider.to_string(),
            updated_at_ms: unix_now_millis()?,
        },
    );
    let pretty = serde_json::to_string_pretty(&records)?;
    std::fs::write(&path, format!("{pretty}\n"))
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

fn read_migration_records(path: &Path) -> Result<BTreeMap<String, SessionProviderMoveRecord>> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(BTreeMap::new());
    }
    serde_json::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))
}

fn migration_log_path(codex_home: &Path) -> PathBuf {
    codexhub_app_support_dir()
        .join("codex-app")
        .join(codex_home_id(codex_home))
        .join(MIGRATION_LOG_FILE)
}

fn existing_state_db_path(codex_home: &Path) -> Option<PathBuf> {
    let sqlite_home = std::env::var_os("CODEX_SQLITE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| codex_home.to_path_buf());
    [
        "state_5.sqlite",
        "state.v2.sqlite",
        "state.sqlite",
        "codex.sqlite",
    ]
    .into_iter()
    .map(|name| sqlite_home.join(name))
    .find(|path| path.exists())
}

fn open_state_db_readonly(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("failed to open {}", path.display()))?;
    Ok(conn)
}

fn open_state_db_write(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("failed to open {}", path.display()))?;
    conn.busy_timeout(std::time::Duration::from_secs(2))?;
    Ok(conn)
}

fn codexhub_app_support_dir() -> PathBuf {
    if let Some(base) = std::env::var_os("CODEXHUB_HOME").map(PathBuf::from) {
        return base;
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("LOCALAPPDATA")
            .or_else(|| std::env::var_os("APPDATA"))
            .map(PathBuf::from)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."))
            .join("CodexHub")
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join("Library/Application Support/CodexHub"))
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."))
    }
}

fn codex_home_id(codex_home: &Path) -> String {
    let normalized = codex_home
        .to_string_lossy()
        .replace('\\', "/")
        .to_lowercase();
    let digest = Sha256::digest(normalized.as_bytes());
    hex::encode(&digest[..16])
}

fn unix_now_millis() -> Result<i64> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| anyhow!("system time is before UNIX epoch: {err}"))?
        .as_millis();
    i64::try_from(millis).map_err(|_| anyhow!("system time is too large for timestamp"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rewrites_first_session_meta_provider() {
        let dir = unique_temp_dir();
        let rollout = dir.join("rollout.jsonl");
        let first = json!({
            "timestamp": "2026-01-01T00:00:00Z",
            "type": "session_meta",
            "payload": {
                "id": "thread-1",
                "timestamp": "2026-01-01T00:00:00Z",
                "cwd": "D:/repo",
                "originator": "codex",
                "cli_version": "test",
                "source": "vscode",
                "model_provider": "openai"
            }
        });
        let second = json!({
            "timestamp": "2026-01-01T00:00:01Z",
            "type": "response_item",
            "payload": {"type": "message", "role": "user", "content": []}
        });
        std::fs::write(
            &rollout,
            format!("{}\n{}\n", serde_json::to_string(&first).unwrap(), second),
        )
        .unwrap();

        let previous = rewrite_rollout_provider(&rollout, "thread-1", "ai-gateway").unwrap();
        assert_eq!(previous.as_deref(), Some("openai"));
        let raw = std::fs::read_to_string(&rollout).unwrap();
        let lines = raw.lines().collect::<Vec<_>>();
        let first: Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(
            first["payload"]["model_provider"].as_str(),
            Some("ai-gateway")
        );
        assert_eq!(lines.len(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn preserves_rollout_modified_time_when_rewriting_provider() {
        let dir = unique_temp_dir();
        let rollout = dir.join("rollout.jsonl");
        let first = json!({
            "timestamp": "2026-01-01T00:00:00Z",
            "type": "session_meta",
            "payload": {
                "id": "thread-1",
                "timestamp": "2026-01-01T00:00:00Z",
                "cwd": "D:/repo",
                "originator": "codex",
                "cli_version": "test",
                "source": "vscode",
                "model_provider": "openai"
            }
        });
        std::fs::write(
            &rollout,
            format!("{}\n", serde_json::to_string(&first).unwrap()),
        )
        .unwrap();
        let original = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        std::fs::File::options()
            .write(true)
            .open(&rollout)
            .unwrap()
            .set_times(FileTimes::new().set_modified(original))
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));

        rewrite_rollout_provider(&rollout, "thread-1", "ai-gateway").unwrap();

        let modified = std::fs::metadata(&rollout).unwrap().modified().unwrap();
        assert_eq!(modified, original);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn updates_state_db_provider_when_present() {
        let dir = unique_temp_dir();
        let db = dir.join("state.v2.sqlite");
        let conn = Connection::open(&db).unwrap();
        conn.execute(
            "CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT NOT NULL, model_provider TEXT NOT NULL)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO threads (id, rollout_path, model_provider) VALUES ('thread-1', 'x', 'openai')",
            [],
        )
        .unwrap();
        drop(conn);

        assert!(update_state_db_provider(&dir, "thread-1", "ai-gateway").unwrap());
        let conn = Connection::open(&db).unwrap();
        let provider: String = conn
            .query_row(
                "SELECT model_provider FROM threads WHERE id = 'thread-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(provider, "ai-gateway");
        let _ = std::fs::remove_dir_all(dir);
    }

    fn unique_temp_dir() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "codexhub-session-history-test-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
