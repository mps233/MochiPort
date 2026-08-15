use anyhow::{Context, Result};
use serde_json::json;
use std::path::Path;
use tokio::time::{Duration, sleep};

use crate::{
    chain_log,
    im::core::{i18n::ImText, thread::ThreadCreateOption},
    im_runtime::{PendingApproval, approval_request_fingerprint},
};

use super::api::{TelegramApi, TelegramApiError, TelegramInputRichMessage, TelegramParseMode};

const TELEGRAM_MAX_MESSAGE_CHARS: usize = 4096;
const TELEGRAM_CONTINUATION_OVERHEAD: usize = 30;
const TELEGRAM_CHUNK_DELAY_MS: u64 = 100;

#[derive(Clone)]
pub struct TelegramAdapter {
    api: TelegramApi,
}

#[derive(Debug, Clone)]
pub struct TelegramThreadListEntry {
    pub title: String,
    pub state: String,
    pub cwd: Option<String>,
}

impl TelegramAdapter {
    pub fn new(api: TelegramApi) -> Self {
        Self { api }
    }

    pub async fn send_text(&self, target: &str, text: &str) -> Result<String> {
        let text = telegram_cleanup_text(text);
        let mut last_message_id = 0;
        let chunks = telegram_text_chunks(&text);
        log_adapter(
            "send_text_begin",
            format!(
                "chat={} chars={} chunks={} preview={}",
                target,
                text.chars().count(),
                chunks.len(),
                log_text_preview(&text, 500)
            ),
        );
        for (index, chunk) in chunks.iter().enumerate() {
            let html = telegram_markdown_to_html(chunk);
            log_adapter(
                "send_text_chunk_begin",
                format!(
                    "chat={} chunk={}/{} chars={} preview={}",
                    target,
                    index + 1,
                    chunks.len(),
                    chunk.chars().count(),
                    log_text_preview(chunk, 500)
                ),
            );
            last_message_id = match self
                .api
                .send_text_parse_mode(target, &html, TelegramParseMode::Html)
                .await
            {
                Ok(message_id) => {
                    log_adapter(
                        "send_text_chunk_sent",
                        format!(
                            "chat={} chunk={}/{} mode=html message={}",
                            target,
                            index + 1,
                            chunks.len(),
                            message_id
                        ),
                    );
                    message_id
                }
                Err(err) => {
                    log_adapter(
                        "send_text_html_failed",
                        format!(
                            "chat={} chunk={}/{} fallback=plain err={}",
                            target,
                            index + 1,
                            chunks.len(),
                            err
                        ),
                    );
                    let message_id = self.api.send_text(target, &chunk).await?;
                    log_adapter(
                        "send_text_chunk_sent",
                        format!(
                            "chat={} chunk={}/{} mode=plain message={}",
                            target,
                            index + 1,
                            chunks.len(),
                            message_id
                        ),
                    );
                    message_id
                }
            };
            if index + 1 < chunks.len() {
                sleep(Duration::from_millis(TELEGRAM_CHUNK_DELAY_MS)).await;
            }
        }
        log_adapter(
            "send_text_done",
            format!(
                "chat={} chunks={} message={}",
                target,
                chunks.len(),
                last_message_id
            ),
        );
        Ok(last_message_id.to_string())
    }

    pub async fn send_turn_completed(
        &self,
        target: &str,
        reply_text: &str,
        footer_text: &str,
    ) -> Result<String> {
        let chunks = telegram_turn_completed_chunks(reply_text, footer_text);
        let mut last_message_id = String::new();
        for (index, chunk) in chunks.iter().enumerate() {
            let is_last = index + 1 == chunks.len();
            if is_last {
                let (rich_markdown, fallback_markdown) =
                    telegram_turn_completed_messages(chunk, footer_text);
                let rich_message = TelegramInputRichMessage::markdown(rich_markdown);
                last_message_id = self
                    .send_or_update_rich_message(target, None, &rich_message, &fallback_markdown)
                    .await?;
            } else {
                last_message_id = self.send_text(target, chunk).await?;
                sleep(Duration::from_millis(TELEGRAM_CHUNK_DELAY_MS)).await;
            }
        }
        Ok(last_message_id)
    }

    pub async fn send_user_message_quote(
        &self,
        target: &str,
        message_text: &str,
        credit_text: &str,
    ) -> Result<String> {
        let chunks = telegram_user_message_chunks(message_text, credit_text);
        let mut last_message_id = String::new();
        for (index, chunk) in chunks.iter().enumerate() {
            let (rich_html, fallback_markdown) = telegram_user_message_messages(chunk, credit_text);
            let rich_message = TelegramInputRichMessage::html(rich_html);
            last_message_id = self
                .send_or_update_rich_message(target, None, &rich_message, &fallback_markdown)
                .await?;
            if index + 1 < chunks.len() {
                sleep(Duration::from_millis(TELEGRAM_CHUNK_DELAY_MS)).await;
            }
        }
        Ok(last_message_id)
    }

    pub async fn send_context_compaction(
        &self,
        target: &str,
        title_text: &str,
        credit_text: &str,
    ) -> Result<String> {
        let (rich_html, fallback_text) =
            telegram_context_compaction_messages(title_text, credit_text);
        let rich_message = TelegramInputRichMessage::html(rich_html);
        self.send_or_update_rich_message(target, None, &rich_message, &fallback_text)
            .await
    }

    pub async fn send_typing_action(&self, target: &str) -> Result<()> {
        log_adapter("send_typing_action", format!("chat={target}"));
        self.api.send_chat_action(target, "typing").await
    }

    pub async fn send_rich_thinking_draft(&self, target: &str, draft_id: i64) -> Result<bool> {
        let Ok(chat_id) = target.trim().parse::<i64>() else {
            log_adapter(
                "send_thinking_rich_draft_fallback",
                format!("chat={target} reason=non_numeric_private_chat"),
            );
            return Ok(false);
        };
        if chat_id <= 0 || draft_id == 0 {
            log_adapter(
                "send_thinking_rich_draft_fallback",
                format!("chat={target} draft={draft_id} reason=non_private_chat_or_invalid_draft"),
            );
            return Ok(false);
        }

        let rich_message = TelegramInputRichMessage::html("<tg-thinking>Thinking...</tg-thinking>");
        log_adapter(
            "send_thinking_rich_draft",
            format!("chat={target} draft={draft_id}"),
        );
        match self
            .api
            .send_rich_message_draft(chat_id, draft_id, &rich_message)
            .await
        {
            Ok(()) => Ok(true),
            Err(err)
                if err
                    .downcast_ref::<TelegramApiError>()
                    .is_some_and(TelegramApiError::should_fallback_from_rich_message_draft) =>
            {
                log_adapter(
                    "send_thinking_rich_draft_fallback",
                    format!("chat={target} draft={draft_id} reason=unsupported err={err}"),
                );
                Ok(false)
            }
            Err(err) => Err(err),
        }
    }

