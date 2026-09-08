use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        mpsc::{SyncSender, sync_channel},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::Context;
static CHAIN_LOG: OnceLock<ChainLog> = OnceLock::new();

struct ChainLog {
    inner: Arc<Mutex<ChainLogInner>>,
    write_tx: SyncSender<ChainLogWrite>,
    diagnostic: bool,
    max_bytes: u64,
}

struct ChainLogWrite {
    line: String,
    flush: bool,
}

struct ChainLogInner {
    file: Option<File>,
    path: PathBuf,
    written_bytes: u64,
}

pub fn init(
    path: &Path,
    diagnostic: bool,
    max_bytes: u64,
    retention_days: u64,
) -> anyhow::Result<()> {
    if CHAIN_LOG.get().is_some() {
        return Ok(());
    }
    let log_dir = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("invalid log path {}", path.display()))?;
    std::fs::create_dir_all(log_dir)
        .with_context(|| format!("failed to create log directory {}", log_dir.display()))?;
    cleanup_old_logs(log_dir, path, retention_days)?;
    rotate_if_large(path, max_bytes)?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("failed to open chain log {}", path.display()))?;
    let _ = writeln!(
        file,
        "\n===== mochiport start ts_ms={} =====",
        timestamp_ms()
    );
    let written_bytes = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    let inner = Arc::new(Mutex::new(ChainLogInner {
        file: Some(file),
        path: path.to_path_buf(),
        written_bytes,
    }));
    let (write_tx, write_rx) = sync_channel::<ChainLogWrite>(4096);
    let writer_inner = inner.clone();
    std::thread::Builder::new()
        .name("mochiport-chain-log-writer".to_string())
        .spawn(move || {
            while let Ok(command) = write_rx.recv() {
                write_line_sync(&writer_inner, max_bytes, &command.line, command.flush);
            }
        })
        .context("failed to start chain log writer thread")?;

    if let Err(chain_log) = CHAIN_LOG.set(ChainLog {
        inner,
        write_tx,
        diagnostic,
        max_bytes,
    }) {
        // Another initializer won the race. Dropping the sender lets the
        // newly started writer exit cleanly after its receiver is drained.
        drop(chain_log.write_tx);
    }
    Ok(())
}

pub fn write_line(line: impl AsRef<str>) {
    let line = line.as_ref();
    if !should_write_default(line) {
        return;
    }
    write_line_inner(line, should_flush(line));
}

/// Write a `tracing_subscriber` formatted line.
///
/// Unlike [`write_line`], this bypasses the error/warn keyword filter: the
/// tracing subscriber already applied its own `EnvFilter`, so re-filtering
/// would silently drop the lines the operator asked for. The line still goes
/// through the same rotation and retention budget as every other chain line.
pub fn write_tracing_line(line: &str) {
    write_line_inner(line, false);
}

/// `tracing_subscriber` sink that feeds the rotating chain log.
///
/// The daemon used to let launchd capture stdout into a file that no rotation
/// policy could reach, so `logging.maxMb` never bounded it. Formatting into the
/// chain log instead keeps a single, capped log file.
#[derive(Debug, Clone, Copy, Default)]
pub struct ChainLogMakeWriter;

pub struct ChainLogWriter {
    buffer: String,
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for ChainLogMakeWriter {
    type Writer = ChainLogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        ChainLogWriter {
            buffer: String::new(),
        }
    }
}

impl std::io::Write for ChainLogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.buffer.push_str(&String::from_utf8_lossy(buf));
        while let Some(index) = self.buffer.find('\n') {
            let line = self.buffer[..index].to_string();
            self.buffer.drain(..=index);
            if !line.trim().is_empty() {
                write_tracing_line(&line);
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if !self.buffer.trim().is_empty() {
            let line = std::mem::take(&mut self.buffer);
            write_tracing_line(line.trim_end());
        }
        Ok(())
    }
}

