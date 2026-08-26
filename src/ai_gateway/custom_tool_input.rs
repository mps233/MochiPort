use serde_json::Value;

/// Extract the free-form input from a completed custom-tool argument object.
///
/// Providers sometimes send the input directly instead of wrapping it in an
/// object. Keep that raw value as the fallback so callers preserve their
/// existing wire-level behavior.
pub(crate) fn completed_custom_tool_input(arguments: &str) -> String {
    serde_json::from_str::<Value>(arguments)
        .ok()
        .and_then(|value| {
            value
                .get("input")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| arguments.to_string())
}

/// Decode the input available in a complete or still-growing custom-tool
/// argument payload. A growing wrapper is accepted only when its first key is
/// `input`; callers can then wait for more data when this returns `None`.
pub(crate) fn partial_custom_tool_input(arguments: &str) -> Option<String> {
    parse_custom_tool_input(arguments).or_else(|| partial_wrapped_input_prefix(arguments))
}

fn parse_custom_tool_input(arguments: &str) -> Option<String> {
    serde_json::from_str::<Value>(arguments)
        .ok()?
        .get("input")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn partial_wrapped_input_prefix(arguments: &str) -> Option<String> {
    let mut rest = arguments.trim_start();
    rest = rest.strip_prefix('{')?.trim_start();

    let (key, after_key) = parse_json_string_prefix(rest)?;
    if key != "input" {
        return None;
    }

    rest = after_key.trim_start();
    rest = rest.strip_prefix(':')?.trim_start();
    parse_json_string_prefix(rest).map(|(value, _)| value)
}

fn parse_json_string_prefix(input: &str) -> Option<(String, &str)> {
    if !input.starts_with('"') {
        return None;
    }

    let mut output = String::new();
    let mut pos = 1;
    while pos < input.len() {
        let ch = input[pos..].chars().next()?;
        match ch {
            '"' => {
                let next = pos + ch.len_utf8();
                return Some((output, &input[next..]));
            }
            '\\' => {
                pos += ch.len_utf8();
                let escaped = input[pos..].chars().next()?;
                match escaped {
                    '"' => output.push('"'),
                    '\\' => output.push('\\'),
                    '/' => output.push('/'),
                    'b' => output.push('\u{0008}'),
                    'f' => output.push('\u{000c}'),
                    'n' => output.push('\n'),
                    'r' => output.push('\r'),
                    't' => output.push('\t'),
                    'u' => {
                        let after_u = pos + escaped.len_utf8();
                        let unicode = decode_json_unicode_escape(input, after_u)?;
                        output.push(unicode.0);
                        pos = unicode.1;
                        continue;
                    }
                    _ => output.push(escaped),
                }
                pos += escaped.len_utf8();
            }
            _ => {
                output.push(ch);
                pos += ch.len_utf8();
            }
        }
    }

    Some((output, ""))
}

fn decode_json_unicode_escape(input: &str, offset: usize) -> Option<(char, usize)> {
    let first = read_hex_u16(input, offset)?;
    let first_end = offset + 4;
    if (0xD800..=0xDBFF).contains(&first) {
        let low_offset = first_end + 2;
        if input.get(first_end..low_offset) != Some("\\u") {
            return None;
        }
        let second = read_hex_u16(input, low_offset)?;
        if !(0xDC00..=0xDFFF).contains(&second) {
            return None;
        }
        let codepoint = 0x10000 + (((first as u32 - 0xD800) << 10) | (second as u32 - 0xDC00));
        char::from_u32(codepoint).map(|ch| (ch, low_offset + 4))
    } else {
        char::from_u32(first as u32).map(|ch| (ch, first_end))
    }
}

fn read_hex_u16(input: &str, offset: usize) -> Option<u16> {
    let hex = input.get(offset..offset + 4)?;
    u16::from_str_radix(hex, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::{completed_custom_tool_input, partial_custom_tool_input};

    #[test]
    fn completed_input_uses_wrapper_or_raw_fallback() {
        assert_eq!(completed_custom_tool_input(r#"{"input":"hello"}"#), "hello");
        assert_eq!(completed_custom_tool_input("raw patch"), "raw patch");
    }

    #[test]
    fn partial_input_decodes_escaped_prefixes() {
        assert_eq!(
            partial_custom_tool_input(r#"{"input":"line 1\nline 2"#),
            Some("line 1\nline 2".to_string())
        );
        assert_eq!(
            partial_custom_tool_input(r#"{"input":"snowman: \u2603"#),
            Some("snowman: ☃".to_string())
        );
        assert_eq!(
            partial_custom_tool_input(r#"{"input":"emoji: \ud83d\ude03"#),
            Some("emoji: 😃".to_string())
        );
        assert_eq!(partial_custom_tool_input(r#"{"input":"bad \"#), None);
        assert_eq!(partial_custom_tool_input(r#"{"other":"value"#), None);
    }
}
