use std::path::PathBuf;

use serde_json::Value;

const SUMMARY_CHAR_LIMIT: usize = 2400;
const JSON_CHAR_LIMIT: usize = 1800;
const DIFF_LINE_LIMIT: usize = 16;

pub(crate) fn render_agent_message_text(text: &str) -> String {
    format!("{}\n\n{}", type_header("agentMessage"), text.trim())
}

pub(crate) fn render_item_text(item: &Value) -> Option<String> {
    let item_type = item.get("type").and_then(|v| v.as_str())?;
    match item_type {
        "agentMessage" => item
            .get("text")
            .and_then(|v| v.as_str())
            .map(render_agent_message_text),
        "userMessage" => render_user_message(item),
        "todoList" => render_todo_list(item),
        "imageGeneration" => render_image_generation(item),
        "imageView" => render_image_view(item),
        "reasoning" => render_reasoning(item),
        "plan" => render_plain_text_item(item, "plan"),
        "commandExecution" => render_command_execution(item),
        "fileChange" => render_file_change(item),
        "mcpToolCall" => render_mcp_tool_call(item),
        "dynamicToolCall" => render_dynamic_tool_call(item),
        "functionToolCall" => render_function_tool_call(item),
        "collabAgentToolCall" => render_collab_agent_tool_call(item),
        "webSearch" => render_web_search(item),
        _ => render_unknown_item(item, item_type),
    }
}

pub(crate) fn image_item_path(item: &Value) -> Option<PathBuf> {
    item.get("savedPath")
        .and_then(|v| v.as_str())
        .or_else(|| item.get("saved_path").and_then(|v| v.as_str()))
        .or_else(|| item.get("path").and_then(|v| v.as_str()))
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

pub(crate) fn image_item_caption(item: &Value) -> String {
    let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("image");
    let mut lines = vec![type_header(item_type)];
    if let Some(status) = string_field(item, "status") {
        lines.push(format!("status: `{status}`"));
    }
    if let Some(path) = image_item_path(item).and_then(|path| path.to_str().map(str::to_string)) {
        lines.push(format!("path: `{}`", truncate_middle(&path, 220)));
    }
    lines.join("\n")
}

fn render_todo_list(item: &Value) -> Option<String> {
    let entries = item.get("items").and_then(|v| v.as_array())?;
    let lines = entries
        .iter()
        .filter_map(|entry| {
            let text = entry.get("text").and_then(|v| v.as_str())?.trim();
            if text.is_empty() {
                return None;
            }
            let status = entry
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("pending");
            let marker = match status {
                "completed" | "done" => "x",
                "in_progress" | "running" => ">",
                _ => " ",
            };
            Some(format!("- [{marker}] {text}"))
        })
        .collect::<Vec<_>>();
    (!lines.is_empty()).then(|| format!("{}\n\n{}", type_header("todoList"), lines.join("\n")))
}

fn render_user_message(item: &Value) -> Option<String> {
    let content = item.get("content").and_then(|v| v.as_array())?;
    let parts = content
        .iter()
        .filter_map(|entry| match entry.get("type").and_then(|v| v.as_str()) {
            Some("text") => entry
                .get("text")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_string),
            Some("image") => entry
                .get("url")
                .and_then(|v| v.as_str())
                .map(|url| format!("image: `{}`", truncate_middle(url, 240))),
            Some("localImage") => entry
                .get("path")
                .and_then(|v| v.as_str())
                .map(|path| format!("localImage: `{}`", truncate_middle(path, 240))),
            Some("mention") => entry
                .get("name")
                .and_then(|v| v.as_str())
                .map(|name| format!("mention: @{name}")),
            Some("skill") => entry
                .get("name")
                .and_then(|v| v.as_str())
                .map(|name| format!("skill: `{name}`")),
            _ => None,
        })
        .collect::<Vec<_>>();
    (!parts.is_empty()).then(|| format!("{}\n\n{}", type_header("userMessage"), parts.join("\n\n")))
}

