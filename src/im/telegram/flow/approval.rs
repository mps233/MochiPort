//! Telegram approval replies: text replies, button replies and the resulting approval outcome handling.
//!
//! Moved verbatim out of `flow.rs`; no behavior changes.

use super::*;

pub(super) fn approval_decision_fallback_text(text: ImText, label: &str) -> String {
    text.approval_decision_submitted_label(&text.approval_decision_label(label))
}

pub(super) async fn handle_telegram_approval_text_reply(
    state: &SharedState,
    outbound_tx: &ImOutboundSender,
    adapter: &TelegramAdapter,
    message: &InboundMessage,
    command: &str,
) -> Result<bool> {
    handle_telegram_approval_outcome(
        state,
        outbound_tx,
        adapter,
        message,
        resolve_approval_reply(state, message, command).await,
    )
    .await
}

pub(super) async fn handle_telegram_approval_button_reply(
    state: &SharedState,
    outbound_tx: &ImOutboundSender,
    adapter: &TelegramAdapter,
    message: &InboundMessage,
    request_fingerprint: &str,
    option_index: usize,
) -> Result<bool> {
    handle_telegram_approval_outcome(
        state,
        outbound_tx,
        adapter,
        message,
        resolve_approval_button_reply(state, message, request_fingerprint, option_index).await,
    )
    .await
}

async fn handle_telegram_approval_outcome(
    state: &SharedState,
    outbound_tx: &ImOutboundSender,
    adapter: &TelegramAdapter,
    message: &InboundMessage,
    outcome: ApprovalReplyOutcome,
) -> Result<bool> {
    match outcome {
        ApprovalReplyOutcome::Ready {
            conversation_key,
            pending,
            option_index,
            decision,
        } => {
            let approval_message_id = pending
                .message_id
                .clone()
                .or_else(|| message.card_message_id.clone());
            let next = submit_approval_decision(state, &pending, &decision).await?;
            let text = im_text_for_state(state);
            let resolved = adapter
                .update_resolved_approval(
                    &message.chat_id,
                    approval_message_id.as_deref(),
                    &pending,
                    option_index,
                    &decision.label,
                    text,
                )
                .await;
            let update_succeeded = match resolved {
                Ok(updated) => updated,
                Err(err) => {
                    state
                        .push_event(
                            "warn",
                            "telegram_approval_update_failed",
                            format!(
                                "conversation={} request_id={} message={} err={err}",
                                conversation_key,
                                pending.request_id,
                                approval_message_id.as_deref().unwrap_or("")
                            ),
                        )
                        .await;
                    false
                }
            };
            if !update_succeeded {
                adapter
                    .send_text(
                        &message.chat_id,
                        &approval_decision_fallback_text(text, &decision.label),
                    )
                    .await?;
            }
            state
                .push_event(
                    "info",
                    "telegram_approval_decision_sent",
                    format!(
                        "conversation={} request_id={} option={} label={}",
                        conversation_key, pending.request_id, option_index, decision.label
                    ),
                )
                .await;
            if let Some((conversation_key, next_approval)) = next {
                events::send_next_approval(state, outbound_tx, &conversation_key, &next_approval)
                    .await?;
            }
        }
        ApprovalReplyOutcome::NoPending => {
            let text = im_text_for_state(state);
            let _ = adapter
                .clear_reply_markup(&message.chat_id, message.card_message_id.as_deref())
                .await;
            adapter
                .send_text(&message.chat_id, text.no_pending_approval())
                .await?;
        }
        ApprovalReplyOutcome::NotCurrent => {
            let text = im_text_for_state(state);
            let _ = adapter
                .clear_reply_markup(&message.chat_id, message.card_message_id.as_deref())
                .await;
            adapter
                .send_text(&message.chat_id, text.approval_not_current())
                .await?;
        }
        ApprovalReplyOutcome::InvalidInput { hint } => {
            let text = im_text_for_state(state);
            adapter
                .send_text(&message.chat_id, &text.invalid_approval_reply(&hint))
                .await?;
        }
    }
    Ok(true)
}