    pub async fn send_thinking_draft(&self, target: &str, draft_id: i64) -> Result<bool> {
        let Ok(chat_id) = target.trim().parse::<i64>() else {
            log_adapter(
                "send_thinking_draft_fallback",
                format!("chat={target} reason=non_numeric_private_chat"),
            );
            return Ok(false);
        };
        if chat_id <= 0 || draft_id == 0 {
            log_adapter(
                "send_thinking_draft_fallback",
                format!("chat={target} draft={draft_id} reason=non_private_chat_or_invalid_draft"),
            );
            return Ok(false);
        }

        log_adapter(
            "send_thinking_draft",
            format!("chat={target} draft={draft_id}"),
        );
        match self.api.send_message_draft(chat_id, draft_id, "").await {
            Ok(()) => Ok(true),
            Err(err)
                if err
                    .downcast_ref::<TelegramApiError>()
                    .is_some_and(TelegramApiError::should_fallback_from_message_draft) =>
            {
                log_adapter(
                    "send_thinking_draft_fallback",
                    format!("chat={target} draft={draft_id} reason=unsupported err={err}"),
                );
                Ok(false)
            }
            Err(err) => Err(err),
        }
    }

    pub async fn send_image_path(
        &self,
        target: &str,
        local_path: &Path,
        caption: Option<&str>,
    ) -> Result<String> {
        let caption_html = caption
            .map(telegram_cleanup_text)
            .map(|caption| telegram_markdown_to_html(&caption));
        log_adapter(
            "send_image_begin",
            format!(
                "chat={} path={} caption_chars={}",
                target,
                local_path.display(),
                caption.map(|value| value.chars().count()).unwrap_or(0)
            ),
        );
        match self
            .api
            .send_photo_file(
                target,
                local_path,
                caption_html.as_deref(),
                Some(TelegramParseMode::Html),
            )
            .await
        {
            Ok(message_id) => {
                log_adapter(
                    "send_image_sent",
                    format!("chat={} method=sendPhoto message={}", target, message_id),
                );
                Ok(message_id.to_string())
            }
            Err(photo_err) => match self
                .api
                .send_document_file(
                    target,
                    local_path,
                    caption_html.as_deref(),
                    Some(TelegramParseMode::Html),
                )
                .await
            {
                Ok(message_id) => {
                    log_adapter(
                        "send_image_sent",
                        format!("chat={} method=sendDocument message={}", target, message_id),
                    );
                    Ok(message_id.to_string())
                }
                Err(document_err) => {
                    log_adapter(
                        "send_image_failed",
                        format!(
                            "chat={} path={} photo_err={} document_err={}",
                            target,
                            local_path.display(),
                            photo_err,
                            document_err
                        ),
                    );
                    Err(photo_err)
                }
            },
        }
    }

    pub async fn send_approval(
        &self,
        target: &str,
        approval: &PendingApproval,
        im_text: ImText,
    ) -> Result<String> {
        let text = telegram_cleanup_text(&approval_text(approval, im_text));
        let Some(keyboard) = approval_keyboard(approval) else {
            return self.send_text(target, &text).await;
        };
        let chunks = telegram_text_chunks(&text);
        let mut last_message_id = 0;
        log_adapter(
            "send_approval_begin",
            format!(
                "chat={} request={} chars={} chunks={} decisions={}",
                target,
                approval.request_id,
                text.chars().count(),
                chunks.len(),
                approval.decisions.len()
            ),
        );
        for (index, chunk) in chunks.iter().enumerate() {
            let is_last = index + 1 == chunks.len();
            if is_last {
                let html = telegram_markdown_to_html(chunk);
                last_message_id = match self
                    .api
                    .send_text_with_reply_markup_parse_mode(
                        target,
                        &html,
                        keyboard.clone(),
                        TelegramParseMode::Html,
                    )
                    .await
                {
                    Ok(message_id) => message_id,
                    Err(err) => {
                        log_adapter(
                            "send_approval_html_failed",
                            format!(
                                "chat={} request={} chunk={}/{} fallback=plain err={}",
                                target,
                                approval.request_id,
                                index + 1,
                                chunks.len(),
                                err
                            ),
                        );
                        self.api
                            .send_text_with_reply_markup(target, chunk, keyboard.clone())
                            .await?
                    }
                };
            } else {
                let html = telegram_markdown_to_html(chunk);
                last_message_id = match self
                    .api
                    .send_text_parse_mode(target, &html, TelegramParseMode::Html)
                    .await
                {
                    Ok(message_id) => message_id,
                    Err(err) => {
                        log_adapter(
                            "send_approval_html_failed",
                            format!(
                                "chat={} request={} chunk={}/{} fallback=plain err={}",
                                target,
                                approval.request_id,
                                index + 1,
                                chunks.len(),
                                err
                            ),
                        );
                        self.api.send_text(target, chunk).await?
                    }
                };
                sleep(Duration::from_millis(TELEGRAM_CHUNK_DELAY_MS)).await;
            }
        }
        log_adapter(
            "send_approval_done",
            format!(
                "chat={} request={} chunks={} message={}",
                target,
                approval.request_id,
                chunks.len(),
                last_message_id
            ),
        );
        Ok(last_message_id.to_string())
    }

    pub async fn answer_callback_query(&self, callback_query_id: &str, text: &str) -> Result<()> {
        log_adapter(
            "answer_callback_begin",
            format!(
                "callback_query={} text_len={}",
                callback_query_id,
                text.chars().count()
            ),
        );
        self.api
            .answer_callback_query(callback_query_id, Some(text))
            .await?;
        log_adapter(
            "answer_callback_done",
            format!("callback_query={}", callback_query_id),
        );
        Ok(())
    }

    /// Update an existing Telegram message and remove its inline keyboard.
    /// Returns `false` when no usable message id was supplied or Telegram no
    /// longer allows editing that message.
    pub async fn update_resolved_approval(
        &self,
        target: &str,
        message_id: Option<&str>,
        approval: &PendingApproval,
        option_index: usize,
        decision_label: &str,
        text: ImText,
    ) -> Result<bool> {
        let Some(message_id) = message_id else {
            return Ok(false);
        };
        let resolved = resolved_approval_text(approval, option_index, decision_label, text);
        let resolved_html = telegram_markdown_to_html(&resolved);
        match self
            .try_edit_message_text(
                target,
                message_id,
                &resolved_html,
                Some(TelegramParseMode::Html),
                Some(empty_inline_keyboard()),
            )
            .await
        {
            Ok(Some(_)) => Ok(true),
            Ok(None) => {
                let _ = self.clear_reply_markup(target, Some(message_id)).await;
                Ok(false)
            }
            Err(err) => {
                let _ = self.clear_reply_markup(target, Some(message_id)).await;
                Err(err)
            }
        }
    }