/// Cap the log file that launchd hands the daemon as stdout/stderr.
///
/// `DaemonLauncher.swift` points `StandardOutPath`/`StandardErrorPath` at
/// `mochiport-daemon-launchd.log`. Because launchd owns that file handle, the
/// rotation applied to the chain log never touched it and the file grew
/// without bound (observed: 1.5 GB in 13 days) while the operator's
/// `logging.maxMb` budget silently applied only to the chain log.
///
/// The daemon cannot rename the file — launchd keeps writing to the inode it
/// opened — so it truncates the descriptor in place instead. Both the
/// truncate and the offset reset must happen on the inherited descriptors
/// themselves: when the descriptor was opened without `O_APPEND` (launchd
/// does not set it), its file offset still points past the old end, and the
/// next write would recreate the full length as a NUL hole.
///
/// Returns `true` when the capture was truncated.
pub fn cap_launchd_capture(max_bytes: u64) -> bool {
    if max_bytes == 0 {
        return false;
    }
    let Some(path) = launchd_capture_path() else {
        return false;
    };
    let Ok(metadata) = std::fs::metadata(&path) else {
        return false;
    };
    if metadata.len() < max_bytes {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::MetadataExt;

        // stdout and stderr usually share one inode; dedupe by (device, inode)
        // so the same file is not truncated twice.
        let mut truncated: Vec<(u64, u64)> = Vec::new();
        for fd in [std::io::stdout().as_raw_fd(), std::io::stderr().as_raw_fd()] {
            let Ok(stat) = std::fs::metadata(format!("/dev/fd/{fd}")) else {
                continue;
            };
            let identity = (stat.dev(), stat.ino());
            if truncated.contains(&identity) {
                continue;
            }
            // SAFETY: `fd` is a live descriptor inherited by this process and
            // ftruncate/lseek only mutate that descriptor's offset and the
            // inode size.
            if unsafe { ftruncate(fd, 0) } != 0 {
                continue;
            }
            // SAFETY: same descriptor; rewinding its offset prevents the NUL
            // hole described above.
            let _ = unsafe { lseek(fd, 0, SEEK_SET) };
            truncated.push(identity);
        }
        if truncated.is_empty() {
            return false;
        }
    }
    #[cfg(not(unix))]
    {
        let Ok(file) = OpenOptions::new().write(true).open(&path) else {
            return false;
        };
        if file.set_len(0).is_err() {
            return false;
        }
    }
    // Keep one note so an operator who opens the file knows why it is short.
    if let Ok(mut file) = OpenOptions::new().append(true).open(&path) {
        let _ = writeln!(
            file,
            "[ts_ms={}] launchd capture truncated; previous content exceeded {} bytes",
            timestamp_ms(),
            max_bytes
        );
    }
    true
}

/// Cap the launchd capture periodically while the daemon keeps running.
///
/// Startup truncation alone is not enough: a daemon that stays up for weeks
/// would grow the capture back past the operator's budget, which is exactly
/// how the file reached 1.5 GB. This is meant to be driven by a periodic
/// task; it never truncates below the budget and stays silent unless it
/// actually trims, so it cannot itself flood the log.
pub fn enforce_launchd_capture_budget(max_bytes: u64) {
    if max_bytes == 0 {
        return;
    }
    if cap_launchd_capture(max_bytes) {
        // Record the trim in the rotating chain log rather than stdout, so
        // the note cannot feed the very file it just trimmed.
        write_line_inner(
            &format!(
                "[chain_log] event=launchd_capture_trimmed max_bytes={max_bytes} \
                 (launchd stdout capture exceeded the configured logging budget)"
            ),
            true,
        );
    }
}

#[cfg(unix)]
const SEEK_SET: std::os::raw::c_int = 0;

#[cfg(unix)]
unsafe extern "C" {
    fn ftruncate(fd: std::os::raw::c_int, length: std::os::raw::c_long) -> std::os::raw::c_int;
    fn lseek(
        fd: std::os::raw::c_int,
        offset: std::os::raw::c_long,
        whence: std::os::raw::c_int,
    ) -> std::os::raw::c_long;
}

