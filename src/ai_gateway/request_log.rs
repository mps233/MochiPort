use std::{
    collections::BTreeMap,
    collections::VecDeque,
    fs,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, Mutex, MutexGuard},
    task::{Context, Poll},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use axum::body::Bytes;
use axum::http::HeaderMap;
use futures_util::Stream;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::warn;

use crate::config::AppConfig;

const DB_FILE_NAME: &str = "ai-gateway-request-logs.sqlite";

#[derive(Debug, Clone, Default)]
pub struct LogUsage {
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub read_cache_tokens: Option<i64>,
    pub read_cache_hit_rate: Option<f64>,
    pub write_cache_tokens: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct RequestLogRecord {
    pub request_id: String,
    pub model_id: String,
    pub stream: bool,
    pub channel: String,
    pub provider_type: String,
    pub status: String,
    pub usage: LogUsage,
    pub cost_usd: Option<f64>,
    pub latency_ms: Option<i64>,
    pub ttft_ms: Option<i64>,
    pub created_at_ms: i64,
    pub error_message: Option<String>,
    pub request_headers_json: Option<String>,
    pub request_json: Option<String>,
    pub upstream_request_body_bytes: Option<i64>,
    pub upstream_request_headers_json: Option<String>,
    pub upstream_request_json: Option<String>,
    pub response_json: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct RequestLogUpdate {
    pub status: Option<String>,
    pub usage: Option<LogUsage>,
    pub cost_usd: Option<f64>,
    pub latency_ms: Option<i64>,
    pub ttft_ms: Option<i64>,
    pub error_message: Option<String>,
    pub upstream_request_body_bytes: Option<i64>,
    pub upstream_request_headers_json: Option<String>,
    pub upstream_request_json: Option<String>,
    pub response_json: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestLogEntry {
    pub id: i64,
    pub request_id: String,
    pub model_id: String,
    pub stream: bool,
    pub channel: String,
    pub provider_type: String,
    pub status: String,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub read_cache_tokens: Option<i64>,
    pub read_cache_hit_rate: Option<f64>,
    pub write_cache_tokens: Option<i64>,
    pub cost_usd: Option<f64>,
    pub latency_ms: Option<i64>,
    pub ttft_ms: Option<i64>,
    pub created_at_ms: i64,
    pub created_at: String,
    pub error_message: Option<String>,
    pub upstream_request_body_bytes: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestLogDetail {
    #[serde(flatten)]
    pub summary: RequestLogEntry,
    pub request_headers_json: Option<String>,
    pub request_json: Option<String>,
    pub upstream_request_headers_json: Option<String>,
    pub upstream_request_json: Option<String>,
    pub response_json: Option<String>,
}

#[derive(Clone)]
pub struct RequestLogContext {
    pub store: RequestLogStore,
    pub log_id: i64,
    pub started_at: Instant,
}

#[derive(Clone)]
pub struct RequestLogStore {
    inner: Arc<RequestLogStoreInner>,
}

struct RequestLogStoreInner {
    db_path: PathBuf,
    conn: Mutex<Option<Connection>>,
}

impl RequestLogStore {
    pub fn new(db_path: PathBuf) -> Self {
        let conn = match open(&db_path) {
            Ok(conn) => Some(conn),
            Err(err) => {
                warn!(
                    error = %err,
                    path = %db_path.display(),
                    "failed to open AI Gateway request log database"
                );
                None
            }
        };
        Self {
            inner: Arc::new(RequestLogStoreInner {
                db_path,
                conn: Mutex::new(conn),
            }),
        }
    }

    #[allow(dead_code)]
    pub fn db_path(&self) -> &Path {
        &self.inner.db_path
    }

    pub fn insert_record(&self, record: &RequestLogRecord) -> rusqlite::Result<i64> {
        self.with_conn(|conn| insert_record_with_conn(conn, record))
    }

    pub fn update_record(&self, id: i64, update: &RequestLogUpdate) -> rusqlite::Result<()> {
        self.with_conn(|conn| update_record_with_conn(conn, id, update))
    }

    pub fn list_recent(&self, limit: usize) -> rusqlite::Result<Vec<RequestLogEntry>> {
        self.with_conn(|conn| list_recent_with_conn(conn, limit))
    }

    pub fn delete_older_than(&self, cutoff_ms: i64) -> rusqlite::Result<usize> {
        self.with_conn(|conn| delete_older_than_with_conn(conn, cutoff_ms))
    }

    pub fn delete_all(&self) -> rusqlite::Result<usize> {
        self.with_conn(delete_all_with_conn)
    }

    pub fn get_detail(&self, id: i64) -> rusqlite::Result<Option<RequestLogDetail>> {
        self.with_conn(|conn| get_detail_with_conn(conn, id))
    }

    fn with_conn<T>(
        &self,
        operation: impl FnOnce(&Connection) -> rusqlite::Result<T>,
    ) -> rusqlite::Result<T> {
        let mut guard = lock_connection(&self.inner.conn);
        if guard.is_none() {
            *guard = Some(open(&self.inner.db_path)?);
        }
        operation(guard.as_ref().expect("request log connection initialized"))
    }
}

fn lock_connection(mutex: &Mutex<Option<Connection>>) -> MutexGuard<'_, Option<Connection>> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            warn!("AI Gateway request log connection lock was poisoned; continuing");
            poisoned.into_inner()
        }
    }
}

pub fn database_path(config: &AppConfig) -> PathBuf {
    #[cfg(test)]
    {
        return legacy_database_path(config);
    }

    #[cfg(not(test))]
    if let Some(dir) = app_data_dir() {
        return dir.join(DB_FILE_NAME);
    }

    #[allow(unreachable_code)]
    legacy_database_path(config)
}

pub fn migrate_legacy_database(config: &AppConfig, target_path: &Path) {
    let source_path = legacy_database_path(config);
    if paths_equivalent(&source_path, target_path) || !source_path.exists() || target_path.exists()
    {
        return;
    }

    if let Some(parent) = target_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
    {
        if let Err(err) = fs::create_dir_all(parent) {
            warn!(
                error = %err,
                path = %parent.display(),
                "failed to create AI Gateway request log directory"
            );
            return;
        }
    }

    let source_conn = match Connection::open(&source_path) {
        Ok(conn) => conn,
        Err(err) => {
            warn!(
                error = %err,
                source = %source_path.display(),
                target = %target_path.display(),
                "failed to open legacy AI Gateway request log database for migration"
            );
            return;
        }
    };
    let _ = source_conn.busy_timeout(std::time::Duration::from_millis(1000));
    let escaped_target = target_path.to_string_lossy().replace('\'', "''");
    if let Err(err) = source_conn.execute_batch(&format!("VACUUM INTO '{escaped_target}'")) {
        warn!(
            error = %err,
            source = %source_path.display(),
            target = %target_path.display(),
            "failed to migrate legacy AI Gateway request log database"
        );
        return;
    }
    drop(source_conn);

    remove_legacy_database_files(&source_path);
}

fn legacy_database_path(config: &AppConfig) -> PathBuf {
    config
        .state_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(DB_FILE_NAME)
}

#[cfg(not(test))]
fn app_data_dir() -> Option<PathBuf> {
    if let Some(base) = std::env::var_os("CODEX_REMOTE_HOME").map(PathBuf::from) {
        return Some(base);
    }

    #[cfg(target_os = "windows")]
    {
        std::env::var_os("LOCALAPPDATA")
            .or_else(|| std::env::var_os("APPDATA"))
            .map(PathBuf::from)
            .map(|base| base.join("Codex Remote"))
    }

    #[cfg(not(target_os = "windows"))]
    {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join("Library/Application Support/Codex Remote"))
    }
}

