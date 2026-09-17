use serde_json::Value;

use crate::{
    im::core::i18n::ImText,
    im_runtime::{
        TelegramCommandProgressEntry, TelegramCommandProgressEntryKind,
        TelegramCommandProgressSnapshot, TelegramCommandProgressStatus, TelegramDiffFileSummary,
        TelegramDiffSummary, TelegramPlanStep, TelegramPlanStepStatus,
    },
};

use super::{collab_progress, commentary, rich_blocks};

const TELEGRAM_COMMAND_PROGRESS_VISIBLE_STEPS: usize = 3;
const TELEGRAM_COMMAND_PROGRESS_COMMAND_CHARS: usize = 180;
const TELEGRAM_COMMAND_PROGRESS_RICH_COMMAND_CHARS: usize = 56;
pub(crate) const TELEGRAM_COMMAND_PROGRESS_FAILURE_CHARS: usize = 480;
const TELEGRAM_COMMAND_PROGRESS_FAILURE_LINES: usize = 6;
const TELEGRAM_COMMAND_PROGRESS_RETRY_ERROR_CHARS: usize = 600;
/// 「思考摘要」在气泡里**只占一行**，超出部分截断并以 `…` 结尾。
///
/// 之前这里是 720，且走的是保留换行的 `compact_text`，于是 Codex 那种累积式、
/// 带项目符号一大段的 reasoning summary 会整段铺在卡片顶部，把「执行完成」气泡
/// 撑得很高。需求是"截短放到顶部"，所以改成单行 + 省略号。
const TELEGRAM_REASONING_RENDER_CHARS: usize = 80;
const TELEGRAM_PLAN_RENDER_STEPS: usize = 6;
const TELEGRAM_PLAN_STEP_CHARS: usize = 180;
const TELEGRAM_DIFF_RENDER_PATHS: usize = 8;
const TELEGRAM_DIFF_PATH_CHARS: usize = 180;
const TELEGRAM_DIFF_TABLE_PATH_CHARS: usize = 48;
const TELEGRAM_DIFF_MAX_PATHS: usize = 128;
/// 折叠的工具摘要里，除优先级选中的步骤外，最多再展开的历史步骤数。
///
/// 与 `TELEGRAM_COMMAND_PROGRESS_VISIBLE_STEPS` 相加，气泡里同时参与交错渲染的
/// 工具步骤上限是 **30**。
///
/// 名额不能太少：窗口是从最新往回连续取的，一旦总步数超过上限，中段的工具会
/// 被整体挤出，思考之间失去间隔，气泡就退化成"思考全并在一起、工具全并在一起"
/// （实测 35 步时必然出现）。
const TELEGRAM_COMMAND_PROGRESS_DETAILS_STEPS: usize = 27;
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

