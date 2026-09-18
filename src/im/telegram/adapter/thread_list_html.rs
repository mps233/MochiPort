//! Extracted verbatim from `adapter.rs`; no behavior changes.

use super::*;

pub(super) fn thread_entries_table_html(
    entries: &[TelegramThreadListEntry],
    text: ImText,
) -> String {
    let mut lines = Vec::new();
    let mut current_cwd: Option<&str> = None;
    for (index, entry) in entries.iter().enumerate() {
        let cwd = entry
            .cwd
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if current_cwd != cwd {
            if !lines.is_empty() {
                lines.push(String::new());
            }
            lines.push(project_header_html(cwd, text));
            current_cwd = cwd;
        }
        lines.push(thread_entry_table_html(index, entry, text));
    }
    lines.join("\n")
}

pub(super) fn thread_entry_table_html(
    index: usize,
    entry: &TelegramThreadListEntry,
    text: ImText,
) -> String {
    let title = entry.title.trim();
    let title = if title.is_empty() {
        text.untitled_session()
    } else {
        title
    };
    let title = truncate_display_text(title, 22);
    let state = thread_state_suffix(&entry.state, text)
        .map(|state| format!(" <code>{}</code>", telegram_html_escape(state)))
        .unwrap_or_default();
    format!(
        "/{} <b>{}</b>{state}",
        index + 1,
        telegram_html_escape(&title)
    )
}

pub(super) fn project_header_html(cwd: Option<&str>, text: ImText) -> String {
    match cwd {
        Some(cwd) => {
            let name = project_name(cwd);
            format!(
                "<b>{}</b>\n<code>{}</code>",
                telegram_html_escape(&truncate_display_text(&text.project_header(&name), 32)),
                telegram_html_escape(&truncate_middle(cwd, 68))
            )
        }
        None => format!(
            "<b>{}</b>",
            telegram_html_escape(text.unknown_project_header())
        ),
    }
}

pub(super) fn thread_state_suffix(state: &str, text: ImText) -> Option<&'static str> {
    if state.contains("当前会话") || state.contains("Current session") {
        Some(text.current_short())
    } else if state.contains("已加载") || state.contains("Loaded") {
        Some(text.loaded_short())
    } else {
        None
    }
}

pub(super) fn thread_list_html_text(
    title: &str,
    body: &str,
    page: usize,
    entries_html: &str,
    text: ImText,
) -> String {
    format!(
        "<b>{}</b>\n{}\n\n{}\n\n<code>{}</code>",
        telegram_html_escape(title),
        telegram_markdown_to_html(&telegram_cleanup_text(body)),
        entries_html,
        text.page_label(page)
    )
}

pub(super) fn truncate_display_text(text: &str, max_chars: usize) -> String {
    let text = text
        .replace(['\r', '\n'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if text.chars().count() <= max_chars {
        return text;
    }
    let mut output = text
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    output.push('…');
    output
}

pub(super) fn project_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|value| value.to_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| path.to_string())
}

pub(super) fn looks_like_path(value: &str) -> bool {
    let value = value.trim();
    value.contains('\\') || value.contains('/') || value.starts_with('~')
}

pub(super) fn truncate_middle(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
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
