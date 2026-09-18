use crate::{im::core::i18n::ImText, im::runtime::TelegramCommentaryEntry};

/// 过程文案与工具摘要共用同一条聚合气泡，这里只挑选预算内的可见条目。
pub(crate) const TELEGRAM_COMMENTARY_MAX_CHARS: usize = 3_600;
const SECTION_SEPARATOR: &str = "\n\n";

/// 一条可见的过程文案（携带到达序号，供与工具步骤交错排序）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TelegramCommentaryRenderedEntry {
    pub text: String,
    pub sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TelegramCommentaryRender {
    /// 保留下来、需要在气泡里直接显示的条目，按发生顺序排列。
    pub entries: Vec<TelegramCommentaryRenderedEntry>,
    /// 为满足字符预算被丢弃的较早条目总数（含此前已丢弃的条目）。
    pub dropped: usize,
}

/// 过程文案保持可见（不折叠成 `<details>`），超预算时从最早的条目开始丢弃，
/// 只保留一条时改为截断中段，并在气泡顶部标注省略数量。
pub(crate) fn render_commentary(
    entries: &[TelegramCommentaryEntry],
    dropped: usize,
    text: ImText,
) -> TelegramCommentaryRender {
    // 先按到达序号排序，保证「丢弃最早的条目」与真实发生顺序一致。
    let mut ordered = entries.to_vec();
    ordered.sort_by_key(|entry| entry.sequence);
    let mut entries = ordered
        .iter()
        .map(|entry| TelegramCommentaryRenderedEntry {
            text: entry.text.trim().to_string(),
            sequence: entry.sequence,
        })
        .filter(|entry| !entry.text.is_empty())
        .collect::<Vec<_>>();
    let mut dropped = dropped;

    loop {
        let rendered = TelegramCommentaryRender {
            entries: entries.clone(),
            dropped,
        };
        if rendered_char_count(&rendered, text) <= TELEGRAM_COMMENTARY_MAX_CHARS {
            return rendered;
        }

        if entries.len() > 1 {
            entries.remove(0);
            dropped = dropped.saturating_add(1);
            continue;
        }

        if let Some(entry) = entries.first_mut() {
            let entry_budget = largest_entry_budget(&entry.text, dropped, text);
            entry.text = truncate_middle(&entry.text, entry_budget);
        }
        return TelegramCommentaryRender { entries, dropped };
    }
}

/// 可见条目的 Markdown 回退文本：无富消息支持时按同样顺序直出。
pub(crate) fn render_commentary_fallback(
    rendered: &TelegramCommentaryRender,
    text: ImText,
) -> String {
    let mut sections = Vec::new();
    if rendered.dropped > 0 {
        sections.push(text.telegram_commentary_omitted(rendered.dropped));
    }
    sections.extend(rendered.entries.iter().map(|entry| entry.text.clone()));
    sections.join(SECTION_SEPARATOR)
}

fn largest_entry_budget(entry: &str, dropped: usize, text: ImText) -> usize {
    let mut lower = 0;
    let mut upper = entry.chars().count();
    while lower < upper {
        let candidate = lower + (upper - lower).div_ceil(2);
        let rendered = TelegramCommentaryRender {
            entries: vec![TelegramCommentaryRenderedEntry {
                text: truncate_middle(entry, candidate),
                sequence: 0,
            }],
            dropped,
        };
        if rendered_char_count(&rendered, text) <= TELEGRAM_COMMENTARY_MAX_CHARS {
            lower = candidate;
        } else {
            upper = candidate - 1;
        }
    }
    lower
}