fn paths_equivalent(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn remove_legacy_database_files(db_path: &Path) {
    for path in companion_database_paths(db_path) {
        if !path.exists() {
            continue;
        }
        if let Err(err) = fs::remove_file(&path) {
            warn!(
                error = %err,
                path = %path.display(),
                "failed to remove legacy AI Gateway request log file"
            );
        }
    }
}

fn companion_database_paths(db_path: &Path) -> [PathBuf; 3] {
    [
        db_path.to_path_buf(),
        PathBuf::from(format!("{}-wal", db_path.display())),
        PathBuf::from(format!("{}-shm", db_path.display())),
    ]
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

pub fn elapsed_ms(started_at: Instant) -> i64 {
    started_at.elapsed().as_millis() as i64
}

pub fn headers_to_json(headers: &HeaderMap) -> Option<String> {
    let mut grouped: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, value) in headers.iter() {
        let value = String::from_utf8_lossy(value.as_bytes()).into_owned();
        grouped
            .entry(name.as_str().to_string())
            .or_default()
            .push(value);
    }

    let mut object = serde_json::Map::new();
    for (name, values) in grouped {
        if values.len() == 1 {
            object.insert(name, Value::String(values.into_iter().next().unwrap()));
        } else {
            object.insert(
                name,
                Value::Array(values.into_iter().map(Value::String).collect()),
            );
        }
    }
    serde_json::to_string(&Value::Object(object)).ok()
}

pub fn json_body_size_bytes(value: &Value) -> Option<i64> {
    serde_json::to_vec(value)
        .ok()
        .and_then(|bytes| i64::try_from(bytes.len()).ok())
}

#[cfg(test)]
pub fn insert_record(db_path: &Path, record: &RequestLogRecord) -> rusqlite::Result<i64> {
    let conn = open(db_path)?;
    insert_record_with_conn(&conn, record)
}

fn insert_record_with_conn(conn: &Connection, record: &RequestLogRecord) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO ai_gateway_request_logs (
            request_id, model_id, stream, channel, provider_type, status,
            input_tokens, output_tokens, total_tokens, read_cache_tokens,
            read_cache_hit_rate, write_cache_tokens, cost_usd, latency_ms,
            ttft_ms, created_at_ms, error_message, request_headers_json, request_json,
            upstream_request_body_bytes, upstream_request_headers_json, upstream_request_json,
            response_json
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)",
        params![
            &record.request_id,
            &record.model_id,
            record.stream as i64,
            &record.channel,
            &record.provider_type,
            &record.status,
            record.usage.input_tokens,
            record.usage.output_tokens,
            record.usage.total_tokens,
            record.usage.read_cache_tokens,
            record.usage.read_cache_hit_rate,
            record.usage.write_cache_tokens,
            record.cost_usd,
            record.latency_ms,
            record.ttft_ms,
            record.created_at_ms,
            &record.error_message,
            &record.request_headers_json,
            &record.request_json,
            record.upstream_request_body_bytes,
            &record.upstream_request_headers_json,
            &record.upstream_request_json,
            &record.response_json,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

#[cfg(test)]
pub fn update_record(db_path: &Path, id: i64, update: &RequestLogUpdate) -> rusqlite::Result<()> {
    let conn = open(db_path)?;
    update_record_with_conn(&conn, id, update)
}

fn update_record_with_conn(
    conn: &Connection,
    id: i64,
    update: &RequestLogUpdate,
) -> rusqlite::Result<()> {
    let existing = conn
        .query_row(
            "SELECT
                status, input_tokens, output_tokens, total_tokens, read_cache_tokens,
                read_cache_hit_rate, write_cache_tokens, cost_usd, latency_ms,
                ttft_ms, error_message, upstream_request_body_bytes,
                upstream_request_headers_json, upstream_request_json, response_json
             FROM ai_gateway_request_logs WHERE id = ?1",
            params![id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, Option<f64>>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                    row.get::<_, Option<f64>>(7)?,
                    row.get::<_, Option<i64>>(8)?,
                    row.get::<_, Option<i64>>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, Option<i64>>(11)?,
                    row.get::<_, Option<String>>(12)?,
                    row.get::<_, Option<String>>(13)?,
                    row.get::<_, Option<String>>(14)?,
                ))
            },
        )
        .optional()?;

    let Some(existing) = existing else {
        return Ok(());
    };
    let usage = update.usage.clone().unwrap_or(LogUsage {
        input_tokens: existing.1,
        output_tokens: existing.2,
        total_tokens: existing.3,
        read_cache_tokens: existing.4,
        read_cache_hit_rate: existing.5,
        write_cache_tokens: existing.6,
    });

    conn.execute(
        "UPDATE ai_gateway_request_logs SET
            status = ?1,
            input_tokens = ?2,
            output_tokens = ?3,
            total_tokens = ?4,
            read_cache_tokens = ?5,
            read_cache_hit_rate = ?6,
            write_cache_tokens = ?7,
            cost_usd = ?8,
            latency_ms = ?9,
            ttft_ms = ?10,
            error_message = ?11,
            upstream_request_body_bytes = ?12,
            upstream_request_headers_json = ?13,
            upstream_request_json = ?14,
            response_json = ?15
         WHERE id = ?16",
        params![
            update.status.as_deref().unwrap_or(&existing.0),
            usage.input_tokens,
            usage.output_tokens,
            usage.total_tokens,
            usage.read_cache_tokens,
            usage.read_cache_hit_rate,
            usage.write_cache_tokens,
            update.cost_usd.or(existing.7),
            update.latency_ms.or(existing.8),
            update.ttft_ms.or(existing.9),
            update.error_message.clone().or(existing.10),
            update.upstream_request_body_bytes.or(existing.11),
            update.upstream_request_headers_json.clone().or(existing.12),
            update.upstream_request_json.clone().or(existing.13),
            update.response_json.clone().or(existing.14),
            id,
        ],
    )?;
    Ok(())
}