/// Resolve the path launchd captured as the daemon's stdout.
///
/// `/dev/fd/1` cannot be `readlink`-ed on macOS, so ask the kernel for the
/// path behind the descriptor (`F_GETPATH`). Returns `None` for terminals,
/// pipes, or any platform without the call, in which case there is nothing to
/// truncate and the caller simply leaves stdout alone.
fn launchd_capture_path() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;

        let stdout = std::io::stdout();
        let fd = stdout.as_raw_fd();
        let mut buffer = vec![0i8; MAXPATHLEN + 1];
        // SAFETY: `buffer` is a valid, writable allocation of MAXPATHLEN + 1
        // bytes, which is what F_GETPATH requires, and `fd` is a live
        // descriptor owned by the process for the duration of the call.
        let result = fcntl_getpath(fd, buffer.as_mut_ptr());
        if result < 0 {
            return None;
        }
        let bytes: Vec<u8> = buffer
            .iter()
            .take_while(|value| **value != 0)
            .map(|value| *value as u8)
            .collect();
        let path = PathBuf::from(String::from_utf8(bytes).ok()?);
        path.is_file().then_some(path)
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

#[cfg(target_os = "macos")]
const MAXPATHLEN: usize = 1024;

/// `F_GETPATH` from `<sys/fcntl.h>`; `libc` does not export it.
#[cfg(target_os = "macos")]
const F_GETPATH: std::os::raw::c_int = 50;

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn fcntl(fd: std::os::raw::c_int, cmd: std::os::raw::c_int, ...) -> std::os::raw::c_int;
}

#[cfg(target_os = "macos")]
fn fcntl_getpath(
    fd: std::os::raw::c_int,
    buffer: *mut std::os::raw::c_char,
) -> std::os::raw::c_int {
    // SAFETY: forwarded from the caller, which guarantees `buffer` is a
    // writable MAXPATHLEN + 1 allocation and `fd` is live.
    unsafe { fcntl(fd, F_GETPATH, buffer) }
}

pub fn write_diagnostic_lazy(build: impl FnOnce() -> String) {
    if !diagnostic_enabled() {
        return;
    }
    let line = build();
    write_line_inner(&line, false);
}

pub fn diagnostic_enabled() -> bool {
    CHAIN_LOG.get().is_some_and(|log| log.diagnostic)
}

fn write_line_inner(line: &str, flush: bool) {
    let Some(log) = CHAIN_LOG.get() else {
        return;
    };
    let command = ChainLogWrite {
        line: line.to_string(),
        flush,
    };
    if let Err(error) = log.write_tx.send(command) {
        write_line_sync(&log.inner, log.max_bytes, &error.0.line, error.0.flush);
    }
}

fn write_line_sync(inner: &Mutex<ChainLogInner>, max_bytes: u64, line: &str, flush: bool) {
    let Ok(mut inner) = inner.lock() else {
        return;
    };
    if max_bytes > 0 && inner.written_bytes >= max_bytes {
        rotate_open_log(&mut inner);
    }
    let wrote = if let Some(file) = inner.file.as_mut() {
        let _ = writeln!(file, "[ts_ms={}] {line}", timestamp_ms());
        if flush {
            let _ = file.flush();
        }
        true
    } else {
        false
    };
    if wrote {
        inner.written_bytes = inner
            .written_bytes
            .saturating_add(line.len() as u64)
            .saturating_add(24);
    }
}

fn rotate_open_log(inner: &mut ChainLogInner) {
    if let Some(mut file) = inner.file.take() {
        let _ = file.flush();
    }
    let rotated = rotated_path(&inner.path);
    let _ = std::fs::remove_file(&rotated);
    if inner.path.exists() {
        let _ = std::fs::rename(&inner.path, &rotated);
    }
    match OpenOptions::new()
        .create(true)
        .append(true)
        .open(&inner.path)
    {
        Ok(file) => {
            inner.file = Some(file);
            inner.written_bytes = 0;
        }
        Err(_) => {
            inner.file = None;
            inner.written_bytes = 0;
        }
    }
}

fn should_write_default(line: &str) -> bool {
    if CHAIN_LOG.get().is_some_and(|log| log.diagnostic) {
        return true;
    }
    let lower = line.to_ascii_lowercase();
    lower.contains("level=error")
        || lower.contains("level=warn")
        || lower.contains(" error")
        || lower.contains("err=")
        || lower.contains("failed")
        || lower.contains("timeout")
}

