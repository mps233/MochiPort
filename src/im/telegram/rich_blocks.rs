use serde_json::{Value, json};

/// 极简卡片风的卡片头分隔线。
pub(crate) const TELEGRAM_CARD_SEPARATOR: &str = "──────────────";

pub(crate) fn text(value: impl Into<String>) -> Value {
    Value::String(value.into())
}

pub(crate) fn rich_text(parts: Vec<Value>) -> Value {
    Value::Array(parts)
}

pub(crate) fn inline_markdown(value: &str) -> Value {
    let mut parts = Vec::new();
    let mut plain = String::new();
    let mut rest = value;

    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("**")
            && let Some(end) = after.find("**")
            && end > 0
        {
            push_plain(&mut parts, &mut plain);
            parts.push(bold(&after[..end]));
            rest = &after[end + 2..];
            continue;
        }
        if let Some(after) = rest.strip_prefix('`')
            && let Some(end) = after.find('`')
            && end > 0
        {
            push_plain(&mut parts, &mut plain);
            parts.push(code(&after[..end]));
            rest = &after[end + 1..];
            continue;
        }
        if let Some(after_label) = rest.strip_prefix('[')
            && let Some(label_end) = after_label.find("](")
            && let Some(url_end) = after_label[label_end + 2..].find(')')
        {
            let label = &after_label[..label_end];
            let target = &after_label[label_end + 2..label_end + 2 + url_end];
            if !label.is_empty() {
                push_plain(&mut parts, &mut plain);
                if target.starts_with("https://") || target.starts_with("http://") {
                    parts.push(url(text(label), target));
                } else {
                    // 非 http(s) 目标（典型是 `/Users/...` 本地路径）无法变成可点的
                    // 链接。此时**只保留链接文字**，不要把 `[文字](路径)` 原文漏出去——
                    // 那正是用户看到的"markdown 原文格式"。
                    parts.push(text(label));
                }
                rest = &after_label[label_end + 2 + url_end + 1..];
                continue;
            }
        }
        // 斜体 `_文字_` / `*文字*`：协议没有斜体块，剥掉记号保留文字即可。
        // 只在成对且外侧是词边界时处理，避免破坏 `01_游戏_LOGO.png` 这类文件名。
        if (rest.starts_with('_') || rest.starts_with('*')) && !rest.starts_with("**") {
            let marker = rest.chars().next().expect("marker");
            if let Some(end) = rest[1..].find(marker) {
                let inner = &rest[1..1 + end];
                let after = &rest[1 + end + 1..];
                let boundary_before = plain.is_empty() || plain.ends_with(char::is_whitespace);
                let boundary_after = after.is_empty()
                    || after.starts_with(char::is_whitespace)
                    || after.starts_with([',', '.', '。', '，', '；', ';', ':', '：', ')', '（']);
                if boundary_before
                    && boundary_after
                    && !inner.is_empty()
                    && !inner.contains(marker)
                    && !inner.starts_with(char::is_whitespace)
                    && !inner.ends_with(char::is_whitespace)
                {
                    push_plain(&mut parts, &mut plain);
                    parts.push(text(inner));
                    rest = after;
                    continue;
                }
            }
        }

        let ch = rest.chars().next().expect("rest is non-empty");
        plain.push(ch);
        rest = &rest[ch.len_utf8()..];
    }
    push_plain(&mut parts, &mut plain);

    match parts.len() {
        0 => text(""),
        1 => parts.pop().expect("one inline part"),
        _ => rich_text(parts),
    }
}

fn push_plain(parts: &mut Vec<Value>, plain: &mut String) {
    if !plain.is_empty() {
        parts.push(text(std::mem::take(plain)));
    }
}

pub(crate) fn bold(value: impl Into<String>) -> Value {
    json!({
        "type": "bold",
        "text": value.into(),
    })
}

pub(crate) fn code(value: impl Into<String>) -> Value {
    json!({
        "type": "code",
        "text": value.into(),
    })
}

pub(crate) fn url(text: Value, target: impl Into<String>) -> Value {
    json!({
        "type": "url",
        "text": text,
        "url": target.into(),
    })
}

pub(crate) fn paragraph(value: Value) -> Value {
    json!({
        "type": "paragraph",
        "text": value,
    })
}

pub(crate) fn heading(value: Value, size: u8) -> Value {
    debug_assert!((1..=6).contains(&size));
    json!({
        "type": "heading",
        "text": value,
        "size": size.clamp(1, 6),
    })
}

pub(crate) fn preformatted(value: impl Into<String>, language: Option<&str>) -> Value {
    let mut block = json!({
        "type": "pre",
        "text": value.into(),
    });
    if let Some(language) = language.map(str::trim).filter(|value| !value.is_empty()) {
        block["language"] = text(language);
    }
    block
}

pub(crate) fn footer(value: Value) -> Value {
    json!({
        "type": "footer",
        "text": value,
    })
}

pub(crate) fn details(summary: Value, blocks: Vec<Value>, is_open: bool) -> Value {
    let mut block = json!({
        "type": "details",
        "summary": summary,
        "blocks": blocks,
    });
    if is_open {
        block["is_open"] = Value::Bool(true);
    }
    block
}

pub(crate) fn list(items: Vec<Value>) -> Value {
    json!({
        "type": "list",
        "items": items,
    })
}

pub(crate) fn list_item(blocks: Vec<Value>) -> Value {
    json!({ "blocks": blocks })
}

pub(crate) fn checklist_item(blocks: Vec<Value>, checked: bool) -> Value {
    let mut item = json!({
        "blocks": blocks,
        "has_checkbox": true,
    });
    if checked {
        item["is_checked"] = Value::Bool(true);
    }
    item
}