    pub async fn clear_reply_markup(&self, target: &str, message_id: Option<&str>) -> Result<bool> {
        let Some(message_id) = message_id else {
            return Ok(false);
        };
        let Ok(message_id_number) = message_id.trim().parse::<i64>() else {
            return Ok(false);
        };
        match self
            .api
            .edit_message_reply_markup(target, message_id_number, empty_inline_keyboard())
            .await
        {
            Ok(_) => Ok(true),
            Err(err) => {
                let api_error = err.downcast_ref::<TelegramApiError>();
                if api_error.is_some_and(TelegramApiError::is_message_not_modified) {
                    return Ok(true);
                }
                if api_error.is_some_and(TelegramApiError::is_edit_target_unavailable) {
                    return Ok(false);
                }
                Err(err).with_context(|| {
                    format!(
                        "failed to clear telegram message {} reply markup in chat {}",
                        message_id, target
                    )
                })
            }
        }
    }

    pub async fn send_or_update_text_with_reply_markup(
        &self,
        target: &str,
        message_id: Option<&str>,
        text: &str,
        reply_markup: serde_json::Value,
    ) -> Result<String> {
        let text = telegram_cleanup_text(text);
        let updated = self
            .try_edit_message_text(
                target,
                message_id.unwrap_or_default(),
                &text,
                None,
                Some(reply_markup.clone()),
            )
            .await?;
        if let Some(message_id) = updated {
            return Ok(message_id.to_string());
        }
        let _ = self.clear_reply_markup(target, message_id).await;
        Ok(self
            .api
            .send_text_with_reply_markup(target, &text, reply_markup)
            .await?
            .to_string())
    }

    pub async fn send_or_update_text_with_reply_markup_parse_mode(
        &self,
        target: &str,
        message_id: Option<&str>,
        text: &str,
        reply_markup: serde_json::Value,
        parse_mode: TelegramParseMode,
    ) -> Result<String> {
        let text = telegram_cleanup_text(text);
        let updated = self
            .try_edit_message_text(
                target,
                message_id.unwrap_or_default(),
                &text,
                Some(parse_mode),
                Some(reply_markup.clone()),
            )
            .await?;
        if let Some(message_id) = updated {
            return Ok(message_id.to_string());
        }
        let _ = self.clear_reply_markup(target, message_id).await;
        Ok(self
            .api
            .send_text_with_reply_markup_parse_mode(target, &text, reply_markup, parse_mode)
            .await?
            .to_string())
    }

    pub async fn send_or_update_text(
        &self,
        target: &str,
        message_id: Option<&str>,
        text: &str,
    ) -> Result<String> {
        let text = telegram_cleanup_text(text);
        let updated = self
            .try_edit_message_text(
                target,
                message_id.unwrap_or_default(),
                &telegram_markdown_to_html(&text),
                Some(TelegramParseMode::Html),
                Some(empty_inline_keyboard()),
            )
            .await?;
        if let Some(message_id) = updated {
            return Ok(message_id.to_string());
        }
        let _ = self.clear_reply_markup(target, message_id).await;
        self.send_text(target, &text).await
    }

    pub async fn send_or_update_rich_markdown(
        &self,
        target: &str,
        message_id: Option<&str>,
        markdown: &str,
    ) -> Result<String> {
        let markdown = telegram_cleanup_text(markdown);
        let rich_message = TelegramInputRichMessage::markdown(markdown.clone());
        self.send_or_update_rich_message(target, message_id, &rich_message, &markdown)
            .await
    }

    pub async fn send_or_update_rich_markdown_with_fallback(
        &self,
        target: &str,
        message_id: Option<&str>,
        markdown: &str,
        fallback_markdown: &str,
    ) -> Result<String> {
        let markdown = telegram_cleanup_text(markdown);
        let fallback_markdown = telegram_cleanup_text(fallback_markdown);
        let rich_message = TelegramInputRichMessage::markdown(markdown);
        self.send_or_update_rich_message(target, message_id, &rich_message, &fallback_markdown)
            .await
    }

    pub async fn send_or_update_rich_blocks(
        &self,
        target: &str,
        message_id: Option<&str>,
        blocks: Vec<serde_json::Value>,
        fallback_markdown: &str,
    ) -> Result<String> {
        let fallback_markdown = telegram_cleanup_text(fallback_markdown);
        let rich_message = TelegramInputRichMessage::blocks(blocks);
        self.send_or_update_rich_message(target, message_id, &rich_message, &fallback_markdown)
            .await
    }

    async fn send_or_update_rich_message(
        &self,
        target: &str,
        message_id: Option<&str>,
        rich_message: &TelegramInputRichMessage,
        fallback_markdown: &str,
    ) -> Result<String> {
        match self
            .try_edit_rich_message(
                target,
                message_id.unwrap_or_default(),
                rich_message,
                Some(empty_inline_keyboard()),
            )
            .await
        {
            Ok(Some(updated_id)) => return Ok(updated_id.to_string()),
            Ok(None) => {}
            Err(err) if should_fallback_from_rich_message(&err) => {
                log_adapter(
                    "edit_rich_message_fallback",
                    format!(
                        "chat={} message={} fallback=text err={}",
                        target,
                        message_id.unwrap_or_default(),
                        err
                    ),
                );
                return self
                    .send_or_update_text(target, message_id, fallback_markdown)
                    .await;
            }
            Err(err) => return Err(err),
        }

        match self.api.send_rich_message(target, &rich_message).await {
            Ok(message_id) => {
                log_adapter(
                    "send_rich_message_done",
                    format!("chat={} message={}", target, message_id),
                );
                Ok(message_id.to_string())
            }
            Err(err) if should_fallback_from_rich_message(&err) => {
                log_adapter(
                    "send_rich_message_fallback",
                    format!("chat={} fallback=text err={}", target, err),
                );
                self.send_or_update_text(target, message_id, fallback_markdown)
                    .await
            }
            Err(err) => Err(err)
                .with_context(|| format!("failed to send telegram rich message in chat {target}")),
        }
    }

