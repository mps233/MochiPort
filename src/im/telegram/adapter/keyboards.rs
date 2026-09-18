//! Extracted verbatim from `adapter.rs`; no behavior changes.

use super::*;

pub(super) fn inline_keyboard(rows: Vec<Vec<serde_json::Value>>) -> serde_json::Value {
    json!({ "inline_keyboard": rows })
}

pub(super) fn empty_inline_keyboard() -> serde_json::Value {
    inline_keyboard(Vec::new())
}

pub(super) fn button(text: &str, callback_data: &str) -> serde_json::Value {
    json!({
        "text": truncate_button_text(text),
        "callback_data": callback_data,
    })
}

/// 会话设置选项键盘：每个选项一个按钮（tcs 逐项选择），外加翻页与返回。
/// 数字回复的后备路径不变，按钮与 `/N` 编号一一对应。
pub(super) fn create_options_keyboard(
    request_id: &str,
    field: &str,
    page: usize,
    options: &[ThreadCreateOption],
    has_prev: bool,
    has_next: bool,
    text: ImText,
) -> serde_json::Value {
    let mut rows: Vec<Vec<serde_json::Value>> = options
        .iter()
        .enumerate()
        .map(|(index, option)| {
            vec![button(
                option.label.replace('`', "").trim(),
                &format!("tcs:{request_id}:{field}:{page}:{index}"),
            )]
        })
        .collect();
    let mut nav = Vec::new();
    if has_prev {
        nav.push(button(
            text.previous_page_button(),
            &format!("tcp:{request_id}:{field}:prev"),
        ));
    }
    if has_next {
        nav.push(button(
            text.next_page_button(),
            &format!("tcp:{request_id}:{field}:next"),
        ));
    }
    if !nav.is_empty() {
        rows.push(nav);
    }
    if field == "cwd" {
        rows.push(vec![button(
            text.custom_cwd_label(),
            &format!("tcv:{request_id}:cwd:__custom__"),
        )]);
    }
    rows.push(vec![button(
        text.back_to_create_settings_button(),
        &format!("trc:{request_id}:new"),
    )]);
    inline_keyboard(rows)
}

pub(super) fn thread_settings_keyboard(
    request: &TelegramModelSwitchRequestState,
    text: ImText,
) -> serde_json::Value {
    let request_id = &request.request_id;
    let revision = request.revision;
    let mut rows = Vec::new();
    match request.stage {
        TelegramThreadSettingsStage::Overview => {
            rows.push(vec![
                button(
                    text.telegram_thread_settings_model_button(),
                    &format!("tmo:{request_id}:{revision}:model"),
                ),
                button(
                    text.telegram_thread_settings_effort_button(),
                    &format!("tmo:{request_id}:{revision}:effort"),
                ),
            ]);
            rows.push(vec![button(
                text.telegram_thread_settings_speed_button(),
                &format!("tmo:{request_id}:{revision}:speed"),
            )]);
            rows.push(vec![
                button(
                    text.telegram_thread_settings_cancel_button(),
                    &format!("tmc:{request_id}:{revision}"),
                ),
                button(
                    text.telegram_thread_settings_apply_button(),
                    &format!("tma:{request_id}:{revision}"),
                ),
            ]);
        }
        TelegramThreadSettingsStage::Model => {
            let page = request.model_page.max(1);
            let start = (page - 1) * 8;
            let end = (start + 8).min(request.catalog.len());
            for (index, choice) in request.catalog[start..end].iter().enumerate() {
                rows.push(vec![button(
                    &choice.label,
                    &format!("tms:{request_id}:{revision}:{page}:{index}"),
                )]);
            }
            let mut nav = Vec::new();
            if page > 1 {
                nav.push(button(
                    text.previous_page_button(),
                    &format!("tmp:{request_id}:{revision}:prev"),
                ));
            }
            if end < request.catalog.len() {
                nav.push(button(
                    text.next_page_button(),
                    &format!("tmp:{request_id}:{revision}:next"),
                ));
            }
            if !nav.is_empty() {
                rows.push(nav);
            }
            rows.push(vec![button(
                text.telegram_thread_settings_back_button(),
                &format!("tmb:{request_id}:{revision}"),
            )]);
        }
        TelegramThreadSettingsStage::Effort => {
            if let Some(choice) = thread_settings_selected_model(request) {
                for (index, effort) in choice.supported_efforts.iter().enumerate() {
                    rows.push(vec![button(
                        &text.reasoning_effort_label(effort),
                        &format!("tme:{request_id}:{revision}:{index}"),
                    )]);
                }
            }
            rows.push(vec![button(
                text.telegram_thread_settings_back_button(),
                &format!("tmb:{request_id}:{revision}"),
            )]);
        }
        TelegramThreadSettingsStage::Speed => {
            rows.push(vec![button(
                text.telegram_thread_settings_standard_speed(),
                &format!("tmv:{request_id}:{revision}:std"),
            )]);
            if thread_settings_selected_model(request).is_some_and(|choice| choice.supports_fast) {
                rows.push(vec![button(
                    text.telegram_thread_settings_fast_speed(),
                    &format!("tmv:{request_id}:{revision}:fast"),
                )]);
            }
            rows.push(vec![button(
                text.telegram_thread_settings_back_button(),
                &format!("tmb:{request_id}:{revision}"),
            )]);
        }
        TelegramThreadSettingsStage::CompatibilityConfirmation => {
            rows.push(vec![button(
                text.telegram_thread_settings_confirm_button(),
                &format!("tmq:{request_id}:{revision}:yes"),
            )]);
            rows.push(vec![button(
                text.telegram_thread_settings_back_button(),
                &format!("tmq:{request_id}:{revision}:no"),
            )]);
        }
    }
    inline_keyboard(rows)
}
