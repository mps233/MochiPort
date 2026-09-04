use std::{
    env,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{daemon_process, types::now_ms};

pub const MOCHIPORT_HOME_ENV: &str = "MOCHIPORT_HOME";

const LEGACY_HOME_ENV_KEYS: [&str; 2] = ["THREADRELAY_HOME", "CODEXHUB_HOME"];
const LEGACY_DIRECTORY_NAMES: [&str; 2] = ["ThreadRelay", "CodexHub"];
const STORAGE_MANIFEST_FILE_NAME: &str = "storage-migration.json";
const PENDING_STORAGE_MANIFEST_FILE_NAME: &str = ".mochiport-storage-migration.pending.json";
const STORAGE_MIGRATION_LOCK_FILE_NAME: &str = ".mochiport-storage-migration.lock";
const STORAGE_MIGRATION_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageMigrationManifest {
    pub schema_version: u32,
    pub migration_id: String,
    pub source_directory: String,
    pub destination_directory: String,
    pub migrated_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageMigrationReport {
    pub source_directory: PathBuf,
    pub destination_directory: PathBuf,
    pub manifest_path: PathBuf,
    pub migrated: bool,
}

/// Returns the one persistent MochiPort home used by ordinary commands.
///
/// Historical home variables deliberately do not participate here. They are
/// only examined by `migrate_storage` after the user explicitly invokes it.
pub fn current_storage_home() -> PathBuf {
    current_storage_home_from(
        env::var_os(MOCHIPORT_HOME_ENV).map(PathBuf::from),
        platform_application_support_directory(),
        env::current_dir().ok(),
    )
}

pub fn current_config_path() -> PathBuf {
    current_storage_home().join("config.toml")
}

/// Lists historical storage roots for an explicit storage migration. This is
/// intentionally not used by ordinary startup or management requests.
pub fn legacy_storage_candidates() -> Vec<PathBuf> {
    let mut candidates = LEGACY_HOME_ENV_KEYS
        .iter()
        .filter_map(|key| env::var_os(key))
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map(absolutize)
        .collect::<Vec<_>>();
    candidates.extend(legacy_standard_storage_homes());
    deduplicate_paths(candidates)
}

/// Standard historical locations are used solely for read-only daemon-lock
/// checks and explicit migration discovery. Legacy environment variables are
/// intentionally excluded so ordinary startup never follows them.
pub fn legacy_standard_storage_homes() -> Vec<PathBuf> {
    platform_application_support_directory()
        .into_iter()
        .flat_map(|base| {
            LEGACY_DIRECTORY_NAMES
                .iter()
                .map(move |name| base.join(name))
        })
        .map(absolutize)
        .collect()
}

pub fn migrate_storage(requested_source: Option<PathBuf>) -> Result<StorageMigrationReport> {
    migrate_storage_to(
        requested_source,
        current_storage_home(),
        legacy_storage_candidates(),
    )
}

fn migrate_storage_to(
    requested_source: Option<PathBuf>,
    destination_directory: PathBuf,
    legacy_candidates: Vec<PathBuf>,
) -> Result<StorageMigrationReport> {
    let destination_directory = absolutize(destination_directory);
    let legacy_candidates = deduplicate_paths(
        legacy_candidates
            .into_iter()
            .map(absolutize)
            .collect::<Vec<_>>(),
    );
    let _migration_lock = StorageMigrationLock::acquire(&destination_directory)?;

    if let Some(report) = recover_or_report_completed_migration(
        requested_source.as_deref(),
        &destination_directory,
        &legacy_candidates,
    )? {
        return Ok(report);
    }

    if daemon_process::current_daemon_is_active_at(&destination_directory)? {
        anyhow::bail!(
            "MochiPort daemon is running from `{}`; stop it before migrating storage",
            destination_directory.display()
        );
    }

    if destination_directory.exists() {
        anyhow::bail!(
            "MochiPort storage already exists at `{}`. Storage migration never merges directories.",
            destination_directory.display()
        );
    }

    let source_directory =
        select_legacy_source(requested_source, &legacy_candidates, &destination_directory)?;
    if destination_directory.starts_with(&source_directory) {
        anyhow::bail!(
            "MochiPort storage destination `{}` may not be inside legacy storage `{}`",
            destination_directory.display(),
            source_directory.display()
        );
    }
    validate_legacy_source(&source_directory)?;

    if let Some(active) = daemon_process::active_legacy_daemon_at(&source_directory)? {
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
        anyhow::bail!(
            "legacy daemon still holds `{}` ({owner}); stop it before migrating storage",
            active.lock_path.display()
        );
    }

    let destination_parent = destination_directory
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| anyhow!("MochiPort storage destination has no parent"))?;
    fs::create_dir_all(destination_parent).with_context(|| {
        format!(
            "failed to create MochiPort storage parent `{}`",
            destination_parent.display()
        )
    })?;

    let pending_manifest_path = source_directory.join(PENDING_STORAGE_MANIFEST_FILE_NAME);
    let manifest = if pending_manifest_path.exists() {
        let pending = read_manifest(&pending_manifest_path)?;
        validate_manifest_destination(&pending, &destination_directory)?;
        if !same_path(
            &absolutize(PathBuf::from(&pending.source_directory)),
            &source_directory,
        ) {
            anyhow::bail!(
                "pending migration manifest in `{}` belongs to another legacy storage directory",
                source_directory.display()
            );
        }
        pending
    } else {
        let manifest = StorageMigrationManifest {
            schema_version: STORAGE_MIGRATION_SCHEMA_VERSION,
            migration_id: Uuid::new_v4().to_string(),
            source_directory: source_directory.display().to_string(),
            destination_directory: destination_directory.display().to_string(),
            migrated_at_ms: now_ms().min(u64::MAX as u128) as u64,
        };
        write_json_atomically(&pending_manifest_path, &manifest)?;
        manifest
    };

    // `rename` is atomic only within a filesystem. Do not copy as a fallback:
    // a partial copy would make a storage migration unrecoverable to audit.
    fs::rename(&source_directory, &destination_directory).with_context(|| {
        format!(
            "failed to atomically move legacy storage `{}` to `{}`; both locations must be on the same filesystem",
            source_directory.display(),
            destination_directory.display()
        )
    })?;
    sync_directory(destination_parent)?;

    let manifest_path = destination_directory.join(STORAGE_MANIFEST_FILE_NAME);
    write_json_atomically(&manifest_path, &manifest)?;
    let _ = fs::remove_file(destination_directory.join(PENDING_STORAGE_MANIFEST_FILE_NAME));
    sync_directory(&destination_directory)?;

    Ok(StorageMigrationReport {
        source_directory,
        destination_directory,
        manifest_path,
        migrated: true,
    })
}

fn recover_or_report_completed_migration(
    requested_source: Option<&Path>,
    destination_directory: &Path,
    legacy_candidates: &[PathBuf],
) -> Result<Option<StorageMigrationReport>> {
    if !destination_directory.exists() {
        return Ok(None);
    }

    let manifest_path = destination_directory.join(STORAGE_MANIFEST_FILE_NAME);
    if manifest_path.is_file() {
        let manifest = read_manifest(&manifest_path)?;
        return report_completed_migration(
            requested_source,
            destination_directory,
            legacy_candidates,
            manifest_path,
            manifest,
        )
        .map(Some);
    }

    let pending_path = destination_directory.join(PENDING_STORAGE_MANIFEST_FILE_NAME);
    if !pending_path.is_file() {
        return Ok(None);
    }

    let manifest = read_manifest(&pending_path)?;
    validate_manifest_destination(&manifest, destination_directory)?;
    if Path::new(&manifest.source_directory).exists() {
        anyhow::bail!(
            "pending migration manifest in `{}` has both source and destination present; refusing to guess how to merge storage",
            destination_directory.display()
        );
    }
    let final_path = destination_directory.join(STORAGE_MANIFEST_FILE_NAME);
    write_json_atomically(&final_path, &manifest)?;
    let _ = fs::remove_file(pending_path);
    sync_directory(destination_directory)?;
    report_completed_migration(
        requested_source,
        destination_directory,
        legacy_candidates,
        final_path,
        manifest,
    )
    .map(Some)
}

fn report_completed_migration(
    requested_source: Option<&Path>,
    destination_directory: &Path,
    legacy_candidates: &[PathBuf],
    manifest_path: PathBuf,
    manifest: StorageMigrationManifest,
) -> Result<StorageMigrationReport> {
    validate_manifest_destination(&manifest, destination_directory)?;
    let recorded_source = absolutize(PathBuf::from(&manifest.source_directory));

    if let Some(requested_source) = requested_source {
        let requested_source = absolutize(requested_source.to_path_buf());
        if !same_path(&requested_source, &recorded_source) {
            anyhow::bail!(
                "MochiPort storage at `{}` was already migrated from `{}`. A second legacy directory will not be merged.",
                destination_directory.display(),
                recorded_source.display()
            );
        }
    } else if legacy_candidates
        .iter()
        .filter(|candidate| is_legacy_storage_directory(candidate))
        .any(|candidate| !same_path(candidate, &recorded_source))
    {
        anyhow::bail!(
            "MochiPort storage at `{}` already has a migration manifest. Select and resolve any remaining legacy directory manually; migrations never merge storage.",
            destination_directory.display()
        );
    }

    Ok(StorageMigrationReport {
        source_directory: recorded_source,
        destination_directory: destination_directory.to_path_buf(),
        manifest_path,
        migrated: false,
    })
}

fn select_legacy_source(
    requested_source: Option<PathBuf>,
    legacy_candidates: &[PathBuf],
    destination_directory: &Path,
) -> Result<PathBuf> {
    if let Some(source) = requested_source {
        let source = absolutize(source);
        if same_path(&source, destination_directory) {
            anyhow::bail!(
                "`{}` is already the MochiPort storage directory",
                source.display()
            );
        }
        if !legacy_candidates
            .iter()
            .any(|candidate| same_path(candidate, &source))
        {
            anyhow::bail!(
                "`{}` is not a detected ThreadRelay or CodexHub storage directory",
                source.display()
            );
        }
        return Ok(source);
    }

    let candidates = legacy_candidates
        .iter()
        .filter(|candidate| is_legacy_storage_directory(candidate))
        .cloned()
        .collect::<Vec<_>>();
    match candidates.as_slice() {
        [] => anyhow::bail!(
            "no legacy ThreadRelay or CodexHub storage was found. Use `mochiport migrate-storage --from PATH` after setting the historical home variable if needed"
        ),
        [source] => Ok(source.clone()),
        _ => anyhow::bail!(
            "multiple legacy storage directories were found: {}. Re-run with `mochiport migrate-storage --from PATH`.",
            candidates
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn validate_legacy_source(source_directory: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(source_directory).with_context(|| {
        format!(
            "failed to inspect legacy storage directory `{}`",
            source_directory.display()
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        anyhow::bail!(
            "legacy storage `{}` must be a real directory",
            source_directory.display()
        );
    }
    if !is_legacy_storage_directory(source_directory) {
        anyhow::bail!(
            "legacy storage `{}` does not contain a recognized MochiPort, ThreadRelay, or CodexHub data file",
            source_directory.display()
        );
    }
    Ok(())
}

fn is_legacy_storage_directory(path: &Path) -> bool {
    path.is_dir()
        && [
            "config.toml",
            "mochiport-state.json",
            "threadrelay-control.json",
            "codexhub-control.json",
            "mochiport-control.json",
            "threadrelay-daemon.lock",
            "codexhub-daemon.lock",
            "mochiport-daemon.lock",
            "runtimes",
        ]
        .iter()
        .any(|name| path.join(name).exists())
}

fn read_manifest(path: &Path) -> Result<StorageMigrationManifest> {
    let bytes = fs::read(path).with_context(|| {
        format!(
            "failed to read storage migration manifest `{}`",
            path.display()
        )
    })?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("invalid storage migration manifest `{}`", path.display()))
}

fn validate_manifest_destination(
    manifest: &StorageMigrationManifest,
    destination_directory: &Path,
) -> Result<()> {
    if manifest.schema_version != STORAGE_MIGRATION_SCHEMA_VERSION
        || manifest.migration_id.trim().is_empty()
        || !same_path(
            &absolutize(PathBuf::from(&manifest.destination_directory)),
            destination_directory,
        )
    {
        anyhow::bail!(
            "storage migration manifest in `{}` does not describe this MochiPort storage directory",
            destination_directory.display()
        );
    }
    Ok(())
}

fn write_json_atomically<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| anyhow!("storage migration manifest has no parent"))?;
    fs::create_dir_all(parent).with_context(|| {
        format!(
            "failed to create migration directory `{}`",
            parent.display()
        )
    })?;
    let bytes = serde_json::to_vec_pretty(value)?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".mochiport-storage-migration.")
        .tempfile_in(parent)
        .with_context(|| format!("failed to prepare migration manifest `{}`", path.display()))?;
    temporary
        .write_all(&bytes)
        .and_then(|_| temporary.as_file().write_all(b"\n"))
        .and_then(|_| temporary.as_file().sync_all())
        .with_context(|| format!("failed to write migration manifest `{}`", path.display()))?;
    temporary
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| {
            format!(
                "failed to atomically publish migration manifest `{}`",
                path.display()
            )
        })?;
    sync_directory(parent)
}

fn current_storage_home_from(
    mochiport_home: Option<PathBuf>,
    application_support: Option<PathBuf>,
    current_directory: Option<PathBuf>,
) -> PathBuf {
    mochiport_home
        .filter(|path| !path.as_os_str().is_empty())
        .map(absolutize)
        .or_else(|| application_support.map(|base| absolutize(base.join("MochiPort"))))
        .unwrap_or_else(|| {
            current_directory
                .map(|directory| absolutize(directory.join("MochiPort")))
                .unwrap_or_else(|| PathBuf::from("MochiPort"))
        })
}

#[cfg(target_os = "windows")]
fn platform_application_support_directory() -> Option<PathBuf> {
    env::var_os("LOCALAPPDATA")
        .or_else(|| env::var_os("APPDATA"))
        .map(PathBuf::from)
}

#[cfg(not(target_os = "windows"))]
fn platform_application_support_directory() -> Option<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join("Library/Application Support"))
}