    async fn try_edit_rich_message(
        &self,
        target: &str,
        message_id: &str,
        rich_message: &TelegramInputRichMessage,
        reply_markup: Option<serde_json::Value>,
    ) -> Result<Option<i64>> {
        let Ok(message_id_number) = message_id.trim().parse::<i64>() else {
            if !message_id.trim().is_empty() {
                log_adapter(
                    "edit_rich_message_invalid_id",
                    format!("chat={} message={}", target, message_id),
                );
            }
            return Ok(None);
        };
        match self
            .api
            .edit_rich_message(target, message_id_number, rich_message, reply_markup)
            .await
        {
            Ok(updated_id) => {
                log_adapter(
                    "edit_rich_message_done",
                    format!("chat={} message={}", target, updated_id),
                );
                Ok(Some(updated_id))
            }
            Err(err) => {
                let api_error = err.downcast_ref::<TelegramApiError>();
                if api_error.is_some_and(TelegramApiError::is_message_not_modified) {
                    return Ok(Some(message_id_number));
                }
                if api_error.is_some_and(TelegramApiError::is_edit_target_unavailable) {
                    log_adapter(
                        "edit_rich_message_unavailable",
                        format!(
                            "chat={} message={} fallback=send err={}",
                            target, message_id, err
                        ),
                    );
                    return Ok(None);
                }
                Err(err).with_context(|| {
                    format!(
                        "failed to edit telegram rich message {} in chat {}",
                        message_id, target
                    )
                })
            }
        }
    }

    async fn try_edit_message_text(
        &self,
        target: &str,
        message_id: &str,
        text: &str,
        parse_mode: Option<TelegramParseMode>,
        reply_markup: Option<serde_json::Value>,
    ) -> Result<Option<i64>> {
        let Ok(message_id_number) = message_id.trim().parse::<i64>() else {
            if !message_id.trim().is_empty() {
                log_adapter(
                    "edit_message_invalid_id",
                    format!("chat={} message={}", target, message_id),
                );
            }
            return Ok(None);
        };
        match self
            .api
            .edit_message_text(target, message_id_number, text, parse_mode, reply_markup)
            .await
        {
            Ok(updated_id) => {
                log_adapter(
                    "edit_message_done",
                    format!("chat={} message={}", target, updated_id),
                );
                Ok(Some(updated_id))
            }
            Err(err) => {
                let unavailable = err
                    .downcast_ref::<TelegramApiError>()
                    .is_some_and(TelegramApiError::is_edit_target_unavailable);
                let unchanged = err
                    .downcast_ref::<TelegramApiError>()
                    .is_some_and(TelegramApiError::is_message_not_modified);
                if unchanged {
                    return Ok(Some(message_id_number));
                }
                if unavailable {
                    log_adapter(
                        "edit_message_unavailable",
                        format!(
                            "chat={} message={} fallback=send err={}",
                            target, message_id, err
                        ),
                    );
                    return Ok(None);
                }
                Err(err).with_context(|| {
                    format!(
                        "failed to edit telegram message {} in chat {}",
                        message_id, target
                    )
                })
            }
        }
    }

    pub async fn send_thread_routing_choice(
        &self,
        target: &str,
        request_id: &str,
        message_id: Option<&str>,
        text: ImText,
    ) -> Result<String> {
        let keyboard = inline_keyboard(vec![
            vec![button(
                text.create_new_session_button(),
                &format!("trc:{request_id}:new"),
            )],
            vec![button(
                text.restore_history_button(),
                &format!("trc:{request_id}:load"),
            )],
        ]);
        let body = text.create_choice_telegram();
        log_adapter(
            "send_thread_routing_choice_begin",
            format!("chat={} request={}", target, request_id),
        );
        let message_id = self
            .send_or_update_text_with_reply_markup(target, message_id, body, keyboard)
            .await?;
        log_adapter(
            "send_thread_routing_choice_done",
            format!(
                "chat={} request={} message={}",
                target, request_id, message_id
            ),
        );
        Ok(message_id.to_string())
    }

    pub async fn send_thread_create_settings(
        &self,
        target: &str,
        request_id: &str,
        text: &str,
        message_id: Option<&str>,
        im_text: ImText,
    ) -> Result<String> {
        let keyboard = inline_keyboard(vec![
            vec![
                button(im_text.directory_button(), &format!("tce:{request_id}:cwd")),
                button(im_text.model_button(), &format!("tce:{request_id}:model")),
            ],
            vec![
                button(im_text.effort_button(), &format!("tce:{request_id}:effort")),
                button(
                    im_text.permission_button(),
                    &format!("tce:{request_id}:perm"),
                ),
            ],
            vec![button(
                im_text.create_button(),
                &format!("tcc:{request_id}"),
            )],
            vec![button(
                im_text.restore_history_button(),
                &format!("trc:{request_id}:load"),
            )],
        ]);
        log_adapter(
            "send_thread_create_settings_begin",
            format!(
                "chat={} request={} text_len={}",
                target,
                request_id,
                text.chars().count()
            ),
        );
        let message_id = self
            .send_or_update_text_with_reply_markup(target, message_id, text, keyboard)
            .await?;
        log_adapter(
            "send_thread_create_settings_done",
            format!(
                "chat={} request={} message={}",
                target, request_id, message_id
            ),
        );
        Ok(message_id.to_string())
    }

    pub async fn send_thread_create_options(
        &self,
        target: &str,
        request_id: &str,
        field: &str,
        title: &str,
        body: &str,
        options: &[ThreadCreateOption],
        page: usize,
        has_prev: bool,
        has_next: bool,
        message_id: Option<&str>,
        text: ImText,
    ) -> Result<String> {
        let mut rows = Vec::new();
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

        let options_html = create_options_table_html(options);
        let text_html =
            create_options_html_text(title, body, page, options.len(), &options_html, text);
        log_adapter(
            "send_thread_create_options_begin",
            format!(
                "chat={} request={} field={} page={} options={} text_len={}",
                target,
                request_id,
                field,
                page,
                options.len(),
                text_html.chars().count()
            ),
        );
        let message_id = self
            .send_or_update_text_with_reply_markup_parse_mode(
                target,
                message_id,
                &text_html,
                inline_keyboard(rows),
                TelegramParseMode::Html,
            )
            .await?;
        log_adapter(
            "send_thread_create_options_done",
            format!(
                "chat={} request={} field={} page={} message={}",
                target, request_id, field, page, message_id
            ),
        );
        Ok(message_id.to_string())
    }

