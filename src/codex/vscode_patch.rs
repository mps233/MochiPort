use std::{
    env, fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const APP_SERVER_SPAWN_MARKER: &str = "Spawning codex app-server";
const APP_SERVER_SUBCOMMAND: &str = "app-server";
const ANALYTICS_FLAG: &str = "--analytics-default-enabled";
const REMOTE_CONTROL_FLAG: &str = "--remote-control";
const APP_SERVER_LAUNCH_SCAN_BYTES: usize = 2048;
const BACKUP_SUFFIX: &str = ".bak-mochiport";
const STATE_SUFFIX: &str = ".mochiport-state.json";
const LEGACY_BACKUP_SUFFIX: &str = ".bak-codexhub";
const LEGACY_STATE_SUFFIX: &str = ".codexhub-state.json";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VsCodeExtensionPatchReport {
    pub extension_dir: Option<PathBuf>,
    pub extension_js: Option<PathBuf>,
    pub backup_path: Option<PathBuf>,
    pub action: String,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PatchState {
    extension_js: PathBuf,
    backup_path: PathBuf,
    original_sha256: String,
    patched_sha256: String,
    patched_at_unix_secs: u64,
}

#[derive(Debug)]
struct ExtensionInstall {
    dir: PathBuf,
    extension_js: PathBuf,
    version_key: Vec<u64>,
    modified: SystemTime,
}

fn patch_remote_control_source(source: &str) -> Result<Option<String>> {
    let Some((array_end, args)) = find_app_server_launch_args(source) else {
        return Ok(None);
    };
    if args.iter().any(|arg| arg == REMOTE_CONTROL_FLAG) {
        return Ok(Some(source.to_string()));
    }

    let mut patched = String::with_capacity(source.len() + REMOTE_CONTROL_FLAG.len() + 3);
    patched.push_str(&source[..array_end]);
    patched.push_str(",\"--remote-control\"");
    patched.push_str(&source[array_end..]);
    Ok(Some(patched))
}

fn find_app_server_launch_args(source: &str) -> Option<(usize, Vec<String>)> {
    for (marker_start, _) in source.match_indices(APP_SERVER_SPAWN_MARKER) {
        let marker_end = marker_start + APP_SERVER_SPAWN_MARKER.len();
        let mut scan_end = marker_end
            .saturating_add(APP_SERVER_LAUNCH_SCAN_BYTES)
            .min(source.len());
        while scan_end > marker_end && !source.is_char_boundary(scan_end) {
            scan_end -= 1;
        }
        let search_area = &source[marker_end..scan_end];
        for (relative_start, _) in search_area.match_indices('[') {
            let array_start = marker_end + relative_start;
            let Some(array_end) = find_array_end(source, array_start, scan_end) else {
                continue;
            };
            let Ok(args) = serde_json::from_str::<Vec<String>>(&source[array_start..=array_end])
            else {
                continue;
            };
            if !args.iter().any(|arg| arg == ANALYTICS_FLAG) {
                continue;
            }
            let context = &source[marker_end..array_start];
            if args.iter().any(|arg| arg == APP_SERVER_SUBCOMMAND)
                || context.contains("\"app-server\"")
            {
                return Some((array_end, args));
            }
        }
    }
    None
}

fn find_array_end(source: &str, array_start: usize, scan_end: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (index, byte) in bytes.iter().enumerate().take(scan_end).skip(array_start) {
        if in_string {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                in_string = false;
            }
            continue;
        }

        match *byte {
            b'"' => in_string = true,
            b'[' => depth = depth.saturating_add(1),
            b']' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

/// Inspects the installed extension without modifying it. An extension that
/// already launches its app-server with `--remote-control` uses its own
/// supported capability and never needs a MochiPort patch.
pub fn inspect_remote_control() -> Result<VsCodeExtensionPatchReport> {
    let Some(install) = find_latest_codex_extension()? else {
        return Ok(VsCodeExtensionPatchReport {
            extension_dir: None,
            extension_js: None,
            backup_path: None,
            action: "not_found".to_string(),
            message: "没有找到 OpenAI Codex VS Code 插件安装目录。".to_string(),
        });
    };
    inspect_remote_control_install(install)
}

fn inspect_remote_control_install(install: ExtensionInstall) -> Result<VsCodeExtensionPatchReport> {
    let extension_js = install.extension_js;
    let managed_backup = managed_backup_path(&extension_js);
    let managed_state = managed_state_path(&extension_js);
    let source = fs::read_to_string(&extension_js)
        .with_context(|| format!("failed to read {}", extension_js.display()))?;

    let Some(patched) = patch_remote_control_source(&source)? else {
        return Ok(VsCodeExtensionPatchReport {
            extension_dir: Some(install.dir),
            extension_js: Some(extension_js),
            backup_path: None,
            action: "unsupported".to_string(),
            message: "当前 VS Code Codex 插件未暴露可识别的远程控制启动能力。".to_string(),
        });
    };
    if patched == source {
        let managed_patch = managed_backup.is_some() || managed_state.is_some();
        let reported_backup = managed_backup.or_else(|| {
            managed_state.as_ref().map(|path| {
                if *path == legacy_state_path(&extension_js) {
                    legacy_backup_path(&extension_js)
                } else {
                    backup_path(&extension_js)
                }
            })
        });
        return Ok(VsCodeExtensionPatchReport {
            extension_dir: Some(install.dir),
            extension_js: Some(extension_js),
            backup_path: reported_backup,
            action: if managed_patch {
                "legacy_fallback_installed".to_string()
            } else {
                "official_supported".to_string()
            },
            message: if managed_patch {
                "VS Code Codex 插件包含 --remote-control；检测到旧版 MochiPort 补丁记录。"
                    .to_string()
            } else {
                "VS Code Codex 插件已原生支持 --remote-control。".to_string()
            },
        });
    }

    Ok(VsCodeExtensionPatchReport {
        extension_dir: Some(install.dir),
        extension_js: Some(extension_js),
        backup_path: None,
        action: "legacy_fallback_required".to_string(),
        message: "当前 VS Code Codex 插件未原生支持 --remote-control；只有显式确认后才可使用旧版兼容补丁。".to_string(),
    })
}

/// Applies the source rewrite only after an explicit user command. Normal
/// daemon start and shutdown paths must call `inspect_remote_control` instead.
pub fn enable_legacy_patch_fallback() -> Result<VsCodeExtensionPatchReport> {
    let Some(install) = find_latest_codex_extension()? else {
        return Ok(VsCodeExtensionPatchReport {
            extension_dir: None,
            extension_js: None,
            backup_path: None,
            action: "not_found".to_string(),
            message: "没有找到 OpenAI Codex VS Code 插件安装目录。".to_string(),
        });
    };
    enable_legacy_patch_fallback_for_install(install)
}

fn enable_legacy_patch_fallback_for_install(
    install: ExtensionInstall,
) -> Result<VsCodeExtensionPatchReport> {
    let extension_js = install.extension_js;
    let new_backup_path = backup_path(&extension_js);
    let new_state_path = state_path(&extension_js);
    let source = fs::read_to_string(&extension_js)
        .with_context(|| format!("failed to read {}", extension_js.display()))?;

    let Some(patched) = patch_remote_control_source(&source)? else {
        return Err(anyhow!(
            "无法识别 VS Code Codex 插件启动参数位置: {}",
            extension_js.display()
        ));
    };
    if patched == source {
        let managed_backup = managed_backup_path(&extension_js);
        if let Some(backup_path) = managed_backup.as_ref()
            && managed_state_path(&extension_js).is_none()
        {
            ensure_state_for_existing_patch(&extension_js, backup_path, &new_state_path, &source)?;
        }
        return Ok(VsCodeExtensionPatchReport {
            extension_dir: Some(install.dir),
            extension_js: Some(extension_js),
            backup_path: managed_backup.clone(),
            action: if managed_backup.is_some() {
                "already_patched".to_string()
            } else {
                "official_supported".to_string()
            },
            message: if managed_backup.is_some() {
                "VS Code Codex 插件已存在 MochiPort 兼容补丁。".to_string()
            } else {
                "VS Code Codex 插件已原生支持 --remote-control，未写入补丁。".to_string()
            },
        });
    }

    if !new_backup_path.exists() {
        fs::copy(&extension_js, &new_backup_path).with_context(|| {
            format!(
                "failed to backup {} to {}",
                extension_js.display(),
                new_backup_path.display()
            )
        })?;
    }

    fs::write(&extension_js, &patched)
        .with_context(|| format!("failed to write {}", extension_js.display()))?;
    write_patch_state(
        &extension_js,
        &new_backup_path,
        &source,
        &patched,
        &new_state_path,
    )?;

    Ok(VsCodeExtensionPatchReport {
        extension_dir: Some(install.dir),
        extension_js: Some(extension_js),
        backup_path: Some(new_backup_path),
        action: "patched".to_string(),
        message: "已为 VS Code Codex 插件启动参数加入 --remote-control。".to_string(),
    })
}

/// Restores a patch created by `enable_legacy_patch_fallback`. This is an
/// explicit maintenance action, never a daemon lifecycle hook.
pub fn restore_legacy_patch_fallback() -> Result<VsCodeExtensionPatchReport> {
    let Some(install) = find_latest_codex_extension()? else {
        return Ok(VsCodeExtensionPatchReport {
            extension_dir: None,
            extension_js: None,
            backup_path: None,
            action: "not_found".to_string(),
            message: "没有找到 OpenAI Codex VS Code 插件安装目录。".to_string(),
        });
    };
    restore_legacy_patch_fallback_for_install(install)
}

fn restore_legacy_patch_fallback_for_install(
    install: ExtensionInstall,
) -> Result<VsCodeExtensionPatchReport> {
    let extension_js = install.extension_js;
    let managed_state_path = managed_state_path(&extension_js);
    let state = managed_state_path
        .as_deref()
        .and_then(|path| read_patch_state(path).ok());
    let current_backup_path = backup_path(&extension_js);
    let managed_backup_path = managed_backup_path(&extension_js);
    let Some(managed_backup_path) = managed_backup_path else {
        return Ok(VsCodeExtensionPatchReport {
            extension_dir: Some(install.dir),
            extension_js: Some(extension_js),
            backup_path: Some(current_backup_path),
            action: "no_backup".to_string(),
            message: "没有找到 MochiPort 创建的插件备份，未还原。".to_string(),
        });
    };

    let current = fs::read_to_string(&extension_js)
        .with_context(|| format!("failed to read {}", extension_js.display()))?;
    if let Some(state) = state.as_ref()
        && state.patched_sha256 != sha256_hex(current.as_bytes())
    {
        return Ok(VsCodeExtensionPatchReport {
            extension_dir: Some(install.dir),
            extension_js: Some(extension_js),
            backup_path: Some(managed_backup_path),
            action: "skipped_modified".to_string(),
            message: "VS Code 插件文件已被用户或插件更新修改，未自动还原。".to_string(),
        });
    }

    let current_has_remote_control =
        patch_remote_control_source(&current)?.is_some_and(|transformed| transformed == current);
    if state.is_none() && !current_has_remote_control {
        return Ok(VsCodeExtensionPatchReport {
            extension_dir: Some(install.dir),
            extension_js: Some(extension_js),
            backup_path: Some(managed_backup_path),
            action: "skipped_unmanaged".to_string(),
            message: "当前插件文件不像 MochiPort 写入的版本，未自动还原。".to_string(),
        });
    }

    fs::copy(&managed_backup_path, &extension_js).with_context(|| {
        format!(
            "failed to restore {} from {}",
            extension_js.display(),
            managed_backup_path.display()
        )
    })?;
    for path in [state_path(&extension_js), legacy_state_path(&extension_js)] {
        let _ = fs::remove_file(path);
    }

    Ok(VsCodeExtensionPatchReport {
        extension_dir: Some(install.dir),
        extension_js: Some(extension_js),
        backup_path: Some(managed_backup_path),
        action: "restored".to_string(),
        message: "已还原 VS Code Codex 插件原始启动方式。".to_string(),
    })
}

fn find_latest_codex_extension() -> Result<Option<ExtensionInstall>> {
    let mut installs = Vec::new();
    for root in extension_roots() {
        let Ok(entries) = fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let Some(install) = inspect_extension_dir(&dir)? else {
                continue;
            };
            installs.push(install);
        }
    }
    installs.sort_by(|a, b| {
        a.version_key
            .cmp(&b.version_key)
            .then_with(|| a.modified.cmp(&b.modified))
    });
    Ok(installs.pop())
}

fn inspect_extension_dir(dir: &Path) -> Result<Option<ExtensionInstall>> {
    let package_path = dir.join("package.json");
    if !package_path.exists() {
        return Ok(None);
    }
    let package_text = fs::read_to_string(&package_path)
        .with_context(|| format!("failed to read {}", package_path.display()))?;
    let package: serde_json::Value = serde_json::from_str(&package_text)
        .with_context(|| format!("failed to parse {}", package_path.display()))?;
    if package.get("publisher").and_then(|value| value.as_str()) != Some("openai")
        || package.get("name").and_then(|value| value.as_str()) != Some("chatgpt")
    {
        return Ok(None);
    }

    let main = package
        .get("main")
        .and_then(|value| value.as_str())
        .unwrap_or("./out/extension.js")
        .trim_start_matches("./");
    let extension_js = dir.join(main);
    if !extension_js.exists() {
        return Ok(None);
    }
    let version = package
        .get("version")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let modified = fs::metadata(&extension_js)
        .and_then(|metadata| metadata.modified())
        .unwrap_or(UNIX_EPOCH);
    Ok(Some(ExtensionInstall {
        dir: dir.to_path_buf(),
        extension_js,
        version_key: version_key(version),
        modified,
    }))
}

fn extension_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(value) = env::var_os("VSCODE_EXTENSIONS") {
        roots.push(PathBuf::from(value));
    }
    if let Some(home) = env::var_os("USERPROFILE").map(PathBuf::from) {
        roots.push(home.join(".vscode").join("extensions"));
        roots.push(home.join(".vscode-insiders").join("extensions"));
        roots.push(home.join(".vscodium").join("extensions"));
    }
    roots.sort();
    roots.dedup();
    roots
}

fn version_key(version: &str) -> Vec<u64> {
    version
        .split(|ch: char| !ch.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .map(|part| part.parse::<u64>().unwrap_or(0))
        .collect()
}

fn backup_path(extension_js: &Path) -> PathBuf {
    backup_path_for_suffix(extension_js, BACKUP_SUFFIX)
}

fn state_path(extension_js: &Path) -> PathBuf {
    state_path_for_suffix(extension_js, STATE_SUFFIX)
}

fn legacy_backup_path(extension_js: &Path) -> PathBuf {
    backup_path_for_suffix(extension_js, LEGACY_BACKUP_SUFFIX)
}

fn legacy_state_path(extension_js: &Path) -> PathBuf {
    state_path_for_suffix(extension_js, LEGACY_STATE_SUFFIX)
}

fn backup_path_for_suffix(extension_js: &Path, suffix: &str) -> PathBuf {
    path_with_suffix(extension_js, suffix)
}

fn state_path_for_suffix(extension_js: &Path, suffix: &str) -> PathBuf {
    path_with_suffix(extension_js, suffix)
}

fn path_with_suffix(extension_js: &Path, suffix: &str) -> PathBuf {
    extension_js.with_file_name(format!(
        "{}{}",
        extension_js
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("extension.js"),
        suffix
    ))
}

fn managed_backup_path(extension_js: &Path) -> Option<PathBuf> {
    [backup_path(extension_js), legacy_backup_path(extension_js)]
        .into_iter()
        .find(|path| path.exists())
}

fn managed_state_path(extension_js: &Path) -> Option<PathBuf> {
    [state_path(extension_js), legacy_state_path(extension_js)]
        .into_iter()
        .find(|path| path.exists())
}

fn ensure_state_for_existing_patch(
    extension_js: &Path,
    backup_path: &Path,
    state_path: &Path,
    current: &str,
) -> Result<()> {
    if state_path.exists() || !backup_path.exists() {
        return Ok(());
    }
    let original = fs::read_to_string(backup_path)
        .with_context(|| format!("failed to read {}", backup_path.display()))?;
    write_patch_state(extension_js, backup_path, &original, current, state_path)
}

fn write_patch_state(
    extension_js: &Path,
    backup_path: &Path,
    original: &str,
    patched: &str,
    state_path: &Path,
) -> Result<()> {
    let state = PatchState {
        extension_js: extension_js.to_path_buf(),
        backup_path: backup_path.to_path_buf(),
        original_sha256: sha256_hex(original.as_bytes()),
        patched_sha256: sha256_hex(patched.as_bytes()),
        patched_at_unix_secs: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    };
    let text = serde_json::to_string_pretty(&state)?;
    fs::write(state_path, text).with_context(|| format!("failed to write {}", state_path.display()))
}

fn read_patch_state(path: &Path) -> io::Result<PatchState> {
    let text = fs::read_to_string(path)?;
    serde_json::from_str(&text).map_err(io::Error::other)
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf, time::SystemTime};

    use tempfile::tempdir;

    use super::{
        BACKUP_SUFFIX, ExtensionInstall, LEGACY_BACKUP_SUFFIX, LEGACY_STATE_SUFFIX,
        REMOTE_CONTROL_FLAG, STATE_SUFFIX, enable_legacy_patch_fallback_for_install,
        inspect_remote_control_install, legacy_backup_path, legacy_state_path,
        patch_remote_control_source, restore_legacy_patch_fallback_for_install, sha256_hex,
        state_path, write_patch_state,
    };

    const SOURCE: &str = r#"this.logger.info("Spawning codex app-server"),e=Cde(this.extensionUri,["app-server","--analytics-default-enabled"] )"#;

    fn test_install(extension_js: PathBuf, dir: PathBuf) -> ExtensionInstall {
        ExtensionInstall {
            dir,
            extension_js,
            version_key: vec![1],
            modified: SystemTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn patches_current_26_707_launch_arguments() {
        let source = r#"this.logger.info("Spawning codex app-server"),e=Cde(this.extensionUri,["-c","features.code_mode_host=true","app-server","--analytics-default-enabled"])"#;

        let patched = patch_remote_control_source(source)
            .expect("source transformation should succeed")
            .expect("supported launch site");

        assert!(patched.contains(
            r#"["-c","features.code_mode_host=true","app-server","--analytics-default-enabled","--remote-control"]"#
        ));
    }

    #[test]
    fn patches_legacy_launch_arguments() {
        let source = r#"this.logger.info("Spawning codex app-server"),e=Xle(this.extensionUri,"app-server",["--analytics-default-enabled"])"#;

        let patched = patch_remote_control_source(source)
            .expect("source transformation should succeed")
            .expect("supported launch site");

        assert!(patched.contains(r#"["--analytics-default-enabled","--remote-control"]"#));
    }

    #[test]
    fn patches_launch_arguments_with_extra_and_reordered_flags() {
        let source = r#"this.logger.info("Spawning codex app-server"),e=Cde(this.extensionUri,["--analytics-default-enabled","-c","feature.future=true","app-server","--listen","stdio://"])"#;

        let patched = patch_remote_control_source(source)
            .expect("source transformation should succeed")
            .expect("supported launch site");

        assert_eq!(patched.matches(REMOTE_CONTROL_FLAG).count(), 1);
        assert!(patched.contains(
            r#"["--analytics-default-enabled","-c","feature.future=true","app-server","--listen","stdio://","--remote-control"]"#
        ));
    }

    #[test]
    fn leaves_already_supported_launch_arguments_unchanged() {
        let source = r#"this.logger.info("Spawning codex app-server"),e=Cde(this.extensionUri,["app-server","--analytics-default-enabled","--remote-control"])"#;

        let transformed = patch_remote_control_source(source)
            .expect("source transformation should succeed")
            .expect("supported launch site");

        assert_eq!(transformed, source);
    }

    #[test]
    fn skips_unrelated_arrays_before_the_launch_arguments() {
        let source = r#"this.logger.info("Spawning codex app-server"),x=["unrelated"],e=Cde(this.extensionUri,["app-server","--analytics-default-enabled"])"#;

        let patched = patch_remote_control_source(source)
            .expect("source transformation should succeed")
            .expect("supported launch site");

        assert!(
            patched.contains(r#"["app-server","--analytics-default-enabled","--remote-control"]"#)
        );
        assert!(patched.contains(r#"x=["unrelated"]"#));
    }

    #[test]
    fn rejects_analytics_array_without_app_server_context() {
        let source =
            r#"this.logger.info("Spawning codex app-server"),x=["--analytics-default-enabled"]"#;

        let transformed =
            patch_remote_control_source(source).expect("source transformation should succeed");

        assert!(transformed.is_none());
    }

    #[test]
    fn rejects_source_without_app_server_spawn_marker() {
        let source = r#"e=Cde(this.extensionUri,["app-server","--analytics-default-enabled"])"#;

        let transformed =
            patch_remote_control_source(source).expect("source transformation should succeed");

        assert!(transformed.is_none());
    }

    #[test]
    fn new_fallback_writes_mochiport_artifacts() {
        let temporary = tempdir().expect("temporary directory");
        let extension_js = temporary.path().join("extension.js");
        fs::write(&extension_js, SOURCE).expect("write extension source");

        let report = enable_legacy_patch_fallback_for_install(test_install(
            extension_js.clone(),
            temporary.path().to_path_buf(),
        ))
        .expect("enable fallback");

        let backup = extension_js.with_file_name(format!("extension.js{BACKUP_SUFFIX}"));
        let state = extension_js.with_file_name(format!("extension.js{STATE_SUFFIX}"));
        let old_backup = extension_js.with_file_name(format!("extension.js{LEGACY_BACKUP_SUFFIX}"));
        let old_state = extension_js.with_file_name(format!("extension.js{LEGACY_STATE_SUFFIX}"));

        assert_eq!(report.action, "patched");
        assert_eq!(report.backup_path, Some(backup.clone()));
        assert!(backup.exists());
        assert!(state.exists());
        assert!(!old_backup.exists());
        assert!(!old_state.exists());
        assert!(
            fs::read_to_string(&extension_js)
                .expect("read patched source")
                .contains(REMOTE_CONTROL_FLAG)
        );
    }

    #[test]
    fn inspect_recognizes_legacy_codexhub_artifacts() {
        let temporary = tempdir().expect("temporary directory");
        let extension_js = temporary.path().join("extension.js");
        let patched = patch_remote_control_source(SOURCE)
            .expect("source transformation should succeed")
            .expect("supported launch site");
        fs::write(&extension_js, &patched).expect("write patched source");

        let backup = legacy_backup_path(&extension_js);
        fs::write(&backup, SOURCE).expect("write legacy backup");
        let state = legacy_state_path(&extension_js);
        write_patch_state(&extension_js, &backup, SOURCE, &patched, &state)
            .expect("write legacy state");

        let report = inspect_remote_control_install(test_install(
            extension_js,
            temporary.path().to_path_buf(),
        ))
        .expect("inspect fallback");

        assert_eq!(report.action, "legacy_fallback_installed");
        assert_eq!(report.backup_path, Some(backup));
    }

    #[test]
    fn restore_uses_legacy_codexhub_backup_and_state() {
        let temporary = tempdir().expect("temporary directory");
        let extension_js = temporary.path().join("extension.js");
        let patched = patch_remote_control_source(SOURCE)
            .expect("source transformation should succeed")
            .expect("supported launch site");
        fs::write(&extension_js, &patched).expect("write patched source");

        let backup = legacy_backup_path(&extension_js);
        fs::write(&backup, SOURCE).expect("write legacy backup");
        let state = legacy_state_path(&extension_js);
        write_patch_state(&extension_js, &backup, SOURCE, &patched, &state)
            .expect("write legacy state");

        let report = restore_legacy_patch_fallback_for_install(test_install(
            extension_js.clone(),
            temporary.path().to_path_buf(),
        ))
        .expect("restore fallback");

        assert_eq!(report.action, "restored");
        assert_eq!(report.backup_path, Some(backup.clone()));
        assert_eq!(
            fs::read_to_string(&extension_js).expect("read restored source"),
            SOURCE
        );
        assert!(backup.exists());
        assert!(!state.exists());
        assert!(!state_path(&extension_js).exists());
        assert_eq!(
            sha256_hex(SOURCE.as_bytes()),
            sha256_hex(&fs::read(&extension_js).expect("read restored source bytes"))
        );
    }

    #[test]
    fn restore_prefers_mochiport_backup_when_both_artifacts_exist() {
        let temporary = tempdir().expect("temporary directory");
        let extension_js = temporary.path().join("extension.js");
        let patched = patch_remote_control_source(SOURCE)
            .expect("source transformation should succeed")
            .expect("supported launch site");
        fs::write(&extension_js, &patched).expect("write patched source");

        let new_backup = extension_js.with_file_name(format!("extension.js{BACKUP_SUFFIX}"));
        let old_backup = legacy_backup_path(&extension_js);
        fs::write(&new_backup, SOURCE).expect("write MochiPort backup");
        fs::write(&old_backup, "stale CodexHub backup").expect("write legacy backup");
        let state = state_path(&extension_js);
        write_patch_state(&extension_js, &new_backup, SOURCE, &patched, &state)
            .expect("write MochiPort state");

        let report = restore_legacy_patch_fallback_for_install(test_install(
            extension_js.clone(),
            temporary.path().to_path_buf(),
        ))
        .expect("restore fallback");

        assert_eq!(report.action, "restored");
        assert_eq!(report.backup_path, Some(new_backup));
        assert_eq!(
            fs::read_to_string(&extension_js).expect("read restored source"),
            SOURCE
        );
        assert!(old_backup.exists());
    }
}
