//! Extracted verbatim from `adapter.rs`; no behavior changes.

use super::*;

pub(super) fn approval_text(approval: &PendingApproval, text: ImText) -> String {
    let kind = text.approval_kind_label(&approval.request_kind);
    let mut summary = text.localize_approval_summary(&truncate_approval_summary(&approval.summary));
    // 有 inline 按钮时按钮即选项，数字列表只保留给没有按钮的兜底形态。
    let mut action_lines = if approval.decisions.is_empty() {
        vec![
            format!("`/y` · {}", text.approval_accept_command_label()),
            format!("`/n` · {}", text.approval_decline_command_label()),
        ]
    } else {
        Vec::new()
    };
    let mut footer = if approval.decisions.is_empty() {
        text.telegram_approval_fallback_footer(&text.approval_reply_hint(approval))
    } else {
        text.telegram_approval_reply_footer().to_string()
    };

    // Approval cards need to remain a single message because only one message
    // id is retained for the later resolved-state edit. Keep the summary and
    // action list readable while trimming oversized protocol payloads.
    loop {
        let rendered = render_approval_text(&kind, &summary, &action_lines, &footer, text);
        if rendered.chars().count() <= TELEGRAM_MAX_MESSAGE_CHARS {
            return rendered;
        }

        let excess = rendered
            .chars()
            .count()
            .saturating_sub(TELEGRAM_MAX_MESSAGE_CHARS);
        if !summary.is_empty() {
            let keep = summary.chars().count().saturating_sub(excess);
            summary = truncate_text_with_ellipsis(&summary, keep);
        } else if action_lines.len() > 1 {
            action_lines.pop();
        } else {
            // A pathological number of options can also make the reply hint
            // itself too long. It is only a convenience string; the buttons
            // and the visible first option remain usable after this trim.
            let without_footer = render_approval_text(&kind, &summary, &action_lines, "", text);
            let footer_budget =
                TELEGRAM_MAX_MESSAGE_CHARS.saturating_sub(without_footer.chars().count());
            footer = truncate_text_with_ellipsis(&footer, footer_budget);
        }
    }
}

pub(super) fn render_approval_text(
    kind: &str,
    summary: &str,
    action_lines: &[String],
    footer: &str,
    text: ImText,
) -> String {
    let mut lines = vec![
        format!("**{}**", text.approval_pending_title()),
        text.field_line(text.approval_type_label(), &format!("`{kind}`")),
        String::new(),
        format!("**{}**", text.approval_details_label()),
        summary.to_string(),
        String::new(),
    ];
    if !action_lines.is_empty() {
        lines.push(format!("**{}**", text.approval_actions_label()));
        lines.extend(action_lines.iter().cloned());
        lines.push(String::new());
    }
    lines.push(footer.to_string());
    lines.join("\n")
}

pub(super) fn truncate_text_with_ellipsis(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    if max_chars <= 1 {
        return "…".chars().take(max_chars).collect();
    }
    let mut output = text
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    output.push('…');
    output
}

pub(super) fn resolved_approval_text(
    approval: &PendingApproval,
    decision_label: &str,
    text: ImText,
) -> String {
    let kind = text.approval_kind_label(&approval.request_kind);
    let summary = text.localize_approval_summary(&truncate_approval_summary(&approval.summary));
    [
        format!("✅ **{}**", text.approval_resolved_title()),
        text.field_line(text.approval_type_label(), &format!("`{kind}`")),
        String::new(),
        format!("**{}**", text.approval_details_label()),
        summary,
        String::new(),
        format!(
            "**{}**",
            text.approval_selected_label(&approval_decision_display_label(text, decision_label))
        ),
    ]
    .join("\n")
}

pub(super) fn approval_keyboard(
    approval: &PendingApproval,
    text: ImText,
) -> Option<serde_json::Value> {
    let fingerprint = approval_request_fingerprint(&approval.request_key());
    let rows = approval
        .decisions
        .iter()
        .enumerate()
        .map(|(index, decision)| {
            vec![approval_button(
                &approval_button_label(text, &decision.label),
                &format!("ap:{fingerprint}:{}", index + 1),
            )]
        })
        .collect::<Vec<_>>();
    (!rows.is_empty()).then(|| inline_keyboard(rows))
}

pub(super) fn approval_button(text: &str, callback_data: &str) -> serde_json::Value {
    json!({
        "text": truncate_button_text(text),
        "callback_data": callback_data,
    })
}

pub(super) fn approval_button_label(text: ImText, label: &str) -> String {
    approval_decision_display_label(text, label)
}

pub(super) fn approval_decision_display_label(text: ImText, label: &str) -> String {
    truncate_display_text(
        &text.approval_decision_label(label).replace('`', ""),
        TELEGRAM_APPROVAL_DECISION_MAX_CHARS,
    )
}

pub(super) fn truncate_approval_summary(summary: &str) -> String {
    let summary = telegram_cleanup_text(summary);
    let summary = summary.trim();
    if summary.chars().count() <= TELEGRAM_APPROVAL_SUMMARY_MAX_CHARS {
        return summary.to_string();
    }
    let mut output = summary
        .chars()
        .take(TELEGRAM_APPROVAL_SUMMARY_MAX_CHARS.saturating_sub(1))
        .collect::<String>();
    output.push('…');
    output
}
