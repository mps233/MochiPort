use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::types::now_ms;

pub const DAEMON_INSTANCE_ENV: &str = "MOCHIPORT_DAEMON_INSTANCE_ID";
const THREADRELAY_DAEMON_INSTANCE_ENV: &str = "THREADRELAY_DAEMON_INSTANCE_ID";
const LEGACY_DAEMON_INSTANCE_ENV: &str = "CODEXHUB_DAEMON_INSTANCE_ID";
// Keep the GUI PID variable stable because relaunch helpers may outlive the
// executable that spawned them during an in-place upgrade.
pub const CODEXHUB_GUI_PID_ENV: &str = "CODEXHUB_GUI_PID";
// Keep the management protocol service value stable across the product rename.
// The executable and user-facing product are MochiPort; existing clients still
// identify the local daemon through the `threadrelay` service contract.
pub const DAEMON_SERVICE_NAME: &str = "threadrelay";
const LEGACY_DAEMON_SERVICE_NAME: &str = "codexhub";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonIdentity {
    pub service: String,
    pub pid: u32,
    pub instance_id: String,
    pub started_at_ms: u64,
}

impl DaemonIdentity {
    pub fn new() -> Self {
        let instance_id = std::env::var(DAEMON_INSTANCE_ENV)
            .or_else(|_| std::env::var(THREADRELAY_DAEMON_INSTANCE_ENV))
            .or_else(|_| std::env::var(LEGACY_DAEMON_INSTANCE_ENV))
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        Self {
            service: DAEMON_SERVICE_NAME.to_string(),
            pid: std::process::id(),
            instance_id,
            started_at_ms: now_ms().min(u64::MAX as u128) as u64,
        }
    }

    pub fn is_mochiport_compatible(&self) -> bool {
        matches!(
            self.service.as_str(),
            DAEMON_SERVICE_NAME | LEGACY_DAEMON_SERVICE_NAME
        ) && self.pid > 0
            && !self.instance_id.trim().is_empty()
    }

    #[allow(dead_code)]
    pub fn is_codexhub(&self) -> bool {
        self.is_mochiport_compatible()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonMetadata {
    #[serde(flatten)]
    pub identity: DaemonIdentity,
    pub executable: String,
    pub config_path: String,
}

pub struct DaemonInstanceLock {
    files: Vec<File>,
    metadata_path: PathBuf,
}

impl DaemonInstanceLock {
    pub fn acquire(config_path: &Path, identity: &DaemonIdentity) -> Result<Self> {
        let path = daemon_lock_path(config_path);
        if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).with_context(|| {
                format!(
                    "failed to create daemon lock directory `{}`",
                    parent.display()
                )
            })?;
        }
        // Hold the legacy lock as well as the new lock. This prevents an older
        // Hold both legacy locks so an older build cannot start beside MochiPort.
        let mut files = Vec::with_capacity(3);
        for lock_path in [
            legacy_daemon_lock_path(config_path),
            threadrelay_daemon_lock_path(config_path),
            path.clone(),
        ] {
            let file = OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(&lock_path)
                .with_context(|| format!("failed to open daemon lock `{}`", lock_path.display()))?;
            if let Err(err) = FileExt::try_lock_exclusive(&file) {
                let owner = read_daemon_metadata(config_path)
                    .map(|metadata| {
                        format!(
                            "pid={} instance_id={}",
                            metadata.identity.pid, metadata.identity.instance_id
                        )
                    })
                    .unwrap_or_else(|| "owner=unknown".to_string());
                return Err(anyhow!(
                    "another MochiPort, ThreadRelay, or CodexHub daemon holds `{}` ({owner}): {err}",
                    lock_path.display()
                ));
            }
            files.push(file);
        }

        let metadata = DaemonMetadata {
            identity: identity.clone(),
            executable: std::env::current_exe()
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
            config_path: config_path.display().to_string(),
        };
        let metadata_path = daemon_metadata_path(config_path);
        let mut metadata_file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&metadata_path)
            .with_context(|| {
                format!(
                    "failed to open daemon metadata `{}`",
                    metadata_path.display()
                )
            })?;
        serde_json::to_writer(&mut metadata_file, &metadata)?;
        metadata_file.write_all(b"\n")?;
        metadata_file.sync_data()?;
        Ok(Self {
            files,
            metadata_path,
        })
    }
}

impl Drop for DaemonInstanceLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.metadata_path);
        for file in &self.files {
            let _ = FileExt::unlock(file);
        }
    }
}

pub fn daemon_lock_path(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("mochiport-daemon.lock")
}

pub fn daemon_metadata_path(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("mochiport-daemon.json")
}

pub fn read_daemon_metadata(config_path: &Path) -> Option<DaemonMetadata> {
    daemon_file_pairs(config_path)
        .into_iter()
        .find_map(|(lock_path, metadata_path)| {
            let bytes = std::fs::read(metadata_path)
                .or_else(|_| std::fs::read(lock_path))
                .ok()?;
            serde_json::from_slice(&bytes).ok()
        })
}

pub fn read_active_daemon_metadata(config_path: &Path) -> Option<DaemonMetadata> {
    for (lock_path, metadata_path) in daemon_file_pairs(config_path) {
        let Ok(bytes) = std::fs::read(metadata_path).or_else(|_| std::fs::read(&lock_path)) else {
            continue;
        };
        let Ok(metadata) = serde_json::from_slice::<DaemonMetadata>(&bytes) else {
            continue;
        };
        let Ok(lock_file) = OpenOptions::new().read(true).write(true).open(lock_path) else {
            continue;
        };
        match FileExt::try_lock_exclusive(&lock_file) {
            Ok(()) => {
                let _ = FileExt::unlock(&lock_file);
            }
            Err(_) => return Some(metadata),
        }
    }
    None
}

