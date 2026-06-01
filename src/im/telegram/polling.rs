use anyhow::Result;
use tokio::sync::mpsc;
use tokio::time::{Duration, sleep};

use crate::{
    app_state::SharedState,
    types::{
        ChatType, ImPlatformKind, InboundAction, InboundMessage, ThreadRouteDirection, now_ms,
    },
};

use super::{
    api::{TelegramApi, TelegramApiError, TelegramCallbackQuery, TelegramMessage},
    types::TelegramSettings,
};

const TELEGRAM_ACCOUNT_ID: &str = "telegram";
const TELEGRAM_LONG_POLL_TIMEOUT_SECONDS: u32 = 25;
const TELEGRAM_STARTUP_PROBE_RETRY_SECONDS: u64 = 5;
const TELEGRAM_CONFLICT_BACKOFF_SECONDS: u64 = 35;
const TELEGRAM_GENERIC_RETRY_SECONDS: u64 = 5;

pub async fn listen_polling(
    state: SharedState,
    api: TelegramApi,
    tx: mpsc::Sender<InboundMessage>,
) -> Result<()> {
    let mut offset = None;
    set_polling_state(&state, true, false, None).await;
    claim_polling_slot(&state, &api, &mut offset).await;
    loop {
        let updates = match api
            .get_updates(offset, TELEGRAM_LONG_POLL_TIMEOUT_SECONDS)
            .await
        {
            Ok(updates) => updates,
            Err(err) => {
                handle_polling_error(&state, &err).await;
                continue;
            }
        };
        set_polling_state(&state, true, true, None).await;
        let update_count = updates.len();
        for update in updates {
            offset = Some(update.update_id + 1);
            if let Some(callback) = update.callback_query {
                let callback_id = callback.id.clone();
                if let Some(inbound) = inbound_from_callback(api.settings(), callback) {
                    let _ = api
                        .answer_callback_query(&callback_id, Some("已收到"))
                        .await;
                    tx.send(inbound)
                        .await
                        .map_err(|_| anyhow::anyhow!("telegram inbound pump closed"))?;
                    update_last_inbound(&state).await;
                } else {
                    let _ = api
                        .answer_callback_query(&callback_id, Some("这个操作不可用"))
                        .await;
                }
                continue;
            }
            if let Some(message) = update.message
                && let Some(inbound) = inbound_from_message(api.settings(), message)
            {
                let _ = api.send_chat_action(&inbound.chat_id, "typing").await;
                tx.send(inbound)
                    .await
                    .map_err(|_| anyhow::anyhow!("telegram inbound pump closed"))?;
                update_last_inbound(&state).await;
            }
        }
        if update_count > 0 {
            state
                .push_event(
                    "info",
                    "telegram_poll_ok",
                    format!("updates={update_count}"),
                )
                .await;
        }
    }
}

async fn claim_polling_slot(state: &SharedState, api: &TelegramApi, offset: &mut Option<i64>) {
    loop {
        match api.get_updates(*offset, 0).await {
            Ok(updates) => {
                for update in updates {
                    *offset = Some(update.update_id + 1);
                }
                set_polling_state(state, true, true, None).await;
                state
                    .push_event(
                        "info",
                        "telegram_poll_ready",
                        "startup probe ok".to_string(),
                    )
                    .await;
                return;
            }
            Err(err) => {
                let delay = retry_delay_seconds(&err, TELEGRAM_STARTUP_PROBE_RETRY_SECONDS);
                set_polling_state(state, true, false, Some(err.to_string())).await;
                state
                    .push_event(
                        "warn",
                        "telegram_poll_probe_failed",
                        format!("retry_in={delay}s err={err}"),
                    )
                    .await;
                sleep(Duration::from_secs(delay)).await;
            }
        }
    }
}

async fn handle_polling_error(state: &SharedState, err: &anyhow::Error) {
    let delay = retry_delay_seconds(err, TELEGRAM_GENERIC_RETRY_SECONDS);
    let kind = err
        .downcast_ref::<TelegramApiError>()
        .filter(|api_error| api_error.is_conflict())
        .map(|_| "telegram_poll_conflict")
        .unwrap_or("telegram_poll_failed");
    set_polling_state(state, true, false, Some(err.to_string())).await;
    state
        .push_event("warn", kind, format!("retry_in={delay}s err={err}"))
        .await;
    sleep(Duration::from_secs(delay)).await;
}