pub(crate) fn table(rows: Vec<Vec<Value>>, bordered: bool, striped: bool) -> Value {
    let mut block = json!({
        "type": "table",
        "cells": rows,
    });
    if bordered {
        block["is_bordered"] = Value::Bool(true);
    }
    if striped {
        block["is_striped"] = Value::Bool(true);
    }
    block
}

pub(crate) fn table_cell(text: Value, is_header: bool, align: &str) -> Value {
    debug_assert!(matches!(align, "left" | "center" | "right"));
    let mut cell = json!({
        "text": text,
        "align": align,
        "valign": if is_header { "middle" } else { "top" },
    });
    if is_header {
        cell["is_header"] = Value::Bool(true);
    }
    cell
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_collapsible_preformatted_details() {
        assert_eq!(
            details(
                text("最近错误"),
                vec![preformatted("503 Service Unavailable", Some("text"))],
                false,
            ),
            json!({
                "type": "details",
                "summary": "最近错误",
                "blocks": [{
                    "type": "pre",
                    "text": "503 Service Unavailable",
                    "language": "text",
                }],
            })
        );
    }

    #[test]
    fn unchecked_list_items_omit_is_checked() {
        assert_eq!(
            checklist_item(vec![paragraph(text("运行测试"))], false),
            json!({
                "blocks": [{"type": "paragraph", "text": "运行测试"}],
                "has_checkbox": true,
            })
        );
    }

    #[test]
    fn checked_list_items_use_telegram_native_state() {
        assert_eq!(
            checklist_item(vec![paragraph(text("运行测试"))], true),
            json!({
                "blocks": [{"type": "paragraph", "text": "运行测试"}],
                "has_checkbox": true,
                "is_checked": true,
            })
        );
    }

    #[test]
    fn composes_inline_rich_text() {
        assert_eq!(
            paragraph(rich_text(vec![
                bold("模型"),
                text("  "),
                code("gpt-5.6-sol")
            ])),
            json!({
                "type": "paragraph",
                "text": [
                    {"type": "bold", "text": "模型"},
                    "  ",
                    {"type": "code", "text": "gpt-5.6-sol"},
                ],
            })
        );
    }

    #[test]
    fn builds_clickable_url_rich_text() {
        assert_eq!(
            url(bold("Telegram"), "https://core.telegram.org"),
            json!({
                "type": "url",
                "text": {"type": "bold", "text": "Telegram"},
                "url": "https://core.telegram.org",
            })
        );
    }

    #[test]
    fn parses_supported_inline_markdown_without_leaking_markers() {
        assert_eq!(
            inline_markdown("**Done** with `cargo test` via [Telegram](https://telegram.org)"),
            json!([
                {"type": "bold", "text": "Done"},
                " with ",
                {"type": "code", "text": "cargo test"},
                " via ",
                {
                    "type": "url",
                    "text": "Telegram",
                    "url": "https://telegram.org",
                },
            ])
        );
    }

    #[test]
    fn keeps_unmatched_inline_markdown_as_plain_text() {
        assert_eq!(inline_markdown("**unfinished"), text("**unfinished"));
    }

    /// 斜体剥离不能误伤文件名里的下划线。
    ///
    /// `01_游戏_LOGO_2019版_250px.png` 这类素材名在同一段文案里很常见；若把
    /// 成对下划线一律当斜体，文件名会被吃掉或变形。
    #[test]
    fn italic_stripping_does_not_mangle_snake_case_filenames() {
        assert_eq!(
            inline_markdown("素材 01_游戏_LOGO_2019版_250px.png 已下载"),
            text("素材 01_游戏_LOGO_2019版_250px.png 已下载")
        );
        // 真正的斜体仍要被剥掉。
        assert_eq!(
            inline_markdown("_更多_ 里还有"),
            json!([text("更多"), " 里还有"])
        );
    }

    /// 非 http(s) 链接只保留文字，不泄漏 `[文字](路径)` 原文。
    #[test]
    fn local_path_links_keep_only_the_label() {
        assert_eq!(
            inline_markdown("[战双 logo](/Users/miaopasi/a.png) 已整理"),
            json!([text("战双 logo"), " 已整理"])
        );
        // http(s) 仍转成可点链接（只有一段时会折叠成裸对象，不包数组）。
        assert_eq!(
            inline_markdown("[README](https://example.com/r.md)"),
            json!({"type": "url", "text": "README", "url": "https://example.com/r.md"})
        );
    }

    #[test]
    fn builds_bordered_table_with_required_cell_alignment() {
        assert_eq!(
            table(
                vec![
                    vec![
                        table_cell(text("文件"), true, "left"),
                        table_cell(text("新增"), true, "right"),
                    ],
                    vec![
                        table_cell(code("src/main.rs"), false, "left"),
                        table_cell(text("+2"), false, "right"),
                    ],
                ],
                true,
                true,
            ),
            json!({
                "type": "table",
                "cells": [
                    [
                        {
                            "text": "文件",
                            "is_header": true,
                            "align": "left",
                            "valign": "middle",
                        },
                        {
                            "text": "新增",
                            "is_header": true,
                            "align": "right",
                            "valign": "middle",
                        },
                    ],
                    [
                        {
                            "text": {"type": "code", "text": "src/main.rs"},
                            "align": "left",
                            "valign": "top",
                        },
                        {
                            "text": "+2",
                            "align": "right",
                            "valign": "top",
                        },
                    ],
                ],
                "is_bordered": true,
                "is_striped": true,
            })
        );
    }
}
