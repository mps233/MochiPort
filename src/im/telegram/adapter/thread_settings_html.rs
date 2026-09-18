//! Extracted verbatim from `adapter.rs`; no behavior changes.

use super::*;

pub(super) fn thread_settings_html(
    request: &TelegramModelSwitchRequestState,
    text: ImText,
) -> String {
    match request.stage {
        TelegramThreadSettingsStage::Overview => thread_settings_overview_html(request, text),
        TelegramThreadSettingsStage::Model => thread_settings_model_html(request, text),
        TelegramThreadSettingsStage::Effort => thread_settings_effort_html(request, text),
        TelegramThreadSettingsStage::Speed => thread_settings_speed_html(request, text),
        TelegramThreadSettingsStage::CompatibilityConfirmation => {
            let compatibility = request
                .compatibility
                .as_ref()
                .expect("compatibility stage always has a compatibility plan");
            format!(
                "<b>{}</b>\n{}",
                telegram_html_escape(text.telegram_thread_settings_confirm_title()),
                telegram_html_escape(&text.telegram_thread_settings_confirm_body(
                    compatibility.reset_effort,
                    compatibility.reset_speed,
                )),
            )
        }
    }
}

pub(super) fn thread_settings_overview_html(
    request: &TelegramModelSwitchRequestState,
    text: ImText,
) -> String {
    let effective_model = thread_settings_observed_value(&request.observed.model, text, false);
    let effective_effort = thread_settings_observed_value(&request.observed.effort, text, false);
    let effective_speed =
        thread_settings_observed_value(&request.observed.service_tier, text, true);
    let draft_model = request
        .draft
        .model
        .as_deref()
        .map(thread_settings_code)
        .unwrap_or_else(|| telegram_html_escape(text.telegram_thread_settings_unchanged()));
    let draft_effort = request
        .draft
        .effort
        .as_deref()
        .map(|value| telegram_html_escape(&text.reasoning_effort_label(value)))
        .unwrap_or_else(|| telegram_html_escape(text.telegram_thread_settings_unchanged()));
    let draft_speed = match request.draft.speed {
        Some(TelegramThreadSettingsSpeed::Standard) => {
            telegram_html_escape(text.telegram_thread_settings_standard_speed())
        }
        Some(TelegramThreadSettingsSpeed::Fast) => {
            telegram_html_escape(text.telegram_thread_settings_fast_speed())
        }
        None => telegram_html_escape(text.telegram_thread_settings_unchanged()),
    };
    let stale = request.stale.then(|| {
        format!(
            "\n\n<b>{}</b>",
            telegram_html_escape(text.telegram_thread_settings_stale())
        )
    });
    format!(
        "<b>{}</b>\n<code>{}</code>\n\n<b>{}</b>\n{}：{}\n{}：{}\n{}：{}\n\n<b>{}</b>\n{}：{}\n{}：{}\n{}：{}{}",
        telegram_html_escape(text.telegram_thread_settings_title()),
        telegram_html_escape(&request.expected_thread_id),
        telegram_html_escape(text.telegram_thread_settings_effective_heading()),
        telegram_html_escape(text.telegram_thread_settings_model_button()),
        effective_model,
        telegram_html_escape(text.telegram_thread_settings_effort_button()),
        effective_effort,
        telegram_html_escape(text.telegram_thread_settings_speed_button()),
        effective_speed,
        telegram_html_escape(text.telegram_thread_settings_draft_heading()),
        telegram_html_escape(text.telegram_thread_settings_model_button()),
        draft_model,
        telegram_html_escape(text.telegram_thread_settings_effort_button()),
        draft_effort,
        telegram_html_escape(text.telegram_thread_settings_speed_button()),
        draft_speed,
        stale.unwrap_or_default(),
    )
}

pub(super) fn thread_settings_model_html(
    request: &TelegramModelSwitchRequestState,
    text: ImText,
) -> String {
    let page = request.model_page.max(1);
    let start = (page - 1) * 8;
    let end = (start + 8).min(request.catalog.len());
    let choices = request.catalog[start..end]
        .iter()
        .map(|choice| format!("<b>{}</b>", telegram_html_escape(&choice.label)))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "<b>{}</b>\n{}",
        telegram_html_escape(text.telegram_thread_settings_choose_model()),
        choices
    )
}

