use serde_json::Value;

use crate::{
    im::core::i18n::ImText,
    im_runtime::{
        TelegramCommandProgressEntry, TelegramCommandProgressEntryKind,
        TelegramCommandProgressSnapshot, TelegramCommandProgressStatus, TelegramDiffFileSummary,
        TelegramDiffSummary, TelegramPlanStep, TelegramPlanStepStatus,
    },
};

use super::{collab_progress, rich_blocks};

const TELEGRAM_COMMAND_PROGRESS_VISIBLE_STEPS: usize = 3;
const TELEGRAM_COMMAND_PROGRESS_COMMAND_CHARS: usize = 180;
const TELEGRAM_COMMAND_PROGRESS_RICH_COMMAND_CHARS: usize = 56;
const TELEGRAM_COMMAND_PROGRESS_FAILURE_CHARS: usize = 480;
const TELEGRAM_COMMAND_PROGRESS_FAILURE_LINES: usize = 6;
const TELEGRAM_COMMAND_PROGRESS_RETRY_ERROR_CHARS: usize = 600;
const TELEGRAM_REASONING_RENDER_CHARS: usize = 720;
const TELEGRAM_PLAN_RENDER_STEPS: usize = 6;
const TELEGRAM_PLAN_STEP_CHARS: usize = 180;
const TELEGRAM_DIFF_RENDER_PATHS: usize = 8;
const TELEGRAM_DIFF_PATH_CHARS: usize = 180;
const TELEGRAM_DIFF_TABLE_PATH_CHARS: usize = 48;
const TELEGRAM_DIFF_MAX_PATHS: usize = 128;
const TELEGRAM_COMMAND_PROGRESS_DETAILS_STEPS: usize = 12;
const TELEGRAM_WEB_SEARCH_VISIBLE_ENTRIES: usize = 2;
const TELEGRAM_WEB_SEARCH_HISTORY_ENTRIES: usize = 8;
const TELEGRAM_WEB_SEARCH_SUMMARY_CHARS: usize = 140;
const TELEGRAM_WEB_SEARCH_FALLBACK_CHARS: usize = 900;
const TELEGRAM_TASK_PROGRESS_FALLBACK_MAX_CHARS: usize = 3_800;
pub(crate) const TELEGRAM_COMMAND_PROGRESS_MAX_CHARS: usize = 3_600;

#[derive(Debug, Clone)]
pub(crate) struct TelegramTaskProgressRender {
    pub blocks: Vec<Value>,
    pub fallback_markdown: String,
}

pub(crate) fn reasoning_summary_from_item(item: &Value) -> Option<String> {
    for key in ["summary", "content"] {
        let Some(values) = item.get(key).and_then(Value::as_array) else {
            continue;
        };
        if let Some(text) = values.iter().rev().find_map(|value| {
            value
                .get("text")
                .and_then(Value::as_str)
                .or_else(|| value.as_str())
                .map(str::trim)
                .filter(|text| !text.is_empty())
        }) {
            return Some(text.to_string());
        }
    }
    None
}

pub(crate) fn plan_from_params(params: &Value) -> (Option<String>, Vec<TelegramPlanStep>) {
    let explanation = params
        .get("explanation")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let steps = params
        .get("plan")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| {
            let step = value
                .get("step")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())?;
            let status = match value.get("status").and_then(Value::as_str) {
                Some("completed" | "done") => TelegramPlanStepStatus::Completed,
                Some("inProgress" | "in_progress") => TelegramPlanStepStatus::InProgress,
                _ => TelegramPlanStepStatus::Pending,
            };
            Some(TelegramPlanStep {
                step: truncate_middle(
                    &single_line(step).replace('`', "'"),
                    TELEGRAM_PLAN_STEP_CHARS,
                ),
                status,
            })
        })
        .collect();
    (explanation, steps)
}

pub(crate) fn parse_plan_update(params: &Value) -> (Option<String>, Vec<TelegramPlanStep>) {
    plan_from_params(params)
}

