//! Extracted verbatim from `adapter.rs`; no behavior changes.

use super::*;

pub(super) fn telegram_markdown_to_html(text: &str) -> String {
    let text = telegram_cleanup_text(text);
    let mut html = String::new();
    let mut in_code_block = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            if in_code_block {
                html.push_str("</code></pre>\n");
                in_code_block = false;
            } else {
                match fence_language(line) {
                    Some(language) => {
                        html.push_str("<pre><code class=\"language-");
                        html.push_str(&language);
                        html.push_str("\">");
                    }
                    None => html.push_str("<pre><code>"),
                }
                in_code_block = true;
            }
            continue;
        }
        if in_code_block {
            html.push_str(&telegram_html_escape(line));
            html.push('\n');
        } else {
            html.push_str(&telegram_inline_markdown_to_html(line));
            html.push('\n');
        }
    }
    if in_code_block {
        html.push_str("</code></pre>");
    }
    html.trim_end().to_string()
}

pub(super) fn telegram_inline_markdown_to_html(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("**")
            && let Some(end) = after.find("**")
        {
            out.push_str("<b>");
            out.push_str(&telegram_html_escape(&after[..end]));
            out.push_str("</b>");
            rest = &after[end + 2..];
            continue;
        }
        if let Some(after) = rest.strip_prefix('`')
            && let Some(end) = after.find('`')
        {
            out.push_str("<code>");
            out.push_str(&telegram_html_escape(&after[..end]));
            out.push_str("</code>");
            rest = &after[end + 1..];
            continue;
        }
        if let Some(after_label) = rest.strip_prefix('[')
            && let Some(label_end) = after_label.find("](")
            && let Some(url_end) = after_label[label_end + 2..].find(')')
        {
            let label = &after_label[..label_end];
            let url = &after_label[label_end + 2..label_end + 2 + url_end];
            if url.starts_with("http://") || url.starts_with("https://") {
                out.push_str("<a href=\"");
                out.push_str(&telegram_html_attr_escape(url));
                out.push_str("\">");
                out.push_str(&telegram_html_escape(label));
                out.push_str("</a>");
            } else {
                out.push_str(&telegram_html_escape(label));
            }
            rest = &after_label[label_end + 2 + url_end + 1..];
            continue;
        }
        let ch = rest.chars().next().expect("rest is non-empty");
        out.push_str(&telegram_html_escape(&ch.to_string()));
        rest = &rest[ch.len_utf8()..];
    }
    out
}

pub(super) fn telegram_cleanup_text(text: &str) -> String {
    strip_codex_ui_directives(text)
        .replace("<font color='grey'>", "")
        .replace("<font color=\"grey\">", "")
        .replace("</font>", "")
}

pub(super) fn strip_codex_ui_directives(text: &str) -> String {
    let mut in_fenced_code = false;
    let mut removed_any = false;
    let mut lines = Vec::new();

    for raw_line in text.lines() {
        let line = raw_line.trim_end_matches('\r');
        if !in_fenced_code && is_codex_ui_directive_line(line) {
            removed_any = true;
            continue;
        }
        lines.push(line);
        if is_markdown_fence_line(line) {
            in_fenced_code = !in_fenced_code;
        }
    }

    if !removed_any {
        return text.to_string();
    }

    let mut output = String::new();
    for line in lines {
        let is_blank = line.trim().is_empty();
        if is_blank && output.ends_with('\n') {
            continue;
        }
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str(line);
    }
    output.trim().to_string()
}

pub(super) fn is_codex_ui_directive_line(line: &str) -> bool {
    let line = line.trim();
    let Some(rest) = line.strip_prefix("::") else {
        return false;
    };
    let Some(open_brace) = rest.find('{') else {
        return false;
    };
    let name = &rest[..open_brace];
    let arguments = &rest[open_brace + 1..];
    arguments.ends_with('}')
        && matches!(
            name,
            "code-comment"
                | "git-commit"
                | "git-create-branch"
                | "git-create-pr"
                | "git-push"
                | "git-stage"
        )
}

pub(super) fn is_markdown_fence_line(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with("```") || line.starts_with("~~~")
}

