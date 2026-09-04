use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{storage_migration, types::now_ms};

pub const DAEMON_INSTANCE_ENV: &str = "MOCHIPORT_DAEMON_INSTANCE_ID";
pub const DAEMON_SERVICE_NAME: &str = "mochiport";

const CURRENT_LOCK_FILE_NAME: &str = "mochiport-daemon.lock";
const CURRENT_METADATA_FILE_NAME: &str = "mochiport-daemon.json";

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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonMetadata {
    #[serde(flatten)]
    pub identity: DaemonIdentity,
    pub executable: String,
    pub config_path: String,
}

#[derive(Debug, Clone)]
pub struct ActiveLegacyDaemon {
    pub lock_path: PathBuf,
    pub metadata: Option<DaemonMetadata>,
}

#[derive(Debug)]
pub struct DaemonInstanceLock {
    file: File,
    metadata_path: PathBuf,
}

impl DaemonInstanceLock {
    /// Acquires the singleton lock under the current MochiPort home, regardless
    /// of a user-supplied config path. Historical locations are inspected
    /// read-only only to avoid a second daemon during migration.
    pub fn acquire(config_path: &Path, identity: &DaemonIdentity) -> Result<Self> {
        let storage_home = storage_migration::current_storage_home();
        let legacy_homes = storage_migration::legacy_standard_storage_homes();
        Self::acquire_at(config_path, identity, &storage_home, &legacy_homes)
    }

    fn acquire_at(
        config_path: &Path,
        identity: &DaemonIdentity,
        storage_home: &Path,
        legacy_homes: &[PathBuf],
    ) -> Result<Self> {
        std::fs::create_dir_all(storage_home).with_context(|| {
            format!(
                "failed to create MochiPort daemon directory `{}`",
                storage_home.display()
            )
        })?;

        let lock_path = daemon_lock_path(storage_home);
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .with_context(|| format!("failed to open daemon lock `{}`", lock_path.display()))?;
        if let Err(error) = FileExt::try_lock_exclusive(&file) {
            let owner = read_daemon_metadata_at(storage_home)
                .map(|metadata| {
                    format!(
                        "pid={} instance_id={}",
                        metadata.identity.pid, metadata.identity.instance_id
                    )
                })
                .unwrap_or_else(|| "owner=unknown".to_string());
            return Err(anyhow!(
                "another MochiPort daemon holds `{}` ({owner}): {error}",
                lock_path.display()
            ));
        }

        for legacy_home in legacy_homes {
            if same_path(legacy_home, storage_home) {
                continue;
            }
            if let Some(active) = active_legacy_daemon_at(legacy_home)? {
                let owner = active
                    .metadata
                    .as_ref()
                    .map(|metadata| {
                        format!(
                            "pid={} instance_id={}",
                            metadata.identity.pid, metadata.identity.instance_id
                        )
                    })
                    .unwrap_or_else(|| "owner=unknown".to_string());
                let _ = FileExt::unlock(&file);
                return Err(anyhow!(
                    "legacy daemon holds `{}` ({owner}); run `mochiport migrate-storage` after it is stopped",
                    active.lock_path.display()
                ));
            }
        }

        let metadata = DaemonMetadata {
            identity: identity.clone(),
            executable: std::env::current_exe()
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
            config_path: config_path.display().to_string(),
        };
        let metadata_path = daemon_metadata_path(storage_home);
        write_daemon_metadata(&metadata_path, &metadata)?;
        Ok(Self {
            file,
            metadata_path,
        })
    }
}

impl Drop for DaemonInstanceLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.metadata_path);
        let _ = FileExt::unlock(&self.file);
    }
}

pub fn daemon_lock_path(storage_home: &Path) -> PathBuf {
    storage_home.join(CURRENT_LOCK_FILE_NAME)
}

pub fn daemon_metadata_path(storage_home: &Path) -> PathBuf {
    storage_home.join(CURRENT_METADATA_FILE_NAME)
}

pub fn current_daemon_is_active_at(storage_home: &Path) -> Result<bool> {
    lock_is_held(&daemon_lock_path(storage_home))
}

/// Performs a read-only inspection of historical lock files. It never creates,
/// truncates, or acquires an exclusive legacy lock.
pub fn active_legacy_daemon_at(storage_home: &Path) -> Result<Option<ActiveLegacyDaemon>> {
    for (lock_path, metadata_path) in legacy_daemon_file_pairs(storage_home) {
        if !lock_path.exists() {
            continue;
        }
        if lock_is_held(&lock_path)? {
            let metadata = std::fs::read(metadata_path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<DaemonMetadata>(&bytes).ok());
            return Ok(Some(ActiveLegacyDaemon {
                lock_path,
                metadata,
            }));
        }
    }
    Ok(None)
}

