use serde_json::Value;

use crate::{
    im::core::i18n::ImText,
    im::runtime::{
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
/// 「最终回复」折叠块正文的字符预算。
///
/// 整个气泡另有 `TELEGRAM_COMMAND_PROGRESS_MAX_CHARS`（3600）兜底；这里先给最终
/// 回复留一段确定的额度，避免它把工具步骤挤到完全看不见，也避免超长回复让
/// 末尾的 `truncate_middle` 从中间截断正文。
const TELEGRAM_FINAL_REPLY_MAX_CHARS: usize = 2_400;
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

mod command_progress;
mod diff;
mod entries;
mod interleaved;
mod markdown;
mod plan;
#[cfg(test)]
mod tests;

use command_progress::*;
use diff::*;
use entries::*;
use interleaved::*;
use markdown::*;
use plan::*;

// External modules (events.rs, bridge.rs, ...) reach these through
// `telegram_progress`, so they are re-exported at crate visibility. They were
// `pub(crate)` before the split and keep that visibility.
pub(crate) use command_progress::render_command_progress;
pub(crate) use diff::{
    diff_summary_from_diff, diff_summary_from_item, file_change_diff_summary,
    render_diff_standalone,
};
pub(crate) use entries::{
    completed_entry, mcp_completed_entry, mcp_running_entry, reasoning_render_line, render_entry,
    running_entry,
};
pub(crate) use plan::{plan_from_item, render_plan_standalone};

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

    // 「最终回复」内嵌在完成气泡里，而不是单独再发一条气泡。
    //
    // 用 `details(.., is_open = true)`：和上面的「思考过程」「工具摘要」同一套折叠
    // 组件，读者可以随时收起；默认展开，因为这是整个 turn 最该先看到的内容。
    if let Some(final_reply) = snapshot
        .final_reply
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        blocks.push(rich_blocks::details(
            rich_blocks::text(text.telegram_final_reply_heading()),
            commentary_entry_blocks(&truncate_middle(
                final_reply,
                TELEGRAM_FINAL_REPLY_MAX_CHARS,
            )),
            true,
        ));
    }

    blocks.push(rich_blocks::footer(rich_blocks::rich_text(vec![
        rich_blocks::text("turn "),
        rich_blocks::code(short_identifier(&snapshot.turn_id)),
    ])));
    blocks
}
