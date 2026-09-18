//! Extracted verbatim from `progress.rs`; no behavior changes.

use super::*;

pub(crate) fn diff_summary_from_item(item: &Value) -> Option<TelegramDiffSummary> {
    let changes = item.get("changes").and_then(Value::as_array)?;
    let mut files = Vec::new();
    let mut additions = 0usize;
    let mut deletions = 0usize;
    for change in changes {
        let path = change
            .get("path")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .unwrap_or("unknown");
        let move_path = change
            .get("kind")
            .and_then(|kind| kind.get("move_path"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|path| !path.is_empty());
        let display_path = match move_path {
            Some(move_path) if move_path != path => format!("{path} -> {move_path}"),
            _ => path.to_string(),
        };
        let (added, removed) = diff_change_stats(change);
        additions = additions.saturating_add(added);
        deletions = deletions.saturating_add(removed);
        push_unique_diff_file(
            &mut files,
            TelegramDiffFileSummary {
                path: display_path,
                additions: added,
                deletions: removed,
            },
        );
    }
    let file_count = changes.len().max(files.len());
    let paths: Vec<String> = files.iter().map(|file| file.path.clone()).collect();
    let omitted_paths = file_count.saturating_sub(paths.len());
    (file_count > 0).then_some(TelegramDiffSummary {
        file_count,
        additions,
        deletions,
        files,
        paths,
        omitted_paths,
    })
}

pub(crate) fn file_change_diff_summary(item: &Value) -> Option<TelegramDiffSummary> {
    diff_summary_from_item(item)
}

pub(crate) fn diff_summary_from_diff(diff: &str) -> Option<TelegramDiffSummary> {
    if diff.trim().is_empty() {
        return None;
    }
    let mut files = Vec::new();
    let mut file_count = 0usize;
    let mut current_path = None;
    let mut current_additions = 0usize;
    let mut current_deletions = 0usize;
    let mut pending_old_path = None;
    let mut additions = 0usize;
    let mut deletions = 0usize;
    let mut current_has_header = false;
    let mut in_hunk = false;
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            finish_diff_file(
                &mut files,
                &mut current_path,
                &mut current_additions,
                &mut current_deletions,
            );
            file_count = file_count.saturating_add(1);
            current_path = diff_git_target_path(rest);
            current_has_header = false;
            pending_old_path = None;
            in_hunk = false;
        } else if let Some(rest) = line.strip_prefix("--- ") {
            if current_has_header {
                finish_diff_file(
                    &mut files,
                    &mut current_path,
                    &mut current_additions,
                    &mut current_deletions,
                );
                file_count = file_count.saturating_add(1);
                current_has_header = false;
                in_hunk = false;
            }
            pending_old_path = diff_header_path(rest, 'a');
            if pending_old_path.as_deref() == Some("/dev/null") {
                pending_old_path = None;
            }
            if file_count == 0 {
                file_count = file_count.saturating_add(1);
            }
        } else if let Some(rest) = line.strip_prefix("+++ ") {
            let new_path = diff_header_path(rest, 'b');
            let path = new_path
                .filter(|path| path != "/dev/null")
                .or(pending_old_path.take());
            if let Some(path) = path.filter(|path| path != "/dev/null") {
                current_path = Some(path);
            }
            current_has_header = true;
        } else if line.starts_with("@@") {
            in_hunk = true;
        } else if let Some(rest) = line.strip_prefix("rename to ") {
            current_path = Some(rest.trim().to_string());
        } else if in_hunk && line.starts_with('+') {
            current_additions = current_additions.saturating_add(1);
            additions = additions.saturating_add(1);
        } else if in_hunk && line.starts_with('-') {
            current_deletions = current_deletions.saturating_add(1);
            deletions = deletions.saturating_add(1);
        }
    }
    finish_diff_file(
        &mut files,
        &mut current_path,
        &mut current_additions,
        &mut current_deletions,
    );
    file_count = file_count.max(files.len());
    let paths: Vec<String> = files.iter().map(|file| file.path.clone()).collect();
    let omitted_paths = file_count.saturating_sub(paths.len());
    (file_count > 0).then_some(TelegramDiffSummary {
        file_count,
        additions,
        deletions,
        files,
        paths,
        omitted_paths,
    })
}

