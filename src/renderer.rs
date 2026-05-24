use serde_json::json;

const DEFAULT_CARD_TEMPLATE: &str = "blue";

fn normalize_card_markdown(text: &str) -> String {
    text.replace("\r\n", "\n").trim().to_string()
}

fn truncate_text(text: &str, max_chars: usize) -> String {
    let normalized = text.replace("\r\n", "\n");
    let mut out = String::new();
    for (idx, ch) in normalized.chars().enumerate() {
        if idx >= max_chars {
            out.push_str("\n\n<font color='grey'>内容较长，已截断显示。</font>");
            break;
        }
        out.push(ch);
    }
    out
}

pub fn build_markdown_card(
    text: &str,
    title: Option<&str>,
    template: Option<&str>,
) -> serde_json::Value {
    let content = normalize_card_markdown(text);
    let mut card = json!({
        "schema": "2.0",
        "config": {
            "wide_screen_mode": true
        },
        "body": {
            "elements": [
                {
                    "tag": "markdown",
                    "content": content
                }
            ]
        }
    });
    if let Some(title) = title.map(str::trim).filter(|value| !value.is_empty()) {
        card["header"] = json!({
            "title": {
                "tag": "plain_text",
                "content": title
            },
            "template": template.unwrap_or(DEFAULT_CARD_TEMPLATE)
        });
    }
    card
}

pub fn build_processing_card(prompt: &str) -> serde_json::Value {
    let prompt = truncate_text(&normalize_card_markdown(prompt), 800);
    build_markdown_card(
        &format!("**用户输入**\n{prompt}\n\n<font color='grey'>Codex 正在处理...</font>"),
        Some("Codex Remote"),
        Some("indigo"),
    )
}

pub fn build_desktop_user_message_card(prompt: &str) -> serde_json::Value {
    let content = truncate_text(&normalize_card_markdown(prompt), 4000);
    json!({
        "schema": "2.0",
        "config": {
            "wide_screen_mode": true
        },
        "body": {
            "padding": "0px",
            "elements": [
                {
                    "tag": "column_set",
                    "flex_mode": "none",
                    "horizontal_spacing": "0px",
                    "background_style": "yellow-50",
                    "columns": [
                        {
                            "tag": "column",
                            "width": "weighted",
                            "weight": 1,
                            "padding": "0px",
                            "margin": "0px",
                            "vertical_spacing": "8px",
                            "elements": [
                                {
                                    "tag": "markdown",
                                    "content": if content.is_empty() {
                                        "用户发送了一条消息。".to_string()
                                    } else {
                                        content
                                    }
                                },
                                {
                                    "tag": "hr"
                                },
                                {
                                    "tag": "markdown",
                                    "content": "_消息来源：Desktop userMessage_"
                                }
                            ]
                        }
                    ]
                }
            ]
        }
    })
}

pub fn build_approval_card(summary: &str) -> serde_json::Value {
    let content = normalize_card_markdown(summary);
    let mut card = build_markdown_card(&content, Some("待审批"), Some("orange"));
    card["body"]["elements"] = json!([
        {
            "tag": "markdown",
            "content": "Codex 当前需要你的确认，确认后才会继续执行。"
        },
        {
            "tag": "markdown",
            "content": content
        },
        {
            "tag": "hr"
        },
        {
            "tag": "markdown",
            "content": "**操作方式**\n- 在飞书回复 `/y` 通过\n- 在飞书回复 `/n` 拒绝"
        }
    ]);
    card
}

pub fn build_streaming_reply_card(text: &str, is_completed: bool) -> serde_json::Value {
    let content = if text.trim().is_empty() {
        "<font color='grey'>等待 Codex 输出...</font>".to_string()
    } else {
        truncate_text(&normalize_card_markdown(text), 12000)
    };
    let footer = if is_completed {
        "<font color='green'>已完成</font>"
    } else {
        "<font color='grey'>生成中</font>"
    };
    json!({
        "schema": "2.0",
        "config": {
            "wide_screen_mode": true
        },
        "header": {
            "title": {
                "tag": "plain_text",
                "content": "Codex"
            },
            "template": if is_completed { "green" } else { "blue" },
            "icon": {
                "tag": "standard_icon",
                "token": "robot_outlined",
                "color": "blue"
            }
        },
        "body": {
            "elements": [
                {
                    "tag": "markdown",
                    "content": content
                },
                {
                    "tag": "hr"
                },
                {
                    "tag": "markdown",
                    "content": footer
                }
            ]
        }
    })
}

pub fn build_turn_terminal_mark_card(state_text: &str) -> serde_json::Value {
    json!({
        "schema": "2.0",
        "config": {
            "wide_screen_mode": true
        },
        "body": {
            "padding": "0px 8px 0px 8px",
            "elements": [
                {
                    "tag": "column_set",
                    "flex_mode": "none",
                    "horizontal_spacing": "0px",
                    "columns": [
                        {
                            "tag": "column",
                            "width": "weighted",
                            "weight": 1,
                            "vertical_align": "center",
                            "padding": "6px 0px 6px 0px",
                            "elements": [
                                {
                                    "tag": "markdown",
                                    "content": state_text
                                }
                            ]
                        }
                    ]
                }
            ]
        }
    })
}