    pub async fn send_thread_list(
        &self,
        target: &str,
        request_id: &str,
        title: &str,
        body: &str,
        entries: &[TelegramThreadListEntry],
        page: usize,
        has_prev: bool,
        has_next: bool,
        message_id: Option<&str>,
        text: ImText,
    ) -> Result<String> {
        let mut rows = Vec::new();
        let mut nav = Vec::new();
        if has_prev {
            nav.push(button(
                text.previous_page_button(),
                &format!("tlp:{request_id}:prev"),
            ));
        }
        if has_next {
            nav.push(button(
                text.next_page_button(),
                &format!("tlp:{request_id}:next"),
            ));
        }
        if !nav.is_empty() {
            rows.push(nav);
        }
        rows.push(vec![button(
            text.create_new_session_button(),
            &format!("trc:{request_id}:new"),
        )]);

        let entries_html = if entries.is_empty() {
            telegram_html_escape(text.no_restorable_history())
        } else {
            thread_entries_table_html(entries, text)
        };
        let text_html = thread_list_html_text(title, body, page, &entries_html, text);
        log_adapter(
            "send_thread_list_begin",
            format!(
                "chat={} request={} page={} entries={} text_len={}",
                target,
                request_id,
                page,
                entries.len(),
                text_html.chars().count()
            ),
        );
        let message_id = self
            .send_or_update_text_with_reply_markup_parse_mode(
                target,
                message_id,
                &text_html,
                inline_keyboard(rows),
                TelegramParseMode::Html,
            )
            .await?;
        log_adapter(
            "send_thread_list_done",
            format!(
                "chat={} request={} page={} entries={} message={}",
                target,
                request_id,
                page,
                entries.len(),
                message_id
            ),
        );
        Ok(message_id.to_string())
    }

    pub async fn send_thread_routing_result(
        &self,
        target: &str,
        title: &str,
        body: &str,
        message_id: Option<&str>,
    ) -> Result<String> {
        self.send_or_update_text(target, message_id, &format!("{title}\n\n{body}"))
            .await
    }
}

fn approval_text(approval: &PendingApproval, text: ImText) -> String {
    let mut lines = vec![
        text.approval_request_heading().to_string(),
        format!("request_kind: `{}`", approval.request_kind),
        String::new(),
        approval.summary.trim().to_string(),
        String::new(),
        format!("{}:", text.available_decisions_label()),
    ];
    if approval.decisions.is_empty() {
        lines.push("/y".to_string());
        lines.push("/n".to_string());
    } else {
        lines.extend(
            approval
                .decisions
                .iter()
                .enumerate()
                .map(|(index, decision)| format!("/{} {}", index + 1, decision.label)),
        );
    }
    lines.push(String::new());
    lines.push(text.approval_reply_footer(&text.approval_reply_hint(approval)));
    lines.join("\n")
}

fn resolved_approval_text(
    approval: &PendingApproval,
    option_index: usize,
    decision_label: &str,
    text: ImText,
) -> String {
    vec![
        text.approval_resolved_title().to_string(),
        format!(
            "{}: `{}`",
            text.approval_request_heading(),
            approval.request_kind
        ),
        String::new(),
        approval.summary.trim().to_string(),
        String::new(),
        text.approval_selected_label(option_index, decision_label.trim()),
    ]
    .join("\n")
}

fn approval_keyboard(approval: &PendingApproval) -> Option<serde_json::Value> {
    let fingerprint = approval_request_fingerprint(&approval.request_key());
    let rows = approval
        .decisions
        .iter()
        .enumerate()
        .map(|(index, decision)| {
            vec![approval_button(
                &decision.label,
                &format!("ap:{fingerprint}:{}", index + 1),
            )]
        })
        .collect::<Vec<_>>();
    (!rows.is_empty()).then(|| inline_keyboard(rows))
}

fn inline_keyboard(rows: Vec<Vec<serde_json::Value>>) -> serde_json::Value {
    json!({ "inline_keyboard": rows })
}

fn empty_inline_keyboard() -> serde_json::Value {
    inline_keyboard(Vec::new())
}

fn should_fallback_from_rich_message(err: &anyhow::Error) -> bool {
    err.downcast_ref::<TelegramApiError>()
        .is_some_and(TelegramApiError::should_fallback_from_rich_message)
}

fn log_adapter(event: &str, message: impl AsRef<str>) {
    chain_log::write_diagnostic_lazy(|| {
        format!("[telegram_adapter] event={} {}", event, message.as_ref())
    });
}

fn log_text_preview(text: &str, limit: usize) -> String {
    let compact = text.replace("\r\n", "\n").replace('\n', "\\n");
    let mut out = String::new();
    for ch in compact.chars().take(limit) {
        out.push(ch);
    }
    if compact.chars().count() > limit {
        out.push_str("...");
    }
    out
}

fn button(text: &str, callback_data: &str) -> serde_json::Value {
    json!({
        "text": truncate_button_text(text),
        "callback_data": callback_data,
    })
}

fn approval_button(text: &str, callback_data: &str) -> serde_json::Value {
    json!({
        "text": text.trim(),
        "callback_data": callback_data,
    })
}

fn thread_entries_table_html(entries: &[TelegramThreadListEntry], text: ImText) -> String {
    let mut lines = Vec::new();
    let mut current_cwd: Option<&str> = None;
    for (index, entry) in entries.iter().enumerate() {
        let cwd = entry
            .cwd
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if current_cwd != cwd {
            if !lines.is_empty() {
                lines.push(String::new());
            }
            lines.push(project_header_html(cwd, text));
            current_cwd = cwd;
        }
        lines.push(thread_entry_table_html(index, entry, text));
    }
    lines.join("\n")
}

fn create_options_table_html(options: &[ThreadCreateOption]) -> String {
    let mut lines = Vec::new();
    for (index, option) in options.iter().enumerate() {
        lines.push(create_option_row_html(index, option));
        lines.push(String::new());
    }
    lines.join("\n")
}