fn absolutize(path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        env::current_dir()
            .map(|current| current.join(path.clone()))
            .unwrap_or(path)
    }
}

fn deduplicate_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut result = Vec::<PathBuf>::new();
    for path in paths {
        if !result.iter().any(|seen| same_path(seen, &path)) {
            result.push(path);
        }
    }
    result
}

fn same_path(left: &Path, right: &Path) -> bool {
    let left = fs::canonicalize(left).unwrap_or_else(|_| left.to_path_buf());
    let right = fs::canonicalize(right).unwrap_or_else(|_| right.to_path_buf());
    left == right
}

fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        File::open(path)
            .and_then(|directory| directory.sync_all())
            .with_context(|| format!("failed to sync directory `{}`", path.display()))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

struct StorageMigrationLock {
    file: File,
}

impl StorageMigrationLock {
    fn acquire(destination_directory: &Path) -> Result<Self> {
        let parent = destination_directory
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .ok_or_else(|| anyhow!("MochiPort storage destination has no parent"))?;
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create MochiPort storage parent `{}`",
                parent.display()
            )
        })?;
        let path = parent.join(STORAGE_MIGRATION_LOCK_FILE_NAME);
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&path)
            .with_context(|| {
                format!("failed to open storage migration lock `{}`", path.display())
            })?;
        FileExt::try_lock_exclusive(&file).map_err(|error| {
            anyhow!(
                "another MochiPort storage migration is already running (`{}`): {error}",
                path.display()
            )
        })?;
        Ok(Self { file })
    }
}