fn legacy_daemon_lock_path(config_path: &Path) -> PathBuf {
    config_directory(config_path).join("codexhub-daemon.lock")
}

fn threadrelay_daemon_lock_path(config_path: &Path) -> PathBuf {
    config_directory(config_path).join("threadrelay-daemon.lock")
}

fn threadrelay_daemon_metadata_path(config_path: &Path) -> PathBuf {
    config_directory(config_path).join("threadrelay-daemon.json")
}

fn legacy_daemon_metadata_path(config_path: &Path) -> PathBuf {
    config_directory(config_path).join("codexhub-daemon.json")
}

fn config_directory(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn daemon_file_pairs(config_path: &Path) -> [(PathBuf, PathBuf); 3] {
    [
        (
            daemon_lock_path(config_path),
            daemon_metadata_path(config_path),
        ),
        (
            threadrelay_daemon_lock_path(config_path),
            threadrelay_daemon_metadata_path(config_path),
        ),
        (
            legacy_daemon_lock_path(config_path),
            legacy_daemon_metadata_path(config_path),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_lock_path_follows_config_directory() {
        let config = PathBuf::from("root").join("config.toml");
        assert_eq!(
            daemon_lock_path(&config),
            PathBuf::from("root").join("mochiport-daemon.lock")
        );
        assert_eq!(
            daemon_metadata_path(&config),
            PathBuf::from("root").join("mochiport-daemon.json")
        );
        assert_eq!(
            legacy_daemon_lock_path(&config),
            PathBuf::from("root").join("codexhub-daemon.lock")
        );
    }

    #[test]
    fn daemon_identity_accepts_current_and_legacy_services() {
        let mut identity = DaemonIdentity::new();
        assert!(identity.is_mochiport_compatible());
        identity.service = DAEMON_SERVICE_NAME.to_string();
        assert!(identity.is_mochiport_compatible());
        assert!(identity.is_codexhub());
        identity.service = LEGACY_DAEMON_SERVICE_NAME.to_string();
        assert!(identity.is_mochiport_compatible());
        identity.service = "other".to_string();
        assert!(!identity.is_mochiport_compatible());
    }

    #[test]
    fn daemon_metadata_is_readable_while_lock_is_held() {
        let root = std::env::temp_dir().join(format!("codexhub-lock-test-{}", Uuid::new_v4()));
        let config_path = root.join("config.toml");
        std::fs::create_dir_all(&root).expect("create temp directory");
        let identity = DaemonIdentity::new();

        let daemon_lock =
            DaemonInstanceLock::acquire(&config_path, &identity).expect("acquire daemon lock");
        let metadata_bytes =
            std::fs::read(daemon_metadata_path(&config_path)).expect("read daemon metadata file");
        let metadata: DaemonMetadata =
            serde_json::from_slice(&metadata_bytes).expect("parse daemon metadata");
        assert_eq!(metadata.identity.pid, identity.pid);
        assert_eq!(metadata.identity.instance_id, identity.instance_id);
        let active_metadata =
            read_active_daemon_metadata(&config_path).expect("read active daemon metadata");
        assert_eq!(active_metadata.identity.instance_id, identity.instance_id);

        let second_identity = DaemonIdentity::new();
        let error = DaemonInstanceLock::acquire(&config_path, &second_identity)
            .err()
            .expect("second lock should fail")
            .to_string();
        assert!(error.contains(&format!("pid={}", identity.pid)));
        assert!(error.contains(&format!("instance_id={}", identity.instance_id)));

        drop(daemon_lock);
        assert!(!daemon_metadata_path(&config_path).exists());

        std::fs::write(daemon_metadata_path(&config_path), metadata_bytes)
            .expect("write stale daemon metadata");
        assert!(read_daemon_metadata(&config_path).is_some());
        assert!(read_active_daemon_metadata(&config_path).is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn active_legacy_daemon_metadata_remains_visible() {
        let root =
            std::env::temp_dir().join(format!("threadrelay-legacy-lock-test-{}", Uuid::new_v4()));
        let config_path = root.join("config.toml");
        std::fs::create_dir_all(&root).expect("create temp directory");
        let identity = DaemonIdentity {
            service: LEGACY_DAEMON_SERVICE_NAME.to_string(),
            pid: std::process::id(),
            instance_id: Uuid::new_v4().to_string(),
            started_at_ms: 1,
        };
        let metadata = DaemonMetadata {
            identity: identity.clone(),
            executable: "codexhub".to_string(),
            config_path: config_path.display().to_string(),
        };
        let legacy_lock_path = legacy_daemon_lock_path(&config_path);
        let legacy_lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&legacy_lock_path)
            .expect("open legacy lock");
        FileExt::try_lock_exclusive(&legacy_lock).expect("lock legacy daemon file");
        std::fs::write(
            legacy_daemon_metadata_path(&config_path),
            serde_json::to_vec(&metadata).unwrap(),
        )
        .expect("write legacy metadata");

        let active = read_active_daemon_metadata(&config_path).expect("find legacy daemon");
        assert_eq!(active.identity.instance_id, identity.instance_id);
        assert!(DaemonInstanceLock::acquire(&config_path, &DaemonIdentity::new()).is_err());

        FileExt::unlock(&legacy_lock).unwrap();
        drop(legacy_lock);
        let _ = std::fs::remove_dir_all(root);
    }
}
