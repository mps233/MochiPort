/// Build a bounded, single-line preview for diagnostic logs.
///
/// Keep this deliberately independent of any platform renderer so all IM
/// adapters use the same newline escaping and character-count semantics.
pub(crate) fn log_text_preview(text: &str, limit: usize) -> String {
    let compact = text.replace("\r\n", "\n").replace('\n', "\\n");
    let mut chars = compact.chars();
    let mut output = chars.by_ref().take(limit).collect::<String>();
    if chars.next().is_some() {
        output.push_str("...");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::log_text_preview;

    #[test]
    fn log_text_preview_escapes_newlines_and_bounds_unicode_chars() {
        assert_eq!(log_text_preview("a\r\nb\n中", 5), r"a\nb\...");
        assert_eq!(log_text_preview("short", 32), "short");
    }
}
