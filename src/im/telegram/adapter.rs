use anyhow::{Context, Result, anyhow};
use serde_json::json;
use std::path::{Path, PathBuf};
use tokio::time::{Duration, sleep};

use crate::{
    chain_log,
    im::core::{
        i18n::{ImLocale, ImText},
        text_utils::log_text_preview,
        thread::{ThreadCreateOption, ThreadModelChoice},
    },
    im_runtime::{
        ObservedSetting, PendingApproval, TelegramModelSwitchRequestState,
        TelegramThreadSettingsSpeed, TelegramThreadSettingsStage, approval_request_fingerprint,
    },
    types::{now_ms, split_telegram_message_target},
};

use super::api::{
    TELEGRAM_MAX_MEDIA_GROUP_ITEMS, TelegramApi, TelegramApiError, TelegramInputRichMessage,
    TelegramParseMode,
};

const TELEGRAM_MAX_MESSAGE_CHARS: usize = 4096;
const TELEGRAM_CONTINUATION_OVERHEAD: usize = 30;
/// 代码围栏跨段时补 ```` ``` ```` 的开销预算（语言标注最长 24 字符）。
const TELEGRAM_FENCE_RESERVE: usize = 32;
/// 最终回复切块数超过该值时改发文件附件，避免长输出刷屏。
const TELEGRAM_TURN_DOCUMENT_CHUNK_LIMIT: usize = 2;
const TELEGRAM_CHUNK_DELAY_MS: u64 = 100;
const TELEGRAM_APPROVAL_SUMMARY_MAX_CHARS: usize = 2800;
const TELEGRAM_APPROVAL_DECISION_MAX_CHARS: usize = 120;

#[derive(Clone)]
pub struct TelegramAdapter {
    api: TelegramApi,
    locale: ImLocale,
}

#[derive(Debug, Clone)]
pub struct TelegramThreadListEntry {
    pub title: String,
    pub state: String,
    pub cwd: Option<String>,
}

impl TelegramAdapter {
    pub fn new(api: TelegramApi) -> Self {
        Self {
            api,
            locale: ImLocale::ZhCn,
        }
    }

    /// 按配置语言渲染切块续段标记等适配层文案。
    pub fn with_locale(api: TelegramApi, locale: ImLocale) -> Self {
        Self { api, locale }
    }

    fn chunk_markers(&self) -> (&'static str, &'static str) {
        match self.locale {
            ImLocale::ZhCn => ("（未完待续）", "（接上文）"),
            ImLocale::EnUs => ("(continues...)", "(continued)"),
        }
    }

    pub async fn send_text(&self, target: &str, text: &str) -> Result<String> {
        self.send_text_with_silence(target, text, false).await
    }