/// Test-only alias kept so the plan-parsing fixtures can call the same entry
/// point the router used before it moved to `plan_from_params` directly.
#[cfg(test)]
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
        // 真实序号由 upsert 在新增条目时分配（更新条目会沿用原值）。
        sequence: 0,
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
        sequence: 0,
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
        // 真实序号由 upsert 在新增条目时分配（更新条目会沿用原值）。
        sequence: 0,
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
        sequence: 0,
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
    let commentary = commentary::render_commentary(
        &snapshot.commentary,
        snapshot.commentary_dropped_entries,
        text,
    );
    let commentary_text = commentary::render_commentary_fallback(&commentary, text);
    let command_fallback = render_command_progress(snapshot, text);
    // 富消息不可用时无法折叠：回退文本按「过程文案 → 工具摘要 → 协作」顺序直出。
    // 预算仍要守：先保工具摘要，过程文案按尾部（最新内容）截断。
    let commentary_text = truncate_tail(
        &commentary_text,
        TELEGRAM_TASK_PROGRESS_FALLBACK_MAX_CHARS
            .saturating_sub(command_fallback.chars().count().saturating_add(2)),
    );
    let command_fallback = if commentary_text.is_empty() {
        command_fallback
    } else {
        format!("{commentary_text}\n\n{command_fallback}")
    };
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

    // 思考摘要放在「执行中 · N 步」正下方，**只占一行**。
    //
    // Codex 的 reasoning summary 是**累积式**的一大段（不是按轮次分条），
    // 所以不参与与工具步骤的交错排序——放顶部更符合阅读顺序，也让主体信息
    // （思考→工具→结论）自上而下展开。截断值为 TELEGRAM_REASONING_RENDER_CHARS，
    // 每次进度刷新都原地重渲染这条气泡，因此这里的内容会随之更新。
    //
    // 这里**不加「思考摘要」标题**：单行文案本身已经足够表意，标题只会白占一行高度。
    //
    // 用 `code`（等宽蓝底）而不是 `inline_markdown`：与底部 `turn <id>` 的样式一致，
    // 一眼能区分"这是模型的过程说明"，也不会和下面的工具/正文混淆。
    if let Some(reasoning) = snapshot.reasoning_summary.as_deref() {
        let trimmed = reasoning.trim();
        if !trimmed.is_empty() {
            blocks.push(rich_blocks::paragraph(rich_blocks::code(
                reasoning_render_line(trimmed),
            )));
        }
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

    // 过程文案与工具步骤按**实际到达顺序**交错渲染：
    // 文案（思考内容）保持可见，工具步骤按批次收进折叠块。
    render_interleaved_progress(snapshot, text, &mut blocks);

    blocks.extend(render_web_search_progress_blocks(snapshot));
    if let Some(collab) = snapshot.collab.as_ref() {
        blocks.push(collab_progress::render_collab_progress_details(
            collab, text,
        ));
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

/// 工具/命令步骤的折叠块：标题带步骤总数，内部保留优先级筛选与省略说明。
/// 按到达顺序交错渲染「过程文案」与「工具步骤」。
///
/// 规则：
/// - 文案（Codex 的说明文字）保持可见、不折叠——用户要能直接读到思考内容；
/// - 连续的多个工具步骤合并成一个折叠块，标题带该批次的步骤数；
/// - 两者按 `sequence`（到达序号）排序，因此顺序与真实发生顺序一致。
///
/// 超预算的文案由 `commentary::render_commentary` 从最早开始丢弃，并在顶部标注。
fn render_interleaved_progress(
    snapshot: &TelegramCommandProgressSnapshot,
    text: ImText,
    blocks: &mut Vec<Value>,
) {
    let commentary = commentary::render_commentary(
        &snapshot.commentary,
        snapshot.commentary_dropped_entries,
        text,
    );
    if commentary.dropped > 0 {
        blocks.push(rich_blocks::paragraph(rich_blocks::text(
            text.telegram_commentary_omitted(commentary.dropped),
        )));
    }

    // 只有进入渲染范围的步骤才参与交错：优先级步骤 + 最近的历史步骤，
    // 与折叠块原先的选择口径保持一致。
    let selected = interleaved_tool_indices(snapshot);
    let selected_count = selected.len();

    let mut items: Vec<InterleavedItem<'_>> = Vec::new();
    for entry in &commentary.entries {
        items.push(InterleavedItem {
            sequence: entry.sequence,
            kind: InterleavedKind::Commentary(entry),
        });
    }
    for index in selected {
        let entry = &snapshot.entries[index];
        items.push(InterleavedItem {
            sequence: entry.sequence,
            kind: InterleavedKind::Tool(entry),
        });
    }
    items.sort_by_key(|item| item.sequence);

    // 诊断：dump 排序后的 (kind, seq)，直接反映交错顺序。

    let total = snapshot
        .dropped_entries
        .saturating_add(snapshot.entries.len());
    let omitted = total.saturating_sub(selected_count);

    // 相邻的同类条目合并成一个批次：连续的文案 → 一个「思考过程（N）」，
    // 连续的工具 → 一个「工具摘要（N）」。被异类隔开就各自成批。
    let mut batches: Vec<InterleavedBatch<'_>> = Vec::new();
    for item in items {
        match item.kind {
            InterleavedKind::Commentary(entry) => match batches.last_mut() {
                Some(InterleavedBatch::Commentary(entries)) => entries.push(entry),
                _ => batches.push(InterleavedBatch::Commentary(vec![entry])),
            },
            InterleavedKind::Tool(entry) => match batches.last_mut() {
                Some(InterleavedBatch::Tool(entries)) => entries.push(entry),
                _ => batches.push(InterleavedBatch::Tool(vec![entry])),
            },
        }
    }

    let last_tool_batch = batches
        .iter()
        .rposition(|batch| matches!(batch, InterleavedBatch::Tool(_)));
    let mut tool_batch_seen = 0usize;
    for (index, batch) in batches.iter().enumerate() {
        match batch {
            // 思考过程：可折叠但**默认展开**，用户要能直接读到内容。
            InterleavedBatch::Commentary(entries) => {
                let mut panel = Vec::new();
                for entry in entries {
                    panel.extend(commentary_entry_blocks(&entry.text));
                }
                if !panel.is_empty() {
                    blocks.push(rich_blocks::details(
                        rich_blocks::text(text.telegram_commentary_heading(entries.len())),
                        panel,
                        true,
                    ));
                }
            }
            // 工具摘要：默认折叠，标题带该批次的步骤数。
            InterleavedBatch::Tool(entries) => {
                let is_first = tool_batch_seen == 0;
                tool_batch_seen += 1;
                let batch_omitted = if Some(index) == last_tool_batch {
                    omitted
                } else {
                    0
                };
                flush_tool_batch(snapshot, entries, text, blocks, is_first, batch_omitted);
            }
        }
    }
    // 没有任何工具批次时，省略提示单独成段。
    if omitted > 0 && last_tool_batch.is_none() {
        blocks.push(rich_blocks::paragraph(rich_blocks::text(
            text.telegram_command_progress_omitted(omitted),
        )));
    }
}

/// 交错渲染中的一个批次：相邻同类条目合并而成。
enum InterleavedBatch<'a> {
    Commentary(Vec<&'a commentary::TelegramCommentaryRenderedEntry>),
    Tool(Vec<&'a TelegramCommandProgressEntry>),
}

enum InterleavedKind<'a> {
    Commentary(&'a commentary::TelegramCommentaryRenderedEntry),
    Tool(&'a TelegramCommandProgressEntry),
}

struct InterleavedItem<'a> {
    sequence: u64,
    kind: InterleavedKind<'a>,
}

/// 把一批连续的工具步骤渲染成一个折叠块（标题带该批次数量）。
fn flush_tool_batch(
    snapshot: &TelegramCommandProgressSnapshot,
    pending: &[&TelegramCommandProgressEntry],
    text: ImText,
    blocks: &mut Vec<Value>,
    is_first_batch: bool,
    omitted: usize,
) {
    if pending.is_empty() {
        return;
    }
    let mut panel = Vec::new();
    // 首批带上整体执行进度标题（"执行中 · 4 步 · 1 个进行中"）。
    if is_first_batch && has_plan_progress(snapshot) {
        panel.push(rich_blocks::paragraph(rich_blocks::bold(
            command_execution_progress_title(snapshot, text),
        )));
    }
    for entry in pending {
        panel.extend(rich_command_entry_blocks(entry, text));
        if let Some(output) = entry.failure_output.as_deref() {
            panel.push(rich_blocks::details(
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
    if omitted > 0 {
        panel.push(rich_blocks::paragraph(rich_blocks::text(
            text.telegram_command_progress_omitted(omitted),
        )));
    }
    blocks.push(rich_blocks::details(
        rich_blocks::text(text.telegram_tools_summary_heading(pending.len())),
        panel,
        false,
    ));
}

fn commentary_entry_blocks(entry: &str) -> Vec<Value> {
    let mut blocks = Vec::new();
    let mut paragraph = String::new();
    let mut fenced: Option<(String, Option<String>)> = None;
    let mut list_items: Vec<Value> = Vec::new();

    let lines: Vec<&str> = entry.lines().collect();
    let mut index = 0;
    while index < lines.len() {
        let remaining = &lines[index..];
        let line = remaining[0];
        // 默认前进一行；表格分支会按解析到的行数再额外消费。
        index += 1;

        if let Some((body, language)) = fenced.as_mut() {
            if line.trim_start().starts_with("```") {
                blocks.push(rich_blocks::preformatted(
                    body.trim_end_matches('\n').to_string(),
                    language.as_deref(),
                ));
                fenced = None;
            } else {
                body.push_str(line);
                body.push('\n');
            }
            continue;
        }
        if line.trim_start().starts_with("```") {
            push_commentary_paragraph(&mut blocks, &mut paragraph);
            flush_commentary_list(&mut blocks, &mut list_items);
            fenced = Some((String::new(), fence_language(line)));
            continue;
        }
        if line.trim().is_empty() {
            push_commentary_paragraph(&mut blocks, &mut paragraph);
            flush_commentary_list(&mut blocks, &mut list_items);
            continue;
        }
        // `| a | b |` + `|---|---|` + 数据行 → 协议的 table 块。
        //
        // 不处理的话整张表会被并进普通段落，渲染成一堆带竖线的原文（用户实测）。
        if let Some((table, consumed)) = commentary_table_block(remaining) {
            push_commentary_paragraph(&mut blocks, &mut paragraph);
            flush_commentary_list(&mut blocks, &mut list_items);
            blocks.push(table);
            index += consumed - 1;
            continue;
        }
        // `## 结论` → heading 块，同样避免露出 `##` 记号。
        if let Some((level, content)) = heading_line(line) {
            push_commentary_paragraph(&mut blocks, &mut paragraph);
            flush_commentary_list(&mut blocks, &mut list_items);
            if !content.trim().is_empty() {
                blocks.push(rich_blocks::heading(
                    rich_blocks::inline_markdown(content.trim()),
                    heading_render_size(level),
                ));
            }
            continue;
        }
        // `- 项目` / `* 项目` / `+ 项目` 折成协议的 list 块。
        //
        // 不处理的话这些行会被并进普通段落，渲染出一条带 `-` 的长文本——正是
        // 用户看到的"markdown 原文格式"。缩进的续行并进上一个项目。
        if let Some(item) = bullet_item_text(line) {
            push_commentary_paragraph(&mut blocks, &mut paragraph);
            list_items.push(rich_blocks::list_item(vec![rich_blocks::paragraph(
                rich_blocks::inline_markdown(item),
            )]));
            continue;
        }
        if !list_items.is_empty() && line.starts_with([' ', '\t']) {
            if let Some(last) = list_items.last_mut()
                && let Some(items) = last.get_mut("blocks").and_then(Value::as_array_mut)
                && let Some(first) = items.first_mut()
            {
                let existing = first["text"].clone();
                first["text"] = rich_blocks::inline_markdown(&format!(
                    "{} {}",
                    inline_plain_text(&existing),
                    line.trim()
                ));
            }
            continue;
        }
        if !paragraph.is_empty() {
            paragraph.push('\n');
        }
        paragraph.push_str(line);
    }

    if let Some((body, language)) = fenced {
        blocks.push(rich_blocks::preformatted(
            body.trim_end_matches('\n').to_string(),
            language.as_deref(),
        ));
    }
    push_commentary_paragraph(&mut blocks, &mut paragraph);
    flush_commentary_list(&mut blocks, &mut list_items);
    blocks
}

/// 解析 markdown 表格，返回 (table 块, 消费的行数)。
///
/// 形态必须是「表头行 + 分隔行 [+ 数据行…]」；列数按**分隔行**对齐，数据行缺列
/// 补空、多列丢弃，避免某一行多打一个 `|` 就把整张表打乱。
fn commentary_table_block(lines: &[&str]) -> Option<(Value, usize)> {
    let header = *lines.first()?;
    if !is_table_row(header) {
        return None;
    }
    let aligns = table_separator_alignments(lines.get(1)?)?;
    let header_cells = split_table_row(header);
    if header_cells.len() != aligns.len() {
        return None;
    }

    let mut rows = vec![
        header_cells
            .iter()
            .zip(aligns.iter())
            .map(|(cell, align)| {
                rich_blocks::table_cell(rich_blocks::inline_markdown(cell), true, align)
            })
            .collect::<Vec<_>>(),
    ];
    let mut consumed = 2;
    for line in &lines[2..] {
        if !is_table_row(line) {
            break;
        }
        let cells = split_table_row(line);
        rows.push(
            aligns
                .iter()
                .enumerate()
                .map(|(column, align)| {
                    let cell = cells.get(column).copied().unwrap_or("");
                    rich_blocks::table_cell(rich_blocks::inline_markdown(cell), false, align)
                })
                .collect(),
        );
        consumed += 1;
    }
    Some((rich_blocks::table(rows, true, true), consumed))
}

fn is_table_row(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.len() >= 2 && trimmed.starts_with('|') && trimmed.ends_with('|')
}

fn split_table_row(line: &str) -> Vec<&str> {
    let trimmed = line.trim();
    let inner = trimmed.strip_prefix('|').unwrap_or(trimmed);
    let inner = inner.strip_suffix('|').unwrap_or(inner);
    inner.split('|').map(str::trim).collect()
}

/// 解析分隔行（`|---|:---:|---:|`）得到每列对齐方式；不是分隔行时返回 `None`。
fn table_separator_alignments(line: &str) -> Option<Vec<&'static str>> {
    if !is_table_row(line) {
        return None;
    }
    let cells = split_table_row(line);
    if cells.is_empty() {
        return None;
    }
    let mut aligns = Vec::with_capacity(cells.len());
    for cell in cells {
        let core = cell.trim_matches(':').trim();
        if core.is_empty() || !core.chars().all(|ch| ch == '-') {
            return None;
        }
        aligns.push(match (cell.starts_with(':'), cell.ends_with(':')) {
            (true, true) => "center",
            (false, true) => "right",
            _ => "left",
        });
    }
    Some(aligns)
}

/// 解析 ATX 标题（`# ` ~ `###### `），返回 (级别, 正文)。
///
/// 必须带空格分隔：`#1` 这类文本不是标题，不能当成标题吃掉。
fn heading_line(line: &str) -> Option<(usize, &str)> {
    let trimmed = line.trim_start();
    let level = trimmed.chars().take_while(|ch| *ch == '#').count();
    if level == 0 || level > 6 {
        return None;
    }
    let rest = &trimmed[level..];
    if rest.is_empty() {
        return Some((level, ""));
    }
    let content = rest.strip_prefix(' ')?;
    Some((level, content.trim()))
}

/// markdown 标题级别 → 卡片内的 heading size。
///
/// 卡片大标题用的是 size 3，正文标题必须**更小**（size 越大字越小），否则
/// `## 结论` 会比「执行完成」还显眼。
fn heading_render_size(level: usize) -> u8 {
    match level {
        0..=2 => 4,
        3 | 4 => 5,
        _ => 6,
    }
}

/// 取 `- xxx` / `* xxx` / `+ xxx` 的项目正文；不是列表行时返回 `None`。
fn bullet_item_text(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let rest = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
        .or_else(|| trimmed.strip_prefix("+ "))?;
    let rest = rest.trim();
    (!rest.is_empty()).then_some(rest)
}

fn flush_commentary_list(blocks: &mut Vec<Value>, list_items: &mut Vec<Value>) {
    if !list_items.is_empty() {
        blocks.push(rich_blocks::list(std::mem::take(list_items)));
    }
}

/// 把已是富文本的 value 还原成纯文字（用于续行拼接）。
fn inline_plain_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts.iter().map(inline_plain_text).collect(),
        Value::Object(map) => map.get("text").map(inline_plain_text).unwrap_or_default(),
        _ => String::new(),
    }
}

fn push_commentary_paragraph(blocks: &mut Vec<Value>, paragraph: &mut String) {
    let trimmed = paragraph.trim();
    if !trimmed.is_empty() {
        blocks.push(rich_blocks::paragraph(rich_blocks::inline_markdown(
            trimmed,
        )));
    }
    paragraph.clear();
}

/// 提取围栏行上的语言标注；仅保留安全字符。
fn fence_language(line: &str) -> Option<String> {
    let info = line.trim_start().strip_prefix("```")?.trim();
    let language: String = info
        .split_whitespace()
        .next()
        .unwrap_or("")
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_' || *ch == '+')
        .take(24)
        .collect();
    (!language.is_empty()).then_some(language)
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
    // 极简卡片头：状态 emoji + 原标题 + 分隔线。
    let status_emoji = if snapshot.failed { "❌" } else { "🔄" };
    let title = format!(
        "{status_emoji} {}\n{}",
        command_progress_title(snapshot, text),
        rich_blocks::TELEGRAM_CARD_SEPARATOR
    );
    let mut sections = vec![title];
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
    } else if (total == 0 && has_supplemental) || has_plan_progress(snapshot) {
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
        // 不带「思考摘要」标题，只留那一行文案。
        sections.push(reasoning_render_line(reasoning));
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
    render_plan_standalone(snapshot.plan_explanation.as_deref(), &snapshot.plan, text)
}

/// 完整颗粒度下的独立计划消息：每次计划更新单独成条，不进聚合气泡。
pub(crate) fn render_plan_standalone(
    explanation: Option<&str>,
    steps: &[TelegramPlanStep],
    text: ImText,
) -> Option<String> {
    if explanation.is_none() && steps.is_empty() {
        return None;
    }
    let completed = steps
        .iter()
        .filter(|step| step.status == TelegramPlanStepStatus::Completed)
        .count();
    let mut lines = vec![text.telegram_plan_heading(completed, steps.len())];
    if let Some(explanation) = explanation {
        lines.push(compact_text(explanation, TELEGRAM_PLAN_STEP_CHARS));
    }
    for step in steps.iter().take(TELEGRAM_PLAN_RENDER_STEPS) {
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
    if steps.len() > TELEGRAM_PLAN_RENDER_STEPS {
        lines.push(text.telegram_plan_omitted(steps.len() - TELEGRAM_PLAN_RENDER_STEPS));
    }
    Some(lines.join("\n"))
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

/// 参与交错渲染的工具步骤下标：优先级步骤 + 最近的历史步骤。
///
/// 与折叠块原先的口径一致——折叠块本身不占屏面，展开后能看到较完整的上下文；
/// 只有在交错视图里这些步骤才会和文案一起排序。
fn interleaved_tool_indices(snapshot: &TelegramCommandProgressSnapshot) -> Vec<usize> {
    let priority = selected_entry_indices(&snapshot.entries);
    let mut shown = priority.clone();
    let budget = TELEGRAM_COMMAND_PROGRESS_DETAILS_STEPS;

    // 先为**每条思考**保留它前后紧邻的工具。
    //
    // 只按"最近的 N 条"取会有一个必然的塌陷：名额全部堆在时间轴末端，一旦工具
    // 总数超过名额，中段的工具就被整体挤出，思考之间失去间隔，气泡退化成
    // 「思考全并成一批 + 工具全并成一批」（实测 35 步以上必然发生）。
    //
    // 锚定每条思考的相邻工具后，无论任务多长，思考之间都至少隔着一个工具，
    // 交错结构因此不会随步数增长而消失。
    let mut anchors = Vec::new();
    for sequence in snapshot.commentary.iter().map(|entry| entry.sequence) {
        let before = snapshot
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.sequence < sequence)
            .map(|(index, _)| index)
            .next_back();
        let after = snapshot
            .entries
            .iter()
            .position(|entry| entry.sequence > sequence);
        anchors.extend(before);
        anchors.extend(after);
    }
    for index in anchors {
        if !shown.contains(&index) && shown.len() < budget + priority.len() {
            shown.push(index);
        }
    }

    // 剩余名额按"最近优先"补齐；从后往前取，保证刚完成的步骤不会消失。
    //
    // 曾经写成从前往后 `.take(N)`，取到的是**最旧**的 N 条。于是刚完成的步骤
    // 两头都不在——既掉出"最后 3 条"的优先级窗口，又不属于最旧的 N 条——因而
    // 在完成的一瞬间从气泡里消失（表现为"闪一下就没了"）。
    for index in (0..snapshot.entries.len()).rev() {
        if shown.len() >= budget + priority.len() {
            break;
        }
        if !shown.contains(&index) {
            shown.push(index);
        }
    }
    shown.sort_unstable();
    shown
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

pub(crate) fn render_entry(
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
        line.push('\n');
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

/// 把「思考摘要」折成**一行**并按 `TELEGRAM_REASONING_RENDER_CHARS` 截断。
///
/// 不能直接用 `compact_text`：它会保留换行（`lines.join("\n")`），而 Codex 的
/// reasoning summary 是累积式的多行文本，整段贴上去会把进度气泡撑爆。先折叠
/// 所有空白成单行，再截断加 `…`。
///
/// 还要剥掉内联 markdown 记号：这条文案用 `code`（等宽蓝底）渲染，`code` 是
/// **字面量**、不做 markdown 解析，不剥的话 `**Check**` 会原样显示成星号。
pub(crate) fn reasoning_render_line(reasoning: &str) -> String {
    truncate_text_with_ellipsis(
        &strip_inline_markdown(&single_line(reasoning)),
        TELEGRAM_REASONING_RENDER_CHARS,
    )
}

/// 去掉内联 markdown 记号，只留可读文字（用于按字面量渲染的场景）。
fn strip_inline_markdown(text: &str) -> String {
    let mut output = text.replace("**", "").replace("__", "");
    output = output.replace('`', "");
    // 把 `[文字](链接)` 压成 `文字`，避免等宽样式里出现原始 URL。
    while let Some(start) = output.find('[') {
        let Some(label_end) = output[start..].find("](") else {
            break;
        };
        let label_end = start + label_end;
        let Some(url_end) = output[label_end..].find(')') else {
            break;
        };
        let url_end = label_end + url_end;
        let label = output[start + 1..label_end].to_string();
        output.replace_range(start..=url_end, &label);
    }
    output.trim().to_string()
}

/// 只从尾部截断并补 `…`，用于"只显示一行"的场景（区别于居中截断的 `truncate_middle`）。
fn truncate_text_with_ellipsis(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    if max_chars <= 1 {
        return "…".chars().take(max_chars).collect();
    }
    let mut output = text
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    output.push('…');
    output
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
        .rsplit(['/', '\\'])
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
            TelegramCollabProgressStatus, TelegramCommandProgressEntry,
            TelegramCommandProgressEntryKind, TelegramCommandProgressSnapshot,
            TelegramCommandProgressStatus, TelegramCommentaryEntry,
        },
    };

    use super::interleaved_tool_indices;

    use super::{
        TELEGRAM_COMMAND_PROGRESS_MAX_CHARS, TELEGRAM_DIFF_TABLE_PATH_CHARS,
        TELEGRAM_REASONING_RENDER_CHARS, commentary_entry_blocks, completed_entry,
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
                commentary: Vec::new(),
                commentary_dropped_entries: 0,
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
                commentary: Vec::new(),
                commentary_dropped_entries: 0,
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
    fn rich_progress_folds_every_mcp_step_into_the_tools_summary() {
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
                commentary: Vec::new(),
                commentary_dropped_entries: 0,
                collab: None,
                completed: true,
                failed: false,
            },
            ImText::zh_cn(),
        );

        let tools = rendered
            .blocks
            .iter()
            .find(|block| block["type"] == "details")
            .expect("tools summary panel");
        assert_eq!(tools["summary"], "工具摘要（8）");
        let panel = tools["blocks"].as_array().expect("tools panel blocks");
        let visible_commands = panel
            .iter()
            .filter(|block| block["type"] == "pre")
            .collect::<Vec<_>>();
        assert_eq!(visible_commands.len(), 8);
        assert!(
            visible_commands
                .iter()
                .all(|block| block["language"] == "text")
        );
        assert!(
            !panel
                .iter()
                .any(|block| block["text"] == "… 另外 5 个较早步骤"),
            "steps inside the history budget stay available in the folded panel"
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
                commentary: Vec::new(),
                commentary_dropped_entries: 0,
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
                commentary: Vec::new(),
                commentary_dropped_entries: 0,
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
                commentary: Vec::new(),
                commentary_dropped_entries: 0,
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
                commentary: Vec::new(),
                commentary_dropped_entries: 0,
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
                commentary: Vec::new(),
                commentary_dropped_entries: 0,
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
            commentary: Vec::new(),
            commentary_dropped_entries: 0,
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
                commentary: Vec::new(),
                commentary_dropped_entries: 0,
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
                commentary: Vec::new(),
                commentary_dropped_entries: 0,
                collab: None,
                completed: true,
                failed: false,
            },
            ImText::zh_cn(),
        );

        assert!(rendered.chars().count() <= TELEGRAM_COMMAND_PROGRESS_MAX_CHARS);
        // 思考摘要已去掉标题，只保留那一行内容。
        assert!(!rendered.contains("思考摘要"));
        assert!(rendered.contains("first thought second thought"));
        assert!(rendered.contains("计划 · 1/2"));
        assert!(rendered.contains("文件修改 · 1 个文件 · +2 -1"));
        assert!(rendered.contains("• main.rs"));
        assert!(!rendered.contains("• src/main.rs"));
        assert!(!rendered.contains("```diff"));
    }

    /// 回归：「思考摘要」必须**只占一行**。
    ///
    /// 症状（用户实测）：Codex 的 reasoning summary 是累积式的一大段（带项目符号、
    /// 多行），卡片顶部把整段铺出来，把「执行完成」气泡撑得很高。
    ///
    /// 原因：原先走的是保留换行的 `compact_text(.., 720)`：
    ///   1. 换行被保留 → 多行原样输出；
    ///   2. 720 的阈值又高于多数摘要长度 → 连截断都不触发，等于全量显示。
    ///
    /// 断言渲染结果里思考摘要**恰好一行**，且超长时以 `…` 结尾。
    #[test]
    fn reasoning_summary_renders_as_a_single_truncated_line() {
        // 复刻真实形态：多行、带项目符号、总长远超单行预算。
        let reasoning = "The folder is open. Now write the final message.\n\nFinal message:\n- 找到了，已整理到 Downloads\n- 01 英文矢量 SVG\n- 02 英文 960px\n- 03 中文版 logo\nKeep concise. Done.";
        let snapshot = TelegramCommandProgressSnapshot {
            turn_id: "turn".to_string(),
            revision: 1,
            message_id: None,
            entries: Vec::new(),
            dropped_entries: 0,
            retry_count: 0,
            retry_error: None,
            reasoning_summary: Some(reasoning.to_string()),
            plan_explanation: None,
            plan: Vec::new(),
            diff_summary: None,
            web_searches: Vec::new(),
            dropped_web_searches: 0,
            commentary: Vec::new(),
            commentary_dropped_entries: 0,
            collab: None,
            completed: true,
            failed: false,
        };
        let rendered = render_command_progress(&snapshot, ImText::zh_cn());

        // 已去掉「思考摘要」标题：正文行直接以开头文案起始。
        assert!(!rendered.contains("思考摘要"), "不应再渲染「思考摘要」标题");
        let line = rendered
            .lines()
            .find(|line| line.starts_with("The folder is open"))
            .expect("应渲染出思考摘要正文行")
            .to_string();

        // 1) 必须是一行：正文里不能再夹带原始换行/项目符号。
        assert!(!line.contains('\n'), "思考摘要不应包含换行，实际: {line:?}");
        // 2) 超长时必须截断并加省略号，且不超过预算。
        assert!(
            line.ends_with('…'),
            "超长思考摘要应以 … 结尾，实际: {line:?}"
        );
        assert!(
            line.chars().count() <= TELEGRAM_REASONING_RENDER_CHARS,
            "思考摘要超过单行预算: {} > {}",
            line.chars().count(),
            TELEGRAM_REASONING_RENDER_CHARS
        );
        // 3) 原始的多行内容不得整段出现。
        assert!(
            !rendered.contains("Keep concise. Done."),
            "思考摘要不应把整段累积文本都渲染出来"
        );
    }

    /// 回归：最终回复里的 markdown **表格**与**标题**不能露出原文。
    ///
    /// 症状（用户实测）：气泡里直接显示
    /// `|时间|最后那条消息|记录显示|` / `|---|---|---|` 和 `## 结论`。
    ///
    /// 原因：内嵌走 `commentary_entry_blocks` 后，它只处理段落/代码/列表/链接，
    /// 没有表格与标题分支，于是这些行被并进普通段落原样输出。旧路径由 Telegram
    /// 原生渲染 markdown，所以这是内嵌改造引入的回退。
    #[test]
    fn commentary_markdown_renders_tables_and_headings() {
        let reply = "研究完了——原因找到了。直接说结论：\n## 结论\n四个时间点全部对得上：\n| 时间 | 最后那条消息 | 记录显示 |\n|---|---|---|\n|11:29:38|好，继续找战双|正常完成|\n|11:58:42|好，找《终末地》|同上|\n证据链都来自本机日志。";
        let blocks = commentary_entry_blocks(reply);
        let encoded = serde_json::to_string(&serde_json::Value::Array(blocks.clone())).unwrap();

        // 1) 表格必须变成 table 块，而不是带竖线的段落原文。
        assert!(!encoded.contains("|---|"), "表格分隔行原文泄漏: {encoded}");
        assert!(!encoded.contains("|11:29:38|"), "表格数据行原文泄漏");
        let table = blocks
            .iter()
            .find(|block| block["type"] == "table")
            .expect("应渲染出 table 块");
        let rows = table["cells"].as_array().expect("table cells");
        assert_eq!(rows.len(), 3, "表头 + 两条数据行: {table}");
        // 表头单元格要标记 is_header，三列都要有。
        let header = rows[0].as_array().unwrap();
        assert_eq!(header.len(), 3);
        assert!(
            header.iter().all(|cell| cell["is_header"] == true),
            "表头单元格应标记 is_header"
        );
        // 2) `## 结论` 必须变成 heading 块，记号被剥掉。
        assert!(!encoded.contains("## 结论"), "标题记号未剥离");
        let heading = blocks
            .iter()
            .find(|block| block["type"] == "heading")
            .expect("应渲染出 heading 块");
        assert_eq!(heading["text"], "结论");
        // 3) 正文标题必须比卡片大标题（size 3）**小**，否则喧宾夺主。
        assert!(
            heading["size"].as_u64().unwrap_or(0) > 3,
            "正文标题应小于卡片标题: {heading}"
        );
    }

    /// 表格对齐要尊重分隔行的 `:` 标记；列数不齐的数据行不能把表打乱。
    #[test]
    fn markdown_table_honors_alignment_and_ragged_rows() {
        let blocks = commentary_entry_blocks(
            "| 左 | 中 | 右 |\n|:---|---:|:---:|\n| a | b | c |\n| 只有一列 |",
        );
        let table = blocks
            .iter()
            .find(|block| block["type"] == "table")
            .expect("table");
        let rows = table["cells"].as_array().unwrap();
        let header = rows[0].as_array().unwrap();
        assert_eq!(header[0]["align"], "left");
        assert_eq!(header[1]["align"], "right");
        assert_eq!(header[2]["align"], "center");
        // 列数不足的数据行补空单元格，保持列数一致。
        let ragged = rows[2].as_array().unwrap();
        assert_eq!(ragged.len(), 3, "缺列应补空: {ragged:?}");
        assert_eq!(ragged[0]["text"], "只有一列");
        assert_eq!(ragged[1]["text"], "");
    }

    /// 不是表格的普通竖线文本不能被误判成表格。
    #[test]
    fn non_table_pipe_text_stays_a_paragraph() {
        let blocks = commentary_entry_blocks("管道符 | 不是表格\n第二行");
        assert!(
            blocks.iter().all(|block| block["type"] != "table"),
            "不应误判为表格: {blocks:?}"
        );
        assert!(
            blocks.iter().any(|block| block["type"] == "paragraph"),
            "应保持段落"
        );
    }

    /// `#1` 这类没有空格分隔的文本不能当成标题吃掉。
    #[test]
    fn hash_without_space_is_not_a_heading() {
        let blocks = commentary_entry_blocks("#1 号方案 与 #2 号方案");
        assert!(
            blocks.iter().all(|block| block["type"] != "heading"),
            "`#1` 不应被当作标题: {blocks:?}"
        );
        assert!(blocks.iter().any(|block| block["type"] == "paragraph"));
    }

    /// 回归：最终回复的 markdown 不能以"原文"形式露出。
    ///
    /// 症状（用户实测）：气泡里直接显示 `[Logo 合集/战双 logo](/Users/...)`，
    /// 以及成排的 `- xxx` 列表原文。
    ///
    /// 原因：最终回复内嵌进 blocks 后，走的是 `commentary_entry_blocks`，而它当时
    /// ① 只把 http(s) 链接转成 url（本地路径原样输出）② 完全不处理 `- ` 列表。
    /// 旧路径用的是 `TelegramInputRichMessage::markdown(..)`，由 Telegram 原生渲染，
    /// 所以这是内嵌改造引入的回退。
    #[test]
    fn commentary_markdown_does_not_leak_raw_syntax() {
        let reply = "找到了，已整理到 [Logo 合集/战双 logo](/Users/miaopasi/Downloads/Logo 合集/战双 logo)（Finder 已打开）。\n核心的几张：\n- [01 游戏 LOGO](/Users/miaopasi/a.png) —— 早期 LOGO\n- [02 Steam 头图](/Users/miaopasi/b.jpg)\n_更多_ 里还有一张维基版图标。来源都写在 [README.md](https://example.com/r.md) 里。";
        let blocks = commentary_entry_blocks(reply);
        let encoded = serde_json::to_string(&serde_json::Value::Array(blocks.clone())).unwrap();

        // 1) 本地路径链接不能以 `[文字](/Users/...)` 原文出现。
        assert!(
            !encoded.contains("](/Users/"),
            "本地路径链接原文泄漏: {encoded}"
        );
        // 2) 链接文字要保留为可读文本。
        assert!(encoded.contains("Logo 合集/战双 logo"), "链接文字应保留");
        assert!(encoded.contains("01 游戏 LOGO"), "列表项链接文字应保留");
        // 3) `- ` 列表要变成 list 块，而不是带 `-` 的普通段落。
        let list = blocks
            .iter()
            .find(|block| block["type"] == "list")
            .expect("`- ` 列表应渲染为 list 块");
        assert_eq!(
            list["items"].as_array().map(Vec::len),
            Some(2),
            "两条列表项应合入同一个 list: {list}"
        );
        // 4) 斜体 `_更多_` 的记号要剥掉。
        assert!(!encoded.contains("_更多_"), "斜体记号未剥离: {encoded}");
        assert!(encoded.contains("更多"), "斜体文字应保留");
        // 5) http 链接仍要转成可点 url。
        assert!(encoded.contains("https://example.com/r.md"));
        assert!(encoded.contains(r#""type":"url""#));
    }

    /// 回归：思考摘要必须用 `code`（等宽蓝底）渲染，且剥掉内联 markdown 记号。
    ///
    /// 需求：让它和底部 `turn <id>` 一样显示成蓝色。`code` 是**字面量**渲染、
    /// 不解析 markdown，所以 `**Check**` 必须被剥成 `Check`，否则界面上会出现星号。
    #[test]
    fn reasoning_summary_renders_as_blue_code_without_markdown_markers() {
        let snapshot = TelegramCommandProgressSnapshot {
            turn_id: "turn".to_string(),
            revision: 1,
            message_id: None,
            entries: Vec::new(),
            dropped_entries: 0,
            retry_count: 0,
            retry_error: None,
            reasoning_summary: Some(
                "**Check** the `Telegram` [state](https://telegram.org)".to_string(),
            ),
            plan_explanation: None,
            plan: Vec::new(),
            diff_summary: None,
            web_searches: Vec::new(),
            dropped_web_searches: 0,
            commentary: Vec::new(),
            commentary_dropped_entries: 0,
            collab: None,
            completed: false,
            failed: false,
        };
        let rendered = render_command_progress(&snapshot, ImText::zh_cn());
        let blocks =
            serde_json::Value::Array(render_task_progress(&snapshot, ImText::zh_cn()).blocks);
        let encoded = serde_json::to_string(&blocks).expect("serialize");

        // 必须有一个 code 类型的段落，内容已剥掉 markdown 记号。
        let code_block = blocks
            .as_array()
            .unwrap()
            .iter()
            .find(|block| block["type"] == "paragraph" && block["text"]["type"] == "code")
            .expect("思考摘要应渲染为 code 块（蓝色）");
        let code_text = code_block["text"]["text"].as_str().unwrap();
        assert!(
            code_text.contains("Check") && code_text.contains("Telegram"),
            "应保留可读文字，实际: {code_text:?}"
        );
        assert!(!code_text.contains("**"), "不应残留 ** 记号: {code_text:?}");
        assert!(!code_text.contains('`'), "不应残留反引号: {code_text:?}");
        assert!(
            !code_text.contains("https://"),
            "链接应压成文字，实际: {code_text:?}"
        );
        assert!(!encoded.contains("**Check**"));
        assert!(rendered.contains("Check"));
    }

    /// 短思考摘要不截断、不加省略号。
    #[test]
    fn short_reasoning_summary_is_not_ellipsized() {
        let snapshot = TelegramCommandProgressSnapshot {
            turn_id: "turn".to_string(),
            revision: 1,
            message_id: None,
            entries: Vec::new(),
            dropped_entries: 0,
            retry_count: 0,
            retry_error: None,
            reasoning_summary: Some("checking the build output".to_string()),
            plan_explanation: None,
            plan: Vec::new(),
            diff_summary: None,
            web_searches: Vec::new(),
            dropped_web_searches: 0,
            commentary: Vec::new(),
            commentary_dropped_entries: 0,
            collab: None,
            completed: true,
            failed: false,
        };
        let rendered = render_command_progress(&snapshot, ImText::zh_cn());
        assert!(rendered.contains("checking the build output"));
        assert!(!rendered.contains("checking the build output…"));
    }

    /// 思考文案与工具步骤必须按到达序号交错：文案可见、工具折叠、顺序与发生顺序一致。
    /// 回归：展示窗口必须是**连续的最新一段**，否则刚完成的步骤会"闪一下就消失"。
    ///
    /// 曾经从前往后 `.take(N)` 取填充窗口，拿到的是**最旧**的 N 条。步骤一完成就
    /// 掉出"最后 3 条"的优先级窗口，又不在最旧的 N 条里，于是从气泡中消失。
    ///
    /// 只断言"最新一条可见"抓不到这个 bug（它始终在优先级窗口里）；正确的特征是
    /// **窗口必须是从最新一条往回的连续区间，中间没有空洞**。
    /// 回归：工具数超过名额上限时，思考**不能**全被挤到一起。
    ///
    /// 症状（用户实测）：任务前面显示正常，步数涨上去后"思考过程和工具摘要
    /// 突然全部合并"。
    ///
    /// 原因：名额原先只按"最近的 N 条"分配，全堆在时间轴末端。总量一超过名额，
    /// 中段工具被整体挤出，思考之间失去间隔，于是各自并成一大批。
    ///
    /// 修法：为每条思考锚定它前后紧邻的工具，剩余名额再按最近补齐。
    #[test]
    fn thinking_stays_interleaved_when_tool_count_exceeds_the_budget() {
        // 三条思考分布在工具流的前段，之后是大量工具——正是长任务的实际形态。
        for tool_count in [30usize, 44, 60, 100] {
            let commentary_sequences = [0u64, 8, 16];
            let mut entries = Vec::new();
            let mut sequence = 1u64;
            for _ in 0..tool_count {
                while commentary_sequences.contains(&sequence) {
                    sequence += 1;
                }
                let mut entry = completed_entry(
                    &format!("cmd-{sequence}"),
                    &json!({"command": "cargo test"}),
                );
                entry.sequence = sequence;
                entries.push(entry);
                sequence += 1;
            }

            let snapshot = TelegramCommandProgressSnapshot {
                turn_id: "turn".to_string(),
                revision: 1,
                message_id: None,
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
                commentary: commentary_sequences
                    .iter()
                    .map(|sequence| TelegramCommentaryEntry {
                        item_id: format!("think-{sequence}"),
                        text: "思考".to_string(),
                        sequence: *sequence,
                    })
                    .collect(),
                commentary_dropped_entries: 0,
                collab: None,
                completed: false,
                failed: false,
            };

            let shown = interleaved_tool_indices(&snapshot);

            // 组装排序后的条目序列（思考 + 命中的工具），断言**没有两条思考相邻**。
            //
            // 这正是"思考过程全部合并"的直接特征：相邻同类会被合并成一个批次。
            let mut items: Vec<(u64, bool)> = snapshot
                .commentary
                .iter()
                .map(|entry| (entry.sequence, true))
                .chain(
                    shown
                        .iter()
                        .map(|index| (snapshot.entries[*index].sequence, false)),
                )
                .collect();
            items.sort_unstable();

            for window in items.windows(2) {
                assert!(
                    !(window[0].1 && window[1].1),
                    "共 {tool_count} 个工具时，思考 {:?} 与 {:?} 相邻，\
                     会被合并成一批：{items:?}",
                    window[0].0,
                    window[1].0
                );
            }
        }
    }

    #[test]
    fn recent_tool_window_is_contiguous_so_completed_steps_do_not_vanish() {
        for count in [4usize, 13, 16, 20, 40] {
            let entries: Vec<TelegramCommandProgressEntry> = (0..count)
                .map(|index| {
                    let mut entry =
                        completed_entry(&format!("cmd-{index}"), &json!({"command": "cargo test"}));
                    entry.sequence = index as u64;
                    entry
                })
                .collect();

            let snapshot = TelegramCommandProgressSnapshot {
                turn_id: "turn".to_string(),
                revision: 1,
                message_id: None,
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
                commentary: Vec::new(),
                commentary_dropped_entries: 0,
                collab: None,
                completed: false,
                failed: false,
            };

            let shown = interleaved_tool_indices(&snapshot);
            let max_index = snapshot.entries.len() - 1;
            // 期望：从最新一条往回、长度为 shown.len() 的连续区间。
            // 旧实现取"最旧 N 条"，会在中段留下空洞（例如 16 条时缺下标 12），
            // 这正是"闪一下就消失"的表现。
            let expected: Vec<usize> =
                ((max_index + 1).saturating_sub(shown.len())..=max_index).collect();
            assert_eq!(
                shown, expected,
                "共 {count} 条时展示窗口不是连续的最新区间（中间有空洞）"
            );
        }
    }

    #[test]
    fn render_task_progress_interleaves_commentary_and_tool_batches() {
        let mut tool_a = completed_entry("tool-a", &json!({"command": "cargo test"}));
        tool_a.sequence = 1;
        let mut tool_b = completed_entry("tool-b", &json!({"command": "cargo build"}));
        tool_b.sequence = 3;

        let rendered = render_task_progress(
            &TelegramCommandProgressSnapshot {
                turn_id: "turn-interleave".to_string(),
                revision: 1,
                message_id: None,
                entries: vec![tool_a, tool_b],
                dropped_entries: 0,
                retry_count: 0,
                retry_error: None,
                reasoning_summary: None,
                plan_explanation: None,
                plan: Vec::new(),
                diff_summary: None,
                web_searches: Vec::new(),
                dropped_web_searches: 0,
                commentary: vec![
                    TelegramCommentaryEntry {
                        item_id: "c1".to_string(),
                        text: "先看事件流".to_string(),
                        sequence: 0,
                    },
                    TelegramCommentaryEntry {
                        item_id: "c2".to_string(),
                        text: "再看渲染".to_string(),
                        sequence: 2,
                    },
                ],
                commentary_dropped_entries: 0,
                collab: None,
                completed: false,
                failed: false,
            },
            ImText::zh_cn(),
        );

        // 顶层块序列：思考批次与工具批次交替出现。
        let summaries: Vec<String> = rendered
            .blocks
            .iter()
            .filter(|block| block["type"] == "details")
            .map(|block| {
                format!(
                    "{}|open={}",
                    block["summary"].as_str().unwrap_or(""),
                    block["is_open"].as_bool().unwrap_or(false)
                )
            })
            .collect();

        assert_eq!(
            summaries,
            vec![
                // 思考默认展开（is_open=true），工具默认折叠。
                "思考过程（1）|open=true",
                "工具摘要（1）|open=false",
                "思考过程（1）|open=true",
                "工具摘要（1）|open=false",
            ],
            "思考与工具应按到达序号交错，且思考展开、工具折叠"
        );

        // 文案内容确实在「思考过程」块里，而不是被折叠丢弃。
        let first_thinking = rendered
            .blocks
            .iter()
            .find(|block| block["type"] == "details" && block["summary"] == "思考过程（1）")
            .expect("思考过程块");
        let encoded = first_thinking.to_string();
        assert!(encoded.contains("先看事件流"), "思考块应含文案：{encoded}");
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
                commentary: Vec::new(),
                commentary_dropped_entries: 0,
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
        let tools_index = blocks
            .as_array()
            .unwrap()
            .iter()
            // 标题显示的是**该批次**的步骤数（与图中 `工具摘要（1）` 的口径一致），
            // 历史省略数由面板内的"...另外 N 个较早步骤"提示。
            .position(|block| block["type"] == "details" && block["summary"] == "工具摘要（2）")
            .expect("tools should be folded into one summary panel");
        let panel = blocks[tools_index]["blocks"]
            .as_array()
            .expect("tools panel blocks");
        assert_eq!(panel[0]["type"], "paragraph");
        assert_eq!(panel[0]["text"]["type"], "bold");
        assert_eq!(panel[0]["text"]["text"], "执行中 · 4 步 · 1 个进行中");
        assert_eq!(panel[1]["type"], "pre");
        assert_eq!(panel[1]["language"], "shell");
        assert!(
            panel
                .iter()
                .any(|block| block["text"] == "… 另外 2 个较早步骤")
        );
        assert_eq!(
            blocks
                .as_array()
                .unwrap()
                .iter()
                .filter(|block| block["type"] == "divider")
                .count(),
            0,
            "the folded tools summary replaces the plan-to-execution divider"
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
        // 思考摘要已去掉标题，并改为 `code`（等宽蓝底，与底部 `turn <id>` 一致）。
        assert!(!encoded.contains("思考摘要"), "不应再渲染「思考摘要」标题");
        let reasoning_body = blocks
            .as_array()
            .unwrap()
            .iter()
            .find(|block| {
                block["type"] == "paragraph"
                    && block["text"]["type"] == "code"
                    && block["text"]["text"]
                        .as_str()
                        .is_some_and(|t| t.contains("Check"))
            })
            .expect("reasoning body should be always visible");
        assert_eq!(reasoning_body["type"], "paragraph");
        assert_eq!(reasoning_body["text"]["type"], "code");
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
        assert!(
            rendered
                .fallback_markdown
                .starts_with("🔄 任务进行中\n──────────────")
        );
        let fallback_plan = rendered
            .fallback_markdown
            .find("计划 · 1/2")
            .expect("fallback plan heading");
        let fallback_execution = rendered
            .fallback_markdown
            .find("执行中 · 4 步 · 1 个进行中")
            .expect("fallback execution heading");
        // 思考摘要已去掉标题，用正文内容定位它在 fallback 中的位置。
        let fallback_reasoning = rendered
            .fallback_markdown
            .find("Check")
            .expect("fallback reasoning body");
        let fallback_diff = rendered
            .fallback_markdown
            .find("文件修改 · 1 个文件 · +12 -3")
            .expect("fallback diff heading");
        assert!(fallback_plan < fallback_execution);
        assert!(fallback_execution < fallback_reasoning);
        assert!(fallback_reasoning < fallback_diff);
        // 卡片头（🔄/❌）现在是 fallback 的合法组成部分，不再视为泄漏。
        for marker in ["✅", "⚠️", "⏳", "🛠"] {
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
                commentary: Vec::new(),
                commentary_dropped_entries: 0,
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
                commentary: Vec::new(),
                commentary_dropped_entries: 0,
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
                commentary: Vec::new(),
                commentary_dropped_entries: 0,
                collab: None,
                completed: true,
                failed: true,
            },
            ImText::zh_cn(),
        );

        let tools = rendered
            .blocks
            .iter()
            .find(|block| block["type"] == "details" && block["summary"] == "工具摘要（1）")
            .expect("tools summary panel");
        let panel = tools["blocks"].as_array().expect("tools panel blocks");
        let error_details = panel
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

        let command_block_index = panel
            .iter()
            .position(|block| block["type"] == "pre" && block["language"] == "shell")
            .expect("command block");
        assert_eq!(
            panel[command_block_index],
            json!({
                "type": "pre",
                "text": "prefix-xxxxxxxxxxxxxxxxxxx...xxxxxxxxxxxxxxxxxxxx-suffix",
                "language": "shell",
            })
        );
        assert_eq!(panel[command_block_index + 1]["type"], "footer");
        assert_eq!(
            panel[command_block_index + 1]["text"],
            json!([
                {"type": "bold", "text": "失败"},
                " · exit 1",
            ])
        );
        assert!(rendered.fallback_markdown.contains(&command));
    }

    #[test]
    fn rich_progress_without_plan_folds_the_only_step_into_the_tools_summary() {
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
                commentary: Vec::new(),
                commentary_dropped_entries: 0,
                collab: None,
                completed: false,
                failed: false,
            },
            ImText::zh_cn(),
        );

        let blocks = serde_json::Value::Array(rendered.blocks);
        assert_eq!(blocks[0]["type"], "heading");
        assert_eq!(blocks[0]["text"], "执行中 · 1 步 · 1 个进行中");
        assert_eq!(blocks[1]["type"], "details");
        assert_eq!(blocks[1]["summary"], "工具摘要（1）");
        let panel = blocks[1]["blocks"].as_array().expect("tools panel blocks");
        assert_eq!(panel[0]["type"], "pre");
        assert_eq!(panel[0]["language"], "shell");
        assert_eq!(panel[1]["type"], "footer");
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
                .starts_with("🔄 执行中 · 1 步 · 1 个进行中\n──────────────")
        );
    }

    #[test]
    fn commentary_is_visible_and_tools_are_folded() {
        let rendered = render_task_progress(
            &TelegramCommandProgressSnapshot {
                turn_id: "turn".to_string(),
                revision: 3,
                message_id: None,
                entries: vec![{
                    // 真实系统里 upsert 会分配递增序号；这里给工具序号 2，
                    // 表示两条思考之后才发生（避免与文案的序号冲突）。
                    let mut entry =
                        completed_entry("cmd", &json!({"command": "cargo test", "exitCode": 0}));
                    entry.sequence = 2;
                    entry
                }],
                dropped_entries: 0,
                retry_count: 0,
                retry_error: None,
                reasoning_summary: None,
                plan_explanation: None,
                plan: Vec::new(),
                diff_summary: None,
                web_searches: Vec::new(),
                dropped_web_searches: 0,
                commentary: vec![
                    TelegramCommentaryEntry {
                        item_id: "commentary-1".to_string(),
                        text: "先看 `src/im/events.rs`".to_string(),
                        sequence: 0,
                    },
                    TelegramCommentaryEntry {
                        item_id: "commentary-2".to_string(),
                        text: "跑测试：\n```shell\ncargo test\n```".to_string(),
                        sequence: 1,
                    },
                ],
                commentary_dropped_entries: 2,
                collab: None,
                completed: false,
                failed: false,
            },
            ImText::zh_cn(),
        );

        let blocks = serde_json::Value::Array(rendered.blocks.clone());
        let array = blocks.as_array().expect("rich blocks");

        // 思考过程：可折叠但默认展开（is_open = true），内容直接可见。
        // 两条连续的思考文案合并成一张卡片（中间没有工具插入）。
        let thinking = array
            .iter()
            .find(|block| block["type"] == "details" && block["summary"] == "思考过程（2）")
            .expect("thinking card");
        assert!(
            thinking["is_open"].as_bool().unwrap_or(false),
            "思考过程必须默认展开，否则用户读不到内容"
        );
        let thinking_encoded = thinking.to_string();
        assert!(thinking_encoded.contains("先看"), "{thinking_encoded}");
        assert!(thinking_encoded.contains("跑测试"), "{thinking_encoded}");
        // 代码围栏保留在思考块内（不被折叠丢弃）。
        assert!(
            thinking["blocks"]
                .as_array()
                .expect("thinking panel")
                .iter()
                .any(|block| block["type"] == "pre" && block["language"] == "shell"),
            "思考块内应保留代码围栏"
        );

        // 工具摘要：默认折叠。
        let tools = array
            .iter()
            .find(|block| block["type"] == "details" && block["summary"] == "工具摘要（1）")
            .expect("tools summary panel");
        // rich_blocks::details 在折叠（false）时不写 is_open 字段，缺省即折叠。
        assert!(
            !tools["is_open"].as_bool().unwrap_or(false),
            "工具摘要必须默认折叠"
        );

        // 省略提示仍然出现（2 条较早进展被丢弃），且位于思考卡片之前。
        let omitted_block = array
            .iter()
            .position(|block| block["text"] == "… 另外 2 条较早进展已省略")
            .expect("省略提示");
        let thinking_index = array
            .iter()
            .position(|block| block["type"] == "details" && block["summary"] == "思考过程（2）")
            .expect("thinking");
        assert!(omitted_block < thinking_index, "省略提示应在思考卡片之前");
        let tools_index = array
            .iter()
            .position(|block| block["type"] == "details" && block["summary"] == "工具摘要（1）")
            .expect("tools");
        assert!(thinking_index < tools_index);
    }

    #[test]
    fn commentary_and_tools_stay_within_the_fallback_budget() {
        let rendered = render_task_progress(
            &TelegramCommandProgressSnapshot {
                turn_id: "turn".to_string(),
                revision: 5,
                message_id: None,
                entries: vec![completed_entry(
                    "cmd",
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
                commentary: (0..4)
                    .map(|index| TelegramCommentaryEntry {
                        item_id: format!("commentary-{index}"),
                        text: "x".repeat(1_500),
                        sequence: 0,
                    })
                    .collect(),
                commentary_dropped_entries: 0,
                collab: None,
                completed: false,
                failed: false,
            },
            ImText::zh_cn(),
        );

        assert!(
            rendered.fallback_markdown.chars().count()
                <= super::TELEGRAM_TASK_PROGRESS_FALLBACK_MAX_CHARS
        );
        assert!(
            rendered.fallback_markdown.contains("cargo test"),
            "the tools fallback must survive the commentary truncation"
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