fn rendered_char_count(rendered: &TelegramCommentaryRender, text: ImText) -> usize {
    let mut total = rendered
        .entries
        .iter()
        .map(|entry| entry.text.chars().count() + SECTION_SEPARATOR.chars().count())
        .sum::<usize>();
    if rendered.dropped > 0 {
        total = total.saturating_add(
            text.telegram_commentary_omitted(rendered.dropped)
                .chars()
                .count(),
        );
    }
    total
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

    /// 测试辅助：把渲染结果抽成纯文本列表。
    fn entry_texts(rendered: &TelegramCommentaryRender) -> Vec<String> {
        rendered
            .entries
            .iter()
            .map(|entry| entry.text.clone())
            .collect()
    }

    fn entries(count: usize) -> Vec<TelegramCommentaryEntry> {
        (1..=count)
            .map(|index| TelegramCommentaryEntry {
                item_id: format!("item-{index}"),
                text: format!("update {index}"),
                sequence: 0,
            })
            .collect()
    }

    #[test]
    fn one_entry_is_visible_without_omission() {
        let rendered = render_commentary(&entries(1), 0, ImText::zh_cn());

        assert_eq!(entry_texts(&rendered), vec!["update 1".to_string()]);
        assert_eq!(rendered.dropped, 0);
        assert_eq!(
            render_commentary_fallback(&rendered, ImText::zh_cn()),
            "update 1"
        );
    }

    #[test]
    fn every_entry_stays_visible_while_within_budget() {
        let rendered = render_commentary(&entries(8), 0, ImText::zh_cn());

        assert_eq!(rendered.entries.len(), 8);
        assert_eq!(rendered.dropped, 0);
    }

    #[test]
    fn dropped_history_is_reported_without_hiding_fresh_entries() {
        let rendered = render_commentary(&entries(3), 4, ImText::zh_cn());

        assert_eq!(rendered.entries.len(), 3);
        assert_eq!(rendered.dropped, 4);
        assert_eq!(
            render_commentary_fallback(&rendered, ImText::zh_cn()),
            "… 另外 4 条较早进展已省略\n\nupdate 1\n\nupdate 2\n\nupdate 3"
        );
    }

    #[test]
    fn strips_blank_entries_without_changing_order() {
        let rendered = render_commentary(
            &[
                TelegramCommentaryEntry {
                    item_id: "blank".to_string(),
                    text: "  ".to_string(),
                    sequence: 0,
                },
                TelegramCommentaryEntry {
                    item_id: "first".to_string(),
                    text: " first ".to_string(),
                    sequence: 0,
                },
                TelegramCommentaryEntry {
                    item_id: "second".to_string(),
                    text: "second".to_string(),
                    sequence: 0,
                },
            ],
            0,
            ImText::zh_cn(),
        );

        assert_eq!(
            entry_texts(&rendered),
            vec!["first".to_string(), "second".to_string()]
        );
        assert_eq!(rendered.dropped, 0);
    }

    #[test]
    fn discards_oldest_entries_to_fit_the_budget_and_accumulates_dropped() {
        let long = (1..=8)
            .map(|index| TelegramCommentaryEntry {
                item_id: format!("item-{index}"),
                text: format!("entry-{index}:{}", "x".repeat(900)),
                sequence: 0,
            })
            .collect::<Vec<_>>();
        let rendered = render_commentary(&long, 3, ImText::zh_cn());

        assert!(rendered_char_count(&rendered, ImText::zh_cn()) <= TELEGRAM_COMMENTARY_MAX_CHARS);
        assert!(rendered.dropped > 3);
        assert!(
            !rendered
                .entries
                .iter()
                .any(|entry| entry.text.starts_with("entry-1:"))
        );
        assert!(
            rendered
                .entries
                .iter()
                .any(|entry| entry.text.starts_with("entry-8:"))
        );
        assert!(
            render_commentary_fallback(&rendered, ImText::zh_cn())
                .contains(&format!("另外 {} 条较早进展已省略", rendered.dropped))
        );
    }

    #[test]
    fn truncates_one_oversized_entry_instead_of_dropping_it() {
        let rendered = render_commentary(
            &[TelegramCommentaryEntry {
                item_id: "latest".to_string(),
                text: format!("latest:{}", "界".repeat(5_000)),
                sequence: 0,
            }],
            4,
            ImText::zh_cn(),
        );

        assert!(rendered_char_count(&rendered, ImText::zh_cn()) <= TELEGRAM_COMMENTARY_MAX_CHARS);
        assert_eq!(rendered.entries.len(), 1);
        assert!(rendered.entries[0].text.contains('…'));
        assert!(rendered.entries[0].text.starts_with("latest:"));
        assert_eq!(rendered.dropped, 4);
    }

    #[test]
    fn english_labels_are_used_for_the_omitted_summary() {
        let en = ImText::for_locale(crate::im::core::i18n::ImLocale::EnUs);
        let rendered = render_commentary(&entries(2), 5, en);

        assert_eq!(rendered.dropped, 5);
        assert!(
            render_commentary_fallback(&rendered, en).starts_with("… 5 earlier updates omitted")
        );
    }
}