fn render_image_generation(item: &Value) -> Option<String> {
    let mut sections = Vec::new();
    if let Some(status) = string_field(item, "status") {
        sections.push(format!("status: `{status}`"));
    }
    if let Some(path) = image_item_path(item).and_then(|path| path.to_str().map(str::to_string)) {
        sections.push(format!("path: `{}`", truncate_middle(&path, 260)));
    }
    if let Some(prompt) =
        string_field(item, "revisedPrompt").or_else(|| string_field(item, "revised_prompt"))
    {
        sections.push(format!(
            "revisedPrompt:\n```text\n{}\n```",
            truncate_summary(&prompt)
        ));
    }
    if let Some(result) =
        string_field(item, "result").filter(|value| !looks_like_image_payload(value))
    {
        sections.push(format!(
            "result:\n```text\n{}\n```",
            truncate_summary(&result)
        ));
    }
    if sections.is_empty() {
        None
    } else {
        Some(format!(
            "{}\n\n{}",
            type_header("imageGeneration"),
            sections.join("\n\n")
        ))
    }
}

fn render_image_view(item: &Value) -> Option<String> {
    let path = item
        .get("path")
        .and_then(|v| v.as_str())
        .or_else(|| item.get("savedPath").and_then(|v| v.as_str()))
        .map(str::trim)
        .filter(|v| !v.is_empty())?;
    Some(format!(
        "{}\n\npath: `{}`",
        type_header("imageView"),
        truncate_middle(path, 260)
    ))
}

fn render_reasoning(item: &Value) -> Option<String> {
    let text = collect_reasoning_text(item)?;
    Some(format!(
        "{}\n\n{}",
        type_header("reasoning"),
        truncate_summary(&text)
    ))
}

fn render_plain_text_item(item: &Value, tag: &str) -> Option<String> {
    let text = item
        .get("text")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|v| !v.is_empty())?;
    Some(format!("{}\n\n{text}", type_header(tag)))
}

fn render_command_execution(item: &Value) -> Option<String> {
    let command = command_text(item);
    let mut lines = Vec::new();
    let mut meta = Vec::new();
    if let Some(status) = string_field(item, "status") {
        meta.push(format!("status `{status}`"));
    }
    if let Some(exit_code) = item.get("exitCode").and_then(|v| v.as_i64()) {
        meta.push(format!("exit `{exit_code}`"));
    }
    if let Some(duration_ms) = item.get("durationMs").and_then(|v| v.as_u64()) {
        meta.push(format!("{duration_ms}ms"));
    }
    if !meta.is_empty() {
        lines.push(meta.join(" · "));
    }
    if let Some(command) = command {
        lines.push(format!("```bash\n{}\n```", truncate_summary(&command)));
    }
    (!lines.is_empty()).then(|| {
        format!(
            "{}\n\n{}",
            type_header("commandExecution"),
            lines.join("\n\n")
        )
    })
}

fn render_file_change(item: &Value) -> Option<String> {
    let changes = item.get("changes").and_then(|v| v.as_array())?;
    if changes.is_empty() {
        return Some(format!("{}\n\nchanges: []", type_header("fileChange")));
    }
    let sections = changes
        .iter()
        .take(8)
        .filter_map(|change| {
            let path = change.get("path").and_then(|v| v.as_str())?.trim();
            if path.is_empty() {
                return None;
            }
            let kind = change
                .get("kind")
                .or_else(|| change.get("type"))
                .and_then(|v| v.as_str())
                .unwrap_or("change");
            let additions = change
                .get("additions")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let deletions = change
                .get("deletions")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let mut section = format!(
                "- kind: `{kind}`\n  path: `{}`\n  stats: `+{} -{}`",
                truncate_middle(path, 240),
                additions,
                deletions
            );
            if let Some(diff) = change.get("diff").and_then(|v| v.as_str()) {
                let preview = summarize_lines(diff, DIFF_LINE_LIMIT, 0);
                if !preview.trim().is_empty() {
                    section.push_str(&format!("\n  diff:\n```diff\n{preview}\n```"));
                }
            }
            Some(section)
        })
        .collect::<Vec<_>>();
    (!sections.is_empty())
        .then(|| format!("{}\n\n{}", type_header("fileChange"), sections.join("\n\n")))
}

fn render_mcp_tool_call(item: &Value) -> Option<String> {
    let mut lines = Vec::new();
    if let Some(server) = string_field(item, "server") {
        lines.push(format!("server: `{server}`"));
    }
    if let Some(tool) = string_field(item, "tool") {
        lines.push(format!("tool: `{tool}`"));
    }
    if let Some(status) = string_field(item, "status") {
        lines.push(format!("status: `{status}`"));
    }
    push_json_section(
        &mut lines,
        "arguments",
        item.get("arguments"),
        JSON_CHAR_LIMIT,
    );
    push_json_section(&mut lines, "result", item.get("result"), JSON_CHAR_LIMIT);
    push_json_section(&mut lines, "error", item.get("error"), JSON_CHAR_LIMIT);
    (!lines.is_empty()).then(|| format!("{}\n\n{}", type_header("mcpToolCall"), lines.join("\n\n")))
}