#[cfg(test)]
pub fn list_recent(db_path: &Path, limit: usize) -> rusqlite::Result<Vec<RequestLogEntry>> {
    let conn = open(db_path)?;
    list_recent_with_conn(&conn, limit)
}

fn list_recent_with_conn(
    conn: &Connection,
    limit: usize,
) -> rusqlite::Result<Vec<RequestLogEntry>> {
    let mut stmt = conn.prepare(
        "SELECT
            id, request_id, model_id, stream, channel, provider_type, status,
            input_tokens, output_tokens, total_tokens, read_cache_tokens,
            read_cache_hit_rate, write_cache_tokens, cost_usd, latency_ms,
            ttft_ms, created_at_ms,
            datetime(created_at_ms / 1000, 'unixepoch', 'localtime') AS created_at,
            error_message, upstream_request_body_bytes
         FROM ai_gateway_request_logs
         ORDER BY created_at_ms DESC, id DESC
         LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit.min(500) as i64], |row| {
        Ok(RequestLogEntry {
            id: row.get(0)?,
            request_id: row.get(1)?,
            model_id: row.get(2)?,
            stream: row.get::<_, i64>(3)? != 0,
            channel: row.get(4)?,
            provider_type: row.get(5)?,
            status: row.get(6)?,
            input_tokens: row.get(7)?,
            output_tokens: row.get(8)?,
            total_tokens: row.get(9)?,
            read_cache_tokens: row.get(10)?,
            read_cache_hit_rate: row.get(11)?,
            write_cache_tokens: row.get(12)?,
            cost_usd: row.get(13)?,
            latency_ms: row.get(14)?,
            ttft_ms: row.get(15)?,
            created_at_ms: row.get(16)?,
            created_at: row.get(17)?,
            error_message: row.get(18)?,
            upstream_request_body_bytes: row.get(19)?,
        })
    })?;

    let mut logs = Vec::new();
    for row in rows {
        logs.push(row?);
    }
    Ok(logs)
}

#[cfg(test)]
pub fn delete_older_than(db_path: &Path, cutoff_ms: i64) -> rusqlite::Result<usize> {
    let conn = open(db_path)?;
    delete_older_than_with_conn(&conn, cutoff_ms)
}

fn delete_older_than_with_conn(conn: &Connection, cutoff_ms: i64) -> rusqlite::Result<usize> {
    conn.execute(
        "DELETE FROM ai_gateway_request_logs WHERE created_at_ms < ?1",
        params![cutoff_ms],
    )
}

