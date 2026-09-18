//! Extracted verbatim from `progress.rs`; no behavior changes.

use super::*;

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

pub(super) fn rich_command_entry_blocks(
    entry: &TelegramCommandProgressEntry,
    text: ImText,
) -> Vec<Value> {
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

pub(super) fn mcp_tool_text(item: &Value) -> String {
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

pub(super) fn command_text(item: &Value) -> String {
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

pub(super) fn command_value_text(value: &Value) -> Option<String> {
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

pub(super) fn completed_status(item: &Value) -> TelegramCommandProgressStatus {
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

pub(super) fn mcp_completed_status(item: &Value) -> TelegramCommandProgressStatus {
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

pub(super) fn mcp_failure_output(item: &Value) -> Option<String> {
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

pub(super) fn json_value_has_content(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::String(value) => !value.trim().is_empty(),
        Value::Array(values) => !values.is_empty(),
        Value::Object(values) => !values.is_empty(),
        _ => true,
    }
}

pub(super) fn json_value_text(value: &Value) -> String {
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

pub(super) fn failure_output_tail(item: &Value) -> Option<String> {
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

pub(super) fn single_line(text: &str) -> String {
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
pub(super) fn strip_inline_markdown(text: &str) -> String {
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
pub(super) fn truncate_text_with_ellipsis(text: &str, max_chars: usize) -> String {
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

pub(super) fn compact_text(text: &str, max_chars: usize) -> String {
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

pub(super) fn file_name(path: &str) -> &str {
    let trimmed = path.trim().trim_matches('"');
    trimmed
        .rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or(trimmed)
}

pub(super) fn truncate_middle(text: &str, max_chars: usize) -> String {
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

pub(super) fn truncate_tail(text: &str, max_chars: usize) -> String {
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

pub(super) fn format_duration(duration_ms: u64) -> String {
    if duration_ms < 1_000 {
        format!("{duration_ms}ms")
    } else {
        format!("{:.1}s", duration_ms as f64 / 1_000.0)
    }
}