fn retry_delay_seconds(err: &anyhow::Error, default_delay: u64) -> u64 {
    if let Some(api_error) = err.downcast_ref::<TelegramApiError>() {
        if api_error.is_conflict() {
            return TELEGRAM_CONFLICT_BACKOFF_SECONDS;
        }
        if let Some(retry_after) = api_error.retry_after {
            return retry_after.max(1);
        }
    }
    default_delay
}

fn inbound_from_message(
    settings: &TelegramSettings,
    message: TelegramMessage,
) -> Option<InboundMessage> {
    if message.chat.kind != "private" {
        return None;
    }
    let text = message.text?.trim().to_string();
    if text.is_empty() {
        return None;
    }
    let chat_id = message.chat.id.to_string();
    if !chat_allowed(settings, &chat_id) {
        return None;
    }
    let sender_id = message
        .from
        .as_ref()
        .map(|user| user.id.to_string())
        .unwrap_or_else(|| chat_id.clone());

    Some(InboundMessage {
        platform: ImPlatformKind::Telegram,
        account_id: TELEGRAM_ACCOUNT_ID.to_string(),
        sender_id,
        chat_id,
        chat_type: ChatType::Direct,
        message_id: message.message_id.to_string(),
        text,
        mentioned: true,
        approval_request_key: None,
        action: None,
        card_message_id: None,
        attachments: vec![],
    })
}

fn inbound_from_callback(
    settings: &TelegramSettings,
    callback: TelegramCallbackQuery,
) -> Option<InboundMessage> {
    let data = callback.data?;
    let action = action_from_callback_data(&data)?;
    let message = callback.message?;
    if message.chat.kind != "private" {
        return None;
    }
    let chat_id = message.chat.id.to_string();
    if !chat_allowed(settings, &chat_id) {
        return None;
    }

    Some(InboundMessage {
        platform: ImPlatformKind::Telegram,
        account_id: TELEGRAM_ACCOUNT_ID.to_string(),
        sender_id: callback.from.id.to_string(),
        chat_id,
        chat_type: ChatType::Direct,
        message_id: message.message_id.to_string(),
        text: data,
        mentioned: true,
        approval_request_key: None,
        action: Some(action),
        card_message_id: Some(message.message_id.to_string()),
        attachments: vec![],
    })
}

async fn set_polling_state(
    state: &SharedState,
    polling: bool,
    connected: bool,
    last_error: Option<String>,
) {
    let mut telegram = state.telegram.lock().await;
    telegram.polling = polling;
    telegram.connected = connected;
    telegram.last_error = last_error;
    telegram.last_event_at_ms = Some(now_ms());
}

async fn update_last_inbound(state: &SharedState) {
    let mut telegram = state.telegram.lock().await;
    let now = now_ms();
    telegram.last_event_at_ms = Some(now);
    telegram.last_inbound_at_ms = Some(now);
}

fn chat_allowed(settings: &TelegramSettings, chat_id: &str) -> bool {
    settings.allowed_chat_ids.is_empty()
        || settings
            .allowed_chat_ids
            .iter()
            .any(|allowed| allowed == chat_id)
}