#[cfg(test)]
pub fn delete_all(db_path: &Path) -> rusqlite::Result<usize> {
    let conn = open(db_path)?;
    delete_all_with_conn(&conn)
}

fn delete_all_with_conn(conn: &Connection) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM ai_gateway_request_logs", [])
}

#[cfg(test)]
pub fn get_detail(db_path: &Path, id: i64) -> rusqlite::Result<Option<RequestLogDetail>> {
    let conn = open(db_path)?;
    get_detail_with_conn(&conn, id)
}

fn get_detail_with_conn(conn: &Connection, id: i64) -> rusqlite::Result<Option<RequestLogDetail>> {
    conn.query_row(
        "SELECT
            id, request_id, model_id, stream, channel, provider_type, status,
            input_tokens, output_tokens, total_tokens, read_cache_tokens,
            read_cache_hit_rate, write_cache_tokens, cost_usd, latency_ms,
            ttft_ms, created_at_ms,
            datetime(created_at_ms / 1000, 'unixepoch', 'localtime') AS created_at,
            error_message, request_headers_json, request_json,
            upstream_request_body_bytes, upstream_request_headers_json, upstream_request_json,
            response_json
         FROM ai_gateway_request_logs
         WHERE id = ?1",
        params![id],
        |row| {
            Ok(RequestLogDetail {
                summary: RequestLogEntry {
                    id: row.get(0)?,
                    request_id: row.get(1)?,
                    model_id: row.get(2)?,
                    stream: row.get::<_, i64>(3)? != 0,
                    channel: row.get(4)?,
                    provider_type: row.get(5)?,
                    status: row.get(6)?,
                    input_tokens: row.get(7)?,
                    output_tokens: row.get(8)?,
                    total_tokens: row.get(9)?,
                    read_cache_tokens: row.get(10)?,
                    read_cache_hit_rate: row.get(11)?,
                    write_cache_tokens: row.get(12)?,
                    cost_usd: row.get(13)?,
                    latency_ms: row.get(14)?,
                    ttft_ms: row.get(15)?,
                    created_at_ms: row.get(16)?,
                    created_at: row.get(17)?,
                    error_message: row.get(18)?,
                    upstream_request_body_bytes: row.get(21)?,
                },
                request_headers_json: row.get(19)?,
                request_json: row.get(20)?,
                upstream_request_headers_json: row.get(22)?,
                upstream_request_json: row.get(23)?,
                response_json: row.get(24)?,
            })
        },
    )
    .optional()
}

pub fn usage_from_response_value(response: &Value) -> LogUsage {
    let Some(usage) = response.get("usage") else {
        return LogUsage::default();
    };

    let input_tokens = first_i64(usage, &["input_tokens", "prompt_tokens"]);
    let output_tokens = first_i64(usage, &["output_tokens", "completion_tokens"]);
    let total_tokens =
        first_i64(usage, &["total_tokens"]).or_else(|| match (input_tokens, output_tokens) {
            (Some(input), Some(output)) => Some(input + output),
            _ => None,
        });
    let read_cache_tokens = usage
        .get("input_tokens_details")
        .and_then(|details| first_i64(details, &["cached_tokens", "cache_read_input_tokens"]))
        .or_else(|| {
            usage.get("prompt_tokens_details").and_then(|details| {
                first_i64(details, &["cached_tokens", "cache_read_input_tokens"])
            })
        })
        .or_else(|| {
            first_i64(
                usage,
                &[
                    "cache_read_input_tokens",
                    "cached_tokens",
                    "prompt_cache_hit_tokens",
                ],
            )
        });
    let write_cache_tokens = usage
        .get("input_tokens_details")
        .and_then(|details| {
            first_i64(
                details,
                &[
                    "cache_creation_tokens",
                    "cache_write_input_tokens",
                    "write_cached_tokens",
                ],
            )
        })
        .or_else(|| {
            usage.get("prompt_tokens_details").and_then(|details| {
                first_i64(
                    details,
                    &[
                        "cache_creation_tokens",
                        "cache_write_input_tokens",
                        "write_cached_tokens",
                    ],
                )
            })
        })
        .or_else(|| {
            first_i64(
                usage,
                &[
                    "cache_creation_input_tokens",
                    "cache_write_input_tokens",
                    "write_cached_tokens",
                ],
            )
        });
    let read_cache_hit_rate = match (read_cache_tokens, input_tokens) {
        (Some(cached), Some(input)) if input > 0 => Some(cached as f64 / input as f64),
        _ => None,
    };

    LogUsage {
        input_tokens,
        output_tokens,
        total_tokens,
        read_cache_tokens,
        read_cache_hit_rate,
        write_cache_tokens,
    }
}

pub fn status_from_response_value(response: &Value) -> String {
    response
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("completed")
        .to_string()
}

pub fn log_insert_error(err: rusqlite::Error) {
    warn!(error = %err, "failed to write AI Gateway request log");
}

pub fn log_update_error(err: rusqlite::Error) {
    warn!(error = %err, "failed to update AI Gateway request log");
}

pub struct ResponsesSseLogStream<S> {
    inner: S,
    context: RequestLogContext,
    line_buf: String,
    completed: bool,
    ttft_recorded: bool,
    output_queue: VecDeque<Result<Bytes, std::io::Error>>,
}

