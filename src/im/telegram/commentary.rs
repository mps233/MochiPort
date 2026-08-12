use crate::im::core::i18n::ImText;

pub(crate) const TELEGRAM_COMMENTARY_MAX_CHARS: usize = 3_600;
const VISIBLE_COMMENTARY_COUNT: usize = 2;
const RICH_SECTION_SEPARATOR: &str = "\n\n---\n\n";
const FALLBACK_SECTION_SEPARATOR: &str = "\n\n";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TelegramCommentaryRender {
    /// Rich Markdown accepted by Telegram's `sendRichMessage` and
    /// `editMessageText` endpoints.
    pub rich_markdown: String,
    /// Markdown fallback for Bot API servers without rich-message support.
    pub fallback_markdown: String,
    /// Entries discarded from the head, including entries discarded earlier.
    pub dropped: usize,
}

/// Render commentary entries into one message while keeping the latest updates
/// immediately visible. Rich Markdown is used instead of hand-built blocks so
/// formatting inside each entry remains intact inside `<details>`.
pub(crate) fn render_commentary(
    entries: &[String],
    dropped: usize,
    text: ImText,
) -> TelegramCommentaryRender {
    let mut entries = entries
        .iter()
        .map(|entry| entry.trim())
        .filter(|entry| !entry.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    let mut dropped = dropped;

    loop {
        let rendered = render_with_entries(&entries, dropped, text);
        if render_char_count(&rendered) <= TELEGRAM_COMMENTARY_MAX_CHARS {
            return rendered;
        }

        if entries.len() > 1 {
            entries.remove(0);
            dropped = dropped.saturating_add(1);
            continue;
        }

        if let Some(entry) = entries.first_mut() {
            let entry_budget = largest_entry_budget(entry, dropped, text);
            *entry = truncate_middle(entry, entry_budget);
        }
        return render_with_entries(&entries, dropped, text);
    }
}

fn largest_entry_budget(entry: &str, dropped: usize, text: ImText) -> usize {
    let mut lower = 0;
    let mut upper = entry.chars().count();
    while lower < upper {
        let candidate = lower + (upper - lower).div_ceil(2);
        let rendered = render_with_entries(&[truncate_middle(entry, candidate)], dropped, text);
        if render_char_count(&rendered) <= TELEGRAM_COMMENTARY_MAX_CHARS {
            lower = candidate;
        } else {
            upper = candidate - 1;
        }
    }
    lower
}

fn render_with_entries(
    entries: &[String],
    dropped: usize,
    text: ImText,
) -> TelegramCommentaryRender {
    render_with_labels(
        entries,
        dropped,
        |count| text.telegram_commentary_earlier(count),
        |count| text.telegram_commentary_omitted(count),
    )
}

fn render_with_labels(
    entries: &[String],
    dropped: usize,
    earlier_label: impl Fn(usize) -> String,
    omitted_label: impl Fn(usize) -> String,
) -> TelegramCommentaryRender {
    let visible_start = entries.len().saturating_sub(VISIBLE_COMMENTARY_COUNT);
    let hidden = &entries[..visible_start];
    let visible = &entries[visible_start..];
    let earlier_count = dropped.saturating_add(hidden.len());

    let mut earlier_rich = None;
    if earlier_count > 0 {
        let mut details_body = Vec::new();
        if dropped > 0 {
            details_body.push(omitted_label(dropped));
        }
        details_body.extend(hidden.iter().cloned());
        earlier_rich = Some(format!(
            "<details><summary>{}</summary>\n\n{}\n\n</details>",
            escape_summary(&earlier_label(earlier_count)),
            details_body.join(RICH_SECTION_SEPARATOR)
        ));
    }
    let visible_rich = visible.join(RICH_SECTION_SEPARATOR);
    let rich_markdown = match (earlier_rich, visible_rich.is_empty()) {
        (Some(earlier), false) => format!("{earlier}\n\n{visible_rich}"),
        (Some(earlier), true) => earlier,
        (None, _) => visible_rich,
    };

    let mut fallback_sections = Vec::new();
    if earlier_count > 0 {
        fallback_sections.push(earlier_label(earlier_count));
    }
    fallback_sections.extend(visible.iter().cloned());

    TelegramCommentaryRender {
        rich_markdown,
        fallback_markdown: fallback_sections.join(FALLBACK_SECTION_SEPARATOR),
        dropped,
    }
}

fn escape_summary(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn render_char_count(rendered: &TelegramCommentaryRender) -> usize {
    rendered
        .rich_markdown
        .chars()
        .count()
        .max(rendered.fallback_markdown.chars().count())
}

fn truncate_middle(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    if max_chars == 0 {
        return String::new();
    }
    if max_chars == 1 {
        return "…".to_string();
    }
    let head_len = (max_chars - 1).div_ceil(2);
    let tail_len = max_chars - 1 - head_len;
    let head = value.chars().take(head_len).collect::<String>();
    let tail = value
        .chars()
        .rev()
        .take(tail_len)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("{head}…{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(count: usize) -> Vec<String> {
        (1..=count).map(|index| format!("update {index}")).collect()
    }

    #[test]
    fn one_entry_is_visible_without_details() {
        let rendered = render_commentary(&entries(1), 0, ImText::zh_cn());

        assert_eq!(rendered.rich_markdown, "update 1");
        assert_eq!(rendered.fallback_markdown, "update 1");
        assert_eq!(rendered.dropped, 0);
    }

    #[test]
    fn dropped_history_is_summarized_even_when_no_retained_entries_remain() {
        let rendered = render_commentary(&[], 3, ImText::zh_cn());

        assert!(
            rendered
                .rich_markdown
                .starts_with("<details><summary>较早进展 · 3 条</summary>")
        );
        assert!(rendered.rich_markdown.contains("另外 3 条较早进展已省略"));
        assert_eq!(rendered.fallback_markdown, "较早进展 · 3 条");
    }

    #[test]
    fn two_entries_are_both_visible_without_details() {
        let rendered = render_commentary(&entries(2), 0, ImText::zh_cn());

        assert_eq!(rendered.rich_markdown, "update 1\n\n---\n\nupdate 2");
        assert!(!rendered.rich_markdown.contains("<details>"));
        assert_eq!(rendered.fallback_markdown, "update 1\n\nupdate 2");
    }

    #[test]
    fn eight_entries_fold_the_first_six_and_leave_the_latest_two_visible() {
        let rendered = render_commentary(&entries(8), 0, ImText::zh_cn());

        assert!(
            rendered
                .rich_markdown
                .starts_with("<details><summary>较早进展 · 6 条</summary>")
        );
        assert!(rendered.rich_markdown.contains("update 1"));
        assert!(rendered.rich_markdown.contains("update 6"));
        assert!(
            rendered
                .rich_markdown
                .contains("update 1\n\n---\n\nupdate 2")
        );
        assert!(
            rendered
                .rich_markdown
                .ends_with("update 7\n\n---\n\nupdate 8")
        );
        assert!(rendered.rich_markdown.contains("</details>\n\nupdate 7"));
        assert!(
            !rendered
                .rich_markdown
                .contains("</details>\n\n---\n\nupdate 7")
        );
        assert!(!rendered.rich_markdown.contains("<details open>"));
        assert_eq!(
            rendered.fallback_markdown,
            "较早进展 · 6 条\n\nupdate 7\n\nupdate 8"
        );
    }

    #[test]
    fn rich_markdown_preserves_entry_formatting() {
        let entries = vec![
            "**bold** and [link](https://example.com)".to_string(),
            "```rust\nfn main() {}\n```".to_string(),
            "latest `code`".to_string(),
        ];
        let rendered = render_commentary(&entries, 0, ImText::zh_cn());

        for marker in [
            "**bold**",
            "[link](https://example.com)",
            "```rust\nfn main() {}\n```",
            "latest `code`",
        ] {
            assert!(rendered.rich_markdown.contains(marker));
        }
        assert!(!rendered.fallback_markdown.contains("**bold**"));
        assert!(rendered.fallback_markdown.contains("latest `code`"));
    }

    #[test]
    fn strips_blank_entries_without_changing_order() {
        let rendered = render_commentary(
            &[
                "  ".to_string(),
                " first ".to_string(),
                "second".to_string(),
            ],
            0,
            ImText::zh_cn(),
        );

        assert_eq!(rendered.rich_markdown, "first\n\n---\n\nsecond");
        assert_eq!(rendered.fallback_markdown, "first\n\nsecond");
        assert_eq!(rendered.dropped, 0);
    }

    #[test]
    fn discards_oldest_entries_to_fit_the_safe_limit_and_accumulates_dropped() {
        let entries = (1..=8)
            .map(|index| format!("entry-{index}:{}", "x".repeat(900)))
            .collect::<Vec<_>>();
        let rendered = render_commentary(&entries, 3, ImText::zh_cn());

        assert!(rendered.rich_markdown.chars().count() <= TELEGRAM_COMMENTARY_MAX_CHARS);
        assert!(rendered.fallback_markdown.chars().count() <= TELEGRAM_COMMENTARY_MAX_CHARS);
        assert!(rendered.dropped > 3);
        assert!(!rendered.rich_markdown.contains("entry-1:"));
        assert!(rendered.rich_markdown.contains("entry-8:"));
        assert!(
            rendered
                .rich_markdown
                .contains(&text_omitted_zh(rendered.dropped))
        );
    }

    #[test]
    fn truncates_one_oversized_visible_entry_without_breaking_details_markup() {
        let rendered = render_commentary(
            &[format!("latest:{}", "界".repeat(5_000))],
            4,
            ImText::zh_cn(),
        );

        assert!(rendered.rich_markdown.chars().count() <= TELEGRAM_COMMENTARY_MAX_CHARS);
        assert!(rendered.fallback_markdown.chars().count() <= TELEGRAM_COMMENTARY_MAX_CHARS);
        assert!(rendered.rich_markdown.contains("</details>"));
        assert!(rendered.rich_markdown.contains("latest:"));
        assert_eq!(rendered.dropped, 4);
    }

    #[test]
    fn chinese_labels_are_localized() {
        let rendered = render_commentary(&entries(4), 2, ImText::zh_cn());

        assert!(
            rendered
                .rich_markdown
                .contains("<summary>较早进展 · 4 条</summary>")
        );
        assert!(rendered.rich_markdown.contains("… 另外 2 条较早进展已省略"));
        assert_eq!(
            rendered.fallback_markdown,
            "较早进展 · 4 条\n\nupdate 3\n\nupdate 4"
        );
    }

    #[test]
    fn accepts_english_localized_labels() {
        let rendered = render_with_labels(
            &entries(4),
            2,
            |count| format!("Earlier updates · {count}"),
            |count| format!("… {count} earlier updates omitted"),
        );

        assert!(
            rendered
                .rich_markdown
                .contains("<summary>Earlier updates · 4</summary>")
        );
        assert!(
            rendered
                .rich_markdown
                .contains("… 2 earlier updates omitted")
        );
        assert_eq!(
            rendered.fallback_markdown,
            "Earlier updates · 4\n\nupdate 3\n\nupdate 4"
        );
    }

    fn text_omitted_zh(count: usize) -> String {
        format!("… 另外 {count} 条较早进展已省略")
    }
}