fn render_dynamic_tool_call(item: &Value) -> Option<String> {
    let mut lines = Vec::new();
    if let Some(tool) = string_field(item, "tool") {
        lines.push(format!("tool: `{tool}`"));
    }
    if let Some(status) = string_field(item, "status") {
        lines.push(format!("status: `{status}`"));
    }
    if let Some(success) = item.get("success").and_then(|v| v.as_bool()) {
        lines.push(format!("success: `{success}`"));
    }
    push_json_section(
        &mut lines,
        "arguments",
        item.get("arguments"),
        JSON_CHAR_LIMIT,
    );
    push_json_section(
        &mut lines,
        "contentItems",
        item.get("contentItems"),
        JSON_CHAR_LIMIT,
    );
    (!lines.is_empty()).then(|| {
        format!(
            "{}\n\n{}",
            type_header("dynamicToolCall"),
            lines.join("\n\n")
        )
    })
}

fn render_function_tool_call(item: &Value) -> Option<String> {
    let mut lines = Vec::new();
    if let Some(tool) = string_field(item, "toolName").or_else(|| string_field(item, "tool")) {
        lines.push(format!("tool: `{tool}`"));
    }
    if let Some(args) = string_field(item, "arguments") {
        lines.push(format!(
            "arguments:\n```json\n{}\n```",
            truncate_text(&args, JSON_CHAR_LIMIT)
        ));
    }
    if let Some(output) = string_field(item, "output") {
        lines.push(format!(
            "output:\n```text\n{}\n```",
            truncate_summary(&output)
        ));
    }
    (!lines.is_empty()).then(|| {
        format!(
            "{}\n\n{}",
            type_header("functionToolCall"),
            lines.join("\n\n")
        )
    })
}

fn render_collab_agent_tool_call(item: &Value) -> Option<String> {
    let mut lines = Vec::new();
    if let Some(tool) = string_field(item, "tool") {
        lines.push(format!("tool: `{tool}`"));
    }
    if let Some(sender) = string_field(item, "senderThreadId") {
        lines.push(format!(
            "senderThreadId: `{}`",
            truncate_middle(&sender, 120)
        ));
    }
    push_json_section(
        &mut lines,
        "receiverThreadIds",
        item.get("receiverThreadIds"),
        JSON_CHAR_LIMIT,
    );
    if let Some(prompt) = string_field(item, "prompt") {
        lines.push(format!(
            "prompt:\n```text\n{}\n```",
            truncate_summary(&prompt)
        ));
    }
    (!lines.is_empty()).then(|| {
        format!(
            "{}\n\n{}",
            type_header("collabAgentToolCall"),
            lines.join("\n\n")
        )
    })
}

fn render_web_search(item: &Value) -> Option<String> {
    let query = string_field(item, "query")?;
    let mut lines = vec![format!("query: {}", truncate_text(&query, 500))];
    push_json_section(&mut lines, "action", item.get("action"), JSON_CHAR_LIMIT);
    push_json_section(&mut lines, "results", item.get("results"), JSON_CHAR_LIMIT);
    Some(format!(
        "{}\n\n{}",
        type_header("webSearch"),
        lines.join("\n\n")
    ))
}

fn render_unknown_item(item: &Value, item_type: &str) -> Option<String> {
    let summary = serde_json::to_string_pretty(item).ok()?;
    Some(format!(
        "{}\n\n```json\n{}\n```",
        type_header(item_type),
        truncate_text(&summary, SUMMARY_CHAR_LIMIT)
    ))
}

fn type_header(item_type: &str) -> String {
    let icon = match item_type {
        "agentMessage" => "💬",
        "userMessage" => "👤",
        "todoList" => "☑",
        "imageGeneration" | "imageView" => "🖼",
        "reasoning" => "🧠",
        "plan" => "📋",
        "commandExecution" => "💻",
        "fileChange" => "📝",
        "mcpToolCall" | "dynamicToolCall" | "functionToolCall" | "collabAgentToolCall" => "🛠",
        "webSearch" => "🔎",
        _ => "•",
    };
    format!("{icon} {item_type}")
}

