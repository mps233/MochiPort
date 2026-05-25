use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::Path,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Context;
use once_cell::sync::OnceCell;

static CHAIN_LOG_FILE: OnceCell<Mutex<File>> = OnceCell::new();

pub fn init(path: &Path) -> anyhow::Result<()> {
    let log_dir = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("invalid log path {}", path.display()))?;
    std::fs::create_dir_all(log_dir)
        .with_context(|| format!("failed to create log directory {}", log_dir.display()))?;
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
    let _ = CHAIN_LOG_FILE.set(Mutex::new(file));
    Ok(())
}

pub fn write_line(line: impl AsRef<str>) {
    let Some(file) = CHAIN_LOG_FILE.get() else {
        return;
    };
    let Ok(mut file) = file.lock() else {
        return;
    };
    let _ = writeln!(file, "{}", line.as_ref());
    let _ = file.flush();
}

fn timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
