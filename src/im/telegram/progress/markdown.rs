//! Extracted verbatim from `progress.rs`; no behavior changes.

use super::*;

pub(super) fn commentary_entry_blocks(entry: &str) -> Vec<Value> {
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
pub(super) fn commentary_table_block(lines: &[&str]) -> Option<(Value, usize)> {
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

pub(super) fn is_table_row(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.len() >= 2 && trimmed.starts_with('|') && trimmed.ends_with('|')
}

pub(super) fn split_table_row(line: &str) -> Vec<&str> {
    let trimmed = line.trim();
    let inner = trimmed.strip_prefix('|').unwrap_or(trimmed);
    let inner = inner.strip_suffix('|').unwrap_or(inner);
    inner.split('|').map(str::trim).collect()
}

/// 解析分隔行（`|---|:---:|---:|`）得到每列对齐方式；不是分隔行时返回 `None`。
pub(super) fn table_separator_alignments(line: &str) -> Option<Vec<&'static str>> {
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
pub(super) fn heading_line(line: &str) -> Option<(usize, &str)> {
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
pub(super) fn heading_render_size(level: usize) -> u8 {
    match level {
        0..=2 => 4,
        3 | 4 => 5,
        _ => 6,
    }
}

/// 取 `- xxx` / `* xxx` / `+ xxx` 的项目正文；不是列表行时返回 `None`。
pub(super) fn bullet_item_text(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let rest = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
        .or_else(|| trimmed.strip_prefix("+ "))?;
    let rest = rest.trim();
    (!rest.is_empty()).then_some(rest)
}

pub(super) fn flush_commentary_list(blocks: &mut Vec<Value>, list_items: &mut Vec<Value>) {
    if !list_items.is_empty() {
        blocks.push(rich_blocks::list(std::mem::take(list_items)));
    }
}

/// 把已是富文本的 value 还原成纯文字（用于续行拼接）。
pub(super) fn inline_plain_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts.iter().map(inline_plain_text).collect(),
        Value::Object(map) => map.get("text").map(inline_plain_text).unwrap_or_default(),
        _ => String::new(),
    }
}

pub(super) fn push_commentary_paragraph(blocks: &mut Vec<Value>, paragraph: &mut String) {
    let trimmed = paragraph.trim();
    if !trimmed.is_empty() {
        blocks.push(rich_blocks::paragraph(rich_blocks::inline_markdown(
            trimmed,
        )));
    }
    paragraph.clear();
}

/// 提取围栏行上的语言标注；仅保留安全字符。
pub(super) fn fence_language(line: &str) -> Option<String> {
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