    /// `silent = true` 用于“完整”颗粒度的过程文本：消息照发，但不触发响铃。
    /// 多段回复的续段（第二段起）始终静默，只有首段会响铃。
    pub async fn send_text_with_silence(
        &self,
        target: &str,
        text: &str,
        silent: bool,
    ) -> Result<String> {
        let text = telegram_cleanup_text(text);
        let mut last_message_id = 0;
        let (continues_marker, continued_marker) = self.chunk_markers();
        let chunks = telegram_text_chunks(&text, continues_marker, continued_marker);
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
            let chunk_silent = silent || index > 0;
            let html = telegram_markdown_to_html(chunk);
            log_adapter(
                "send_text_chunk_begin",
                format!(
                    "chat={} chunk={}/{} silent={} chars={} preview={}",
                    target,
                    index + 1,
                    chunks.len(),
                    chunk_silent,
                    chunk.chars().count(),
                    log_text_preview(chunk, 500)
                ),
            );
            last_message_id = if chunk_silent {
                match self
                    .api
                    .send_text_parse_mode_silent(target, &html, TelegramParseMode::Html)
                    .await
                {
                    Ok(message_id) => {
                        log_adapter(
                            "send_text_chunk_sent",
                            format!(
                                "chat={} chunk={}/{} mode=html silent=true message={}",
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
                        let message_id = self.api.send_text_silent(target, chunk).await?;
                        log_adapter(
                            "send_text_chunk_sent",
                            format!(
                                "chat={} chunk={}/{} mode=plain silent=true message={}",
                                target,
                                index + 1,
                                chunks.len(),
                                message_id
                            ),
                        );
                        message_id
                    }
                }
            } else {
                match self
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
                        let message_id = self.api.send_text(target, chunk).await?;
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
        elapsed_ms: Option<u128>,
    ) -> Result<String> {
        let header = self.turn_completed_card_header(elapsed_ms);
        let (continues_marker, continued_marker) = self.chunk_markers();
        let chunks =
            telegram_turn_completed_chunks(reply_text, &header, continues_marker, continued_marker);
        // 超过切块上限的长回复改发文件附件：避免刷屏，保存和复制也更方便；
        // 发送失败时回退到普通分段发送。
        if chunks.len() > TELEGRAM_TURN_DOCUMENT_CHUNK_LIMIT {
            match self
                .send_turn_completed_document(target, reply_text, footer_text)
                .await
            {
                Ok(message_id) => return Ok(message_id),
                Err(err) => {
                    log_adapter(
                        "send_turn_document_failed",
                        format!("chat={} chunks={} err={}", target, chunks.len(), err),
                    );
                }
            }
        }
        let mut last_message_id = String::new();
        for (index, chunk) in chunks.iter().enumerate() {
            let is_last = index + 1 == chunks.len();
            if is_last {
                let (rich_markdown, fallback_markdown) =
                    telegram_turn_completed_messages(chunk, &header);
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

    /// 超长最终回复改发 `.md` 文件；caption 放通知语和署名，正文完整进附件。
    async fn send_turn_completed_document(
        &self,
        target: &str,
        reply_text: &str,
        footer_text: &str,
    ) -> Result<String> {
        let stamp = now_ms() as u64;
        let path = std::env::temp_dir().join(format!("mochiport-turn-{stamp}.md"));
        std::fs::write(&path, reply_text)
            .with_context(|| format!("failed to write {}", path.display()))?;
        let mut caption = self.turn_document_notice(reply_text.chars().count());
        let footer = footer_text.trim();
        if !footer.is_empty() {
            caption.push_str("\n\n");
            caption.push_str(footer);
        }
        caption = caption.chars().take(980).collect();
        let send_result = self
            .api
            .send_document_file(
                &target,
                &path,
                Some(&caption),
                Some(TelegramParseMode::Html),
            )
            .await;
        let _ = std::fs::remove_file(&path);
        let message_id = send_result?;
        log_adapter(
            "send_turn_document_sent",
            format!(
                "chat={} chars={} message={}",
                target,
                reply_text.chars().count(),
                message_id
            ),
        );
        Ok(message_id.to_string())
    }

    fn turn_document_notice(&self, chars: usize) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("回复较长（{chars} 字符），已作为附件发送。"),
            ImLocale::EnUs => format!("Long reply ({chars} chars) attached as a file."),
        }
    }

    /// 极简卡片头：`✅ 已完成 · 3分12秒`；耗时不足 1 秒或未知时只显示状态。
    fn turn_completed_card_header(&self, elapsed_ms: Option<u128>) -> String {
        let base = match self.locale {
            ImLocale::ZhCn => "✅ 已完成",
            ImLocale::EnUs => "✅ Completed",
        };
        match elapsed_ms.filter(|ms| *ms >= 1_000) {
            Some(ms) => format!("{base} · {}", format_turn_elapsed(self.locale, ms)),
            None => base.to_string(),
        }
    }

    pub async fn send_user_message_quote(
        &self,
        target: &str,
        message_text: &str,
        credit_text: &str,
    ) -> Result<String> {
        let (continues_marker, continued_marker) = self.chunk_markers();
        let chunks = telegram_user_message_chunks(
            message_text,
            credit_text,
            continues_marker,
            continued_marker,
        );
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
        let (raw_chat_id, _) = split_telegram_message_target(target);
        let Ok(chat_id) = raw_chat_id.trim().parse::<i64>() else {
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
            .send_rich_message_draft_to_target(target, draft_id, &rich_message)
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
        let (raw_chat_id, _) = split_telegram_message_target(target);
        let Ok(chat_id) = raw_chat_id.trim().parse::<i64>() else {
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
        match self
            .api
            .send_message_draft_to_target(target, draft_id, "")
            .await
        {
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

    /// Send multiple local images as Telegram albums, splitting at Telegram's
    /// ten-item limit. A failed album request falls back to the existing
    /// single-image path for every item so the document fallback remains
    /// available and one bad album does not prevent later images from being
    /// delivered.
    pub async fn send_image_paths(
        &self,
        target: &str,
        items: &[(PathBuf, Option<String>, Option<String>)],
    ) -> Result<Vec<String>> {
        if items.is_empty() {
            return Ok(Vec::new());
        }
        if items.len() == 1 {
            let (path, caption, fallback_text) = &items[0];
            return self
                .send_image_path_with_fallback(
                    target,
                    path,
                    caption.as_deref(),
                    fallback_text.as_deref(),
                )
                .await
                .map(|message_id| vec![message_id]);
        }

        log_adapter(
            "send_image_group_begin",
            format!(
                "chat={} images={} groups={} captions_chars={}",
                target,
                items.len(),
                Self::telegram_media_group_sizes(items.len()).len(),
                items
                    .iter()
                    .map(|(_, caption, _)| caption.as_deref().unwrap_or("").chars().count())
                    .sum::<usize>()
            ),
        );
        let mut message_ids = Vec::with_capacity(items.len());
        let mut failures = Vec::new();
        let group_sizes = Self::telegram_media_group_sizes(items.len());
        let mut group_start = 0;
        for (group_index, group_size) in group_sizes.iter().copied().enumerate() {
            let group_end = group_start + group_size;
            let group = &items[group_start..group_end];
            group_start = group_end;
            // A remainder of one after splitting (for example, item 11) must
            // use sendPhoto directly because Telegram albums require >=2.
            if group.len() == 1 {
                let (path, caption, fallback_text) = &group[0];
                match self
                    .send_image_path_with_fallback(
                        target,
                        path,
                        caption.as_deref(),
                        fallback_text.as_deref(),
                    )
                    .await
                {
                    Ok(message_id) => message_ids.push(message_id),
                    Err(err) => failures.push(format!(
                        "image {} ({}) failed: {err}",
                        group_index * TELEGRAM_MAX_MEDIA_GROUP_ITEMS + 1,
                        path.display()
                    )),
                }
                continue;
            }

            let group_items = group
                .iter()
                .map(|(path, caption, _)| {
                    let caption_html = caption
                        .as_deref()
                        .map(telegram_cleanup_text)
                        .map(|caption| telegram_markdown_to_html(&caption));
                    (path.clone(), caption_html)
                })
                .collect::<Vec<_>>();
            match self.api.send_photo_group_files(target, &group_items).await {
                Ok(group_message_ids) => {
                    log_adapter(
                        "send_image_group_sent",
                        format!(
                            "chat={} group={}/{} images={} method=sendMediaGroup",
                            target,
                            group_index + 1,
                            group_sizes.len(),
                            group.len()
                        ),
                    );
                    message_ids.extend(
                        group_message_ids
                            .into_iter()
                            .map(|message_id| message_id.to_string()),
                    );
                }
                Err(group_err) => {
                    log_adapter(
                        "send_image_group_failed",
                        format!(
                            "chat={} group={}/{} images={} fallback=single err={group_err}",
                            target,
                            group_index + 1,
                            group_sizes.len(),
                            group.len()
                        ),
                    );
                    for (offset, (path, caption, fallback_text)) in group.iter().enumerate() {
                        match self
                            .send_image_path_with_fallback(
                                target,
                                path,
                                caption.as_deref(),
                                fallback_text.as_deref(),
                            )
                            .await
                        {
                            Ok(message_id) => message_ids.push(message_id),
                            Err(err) => failures.push(format!(
                                "group {} image {} ({}) failed after album error ({group_err}): {err}",
                                group_index + 1,
                                group_index * TELEGRAM_MAX_MEDIA_GROUP_ITEMS + offset + 1,
                                path.display()
                            )),
                        }
                    }
                }
            }
        }

        if failures.is_empty() {
            log_adapter(
                "send_image_group_done",
                format!(
                    "chat={} images={} delivered={}",
                    target,
                    items.len(),
                    message_ids.len()
                ),
            );
            Ok(message_ids)
        } else {
            Err(anyhow!(
                "telegram image delivery failed for {} of {} images: {}",
                failures.len(),
                items.len(),
                failures.join("; ")
            ))
        }
    }

    fn telegram_media_group_sizes(item_count: usize) -> Vec<usize> {
        if item_count == 0 {
            return Vec::new();
        }
        if item_count <= TELEGRAM_MAX_MEDIA_GROUP_ITEMS {
            return vec![item_count];
        }

        let group_count = item_count.div_ceil(TELEGRAM_MAX_MEDIA_GROUP_ITEMS);
        let base_size = item_count / group_count;
        let remainder = item_count % group_count;
        (0..group_count)
            .map(|index| base_size + usize::from(index < remainder))
            .collect()
    }

    async fn send_image_path_with_fallback(
        &self,
        target: &str,
        local_path: &Path,
        caption: Option<&str>,
        fallback_text: Option<&str>,
    ) -> Result<String> {
        match self.send_image_path(target, local_path, caption).await {
            Ok(message_id) => Ok(message_id),
            Err(image_err) => {
                let Some(fallback_text) = fallback_text
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                else {
                    return Err(image_err);
                };
                self.send_text(target, fallback_text)
                    .await
                    .map_err(|fallback_err| anyhow!("image={image_err}; fallback={fallback_err}"))
            }
        }
    }

    pub async fn send_approval(
        &self,
        target: &str,
        approval: &PendingApproval,
        im_text: ImText,
    ) -> Result<String> {
        let text = telegram_cleanup_text(&approval_text(approval, im_text));
        let Some(keyboard) = approval_keyboard(approval, im_text) else {
            return self.send_text(target, &text).await;
        };
        let (continues_marker, continued_marker) = self.chunk_markers();
        let chunks = telegram_text_chunks(&text, continues_marker, continued_marker);
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
        _option_index: usize,
        decision_label: &str,
        text: ImText,
    ) -> Result<bool> {
        let Some(message_id) = message_id else {
            return Ok(false);
        };
        let resolved = resolved_approval_text(approval, decision_label, text);
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

        match self.api.send_rich_message(target, rich_message).await {
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
        let keyboard =
            create_options_keyboard(request_id, field, page, options, has_prev, has_next, text);

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
                keyboard,
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

    /// Render the model picker for an already-bound Codex thread.
    ///
    /// `models` is the current page (with indexes starting at zero). The
    /// callback only carries the request id, page and index; the flow resolves
    /// the selected model from its pending request state. Keeping model ids
    /// out of callback data avoids Telegram's 64-byte callback limit and also
    /// prevents stale buttons from selecting a value from a newer list.
    pub async fn send_thread_model_list(
        &self,
        target: &str,
        request_id: &str,
        title: &str,
        body: &str,
        models: &[ThreadModelChoice],
        page: usize,
        has_prev: bool,
        has_next: bool,
        message_id: Option<&str>,
        text: ImText,
    ) -> Result<String> {
        let keyboard = model_list_keyboard(request_id, page, models, has_prev, has_next, text);
        let text_html = model_list_html_text(title, body, page, models, text);
        log_adapter(
            "send_thread_model_list_begin",
            format!(
                "chat={} request={} page={} models={} text_len={}",
                target,
                request_id,
                page,
                models.len(),
                text_html.chars().count()
            ),
        );
        let message_id = self
            .send_or_update_text_with_reply_markup_parse_mode(
                target,
                message_id,
                &text_html,
                keyboard,
                TelegramParseMode::Html,
            )
            .await?;
        log_adapter(
            "send_thread_model_list_done",
            format!(
                "chat={} request={} page={} models={} message={}",
                target,
                request_id,
                page,
                models.len(),
                message_id
            ),
        );
        Ok(message_id.to_string())
    }

    /// Render one view of Telegram's staged settings editor for an existing
    /// Codex thread. The request owns all selected values; callbacks contain
    /// only short indexes or fixed tokens.
    pub async fn send_thread_settings_editor(
        &self,
        target: &str,
        request: &TelegramModelSwitchRequestState,
        text: ImText,
    ) -> Result<String> {
        let keyboard = thread_settings_keyboard(request, text);
        let text_html = thread_settings_html(request, text);
        self.send_or_update_text_with_reply_markup_parse_mode(
            target,
            request.message_id.as_deref(),
            &text_html,
            keyboard,
            TelegramParseMode::Html,
        )
        .await
        .map(|message_id| message_id.to_string())
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

fn render_approval_text(
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

fn truncate_text_with_ellipsis(text: &str, max_chars: usize) -> String {
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

fn resolved_approval_text(
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

fn approval_keyboard(approval: &PendingApproval, text: ImText) -> Option<serde_json::Value> {
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

fn button(text: &str, callback_data: &str) -> serde_json::Value {
    json!({
        "text": truncate_button_text(text),
        "callback_data": callback_data,
    })
}

fn approval_button(text: &str, callback_data: &str) -> serde_json::Value {
    json!({
        "text": truncate_button_text(text),
        "callback_data": callback_data,
    })
}

fn approval_button_label(text: ImText, label: &str) -> String {
    approval_decision_display_label(text, label)
}

fn approval_decision_display_label(text: ImText, label: &str) -> String {
    truncate_display_text(
        &text.approval_decision_label(label).replace('`', ""),
        TELEGRAM_APPROVAL_DECISION_MAX_CHARS,
    )
}

fn truncate_approval_summary(summary: &str) -> String {
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

/// 会话设置选项键盘：每个选项一个按钮（tcs 逐项选择），外加翻页与返回。
/// 数字回复的后备路径不变，按钮与 `/N` 编号一一对应。
fn create_options_keyboard(
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

fn create_options_table_html(options: &[ThreadCreateOption]) -> String {
    let mut lines = Vec::new();
    for (index, option) in options.iter().enumerate() {
        lines.push(create_option_row_html(index, option));
        lines.push(String::new());
    }
    lines.join("\n")
}

fn model_list_keyboard(
    request_id: &str,
    page: usize,
    models: &[ThreadModelChoice],
    has_prev: bool,
    has_next: bool,
    text: ImText,
) -> serde_json::Value {
    let page = page.max(1);
    let mut rows = Vec::with_capacity(models.len() + 1);
    let mut nav = Vec::new();
    if has_prev {
        nav.push(button(
            text.previous_page_button(),
            &format!("tmp:{request_id}:prev"),
        ));
    }
    if has_next {
        nav.push(button(
            text.next_page_button(),
            &format!("tmp:{request_id}:next"),
        ));
    }
    if !nav.is_empty() {
        rows.push(nav);
    }
    for (index, model) in models.iter().enumerate() {
        let label = model
            .label
            .trim()
            .strip_prefix('`')
            .and_then(|value| value.strip_suffix('`'))
            .unwrap_or_else(|| model.label.trim());
        let label = if label.is_empty() {
            model.value.trim()
        } else {
            label
        };
        rows.push(vec![button(
            label,
            &format!("tms:{request_id}:{page}:{index}"),
        )]);
    }
    inline_keyboard(rows)
}

fn model_list_html_text(
    title: &str,
    body: &str,
    page: usize,
    models: &[ThreadModelChoice],
    text: ImText,
) -> String {
    let options_html = models
        .iter()
        .enumerate()
        .map(|(index, model)| model_entry_html(index, model))
        .collect::<Vec<_>>()
        .join("\n\n");
    let hint = if models.is_empty() {
        text.no_options().to_string()
    } else {
        text.page_click_hint(page, models.len())
    };
    format!(
        "<b>{}</b>\n{}\n\n{}\n<code>{}</code>",
        telegram_html_escape(title),
        telegram_markdown_to_html(&telegram_cleanup_text(body)),
        options_html,
        telegram_html_escape(&hint)
    )
}

fn model_entry_html(index: usize, model: &ThreadModelChoice) -> String {
    let label = model
        .label
        .trim()
        .strip_prefix('`')
        .and_then(|value| value.strip_suffix('`'))
        .unwrap_or_else(|| model.label.trim());
    let label = if label.is_empty() {
        model.value.trim()
    } else {
        label
    };
    let label = truncate_display_text(label, 72);
    format!("/{} <b>{}</b>", index + 1, telegram_html_escape(&label))
}

fn thread_settings_keyboard(
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

fn thread_settings_html(request: &TelegramModelSwitchRequestState, text: ImText) -> String {
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

fn thread_settings_overview_html(
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

fn thread_settings_model_html(request: &TelegramModelSwitchRequestState, text: ImText) -> String {
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

fn thread_settings_effort_html(request: &TelegramModelSwitchRequestState, text: ImText) -> String {
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

fn thread_settings_speed_html(request: &TelegramModelSwitchRequestState, text: ImText) -> String {
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

fn thread_settings_selected_model(
    request: &TelegramModelSwitchRequestState,
) -> Option<&crate::im_runtime::TelegramThreadSettingsModelChoice> {
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

fn thread_settings_observed_value(
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

fn thread_settings_code(value: &str) -> String {
    format!("<code>{}</code>", telegram_html_escape(value))
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
        .replace(['\r', '\n'], " ")
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
                in_code_block = false;
            } else {
                match fence_language(line) {
                    Some(language) => {
                        html.push_str("<pre><code class=\"language-");
                        html.push_str(&language);
                        html.push_str("\">");
                    }
                    None => html.push_str("<pre><code>"),
                }
                in_code_block = true;
            }
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

/// 极简卡片：`**✅ 已完成 · 耗时**` + 分隔线 + 正文；署名不再随正文发送。
fn telegram_turn_completed_messages(reply_text: &str, header: &str) -> (String, String) {
    let reply_text = telegram_cleanup_text(reply_text).trim().to_string();
    let header = header.trim();
    let separator = super::rich_blocks::TELEGRAM_CARD_SEPARATOR;
    if reply_text.is_empty() {
        return (format!("**{header}**"), header.to_string());
    }
    (
        format!("**{header}**\n{separator}\n\n{reply_text}"),
        format!("{header}\n{separator}\n\n{reply_text}"),
    )
}

fn format_turn_elapsed(locale: ImLocale, elapsed_ms: u128) -> String {
    let total_seconds = (elapsed_ms / 1000) as u64;
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;
    match locale {
        ImLocale::ZhCn => {
            if hours > 0 {
                format!("{hours}小时{minutes}分")
            } else if minutes > 0 {
                format!("{minutes}分{seconds}秒")
            } else {
                format!("{seconds}秒")
            }
        }
        ImLocale::EnUs => {
            if hours > 0 {
                format!("{hours}h{minutes:02}m")
            } else if minutes > 0 {
                format!("{minutes}m{seconds:02}s")
            } else {
                format!("{seconds}s")
            }
        }
    }
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

fn telegram_text_chunks(text: &str, continues_marker: &str, continued_marker: &str) -> Vec<String> {
    telegram_text_chunks_with_limit(
        text,
        TELEGRAM_MAX_MESSAGE_CHARS,
        continues_marker,
        continued_marker,
    )
}

fn telegram_turn_completed_chunks(
    text: &str,
    header: &str,
    continues_marker: &str,
    continued_marker: &str,
) -> Vec<String> {
    // 头部 + 分隔线 + 分段空行的字符预算。
    let reserved_chars = header.trim().chars().count()
        + super::rich_blocks::TELEGRAM_CARD_SEPARATOR.chars().count()
        + 4;
    let max_chars = TELEGRAM_MAX_MESSAGE_CHARS
        .saturating_sub(reserved_chars)
        .max(TELEGRAM_CONTINUATION_OVERHEAD + 1);
    telegram_text_chunks_with_limit(text, max_chars, continues_marker, continued_marker)
}

fn telegram_user_message_chunks(
    text: &str,
    credit_text: &str,
    continues_marker: &str,
    continued_marker: &str,
) -> Vec<String> {
    let reserved_chars = credit_text.trim().chars().count().saturating_add(2);
    let max_chars = TELEGRAM_MAX_MESSAGE_CHARS
        .saturating_sub(reserved_chars)
        .max(TELEGRAM_CONTINUATION_OVERHEAD + 1);
    telegram_text_chunks_with_limit(text, max_chars, continues_marker, continued_marker)
}

fn telegram_text_chunks_with_limit(
    text: &str,
    max_chars: usize,
    continues_marker: &str,
    continued_marker: &str,
) -> Vec<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return vec![" ".to_string()];
    }
    if trimmed.chars().count() <= max_chars {
        return vec![trimmed.to_string()];
    }

    // 含代码围栏时预留跨段补围栏的开销，避免补齐后超出单条上限。
    let effective_max = if trimmed.contains("```") {
        max_chars.saturating_sub(TELEGRAM_FENCE_RESERVE)
    } else {
        max_chars
    };
    let chunks = rebalance_code_fences(split_message_for_telegram(trimmed, effective_max));
    let chunk_count = chunks.len();
    chunks
        .into_iter()
        .enumerate()
        .map(|(index, chunk)| {
            if index == 0 {
                format!("{chunk}\n\n{continues_marker}")
            } else if index + 1 == chunk_count {
                format!("{continued_marker}\n\n{chunk}")
            } else {
                format!("{continued_marker}\n\n{chunk}\n\n{continues_marker}")
            }
        })
        .collect()
}

/// 提取 ``` 围栏行上的语言标注；仅保留安全字符，用于 `<code class="language-…">`。
fn fence_language(line: &str) -> Option<String> {
    let info = line.trim_start().strip_prefix("```")?.trim();
    let language: String = info
        .split_whitespace()
        .next()
        .unwrap_or("")
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_' || *ch == '+')
        .take(24)
        .collect();
    if language.is_empty() {
        None
    } else {
        Some(language)
    }
}

/// 切块可能落在代码围栏中间：跨段时补齐闭合/起始围栏，
/// 否则后续分段会被当普通文本渲染，样式全部丢失。
fn rebalance_code_fences(chunks: Vec<String>) -> Vec<String> {
    let mut rebalanced = Vec::with_capacity(chunks.len());
    let mut open_language: Option<String> = None;
    for chunk in chunks {
        let mut body = String::with_capacity(chunk.len() + 16);
        if let Some(language) = &open_language {
            body.push_str("```");
            body.push_str(language);
            body.push('\n');
        }
        for line in chunk.lines() {
            body.push_str(line);
            body.push('\n');
            if line.trim_start().starts_with("```") {
                open_language = match open_language {
                    Some(_) => None,
                    None => fence_language(line),
                };
            }
        }
        if open_language.is_some() {
            body.push_str("```");
        }
        rebalanced.push(body.trim_end().to_string());
    }
    rebalanced
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
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use crate::{
        im::core::{i18n::ImText, thread::ThreadCreateOption},
        im::telegram::{
            api::{TELEGRAM_MAX_MEDIA_GROUP_ITEMS, TelegramApi},
            types::TelegramSettings,
        },
        im_runtime::{
            ApprovalDecisionOption, ObservedSetting, PendingApproval,
            TelegramModelSwitchRequestState, TelegramThreadSettingsDraft,
            TelegramThreadSettingsModelChoice, TelegramThreadSettingsSpeed,
            TelegramThreadSettingsStage, ThreadSettingsSnapshot,
        },
    };

    use super::{
        TELEGRAM_MAX_MESSAGE_CHARS, TelegramAdapter, approval_keyboard, approval_text,
        create_options_keyboard, empty_inline_keyboard, resolved_approval_text,
        telegram_cleanup_text, telegram_context_compaction_messages, telegram_markdown_to_html,
        telegram_text_chunks, telegram_turn_completed_chunks, telegram_turn_completed_messages,
        telegram_user_message_chunks, telegram_user_message_messages, thread_settings_html,
        thread_settings_keyboard,
    };

    #[tokio::test]
    async fn thinking_draft_skips_targets_that_cannot_be_private_chats() {
        let adapter = TelegramAdapter::new(TelegramApi::new(TelegramSettings::default()));

        assert!(!adapter.send_thinking_draft("-1001", 7).await.unwrap());
        assert!(!adapter.send_thinking_draft("@channel", 7).await.unwrap());
        assert!(!adapter.send_thinking_draft("42", 0).await.unwrap());
    }

    #[test]
    fn balances_large_image_sets_without_singleton_albums() {
        assert_eq!(
            TelegramAdapter::telegram_media_group_sizes(0),
            Vec::<usize>::new()
        );
        assert_eq!(TelegramAdapter::telegram_media_group_sizes(1), vec![1]);
        assert_eq!(TelegramAdapter::telegram_media_group_sizes(10), vec![10]);
        assert_eq!(TelegramAdapter::telegram_media_group_sizes(11), vec![6, 5]);
        assert_eq!(
            TelegramAdapter::telegram_media_group_sizes(21),
            vec![7, 7, 7]
        );
        assert!(
            TelegramAdapter::telegram_media_group_sizes(101)
                .iter()
                .all(|size| (2..=TELEGRAM_MAX_MEDIA_GROUP_ITEMS).contains(size))
        );
    }

    /// 英文标记的快捷入口，便于断言；默认 locale（中文）的标记由适配层选择。
    fn chunks_en(text: &str) -> Vec<String> {
        telegram_text_chunks(text, "(continues...)", "(continued)")
    }

    #[test]
    fn chunks_long_text_on_char_boundaries() {
        let chunks = chunks_en(&"你好世界".repeat(1100));

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
        let chunks = chunks_en(text);

        assert_eq!(chunks, vec!["hello"]);
    }

    #[test]
    fn empty_message_uses_space_placeholder() {
        let chunks = chunks_en("  \n ");

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
        let chunks = chunks_en(&format!("{first}\n{second}"));

        assert!(chunks[0].contains("(continues...)"));
        assert!(chunks[0].contains('\n'));
        assert!(chunks[0].trim_start().starts_with('a'));
        assert!(chunks[1].contains('b'));
    }

    #[test]
    fn turn_completed_message_uses_a_card_header_with_plain_fallback() {
        let (rich, fallback) =
            telegram_turn_completed_messages("🤖 Codex\n\n**Build:** `323`", "✅ 已完成 · 3分12秒");

        assert_eq!(
            rich,
            "**✅ 已完成 · 3分12秒**\n──────────────\n\n🤖 Codex\n\n**Build:** `323`"
        );
        assert_eq!(
            fallback,
            "✅ 已完成 · 3分12秒\n──────────────\n\n🤖 Codex\n\n**Build:** `323`"
        );
    }

    #[test]
    fn turn_completed_header_handles_an_empty_reply() {
        let (rich, fallback) = telegram_turn_completed_messages("  ", "Done & closed");

        assert_eq!(rich, "**Done & closed**");
        assert_eq!(fallback, "Done & closed");
    }

    #[test]
    fn turn_completed_chunks_reserve_space_for_the_card_header() {
        for text in ["a".repeat(TELEGRAM_MAX_MESSAGE_CHARS), "界".repeat(4_500)] {
            let chunks = telegram_turn_completed_chunks(
                &text,
                "✅ 已完成 · 3分12秒",
                "(continues...)",
                "(continued)",
            );
            assert!(chunks.len() > 1);
            assert!(
                chunks[..chunks.len() - 1]
                    .iter()
                    .all(|chunk| chunk.chars().count() <= TELEGRAM_MAX_MESSAGE_CHARS)
            );
            assert!(
                chunks[..chunks.len() - 1]
                    .iter()
                    .all(|chunk| !chunk.contains("✅ 已完成"))
            );
            let (rich, fallback) = telegram_turn_completed_messages(
                chunks.last().expect("final chunk"),
                "✅ 已完成 · 3分12秒",
            );
            assert!(fallback.chars().count() <= TELEGRAM_MAX_MESSAGE_CHARS);
            assert!(rich.matches("✅ 已完成").count() == 1);
            assert!(!rich.contains("<footer>"));
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
        let chunks = telegram_user_message_chunks(&source, credit, "(continues...)", "(continued)");

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

        let text = resolved_approval_text(&approval, "Allow", ImText::zh_cn());

        assert!(text.contains("审批已处理"));
        assert!(text.contains("已选择：允许"));
        assert!(text.contains("Run `cargo test`"));
        assert!(!text.contains("回复 /1 处理"));
    }

    #[test]
    fn approval_text_uses_localized_sections_and_fallback_commands() {
        let approval = PendingApproval {
            request_id: json!("request-1"),
            request_kind: "command".to_string(),
            method: "item/commandExecution/requestApproval".to_string(),
            params: json!({}),
            summary: "Run `cargo test`".to_string(),
            decisions: vec![ApprovalDecisionOption {
                label: "Yes, just this once".to_string(),
                decision: json!("approved"),
            }],
            message_id: None,
            remote_client_key: None,
        };

        let text = approval_text(&approval, ImText::zh_cn());

        assert!(text.starts_with("**审批待处理**\n类型：`命令执行`"));
        assert!(text.contains("**请求内容**"));
        assert!(!text.contains("可选操作"));
        assert!(text.contains("点击下方按钮处理。"));
        assert!(!text.contains("request_kind"));
    }

    #[test]
    fn approval_keyboard_localizes_labels_without_changing_callback_shape() {
        let approval = PendingApproval {
            request_id: json!("request-1"),
            request_kind: "command".to_string(),
            method: "item/commandExecution/requestApproval".to_string(),
            params: json!({}),
            summary: "Run `cargo test`".to_string(),
            decisions: vec![ApprovalDecisionOption {
                label: "Yes, just this once".to_string(),
                decision: json!("approved"),
            }],
            message_id: None,
            remote_client_key: None,
        };

        let keyboard = approval_keyboard(&approval, ImText::zh_cn()).expect("keyboard");
        let button = &keyboard["inline_keyboard"][0][0];
        assert_eq!(button["text"], "仅本次允许");
        assert!(
            button["callback_data"]
                .as_str()
                .is_some_and(|value| { value.starts_with("ap:") && value.ends_with(":1") })
        );
    }

    #[test]
    fn thread_settings_editor_uses_revision_callbacks_without_numeric_reply_hints() {
        let request = TelegramModelSwitchRequestState {
            request_id: "thread-model-7".to_string(),
            conversation_key: "telegram:bot:chat".to_string(),
            account_id: "bot".to_string(),
            chat_id: "chat".to_string(),
            expected_thread_id: "thread-7".to_string(),
            remote_client_key: "im:telegram:bot:chat".to_string(),
            catalog: vec![TelegramThreadSettingsModelChoice {
                model: "gpt-primary".to_string(),
                label: "GPT <Primary>".to_string(),
                supported_efforts: vec!["medium".to_string()],
                default_effort: Some("medium".to_string()),
                supports_fast: true,
            }],
            observed: ThreadSettingsSnapshot {
                model: ObservedSetting::Known(Some("gpt-primary".to_string())),
                effort: ObservedSetting::Known(Some("medium".to_string())),
                service_tier: ObservedSetting::Known(None),
            },
            draft: TelegramThreadSettingsDraft {
                speed: Some(TelegramThreadSettingsSpeed::Fast),
                ..Default::default()
            },
            revision: 9,
            expires_at_ms: 1,
            stage: TelegramThreadSettingsStage::Overview,
            model_page: 1,
            compatibility: None,
            pending_apply: None,
            stale: false,
            message_id: None,
        };
        let text = ImText::zh_cn();
        let keyboard = thread_settings_keyboard(&request, text);

        assert_eq!(
            keyboard["inline_keyboard"][0][0]["callback_data"],
            "tmo:thread-model-7:9:model"
        );
        assert_eq!(
            keyboard["inline_keyboard"][0][1]["callback_data"],
            "tmo:thread-model-7:9:effort"
        );
        assert_eq!(
            keyboard["inline_keyboard"][1][0]["callback_data"],
            "tmo:thread-model-7:9:speed"
        );
        assert_eq!(
            keyboard["inline_keyboard"][2][1]["callback_data"],
            "tma:thread-model-7:9"
        );

        let rendered = thread_settings_html(&request, text);
        assert!(rendered.contains("<b>已生效</b>"));
        assert!(rendered.contains("<b>待应用</b>"));
        assert!(rendered.contains("快速"));
        assert!(!rendered.contains("/1"));
    }

    #[test]
    fn approval_text_keeps_plain_reply_commands_for_rich_text_fallback() {
        let approval = PendingApproval {
            request_id: json!("request-1"),
            request_kind: "command".to_string(),
            method: "item/commandExecution/requestApproval".to_string(),
            params: json!({}),
            summary: "Run `cargo test`".to_string(),
            decisions: vec![ApprovalDecisionOption {
                label: "Yes, proceed".to_string(),
                decision: json!("approved"),
            }],
            message_id: None,
            remote_client_key: None,
        };

        let plain = approval_text(&approval, ImText::zh_cn());
        let rich = telegram_markdown_to_html(&plain);

        assert!(!plain.contains("`/1`"));
        assert!(plain.contains("点击下方按钮处理。"));
        assert!(rich.contains("<b>审批待处理</b>"));
    }

    #[test]
    fn approval_cards_bound_oversized_summaries_to_one_message() {
        let approval = PendingApproval {
            request_id: json!("request-1"),
            request_kind: "command".to_string(),
            method: "item/commandExecution/requestApproval".to_string(),
            params: json!({}),
            summary: "x".repeat(8_000),
            decisions: vec![ApprovalDecisionOption {
                label: "Yes, proceed".to_string(),
                decision: json!("approved"),
            }],
            message_id: None,
            remote_client_key: None,
        };

        let pending = approval_text(&approval, ImText::zh_cn());
        let resolved = resolved_approval_text(&approval, "Yes, proceed", ImText::zh_cn());

        assert!(pending.chars().count() <= TELEGRAM_MAX_MESSAGE_CHARS);
        assert!(resolved.chars().count() <= TELEGRAM_MAX_MESSAGE_CHARS);
        assert!(pending.contains('…'));
        assert!(resolved.contains('…'));
    }

    #[test]
    fn approval_cards_with_many_options_stay_on_one_editable_message() {
        let approval = PendingApproval {
            request_id: json!("request-many-options"),
            request_kind: "command".to_string(),
            method: "item/commandExecution/requestApproval".to_string(),
            params: json!({}),
            summary: "Run a command".to_string(),
            decisions: (0..600)
                .map(|index| ApprovalDecisionOption {
                    label: format!("Option {index}: allow this command for this session"),
                    decision: json!("approved"),
                })
                .collect(),
            message_id: None,
            remote_client_key: None,
        };

        let pending = approval_text(&approval, ImText::zh_cn());
        assert!(pending.chars().count() <= TELEGRAM_MAX_MESSAGE_CHARS);
        assert_eq!(
            telegram_text_chunks(&pending, "(continues...)", "(continued)").len(),
            1
        );
        assert!(pending.contains("点击下方按钮处理。"));
        assert!(!pending.contains("`/1`"));
        assert!(!pending.contains("`/600`"));
        assert!(!pending.contains("Option 599"));
    }

    #[test]
    fn approval_text_trims_oversized_summary_to_fit_one_message() {
        let approval = PendingApproval {
            request_id: json!("request-summary-and-options"),
            request_kind: "command".to_string(),
            method: "item/commandExecution/requestApproval".to_string(),
            params: json!({}),
            summary: "x".repeat(8_000),
            decisions: vec![ApprovalDecisionOption {
                label: "Yes, proceed".to_string(),
                decision: json!("approved"),
            }],
            message_id: None,
            remote_client_key: None,
        };

        let pending = approval_text(&approval, ImText::zh_cn());
        assert!(pending.chars().count() <= TELEGRAM_MAX_MESSAGE_CHARS);
        assert!(pending.contains("请求内容"));
        assert!(pending.contains('…'));
        assert!(pending.contains("点击下方按钮处理。"));
    }

    #[test]
    fn create_options_keyboard_gives_each_option_a_button() {
        let options = vec![
            ThreadCreateOption {
                label: "使用 Codex App 当前权限".to_string(),
                summary: Some("已选".to_string()),
            },
            ThreadCreateOption {
                label: "默认权限".to_string(),
                summary: Some("适合常规项目，需要时由用户确认。".to_string()),
            },
        ];
        let keyboard = create_options_keyboard(
            "thread-7",
            "permission",
            1,
            &options,
            false,
            true,
            ImText::zh_cn(),
        );
        let rows = keyboard["inline_keyboard"]
            .as_array()
            .expect("keyboard rows");

        let first = rows[0][0].as_object().expect("option button");
        assert_eq!(first["text"], "使用 Codex App 当前权限");
        assert_eq!(first["callback_data"], "tcs:thread-7:permission:1:0");
        let second = rows[1][0].as_object().expect("option button");
        assert_eq!(second["callback_data"], "tcs:thread-7:permission:1:1");
        let nav = rows[2][0].as_object().expect("nav button");
        assert_eq!(nav["callback_data"], "tcp:thread-7:permission:next");
        let back = rows[3][0].as_object().expect("back button");
        assert_eq!(back["callback_data"], "trc:thread-7:new");

        // cwd 字段额外提供自定义入口按钮。
        let cwd_keyboard = create_options_keyboard(
            "thread-7",
            "cwd",
            1,
            &options,
            false,
            false,
            ImText::zh_cn(),
        );
        let cwd_rows = cwd_keyboard["inline_keyboard"].as_array().expect("rows");
        let custom = cwd_rows[2][0].as_object().expect("custom cwd button");
        assert_eq!(custom["callback_data"], "tcv:thread-7:cwd:__custom__");
    }

    #[test]
    fn empty_keyboard_removes_all_inline_buttons() {
        assert_eq!(empty_inline_keyboard(), json!({ "inline_keyboard": [] }));
    }

    #[test]
    fn code_fence_language_annotates_html() {
        let html = telegram_markdown_to_html("```rust\nfn main() {}\n```");
        assert!(html.contains("<pre><code class=\"language-rust\">"));
        assert!(html.contains("</code></pre>"));

        let plain = telegram_markdown_to_html("```\nplain\n```");
        assert!(plain.contains("<pre><code>plain"));

        // 语言标注只保留安全字符，防止围栏行注入属性。
        let hostile = telegram_markdown_to_html("```rust onclick=alert(1)\nfn main() {}\n```");
        assert!(hostile.contains("language-rust"));
        assert!(!hostile.contains("onclick"));
    }

    #[test]
    fn chunks_rebalance_code_fences_across_segments() {
        let mut text = String::from("介绍\n```rust\n");
        for i in 0..400 {
            text.push_str(&format!("let value_{i} = {i}; // 注释内容\n"));
        }
        text.push_str("```\n结尾");

        let chunks = chunks_en(&text);
        assert!(chunks.len() > 1);
        for chunk in &chunks {
            let fence_lines = chunk
                .lines()
                .filter(|line| line.trim_start().starts_with("```"))
                .count();
            assert_eq!(fence_lines % 2, 0, "chunk fences unbalanced:\n{chunk}");
            let html = telegram_markdown_to_html(chunk);
            assert_eq!(
                html.matches("<pre><code").count(),
                html.matches("</code></pre>").count()
            );
        }
        // 续段自动补回围栏和语言标注。
        assert!(chunks[1].contains("```rust"));
        assert!(telegram_markdown_to_html(&chunks[1]).contains("language-rust"));
    }

    #[test]
    fn chunk_markers_follow_adapter_locale() {
        let zh = TelegramAdapter::new(TelegramApi::new(TelegramSettings::default()));
        assert_eq!(zh.chunk_markers(), ("（未完待续）", "（接上文）"));
        let en = TelegramAdapter::with_locale(
            TelegramApi::new(TelegramSettings::default()),
            crate::im::core::i18n::ImLocale::EnUs,
        );
        assert_eq!(en.chunk_markers(), ("(continues...)", "(continued)"));
    }

    #[tokio::test]
    async fn turn_completed_long_output_sends_document() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock Telegram server");
        let address = listener.local_addr().expect("mock Telegram address");
        let captured = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
        let captured_task = captured.clone();
        tokio::spawn(async move {
            loop {
                if let Ok((mut stream, _)) = listener.accept().await {
                    let mut buf = vec![0_u8; 65_536];
                    let count = stream.read(&mut buf).await.unwrap_or(0);
                    let request = String::from_utf8_lossy(&buf[..count]);
                    let line = request.lines().next().unwrap_or("").to_string();
                    if let Ok(mut slot) = captured_task.lock() {
                        slot.get_or_insert(line);
                    }
                    let body = r#"{"ok":true,"result":{"message_id":77,"chat":{"id":42,"type":"private"}}}"#;
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                }
            }
        });
        let api = TelegramApi::new(TelegramSettings {
            account_id: "tg_1".to_string(),
            bot_token: "test-token".to_string(),
            ..Default::default()
        })
        .with_test_api_base(format!("http://{address}"));
        let adapter = TelegramAdapter::new(api);
        let reply = "任务".repeat(4300);

        let result = adapter
            .send_turn_completed("42", &reply, "你 · Codex 电脑端", None)
            .await
            .expect("send long turn output");

        assert_eq!(result, "77");
        let line = captured
            .lock()
            .expect("capture lock")
            .clone()
            .unwrap_or_default();
        assert!(line.contains("sendDocument"), "request line: {line}");
    }
}