impl Drop for StorageMigrationLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn legacy_directory(root: &Path, name: &str) -> PathBuf {
        let directory = root.join(name);
        fs::create_dir_all(&directory).expect("create legacy directory");
        fs::write(directory.join("config.toml"), "bind = '127.0.0.1:3847'\n")
            .expect("write legacy config");
        directory
    }

    #[test]
    fn current_storage_home_only_accepts_mochiport_home() {
        let configured = current_storage_home_from(
            Some(PathBuf::from("/fixture/mochiport")),
            Some(PathBuf::from("/fixture/Application Support")),
            None,
        );
        assert_eq!(configured, PathBuf::from("/fixture/mochiport"));

        let default = current_storage_home_from(
            None,
            Some(PathBuf::from("/fixture/Application Support")),
            None,
        );
        assert_eq!(
            default,
            PathBuf::from("/fixture/Application Support/MochiPort")
        );
    }

    #[test]
    fn migration_moves_directory_and_writes_manifest() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let source = legacy_directory(temporary.path(), "ThreadRelay");
        fs::write(source.join("mochiport-state.json"), "state").expect("write state");
        let destination = temporary.path().join("MochiPort");

        let report = migrate_storage_to(None, destination.clone(), vec![source.clone()])
            .expect("migrate storage");
        assert!(report.migrated);
        assert!(!source.exists());
        assert_eq!(
            fs::read_to_string(destination.join("config.toml")).expect("moved config"),
            "bind = '127.0.0.1:3847'\n"
        );
        let manifest = read_manifest(&report.manifest_path).expect("read manifest");
        assert_eq!(manifest.schema_version, STORAGE_MIGRATION_SCHEMA_VERSION);
        assert_eq!(manifest.source_directory, source.display().to_string());
        assert_eq!(
            manifest.destination_directory,
            destination.display().to_string()
        );
    }

    #[test]
    fn migration_refuses_to_merge_into_existing_mochiport_storage() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let source = legacy_directory(temporary.path(), "ThreadRelay");
        let destination = legacy_directory(temporary.path(), "MochiPort");

        let error = migrate_storage_to(None, destination.clone(), vec![source.clone()])
            .expect_err("existing MochiPort storage must not be merged")
            .to_string();
        assert!(error.contains("never merges directories"));
        assert!(source.join("config.toml").exists());
        assert!(destination.join("config.toml").exists());
    }

    #[test]
    fn migration_requires_an_explicit_source_when_multiple_legacy_directories_exist() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let threadrelay = legacy_directory(temporary.path(), "ThreadRelay");
        let codexhub = legacy_directory(temporary.path(), "CodexHub");
        let destination = temporary.path().join("MochiPort");

        let error = migrate_storage_to(None, destination, vec![threadrelay, codexhub])
            .expect_err("multiple candidates must require a source")
            .to_string();
        assert!(error.contains("multiple legacy storage directories"));
    }

    #[test]
    fn migration_refuses_an_active_legacy_daemon_lock() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let source = legacy_directory(temporary.path(), "ThreadRelay");
        let lock_path = source.join("threadrelay-daemon.lock");
        let lock = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&lock_path)
            .expect("open legacy lock");
        FileExt::try_lock_exclusive(&lock).expect("lock legacy daemon");

        let error = migrate_storage_to(None, temporary.path().join("MochiPort"), vec![source])
            .expect_err("active legacy daemon must block migration")
            .to_string();
        assert!(error.contains("legacy daemon still holds"));
        let _ = FileExt::unlock(&lock);
    }
}
