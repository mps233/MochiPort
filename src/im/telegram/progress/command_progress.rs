//! Extracted verbatim from `progress.rs`; no behavior changes.

use super::*;

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

pub(super) fn render_web_search_progress_blocks(
    snapshot: &TelegramCommandProgressSnapshot,
) -> Vec<Value> {
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

pub(super) fn command_progress_status_key(status: TelegramCommandProgressStatus) -> &'static str {
    match status {
        TelegramCommandProgressStatus::Running => "running",
        TelegramCommandProgressStatus::Interrupted => "interrupted",
        TelegramCommandProgressStatus::Succeeded => "succeeded",
        TelegramCommandProgressStatus::Failed => "failed",
    }
}

pub(super) fn short_identifier(value: &str) -> String {
    const MAX: usize = 8;
    let value = value.trim();
    if value.chars().count() <= MAX {
        return value.to_string();
    }
    format!("{}…", value.chars().take(MAX).collect::<String>())
}

pub(super) fn render_command_progress_with_limits(
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
    // 富消息不可用时的回退：最终回复直接以原文附在末尾（无法折叠）。
    if let Some(final_reply) = snapshot
        .final_reply
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        sections.push(format!(
            "{}\n{}",
            text.telegram_final_reply_heading(),
            truncate_middle(final_reply, TELEGRAM_FINAL_REPLY_MAX_CHARS)
        ));
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

pub(super) fn command_progress_title(
    snapshot: &TelegramCommandProgressSnapshot,
    text: ImText,
) -> String {
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
    let title = if total == 0 && snapshot.retry_count > 0 {
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
    };
    // turn 结束后把耗时拼到标题（原来在单独的「✅ 已完成」气泡头上）。
    // 只在**已结束**时拼：进行中的气泡显示一个还在涨的耗时没有意义。
    match snapshot
        .completed
        .then_some(snapshot.elapsed_ms)
        .flatten()
        .filter(|ms| *ms >= 1_000)
    {
        Some(ms) => format!("{title} · {}", text.telegram_turn_elapsed(ms)),
        None => title,
    }
}

pub(super) fn command_execution_progress_title(
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

pub(super) fn render_supplemental_progress(
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

pub(super) fn render_web_search_progress(
    snapshot: &TelegramCommandProgressSnapshot,
) -> Option<String> {
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