fn command_text(item: &Value) -> Option<String> {
    item.get("commandActions")
        .and_then(|v| v.as_array())
        .and_then(|actions| actions.first())
        .and_then(|action| action.get("command"))
        .and_then(|v| v.as_str())
        .or_else(|| item.get("command").and_then(|v| v.as_str()))
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

fn collect_reasoning_text(item: &Value) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(summary) = item.get("summary").and_then(|v| v.as_array()) {
        for entry in summary {
            if let Some(text) = entry.get("text").and_then(|v| v.as_str()) {
                push_nonempty(&mut parts, text);
            }
        }
    }
    if let Some(content) = item.get("content").and_then(|v| v.as_array()) {
        for entry in content {
            if let Some(text) = entry
                .get("text")
                .and_then(|v| v.as_str())
                .or_else(|| entry.as_str())
            {
                push_nonempty(&mut parts, text);
            }
        }
    }
    parts.dedup();
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

fn push_nonempty(parts: &mut Vec<String>, text: &str) {
    let text = text.trim();
    if !text.is_empty() {
        parts.push(text.to_string());
    }
}

fn push_json_section(lines: &mut Vec<String>, label: &str, value: Option<&Value>, limit: usize) {
    let Some(value) = value.filter(|v| !v.is_null()) else {
        return;
    };
    let text = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
    if text.trim().is_empty() || text == "null" {
        return;
    }
    lines.push(format!(
        "{label}:\n```json\n{}\n```",
        truncate_text(&text, limit)
    ));
}

fn string_field(item: &Value, key: &str) -> Option<String> {
    item.get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

fn truncate_summary(text: &str) -> String {
    truncate_text(text, SUMMARY_CHAR_LIMIT)
}

fn truncate_text(text: &str, max_chars: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let keep = max_chars.saturating_sub(32);
    let head_len = keep / 2;
    let tail_len = keep.saturating_sub(head_len);
    let head = text.chars().take(head_len).collect::<String>();
    let tail = text
        .chars()
        .rev()
        .take(tail_len)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("{head}\n...[truncated]...\n{tail}")
}

fn truncate_middle(text: &str, max_chars: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let keep = max_chars.saturating_sub(16);
    let head_len = keep / 2;
    let tail_len = keep.saturating_sub(head_len);
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

fn summarize_lines(text: &str, head_lines: usize, tail_lines: usize) -> String {
    let lines = text.lines().collect::<Vec<_>>();
    let limit = head_lines + tail_lines;
    if lines.len() <= limit || tail_lines == 0 && lines.len() <= head_lines {
        return truncate_text(text, SUMMARY_CHAR_LIMIT);
    }
    let mut output = Vec::new();
    output.extend(lines.iter().take(head_lines).copied());
    output.push("...[truncated]...");
    if tail_lines > 0 {
        output.extend(
            lines
                .iter()
                .rev()
                .take(tail_lines)
                .copied()
                .collect::<Vec<_>>()
                .into_iter()
                .rev(),
        );
    }
    truncate_text(&output.join("\n"), SUMMARY_CHAR_LIMIT)
}

fn looks_like_image_payload(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.starts_with("data:image/")
        || (trimmed.len() > 256
            && trimmed.chars().all(|ch| {
                ch.is_ascii_alphanumeric() || matches!(ch, '+' | '/' | '=' | '\r' | '\n')
            }))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{render_agent_message_text, render_item_text};

    #[test]
    fn agent_message_is_tagged_and_not_truncated() {
        let text = "hello\n".repeat(3000);
        let rendered = render_agent_message_text(&text);

        assert!(rendered.starts_with("💬 agentMessage"));
        assert!(rendered.contains(&"hello\n".repeat(100)));
        assert!(!rendered.contains("[truncated]"));
    }

    #[test]
    fn todo_list_is_rendered_fully() {
        let item = json!({
            "type": "todoList",
            "items": [
                {"text": "one", "status": "completed"},
                {"text": "two", "status": "pending"}
            ]
        });
        let rendered = render_item_text(&item).expect("todo list");

        assert!(rendered.starts_with("☑ todoList"));
        assert!(rendered.contains("- [x] one"));
        assert!(rendered.contains("- [ ] two"));
    }

    #[test]
    fn command_execution_omits_output() {
        let output = (0..80)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let item = json!({
            "type": "commandExecution",
            "status": "completed",
            "command": "cargo test",
            "aggregatedOutput": output
        });
        let rendered = render_item_text(&item).expect("command item");

        assert!(rendered.starts_with("💻 commandExecution"));
        assert!(rendered.contains("```bash\ncargo test\n```"));
        assert!(rendered.contains("status `completed`"));
        assert!(!rendered.contains("line 0"));
        assert!(!rendered.contains("output:"));
    }
}