fn create_option_row_html(index: usize, option: &ThreadCreateOption) -> String {
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

fn option_summary_html(summary: &str) -> String {
    let summary = truncate_middle(summary, 56);
    if looks_like_path(&summary) {
        format!("<code>{}</code>", telegram_html_escape(&summary))
    } else {
        telegram_html_escape(&summary)
    }
}

fn create_options_html_text(
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

fn thread_entry_table_html(index: usize, entry: &TelegramThreadListEntry, text: ImText) -> String {
    let title = entry.title.trim();
    let title = if title.is_empty() {
        text.untitled_session()
    } else {
        title
    };
    let title = truncate_display_text(title, 22);
    let state = thread_state_suffix(&entry.state, text)
        .map(|state| format!(" <code>{}</code>", telegram_html_escape(state)))
        .unwrap_or_default();
    format!(
        "/{} <b>{}</b>{state}",
        index + 1,
        telegram_html_escape(&title)
    )
}

fn project_header_html(cwd: Option<&str>, text: ImText) -> String {
    match cwd {
        Some(cwd) => {
            let name = project_name(cwd);
            format!(
                "<b>{}</b>\n<code>{}</code>",
                telegram_html_escape(&truncate_display_text(&text.project_header(&name), 32)),
                telegram_html_escape(&truncate_middle(cwd, 68))
            )
        }
        None => format!(
            "<b>{}</b>",
            telegram_html_escape(text.unknown_project_header())
        ),
    }
}

fn thread_state_suffix(state: &str, text: ImText) -> Option<&'static str> {
    if state.contains("当前会话") || state.contains("Current session") {
        Some(text.current_short())
    } else if state.contains("已加载") || state.contains("Loaded") {
        Some(text.loaded_short())
    } else {
        None
    }
}

fn thread_list_html_text(
    title: &str,
    body: &str,
    page: usize,
    entries_html: &str,
    text: ImText,
) -> String {
    format!(
        "<b>{}</b>\n{}\n\n{}\n\n<code>{}</code>",
        telegram_html_escape(title),
        telegram_markdown_to_html(&telegram_cleanup_text(body)),
        entries_html,
        text.page_label(page)
    )
}

fn truncate_display_text(text: &str, max_chars: usize) -> String {
    let text = text
        .replace('\r', " ")
        .replace('\n', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if text.chars().count() <= max_chars {
        return text;
    }
    let mut output = text
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    output.push('…');
    output
}

fn project_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|value| value.to_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| path.to_string())
}

fn looks_like_path(value: &str) -> bool {
    let value = value.trim();
    value.contains('\\') || value.contains('/') || value.starts_with('~')
}

fn truncate_middle(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let head_len = max_chars.saturating_sub(3) / 2;
    let tail_len = max_chars.saturating_sub(3 + head_len);
    let head = text.chars().take(head_len).collect::<String>();
    let tail = text
        .chars()
        .rev()
        .take(tail_len)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("{head}...{tail}")
}

fn telegram_markdown_to_html(text: &str) -> String {
    let text = telegram_cleanup_text(text);
    let mut html = String::new();
    let mut in_code_block = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            if in_code_block {
                html.push_str("</code></pre>\n");
            } else {
                html.push_str("<pre><code>");
            }
            in_code_block = !in_code_block;
            continue;
        }
        if in_code_block {
            html.push_str(&telegram_html_escape(line));
            html.push('\n');
        } else {
            html.push_str(&telegram_inline_markdown_to_html(line));
            html.push('\n');
        }
    }
    if in_code_block {
        html.push_str("</code></pre>");
    }
    html.trim_end().to_string()
}

fn telegram_inline_markdown_to_html(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("**")
            && let Some(end) = after.find("**")
        {
            out.push_str("<b>");
            out.push_str(&telegram_html_escape(&after[..end]));
            out.push_str("</b>");
            rest = &after[end + 2..];
            continue;
        }
        if let Some(after) = rest.strip_prefix('`')
            && let Some(end) = after.find('`')
        {
            out.push_str("<code>");
            out.push_str(&telegram_html_escape(&after[..end]));
            out.push_str("</code>");
            rest = &after[end + 1..];
            continue;
        }
        if let Some(after_label) = rest.strip_prefix('[')
            && let Some(label_end) = after_label.find("](")
            && let Some(url_end) = after_label[label_end + 2..].find(')')
        {
            let label = &after_label[..label_end];
            let url = &after_label[label_end + 2..label_end + 2 + url_end];
            if url.starts_with("http://") || url.starts_with("https://") {
                out.push_str("<a href=\"");
                out.push_str(&telegram_html_attr_escape(url));
                out.push_str("\">");
                out.push_str(&telegram_html_escape(label));
                out.push_str("</a>");
            } else {
                out.push_str(&telegram_html_escape(label));
            }
            rest = &after_label[label_end + 2 + url_end + 1..];
            continue;
        }
        let ch = rest.chars().next().expect("rest is non-empty");
        out.push_str(&telegram_html_escape(&ch.to_string()));
        rest = &rest[ch.len_utf8()..];
    }
    out
}

fn telegram_cleanup_text(text: &str) -> String {
    strip_codex_ui_directives(text)
        .replace("<font color='grey'>", "")
        .replace("<font color=\"grey\">", "")
        .replace("</font>", "")
}

fn strip_codex_ui_directives(text: &str) -> String {
    let mut in_fenced_code = false;
    let mut removed_any = false;
    let mut lines = Vec::new();

    for raw_line in text.lines() {
        let line = raw_line.trim_end_matches('\r');
        if !in_fenced_code && is_codex_ui_directive_line(line) {
            removed_any = true;
            continue;
        }
        lines.push(line);
        if is_markdown_fence_line(line) {
            in_fenced_code = !in_fenced_code;
        }
    }

    if !removed_any {
        return text.to_string();
    }

    let mut output = String::new();
    for line in lines {
        let is_blank = line.trim().is_empty();
        if is_blank && output.ends_with('\n') {
            continue;
        }
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str(line);
    }
    output.trim().to_string()
}

fn is_codex_ui_directive_line(line: &str) -> bool {
    let line = line.trim();
    let Some(rest) = line.strip_prefix("::") else {
        return false;
    };
    let Some(open_brace) = rest.find('{') else {
        return false;
    };
    let name = &rest[..open_brace];
    let arguments = &rest[open_brace + 1..];
    arguments.ends_with('}')
        && matches!(
            name,
            "code-comment"
                | "git-commit"
                | "git-create-branch"
                | "git-create-pr"
                | "git-push"
                | "git-stage"
        )
}

fn is_markdown_fence_line(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with("```") || line.starts_with("~~~")
}

fn telegram_turn_completed_messages(reply_text: &str, footer_text: &str) -> (String, String) {
    let reply_text = telegram_cleanup_text(reply_text).trim().to_string();
    let footer_text = footer_text.trim();
    if footer_text.is_empty() {
        return (reply_text.clone(), reply_text);
    }
    let rich_footer = telegram_html_escape(footer_text);
    if reply_text.is_empty() {
        return (
            format!("<footer>{rich_footer}</footer>"),
            footer_text.to_string(),
        );
    }
    (
        format!("{reply_text}\n\n<footer>{rich_footer}</footer>"),
        format!("{reply_text}\n\n{footer_text}"),
    )
}

fn telegram_user_message_messages(message_text: &str, credit_text: &str) -> (String, String) {
    let message_text = telegram_cleanup_text(message_text).trim().to_string();
    let credit_text = credit_text.trim();
    let rich_body = telegram_markdown_to_html(&message_text);
    let rich_credit = telegram_html_escape(credit_text);
    let rich_html = format!("<blockquote>{rich_body}\n<cite>{rich_credit}</cite></blockquote>");
    let fallback_markdown = if credit_text.is_empty() {
        message_text
    } else if message_text.is_empty() {
        credit_text.to_string()
    } else {
        format!("{message_text}\n\n{credit_text}")
    };
    (rich_html, fallback_markdown)
}

