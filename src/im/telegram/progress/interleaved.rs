//! Extracted verbatim from `progress.rs`; no behavior changes.

use super::*;

/// 工具/命令步骤的折叠块：标题带步骤总数，内部保留优先级筛选与省略说明。
/// 按到达顺序交错渲染「过程文案」与「工具步骤」。
///
/// 规则：
/// - 文案（Codex 的说明文字）保持可见、不折叠——用户要能直接读到思考内容；
/// - 连续的多个工具步骤合并成一个折叠块，标题带该批次的步骤数；
/// - 两者按 `sequence`（到达序号）排序，因此顺序与真实发生顺序一致。
///
/// 超预算的文案由 `commentary::render_commentary` 从最早开始丢弃，并在顶部标注。
pub(super) fn render_interleaved_progress(
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
pub(super) enum InterleavedBatch<'a> {
    Commentary(Vec<&'a commentary::TelegramCommentaryRenderedEntry>),
    Tool(Vec<&'a TelegramCommandProgressEntry>),
}

pub(super) enum InterleavedKind<'a> {
    Commentary(&'a commentary::TelegramCommentaryRenderedEntry),
    Tool(&'a TelegramCommandProgressEntry),
}

pub(super) struct InterleavedItem<'a> {
    sequence: u64,
    kind: InterleavedKind<'a>,
}

/// 把一批连续的工具步骤渲染成一个折叠块（标题带该批次数量）。
pub(super) fn flush_tool_batch(
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

/// 参与交错渲染的工具步骤下标：优先级步骤 + 最近的历史步骤。
///
/// 与折叠块原先的口径一致——折叠块本身不占屏面，展开后能看到较完整的上下文；
/// 只有在交错视图里这些步骤才会和文案一起排序。
pub(super) fn interleaved_tool_indices(snapshot: &TelegramCommandProgressSnapshot) -> Vec<usize> {
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

pub(super) fn selected_entry_indices(entries: &[TelegramCommandProgressEntry]) -> Vec<usize> {
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

pub(super) fn least_important_entry_position(
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