/// 极简卡片：`**✅ 已完成 · 耗时**` + 分隔线 + 正文；署名不再随正文发送。
pub(super) fn telegram_turn_completed_messages(reply_text: &str, header: &str) -> (String, String) {
    let reply_text = telegram_cleanup_text(reply_text).trim().to_string();
    let header = header.trim();
    let separator = crate::im::telegram::rich_blocks::TELEGRAM_CARD_SEPARATOR;
    if reply_text.is_empty() {
        return (format!("**{header}**"), header.to_string());
    }
    (
        format!("**{header}**\n{separator}\n\n{reply_text}"),
        format!("{header}\n{separator}\n\n{reply_text}"),
    )
}

pub(super) fn format_turn_elapsed(locale: ImLocale, elapsed_ms: u128) -> String {
    let total_seconds = (elapsed_ms / 1000) as u64;
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;
    match locale {
        ImLocale::ZhCn => {
            if hours > 0 {
                format!("{hours}小时{minutes}分")
            } else if minutes > 0 {
                format!("{minutes}分{seconds}秒")
            } else {
                format!("{seconds}秒")
            }
        }
        ImLocale::EnUs => {
            if hours > 0 {
                format!("{hours}h{minutes:02}m")
            } else if minutes > 0 {
                format!("{minutes}m{seconds:02}s")
            } else {
                format!("{seconds}s")
            }
        }
    }
}

pub(super) fn telegram_user_message_messages(
    message_text: &str,
    credit_text: &str,
) -> (String, String) {
    let message_text = telegram_cleanup_text(message_text).trim().to_string();
    let credit_text = credit_text.trim();
    let rich_body = telegram_markdown_to_html(&message_text);
    let rich_credit = telegram_html_escape(credit_text);
    let rich_html = format!("<blockquote>{rich_body}\n<cite>{rich_credit}</cite></blockquote>");
    let fallback_markdown = if credit_text.is_empty() {
        message_text
    } else if message_text.is_empty() {
        credit_text.to_string()
    } else {
        format!("{message_text}\n\n{credit_text}")
    };
    (rich_html, fallback_markdown)
}

pub(super) fn telegram_context_compaction_messages(
    title_text: &str,
    credit_text: &str,
) -> (String, String) {
    let title_text = title_text.trim();
    let credit_text = credit_text.trim();
    let rich_title = telegram_html_escape(title_text);
    let rich_credit = telegram_html_escape(credit_text);
    let rich_html = if credit_text.is_empty() {
        format!("<aside>{rich_title}</aside>")
    } else {
        format!("<aside>{rich_title}<cite>{rich_credit}</cite></aside>")
    };
    let fallback_text = if credit_text.is_empty() {
        title_text.to_string()
    } else if title_text.is_empty() {
        credit_text.to_string()
    } else {
        format!("{title_text}\n\n{credit_text}")
    };
    (rich_html, fallback_text)
}

pub(super) fn telegram_html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub(super) fn telegram_html_attr_escape(text: &str) -> String {
    telegram_html_escape(text).replace('"', "&quot;")
}

pub(super) fn truncate_button_text(text: &str) -> String {
    const MAX: usize = 48;
    let text = text.trim();
    if text.chars().count() <= MAX {
        return text.to_string();
    }
    let mut output = text.chars().take(MAX.saturating_sub(1)).collect::<String>();
    output.push('…');
    output
}

pub(super) fn telegram_text_chunks(
    text: &str,
    continues_marker: &str,
    continued_marker: &str,
) -> Vec<String> {
    telegram_text_chunks_with_limit(
        text,
        TELEGRAM_MAX_MESSAGE_CHARS,
        continues_marker,
        continued_marker,
    )
}

pub(super) fn telegram_turn_completed_chunks(
    text: &str,
    header: &str,
    continues_marker: &str,
    continued_marker: &str,
) -> Vec<String> {
    // 头部 + 分隔线 + 分段空行的字符预算。
    let reserved_chars = header.trim().chars().count()
        + crate::im::telegram::rich_blocks::TELEGRAM_CARD_SEPARATOR
            .chars()
            .count()
        + 4;
    let max_chars = TELEGRAM_MAX_MESSAGE_CHARS
        .saturating_sub(reserved_chars)
        .max(TELEGRAM_CONTINUATION_OVERHEAD + 1);
    telegram_text_chunks_with_limit(text, max_chars, continues_marker, continued_marker)
}