fn telegram_context_compaction_messages(title_text: &str, credit_text: &str) -> (String, String) {
    let title_text = title_text.trim();
    let credit_text = credit_text.trim();
    let rich_title = telegram_html_escape(title_text);
    let rich_credit = telegram_html_escape(credit_text);
    let rich_html = if credit_text.is_empty() {
        format!("<aside>{rich_title}</aside>")
    } else {
        format!("<aside>{rich_title}<cite>{rich_credit}</cite></aside>")
    };
    let fallback_text = if credit_text.is_empty() {
        title_text.to_string()
    } else if title_text.is_empty() {
        credit_text.to_string()
    } else {
        format!("{title_text}\n\n{credit_text}")
    };
    (rich_html, fallback_text)
}

fn telegram_html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn telegram_html_attr_escape(text: &str) -> String {
    telegram_html_escape(text).replace('"', "&quot;")
}

fn truncate_button_text(text: &str) -> String {
    const MAX: usize = 48;
    let text = text.trim();
    if text.chars().count() <= MAX {
        return text.to_string();
    }
    let mut output = text.chars().take(MAX.saturating_sub(1)).collect::<String>();
    output.push('…');
    output
}

fn telegram_text_chunks(text: &str) -> Vec<String> {
    telegram_text_chunks_with_limit(text, TELEGRAM_MAX_MESSAGE_CHARS)
}

fn telegram_turn_completed_chunks(text: &str, footer_text: &str) -> Vec<String> {
    let footer_chars = footer_text.trim().chars().count();
    let reserved_chars = if footer_chars == 0 {
        0
    } else {
        footer_chars.saturating_add(2)
    };
    let max_chars = TELEGRAM_MAX_MESSAGE_CHARS
        .saturating_sub(reserved_chars)
        .max(TELEGRAM_CONTINUATION_OVERHEAD + 1);
    telegram_text_chunks_with_limit(text, max_chars)
}

fn telegram_user_message_chunks(text: &str, credit_text: &str) -> Vec<String> {
    let reserved_chars = credit_text.trim().chars().count().saturating_add(2);
    let max_chars = TELEGRAM_MAX_MESSAGE_CHARS
        .saturating_sub(reserved_chars)
        .max(TELEGRAM_CONTINUATION_OVERHEAD + 1);
    telegram_text_chunks_with_limit(text, max_chars)
}

fn telegram_text_chunks_with_limit(text: &str, max_chars: usize) -> Vec<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return vec![" ".to_string()];
    }
    if trimmed.chars().count() <= max_chars {
        return vec![trimmed.to_string()];
    }

    let chunks = split_message_for_telegram(trimmed, max_chars);
    let chunk_count = chunks.len();
    chunks
        .into_iter()
        .enumerate()
        .map(|(index, chunk)| {
            if index == 0 {
                format!("{chunk}\n\n(continues...)")
            } else if index + 1 == chunk_count {
                format!("(continued)\n\n{chunk}")
            } else {
                format!("(continued)\n\n{chunk}\n\n(continues...)")
            }
        })
        .collect()
}

fn split_message_for_telegram(message: &str, max_chars: usize) -> Vec<String> {
    let content_limit = max_chars.saturating_sub(TELEGRAM_CONTINUATION_OVERHEAD);

    let mut chunks = Vec::new();
    let mut remaining = message;
    while !remaining.is_empty() {
        if remaining.chars().count() <= content_limit {
            chunks.push(remaining.to_string());
            break;
        }

        let hard_split = remaining
            .char_indices()
            .nth(content_limit)
            .map_or(remaining.len(), |(idx, _)| idx);
        let search_area = &remaining[..hard_split];
        let chunk_end = best_split_point(search_area, hard_split, content_limit);

        chunks.push(remaining[..chunk_end].trim_end().to_string());
        remaining = remaining[chunk_end..].trim_start();
    }
    chunks
}

