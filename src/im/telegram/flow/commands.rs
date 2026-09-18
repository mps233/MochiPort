//! Telegram command parsing and the `/reply` granularity command.
//!
//! Moved verbatim out of `flow.rs`; no behavior changes.

use super::*;

pub(super) fn command_payload(text: &str) -> &str {
    text.find(char::is_whitespace)
        .map(|index| text[index..].trim())
        .unwrap_or_default()
}

/// `/回复 <档位>`：切换本账号的回复颗粒度并持久化；无参数时显示当前档位与可选项。
pub(super) async fn handle_telegram_reply_granularity_command(
    state: &SharedState,
    adapter: &TelegramAdapter,
    message: &InboundMessage,
    payload: &str,
) -> Result<()> {
    let text = im_text_for_state(state);
    let value = payload.trim();
    if value.is_empty() {
        let current = state
            .config
            .lock()
            .await
            .telegram_reply_granularity(&message.account_id);
        adapter
            .send_text(&message.chat_id, &text.telegram_granularity_status(current))
            .await?;
        return Ok(());
    }
    let Some(granularity) = TelegramReplyGranularity::parse(value) else {
        adapter
            .send_text(&message.chat_id, text.telegram_granularity_unknown())
            .await?;
        return Ok(());
    };
    let update = {
        let mut config = state.config.lock().await;
        match config.telegram_account(&message.account_id) {
            None => None,
            Some(mut account) => {
                account.reply_granularity = granularity;
                config.upsert_telegram_account(account);
                Some(config.save(&state.config_path).err())
            }
        }
    };
    let Some(save_error) = update else {
        adapter
            .send_text(&message.chat_id, text.telegram_granularity_unknown())
            .await?;
        return Ok(());
    };
    if let Some(err) = save_error {
        state
            .push_event(
                "error",
                "telegram_granularity_save_failed",
                format!("chat={} err={err}", message.chat_id),
            )
            .await;
    }
    adapter
        .send_text(
            &message.chat_id,
            &text.telegram_granularity_set(granularity),
        )
        .await?;
    Ok(())
}

pub(crate) fn command(text: &str) -> Option<String> {
    let first = text.split_whitespace().next()?.trim();
    if !first.starts_with('/') {
        return None;
    }
    let command = first
        .split_once('@')
        .map(|(command, _)| command)
        .unwrap_or(first)
        .to_ascii_lowercase();
    Some(command)
}

pub(crate) fn numeric_command_index(command: &str) -> Option<usize> {
    let number = command.strip_prefix('/')?.parse::<usize>().ok()?;
    number.checked_sub(1)
}