fn read_daemon_metadata_at(storage_home: &Path) -> Option<DaemonMetadata> {
    std::fs::read(daemon_metadata_path(storage_home))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

#[cfg(test)]
fn read_active_daemon_metadata_at(storage_home: &Path) -> Option<DaemonMetadata> {
    current_daemon_is_active_at(storage_home)
        .ok()
        .filter(|active| *active)
        .and_then(|_| read_daemon_metadata_at(storage_home))
}

fn legacy_daemon_file_pairs(storage_home: &Path) -> [(PathBuf, PathBuf); 3] {
    [
        (
            storage_home.join("threadrelay-daemon.lock"),
            storage_home.join("threadrelay-daemon.json"),
        ),
        (
            storage_home.join("codexhub-daemon.lock"),
            storage_home.join("codexhub-daemon.json"),
        ),
        (
            storage_home.join(CURRENT_LOCK_FILE_NAME),
            storage_home.join(CURRENT_METADATA_FILE_NAME),
        ),
    ]
}

fn lock_is_held(lock_path: &Path) -> Result<bool> {
    if !lock_path.exists() {
        return Ok(false);
    }
    let file = OpenOptions::new()
        .read(true)
        .open(lock_path)
        .with_context(|| format!("failed to inspect daemon lock `{}`", lock_path.display()))?;
    match FileExt::try_lock_shared(&file) {
        Ok(()) => {
            let _ = FileExt::unlock(&file);
            Ok(false)
        }
        Err(_) => Ok(true),
    }
}

fn write_daemon_metadata(path: &Path, metadata: &DaemonMetadata) -> Result<()> {
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| anyhow!("daemon metadata has no parent"))?;
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)
        .with_context(|| format!("failed to open daemon metadata `{}`", path.display()))?;
    serde_json::to_writer(&mut file, metadata)?;
    file.write_all(b"\n")?;
    file.sync_data()?;
    #[cfg(unix)]
    {
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .with_context(|| format!("failed to sync daemon directory `{}`", parent.display()))?;
    }
    #[cfg(not(unix))]
    let _ = parent;
    Ok(())
}

fn same_path(left: &Path, right: &Path) -> bool {
    let left = std::fs::canonicalize(left).unwrap_or_else(|_| left.to_path_buf());
    let right = std::fs::canonicalize(right).unwrap_or_else(|_| right.to_path_buf());
    left == right
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_identity_uses_only_mochiport_service() {
        let identity = DaemonIdentity::new();
        assert_eq!(identity.service, "mochiport");
        assert!(identity.pid > 0);
        assert!(!identity.instance_id.trim().is_empty());
    }

    #[test]
    fn daemon_lock_is_fixed_to_mochiport_storage_home() {
        let home = PathBuf::from("root").join("MochiPort");
        assert_eq!(
            daemon_lock_path(&home),
            PathBuf::from("root").join("MochiPort/mochiport-daemon.lock")
        );
        assert_eq!(
            daemon_metadata_path(&home),
            PathBuf::from("root").join("MochiPort/mochiport-daemon.json")
        );
    }

    #[test]
    fn daemon_metadata_is_readable_while_current_lock_is_held() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let storage_home = temporary.path().join("MochiPort");
        let config_path = temporary.path().join("custom-config.toml");
        let identity = DaemonIdentity::new();

        let daemon_lock =
            DaemonInstanceLock::acquire_at(&config_path, &identity, &storage_home, &[])
                .expect("acquire daemon lock");
        let metadata_bytes =
            std::fs::read(daemon_metadata_path(&storage_home)).expect("read daemon metadata");
        let metadata: DaemonMetadata =
            serde_json::from_slice(&metadata_bytes).expect("parse daemon metadata");
        assert_eq!(metadata.identity.pid, identity.pid);
        assert_eq!(metadata.config_path, config_path.display().to_string());
        assert_eq!(
            read_active_daemon_metadata_at(&storage_home)
                .expect("read active daemon metadata")
                .identity
                .instance_id,
            identity.instance_id
        );

        let error = DaemonInstanceLock::acquire_at(
            &config_path,
            &DaemonIdentity::new(),
            &storage_home,
            &[],
        )
        .expect_err("second lock must fail")
        .to_string();
        assert!(error.contains("another MochiPort daemon"));

        drop(daemon_lock);
        assert!(!daemon_metadata_path(&storage_home).exists());
    }

    #[test]
    fn active_legacy_lock_blocks_new_daemon_without_creating_legacy_locks() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let storage_home = temporary.path().join("MochiPort");
        let legacy_home = temporary.path().join("ThreadRelay");
        std::fs::create_dir_all(&legacy_home).expect("create legacy directory");
        let legacy_lock_path = legacy_home.join("threadrelay-daemon.lock");
        let legacy_lock = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&legacy_lock_path)
            .expect("open legacy lock");
        FileExt::try_lock_exclusive(&legacy_lock).expect("lock legacy daemon");

        let error = DaemonInstanceLock::acquire_at(
            &temporary.path().join("config.toml"),
            &DaemonIdentity::new(),
            &storage_home,
            std::slice::from_ref(&legacy_home),
        )
        .expect_err("legacy daemon must block current daemon")
        .to_string();
        assert!(error.contains("legacy daemon holds"));
        assert!(!storage_home.join("threadrelay-daemon.lock").exists());

        let _ = FileExt::unlock(&legacy_lock);
    }
}