fn best_split_point(search_area: &str, hard_split: usize, content_limit: usize) -> usize {
    if let Some(pos) = search_area.rfind('\n')
        && search_area[..pos].chars().count() >= content_limit / 2
    {
        return pos + 1;
    }
    if let Some(pos) = search_area.rfind(' ')
        && search_area[..pos].chars().count() >= content_limit / 2
    {
        return pos + 1;
    }
    hard_split
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::{
        im::core::i18n::ImText,
        im::telegram::{api::TelegramApi, types::TelegramSettings},
        im_runtime::{ApprovalDecisionOption, PendingApproval},
    };

    use super::{
        TELEGRAM_MAX_MESSAGE_CHARS, TelegramAdapter, empty_inline_keyboard, resolved_approval_text,
        telegram_cleanup_text, telegram_context_compaction_messages, telegram_text_chunks,
        telegram_turn_completed_chunks, telegram_turn_completed_messages,
        telegram_user_message_chunks, telegram_user_message_messages,
    };

    #[tokio::test]
    async fn thinking_draft_skips_targets_that_cannot_be_private_chats() {
        let adapter = TelegramAdapter::new(TelegramApi::new(TelegramSettings::default()));

        assert!(!adapter.send_thinking_draft("-1001", 7).await.unwrap());
        assert!(!adapter.send_thinking_draft("@channel", 7).await.unwrap());
        assert!(!adapter.send_thinking_draft("42", 0).await.unwrap());
    }

    #[test]
    fn chunks_long_text_on_char_boundaries() {
        let chunks = telegram_text_chunks(&"你好世界".repeat(1100));

        assert!(chunks.len() > 1);
        assert!(
            chunks
                .iter()
                .all(|chunk| chunk.chars().count() <= TELEGRAM_MAX_MESSAGE_CHARS)
        );
        assert!(chunks[0].ends_with("(continues...)"));
        assert!(chunks[1].starts_with("(continued)"));
    }

    #[test]
    fn keeps_single_message_when_within_limit() {
        let text = "hello";
        let chunks = telegram_text_chunks(text);

        assert_eq!(chunks, vec!["hello"]);
    }

    #[test]
    fn empty_message_uses_space_placeholder() {
        let chunks = telegram_text_chunks("  \n ");

        assert_eq!(chunks, vec![" "]);
    }

    #[test]
    fn telegram_cleanup_removes_codex_ui_directives_from_standalone_lines() {
        let text = concat!(
            "提交完成\n\n",
            "::git-stage{cwd=\"/tmp/codexhub\"}\n",
            "::git-commit{cwd=\"/tmp/codexhub\"}\n\n",
            "下一行"
        );

        let cleaned = telegram_cleanup_text(text);

        assert_eq!(cleaned, "提交完成\n\n下一行");
        assert!(!cleaned.contains("::git-stage"));
        assert!(!cleaned.contains("::git-commit"));
    }

    #[test]
    fn telegram_cleanup_preserves_code_examples_and_unknown_directives() {
        let text = concat!(
            "```text\n",
            "::git-commit{cwd=\"/tmp/codexhub\"}\n",
            "```\n",
            "::custom{value=\"keep\"}\n",
            "::git-not-a-real-directive{value=\"keep\"}"
        );

        let cleaned = telegram_cleanup_text(text);

        assert!(cleaned.contains("```text\n::git-commit{cwd=\"/tmp/codexhub\"}\n```"));
        assert!(cleaned.contains("::custom{value=\"keep\"}"));
        assert!(cleaned.contains("::git-not-a-real-directive{value=\"keep\"}"));
    }

    #[test]
    fn prefers_newline_split_for_long_text() {
        let first = "a".repeat(3000);
        let second = "b".repeat(3000);
        let chunks = telegram_text_chunks(&format!("{first}\n{second}"));

        assert!(chunks[0].contains("(continues...)"));
        assert!(chunks[0].contains('\n'));
        assert!(chunks[0].trim_start().starts_with('a'));
        assert!(chunks[1].contains('b'));
    }

    #[test]
    fn turn_completed_message_uses_a_native_footer_with_plain_fallback() {
        let (rich, fallback) =
            telegram_turn_completed_messages("🤖 Codex\n\n**Build:** `323`", "已完成");

        assert_eq!(
            rich,
            "🤖 Codex\n\n**Build:** `323`\n\n<footer>已完成</footer>"
        );
        assert_eq!(fallback, "🤖 Codex\n\n**Build:** `323`\n\n已完成");
    }

    #[test]
    fn turn_completed_footer_escapes_html_and_handles_an_empty_reply() {
        let (rich, fallback) = telegram_turn_completed_messages("  ", "Done & closed");

        assert_eq!(rich, "<footer>Done &amp; closed</footer>");
        assert_eq!(fallback, "Done & closed");
    }

    #[test]
    fn turn_completed_chunks_reserve_space_for_the_fallback_footer() {
        for text in ["a".repeat(TELEGRAM_MAX_MESSAGE_CHARS), "界".repeat(4_500)] {
            let chunks = telegram_turn_completed_chunks(&text, "已完成");
            assert!(chunks.len() > 1);
            assert!(
                chunks[..chunks.len() - 1]
                    .iter()
                    .all(|chunk| chunk.chars().count() <= TELEGRAM_MAX_MESSAGE_CHARS)
            );
            assert!(
                chunks[..chunks.len() - 1]
                    .iter()
                    .all(|chunk| !chunk.contains("已完成"))
            );
            let (rich, fallback) =
                telegram_turn_completed_messages(chunks.last().expect("final chunk"), "已完成");
            assert!(fallback.chars().count() <= TELEGRAM_MAX_MESSAGE_CHARS);
            assert_eq!(rich.matches("<footer>").count(), 1);
            assert_eq!(rich.matches("已完成").count(), 1);
            assert!(!rich.contains("<hr"));
        }
    }

    #[test]
    fn context_compaction_uses_a_native_aside_with_credit_and_plain_fallback() {
        let (rich, fallback) =
            telegram_context_compaction_messages("上下文 <已> & 压缩", "任务 & 继续");

        assert_eq!(
            rich,
            "<aside>上下文 &lt;已&gt; &amp; 压缩<cite>任务 &amp; 继续</cite></aside>"
        );
        assert_eq!(fallback, "上下文 <已> & 压缩\n\n任务 & 继续");
    }

    #[test]
    fn user_message_uses_a_native_quote_with_credit_and_plain_fallback() {
        let (rich, fallback) = telegram_user_message_messages(
            "**打开** `<draft>` & [文档](https://example.com)",
            "你 & 我 · Codex 电脑端",
        );

        assert_eq!(
            rich,
            "<blockquote><b>打开</b> <code>&lt;draft&gt;</code> &amp; <a href=\"https://example.com\">文档</a>\n<cite>你 &amp; 我 · Codex 电脑端</cite></blockquote>"
        );
        assert_eq!(
            fallback,
            "**打开** `<draft>` & [文档](https://example.com)\n\n你 & 我 · Codex 电脑端"
        );
        assert!(!fallback.contains('🤖'));
        assert!(!fallback.contains('👤'));
    }

    #[test]
    fn long_user_message_chunks_preserve_text_and_credit_each_quote() {
        let source = "界".repeat(4_500);
        let credit = "你 · Codex 电脑端";
        let chunks = telegram_user_message_chunks(&source, credit);

        assert!(chunks.len() > 1);
        let restored = chunks
            .iter()
            .map(|chunk| {
                chunk
                    .strip_prefix("(continued)\n\n")
                    .unwrap_or(chunk)
                    .strip_suffix("\n\n(continues...)")
                    .unwrap_or_else(|| chunk.strip_prefix("(continued)\n\n").unwrap_or(chunk))
            })
            .collect::<String>();
        assert_eq!(restored, source);
        for chunk in chunks {
            let (rich, fallback) = telegram_user_message_messages(&chunk, credit);
            assert_eq!(rich.matches("<cite>").count(), 1);
            assert!(fallback.chars().count() <= TELEGRAM_MAX_MESSAGE_CHARS);
        }
    }

    #[test]
    fn resolved_approval_text_replaces_pending_instructions() {
        let approval = PendingApproval {
            request_id: json!("request-1"),
            request_kind: "commandExecution".to_string(),
            method: "item/commandExecution/requestApproval".to_string(),
            params: json!({}),
            summary: "Run `cargo test`".to_string(),
            decisions: vec![ApprovalDecisionOption {
                label: "Allow".to_string(),
                decision: json!("approved"),
            }],
            message_id: Some("77".to_string()),
            remote_client_key: Some("client".to_string()),
        };

        let text = resolved_approval_text(&approval, 1, "Allow", ImText::zh_cn());

        assert!(text.contains("审批已处理"));
        assert!(text.contains("已选择 /1：Allow"));
        assert!(text.contains("Run `cargo test`"));
        assert!(!text.contains("回复 /1 处理"));
    }

    #[test]
    fn empty_keyboard_removes_all_inline_buttons() {
        assert_eq!(empty_inline_keyboard(), json!({ "inline_keyboard": [] }));
    }
}
