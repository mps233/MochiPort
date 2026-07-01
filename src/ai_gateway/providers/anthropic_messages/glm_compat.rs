const GLM_WEB_SEARCH_TOOL_MARKER: &str = "Z.ai Built-in Tool: web_search_prime";
const GLM_WEB_SEARCH_TOOL_END: &str = "*Executing on server...*";
const GLM_WEB_SEARCH_RESULT_MARKER: &str = "**web_search_prime_result_summary:**";
const GLM_OUTPUT_MARKER: &str = "**Output:**";

pub(super) fn clean_private_web_search_text(text: &str) -> Option<String> {
    let cleaned = remove_private_tool_blocks(&remove_private_result_summaries(text));
    let cleaned = cleaned
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n");
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned.to_string())
    }
}

/// 把 GLM 文本缓冲切成「可以立即下发的前缀」和「必须继续保留的尾部」。
///
/// 目标是让普通回答逐 token 流式输出，只有真的出现私有网搜标记
/// (`web_search_prime` 工具块 / 结果摘要) 时，才把那一段保留下来等清洗。
/// 返回 `(emit, keep)`：`emit` 保证不含任何私有片段，可以直接作为 delta 下发；
/// `keep` 需要写回缓冲，等待后续 token 或最终 flush 时用
/// [`clean_private_web_search_text`] 收尾。
pub(super) fn split_streamable(buf: &str) -> (String, String) {
    let tool_pos = buf.find(GLM_WEB_SEARCH_TOOL_MARKER);
    let res_pos = buf.find(GLM_WEB_SEARCH_RESULT_MARKER);
    let use_tool = match (tool_pos, res_pos) {
        (Some(t), Some(r)) => t <= r,
        (Some(_), None) => true,
        (None, Some(_)) => false,
        (None, None) => {
            let hold = holdback_len(buf);
            let split = buf.len() - hold;
            return (buf[..split].to_string(), buf[split..].to_string());
        }
    };

    if use_tool {
        let m = tool_pos.expect("tool marker present");
        let start = tool_block_start(buf, m);
        match buf[m..].find(GLM_WEB_SEARCH_TOOL_END) {
            Some(end_rel) => {
                let end = m + end_rel + GLM_WEB_SEARCH_TOOL_END.len();
                let end = consume_trailing_blank_lines(buf, end);
                let (emit_rest, keep) = split_streamable(&buf[end..]);
                (format!("{}{}", &buf[..start], emit_rest), keep)
            }
            None => (buf[..start].to_string(), buf[start..].to_string()),
        }
    } else {
        let m = res_pos.expect("result marker present");
        let start = output_marker_start(buf, m).unwrap_or(m);
        let after_marker = m + GLM_WEB_SEARCH_RESULT_MARKER.len();
        match result_summary_end(buf, after_marker) {
            Some(end) => {
                let (emit_rest, keep) = split_streamable(&buf[end..]);
                (format!("{}{}", &buf[..start], emit_rest), keep)
            }
            None => (buf[..start].to_string(), buf[start..].to_string()),
        }
    }
}

/// 尾部需要保留的字节数：可能是某个标记的前缀，或是标记的引导片段
/// (加粗 `**`、内联空白、`🌐` 图标)，避免把引导片段先吐出去导致漏网。
fn holdback_len(buf: &str) -> usize {
    marker_prefix_suffix(buf).max(trailing_leadin_len(buf))
}

fn marker_prefix_suffix(buf: &str) -> usize {
    let mut best = 0;
    for trigger in [GLM_WEB_SEARCH_TOOL_MARKER, GLM_WEB_SEARCH_RESULT_MARKER] {
        let max_len = trigger.len().saturating_sub(1).min(buf.len());
        for len in (1..=max_len).rev() {
            let idx = buf.len() - len;
            if !buf.is_char_boundary(idx) {
                continue;
            }
            let suffix = &buf[idx..];
            if trigger.starts_with(suffix) {
                best = best.max(len);
                break;
            }
        }
    }
    best
}

fn trailing_leadin_len(buf: &str) -> usize {
    let mut kept = 0;
    for (i, ch) in buf.char_indices().rev() {
        let is_leadin =
            ch == '*' || ch == '\u{1f310}' || (ch.is_whitespace() && ch != '\n' && ch != '\r');
        if is_leadin {
            kept = buf.len() - i;
        } else {
            break;
        }
    }
    kept
}

fn remove_private_tool_blocks(input: &str) -> String {
    let mut output = input.to_string();
    while let Some(marker_start) = output.find(GLM_WEB_SEARCH_TOOL_MARKER) {
        let start = tool_block_start(&output, marker_start);
        let end = output[start..]
            .find(GLM_WEB_SEARCH_TOOL_END)
            .map(|offset| start + offset + GLM_WEB_SEARCH_TOOL_END.len())
            .unwrap_or(output.len());
        output.replace_range(start..consume_trailing_blank_lines(&output, end), "");
    }
    output
}

fn tool_block_start(text: &str, marker_start: usize) -> usize {
    let line_start = text[..marker_start]
        .rfind('\n')
        .map(|pos| pos + 1)
        .unwrap_or(0);
    let mut start = marker_start;

    start = consume_previous_inline_whitespace(text, start, line_start);
    if let Some((prev_start, prev)) = previous_char(text, start) {
        if !prev.is_ascii() && !prev.is_alphanumeric() {
            start = prev_start;
            start = consume_previous_inline_whitespace(text, start, line_start);
        }
    }
    if text[line_start..start].ends_with("**") {
        start -= 2;
    }

    if text[line_start..start].trim().is_empty() {
        line_start
    } else {
        start
    }
}