impl<S> ResponsesSseLogStream<S> {
    pub fn new(inner: S, context: RequestLogContext) -> Self {
        Self {
            inner,
            context,
            line_buf: String::new(),
            completed: false,
            ttft_recorded: false,
            output_queue: VecDeque::new(),
        }
    }
}

impl<S> Drop for ResponsesSseLogStream<S> {
    fn drop(&mut self) {
        if self.completed {
            return;
        }

        let update = RequestLogUpdate {
            status: Some("cancelled".to_string()),
            latency_ms: Some(elapsed_ms(self.context.started_at)),
            error_message: Some("client disconnected before stream completed".to_string()),
            ..RequestLogUpdate::default()
        };
        if let Err(err) = self
            .context
            .store
            .update_record(self.context.log_id, &update)
        {
            log_update_error(err);
        }
        self.completed = true;
    }
}

impl<S> Stream for ResponsesSseLogStream<S>
where
    S: Stream<Item = Result<Bytes, std::io::Error>> + Unpin,
{
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if let Some(item) = this.output_queue.pop_front() {
            return Poll::Ready(Some(item));
        }

        match Pin::new(&mut this.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(chunk))) => {
                observe_sse_chunk(
                    &chunk,
                    &mut this.line_buf,
                    &this.context,
                    &mut this.completed,
                    &mut this.ttft_recorded,
                );
                Poll::Ready(Some(Ok(chunk)))
            }
            Poll::Ready(Some(Err(err))) => {
                if !this.completed {
                    let update = RequestLogUpdate {
                        status: Some("failed".to_string()),
                        latency_ms: Some(elapsed_ms(this.context.started_at)),
                        error_message: Some(err.to_string()),
                        ..RequestLogUpdate::default()
                    };
                    if let Err(update_err) = this
                        .context
                        .store
                        .update_record(this.context.log_id, &update)
                    {
                        log_update_error(update_err);
                    }
                    this.completed = true;
                }
                Poll::Ready(Some(Err(err)))
            }
            Poll::Ready(None) => {
                if !this.completed {
                    let update = RequestLogUpdate {
                        status: Some("failed".to_string()),
                        latency_ms: Some(elapsed_ms(this.context.started_at)),
                        error_message: Some("stream closed before response.completed".to_string()),
                        ..RequestLogUpdate::default()
                    };
                    if let Err(update_err) = this
                        .context
                        .store
                        .update_record(this.context.log_id, &update)
                    {
                        log_update_error(update_err);
                    }
                    this.completed = true;
                }
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

fn open(db_path: &Path) -> rusqlite::Result<Connection> {
    if let Some(parent) = db_path.parent().filter(|path| !path.as_os_str().is_empty()) {
        let _ = std::fs::create_dir_all(parent);
    }
    let conn = Connection::open(db_path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.busy_timeout(std::time::Duration::from_millis(1000))?;
    init_schema(&conn)?;
    Ok(conn)
}

fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS ai_gateway_request_logs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            request_id TEXT NOT NULL,
            model_id TEXT NOT NULL,
            stream INTEGER NOT NULL,
            channel TEXT NOT NULL,
            provider_type TEXT NOT NULL,
            status TEXT NOT NULL,
            input_tokens INTEGER,
            output_tokens INTEGER,
            total_tokens INTEGER,
            read_cache_tokens INTEGER,
            read_cache_hit_rate REAL,
            write_cache_tokens INTEGER,
            cost_usd REAL,
            latency_ms INTEGER,
            ttft_ms INTEGER,
            created_at_ms INTEGER NOT NULL,
            error_message TEXT,
            request_headers_json TEXT,
            request_json TEXT,
            upstream_request_body_bytes INTEGER,
            upstream_request_headers_json TEXT,
            upstream_request_json TEXT,
            response_json TEXT
        );
        "#,
    )?;

    add_text_column_if_missing(conn, "request_headers_json")?;
    add_integer_column_if_missing(conn, "upstream_request_body_bytes")?;
    add_text_column_if_missing(conn, "upstream_request_headers_json")?;
    add_text_column_if_missing(conn, "upstream_request_json")?;

    conn.execute_batch(
        r#"
        CREATE INDEX IF NOT EXISTS idx_ai_gateway_request_logs_created
            ON ai_gateway_request_logs(created_at_ms DESC, id DESC);
        CREATE INDEX IF NOT EXISTS idx_ai_gateway_request_logs_model
            ON ai_gateway_request_logs(model_id);
        CREATE INDEX IF NOT EXISTS idx_ai_gateway_request_logs_channel
            ON ai_gateway_request_logs(channel);
        CREATE INDEX IF NOT EXISTS idx_ai_gateway_request_logs_status
            ON ai_gateway_request_logs(status);
        "#,
    )
}

fn add_text_column_if_missing(conn: &Connection, column: &str) -> rusqlite::Result<()> {
    match conn.execute(
        &format!("ALTER TABLE ai_gateway_request_logs ADD COLUMN {column} TEXT"),
        [],
    ) {
        Ok(_) => {}
        Err(err) if is_duplicate_column_error(&err) => {}
        Err(err) => return Err(err),
    }
    Ok(())
}

fn add_integer_column_if_missing(conn: &Connection, column: &str) -> rusqlite::Result<()> {
    match conn.execute(
        &format!("ALTER TABLE ai_gateway_request_logs ADD COLUMN {column} INTEGER"),
        [],
    ) {
        Ok(_) => {}
        Err(err) if is_duplicate_column_error(&err) => {}
        Err(err) => return Err(err),
    }
    Ok(())
}