pub(crate) fn plan_from_item(item: &Value) -> (Option<String>, Vec<TelegramPlanStep>) {
    let explanation = item
        .get("text")
        .and_then(Value::as_str)
        .or_else(|| item.get("summary").and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    (explanation, Vec::new())
}

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
    (file_count > 0).then(|| TelegramDiffSummary {
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
    (file_count > 0).then(|| TelegramDiffSummary {
        file_count,
        additions,
        deletions,
        files,
        paths,
        omitted_paths,
    })
}

fn diff_line_stats(diff: &str) -> (usize, usize) {
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

fn diff_change_stats(change: &Value) -> (usize, usize) {
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

fn count_text_lines(text: &str) -> usize {
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

fn diff_git_target_path(rest: &str) -> Option<String> {
    let index = rest.rfind(" b/")?;
    Some(rest[index + 3..].trim_matches('"').to_string())
}

fn diff_header_path(rest: &str, prefix: char) -> Option<String> {
    let raw = rest.split('\t').next()?.trim().trim_matches('"');
    let prefix = format!("{prefix}/");
    Some(raw.strip_prefix(&prefix).unwrap_or(raw).to_string())
}

fn finish_diff_file(
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

fn push_unique_diff_file(files: &mut Vec<TelegramDiffFileSummary>, file: TelegramDiffFileSummary) {
    if let Some(existing) = files.iter_mut().find(|existing| existing.path == file.path) {
        existing.additions = existing.additions.saturating_add(file.additions);
        existing.deletions = existing.deletions.saturating_add(file.deletions);
    } else if files.len() < TELEGRAM_DIFF_MAX_PATHS {
        files.push(file);
    }
}

pub(crate) fn running_entry(item_id: &str, item: &Value) -> TelegramCommandProgressEntry {
    TelegramCommandProgressEntry {
        item_id: item_id.to_string(),
        kind: TelegramCommandProgressEntryKind::Command,
        command: command_text(item),
        status: TelegramCommandProgressStatus::Running,
        exit_code: None,
        duration_ms: item.get("durationMs").and_then(Value::as_u64),
        failure_output: None,
    }
}

pub(crate) fn completed_entry(item_id: &str, item: &Value) -> TelegramCommandProgressEntry {
    let status = completed_status(item);
    TelegramCommandProgressEntry {
        item_id: item_id.to_string(),
        kind: TelegramCommandProgressEntryKind::Command,
        command: command_text(item),
        status,
        exit_code: item.get("exitCode").and_then(Value::as_i64),
        duration_ms: item.get("durationMs").and_then(Value::as_u64),
        failure_output: (status == TelegramCommandProgressStatus::Failed)
            .then(|| failure_output_tail(item))
            .flatten(),
    }
}

pub(crate) fn mcp_running_entry(item_id: &str, item: &Value) -> TelegramCommandProgressEntry {
    TelegramCommandProgressEntry {
        item_id: item_id.to_string(),
        kind: TelegramCommandProgressEntryKind::McpTool,
        command: mcp_tool_text(item),
        status: TelegramCommandProgressStatus::Running,
        exit_code: None,
        duration_ms: item.get("durationMs").and_then(Value::as_u64),
        failure_output: None,
    }
}

pub(crate) fn mcp_completed_entry(item_id: &str, item: &Value) -> TelegramCommandProgressEntry {
    let status = mcp_completed_status(item);
    TelegramCommandProgressEntry {
        item_id: item_id.to_string(),
        kind: TelegramCommandProgressEntryKind::McpTool,
        command: mcp_tool_text(item),
        status,
        exit_code: None,
        duration_ms: item.get("durationMs").and_then(Value::as_u64),
        failure_output: (status == TelegramCommandProgressStatus::Failed)
            .then(|| mcp_failure_output(item))
            .flatten(),
    }
}

pub(crate) fn render_command_progress(
    snapshot: &TelegramCommandProgressSnapshot,
    text: ImText,
) -> String {
    let mut selected = selected_entry_indices(&snapshot.entries);
    let mut failure_output_chars = TELEGRAM_COMMAND_PROGRESS_FAILURE_CHARS;
    let mut retry_error_chars = TELEGRAM_COMMAND_PROGRESS_RETRY_ERROR_CHARS;

    loop {
        let rendered = render_command_progress_with_limits(
            snapshot,
            text,
            &selected,
            failure_output_chars,
            retry_error_chars,
        );
        if rendered.chars().count() <= TELEGRAM_COMMAND_PROGRESS_MAX_CHARS {
            return rendered;
        }

        let has_failure_output = selected.iter().any(|index| {
            snapshot.entries[*index]
                .failure_output
                .as_deref()
                .is_some_and(|output| !output.is_empty())
        });
        if failure_output_chars > 0 && has_failure_output {
            let without_failure_output = render_command_progress_with_limits(
                snapshot,
                text,
                &selected,
                0,
                retry_error_chars,
            );
            if without_failure_output.chars().count() <= TELEGRAM_COMMAND_PROGRESS_MAX_CHARS {
                let mut lower = 0;
                let mut upper = failure_output_chars;
                while lower < upper {
                    let candidate = lower + (upper - lower).div_ceil(2);
                    let candidate_rendered = render_command_progress_with_limits(
                        snapshot,
                        text,
                        &selected,
                        candidate,
                        retry_error_chars,
                    );
                    if candidate_rendered.chars().count() <= TELEGRAM_COMMAND_PROGRESS_MAX_CHARS {
                        lower = candidate;
                    } else {
                        upper = candidate - 1;
                    }
                }
                return render_command_progress_with_limits(
                    snapshot,
                    text,
                    &selected,
                    lower,
                    retry_error_chars,
                );
            }
            failure_output_chars = 0;
        }

        if let Some(position) = least_important_entry_position(snapshot, &selected) {
            selected.remove(position);
            failure_output_chars = TELEGRAM_COMMAND_PROGRESS_FAILURE_CHARS;
            continue;
        }

        if retry_error_chars > 0 && snapshot.retry_count > 0 && snapshot.retry_error.is_some() {
            let without_retry_error =
                render_command_progress_with_limits(snapshot, text, &selected, 0, 0);
            if without_retry_error.chars().count() <= TELEGRAM_COMMAND_PROGRESS_MAX_CHARS {
                let mut lower = 0;
                let mut upper = retry_error_chars;
                while lower < upper {
                    let candidate = lower + (upper - lower).div_ceil(2);
                    let candidate_rendered = render_command_progress_with_limits(
                        snapshot, text, &selected, 0, candidate,
                    );
                    if candidate_rendered.chars().count() <= TELEGRAM_COMMAND_PROGRESS_MAX_CHARS {
                        lower = candidate;
                    } else {
                        upper = candidate - 1;
                    }
                }
                return render_command_progress_with_limits(snapshot, text, &selected, 0, lower);
            }
            retry_error_chars = 0;
            continue;
        }

        return truncate_middle(&rendered, TELEGRAM_COMMAND_PROGRESS_MAX_CHARS);
    }
}

pub(crate) fn render_task_progress(
    snapshot: &TelegramCommandProgressSnapshot,
    text: ImText,
) -> TelegramTaskProgressRender {
    let command_fallback = render_command_progress(snapshot, text);
    let fallback_markdown = match snapshot.collab.as_ref() {
        Some(collab) => {
            let rendered_collab = collab_progress::render_collab_progress(collab, text);
            let combined = format!("{command_fallback}\n\n{rendered_collab}");
            if combined.chars().count() <= TELEGRAM_TASK_PROGRESS_FALLBACK_MAX_CHARS {
                combined
            } else {
                let summary = rendered_collab.lines().next().unwrap_or_default();
                let combined = format!("{command_fallback}\n\n{summary}");
                if combined.chars().count() <= TELEGRAM_TASK_PROGRESS_FALLBACK_MAX_CHARS {
                    combined
                } else {
                    command_fallback
                }
            }
        }
        None => command_fallback,
    };
    TelegramTaskProgressRender {
        blocks: render_task_progress_blocks(snapshot, text),
        fallback_markdown,
    }
}

fn render_task_progress_blocks(
    snapshot: &TelegramCommandProgressSnapshot,
    text: ImText,
) -> Vec<Value> {
    let mut blocks = vec![rich_blocks::heading(
        rich_blocks::text(command_progress_title(snapshot, text)),
        3,
    )];

    if snapshot.retry_count > 0 {
        blocks.push(rich_blocks::paragraph(rich_blocks::text(
            text.telegram_retry_progress_summary(snapshot.retry_count),
        )));
    }

    if snapshot.plan_explanation.is_some() || !snapshot.plan.is_empty() {
        let completed = snapshot
            .plan
            .iter()
            .filter(|step| step.status == TelegramPlanStepStatus::Completed)
            .count();
        blocks.push(rich_blocks::paragraph(rich_blocks::bold(
            text.telegram_plan_heading(completed, snapshot.plan.len()),
        )));
        if let Some(explanation) = snapshot.plan_explanation.as_deref() {
            blocks.push(rich_blocks::paragraph(rich_blocks::text(compact_text(
                explanation,
                TELEGRAM_PLAN_STEP_CHARS,
            ))));
        }
        let items = snapshot
            .plan
            .iter()
            .take(TELEGRAM_PLAN_RENDER_STEPS)
            .map(|step| {
                let mut line = Vec::new();
                if step.status == TelegramPlanStepStatus::InProgress {
                    line.push(rich_blocks::bold(
                        text.telegram_progress_status_label("in_progress"),
                    ));
                    line.push(rich_blocks::text(" · "));
                }
                line.push(rich_blocks::text(compact_text(
                    &step.step,
                    TELEGRAM_PLAN_STEP_CHARS,
                )));
                rich_blocks::checklist_item(
                    vec![rich_blocks::paragraph(rich_blocks::rich_text(line))],
                    step.status == TelegramPlanStepStatus::Completed,
                )
            })
            .collect::<Vec<_>>();
        if !items.is_empty() {
            blocks.push(rich_blocks::list(items));
        }
        if snapshot.plan.len() > TELEGRAM_PLAN_RENDER_STEPS {
            blocks.push(rich_blocks::paragraph(rich_blocks::text(
                text.telegram_plan_omitted(snapshot.plan.len() - TELEGRAM_PLAN_RENDER_STEPS),
            )));
        }
    }

    let selected = selected_entry_indices(&snapshot.entries);
    let command_total = snapshot
        .dropped_entries
        .saturating_add(snapshot.entries.len());
    if has_plan_progress(snapshot) && command_total > 0 {
        blocks.push(rich_blocks::divider());
        blocks.push(rich_blocks::paragraph(rich_blocks::bold(
            command_execution_progress_title(snapshot, text),
        )));
    }
    if !selected.is_empty() {
        for index in &selected {
            blocks.extend(rich_command_entry_blocks(&snapshot.entries[*index], text));
        }
    }

    let hidden = (0..snapshot.entries.len())
        .filter(|index| !selected.contains(index))
        .take(TELEGRAM_COMMAND_PROGRESS_DETAILS_STEPS)
        .collect::<Vec<_>>();
    if !hidden.is_empty() || snapshot.dropped_entries > 0 {
        let omitted = snapshot
            .dropped_entries
            .saturating_add(snapshot.entries.len().saturating_sub(selected.len()));
        let mut hidden_blocks = hidden
            .iter()
            .flat_map(|index| rich_command_entry_blocks(&snapshot.entries[*index], text))
            .collect::<Vec<_>>();
        let still_hidden = omitted.saturating_sub(hidden.len());
        if still_hidden > 0 {
            hidden_blocks.push(rich_blocks::paragraph(rich_blocks::text(
                text.telegram_command_progress_omitted(still_hidden),
            )));
        }
        blocks.push(rich_blocks::details(
            rich_blocks::text(text.telegram_command_progress_omitted(omitted)),
            hidden_blocks,
            false,
        ));
    }

    for index in &selected {
        let entry = &snapshot.entries[*index];
        if let Some(output) = entry.failure_output.as_deref() {
            blocks.push(rich_blocks::details(
                rich_blocks::rich_text(vec![
                    rich_blocks::text(format!(
                        "{} ",
                        text.telegram_command_progress_error_summary()
                            .trim_end_matches([':', '：'])
                    )),
                    rich_blocks::code(truncate_middle(
                        &entry.command,
                        TELEGRAM_COMMAND_PROGRESS_RICH_COMMAND_CHARS,
                    )),
                ]),
                vec![rich_blocks::preformatted(
                    truncate_tail(output, TELEGRAM_COMMAND_PROGRESS_FAILURE_CHARS),
                    Some("text"),
                )],
                false,
            ));
        }
    }

    blocks.extend(render_web_search_progress_blocks(snapshot));
    if let Some(collab) = snapshot.collab.as_ref() {
        blocks.push(collab_progress::render_collab_progress_details(
            collab, text,
        ));
    }
    if let Some(reasoning) = snapshot.reasoning_summary.as_deref() {
        blocks.push(rich_blocks::paragraph(rich_blocks::bold(
            text.telegram_reasoning_heading(),
        )));
        blocks.push(rich_blocks::paragraph(rich_blocks::inline_markdown(
            &compact_text(reasoning, TELEGRAM_REASONING_RENDER_CHARS),
        )));
    }
    if let Some(diff) = snapshot.diff_summary.as_ref() {
        let mut rows = vec![vec![
            rich_blocks::table_cell(
                rich_blocks::text(text.telegram_diff_table_file()),
                true,
                "left",
            ),
            rich_blocks::table_cell(
                rich_blocks::text(text.telegram_diff_table_additions()),
                true,
                "right",
            ),
            rich_blocks::table_cell(
                rich_blocks::text(text.telegram_diff_table_deletions()),
                true,
                "right",
            ),
        ]];
        if diff.files.is_empty() {
            for path in diff.paths.iter().take(TELEGRAM_DIFF_RENDER_PATHS) {
                let file_name = diff_file_display_name(path);
                rows.push(vec![
                    rich_blocks::table_cell(
                        rich_blocks::code(compact_text(&file_name, TELEGRAM_DIFF_TABLE_PATH_CHARS)),
                        false,
                        "left",
                    ),
                    rich_blocks::table_cell(rich_blocks::text("+0"), false, "right"),
                    rich_blocks::table_cell(rich_blocks::text("-0"), false, "right"),
                ]);
            }
        } else {
            for file in diff.files.iter().take(TELEGRAM_DIFF_RENDER_PATHS) {
                let file_name = diff_file_display_name(&file.path);
                rows.push(vec![
                    rich_blocks::table_cell(
                        rich_blocks::code(compact_text(&file_name, TELEGRAM_DIFF_TABLE_PATH_CHARS)),
                        false,
                        "left",
                    ),
                    rich_blocks::table_cell(
                        rich_blocks::text(format!("+{}", file.additions)),
                        false,
                        "right",
                    ),
                    rich_blocks::table_cell(
                        rich_blocks::text(format!("-{}", file.deletions)),
                        false,
                        "right",
                    ),
                ]);
            }
        }
        let visible_files = if diff.files.is_empty() {
            diff.paths.len()
        } else {
            diff.files.len()
        };
        let omitted = diff
            .omitted_paths
            .saturating_add(visible_files.saturating_sub(TELEGRAM_DIFF_RENDER_PATHS));
        blocks.push(rich_blocks::details(
            rich_blocks::text(text.telegram_diff_heading(
                diff.file_count,
                diff.additions,
                diff.deletions,
            )),
            {
                let mut details_blocks = vec![rich_blocks::table(rows, true, true)];
                if omitted > 0 {
                    details_blocks.push(rich_blocks::paragraph(rich_blocks::text(
                        text.telegram_diff_omitted(omitted),
                    )));
                }
                details_blocks
            },
            false,
        ));
    }
    if snapshot.retry_count > 0
        && let Some(error) = snapshot.retry_error.as_deref()
    {
        blocks.push(rich_blocks::details(
            rich_blocks::text(
                text.telegram_retry_error_summary()
                    .trim_end_matches([':', '：']),
            ),
            vec![rich_blocks::preformatted(
                truncate_middle(error, TELEGRAM_COMMAND_PROGRESS_RETRY_ERROR_CHARS),
                Some("text"),
            )],
            false,
        ));
    }

    blocks.push(rich_blocks::footer(rich_blocks::rich_text(vec![
        rich_blocks::text("turn "),
        rich_blocks::code(short_identifier(&snapshot.turn_id)),
    ])));
    blocks
}

fn render_web_search_progress_blocks(snapshot: &TelegramCommandProgressSnapshot) -> Vec<Value> {
    let total = snapshot
        .dropped_web_searches
        .saturating_add(snapshot.web_searches.len());
    if total == 0 {
        return Vec::new();
    }

    let visible_start = snapshot
        .web_searches
        .len()
        .saturating_sub(TELEGRAM_WEB_SEARCH_VISIBLE_ENTRIES);
    let hidden = &snapshot.web_searches[..visible_start];
    let visible = &snapshot.web_searches[visible_start..];
    let earlier_count = snapshot.dropped_web_searches.saturating_add(hidden.len());
    let mut blocks = vec![rich_blocks::paragraph(rich_blocks::bold(format!(
        "搜索 · {total} 次"
    )))];

    if earlier_count > 0 {
        let retained_start = hidden
            .len()
            .saturating_sub(TELEGRAM_WEB_SEARCH_HISTORY_ENTRIES);
        let retained = &hidden[retained_start..];
        let omitted = earlier_count.saturating_sub(retained.len());
        let mut earlier_blocks = Vec::new();
        if omitted > 0 {
            earlier_blocks.push(rich_blocks::paragraph(rich_blocks::text(format!(
                "… 另外 {omitted} 次较早搜索已省略"
            ))));
        }
        earlier_blocks.extend(retained.iter().map(|entry| {
            rich_blocks::paragraph(rich_blocks::text(compact_text(
                &entry.summary,
                TELEGRAM_WEB_SEARCH_SUMMARY_CHARS,
            )))
        }));
        blocks.push(rich_blocks::details(
            rich_blocks::text(format!("较早搜索 · {earlier_count} 次")),
            earlier_blocks,
            false,
        ));
    }

    for entry in visible {
        let body = if entry.blocks.is_empty() {
            vec![rich_blocks::paragraph(rich_blocks::text(
                "未返回可显示结果",
            ))]
        } else {
            entry.blocks.clone()
        };
        blocks.push(rich_blocks::details(
            rich_blocks::text(compact_text(
                &entry.summary,
                TELEGRAM_WEB_SEARCH_SUMMARY_CHARS,
            )),
            body,
            false,
        ));
    }
    blocks
}

fn rich_command_entry_blocks(entry: &TelegramCommandProgressEntry, text: ImText) -> Vec<Value> {
    let command = truncate_middle(&entry.command, TELEGRAM_COMMAND_PROGRESS_RICH_COMMAND_CHARS);
    let language = match entry.kind {
        TelegramCommandProgressEntryKind::Command => "shell",
        TelegramCommandProgressEntryKind::McpTool => "text",
    };

    let mut metadata = Vec::new();
    if entry.kind == TelegramCommandProgressEntryKind::McpTool {
        metadata.push(rich_blocks::bold("MCP"));
        metadata.push(rich_blocks::text(" · "));
    }
    metadata.push(rich_blocks::bold(text.telegram_progress_status_label(
        command_progress_status_key(entry.status),
    )));
    if let Some(duration) = entry.duration_ms {
        metadata.push(rich_blocks::text(format!(
            " · {}",
            format_duration(duration)
        )));
    }
    if let Some(exit_code) = entry
        .exit_code
        .filter(|_| entry.status == TelegramCommandProgressStatus::Failed)
    {
        metadata.push(rich_blocks::text(format!(" · exit {exit_code}")));
    }
    vec![
        rich_blocks::preformatted(command, Some(language)),
        rich_blocks::footer(rich_blocks::rich_text(metadata)),
    ]
}

fn command_progress_status_key(status: TelegramCommandProgressStatus) -> &'static str {
    match status {
        TelegramCommandProgressStatus::Running => "running",
        TelegramCommandProgressStatus::Interrupted => "interrupted",
        TelegramCommandProgressStatus::Succeeded => "succeeded",
        TelegramCommandProgressStatus::Failed => "failed",
    }
}

fn short_identifier(value: &str) -> String {
    const MAX: usize = 8;
    let value = value.trim();
    if value.chars().count() <= MAX {
        return value.to_string();
    }
    format!("{}…", value.chars().take(MAX).collect::<String>())
}

fn render_command_progress_with_limits(
    snapshot: &TelegramCommandProgressSnapshot,
    text: ImText,
    selected: &[usize],
    failure_output_chars: usize,
    retry_error_chars: usize,
) -> String {
    let total = snapshot
        .dropped_entries
        .saturating_add(snapshot.entries.len());
    let omitted = total.saturating_sub(selected.len());
    let mut sections = vec![command_progress_title(snapshot, text)];
    if total > 0 && snapshot.retry_count > 0 {
        sections.push(text.telegram_retry_progress_summary(snapshot.retry_count));
    }
    if let Some(plan) = render_plan_progress(snapshot, text) {
        sections.push(plan);
    }
    if total > 0 && has_plan_progress(snapshot) {
        sections.push(command_execution_progress_title(snapshot, text));
    }
    if omitted > 0 {
        sections.push(text.telegram_command_progress_omitted(omitted));
    }
    for index in selected {
        sections.push(render_entry(
            &snapshot.entries[*index],
            text,
            failure_output_chars,
        ));
    }
    if let Some(supplemental) = render_supplemental_progress(snapshot, text) {
        sections.push(supplemental);
    }
    if snapshot.retry_count > 0
        && let Some(error) = snapshot.retry_error.as_deref()
        && retry_error_chars > 0
    {
        let error = truncate_middle(&error.replace("```", "'''"), retry_error_chars);
        sections.push(format!(
            "{}\n```text\n{}\n```",
            text.telegram_retry_error_summary(),
            error
        ));
    }
    sections.join("\n\n")
}

fn command_progress_title(snapshot: &TelegramCommandProgressSnapshot, text: ImText) -> String {
    let total = snapshot
        .dropped_entries
        .saturating_add(snapshot.entries.len());
    let has_supplemental = snapshot.reasoning_summary.is_some()
        || snapshot.plan_explanation.is_some()
        || !snapshot.plan.is_empty()
        || snapshot.diff_summary.is_some()
        || !snapshot.web_searches.is_empty()
        || snapshot.dropped_web_searches > 0
        || snapshot.collab.is_some();
    if total == 0 && snapshot.retry_count > 0 {
        text.telegram_retry_progress_title(
            snapshot.completed,
            snapshot.failed,
            snapshot.retry_count,
        )
    } else if total == 0 && has_supplemental {
        text.telegram_task_progress_title(snapshot.completed, snapshot.failed)
            .to_string()
    } else if has_plan_progress(snapshot) {
        text.telegram_task_progress_title(snapshot.completed, snapshot.failed)
            .to_string()
    } else {
        command_execution_progress_title(snapshot, text)
    }
}

fn has_plan_progress(snapshot: &TelegramCommandProgressSnapshot) -> bool {
    snapshot.plan_explanation.is_some() || !snapshot.plan.is_empty()
}

fn command_execution_progress_title(
    snapshot: &TelegramCommandProgressSnapshot,
    text: ImText,
) -> String {
    let total = snapshot
        .dropped_entries
        .saturating_add(snapshot.entries.len());
    let failed = snapshot
        .entries
        .iter()
        .filter(|entry| entry.status == TelegramCommandProgressStatus::Failed)
        .count();
    let running = snapshot
        .entries
        .iter()
        .filter(|entry| entry.status == TelegramCommandProgressStatus::Running)
        .count();
    let interrupted = snapshot
        .entries
        .iter()
        .filter(|entry| entry.status == TelegramCommandProgressStatus::Interrupted)
        .count();
    text.telegram_command_progress_title(
        snapshot.completed,
        snapshot.failed,
        total,
        failed,
        running,
        interrupted,
    )
}

fn render_supplemental_progress(
    snapshot: &TelegramCommandProgressSnapshot,
    text: ImText,
) -> Option<String> {
    let mut sections = Vec::new();
    if let Some(searches) = render_web_search_progress(snapshot) {
        sections.push(searches);
    }
    if let Some(reasoning) = snapshot.reasoning_summary.as_deref() {
        let reasoning = compact_text(reasoning, TELEGRAM_REASONING_RENDER_CHARS);
        sections.push(format!(
            "{}\n{}",
            text.telegram_reasoning_heading(),
            reasoning
        ));
    }
    if let Some(diff) = snapshot.diff_summary.as_ref() {
        let mut lines =
            vec![text.telegram_diff_heading(diff.file_count, diff.additions, diff.deletions)];
        for path in diff.paths.iter().take(TELEGRAM_DIFF_RENDER_PATHS) {
            let file_name = diff_file_display_name(path);
            lines.push(format!(
                "• {}",
                compact_text(&file_name, TELEGRAM_DIFF_PATH_CHARS)
            ));
        }
        let omitted = diff
            .omitted_paths
            .saturating_add(diff.paths.len().saturating_sub(TELEGRAM_DIFF_RENDER_PATHS));
        if omitted > 0 {
            lines.push(text.telegram_diff_omitted(omitted));
        }
        sections.push(lines.join("\n"));
    }
    (!sections.is_empty()).then(|| sections.join("\n\n"))
}

fn render_web_search_progress(snapshot: &TelegramCommandProgressSnapshot) -> Option<String> {
    let total = snapshot
        .dropped_web_searches
        .saturating_add(snapshot.web_searches.len());
    if total == 0 {
        return None;
    }
    let visible_start = snapshot
        .web_searches
        .len()
        .saturating_sub(TELEGRAM_WEB_SEARCH_VISIBLE_ENTRIES);
    let earlier_count = snapshot.dropped_web_searches.saturating_add(visible_start);
    let mut sections = vec![format!("搜索 · {total} 次")];
    if earlier_count > 0 {
        sections.push(format!("较早搜索 · {earlier_count} 次（已折叠）"));
    }
    sections.extend(
        snapshot.web_searches[visible_start..].iter().map(|entry| {
            compact_text(&entry.fallback_markdown, TELEGRAM_WEB_SEARCH_FALLBACK_CHARS)
        }),
    );
    Some(sections.join("\n\n"))
}

fn render_plan_progress(
    snapshot: &TelegramCommandProgressSnapshot,
    text: ImText,
) -> Option<String> {
    if !has_plan_progress(snapshot) {
        return None;
    }
    let mut sections = Vec::new();
    if snapshot.plan_explanation.is_some() || !snapshot.plan.is_empty() {
        let completed = snapshot
            .plan
            .iter()
            .filter(|step| step.status == TelegramPlanStepStatus::Completed)
            .count();
        let total = snapshot.plan.len();
        let mut lines = vec![text.telegram_plan_heading(completed, total)];
        if let Some(explanation) = snapshot.plan_explanation.as_deref() {
            lines.push(compact_text(explanation, TELEGRAM_PLAN_STEP_CHARS));
        }
        for step in snapshot.plan.iter().take(TELEGRAM_PLAN_RENDER_STEPS) {
            let status = match step.status {
                TelegramPlanStepStatus::Pending => "pending",
                TelegramPlanStepStatus::InProgress => "in_progress",
                TelegramPlanStepStatus::Completed => "completed",
            };
            lines.push(format!(
                "{} · {}",
                text.telegram_progress_status_label(status),
                compact_text(&step.step, TELEGRAM_PLAN_STEP_CHARS)
            ));
        }
        if snapshot.plan.len() > TELEGRAM_PLAN_RENDER_STEPS {
            lines
                .push(text.telegram_plan_omitted(snapshot.plan.len() - TELEGRAM_PLAN_RENDER_STEPS));
        }
        sections.push(lines.join("\n"));
    }
    (!sections.is_empty()).then(|| sections.join("\n\n"))
}

fn selected_entry_indices(entries: &[TelegramCommandProgressEntry]) -> Vec<usize> {
    let mut selected = Vec::new();
    for status in [
        TelegramCommandProgressStatus::Running,
        TelegramCommandProgressStatus::Failed,
        TelegramCommandProgressStatus::Interrupted,
    ] {
        for (index, entry) in entries.iter().enumerate().rev() {
            if entry.status == status && !selected.contains(&index) {
                selected.push(index);
                if selected.len() == TELEGRAM_COMMAND_PROGRESS_VISIBLE_STEPS {
                    selected.sort_unstable();
                    return selected;
                }
            }
        }
    }
    for index in (0..entries.len()).rev() {
        if !selected.contains(&index) {
            selected.push(index);
            if selected.len() == TELEGRAM_COMMAND_PROGRESS_VISIBLE_STEPS {
                break;
            }
        }
    }
    selected.sort_unstable();
    selected
}

fn least_important_entry_position(
    snapshot: &TelegramCommandProgressSnapshot,
    selected: &[usize],
) -> Option<usize> {
    selected
        .iter()
        .enumerate()
        .min_by_key(|(_, index)| {
            let priority = match snapshot.entries[**index].status {
                TelegramCommandProgressStatus::Succeeded => 0,
                TelegramCommandProgressStatus::Interrupted => 1,
                TelegramCommandProgressStatus::Failed => 2,
                TelegramCommandProgressStatus::Running => 3,
            };
            (priority, **index)
        })
        .map(|(position, _)| position)
}

fn render_entry(
    entry: &TelegramCommandProgressEntry,
    text: ImText,
    failure_output_chars: usize,
) -> String {
    let mut line = text
        .telegram_progress_status_label(command_progress_status_key(entry.status))
        .to_string();
    if entry.kind == TelegramCommandProgressEntryKind::McpTool {
        line.push_str(" · MCP");
    }
    if let Some(duration) = entry.duration_ms {
        line.push_str(" · ");
        line.push_str(&format_duration(duration));
    }
    if let Some(exit_code) = entry
        .exit_code
        .filter(|_| entry.status == TelegramCommandProgressStatus::Failed)
    {
        line.push_str(&format!(" · exit {exit_code}"));
    }
    match entry.kind {
        TelegramCommandProgressEntryKind::Command => {
            line.push_str("\n```shell\n");
            line.push_str(&entry.command);
            line.push_str("\n```");
        }
        TelegramCommandProgressEntryKind::McpTool => {
            line.push('\n');
            line.push_str(&entry.command);
        }
    }
    if let Some(output) = entry.failure_output.as_deref()
        && failure_output_chars > 0
    {
        line.push_str("\n");
        line.push_str(text.telegram_command_progress_error_summary());
        line.push_str("\n```text\n");
        line.push_str(&truncate_tail(output, failure_output_chars));
        line.push_str("\n```");
    }
    line
}

fn mcp_tool_text(item: &Value) -> String {
    let server = item
        .get("server")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let tool = item
        .get("tool")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let tool_name = match (server, tool) {
        (Some(server), Some(tool)) => format!("{server}.{tool}"),
        (Some(server), None) => server.to_string(),
        (None, Some(tool)) => tool.to_string(),
        (None, None) => "MCP tool".to_string(),
    };
    let title = item
        .get("arguments")
        .and_then(|arguments| arguments.get("title"))
        .and_then(Value::as_str)
        .or_else(|| item.get("title").and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != tool_name);
    let text = match title {
        Some(title) => format!("{tool_name} · {title}"),
        None => tool_name,
    };
    truncate_middle(
        &single_line(&text).replace('`', "'"),
        TELEGRAM_COMMAND_PROGRESS_COMMAND_CHARS,
    )
}

fn command_text(item: &Value) -> String {
    let text = item
        .get("commandActions")
        .and_then(Value::as_array)
        .and_then(|actions| actions.first())
        .and_then(|action| action.get("command"))
        .and_then(command_value_text)
        .or_else(|| item.get("command").and_then(command_value_text))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "command".to_string());
    truncate_middle(
        &single_line(&text).replace('`', "'"),
        TELEGRAM_COMMAND_PROGRESS_COMMAND_CHARS,
    )
}

fn command_value_text(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return Some(text.trim().to_string());
    }
    value.as_array().map(|parts| {
        parts
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    })
}

fn completed_status(item: &Value) -> TelegramCommandProgressStatus {
    if let Some(exit_code) = item.get("exitCode").and_then(Value::as_i64) {
        return if exit_code == 0 {
            TelegramCommandProgressStatus::Succeeded
        } else {
            TelegramCommandProgressStatus::Failed
        };
    }
    match item
        .get("status")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
    {
        "failed" | "error" | "canceled" | "cancelled" | "timed_out" | "timedout" => {
            TelegramCommandProgressStatus::Failed
        }
        _ => TelegramCommandProgressStatus::Succeeded,
    }
}

fn mcp_completed_status(item: &Value) -> TelegramCommandProgressStatus {
    if item.get("error").is_some_and(json_value_has_content)
        || item.get("isError").and_then(Value::as_bool) == Some(true)
        || item
            .get("result")
            .and_then(|result| result.get("isError").or_else(|| result.get("is_error")))
            .and_then(Value::as_bool)
            == Some(true)
    {
        return TelegramCommandProgressStatus::Failed;
    }
    completed_status(item)
}

fn mcp_failure_output(item: &Value) -> Option<String> {
    let error = item
        .get("error")
        .filter(|value| json_value_has_content(value))
        .or_else(|| {
            item.get("result")
                .and_then(|result| result.get("error"))
                .filter(|value| json_value_has_content(value))
        })
        .map(json_value_text)
        .or_else(|| {
            item.get("result")
                .and_then(|result| result.get("content"))
                .and_then(Value::as_array)
                .map(|content| {
                    content
                        .iter()
                        .filter_map(|entry| {
                            (entry.get("type").and_then(Value::as_str) == Some("text"))
                                .then(|| entry.get("text").and_then(Value::as_str))
                                .flatten()
                        })
                        .map(str::trim)
                        .filter(|text| !text.is_empty())
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .filter(|text| !text.is_empty())
        })?;
    Some(truncate_tail(
        &compact_text(&error, TELEGRAM_COMMAND_PROGRESS_FAILURE_CHARS).replace("```", "'''"),
        TELEGRAM_COMMAND_PROGRESS_FAILURE_CHARS,
    ))
}

fn json_value_has_content(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::String(value) => !value.trim().is_empty(),
        Value::Array(values) => !values.is_empty(),
        Value::Object(values) => !values.is_empty(),
        _ => true,
    }
}

fn json_value_text(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        return text.to_string();
    }
    for key in ["message", "additionalDetails", "details"] {
        if let Some(text) = value
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
        {
            return text.to_string();
        }
    }
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

fn failure_output_tail(item: &Value) -> Option<String> {
    let output = item.get("aggregatedOutput").and_then(Value::as_str)?.trim();
    if output.is_empty() {
        return None;
    }
    let lines = output.lines().collect::<Vec<_>>();
    let start = lines
        .len()
        .saturating_sub(TELEGRAM_COMMAND_PROGRESS_FAILURE_LINES);
    let tail = lines[start..].join("\n").replace("```", "'''");
    Some(truncate_tail(
        &tail,
        TELEGRAM_COMMAND_PROGRESS_FAILURE_CHARS,
    ))
}

fn single_line(text: &str) -> String {
    text.replace("\r\n", " ")
        .replace(['\r', '\n'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn compact_text(text: &str, max_chars: usize) -> String {
    let normalized = text
        .replace("```", "'''")
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let lines = normalized
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    truncate_middle(&lines.join("\n"), max_chars)
}

fn diff_file_display_name(path: &str) -> String {
    if let Some((from, to)) = path.split_once(" -> ") {
        return format!("{} -> {}", file_name(from), file_name(to));
    }
    file_name(path).to_string()
}

fn file_name(path: &str) -> &str {
    let trimmed = path.trim().trim_matches('"');
    trimmed
        .rsplit(|ch| ch == '/' || ch == '\\')
        .find(|part| !part.is_empty())
        .unwrap_or(trimmed)
}

fn truncate_middle(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    if max_chars <= 3 {
        return text.chars().take(max_chars).collect();
    }
    let head_len = max_chars.saturating_sub(3) / 2;
    let tail_len = max_chars.saturating_sub(3 + head_len);
    let head = text.chars().take(head_len).collect::<String>();
    let tail = text
        .chars()
        .rev()
        .take(tail_len)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("{head}...{tail}")
}

fn truncate_tail(text: &str, max_chars: usize) -> String {
    let chars = text.chars().collect::<Vec<_>>();
    if chars.len() <= max_chars {
        return text.to_string();
    }
    if max_chars <= 3 {
        return chars[chars.len().saturating_sub(max_chars)..]
            .iter()
            .collect();
    }
    format!(
        "...{}",
        chars[chars.len() - max_chars.saturating_sub(3)..]
            .iter()
            .collect::<String>()
    )
}

fn format_duration(duration_ms: u64) -> String {
    if duration_ms < 1_000 {
        format!("{duration_ms}ms")
    } else {
        format!("{:.1}s", duration_ms as f64 / 1_000.0)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::{
        im::core::i18n::ImText,
        im_runtime::{
            TelegramCollabProgressEntry, TelegramCollabProgressSnapshot,
            TelegramCollabProgressStatus, TelegramCommandProgressEntryKind,
            TelegramCommandProgressSnapshot, TelegramCommandProgressStatus,
        },
    };

    use super::{
        TELEGRAM_COMMAND_PROGRESS_MAX_CHARS, TELEGRAM_DIFF_TABLE_PATH_CHARS, completed_entry,
        diff_file_display_name, diff_summary_from_diff, file_change_diff_summary,
        mcp_completed_entry, mcp_running_entry, parse_plan_update, reasoning_summary_from_item,
        render_command_progress, render_task_progress, rich_command_entry_blocks, running_entry,
    };

    use crate::im_runtime::{
        TelegramDiffFileSummary, TelegramDiffSummary, TelegramPlanStep, TelegramPlanStepStatus,
        TelegramWebSearchProgressEntry,
    };

    #[test]
    fn parses_array_commands_and_keeps_failure_tail() {
        let item = json!({
            "commandActions": [{"command": ["cargo", "test", "--all"]}],
            "exitCode": 2,
            "durationMs": 1_250,
            "aggregatedOutput": "one\ntwo\nthree\nfour\nfive\nsix\nseven"
        });

        let entry = completed_entry("item-1", &item);

        assert_eq!(entry.command, "cargo test --all");
        assert_eq!(entry.status, TelegramCommandProgressStatus::Failed);
        assert_eq!(entry.duration_ms, Some(1_250));
        let output = entry.failure_output.expect("failure output");
        assert!(!output.contains("one"));
        assert!(output.contains("two"));
        assert!(output.contains("seven"));
    }

    #[test]
    fn mcp_entries_use_a_compact_tool_label() {
        let item = json!({
            "type": "mcpToolCall",
            "server": "browser",
            "tool": "screenshot",
            "arguments": {"title": "获取页面截图"},
            "status": "completed",
            "durationMs": 850
        });

        let running = mcp_running_entry("mcp-1", &item);
        assert_eq!(running.kind, TelegramCommandProgressEntryKind::McpTool);
        assert_eq!(running.command, "browser.screenshot · 获取页面截图");
        assert_eq!(running.status, TelegramCommandProgressStatus::Running);

        let completed = mcp_completed_entry("mcp-1", &item);
        assert_eq!(completed.status, TelegramCommandProgressStatus::Succeeded);
        assert_eq!(completed.duration_ms, Some(850));
    }

    #[test]
    fn render_mcp_entry_without_a_shell_code_block() {
        let rendered = render_command_progress(
            &TelegramCommandProgressSnapshot {
                turn_id: "turn".to_string(),
                revision: 1,
                message_id: None,
                entries: vec![mcp_running_entry(
                    "mcp-1",
                    &json!({
                        "type": "mcpToolCall",
                        "server": "browser",
                        "tool": "screenshot",
                        "arguments": {"title": "获取页面截图"}
                    }),
                )],
                dropped_entries: 0,
                retry_count: 0,
                retry_error: None,
                reasoning_summary: None,
                plan_explanation: None,
                plan: Vec::new(),
                diff_summary: None,
                web_searches: Vec::new(),
                dropped_web_searches: 0,
                collab: None,
                completed: false,
                failed: false,
            },
            ImText::zh_cn(),
        );

        assert!(rendered.contains("进行中 · MCP\nbrowser.screenshot · 获取页面截图"));
        assert!(!rendered.contains("```shell"));
    }

    #[test]
    fn mcp_result_error_is_rendered_as_a_bounded_failure_summary() {
        let entry = mcp_completed_entry(
            "mcp-1",
            &json!({
                "type": "mcpToolCall",
                "server": "browser",
                "tool": "navigate",
                "status": "completed",
                "result": {
                    "isError": true,
                    "content": [{"type": "text", "text": "503 Service Unavailable"}]
                }
            }),
        );
        assert_eq!(entry.status, TelegramCommandProgressStatus::Failed);
        assert_eq!(
            entry.failure_output.as_deref(),
            Some("503 Service Unavailable")
        );

        let rendered = render_command_progress(
            &TelegramCommandProgressSnapshot {
                turn_id: "turn".to_string(),
                revision: 2,
                message_id: Some("42".to_string()),
                entries: vec![entry],
                dropped_entries: 0,
                retry_count: 0,
                retry_error: None,
                reasoning_summary: None,
                plan_explanation: None,
                plan: Vec::new(),
                diff_summary: None,
                web_searches: Vec::new(),
                dropped_web_searches: 0,
                collab: None,
                completed: false,
                failed: false,
            },
            ImText::zh_cn(),
        );
        assert!(rendered.contains("失败 · MCP\nbrowser.navigate"));
        assert!(rendered.contains("503 Service Unavailable"));
    }

    #[test]
    fn mcp_protocol_error_prefers_the_message_field() {
        let entry = mcp_completed_entry(
            "mcp-1",
            &json!({
                "type": "mcpToolCall",
                "server": "browser",
                "tool": "navigate",
                "status": "failed",
                "error": {
                    "message": "MCP server unavailable",
                    "code": -32000
                }
            }),
        );

        assert_eq!(entry.status, TelegramCommandProgressStatus::Failed);
        assert_eq!(
            entry.failure_output.as_deref(),
            Some("MCP server unavailable")
        );
    }

    #[test]
    fn rich_progress_shows_three_mcp_steps_and_folds_the_rest() {
        let entries = (0..8)
            .map(|index| {
                mcp_completed_entry(
                    &format!("mcp-{index}"),
                    &json!({
                        "type": "mcpToolCall",
                        "server": "browser",
                        "tool": format!("tool-{index}"),
                        "status": "completed"
                    }),
                )
            })
            .collect();
        let rendered = render_task_progress(
            &TelegramCommandProgressSnapshot {
                turn_id: "turn".to_string(),
                revision: 8,
                message_id: Some("42".to_string()),
                entries,
                dropped_entries: 0,
                retry_count: 0,
                retry_error: None,
                reasoning_summary: None,
                plan_explanation: None,
                plan: Vec::new(),
                diff_summary: None,
                web_searches: Vec::new(),
                dropped_web_searches: 0,
                collab: None,
                completed: true,
                failed: false,
            },
            ImText::zh_cn(),
        );

        let visible_commands = rendered
            .blocks
            .iter()
            .filter(|block| block["type"] == "pre")
            .collect::<Vec<_>>();
        assert_eq!(visible_commands.len(), 3);
        assert!(
            visible_commands
                .iter()
                .all(|block| block["language"] == "text")
        );
        let folded = rendered
            .blocks
            .iter()
            .find(|block| block["summary"] == "… 另外 5 个较早步骤")
            .expect("folded earlier steps");
        assert_eq!(
            folded["blocks"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|block| block["type"] == "pre")
                .count(),
            5
        );

        let encoded = serde_json::to_string(&rendered.blocks).expect("rich progress");
        assert!(encoded.contains("browser.tool-7"));
        assert!(encoded.contains("browser.tool-0"));
        assert!(rendered.fallback_markdown.contains("另外 5 个较早步骤"));
        assert!(!rendered.fallback_markdown.contains("browser.tool-2"));
        assert!(rendered.fallback_markdown.contains("browser.tool-7"));
    }

    #[test]
    fn render_prioritizes_running_and_failed_steps() {
        let mut entries = (0..8)
            .map(|index| {
                completed_entry(
                    &format!("item-{index}"),
                    &json!({"command": format!("command {index}"), "exitCode": 0}),
                )
            })
            .collect::<Vec<_>>();
        entries[1] = completed_entry(
            "item-1",
            &json!({"command": "failed early", "exitCode": 1, "aggregatedOutput": "boom"}),
        );
        entries[2] = running_entry("item-2", &json!({"command": "still running"}));
        let rendered = render_command_progress(
            &TelegramCommandProgressSnapshot {
                turn_id: "turn".to_string(),
                revision: 9,
                message_id: Some("42".to_string()),
                entries,
                dropped_entries: 0,
                retry_count: 0,
                retry_error: None,
                reasoning_summary: None,
                plan_explanation: None,
                plan: Vec::new(),
                diff_summary: None,
                web_searches: Vec::new(),
                dropped_web_searches: 0,
                collab: None,
                completed: false,
                failed: false,
            },
            ImText::zh_cn(),
        );

        assert!(rendered.contains("执行中"));
        assert!(rendered.contains("failed early"));
        assert!(rendered.contains("still running"));
        assert!(rendered.contains("另外 5 个较早步骤"));
        assert!(!rendered.contains("command 0"));
    }

    #[test]
    fn render_is_bounded_to_one_telegram_message() {
        let output = "x".repeat(20_000);
        let entries = (0..128)
            .map(|index| {
                completed_entry(
                    &format!("item-{index}"),
                    &json!({
                        "command": format!("{} {index}", "c".repeat(2_000)),
                        "exitCode": 1,
                        "aggregatedOutput": output
                    }),
                )
            })
            .collect();
        let rendered = render_command_progress(
            &TelegramCommandProgressSnapshot {
                turn_id: "turn".to_string(),
                revision: 128,
                message_id: None,
                entries,
                dropped_entries: 25,
                retry_count: 0,
                retry_error: None,
                reasoning_summary: None,
                plan_explanation: None,
                plan: Vec::new(),
                diff_summary: None,
                web_searches: Vec::new(),
                dropped_web_searches: 0,
                collab: None,
                completed: true,
                failed: false,
            },
            ImText::zh_cn(),
        );

        assert!(rendered.chars().count() <= TELEGRAM_COMMAND_PROGRESS_MAX_CHARS);
        assert!(rendered.contains("执行完成"));
        assert!(rendered.contains("另外 150 个较早步骤"));
    }

    #[test]
    fn render_bounds_three_visible_failures_with_a_max_retry_error() {
        let entries = (0..5)
            .map(|index| {
                completed_entry(
                    &format!("item-{index}"),
                    &json!({
                        "command": format!("{} command-tail-{index}", "命".repeat(2_000)),
                        "exitCode": 1,
                        "aggregatedOutput": format!(
                            "{} failure-tail-{index}",
                            "误".repeat(2_000)
                        )
                    }),
                )
            })
            .collect();
        let retry_error = "错".repeat(600);
        let rendered = render_command_progress(
            &TelegramCommandProgressSnapshot {
                turn_id: "turn".to_string(),
                revision: 10,
                message_id: Some("42".to_string()),
                entries,
                dropped_entries: 0,
                retry_count: 5,
                retry_error: Some(retry_error.clone()),
                reasoning_summary: None,
                plan_explanation: None,
                plan: Vec::new(),
                diff_summary: None,
                web_searches: Vec::new(),
                dropped_web_searches: 0,
                collab: None,
                completed: true,
                failed: true,
            },
            ImText::zh_cn(),
        );

        assert!(rendered.chars().count() <= TELEGRAM_COMMAND_PROGRESS_MAX_CHARS);
        assert!(rendered.contains(&retry_error));
        for index in 2..5 {
            assert!(rendered.contains(&format!("command-tail-{index}")));
            assert!(rendered.contains(&format!("failure-tail-{index}")));
        }
        assert!(!rendered.contains("command-tail-0"));
        assert!(!rendered.contains("command-tail-1"));
    }

    #[test]
    fn render_distinguishes_an_interrupted_terminal_step() {
        let mut entry = running_entry("item", &json!({"command": "cargo test"}));
        entry.status = TelegramCommandProgressStatus::Interrupted;
        let rendered = render_command_progress(
            &TelegramCommandProgressSnapshot {
                turn_id: "turn".to_string(),
                revision: 2,
                message_id: Some("42".to_string()),
                entries: vec![entry],
                dropped_entries: 0,
                retry_count: 0,
                retry_error: None,
                reasoning_summary: None,
                plan_explanation: None,
                plan: Vec::new(),
                diff_summary: None,
                web_searches: Vec::new(),
                dropped_web_searches: 0,
                collab: None,
                completed: true,
                failed: false,
            },
            ImText::zh_cn(),
        );

        assert!(rendered.contains("执行结束 · 1 步 · 1 个中断"));
        assert!(rendered.contains("已中断\n```shell\ncargo test\n```"));
        assert!(!rendered.contains("进行中"));
    }

    #[test]
    fn render_marks_a_failed_turn_even_when_commands_succeeded() {
        let rendered = render_command_progress(
            &TelegramCommandProgressSnapshot {
                turn_id: "turn".to_string(),
                revision: 3,
                message_id: Some("42".to_string()),
                entries: vec![completed_entry(
                    "item",
                    &json!({"command": "cargo test", "exitCode": 0}),
                )],
                dropped_entries: 0,
                retry_count: 0,
                retry_error: None,
                reasoning_summary: None,
                plan_explanation: None,
                plan: Vec::new(),
                diff_summary: None,
                web_searches: Vec::new(),
                dropped_web_searches: 0,
                collab: None,
                completed: true,
                failed: true,
            },
            ImText::zh_cn(),
        );

        assert!(rendered.contains("执行失败 · 1 步"));
        assert!(!rendered.contains("执行完成"));
    }

    #[test]
    fn render_retry_only_progress_and_terminal_state() {
        let mut snapshot = TelegramCommandProgressSnapshot {
            turn_id: "turn".to_string(),
            revision: 2,
            message_id: Some("42".to_string()),
            entries: Vec::new(),
            dropped_entries: 0,
            retry_count: 2,
            retry_error: Some("503 Service Unavailable".to_string()),
            reasoning_summary: None,
            plan_explanation: None,
            plan: Vec::new(),
            diff_summary: None,
            web_searches: Vec::new(),
            dropped_web_searches: 0,
            collab: None,
            completed: false,
            failed: false,
        };

        let running = render_command_progress(&snapshot, ImText::zh_cn());
        assert!(running.contains("模型请求重试中 · 第 2 次"));
        assert!(running.contains("```text\n503 Service Unavailable\n```"));

        snapshot.completed = true;
        snapshot.failed = true;
        let failed = render_command_progress(&snapshot, ImText::zh_cn());
        assert!(failed.contains("模型请求失败 · 已重试 2 次"));
    }

    #[test]
    fn render_uses_native_shell_blocks_for_commands() {
        let rendered = render_command_progress(
            &TelegramCommandProgressSnapshot {
                turn_id: "turn".to_string(),
                revision: 1,
                message_id: None,
                entries: vec![running_entry(
                    "item",
                    &json!({"command": "printf 12345678"}),
                )],
                dropped_entries: 0,
                retry_count: 0,
                retry_error: None,
                reasoning_summary: None,
                plan_explanation: None,
                plan: Vec::new(),
                diff_summary: None,
                web_searches: Vec::new(),
                dropped_web_searches: 0,
                collab: None,
                completed: false,
                failed: false,
            },
            ImText::zh_cn(),
        );

        assert!(rendered.contains("```shell\nprintf 12345678\n```"));
        assert!(!rendered.contains("`printf 12345678`"));
    }

    #[test]
    fn parse_plan_update_maps_protocol_statuses_and_skips_blank_steps() {
        let params = json!({
            "threadId": "thread",
            "turnId": "turn",
            "explanation": "  inspect, implement, verify  ",
            "plan": [
                {"step": " inspect ", "status": "completed"},
                {"step": "implement", "status": "inProgress"},
                {"step": "verify", "status": "pending"},
                {"step": " ", "status": "completed"}
            ]
        });

        let (explanation, plan) = parse_plan_update(&params);

        assert_eq!(explanation.as_deref(), Some("inspect, implement, verify"));
        assert_eq!(plan.len(), 3);
        assert_eq!(plan[0].status, TelegramPlanStepStatus::Completed);
        assert_eq!(plan[1].status, TelegramPlanStepStatus::InProgress);
        assert_eq!(plan[2].status, TelegramPlanStepStatus::Pending);
        assert_eq!(plan[0].step, "inspect");
    }

    #[test]
    fn reasoning_summary_parser_uses_only_the_latest_summary_part() {
        let item = json!({
            "type": "reasoning",
            "summary": ["first", "first", {"text": "second"}],
            "content": ["second", {"text": "third"}]
        });

        assert_eq!(
            reasoning_summary_from_item(&item).as_deref(),
            Some("second")
        );
    }

    #[test]
    fn diff_summary_counts_only_unified_hunk_lines() {
        let diff = "diff --git a/src/a.rs b/src/a.rs\nindex 1..2 100644\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,2 +1,3 @@\n-old\n+new\n+another\n+metadata-like-line\ndiff --git a/src/b.rs b/src/b.rs\n--- a/src/b.rs\n+++ b/src/b.rs\n@@ -1,2 +1,1 @@\n-removed\n-removed-again\n kept\n";

        let summary = diff_summary_from_diff(diff).expect("diff summary");

        assert_eq!(summary.file_count, 2);
        assert_eq!(summary.additions, 3);
        assert_eq!(summary.deletions, 3);
        assert_eq!(summary.paths, vec!["src/a.rs", "src/b.rs"]);
        assert_eq!(
            summary.files,
            vec![
                TelegramDiffFileSummary {
                    path: "src/a.rs".to_string(),
                    additions: 3,
                    deletions: 1,
                },
                TelegramDiffFileSummary {
                    path: "src/b.rs".to_string(),
                    additions: 0,
                    deletions: 2,
                },
            ]
        );
        assert_eq!(summary.omitted_paths, 0);
    }

    #[test]
    fn file_change_summary_includes_move_path_and_unified_stats() {
        let item = json!({
            "type": "fileChange",
            "changes": [{
                "path": "src/old.rs",
                "kind": {"type": "update", "move_path": "src/new.rs"},
                "diff": "--- a/src/old.rs\n+++ b/src/new.rs\n@@ -1 +1 @@\n-old\n+new\n"
            }]
        });

        let summary = file_change_diff_summary(&item).expect("file change summary");

        assert_eq!(summary.file_count, 1);
        assert_eq!(summary.additions, 1);
        assert_eq!(summary.deletions, 1);
        assert_eq!(summary.paths, vec!["src/old.rs -> src/new.rs"]);
        assert_eq!(
            summary.files,
            vec![TelegramDiffFileSummary {
                path: "src/old.rs -> src/new.rs".to_string(),
                additions: 1,
                deletions: 1,
            }]
        );
    }

    #[test]
    fn file_change_summary_counts_raw_add_and_delete_content() {
        let item = json!({
            "type": "fileChange",
            "changes": [
                {
                    "path": "src/new.rs",
                    "kind": {"type": "add"},
                    "diff": "fn main() {}\n\n"
                },
                {
                    "path": "src/old.rs",
                    "kind": {"type": "delete"},
                    "diff": "fn old() {}\nremoved\n"
                }
            ]
        });

        let summary = file_change_diff_summary(&item).expect("file change summary");

        assert_eq!(summary.additions, 2);
        assert_eq!(summary.deletions, 2);
        assert_eq!(
            summary.files,
            vec![
                TelegramDiffFileSummary {
                    path: "src/new.rs".to_string(),
                    additions: 2,
                    deletions: 0,
                },
                TelegramDiffFileSummary {
                    path: "src/old.rs".to_string(),
                    additions: 0,
                    deletions: 2,
                },
            ]
        );
    }

    #[test]
    fn diff_summary_splits_unified_headers_without_git_markers() {
        let diff = "--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1 @@\n-old\n+new\n--- a/src/b.rs\n+++ b/src/b.rs\n@@ -1 +1,2 @@\n-kept\n+kept\n+added\n";

        let summary = diff_summary_from_diff(diff).expect("diff summary");

        assert_eq!(summary.file_count, 2);
        assert_eq!(summary.additions, 3);
        assert_eq!(summary.deletions, 2);
        assert_eq!(summary.files.len(), 2);
        assert_eq!(summary.files[1].path, "src/b.rs");
        assert_eq!(summary.files[1].additions, 2);
        assert_eq!(summary.files[1].deletions, 1);
    }

    #[test]
    fn render_includes_reasoning_plan_and_diff_as_compact_text() {
        let rendered = render_command_progress(
            &TelegramCommandProgressSnapshot {
                turn_id: "turn".to_string(),
                revision: 4,
                message_id: None,
                entries: Vec::new(),
                dropped_entries: 0,
                retry_count: 0,
                retry_error: None,
                reasoning_summary: Some("first thought\n\nsecond thought".to_string()),
                plan_explanation: Some("work in order".to_string()),
                plan: vec![
                    TelegramPlanStep {
                        step: "inspect".to_string(),
                        status: TelegramPlanStepStatus::Completed,
                    },
                    TelegramPlanStep {
                        step: "verify".to_string(),
                        status: TelegramPlanStepStatus::InProgress,
                    },
                ],
                diff_summary: Some(TelegramDiffSummary {
                    file_count: 1,
                    additions: 2,
                    deletions: 1,
                    files: vec![TelegramDiffFileSummary {
                        path: "src/main.rs".to_string(),
                        additions: 2,
                        deletions: 1,
                    }],
                    paths: vec!["src/main.rs".to_string()],
                    omitted_paths: 0,
                }),
                web_searches: Vec::new(),
                dropped_web_searches: 0,
                collab: None,
                completed: true,
                failed: false,
            },
            ImText::zh_cn(),
        );

        assert!(rendered.chars().count() <= TELEGRAM_COMMAND_PROGRESS_MAX_CHARS);
        assert!(rendered.contains("思考摘要"));
        assert!(rendered.contains("计划 · 1/2"));
        assert!(rendered.contains("文件修改 · 1 个文件 · +2 -1"));
        assert!(rendered.contains("• main.rs"));
        assert!(!rendered.contains("• src/main.rs"));
        assert!(!rendered.contains("```diff"));
    }

    #[test]
    fn render_task_progress_builds_one_complete_rich_message() {
        let rendered = render_task_progress(
            &TelegramCommandProgressSnapshot {
                turn_id: "turn-019f-example".to_string(),
                revision: 7,
                message_id: Some("42".to_string()),
                entries: vec![
                    running_entry("cmd", &json!({"command": "cargo test"})),
                    mcp_completed_entry(
                        "mcp",
                        &json!({
                            "type": "mcpToolCall",
                            "server": "browser",
                            "tool": "screenshot",
                            "status": "completed",
                            "durationMs": 850
                        }),
                    ),
                ],
                dropped_entries: 2,
                retry_count: 2,
                retry_error: Some("503 Service Unavailable".to_string()),
                reasoning_summary: Some(
                    "**Check** the active `Telegram` delivery [state](https://telegram.org)."
                        .to_string(),
                ),
                plan_explanation: Some("Inspect, implement, verify.".to_string()),
                plan: vec![
                    TelegramPlanStep {
                        step: "Inspect the current flow".to_string(),
                        status: TelegramPlanStepStatus::Completed,
                    },
                    TelegramPlanStep {
                        step: "Run regression tests".to_string(),
                        status: TelegramPlanStepStatus::InProgress,
                    },
                ],
                diff_summary: Some(TelegramDiffSummary {
                    file_count: 1,
                    additions: 12,
                    deletions: 3,
                    files: vec![TelegramDiffFileSummary {
                        path: "src/im/events.rs".to_string(),
                        additions: 12,
                        deletions: 3,
                    }],
                    paths: vec!["src/im/events.rs".to_string()],
                    omitted_paths: 0,
                }),
                web_searches: Vec::new(),
                dropped_web_searches: 0,
                collab: Some(TelegramCollabProgressSnapshot {
                    entries: vec![TelegramCollabProgressEntry {
                        agent_id: "secret-agent-id".to_string(),
                        name: "api_review".to_string(),
                        status: TelegramCollabProgressStatus::Running,
                        detail: Some("Reviewing the official Telegram API".to_string()),
                        started_at_ms: 1_000,
                        updated_at_ms: 2_000,
                    }],
                    dropped_entries: 0,
                    completed: false,
                }),
                completed: false,
                failed: false,
            },
            ImText::zh_cn(),
        );

        let blocks = serde_json::Value::Array(rendered.blocks.clone());
        let encoded = serde_json::to_string(&blocks).expect("rich blocks should serialize");
        assert_eq!(blocks[0]["type"], "heading");
        assert_eq!(blocks[0]["text"], "任务进行中");
        assert_eq!(
            blocks[blocks.as_array().unwrap().len() - 1]["type"],
            "footer"
        );
        let first_divider = blocks
            .as_array()
            .unwrap()
            .iter()
            .position(|block| block["type"] == "divider")
            .expect("plan and execution should have a divider");
        assert_eq!(blocks[first_divider + 1]["type"], "paragraph");
        assert_eq!(blocks[first_divider + 1]["text"]["type"], "bold");
        assert_eq!(
            blocks[first_divider + 1]["text"]["text"],
            "执行中 · 4 步 · 1 个进行中"
        );
        assert_eq!(blocks[first_divider + 2]["type"], "pre");
        assert_eq!(blocks[first_divider + 2]["language"], "shell");
        assert_eq!(
            blocks
                .as_array()
                .unwrap()
                .iter()
                .filter(|block| block["type"] == "divider")
                .count(),
            1,
            "only the plan-to-execution divider should remain"
        );
        assert!(encoded.contains("details"));
        assert!(encoded.contains("browser.screenshot"));
        assert!(encoded.contains("api_review"));
        assert!(encoded.contains("503 Service Unavailable"));
        assert!(encoded.contains("events.rs"));
        assert!(!encoded.contains("src/im/events.rs"));
        assert!(encoded.contains("\"type\":\"table\""));
        assert!(encoded.contains("\"is_bordered\":true"));
        assert!(encoded.contains("\"is_striped\":true"));
        assert!(encoded.contains("\"is_header\":true"));
        assert!(encoded.contains("\"text\":\"+12\""));
        assert!(encoded.contains("\"text\":\"-3\""));
        assert!(!encoded.contains("secret-agent-id"));
        assert_eq!(encoded.matches("\"has_checkbox\":true").count(), 3);
        assert_eq!(encoded.matches("\"is_checked\":true").count(), 1);
        let reasoning_heading = blocks
            .as_array()
            .unwrap()
            .iter()
            .position(|block| {
                block["type"] == "paragraph"
                    && block["text"]["type"] == "bold"
                    && block["text"]["text"] == "思考摘要"
            })
            .expect("reasoning heading should be always visible");
        let reasoning_body = &blocks[reasoning_heading + 1];
        assert_eq!(reasoning_body["type"], "paragraph");
        assert_eq!(reasoning_body["text"][0]["type"], "bold");
        assert_eq!(reasoning_body["text"][0]["text"], "Check");
        assert_eq!(reasoning_body["text"][2]["type"], "code");
        assert_eq!(reasoning_body["text"][4]["type"], "url");
        assert!(
            !blocks
                .as_array()
                .unwrap()
                .iter()
                .any(|block| { block["type"] == "details" && block["summary"] == "思考摘要" })
        );
        assert!(!encoded.contains("**Check**"));
        for marker in ["✅", "❌", "⚠️", "⏳", "🛠", "🔄"] {
            assert!(!encoded.contains(marker), "rich progress leaked {marker}");
        }
        assert!(rendered.fallback_markdown.chars().count() <= 3_800);
        assert!(rendered.fallback_markdown.contains("api_review"));
        assert!(rendered.fallback_markdown.starts_with("任务进行中"));
        let fallback_plan = rendered
            .fallback_markdown
            .find("计划 · 1/2")
            .expect("fallback plan heading");
        let fallback_execution = rendered
            .fallback_markdown
            .find("执行中 · 4 步 · 1 个进行中")
            .expect("fallback execution heading");
        let fallback_reasoning = rendered
            .fallback_markdown
            .find("思考摘要")
            .expect("fallback reasoning heading");
        let fallback_diff = rendered
            .fallback_markdown
            .find("文件修改 · 1 个文件 · +12 -3")
            .expect("fallback diff heading");
        assert!(fallback_plan < fallback_execution);
        assert!(fallback_execution < fallback_reasoning);
        assert!(fallback_reasoning < fallback_diff);
        for marker in ["✅", "❌", "⚠️", "⏳", "🛠", "🔄"] {
            assert!(
                !rendered.fallback_markdown.contains(marker),
                "fallback progress leaked {marker}"
            );
        }
    }

    #[test]
    fn web_searches_are_folded_into_the_task_progress_message() {
        let web_searches = (1..=4)
            .map(|index| TelegramWebSearchProgressEntry {
                item_id: format!("search-{index}"),
                summary: format!("搜索 · query {index} · 1 条结果"),
                blocks: vec![json!({
                    "type": "paragraph",
                    "text": format!("result {index}"),
                })],
                fallback_markdown: format!(
                    "🔎 搜索\n\n关键词：`query {index}`\n结果：1 条\n- result {index}"
                ),
            })
            .collect();
        let rendered = render_task_progress(
            &TelegramCommandProgressSnapshot {
                turn_id: "turn".to_string(),
                revision: 4,
                message_id: Some("42".to_string()),
                entries: Vec::new(),
                dropped_entries: 0,
                retry_count: 0,
                retry_error: None,
                reasoning_summary: None,
                plan_explanation: None,
                plan: Vec::new(),
                diff_summary: None,
                web_searches,
                dropped_web_searches: 2,
                collab: None,
                completed: false,
                failed: false,
            },
            ImText::zh_cn(),
        );

        let blocks = serde_json::Value::Array(rendered.blocks);
        assert_eq!(blocks[0]["text"], "任务进行中");
        assert!(blocks.as_array().unwrap().iter().any(|block| {
            block["type"] == "paragraph"
                && block["text"]["type"] == "bold"
                && block["text"]["text"] == "搜索 · 6 次"
        }));
        assert!(
            blocks
                .as_array()
                .unwrap()
                .iter()
                .any(|block| block["summary"] == "较早搜索 · 4 次")
        );
        assert!(
            blocks
                .as_array()
                .unwrap()
                .iter()
                .any(|block| block["summary"] == "搜索 · query 3 · 1 条结果")
        );
        assert!(
            blocks
                .as_array()
                .unwrap()
                .iter()
                .any(|block| block["summary"] == "搜索 · query 4 · 1 条结果")
        );
        assert!(rendered.fallback_markdown.contains("搜索 · 6 次"));
        assert!(
            rendered
                .fallback_markdown
                .contains("较早搜索 · 4 次（已折叠）")
        );
        assert!(rendered.fallback_markdown.contains("query 3"));
        assert!(rendered.fallback_markdown.contains("query 4"));
        assert!(!rendered.fallback_markdown.contains("query 1"));
    }

    #[test]
    fn rich_diff_table_limits_rows_and_shows_only_file_names() {
        let files = (0..10)
            .map(|index| TelegramDiffFileSummary {
                path: format!(
                    "src/very/long/path/that/should/stay/readable/on/mobile/file-{index}.rs"
                ),
                additions: index + 1,
                deletions: index,
            })
            .collect::<Vec<_>>();
        let paths = files.iter().map(|file| file.path.clone()).collect();
        let rendered = render_task_progress(
            &TelegramCommandProgressSnapshot {
                turn_id: "turn".to_string(),
                revision: 1,
                message_id: None,
                entries: Vec::new(),
                dropped_entries: 0,
                retry_count: 0,
                retry_error: None,
                reasoning_summary: None,
                plan_explanation: None,
                plan: Vec::new(),
                diff_summary: Some(TelegramDiffSummary {
                    file_count: 10,
                    additions: 55,
                    deletions: 45,
                    files,
                    paths,
                    omitted_paths: 0,
                }),
                web_searches: Vec::new(),
                dropped_web_searches: 0,
                collab: None,
                completed: true,
                failed: false,
            },
            ImText::zh_cn(),
        );

        let details = rendered
            .blocks
            .iter()
            .find(|block| block["summary"] == "文件修改 · 10 个文件 · +55 -45")
            .expect("file change details");
        let table = &details["blocks"][0];
        assert_eq!(table["type"], "table");
        assert_eq!(table["cells"].as_array().unwrap().len(), 9);
        assert_eq!(table["cells"][0][0]["text"], "文件");
        assert_eq!(table["cells"][0][1]["text"], "新增");
        assert_eq!(table["cells"][0][2]["text"], "删除");
        assert_eq!(table["cells"][1][1]["text"], "+1");
        assert_eq!(table["cells"][1][2]["text"], "-0");
        assert_eq!(table["cells"][1][0]["text"]["text"], "file-0.rs");
        assert!("file-0.rs".chars().count() <= TELEGRAM_DIFF_TABLE_PATH_CHARS);
        assert_eq!(details["blocks"][1]["text"], "… 另外 2 个文件");
        assert!(rendered.fallback_markdown.contains("• file-0.rs"));
        assert!(!rendered.fallback_markdown.contains("src/very/long/path"));
        assert!(rendered.fallback_markdown.contains("… 另外 2 个文件"));
    }

    #[test]
    fn diff_file_display_name_handles_moves_and_both_path_separators() {
        assert_eq!(diff_file_display_name("src/main.rs"), "main.rs");
        assert_eq!(
            diff_file_display_name(r"C:\\workspace\\src\\main.rs"),
            "main.rs"
        );
        assert_eq!(
            diff_file_display_name("src/old.rs -> nested/new.rs"),
            "old.rs -> new.rs"
        );
    }

    #[test]
    fn rich_failure_details_and_step_share_a_compact_command_budget() {
        let command = format!("prefix-{}-suffix", "x".repeat(100));
        let rendered = render_task_progress(
            &TelegramCommandProgressSnapshot {
                turn_id: "turn".to_string(),
                revision: 1,
                message_id: None,
                entries: vec![completed_entry(
                    "failed",
                    &json!({
                        "command": command,
                        "exitCode": 1,
                        "aggregatedOutput": "boom",
                    }),
                )],
                dropped_entries: 0,
                retry_count: 0,
                retry_error: None,
                reasoning_summary: None,
                plan_explanation: None,
                plan: Vec::new(),
                diff_summary: None,
                web_searches: Vec::new(),
                dropped_web_searches: 0,
                collab: None,
                completed: true,
                failed: true,
            },
            ImText::zh_cn(),
        );

        let error_details = rendered
            .blocks
            .iter()
            .find(|block| block["type"] == "details" && block["blocks"][0]["type"] == "pre")
            .expect("failure details");
        assert_eq!(
            error_details["summary"],
            json!([
                "错误摘要 ",
                {
                    "type": "code",
                    "text": "prefix-xxxxxxxxxxxxxxxxxxx...xxxxxxxxxxxxxxxxxxxx-suffix",
                },
            ])
        );
        assert_eq!(error_details["blocks"][0]["text"], "boom");

        let command_block_index = rendered
            .blocks
            .iter()
            .position(|block| block["type"] == "pre" && block["language"] == "shell")
            .expect("command block");
        assert_eq!(
            rendered.blocks[command_block_index],
            json!({
                "type": "pre",
                "text": "prefix-xxxxxxxxxxxxxxxxxxx...xxxxxxxxxxxxxxxxxxxx-suffix",
                "language": "shell",
            })
        );
        assert_eq!(rendered.blocks[command_block_index + 1]["type"], "footer");
        assert_eq!(
            rendered.blocks[command_block_index + 1]["text"],
            json!([
                {"type": "bold", "text": "失败"},
                " · exit 1",
            ])
        );
        assert!(rendered.fallback_markdown.contains(&command));
    }

    #[test]
    fn rich_progress_without_plan_keeps_execution_as_the_top_level_section() {
        let rendered = render_task_progress(
            &TelegramCommandProgressSnapshot {
                turn_id: "turn".to_string(),
                revision: 1,
                message_id: None,
                entries: vec![running_entry("cmd", &json!({"command": "cargo test"}))],
                dropped_entries: 0,
                retry_count: 0,
                retry_error: None,
                reasoning_summary: None,
                plan_explanation: None,
                plan: Vec::new(),
                diff_summary: None,
                web_searches: Vec::new(),
                dropped_web_searches: 0,
                collab: None,
                completed: false,
                failed: false,
            },
            ImText::zh_cn(),
        );

        let blocks = serde_json::Value::Array(rendered.blocks);
        assert_eq!(blocks[0]["type"], "heading");
        assert_eq!(blocks[0]["text"], "执行中 · 1 步 · 1 个进行中");
        assert_eq!(blocks[1]["type"], "pre");
        assert_eq!(blocks[1]["language"], "shell");
        assert_eq!(
            blocks
                .as_array()
                .unwrap()
                .iter()
                .filter(|block| block["type"] == "divider")
                .count(),
            0,
            "the footer should not add a redundant divider"
        );
        assert!(
            rendered
                .fallback_markdown
                .starts_with("执行中 · 1 步 · 1 个进行中")
        );
    }

    #[test]
    fn command_entries_use_left_aligned_shell_blocks_with_status_footers() {
        let mut interrupted = running_entry("interrupted", &json!({"command": "stop-me"}));
        interrupted.status = TelegramCommandProgressStatus::Interrupted;
        let entries = [
            running_entry("running", &json!({"command": "still-running"})),
            completed_entry("succeeded", &json!({"command": "done", "exitCode": 0})),
            completed_entry("failed", &json!({"command": "broken", "exitCode": 1})),
            interrupted,
        ];

        let rendered = entries
            .iter()
            .map(|entry| rich_command_entry_blocks(entry, ImText::zh_cn()))
            .collect::<Vec<_>>();
        assert!(rendered.iter().all(|blocks| blocks.len() == 2));
        assert!(rendered.iter().all(|blocks| blocks[0]["type"] == "pre"));
        assert!(
            rendered
                .iter()
                .all(|blocks| blocks[0]["language"] == "shell")
        );
        assert!(rendered.iter().all(|blocks| blocks[1]["type"] == "footer"));

        let encoded = serde_json::to_string(&rendered).expect("entries should serialize");
        assert!(encoded.contains("成功"));
        assert!(!encoded.contains("已完成"));
        assert!(encoded.contains("进行中"));
        assert!(encoded.contains("失败"));
        assert!(encoded.contains("已中断"));
        assert!(!encoded.contains("has_checkbox"));
        assert!(!encoded.contains("✅"));
        assert!(!encoded.contains("❌"));
        assert!(!encoded.contains("⚠️"));
    }
}
