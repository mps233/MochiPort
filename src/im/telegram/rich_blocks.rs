use serde_json::{Value, json};

pub(crate) fn text(value: impl Into<String>) -> Value {
    Value::String(value.into())
}

pub(crate) fn rich_text(parts: Vec<Value>) -> Value {
    Value::Array(parts)
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

pub(crate) fn divider() -> Value {
    json!({ "type": "divider" })
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