fn is_duplicate_column_error(err: &rusqlite::Error) -> bool {
    matches!(err, rusqlite::Error::SqliteFailure(_, Some(message)) if message.contains("duplicate column name"))
}

fn observe_sse_chunk(
    chunk: &Bytes,
    line_buf: &mut String,
    context: &RequestLogContext,
    completed: &mut bool,
    ttft_recorded: &mut bool,
) {
    let text = String::from_utf8_lossy(chunk);
    line_buf.push_str(&text);
    while let Some(pos) = line_buf.find('\n') {
        let line = line_buf[..pos].trim_end_matches('\r').to_string();
        *line_buf = line_buf[pos + 1..].to_string();
        let Some(data) = sse_data_value(&line) else {
            continue;
        };
        if data.trim() == "[DONE]" {
            continue;
        }
        let Ok(event) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        let event_type = event.get("type").and_then(Value::as_str).unwrap_or("");
        if is_first_token_event(event_type) && !*ttft_recorded {
            let update = RequestLogUpdate {
                ttft_ms: Some(elapsed_ms(context.started_at)),
                ..RequestLogUpdate::default()
            };
            if let Err(err) = context.store.update_record(context.log_id, &update) {
                log_update_error(err);
            }
            *ttft_recorded = true;
        }
        if !matches!(
            event_type,
            "response.completed" | "response.incomplete" | "response.failed"
        ) {
            continue;
        }
        let response = event.get("response").unwrap_or(&event);
        let status = status_from_response_value(response);
        let usage = usage_from_response_value(response);
        let error_message = response
            .get("error")
            .and_then(|error| error.get("message").or(Some(error)))
            .and_then(Value::as_str)
            .map(str::to_string);
        let update = RequestLogUpdate {
            status: Some(status),
            usage: Some(usage),
            latency_ms: Some(elapsed_ms(context.started_at)),
            error_message,
            response_json: Some(compact_json(response)),
            ..RequestLogUpdate::default()
        };
        if let Err(err) = context.store.update_record(context.log_id, &update) {
            log_update_error(err);
        }
        *completed = true;
    }
}

fn is_first_token_event(event_type: &str) -> bool {
    event_type.starts_with("response.") && event_type.ends_with(".delta")
}

fn sse_data_value(line: &str) -> Option<&str> {
    let data = line.strip_prefix("data:")?;
    Some(data.strip_prefix(' ').unwrap_or(data))
}

fn first_i64(value: &Value, keys: &[&str]) -> Option<i64> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_i64))
}