pub(super) fn diff_line_stats(diff: &str) -> (usize, usize) {
    let mut in_hunk = false;
    let mut additions = 0usize;
    let mut deletions = 0usize;
    for line in diff.lines() {
        if line.starts_with("@@") {
            in_hunk = true;
        } else if in_hunk && line.starts_with('+') {
            additions = additions.saturating_add(1);
        } else if in_hunk && line.starts_with('-') {
            deletions = deletions.saturating_add(1);
        }
    }
    (additions, deletions)
}

pub(super) fn diff_change_stats(change: &Value) -> (usize, usize) {
    let diff = change
        .get("diff")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let kind = change
        .get("kind")
        .and_then(|kind| {
            kind.get("type")
                .and_then(Value::as_str)
                .or_else(|| kind.as_str())
        })
        .or_else(|| change.get("type").and_then(Value::as_str))
        .unwrap_or("change")
        .trim()
        .to_ascii_lowercase();
    match kind.as_str() {
        "add" | "added" | "create" | "created" => (count_text_lines(diff), 0),
        "delete" | "deleted" | "remove" | "removed" => (0, count_text_lines(diff)),
        _ => diff_line_stats(diff),
    }
}

pub(super) fn count_text_lines(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    let normalized = text.replace("\r\n", "\n");
    let lines = normalized.split('\n').collect::<Vec<_>>();
    if lines.last() == Some(&"") {
        lines.len().saturating_sub(1)
    } else {
        lines.len()
    }
}

pub(super) fn diff_git_target_path(rest: &str) -> Option<String> {
    let index = rest.rfind(" b/")?;
    Some(rest[index + 3..].trim_matches('"').to_string())
}

pub(super) fn diff_header_path(rest: &str, prefix: char) -> Option<String> {
    let raw = rest.split('\t').next()?.trim().trim_matches('"');
    let prefix = format!("{prefix}/");
    Some(raw.strip_prefix(&prefix).unwrap_or(raw).to_string())
}

pub(super) fn finish_diff_file(
    files: &mut Vec<TelegramDiffFileSummary>,
    current_path: &mut Option<String>,
    additions: &mut usize,
    deletions: &mut usize,
) {
    if let Some(path) = current_path.take() {
        push_unique_diff_file(
            files,
            TelegramDiffFileSummary {
                path,
                additions: *additions,
                deletions: *deletions,
            },
        );
    }
    *additions = 0;
    *deletions = 0;
}

pub(super) fn push_unique_diff_file(
    files: &mut Vec<TelegramDiffFileSummary>,
    file: TelegramDiffFileSummary,
) {
    if let Some(existing) = files.iter_mut().find(|existing| existing.path == file.path) {
        existing.additions = existing.additions.saturating_add(file.additions);
        existing.deletions = existing.deletions.saturating_add(file.deletions);
    } else if files.len() < TELEGRAM_DIFF_MAX_PATHS {
        files.push(file);
    }
}

/// 完整颗粒度下的独立文件变更消息：标题行 + 逐文件增删行。
pub(crate) fn render_diff_standalone(diff: &TelegramDiffSummary, text: ImText) -> String {
    let mut lines =
        vec![text.telegram_diff_heading(diff.file_count, diff.additions, diff.deletions)];
    for file in diff.files.iter().take(TELEGRAM_PLAN_RENDER_STEPS) {
        lines.push(format!(
            "`{}` · +{} −{}",
            compact_text(&file.path, TELEGRAM_PLAN_STEP_CHARS),
            file.additions,
            file.deletions
        ));
    }
    if diff.files.len() > TELEGRAM_PLAN_RENDER_STEPS {
        lines.push(text.telegram_diff_omitted(diff.files.len() - TELEGRAM_PLAN_RENDER_STEPS));
    }
    lines.join("\n")
}

pub(super) fn diff_file_display_name(path: &str) -> String {
    if let Some((from, to)) = path.split_once(" -> ") {
        return format!("{} -> {}", file_name(from), file_name(to));
    }
    file_name(path).to_string()
}
