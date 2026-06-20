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
    use super::clean_private_web_search_text;

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