fn consume_previous_inline_whitespace(text: &str, mut index: usize, line_start: usize) -> usize {
    while index > line_start {
        let Some((prev_start, prev)) = previous_char(text, index) else {
            break;
        };
        if prev == '\n' || prev == '\r' || !prev.is_whitespace() {
            break;
        }
        index = prev_start;
    }
    index
}

fn previous_char(text: &str, index: usize) -> Option<(usize, char)> {
    text[..index].char_indices().next_back()
}

fn remove_private_result_summaries(input: &str) -> String {
    let mut output = input.to_string();
    while let Some(marker_start) = output.find(GLM_WEB_SEARCH_RESULT_MARKER) {
        let start = output_marker_start(&output, marker_start).unwrap_or(marker_start);
        let after_marker = marker_start + GLM_WEB_SEARCH_RESULT_MARKER.len();
        let end = result_summary_end(&output, after_marker).unwrap_or(output.len());
        output.replace_range(start..end, "");
    }
    output
}

fn output_marker_start(text: &str, marker_start: usize) -> Option<usize> {
    let prefix = &text[..marker_start];
    let output_start = prefix.rfind(GLM_OUTPUT_MARKER)?;
    prefix[output_start + GLM_OUTPUT_MARKER.len()..]
        .trim()
        .is_empty()
        .then_some(output_start)
}

fn result_summary_end(text: &str, from: usize) -> Option<usize> {
    let mut search_from = from;
    while let Some(offset) = text[search_from..].find('\n') {
        let newline = search_from + offset;
        let after_indent = consume_indented_line_prefix(text, newline + 1);
        if after_indent > newline + 1 {
            return Some(after_indent);
        }
        search_from = newline + 1;
    }
    None
}

fn consume_indented_line_prefix(text: &str, mut index: usize) -> usize {
    let start = index;
    while index < text.len() {
        let Some(ch) = text[index..].chars().next() else {
            break;
        };
        if ch != ' ' && ch != '\t' {
            break;
        }
        index += ch.len_utf8();
    }
    if index - start >= 8 { index } else { start }
}

fn consume_trailing_blank_lines(text: &str, mut index: usize) -> usize {
    while index < text.len() {
        let Some(ch) = text[index..].chars().next() else {
            break;
        };
        if ch != '\n' && ch != '\r' && ch != ' ' && ch != '\t' {
            break;
        }
        index += ch.len_utf8();
    }
    index
}

#[cfg(test)]
mod tests {
    use super::{clean_private_web_search_text, split_streamable};

    // 模拟逐 token 增量喂入：把安全前缀 emit，尾部 keep 回缓冲，
    // 最后一次性 flush 时用 clean 收尾。返回 (streamed_prefix, final_flush)。
    fn drive_stream(tokens: &[&str]) -> (String, Option<String>) {
        let mut buf = String::new();
        let mut emitted = String::new();
        for tok in tokens {
            buf.push_str(tok);
            let (emit, keep) = split_streamable(&buf);
            emitted.push_str(&emit);
            buf = keep;
        }
        let flushed = clean_private_web_search_text(&buf);
        (emitted, flushed)
    }

    #[test]
    fn split_streams_plain_text_immediately() {
        let (emitted, flushed) = drive_stream(&["Hello", " world", "!"]);
        assert_eq!(emitted, "Hello world!");
        assert_eq!(flushed, None);
    }

    #[test]
    fn split_holds_back_partial_marker_lead_in() {
        // 结尾是加粗+图标引导，可能是私有工具块的开头，必须保留。
        let (emit, keep) = split_streamable("answer **\u{1f310}");
        assert_eq!(emit, "answer");
        assert_eq!(keep, " **\u{1f310}");
    }

    #[test]
    fn split_filters_private_tool_block_but_streams_rest() {
        let tokens = [
            "让我查一下。",
            "**\u{1f310} Z.ai Built-in Tool: web_search_prime**",
            "\n\n**Input:**\n```json\n{}\n```\n",
            "*Executing on server...*",
            "\n最终答案",
        ];
        let (emitted, flushed) = drive_stream(&tokens);
        assert!(!emitted.contains("web_search_prime"));
        assert!(emitted.starts_with("让我查一下。"));
        assert!(emitted.contains("最终答案") || flushed.as_deref() == Some("最终答案"));
        assert!(!emitted.contains("Executing on server"));
    }

    #[test]
    fn removes_glm_private_tool_block() {
        let text = "**\u{1f310} Z.ai Built-in Tool: web_search_prime**\n\n**Input:**\n```json\n{}\n```\n*Executing on server...*\n";
        assert_eq!(clean_private_web_search_text(text), None);
    }

    #[test]
    fn removes_glm_private_result_summary_and_keeps_answer() {
        let text = "**Output:**\n**web_search_prime_result_summary:** [{\"text\":[{\"title\":\"Result\"}]}]\n                                                最终答案";
        assert_eq!(
            clean_private_web_search_text(text).as_deref(),
            Some("最终答案")
        );
    }

    #[test]
    fn removes_multiple_private_sections() {
        let text = "**Output:**\n**web_search_prime_result_summary:** [{\"text\":[{\"title\":\"A\"}]}]\n                                                继续。**\u{1f310} Z.ai Built-in Tool: web_search_prime**\n\n**Input:**\n```json\n{}\n```\n*Executing on server...*\n";
        assert_eq!(
            clean_private_web_search_text(text).as_deref(),
            Some("继续。")
        );
    }

    #[test]
    fn keeps_prefix_before_inline_private_tool_block() {
        let text = "让我再查一下。**\u{1f310} Z.ai Built-in Tool: web_search_prime**\n\n**Input:**\n```json\n{}\n```\n*Executing on server...*\n";
        assert_eq!(
            clean_private_web_search_text(text).as_deref(),
            Some("让我再查一下。")
        );
    }
}