pub(super) fn thread_settings_effort_html(
    request: &TelegramModelSwitchRequestState,
    text: ImText,
) -> String {
    let Some(choice) = thread_settings_selected_model(request) else {
        return format!(
            "<b>{}</b>\n{}",
            telegram_html_escape(text.telegram_thread_settings_choose_effort()),
            telegram_html_escape(text.telegram_thread_settings_effort_unavailable()),
        );
    };
    if choice.supported_efforts.is_empty() {
        return format!(
            "<b>{}</b>\n{}",
            telegram_html_escape(text.telegram_thread_settings_choose_effort()),
            telegram_html_escape(text.telegram_thread_settings_effort_unavailable()),
        );
    }
    let choices = choice
        .supported_efforts
        .iter()
        .map(|effort| telegram_html_escape(&text.reasoning_effort_label(effort)))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "<b>{}</b>\n{}",
        telegram_html_escape(text.telegram_thread_settings_choose_effort()),
        choices
    )
}

pub(super) fn thread_settings_speed_html(
    request: &TelegramModelSwitchRequestState,
    text: ImText,
) -> String {
    let unavailable =
        if thread_settings_selected_model(request).is_none_or(|choice| !choice.supports_fast) {
            {
                format!(
                    "\n{}",
                    telegram_html_escape(text.telegram_thread_settings_fast_unavailable())
                )
            }
        } else {
            Default::default()
        };
    format!(
        "<b>{}</b>{}",
        telegram_html_escape(text.telegram_thread_settings_choose_speed()),
        unavailable
    )
}

pub(super) fn thread_settings_selected_model(
    request: &TelegramModelSwitchRequestState,
) -> Option<&crate::im::runtime::TelegramThreadSettingsModelChoice> {
    let model = request
        .draft
        .model
        .as_deref()
        .or(match &request.observed.model {
            ObservedSetting::Known(Some(model)) => Some(model.as_str()),
            _ => None,
        })?;
    request.catalog.iter().find(|choice| choice.model == model)
}

pub(super) fn thread_settings_observed_value(
    observed: &ObservedSetting<String>,
    text: ImText,
    speed: bool,
) -> String {
    match observed {
        ObservedSetting::Unknown => telegram_html_escape(text.telegram_thread_settings_unknown()),
        ObservedSetting::Known(None) if speed => {
            telegram_html_escape(text.telegram_thread_settings_standard_speed())
        }
        ObservedSetting::Known(None) => "—".to_string(),
        ObservedSetting::Known(Some(value)) if speed && value == "priority" => {
            telegram_html_escape(text.telegram_thread_settings_fast_speed())
        }
        ObservedSetting::Known(Some(value)) if speed => thread_settings_code(value),
        ObservedSetting::Known(Some(value)) => thread_settings_code(value),
    }
}

pub(super) fn thread_settings_code(value: &str) -> String {
    format!("<code>{}</code>", telegram_html_escape(value))
}

pub(super) fn create_option_row_html(index: usize, option: &ThreadCreateOption) -> String {
    let label = truncate_display_text(option.label.trim(), 34);
    let mut row = format!("/{} <b>{}</b>", index + 1, telegram_html_escape(&label));
    if let Some(summary) = option
        .summary
        .as_deref()
        .map(telegram_cleanup_text)
        .filter(|v| !v.is_empty())
    {
        row.push('\n');
        row.push_str(&option_summary_html(&summary));
    }
    row
}

pub(super) fn option_summary_html(summary: &str) -> String {
    let summary = truncate_middle(summary, 56);
    if looks_like_path(&summary) {
        format!("<code>{}</code>", telegram_html_escape(&summary))
    } else {
        telegram_html_escape(&summary)
    }
}

pub(super) fn create_options_html_text(
    title: &str,
    body: &str,
    page: usize,
    option_count: usize,
    options_html: &str,
    text: ImText,
) -> String {
    let hint = if option_count == 0 {
        text.no_options().to_string()
    } else {
        text.page_click_hint(page, option_count)
    };
    format!(
        "<b>{}</b>\n{}\n\n{}\n<code>{}</code>",
        telegram_html_escape(title),
        telegram_markdown_to_html(&telegram_cleanup_text(body)),
        options_html.trim_end(),
        telegram_html_escape(&hint)
    )
}
