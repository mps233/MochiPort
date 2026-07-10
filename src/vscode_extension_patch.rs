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
const BACKUP_SUFFIX: &str = ".bak-codexhub";
const STATE_SUFFIX: &str = ".codexhub-state.json";

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

pub fn enable_remote_control() -> Result<VsCodeExtensionPatchReport> {
    let Some(install) = find_latest_codex_extension()? else {
        return Ok(VsCodeExtensionPatchReport {
            extension_dir: None,
            extension_js: None,
            backup_path: None,
            action: "not_found".to_string(),
            message: "没有找到 OpenAI Codex VS Code 插件安装目录。".to_string(),
        });
    };

    let extension_js = install.extension_js;
    let backup_path = backup_path(&extension_js);
    let state_path = state_path(&extension_js);
    let source = fs::read_to_string(&extension_js)
        .with_context(|| format!("failed to read {}", extension_js.display()))?;

    let Some(patched) = patch_remote_control_source(&source)? else {
        return Err(anyhow!(
            "无法识别 VS Code Codex 插件启动参数位置: {}",
            extension_js.display()
        ));
    };
    if patched == source {
        let managed_patch = backup_path.exists();
        if managed_patch {
            ensure_state_for_existing_patch(&extension_js, &backup_path, &state_path, &source)?;
        }
        return Ok(VsCodeExtensionPatchReport {
            extension_dir: Some(install.dir),
            extension_js: Some(extension_js),
            backup_path: managed_patch.then_some(backup_path),
            action: if managed_patch {
                "already_patched".to_string()
            } else {
                "already_supported".to_string()
            },
            message: if managed_patch {
                "VS Code Codex 插件已经带有 --remote-control。".to_string()
            } else {
                "VS Code Codex 插件已包含 --remote-control，未创建还原备份。".to_string()
            },
        });
    }

    if !backup_path.exists() {
        fs::copy(&extension_js, &backup_path).with_context(|| {
            format!(
                "failed to backup {} to {}",
                extension_js.display(),
                backup_path.display()
            )
        })?;
    }

    fs::write(&extension_js, &patched)
        .with_context(|| format!("failed to write {}", extension_js.display()))?;
    write_patch_state(&extension_js, &backup_path, &source, &patched, &state_path)?;

    Ok(VsCodeExtensionPatchReport {
        extension_dir: Some(install.dir),
        extension_js: Some(extension_js),
        backup_path: Some(backup_path),
        action: "patched".to_string(),
        message: "已为 VS Code Codex 插件启动参数加入 --remote-control。".to_string(),
    })
}

pub fn restore_remote_control() -> Result<VsCodeExtensionPatchReport> {
    let Some(install) = find_latest_codex_extension()? else {
        return Ok(VsCodeExtensionPatchReport {
            extension_dir: None,
            extension_js: None,
            backup_path: None,
            action: "not_found".to_string(),
            message: "没有找到 OpenAI Codex VS Code 插件安装目录。".to_string(),
        });
    };

    let extension_js = install.extension_js;
    let backup_path = backup_path(&extension_js);
    let state_path = state_path(&extension_js);
    if !backup_path.exists() {
        return Ok(VsCodeExtensionPatchReport {
            extension_dir: Some(install.dir),
            extension_js: Some(extension_js),
            backup_path: Some(backup_path),
            action: "no_backup".to_string(),
            message: "没有找到 CodexHub 创建的插件备份，未还原。".to_string(),
        });
    }

    let current = fs::read_to_string(&extension_js)
        .with_context(|| format!("failed to read {}", extension_js.display()))?;
    let state = read_patch_state(&state_path).ok();
    if let Some(state) = state.as_ref()
        && state.patched_sha256 != sha256_hex(current.as_bytes())
    {
        return Ok(VsCodeExtensionPatchReport {
            extension_dir: Some(install.dir),
            extension_js: Some(extension_js),
            backup_path: Some(backup_path),
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
            backup_path: Some(backup_path),
            action: "skipped_unmanaged".to_string(),
            message: "当前插件文件不像 CodexHub 写入的版本，未自动还原。".to_string(),
        });
    }

    fs::copy(&backup_path, &extension_js).with_context(|| {
        format!(
            "failed to restore {} from {}",
            extension_js.display(),
            backup_path.display()
        )
    })?;
    let _ = fs::remove_file(&state_path);

    Ok(VsCodeExtensionPatchReport {
        extension_dir: Some(install.dir),
        extension_js: Some(extension_js),
        backup_path: Some(backup_path),
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
    extension_js.with_file_name(format!(
        "{}{}",
        extension_js
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("extension.js"),
        BACKUP_SUFFIX
    ))
}

fn state_path(extension_js: &Path) -> PathBuf {
    extension_js.with_file_name(format!(
        "{}{}",
        extension_js
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("extension.js"),
        STATE_SUFFIX
    ))
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
    use super::{REMOTE_CONTROL_FLAG, patch_remote_control_source};

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
}
