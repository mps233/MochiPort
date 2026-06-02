use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::Context;
use once_cell::sync::OnceCell;

static CHAIN_LOG: OnceCell<ChainLog> = OnceCell::new();

struct ChainLog {
    file: Mutex<File>,
    diagnostic: bool,
}

pub fn init(
    path: &Path,
    diagnostic: bool,
    max_bytes: u64,
    retention_days: u64,
) -> anyhow::Result<()> {
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
        "\n===== codex-remote start {} =====",
        timestamp_secs()
    );
    let _ = CHAIN_LOG.set(ChainLog {
        file: Mutex::new(file),
        diagnostic,
    });
    Ok(())
}

pub fn write_line(line: impl AsRef<str>) {
    let line = line.as_ref();
    if !should_write_default(line) {
        return;
    }
    write_line_inner(line, should_flush(line));
}

pub fn write_diagnostic_line(line: impl AsRef<str>) {
    let Some(log) = CHAIN_LOG.get() else {
        return;
    };
    if !log.diagnostic {
        return;
    };
    write_line_inner(line.as_ref(), false);
}

fn write_line_inner(line: &str, flush: bool) {
    let Some(log) = CHAIN_LOG.get() else {
        return;
    };
    let Ok(mut file) = log.file.lock() else {
        return;
    };
    let _ = writeln!(file, "{line}");
    if flush {
        let _ = file.flush();
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
        if path == active_path || !is_codex_remote_log_path(&path) {
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

fn is_codex_remote_log_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|name| name.starts_with("codex-remote") && name.contains(".log"))
}

fn rotated_path(path: &Path) -> PathBuf {
    let mut rotated = path.to_path_buf();
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("codex-remote-chain.log");
    rotated.set_file_name(format!("{file_name}.1"));
    rotated
}

fn timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
