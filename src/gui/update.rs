use std::{
    cell::RefCell,
    process::Command,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use reqwest::blocking::Client;
use serde::Deserialize;
use wxdragon::{prelude::*, timer::Timer};

use super::daemon::hide_command_window;
use super::{
    FrameTimerStore, GuiTimers, UPDATE_CHECK_TIMEOUT, UPDATE_MANIFEST_URL, UPDATE_RELEASE_API_URL,
    UPDATE_RELEASE_PAGE_URL,
};
use super::{confirm_open_update_release, show_error, show_info};

#[derive(Debug)]
struct LatestReleaseInfo {
    version: String,
    release_url: String,
    notes: Option<String>,
}

#[derive(Debug)]
enum UpdateCheckOutcome {
    Newer {
        current_version: String,
        latest_version: String,
        release_url: String,
        notes: Option<String>,
    },
    Current {
        current_version: String,
        latest_version: String,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateManifest {
    version: String,
    #[serde(default, alias = "release_url", alias = "html_url")]
    release_url: Option<String>,
    #[serde(default)]
    notes: Option<String>,
}

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: String,
    body: Option<String>,
}

pub(super) fn check_for_updates_async(
    frame: &Frame,
    gui_timers: &GuiTimers,
    in_flight: &Arc<AtomicBool>,
) {
    if in_flight.swap(true, Ordering::SeqCst) {
        show_info(frame, "正在检查更新，请稍候。");
        return;
    }

    let result: Arc<Mutex<Option<Result<UpdateCheckOutcome, String>>>> = Arc::new(Mutex::new(None));
    {
        let result = result.clone();
        thread::spawn(move || {
            let update = check_for_updates();
            if let Ok(mut slot) = result.lock() {
                slot.replace(update);
            }
        });
    }

    let update_timer_store: FrameTimerStore = Rc::new(RefCell::new(None));
    let update_timer = Timer::new(frame);
    {
        let frame = *frame;
        let in_flight = in_flight.clone();
        let update_timer_store = update_timer_store.clone();
        update_timer.on_tick(move |_| {
            let update = result.lock().ok().and_then(|mut slot| slot.take());
            let Some(update) = update else {
                return;
            };

            if let Some(timer) = update_timer_store.borrow().as_ref() {
                timer.stop();
            }
            in_flight.store(false, Ordering::SeqCst);
            show_update_check_result(&frame, update);
        });
    }
    update_timer.start(100, false);
    update_timer_store.borrow_mut().replace(update_timer);
    gui_timers.track(&update_timer_store);
}

fn check_for_updates() -> Result<UpdateCheckOutcome, String> {
    let client = Client::builder()
        .connect_timeout(UPDATE_CHECK_TIMEOUT)
        .timeout(UPDATE_CHECK_TIMEOUT)
        .build()
        .map_err(|err| format!("创建更新检查客户端失败：{err}"))?;

    let release = fetch_update_manifest(&client).or_else(|manifest_err| {
        fetch_github_latest_release(&client).map_err(|api_err| {
            format!(
                "无法读取 GitHub Release 更新信息：{api_err}\nlatest.json 检查结果：{manifest_err}"
            )
        })
    })?;
    build_update_check_outcome(release)
}

fn fetch_update_manifest(client: &Client) -> Result<LatestReleaseInfo, String> {
    let text = fetch_update_text(client, UPDATE_MANIFEST_URL)?;
    let manifest: UpdateManifest =
        serde_json::from_str(&text).map_err(|err| format!("latest.json 无法解析：{err}"))?;
    let release_url = manifest
        .release_url
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| UPDATE_RELEASE_PAGE_URL.to_string());
    Ok(LatestReleaseInfo {
        version: manifest.version,
        release_url,
        notes: manifest.notes,
    })
}

fn fetch_github_latest_release(client: &Client) -> Result<LatestReleaseInfo, String> {
    let text = fetch_update_text(client, UPDATE_RELEASE_API_URL)?;
    let release: GitHubRelease =
        serde_json::from_str(&text).map_err(|err| format!("GitHub Release API 无法解析：{err}"))?;
    Ok(LatestReleaseInfo {
        version: release.tag_name,
        release_url: release.html_url,
        notes: release.body,
    })
}

fn fetch_update_text(client: &Client, url: &str) -> Result<String, String> {
    let response = client
        .get(url)
        .header("User-Agent", "codex-remote")
        .header("Accept", "application/json")
        .send()
        .map_err(|err| {
            if err.is_timeout() {
                format!("{url} 请求超时：{err}")
            } else {
                format!("{url} 请求失败：{err}")
            }
        })?;
    let status = response.status();
    let text = response.text().map_err(|err| err.to_string())?;
    if status.is_success() {
        Ok(text)
    } else {
        Err(format!("{url} 返回 HTTP {status}: {text}"))
    }
}

fn build_update_check_outcome(release: LatestReleaseInfo) -> Result<UpdateCheckOutcome, String> {
    let current_version = env!("CARGO_PKG_VERSION").to_string();
    let latest_version = release.version.trim().to_string();
    if latest_version.is_empty() {
        return Err("GitHub Release 没有版本号。".to_string());
    }

    if is_version_newer(&latest_version, &current_version)? {
        Ok(UpdateCheckOutcome::Newer {
            current_version,
            latest_version,
            release_url: release.release_url,
            notes: release.notes,
        })
    } else {
        Ok(UpdateCheckOutcome::Current {
            current_version,
            latest_version,
        })
    }
}

fn show_update_check_result(parent: &Frame, result: Result<UpdateCheckOutcome, String>) {
    match result {
        Ok(UpdateCheckOutcome::Current {
            current_version,
            latest_version,
        }) => {
            show_info(
                parent,
                &format!(
                    "已是最新版本。\n当前版本：{current_version}\nGitHub 最新版本：{latest_version}"
                ),
            );
        }
        Ok(UpdateCheckOutcome::Newer {
            current_version,
            latest_version,
            release_url,
            notes,
        }) => {
            let notes = update_notes_for_dialog(notes.as_deref());
            let message = format!(
                "发现新版本。\n当前版本：{current_version}\n最新版本：{latest_version}\n\n{notes}\n\n是否打开 GitHub Releases 下载？"
            );
            if confirm_open_update_release(parent, &message) {
                if let Err(err) = open_url_in_browser(&release_url) {
                    show_error(parent, &err);
                }
            }
        }
        Err(err) => {
            show_error(parent, &format!("检查更新失败：{err}"));
        }
    }
}

fn update_notes_for_dialog(notes: Option<&str>) -> String {
    let notes = notes.unwrap_or_default().trim();
    if notes.is_empty() {
        return "Release 页面包含安装包和更新说明。".to_string();
    }
    format!("更新说明：\n{}", truncate_for_dialog(notes, 700))
}

fn truncate_for_dialog(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let mut result = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        result.push_str("\n...");
    }
    result
}