pub(super) fn telegram_user_message_chunks(
    text: &str,
    credit_text: &str,
    continues_marker: &str,
    continued_marker: &str,
) -> Vec<String> {
    let reserved_chars = credit_text.trim().chars().count().saturating_add(2);
    let max_chars = TELEGRAM_MAX_MESSAGE_CHARS
        .saturating_sub(reserved_chars)
        .max(TELEGRAM_CONTINUATION_OVERHEAD + 1);
    telegram_text_chunks_with_limit(text, max_chars, continues_marker, continued_marker)
}

pub(super) fn telegram_text_chunks_with_limit(
    text: &str,
    max_chars: usize,
    continues_marker: &str,
    continued_marker: &str,
) -> Vec<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return vec![" ".to_string()];
    }
    if trimmed.chars().count() <= max_chars {
        return vec![trimmed.to_string()];
    }

    // 含代码围栏时预留跨段补围栏的开销，避免补齐后超出单条上限。
    let effective_max = if trimmed.contains("```") {
        max_chars.saturating_sub(TELEGRAM_FENCE_RESERVE)
    } else {
        max_chars
    };
    let chunks = rebalance_code_fences(split_message_for_telegram(trimmed, effective_max));
    let chunk_count = chunks.len();
    chunks
        .into_iter()
        .enumerate()
        .map(|(index, chunk)| {
            if index == 0 {
                format!("{chunk}\n\n{continues_marker}")
            } else if index + 1 == chunk_count {
                format!("{continued_marker}\n\n{chunk}")
            } else {
                format!("{continued_marker}\n\n{chunk}\n\n{continues_marker}")
            }
        })
        .collect()
}

/// 提取 ``` 围栏行上的语言标注；仅保留安全字符，用于 `<code class="language-…">`。
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
    if language.is_empty() {
        None
    } else {
        Some(language)
    }
}

/// 切块可能落在代码围栏中间：跨段时补齐闭合/起始围栏，
/// 否则后续分段会被当普通文本渲染，样式全部丢失。
pub(super) fn rebalance_code_fences(chunks: Vec<String>) -> Vec<String> {
    let mut rebalanced = Vec::with_capacity(chunks.len());
    let mut open_language: Option<String> = None;
    for chunk in chunks {
        let mut body = String::with_capacity(chunk.len() + 16);
        if let Some(language) = &open_language {
            body.push_str("```");
            body.push_str(language);
            body.push('\n');
        }
        for line in chunk.lines() {
            body.push_str(line);
            body.push('\n');
            if line.trim_start().starts_with("```") {
                open_language = match open_language {
                    Some(_) => None,
                    None => fence_language(line),
                };
            }
        }
        if open_language.is_some() {
            body.push_str("```");
        }
        rebalanced.push(body.trim_end().to_string());
    }
    rebalanced
}

pub(super) fn split_message_for_telegram(message: &str, max_chars: usize) -> Vec<String> {
    let content_limit = max_chars.saturating_sub(TELEGRAM_CONTINUATION_OVERHEAD);

    let mut chunks = Vec::new();
    let mut remaining = message;
    while !remaining.is_empty() {
        if remaining.chars().count() <= content_limit {
            chunks.push(remaining.to_string());
            break;
        }

        let hard_split = remaining
            .char_indices()
            .nth(content_limit)
            .map_or(remaining.len(), |(idx, _)| idx);
        let search_area = &remaining[..hard_split];
        let chunk_end = best_split_point(search_area, hard_split, content_limit);

        chunks.push(remaining[..chunk_end].trim_end().to_string());
        remaining = remaining[chunk_end..].trim_start();
    }
    chunks
}

pub(super) fn best_split_point(
    search_area: &str,
    hard_split: usize,
    content_limit: usize,
) -> usize {
    if let Some(pos) = search_area.rfind('\n')
        && search_area[..pos].chars().count() >= content_limit / 2
    {
        return pos + 1;
    }
    if let Some(pos) = search_area.rfind(' ')
        && search_area[..pos].chars().count() >= content_limit / 2
    {
        return pos + 1;
    }
    hard_split
}
