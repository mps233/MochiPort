use anyhow::{Context, Result, anyhow};
use serde_json::json;
use std::path::{Path, PathBuf};
use tokio::time::{Duration, sleep};

use crate::{
    chain_log,
    im::core::{
        i18n::{ImLocale, ImText},
        text_utils::log_text_preview,
        thread::ThreadCreateOption,
    },
    im::runtime::{
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
    /// Test-only constructor: production builds always supply a locale through
    /// `with_locale`.
    #[cfg(test)]
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
            .send_document_file(target, &path, Some(&caption), Some(TelegramParseMode::Html))
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
        let base = ImText::for_locale(self.locale).telegram_turn_completed_card_title();
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

mod approval;
mod keyboards;
#[cfg(test)]
mod tests;
mod text_blocks;
mod thread_list_html;
mod thread_settings_html;

use approval::*;
use keyboards::*;
use text_blocks::*;
use thread_list_html::*;
use thread_settings_html::*;

fn should_fallback_from_rich_message(err: &anyhow::Error) -> bool {
    err.downcast_ref::<TelegramApiError>()
        .is_some_and(TelegramApiError::should_fallback_from_rich_message)
}

fn log_adapter(event: &str, message: impl AsRef<str>) {
    chain_log::write_diagnostic_lazy(|| {
        format!("[telegram_adapter] event={} {}", event, message.as_ref())
    });
}

fn create_options_table_html(options: &[ThreadCreateOption]) -> String {
    let mut lines = Vec::new();
    for (index, option) in options.iter().enumerate() {
        lines.push(create_option_row_html(index, option));
        lines.push(String::new());
    }
    lines.join("\n")
}