fn is_version_newer(latest: &str, current: &str) -> Result<bool, String> {
    let latest = parse_version_segments(latest)?;
    let current = parse_version_segments(current)?;
    for index in 0..latest.len().max(current.len()) {
        let latest_segment = latest.get(index).copied().unwrap_or_default();
        let current_segment = current.get(index).copied().unwrap_or_default();
        if latest_segment != current_segment {
            return Ok(latest_segment > current_segment);
        }
    }
    Ok(false)
}

fn parse_version_segments(version: &str) -> Result<Vec<u64>, String> {
    let normalized = version
        .trim()
        .trim_start_matches('v')
        .trim_start_matches('V')
        .split(['-', '+'])
        .next()
        .unwrap_or_default();
    let segments = normalized
        .split('.')
        .map(|segment| {
            segment
                .parse::<u64>()
                .map_err(|_| format!("版本号 {version} 无法比较。"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if segments.is_empty() {
        Err(format!("版本号 {version} 无法比较。"))
    } else {
        Ok(segments)
    }
}

#[cfg(test)]
mod update_tests {
    use super::*;

    #[test]
    fn compares_release_versions() {
        assert!(is_version_newer("v0.2.6", "0.2.5").unwrap());
        assert!(is_version_newer("0.3.0", "0.2.99").unwrap());
        assert!(!is_version_newer("v0.2.5", "0.2.5").unwrap());
        assert!(!is_version_newer("v0.2.4", "0.2.5").unwrap());
        assert!(!is_version_newer("v0.2.5-beta.1", "0.2.5").unwrap());
    }
}

fn open_url_in_browser(url: &str) -> Result<(), String> {
    let url = url.trim();
    if url.is_empty() {
        return Err("下载地址为空。".to_string());
    }

    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("cmd");
        command.args(["/C", "start", "", url]);
        hide_command_window(&mut command);
        command
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("open");
        command.arg(url);
        command
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut command = Command::new("xdg-open");
        command.arg(url);
        command
    };

    command
        .spawn()
        .map(|_| ())
        .map_err(|err| format!("无法打开浏览器：{err}\n下载地址：{url}"))
}