fn action_from_callback_data(data: &str) -> Option<InboundAction> {
    let parts = data.split(':').collect::<Vec<_>>();
    match parts.as_slice() {
        ["ap", request_fingerprint, option_index] => Some(InboundAction::ApprovalDecision {
            request_fingerprint: (*request_fingerprint).to_string(),
            option_index: option_index.parse().ok()?,
        }),
        ["trc", request_id, action] => Some(InboundAction::ThreadRouteChoice {
            request_id: (*request_id).to_string(),
            action: match *action {
                "new" => "create_new",
                "load" => "resume_history",
                "back" => "back",
                _ => return None,
            }
            .to_string(),
        }),
        ["trd", request_id] => Some(InboundAction::ThreadRouteCreateDefault {
            request_id: (*request_id).to_string(),
        }),
        ["tcc", request_id] => Some(InboundAction::ThreadRouteCreateConfigured {
            request_id: (*request_id).to_string(),
        }),
        ["tce", request_id, field] => Some(InboundAction::ThreadRouteCreateEdit {
            request_id: (*request_id).to_string(),
            field: (*field).to_string(),
        }),
        ["tcs", request_id, field, page, index] => Some(InboundAction::ThreadRouteCreateSetIndex {
            request_id: (*request_id).to_string(),
            field: (*field).to_string(),
            page: page.parse().ok()?,
            index: index.parse().ok()?,
        }),
        ["tcv", request_id, field, value] => Some(InboundAction::ThreadRouteCreateSetValue {
            request_id: (*request_id).to_string(),
            field: (*field).to_string(),
            value: (*value).to_string(),
        }),
        ["tcp", request_id, field, direction] => {
            Some(InboundAction::ThreadRouteCreateOptionsPage {
                request_id: (*request_id).to_string(),
                field: (*field).to_string(),
                direction: match *direction {
                    "prev" => ThreadRouteDirection::Prev,
                    "next" => ThreadRouteDirection::Next,
                    _ => return None,
                },
            })
        }
        ["trs", request_id, page, index] => Some(InboundAction::ThreadRouteResumeIndex {
            request_id: (*request_id).to_string(),
            page: page.parse().ok()?,
            index: index.parse().ok()?,
        }),
        ["tlp", request_id, direction] => Some(InboundAction::ThreadRouteListPage {
            request_id: (*request_id).to_string(),
            direction: match *direction {
                "prev" => ThreadRouteDirection::Prev,
                "next" => ThreadRouteDirection::Next,
                _ => return None,
            },
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::im::telegram::api::{TelegramChat, TelegramUser};

    #[test]
    fn converts_private_message_to_inbound() {
        let settings = TelegramSettings::default();
        let inbound = inbound_from_message(
            &settings,
            TelegramMessage {
                message_id: 9,
                from: Some(TelegramUser {
                    id: 42,
                    is_bot: false,
                    username: Some("ada".to_string()),
                    first_name: Some("Ada".to_string()),
                    last_name: None,
                }),
                chat: TelegramChat {
                    id: 42,
                    kind: "private".to_string(),
                    title: None,
                    username: Some("ada".to_string()),
                    first_name: Some("Ada".to_string()),
                    last_name: None,
                },
                text: Some("/status".to_string()),
            },
        )
        .expect("inbound message");

        assert_eq!(inbound.platform, ImPlatformKind::Telegram);
        assert_eq!(inbound.conversation_key(), "telegram:telegram:42");
        assert_eq!(inbound.chat_type, ChatType::Direct);
        assert_eq!(inbound.text, "/status");
    }

    #[test]
    fn ignores_group_messages() {
        let settings = TelegramSettings {
            mention_only: true,
            ..TelegramSettings::default()
        };
        let message = TelegramMessage {
            message_id: 10,
            from: Some(TelegramUser {
                id: 42,
                is_bot: false,
                username: Some("ada".to_string()),
                first_name: Some("Ada".to_string()),
                last_name: None,
            }),
            chat: TelegramChat {
                id: -100,
                kind: "group".to_string(),
                title: Some("Codex".to_string()),
                username: None,
                first_name: None,
                last_name: None,
            },
            text: Some("hello".to_string()),
        };

        assert!(inbound_from_message(&settings, message).is_none());
    }

    #[test]
    fn ignores_group_messages_even_when_mentioned() {
        let settings = TelegramSettings {
            mention_only: true,
            ..TelegramSettings::default()
        };
        let inbound = inbound_from_message(
            &settings,
            TelegramMessage {
                message_id: 11,
                from: Some(TelegramUser {
                    id: 42,
                    is_bot: false,
                    username: Some("ada".to_string()),
                    first_name: Some("Ada".to_string()),
                    last_name: None,
                }),
                chat: TelegramChat {
                    id: -100,
                    kind: "group".to_string(),
                    title: Some("Codex".to_string()),
                    username: None,
                    first_name: None,
                    last_name: None,
                },
                text: Some("@codex_bot hello".to_string()),
            },
        );

        assert!(inbound.is_none());
    }

    #[test]
    fn parses_thread_route_callback_data() {
        let action = action_from_callback_data("trs:thread-route-7:2:3").expect("resume action");
        match action {
            InboundAction::ThreadRouteResumeIndex {
                request_id,
                page,
                index,
            } => {
                assert_eq!(request_id, "thread-route-7");
                assert_eq!(page, 2);
                assert_eq!(index, 3);
            }
            other => panic!("unexpected action: {other:?}"),
        }

        let action = action_from_callback_data("ap:abc123:2").expect("approval action");
        match action {
            InboundAction::ApprovalDecision {
                request_fingerprint,
                option_index,
            } => {
                assert_eq!(request_fingerprint, "abc123");
                assert_eq!(option_index, 2);
            }
            other => panic!("unexpected action: {other:?}"),
        }

        let action = action_from_callback_data("tlp:thread-route-7:next").expect("page action");
        match action {
            InboundAction::ThreadRouteListPage {
                request_id,
                direction,
            } => {
                assert_eq!(request_id, "thread-route-7");
                assert_eq!(direction, ThreadRouteDirection::Next);
            }
            other => panic!("unexpected action: {other:?}"),
        }
    }
}