fn compact_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    use futures_util::{StreamExt, stream};
    use serde_json::json;

    fn temp_db_path() -> PathBuf {
        std::env::temp_dir().join(format!(
            "codex-remote-request-log-test-{}.sqlite",
            uuid::Uuid::new_v4()
        ))
    }

    fn test_store(db_path: &Path) -> RequestLogStore {
        RequestLogStore::new(db_path.to_path_buf())
    }

    fn insert_running_test_log(db_path: &Path, request_id: &str) -> RequestLogContext {
        let store = test_store(db_path);
        let record = RequestLogRecord {
            request_id: request_id.to_string(),
            model_id: "deepseek-v4-flash".to_string(),
            stream: true,
            channel: "deepseek".to_string(),
            provider_type: "chat_completions".to_string(),
            status: "running".to_string(),
            usage: LogUsage::default(),
            cost_usd: None,
            latency_ms: None,
            ttft_ms: None,
            created_at_ms: now_ms(),
            error_message: None,
            request_headers_json: None,
            request_json: None,
            upstream_request_body_bytes: None,
            upstream_request_headers_json: None,
            upstream_request_json: None,
            response_json: None,
        };
        let log_id = store.insert_record(&record).unwrap();
        RequestLogContext {
            store,
            log_id,
            started_at: Instant::now(),
        }
    }

    #[test]
    fn usage_from_responses_value_extracts_cache() {
        let value = json!({
            "usage": {
                "input_tokens": 100,
                "output_tokens": 20,
                "total_tokens": 120,
                "input_tokens_details": {
                    "cached_tokens": 80,
                    "cache_creation_tokens": 5
                }
            }
        });

        let usage = usage_from_response_value(&value);
        assert_eq!(usage.input_tokens, Some(100));
        assert_eq!(usage.output_tokens, Some(20));
        assert_eq!(usage.total_tokens, Some(120));
        assert_eq!(usage.read_cache_tokens, Some(80));
        assert_eq!(usage.write_cache_tokens, Some(5));
        assert_eq!(usage.read_cache_hit_rate, Some(0.8));
    }

    #[test]
    fn usage_from_chat_value_extracts_deepseek_cache() {
        let value = json!({
            "usage": {
                "prompt_tokens": 16,
                "completion_tokens": 645,
                "total_tokens": 661,
                "prompt_cache_hit_tokens": 4
            }
        });

        let usage = usage_from_response_value(&value);
        assert_eq!(usage.input_tokens, Some(16));
        assert_eq!(usage.output_tokens, Some(645));
        assert_eq!(usage.total_tokens, Some(661));
        assert_eq!(usage.read_cache_tokens, Some(4));
        assert_eq!(usage.read_cache_hit_rate, Some(0.25));
    }

    #[test]
    fn headers_to_json_preserves_values() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer local-key".parse().unwrap());
        headers.append("x-debug", "one".parse().unwrap());
        headers.append("x-debug", "two".parse().unwrap());

        let value: Value = serde_json::from_str(&headers_to_json(&headers).unwrap()).unwrap();
        assert_eq!(value["authorization"], "Bearer local-key");
        assert_eq!(value["x-debug"], json!(["one", "two"]));
    }

    #[test]
    fn database_path_follows_state_path_in_tests() {
        let config = AppConfig {
            state_path: std::env::temp_dir().join("codex-remote-test-state.json"),
            ..AppConfig::default()
        };

        assert_eq!(
            database_path(&config),
            config.state_path.parent().unwrap().join(DB_FILE_NAME)
        );
    }

    #[test]
    fn request_log_store_reuses_open_connection() {
        let db_path = temp_db_path();
        let store = test_store(&db_path);
        let record = RequestLogRecord {
            request_id: "req-store".to_string(),
            model_id: "deepseek-v4-flash".to_string(),
            stream: false,
            channel: "deepseek".to_string(),
            provider_type: "chat_completions".to_string(),
            status: "running".to_string(),
            usage: LogUsage::default(),
            cost_usd: None,
            latency_ms: None,
            ttft_ms: None,
            created_at_ms: now_ms(),
            error_message: None,
            request_headers_json: None,
            request_json: None,
            upstream_request_body_bytes: None,
            upstream_request_headers_json: None,
            upstream_request_json: None,
            response_json: None,
        };

        let id = store.insert_record(&record).unwrap();
        store
            .update_record(
                id,
                &RequestLogUpdate {
                    status: Some("completed".to_string()),
                    ..RequestLogUpdate::default()
                },
            )
            .unwrap();

        let logs = store.list_recent(10).unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].status, "completed");
        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn sqlite_insert_update_and_list_roundtrip() {
        let db_path = std::env::temp_dir().join(format!(
            "codex-remote-request-log-test-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        let record = RequestLogRecord {
            request_id: "req-1".to_string(),
            model_id: "deepseek-v4-flash".to_string(),
            stream: true,
            channel: "deepseek".to_string(),
            provider_type: "chat_completions".to_string(),
            status: "running".to_string(),
            usage: LogUsage::default(),
            cost_usd: None,
            latency_ms: None,
            ttft_ms: None,
            created_at_ms: now_ms(),
            error_message: None,
            request_headers_json: Some(r#"{"user-agent":"Codex/1.0"}"#.to_string()),
            request_json: Some(r#"{"model":"deepseek-v4-flash"}"#.to_string()),
            upstream_request_body_bytes: Some(42),
            upstream_request_headers_json: None,
            upstream_request_json: Some(
                r#"{"model":"deepseek-v4-flash","messages":[]}"#.to_string(),
            ),
            response_json: None,
        };

        let id = insert_record(&db_path, &record).unwrap();
        update_record(
            &db_path,
            id,
            &RequestLogUpdate {
                status: Some("completed".to_string()),
                usage: Some(LogUsage {
                    input_tokens: Some(10),
                    output_tokens: Some(3),
                    total_tokens: Some(13),
                    read_cache_tokens: Some(8),
                    read_cache_hit_rate: Some(0.8),
                    write_cache_tokens: None,
                }),
                latency_ms: Some(1234),
                upstream_request_headers_json: Some(
                    r#"{"authorization":"Bearer provider-key"}"#.to_string(),
                ),
                response_json: Some(r#"{"status":"completed"}"#.to_string()),
                ..RequestLogUpdate::default()
            },
        )
        .unwrap();

        let logs = list_recent(&db_path, 10).unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].id, id);
        assert_eq!(logs[0].status, "completed");
        assert_eq!(logs[0].total_tokens, Some(13));
        assert_eq!(logs[0].read_cache_tokens, Some(8));
        assert_eq!(logs[0].upstream_request_body_bytes, Some(42));
        assert_eq!(logs[0].latency_ms, Some(1234));
        let detail = get_detail(&db_path, id).unwrap().unwrap();
        assert_eq!(detail.summary.id, id);
        assert_eq!(detail.summary.upstream_request_body_bytes, Some(42));
        assert_eq!(
            detail.request_headers_json.as_deref(),
            Some(r#"{"user-agent":"Codex/1.0"}"#)
        );
        assert_eq!(
            detail.upstream_request_headers_json.as_deref(),
            Some(r#"{"authorization":"Bearer provider-key"}"#)
        );
        assert_eq!(
            detail.upstream_request_json.as_deref(),
            Some(r#"{"model":"deepseek-v4-flash","messages":[]}"#)
        );
        assert_eq!(
            detail.response_json.as_deref(),
            Some(r#"{"status":"completed"}"#)
        );
        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn dropping_unfinished_sse_log_stream_marks_cancelled() {
        let db_path = temp_db_path();
        let context = insert_running_test_log(&db_path, "req-cancelled");

        let wrapped =
            ResponsesSseLogStream::new(stream::pending::<Result<Bytes, std::io::Error>>(), context);
        drop(wrapped);

        let detail = get_detail(&db_path, 1).unwrap().unwrap();
        assert_eq!(detail.summary.status, "cancelled");
        assert!(detail.summary.latency_ms.is_some());
        assert!(
            detail
                .summary
                .error_message
                .as_deref()
                .is_some_and(|message| message.contains("client disconnected"))
        );
        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn sse_log_stream_end_before_completed_is_failed() {
        let db_path = temp_db_path();
        let context = insert_running_test_log(&db_path, "req-closed");
        let mut wrapped =
            ResponsesSseLogStream::new(stream::empty::<Result<Bytes, std::io::Error>>(), context);

        assert!(wrapped.next().await.is_none());
        drop(wrapped);

        let detail = get_detail(&db_path, 1).unwrap().unwrap();
        assert_eq!(detail.summary.status, "failed");
        assert_eq!(
            detail.summary.error_message.as_deref(),
            Some("stream closed before response.completed")
        );
        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn failed_sse_log_stream_is_not_overwritten_by_drop() {
        let db_path = temp_db_path();
        let context = insert_running_test_log(&db_path, "req-failed");
        let inner = stream::iter(vec![Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "upstream closed",
        ))]);
        let mut wrapped = ResponsesSseLogStream::new(inner, context);

        let item = wrapped.next().await.unwrap();
        assert!(item.is_err());
        drop(wrapped);

        let detail = get_detail(&db_path, 1).unwrap().unwrap();
        assert_eq!(detail.summary.status, "failed");
        assert_eq!(
            detail.summary.error_message.as_deref(),
            Some("upstream closed")
        );
        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn sqlite_delete_older_than_keeps_recent_logs() {
        let db_path = std::env::temp_dir().join(format!(
            "codex-remote-request-log-test-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        let now = now_ms();
        for (request_id, created_at_ms) in [("old", now - 5_000), ("recent", now)] {
            let record = RequestLogRecord {
                request_id: request_id.to_string(),
                model_id: "deepseek-v4-flash".to_string(),
                stream: true,
                channel: "deepseek".to_string(),
                provider_type: "chat_completions".to_string(),
                status: "completed".to_string(),
                usage: LogUsage::default(),
                cost_usd: None,
                latency_ms: None,
                ttft_ms: None,
                created_at_ms,
                error_message: None,
                request_headers_json: None,
                request_json: None,
                upstream_request_body_bytes: None,
                upstream_request_headers_json: None,
                upstream_request_json: None,
                response_json: None,
            };
            insert_record(&db_path, &record).unwrap();
        }

        let deleted = delete_older_than(&db_path, now - 1_000).unwrap();
        assert_eq!(deleted, 1);
        let logs = list_recent(&db_path, 10).unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].request_id, "recent");
        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn sqlite_delete_all_removes_every_log() {
        let db_path = std::env::temp_dir().join(format!(
            "codex-remote-request-log-test-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        for request_id in ["req-1", "req-2"] {
            let record = RequestLogRecord {
                request_id: request_id.to_string(),
                model_id: "deepseek-v4-flash".to_string(),
                stream: true,
                channel: "deepseek".to_string(),
                provider_type: "chat_completions".to_string(),
                status: "completed".to_string(),
                usage: LogUsage::default(),
                cost_usd: None,
                latency_ms: None,
                ttft_ms: None,
                created_at_ms: now_ms(),
                error_message: None,
                request_headers_json: None,
                request_json: None,
                upstream_request_body_bytes: None,
                upstream_request_headers_json: None,
                upstream_request_json: None,
                response_json: None,
            };
            insert_record(&db_path, &record).unwrap();
        }

        let deleted = delete_all(&db_path).unwrap();
        assert_eq!(deleted, 2);
        assert!(list_recent(&db_path, 10).unwrap().is_empty());
        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn observe_sse_chunk_records_ttft_and_total_latency_separately() {
        let db_path = std::env::temp_dir().join(format!(
            "codex-remote-request-log-test-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        let record = RequestLogRecord {
            request_id: "req-ttft".to_string(),
            model_id: "deepseek-v4-flash".to_string(),
            stream: true,
            channel: "deepseek".to_string(),
            provider_type: "chat_completions".to_string(),
            status: "running".to_string(),
            usage: LogUsage::default(),
            cost_usd: None,
            latency_ms: None,
            ttft_ms: None,
            created_at_ms: now_ms(),
            error_message: None,
            request_headers_json: None,
            request_json: None,
            upstream_request_body_bytes: None,
            upstream_request_headers_json: None,
            upstream_request_json: None,
            response_json: None,
        };

        let store = test_store(&db_path);
        let log_id = store.insert_record(&record).unwrap();
        let context = RequestLogContext {
            store,
            log_id,
            started_at: Instant::now(),
        };
        let mut line_buf = String::new();
        let mut completed = false;
        let mut ttft_recorded = false;

        observe_sse_chunk(
            &Bytes::from_static(
                b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n\n",
            ),
            &mut line_buf,
            &context,
            &mut completed,
            &mut ttft_recorded,
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
        observe_sse_chunk(
            &Bytes::from_static(
                b"data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n",
            ),
            &mut line_buf,
            &context,
            &mut completed,
            &mut ttft_recorded,
        );

        let logs = list_recent(&db_path, 10).unwrap();
        assert_eq!(logs.len(), 1);
        assert!(logs[0].ttft_ms.is_some());
        assert!(logs[0].latency_ms.is_some());
        assert!(logs[0].latency_ms.unwrap() >= logs[0].ttft_ms.unwrap());
        assert!(completed);
        assert!(ttft_recorded);
        let _ = std::fs::remove_file(db_path);
    }
}