fn should_flush(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.contains("level=error")
        || lower.contains("level=warn")
        || lower.contains("err=")
        || lower.contains("failed")
        || lower.contains("timeout")
}

fn rotate_if_large(path: &Path, max_bytes: u64) -> anyhow::Result<()> {
    if max_bytes == 0 || !path.exists() {
        return Ok(());
    }
    let len = std::fs::metadata(path)
        .with_context(|| format!("failed to stat chain log {}", path.display()))?
        .len();
    if len < max_bytes {
        return Ok(());
    }
    let rotated = rotated_path(path);
    let _ = std::fs::remove_file(&rotated);
    std::fs::rename(path, &rotated).with_context(|| {
        format!(
            "failed to rotate chain log {} to {}",
            path.display(),
            rotated.display()
        )
    })?;
    Ok(())
}

fn cleanup_old_logs(log_dir: &Path, active_path: &Path, retention_days: u64) -> anyhow::Result<()> {
    if retention_days == 0 {
        return Ok(());
    }
    let Ok(entries) = std::fs::read_dir(log_dir) else {
        return Ok(());
    };
    let cutoff = SystemTime::now()
        .checked_sub(Duration::from_secs(
            retention_days.saturating_mul(24 * 60 * 60),
        ))
        .unwrap_or(UNIX_EPOCH);
    for entry in entries.flatten() {
        let path = entry.path();
        if path == active_path || !is_mochiport_log_path(&path) {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let modified = metadata
            .modified()
            .or_else(|_| metadata.created())
            .unwrap_or(SystemTime::now());
        if modified < cutoff {
            let _ = std::fs::remove_file(path);
        }
    }
    Ok(())
}

fn is_mochiport_log_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|name| {
            (name.starts_with("mochiport")
                || name.starts_with("threadrelay")
                || name.starts_with("codexhub"))
                && name.contains(".log")
        })
}

fn rotated_path(path: &Path) -> PathBuf {
    let mut rotated = path.to_path_buf();
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("mochiport-chain.log");
    rotated.set_file_name(format!("{file_name}.1"));
    rotated
}

fn timestamp_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotate_open_log_replaces_active_file() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("mochiport-chain-log-test-{unique}"));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mochiport-chain.log");
        std::fs::write(&path, "old\n").unwrap();
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .unwrap();
        let mut inner = ChainLogInner {
            file: Some(file),
            path: path.clone(),
            written_bytes: 4,
        };

        rotate_open_log(&mut inner);
        writeln!(inner.file.as_mut().unwrap(), "new").unwrap();
        drop(inner);

        assert!(rotated_path(&path).exists());
        assert_eq!(
            std::fs::read_to_string(rotated_path(&path)).unwrap(),
            "old\n"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new\n");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn tracing_writer_splits_lines_and_keeps_them_past_the_keyword_filter() {
        // `mochiport::http` request lines carry no error keyword, so the
        // default chain-log filter would drop them; the tracing sink must not.
        let mut writer = ChainLogWriter {
            buffer: String::new(),
        };
        std::io::Write::write_all(
            &mut writer,
            b"2026-01-01T00:00:00Z  INFO mochiport::http: http request status=200\npartial",
        )
        .unwrap();
        assert_eq!(
            writer.buffer, "partial",
            "a complete line is flushed, the trailing fragment is buffered"
        );
        std::io::Write::flush(&mut writer).unwrap();
        assert!(writer.buffer.is_empty());
    }

    #[test]
    fn cap_launchd_capture_ignores_zero_budget() {
        // A zero budget means "no cap configured". This must return before it
        // touches fd 1/2, otherwise the test runner's own stdout would be
        // truncated.
        assert!(!cap_launchd_capture(0));
    }

    #[test]
    fn enforce_launchd_capture_budget_is_inert_without_a_budget() {
        // Same guarantee as above for the periodic entry point: a zero budget
        // must never trim the test runner's stdout.
        enforce_launchd_capture_budget(0);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn launchd_capture_path_only_resolves_real_files() {
        // Under a pipe or terminal there is nothing to truncate. The test
        // runner may be either; both must yield `None` or a regular file.
        if let Some(path) = launchd_capture_path() {
            assert!(path.is_file(), "only a regular file can be capped");
        }
    }
}
