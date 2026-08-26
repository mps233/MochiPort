use std::path::Path;

use anyhow::{Context, Result, anyhow};
use futures_util::StreamExt;
use reqwest::{StatusCode, multipart};
use serde::{Deserialize, Serialize};

use crate::chain_log;
use crate::types::split_telegram_message_target;

use super::types::TelegramSettings;

const TELEGRAM_API_BASE: &str = "https://api.telegram.org";
const TELEGRAM_MAX_DOWNLOAD_BYTES: u64 = 20 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct TelegramApi {
    settings: TelegramSettings,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramResponse<T> {
    pub ok: bool,
    pub result: Option<T>,
    pub description: Option<String>,
    #[serde(rename = "error_code")]
    pub error_code: Option<i64>,
    pub parameters: Option<TelegramResponseParameters>,
}

#[derive(Debug, Deserialize)]
pub struct TelegramResponseParameters {
    #[serde(rename = "retry_after")]
    pub retry_after: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct TelegramUpdate {
    pub update_id: i64,
    pub message: Option<TelegramMessage>,
    pub callback_query: Option<TelegramCallbackQuery>,
}

#[derive(Debug, Deserialize)]
pub struct TelegramCallbackQuery {
    pub id: String,
    pub from: TelegramUser,
    pub message: Option<TelegramMessage>,
    pub data: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TelegramMessage {
    pub message_id: i64,
    #[serde(default)]
    pub message_thread_id: Option<i64>,
    pub from: Option<TelegramUser>,
    pub chat: TelegramChat,
    pub text: Option<String>,
    pub caption: Option<String>,
    pub photo: Option<Vec<TelegramPhotoSize>>,
    pub document: Option<TelegramMediaFile>,
    pub audio: Option<TelegramMediaFile>,
    pub video: Option<TelegramMediaFile>,
    pub animation: Option<TelegramMediaFile>,
    pub voice: Option<TelegramMediaFile>,
    pub video_note: Option<TelegramMediaFile>,
    pub sticker: Option<TelegramStickerFile>,
    #[serde(default)]
    pub forum_topic_created: Option<TelegramForumTopicCreated>,
    #[serde(default)]
    pub forum_topic_edited: Option<TelegramForumTopicEdited>,
    #[serde(default)]
    pub forum_topic_closed: Option<TelegramForumTopicClosed>,
    #[serde(default)]
    pub forum_topic_reopened: Option<TelegramForumTopicReopened>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TelegramForumTopicCreated {
    pub name: String,
    pub icon_color: i64,
    #[serde(default)]
    pub icon_custom_emoji_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TelegramForumTopicEdited {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub icon_custom_emoji_id: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TelegramForumTopicClosed {}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TelegramForumTopicReopened {}

#[derive(Debug, Clone, Deserialize)]
pub struct TelegramForumTopic {
    pub message_thread_id: i64,
    pub name: String,
    #[serde(default)]
    pub icon_color: Option<i64>,
    #[serde(default)]
    pub icon_custom_emoji_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TelegramUser {
    pub id: i64,
    #[serde(default)]
    pub is_bot: bool,
    pub username: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TelegramUserProfilePhotos {
    pub total_count: u32,
    pub photos: Vec<Vec<TelegramPhotoSize>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TelegramChat {
    pub id: i64,
    #[serde(rename = "type")]
    pub kind: String,
    pub title: Option<String>,
    pub username: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TelegramPhotoSize {
    pub file_id: String,
    pub file_unique_id: String,
    pub width: u32,
    pub height: u32,
    pub file_size: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TelegramMediaFile {
    pub file_id: String,
    pub file_unique_id: String,
    pub file_size: Option<u64>,
    pub file_name: Option<String>,
    pub mime_type: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TelegramStickerFile {
    pub file_id: String,
    pub file_unique_id: String,
    pub file_size: Option<u64>,
    #[serde(default)]
    pub is_animated: bool,
    #[serde(default)]
    pub is_video: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TelegramFile {
    pub file_id: String,
    pub file_unique_id: String,
    pub file_size: Option<u64>,
    pub file_path: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub enum TelegramParseMode {
    #[serde(rename = "MarkdownV2")]
    MarkdownV2,
    #[serde(rename = "HTML")]
    Html,
}

#[derive(Debug, Clone, Serialize)]
pub struct TelegramBotCommand {
    pub command: String,
    pub description: String,
}

/// Rich message content accepted by Telegram's `sendRichMessage` and
/// `editMessageText` methods.
///
/// The content constructors keep the API invariant that exactly one of
/// `markdown`, `html`, or `blocks` is serialized.
#[derive(Debug, Clone, Serialize)]
pub struct TelegramInputRichMessage {
    #[serde(flatten)]
    content: TelegramInputRichMessageContent,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    media: Vec<TelegramInputRichMessageMedia>,
    #[serde(skip_serializing_if = "Option::is_none")]
    is_rtl: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    skip_entity_detection: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
enum TelegramInputRichMessageContent {
    Markdown { markdown: String },
    Html { html: String },
    Blocks { blocks: Vec<serde_json::Value> },
}

#[derive(Debug, Clone, Serialize)]
pub struct TelegramInputRichMessageMedia {
    pub id: String,
    pub media: serde_json::Value,
}

impl TelegramInputRichMessage {
    pub fn markdown(markdown: impl Into<String>) -> Self {
        Self::new(TelegramInputRichMessageContent::Markdown {
            markdown: markdown.into(),
        })
    }

    pub fn html(html: impl Into<String>) -> Self {
        Self::new(TelegramInputRichMessageContent::Html { html: html.into() })
    }

    pub fn blocks(blocks: Vec<serde_json::Value>) -> Self {
        Self::new(TelegramInputRichMessageContent::Blocks { blocks })
    }

    pub fn with_media(mut self, media: Vec<TelegramInputRichMessageMedia>) -> Self {
        self.media = media;
        self
    }

    pub fn with_rtl(mut self, is_rtl: bool) -> Self {
        self.is_rtl = Some(is_rtl);
        self
    }

    pub fn with_skip_entity_detection(mut self, skip_entity_detection: bool) -> Self {
        self.skip_entity_detection = Some(skip_entity_detection);
        self
    }

    fn new(content: TelegramInputRichMessageContent) -> Self {
        Self {
            content,
            media: Vec::new(),
            is_rtl: None,
            skip_entity_detection: None,
        }
    }
}

impl TelegramInputRichMessageMedia {
    pub fn new(id: impl Into<String>, media: serde_json::Value) -> Self {
        Self {
            id: id.into(),
            media,
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error(
    "telegram api {method} failed: status={status} error_code={error_code:?} description={description}"
)]
pub struct TelegramApiError {
    pub method: String,
    pub status: StatusCode,
    pub error_code: Option<i64>,
    pub description: String,
    pub retry_after: Option<u64>,
}

impl TelegramApiError {
    pub fn is_conflict(&self) -> bool {
        self.error_code == Some(409)
    }

    pub fn is_message_not_modified(&self) -> bool {
        self.error_code == Some(400)
            && self
                .description
                .to_ascii_lowercase()
                .contains("message is not modified")
    }

    /// `editForumTopic` returns this error when the topic exists but the
    /// requested edit would not change anything. It is a successful probe.
    pub fn is_topic_not_modified(&self) -> bool {
        if !self.method.eq_ignore_ascii_case("editForumTopic") || self.error_code != Some(400) {
            return false;
        }
        let description = self.description.to_ascii_lowercase();
        description.contains("topic_not_modified") || description.contains("topic is not modified")
    }

    /// Match only Telegram errors that identify a missing forum topic. Other
    /// 400/403/network errors must remain inconclusive so callers do not
    /// create duplicate topics or discard a valid binding.
    pub fn is_forum_topic_missing(&self) -> bool {
        if !self.method.eq_ignore_ascii_case("editForumTopic") {
            return false;
        }
        let description = self.description.to_ascii_lowercase();
        self.error_code == Some(400)
            && (description.contains("topic_id_invalid")
                || description.contains("topic id is invalid")
                || description.contains("message thread not found")
                || description.contains("message thread id is invalid")
                || description.contains("topic not found"))
    }

    pub fn is_edit_target_unavailable(&self) -> bool {
        if self.error_code != Some(400) {
            return false;
        }
        let description = self.description.to_ascii_lowercase();
        description.contains("message to edit not found")
            || description.contains("message can't be edited")
            || description.contains("message identifier is not specified")
    }

    pub fn should_fallback_from_rich_message(&self) -> bool {
        let method = self.method.trim().to_ascii_lowercase();
        let description = self.description.to_ascii_lowercase();
        let is_not_found = self.error_code == Some(404) || self.status == StatusCode::NOT_FOUND;

        // Older Bot API servers do not know the rich message endpoints at all.
        // Restrict 404 fallback to those endpoints so chat errors are preserved.
        if method == "sendrichmessage" && is_not_found {
            return true;
        }

        let is_bad_request = self.error_code == Some(400) || self.status == StatusCode::BAD_REQUEST;
        if !is_bad_request {
            return false;
        }

        let explicitly_rich = description.contains("rich_message")
            || description.contains("rich message")
            || description.contains("sendrichmessage");
        let rich_format_error = description.contains("can't parse entities")
            || description.contains("cannot parse entities")
            || description.contains("can't find end of the entity")
            || description.contains("unsupported start tag")
            || description.contains("unsupported end tag");
        let rich_blocks_capability_error = method == "sendrichmessage"
            && (description.contains("blocks")
                || description.contains("inputrichblock")
                || description.contains("unsupported rich")
                || description.contains("unknown field"));
        let unsupported_rich_edit =
            method == "editmessagetext" && description.contains("message text is empty");

        explicitly_rich
            || rich_format_error
            || rich_blocks_capability_error
            || unsupported_rich_edit
    }

    pub fn should_fallback_from_message_draft(&self) -> bool {
        if !self.method.eq_ignore_ascii_case("sendMessageDraft") {
            return false;
        }

        if self.error_code == Some(404) || self.status == StatusCode::NOT_FOUND {
            return true;
        }

        let is_bad_request = self.error_code == Some(400) || self.status == StatusCode::BAD_REQUEST;
        if !is_bad_request {
            return false;
        }
        let description = self.description.to_ascii_lowercase();
        description.contains("message text is empty")
            || description.contains("private chat")
            || description.contains("method is not available")
            || description.contains("method not found")
            || description.contains("not supported")
    }

    pub fn should_fallback_from_rich_message_draft(&self) -> bool {
        if !self.method.eq_ignore_ascii_case("sendRichMessageDraft") {
            return false;
        }

        // Older Bot API servers do not know the rich draft endpoint at all.
        if self.error_code == Some(404) || self.status == StatusCode::NOT_FOUND {
            return true;
        }

        let is_bad_request = self.error_code == Some(400) || self.status == StatusCode::BAD_REQUEST;
        if !is_bad_request {
            return false;
        }
        let description = self.description.to_ascii_lowercase();
        description.contains("rich_message")
            || description.contains("rich message")
            || description.contains("sendrichmessagedraft")
            || description.contains("inputrichblock")
            || description.contains("thinking")
            || description.contains("unsupported")
            || description.contains("not supported")
            || description.contains("method is not available")
            || description.contains("method not found")
            || description.contains("private chat")
    }
}

impl TelegramApi {
    pub fn new(settings: TelegramSettings) -> Self {
        Self { settings }
    }

    pub fn is_configured(&self) -> bool {
        !self.settings.bot_token.trim().is_empty()
    }

    pub async fn get_updates(
        &self,
        offset: Option<i64>,
        timeout_seconds: u32,
    ) -> Result<Vec<TelegramUpdate>> {
        let mut body = serde_json::json!({
            "timeout": timeout_seconds,
            "allowed_updates": ["message", "callback_query"],
        });
        if let Some(offset) = offset {
            body["offset"] = serde_json::json!(offset);
        }
        self.post("getUpdates", &body).await
    }

    pub async fn set_my_commands(&self, commands: &[TelegramBotCommand]) -> Result<()> {
        let _: bool = self
            .post(
                "setMyCommands",
                &serde_json::json!({ "commands": commands }),
            )
            .await?;
        Ok(())
    }

    /// Create a forum topic in a configured Telegram supergroup.
    pub async fn create_forum_topic(
        &self,
        chat_id: &str,
        name: &str,
    ) -> Result<TelegramForumTopic> {
        let body = create_forum_topic_body(chat_id, name);
        self.post("createForumTopic", &body).await
    }

    /// Remove a forum topic that was created for a session whose binding
    /// could not be completed. This keeps retries from leaving orphan topics.
    pub async fn delete_forum_topic(&self, chat_id: &str, message_thread_id: i64) -> Result<bool> {
        let body = delete_forum_topic_body(chat_id, message_thread_id);
        self.post("deleteForumTopic", &body).await
    }

    /// Probe whether a forum topic still exists by applying its expected name.
    /// Telegram reports `TOPIC_NOT_MODIFIED` when the topic already has that
    /// name, and a topic-id error when it no longer exists.
    pub async fn edit_forum_topic(
        &self,
        chat_id: &str,
        message_thread_id: i64,
        name: &str,
    ) -> Result<bool> {
        let body = edit_forum_topic_body(chat_id, message_thread_id, name);
        match self.post("editForumTopic", &body).await {
            Ok(result) => Ok(result),
            Err(err)
                if err
                    .downcast_ref::<TelegramApiError>()
                    .is_some_and(|error| error.is_topic_not_modified()) =>
            {
                Ok(true)
            }
            Err(err) => Err(err),
        }
    }

    pub async fn send_text(&self, chat_id: &str, text: &str) -> Result<i64> {
        let mut body = serde_json::json!({
            "text": text,
            "disable_web_page_preview": true,
        });
        add_message_target(&mut body, chat_id);
        let message: TelegramMessage = self.post("sendMessage", &body).await?;
        Ok(message.message_id)
    }

    pub async fn send_text_parse_mode(
        &self,
        chat_id: &str,
        text: &str,
        parse_mode: TelegramParseMode,
    ) -> Result<i64> {
        let mut body = serde_json::json!({
            "text": text,
            "parse_mode": parse_mode,
            "disable_web_page_preview": true,
        });
        add_message_target(&mut body, chat_id);
        let message: TelegramMessage = self.post("sendMessage", &body).await?;
        Ok(message.message_id)
    }

    pub async fn send_text_with_reply_markup(
        &self,
        chat_id: &str,
        text: &str,
        reply_markup: serde_json::Value,
    ) -> Result<i64> {
        let mut body = serde_json::json!({
            "text": text,
            "disable_web_page_preview": true,
            "reply_markup": reply_markup,
        });
        add_message_target(&mut body, chat_id);
        let message: TelegramMessage = self.post("sendMessage", &body).await?;
        Ok(message.message_id)
    }

    pub async fn send_text_with_reply_markup_parse_mode(
        &self,
        chat_id: &str,
        text: &str,
        reply_markup: serde_json::Value,
        parse_mode: TelegramParseMode,
    ) -> Result<i64> {
        let mut body = serde_json::json!({
            "text": text,
            "parse_mode": parse_mode,
            "disable_web_page_preview": true,
            "reply_markup": reply_markup,
        });
        add_message_target(&mut body, chat_id);
        let message: TelegramMessage = self.post("sendMessage", &body).await?;
        Ok(message.message_id)
    }

    pub async fn send_rich_message(
        &self,
        chat_id: &str,
        rich_message: &TelegramInputRichMessage,
    ) -> Result<i64> {
        let body = send_rich_message_body(chat_id, rich_message, None);
        let message: TelegramMessage = self.post("sendRichMessage", &body).await?;
        Ok(message.message_id)
    }

    pub async fn send_rich_message_with_reply_markup(
        &self,
        chat_id: &str,
        rich_message: &TelegramInputRichMessage,
        reply_markup: serde_json::Value,
    ) -> Result<i64> {
        let body = send_rich_message_body(chat_id, rich_message, Some(reply_markup));
        let message: TelegramMessage = self.post("sendRichMessage", &body).await?;
        Ok(message.message_id)
    }

    /// Edit a bot-authored text message in place.
    ///
    /// Telegram returns the edited message for regular chat messages.  The
    /// optional reply markup is included in the same request so callers can
    /// atomically replace or remove inline buttons while changing the text.
    pub async fn edit_message_text(
        &self,
        chat_id: &str,
        message_id: i64,
        text: &str,
        parse_mode: Option<TelegramParseMode>,
        reply_markup: Option<serde_json::Value>,
    ) -> Result<i64> {
        let body = edit_message_text_body(chat_id, message_id, text, parse_mode, reply_markup);
        let message: TelegramMessage = self.post("editMessageText", &body).await?;
        Ok(message.message_id)
    }

    /// Edit a bot-authored rich message in place through `editMessageText`.
    pub async fn edit_rich_message(
        &self,
        chat_id: &str,
        message_id: i64,
        rich_message: &TelegramInputRichMessage,
        reply_markup: Option<serde_json::Value>,
    ) -> Result<i64> {
        let body = edit_rich_message_body(chat_id, message_id, rich_message, reply_markup);
        let message: TelegramMessage = self.post("editMessageText", &body).await?;
        Ok(message.message_id)
    }

    /// Replace the inline keyboard attached to an existing message.
    pub async fn edit_message_reply_markup(
        &self,
        chat_id: &str,
        message_id: i64,
        reply_markup: serde_json::Value,
    ) -> Result<i64> {
        let body = edit_message_reply_markup_body(chat_id, message_id, reply_markup);
        let message: TelegramMessage = self.post("editMessageReplyMarkup", &body).await?;
        Ok(message.message_id)
    }

    pub async fn get_file(&self, file_id: &str) -> Result<TelegramFile> {
        self.post("getFile", &serde_json::json!({ "file_id": file_id }))
            .await
    }

    pub async fn download_file(&self, file_path: &str) -> Result<Vec<u8>> {
        if !self.is_configured() {
            return Err(anyhow!("telegram bot_token is empty"));
        }
        let file_path = file_path.trim().trim_start_matches('/');
        if file_path.is_empty() || file_path.split('/').any(|segment| segment == "..") {
            return Err(anyhow!("telegram getFile returned an invalid file_path"));
        }
        let url = format!(
            "{TELEGRAM_API_BASE}/file/bot{}/{}",
            self.settings.bot_token.trim(),
            file_path
        );
        let response = crate::outbound_http::get()
            .get(url)
            .send()
            .await
            .map_err(|err| {
                let message = self.sanitize_error(&err.to_string());
                anyhow!("telegram file download request failed: {message}")
            })?;
        let status = response.status();
        if !status.is_success() {
            return Err(anyhow!("telegram file download failed: status={status}"));
        }
        if response
            .content_length()
            .is_some_and(|bytes| bytes > TELEGRAM_MAX_DOWNLOAD_BYTES)
        {
            return Err(anyhow!("telegram file download exceeds the 20 MB limit"));
        }
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("failed to read telegram file download response")?;
            if bytes.len().saturating_add(chunk.len()) > TELEGRAM_MAX_DOWNLOAD_BYTES as usize {
                return Err(anyhow!("telegram file download exceeds the 20 MB limit"));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }

    pub async fn send_photo_file(
        &self,
        chat_id: &str,
        local_path: &Path,
        caption: Option<&str>,
        parse_mode: Option<TelegramParseMode>,
    ) -> Result<i64> {
        self.post_media_file(
            "sendPhoto",
            chat_id,
            "photo",
            local_path,
            caption,
            parse_mode,
        )
        .await
    }

    pub async fn send_document_file(
        &self,
        chat_id: &str,
        local_path: &Path,
        caption: Option<&str>,
        parse_mode: Option<TelegramParseMode>,
    ) -> Result<i64> {
        self.post_media_file(
            "sendDocument",
            chat_id,
            "document",
            local_path,
            caption,
            parse_mode,
        )
        .await
    }

    pub async fn answer_callback_query(
        &self,
        callback_query_id: &str,
        text: Option<&str>,
    ) -> Result<()> {
        let mut body = serde_json::json!({
            "callback_query_id": callback_query_id,
        });
        if let Some(text) = text.map(str::trim).filter(|value| !value.is_empty()) {
            body["text"] = serde_json::json!(text);
        }
        let _: bool = self.post("answerCallbackQuery", &body).await?;
        Ok(())
    }

    pub async fn send_chat_action(&self, chat_id: &str, action: &str) -> Result<()> {
        let body = send_chat_action_body(chat_id, action);
        let _: bool = self.post("sendChatAction", &body).await?;
        Ok(())
    }

    pub async fn send_message_draft(&self, chat_id: i64, draft_id: i64, text: &str) -> Result<()> {
        if draft_id == 0 {
            return Err(anyhow!("telegram message draft id must be non-zero"));
        }
        let body = send_message_draft_body(chat_id, draft_id, text);
        let _: bool = self.post("sendMessageDraft", &body).await?;
        Ok(())
    }

    pub async fn send_message_draft_to_target(
        &self,
        target: &str,
        draft_id: i64,
        text: &str,
    ) -> Result<()> {
        if draft_id == 0 {
            return Err(anyhow!("telegram message draft id must be non-zero"));
        }
        let (chat_id, _) = split_telegram_message_target(target);
        let chat_id = chat_id
            .trim()
            .parse::<i64>()
            .context("telegram message draft target must contain a numeric chat id")?;
        let body = send_message_draft_body_for_target(target, chat_id, draft_id, text);
        let _: bool = self.post("sendMessageDraft", &body).await?;
        Ok(())
    }

    pub async fn send_rich_message_draft(
        &self,
        chat_id: i64,
        draft_id: i64,
        rich_message: &TelegramInputRichMessage,
    ) -> Result<()> {
        if draft_id == 0 {
            return Err(anyhow!("telegram rich message draft id must be non-zero"));
        }
        let body = send_rich_message_draft_body(chat_id, draft_id, rich_message);
        let _: bool = self.post("sendRichMessageDraft", &body).await?;
        Ok(())
    }

    pub async fn send_rich_message_draft_to_target(
        &self,
        target: &str,
        draft_id: i64,
        rich_message: &TelegramInputRichMessage,
    ) -> Result<()> {
        if draft_id == 0 {
            return Err(anyhow!("telegram rich message draft id must be non-zero"));
        }
        let (chat_id, _) = split_telegram_message_target(target);
        let chat_id = chat_id
            .trim()
            .parse::<i64>()
            .context("telegram rich message draft target must contain a numeric chat id")?;
        let body = send_rich_message_draft_body_for_target(target, chat_id, draft_id, rich_message);
        let _: bool = self.post("sendRichMessageDraft", &body).await?;
        Ok(())
    }

    pub async fn get_me(&self) -> Result<TelegramUser> {
        self.post("getMe", &serde_json::json!({})).await
    }

    pub async fn get_user_profile_photos(
        &self,
        user_id: i64,
        limit: u8,
    ) -> Result<TelegramUserProfilePhotos> {
        self.post(
            "getUserProfilePhotos",
            &serde_json::json!({
                "user_id": user_id,
                "limit": limit.clamp(1, 100),
            }),
        )
        .await
    }

    pub fn settings(&self) -> &TelegramSettings {
        &self.settings
    }

    async fn post<T>(&self, method: &str, body: &serde_json::Value) -> Result<T>
    where
        T: for<'de> Deserialize<'de>,
    {
        if !self.is_configured() {
            return Err(anyhow!("telegram bot_token is empty"));
        }
        let url = format!(
            "{TELEGRAM_API_BASE}/bot{}/{}",
            self.settings.bot_token.trim(),
            method
        );
        let response = crate::outbound_http::get()
            .post(url)
            .json(body)
            .send()
            .await
            .map_err(|err| {
                let message = self.sanitize_error(&err.to_string());
                chain_log::write_line(format!(
                    "[telegram_api] event=request_failed method={} err={}",
                    method, message
                ));
                anyhow!("telegram api {method} request failed: {}", message)
            })?;
        let status = response.status();
        let payload: TelegramResponse<T> = response
            .json()
            .await
            .with_context(|| format!("failed to decode telegram api {method} response"))?;
        if !status.is_success() || !payload.ok {
            let description = payload.description.unwrap_or_default();
            let retry_after = payload
                .parameters
                .and_then(|parameters| parameters.retry_after);
            chain_log::write_line(format!(
                "[telegram_api] event=response_error method={} status={} error_code={:?} retry_after={:?} description={}",
                method, status, payload.error_code, retry_after, description
            ));
            return Err(TelegramApiError {
                method: method.to_string(),
                status,
                error_code: payload.error_code,
                description,
                retry_after,
            }
            .into());
        }
        payload
            .result
            .ok_or_else(|| anyhow!("telegram api {method} returned empty result"))
    }

    async fn post_media_file(
        &self,
        method: &str,
        chat_id: &str,
        field_name: &str,
        local_path: &Path,
        caption: Option<&str>,
        parse_mode: Option<TelegramParseMode>,
    ) -> Result<i64> {
        if !self.is_configured() {
            return Err(anyhow!("telegram bot_token is empty"));
        }
        let bytes = std::fs::read(local_path)
            .with_context(|| format!("failed to read {}", local_path.display()))?;
        let file_name = local_path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("image.png")
            .to_string();
        let mime = mime_guess::from_path(local_path)
            .first_or_octet_stream()
            .to_string();
        let part = multipart::Part::bytes(bytes)
            .file_name(file_name)
            .mime_str(&mime)
            .with_context(|| format!("invalid mime type for {}", local_path.display()))?;
        let (raw_chat_id, topic_id) = split_telegram_message_target(chat_id);
        let mut form = multipart::Form::new()
            .text("chat_id", raw_chat_id.to_string())
            .part(field_name.to_string(), part);
        if let Some(topic_id) = topic_id {
            form = form.text("message_thread_id", topic_id.to_string());
        }
        if let Some(caption) = caption.map(str::trim).filter(|value| !value.is_empty()) {
            form = form.text("caption", caption.to_string());
        }
        if let Some(parse_mode) = parse_mode {
            let parse_mode = serde_json::to_value(parse_mode)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_else(|| "HTML".to_string());
            form = form.text("parse_mode", parse_mode);
        }

        let url = format!(
            "{TELEGRAM_API_BASE}/bot{}/{}",
            self.settings.bot_token.trim(),
            method
        );
        let response = crate::outbound_http::get()
            .post(url)
            .multipart(form)
            .send()
            .await
            .map_err(|err| {
                let message = self.sanitize_error(&err.to_string());
                chain_log::write_line(format!(
                    "[telegram_api] event=request_failed method={} err={}",
                    method, message
                ));
                anyhow!("telegram api {method} request failed: {}", message)
            })?;
        let status = response.status();
        let payload: TelegramResponse<TelegramMessage> = response
            .json()
            .await
            .with_context(|| format!("failed to decode telegram api {method} response"))?;
        if !status.is_success() || !payload.ok {
            let description = payload.description.unwrap_or_default();
            let retry_after = payload
                .parameters
                .and_then(|parameters| parameters.retry_after);
            chain_log::write_line(format!(
                "[telegram_api] event=response_error method={} status={} error_code={:?} retry_after={:?} description={}",
                method, status, payload.error_code, retry_after, description
            ));
            return Err(TelegramApiError {
                method: method.to_string(),
                status,
                error_code: payload.error_code,
                description,
                retry_after,
            }
            .into());
        }
        payload
            .result
            .map(|message| message.message_id)
            .ok_or_else(|| anyhow!("telegram api {method} returned empty result"))
    }

    fn sanitize_error(&self, message: &str) -> String {
        let token = self.settings.bot_token.trim();
        if token.is_empty() {
            message.to_string()
        } else {
            message.replace(token, "***")
        }
    }
}

/// Build the JSON payload separately from the network call so it can be
/// validated without a Telegram token or an HTTP server in unit tests.
fn edit_message_text_body(
    chat_id: &str,
    message_id: i64,
    text: &str,
    parse_mode: Option<TelegramParseMode>,
    reply_markup: Option<serde_json::Value>,
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "message_id": message_id,
        "text": text,
        "disable_web_page_preview": true,
    });
    add_message_target(&mut body, chat_id);
    if let Some(parse_mode) = parse_mode {
        body["parse_mode"] = serde_json::to_value(parse_mode)
            .unwrap_or_else(|_| serde_json::Value::String("HTML".to_string()));
    }
    if let Some(reply_markup) = reply_markup {
        body["reply_markup"] = reply_markup;
    }
    body
}

fn send_rich_message_body(
    chat_id: &str,
    rich_message: &TelegramInputRichMessage,
    reply_markup: Option<serde_json::Value>,
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "rich_message": rich_message,
    });
    add_message_target(&mut body, chat_id);
    if let Some(reply_markup) = reply_markup {
        body["reply_markup"] = reply_markup;
    }
    body
}

fn edit_rich_message_body(
    chat_id: &str,
    message_id: i64,
    rich_message: &TelegramInputRichMessage,
    reply_markup: Option<serde_json::Value>,
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "message_id": message_id,
        "rich_message": rich_message,
    });
    add_message_target(&mut body, chat_id);
    if let Some(reply_markup) = reply_markup {
        body["reply_markup"] = reply_markup;
    }
    body
}

fn edit_message_reply_markup_body(
    chat_id: &str,
    message_id: i64,
    reply_markup: serde_json::Value,
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "message_id": message_id,
        "reply_markup": reply_markup,
    });
    add_message_target(&mut body, chat_id);
    body
}

fn send_chat_action_body(chat_id: &str, action: &str) -> serde_json::Value {
    let mut body = serde_json::json!({
        "action": action,
    });
    add_message_target(&mut body, chat_id);
    body
}

fn create_forum_topic_body(chat_id: &str, name: &str) -> serde_json::Value {
    let mut body = serde_json::json!({
        "name": name.trim(),
    });
    add_message_target(&mut body, chat_id);
    body
}

fn delete_forum_topic_body(chat_id: &str, message_thread_id: i64) -> serde_json::Value {
    serde_json::json!({
        "chat_id": chat_id,
        "message_thread_id": message_thread_id,
    })
}

fn edit_forum_topic_body(chat_id: &str, message_thread_id: i64, name: &str) -> serde_json::Value {
    serde_json::json!({
        "chat_id": chat_id,
        "message_thread_id": message_thread_id,
        "name": name.trim(),
    })
}

fn send_message_draft_body(chat_id: i64, draft_id: i64, text: &str) -> serde_json::Value {
    serde_json::json!({
        "chat_id": chat_id,
        "draft_id": draft_id,
        "text": text,
    })
}

fn send_message_draft_body_for_target(
    target: &str,
    chat_id: i64,
    draft_id: i64,
    text: &str,
) -> serde_json::Value {
    let mut body = send_message_draft_body(chat_id, draft_id, text);
    add_message_thread_id(&mut body, target);
    body
}

fn send_rich_message_draft_body(
    chat_id: i64,
    draft_id: i64,
    rich_message: &TelegramInputRichMessage,
) -> serde_json::Value {
    serde_json::json!({
        "chat_id": chat_id,
        "draft_id": draft_id,
        "rich_message": rich_message,
    })
}

fn send_rich_message_draft_body_for_target(
    target: &str,
    chat_id: i64,
    draft_id: i64,
    rich_message: &TelegramInputRichMessage,
) -> serde_json::Value {
    let mut body = send_rich_message_draft_body(chat_id, draft_id, rich_message);
    add_message_thread_id(&mut body, target);
    body
}

fn add_message_target(body: &mut serde_json::Value, target: &str) {
    let (chat_id, _) = split_telegram_message_target(target);
    body["chat_id"] = serde_json::json!(chat_id);
    add_message_thread_id(body, target);
}

fn add_message_thread_id(body: &mut serde_json::Value, target: &str) {
    if let (_, Some(topic_id)) = split_telegram_message_target(target) {
        body["message_thread_id"] = serde_json::json!(topic_id);
    }
}

#[cfg(test)]
mod tests {
    use reqwest::StatusCode;

    use super::{
        TelegramApi, TelegramApiError, TelegramBotCommand, TelegramInputRichMessage,
        TelegramInputRichMessageMedia, TelegramParseMode, TelegramResponse, TelegramUpdate,
        create_forum_topic_body, delete_forum_topic_body, edit_forum_topic_body,
        edit_message_reply_markup_body, edit_message_text_body, edit_rich_message_body,
        send_chat_action_body, send_message_draft_body, send_rich_message_body,
        send_rich_message_draft_body,
    };
    use crate::im::telegram::types::TelegramSettings;

    fn api_error(
        method: &str,
        status: StatusCode,
        error_code: i64,
        description: &str,
    ) -> TelegramApiError {
        TelegramApiError {
            method: method.to_string(),
            status,
            error_code: Some(error_code),
            description: description.to_string(),
            retry_after: None,
        }
    }

    #[test]
    fn parses_message_and_callback_updates() {
        let raw = serde_json::json!({
            "ok": true,
            "result": [
                {
                    "update_id": 100,
                    "message": {
                        "message_id": 7,
                        "from": {
                            "id": 42,
                            "first_name": "Ada",
                            "username": "ada"
                        },
                        "chat": {
                            "id": -1001,
                            "type": "group",
                            "title": "Codex"
                        },
                        "text": "/status"
                    }
                },
                {
                    "update_id": 101,
                    "callback_query": {
                        "id": "cb-1",
                        "from": {
                            "id": 42,
                            "first_name": "Ada"
                        },
                        "message": {
                            "message_id": 8,
                            "chat": {
                                "id": -1001,
                                "type": "group",
                                "title": "Codex"
                            }
                        },
                        "data": "approve:number:7"
                    }
                }
            ]
        });
        let response: TelegramResponse<Vec<TelegramUpdate>> =
            serde_json::from_value(raw).expect("telegram response");
        let updates = response.result.expect("result");

        assert_eq!(updates[0].update_id, 100);
        assert_eq!(updates[0].message.as_ref().unwrap().chat.id, -1001);
        assert_eq!(
            updates[0].message.as_ref().unwrap().text.as_deref(),
            Some("/status")
        );
        assert_eq!(updates[1].update_id, 101);
        assert_eq!(
            updates[1].callback_query.as_ref().unwrap().data.as_deref(),
            Some("approve:number:7")
        );
    }

    #[test]
    fn parses_forum_topic_state_service_messages() {
        let raw = serde_json::json!({
            "update_id": 102,
            "message": {
                "message_id": 9,
                "message_thread_id": 17,
                "chat": {"id": -1001, "type": "supergroup"},
                "forum_topic_closed": {}
            }
        });
        let update: TelegramUpdate = serde_json::from_value(raw).expect("topic update");
        let message = update.message.expect("message");
        assert_eq!(message.message_thread_id, Some(17));
        assert!(message.forum_topic_closed.is_some());
        assert!(message.forum_topic_reopened.is_none());
    }

    #[test]
    fn parses_forum_topic_rename_service_messages() {
        let raw = serde_json::json!({
            "update_id": 103,
            "message": {
                "message_id": 10,
                "message_thread_id": 17,
                "chat": {"id": -1001, "type": "supergroup"},
                "forum_topic_edited": {"name": "新标题"}
            }
        });
        let update: TelegramUpdate = serde_json::from_value(raw).expect("topic rename update");
        assert_eq!(
            update
                .message
                .and_then(|message| message.forum_topic_edited)
                .and_then(|topic| topic.name),
            Some("新标题".to_string())
        );
    }

    #[test]
    fn builds_edit_message_text_payload_with_markup() {
        let body = edit_message_text_body(
            "42",
            7,
            "<b>done</b>",
            Some(TelegramParseMode::Html),
            Some(serde_json::json!({"inline_keyboard": []})),
        );

        assert_eq!(body["chat_id"], "42");
        assert_eq!(body["message_id"], 7);
        assert_eq!(body["text"], "<b>done</b>");
        assert_eq!(body["parse_mode"], "HTML");
        assert_eq!(
            body["reply_markup"]["inline_keyboard"],
            serde_json::json!([])
        );
        assert_eq!(body["disable_web_page_preview"], true);
    }

    #[test]
    fn adds_forum_topic_to_message_payloads() {
        let body = edit_message_text_body("-100|topic=17", 7, "done", None, None);
        assert_eq!(body["chat_id"], "-100");
        assert_eq!(body["message_thread_id"], 17);

        let body = send_chat_action_body("-100|topic=17", "typing");
        assert_eq!(body["chat_id"], "-100");
        assert_eq!(body["message_thread_id"], 17);
    }

    #[test]
    fn builds_forum_topic_creation_payload() {
        let body = create_forum_topic_body("-100", "  Fix routing  ");
        assert_eq!(body["chat_id"], "-100");
        assert_eq!(body["name"], "Fix routing");
        assert!(body.get("message_thread_id").is_none());
    }

    #[test]
    fn builds_forum_topic_deletion_payload() {
        let body = delete_forum_topic_body("-100", 17);
        assert_eq!(body["chat_id"], "-100");
        assert_eq!(body["message_thread_id"], 17);
    }

    #[test]
    fn builds_forum_topic_probe_payload() {
        let body = edit_forum_topic_body("-100", 17, "  Fix routing  ");
        assert_eq!(
            body,
            serde_json::json!({
                "chat_id": "-100",
                "message_thread_id": 17,
                "name": "Fix routing",
            })
        );
    }

    #[test]
    fn classifies_forum_topic_probe_errors_conservatively() {
        assert!(
            api_error(
                "editForumTopic",
                StatusCode::BAD_REQUEST,
                400,
                "Bad Request: TOPIC_ID_INVALID",
            )
            .is_forum_topic_missing()
        );
        assert!(
            api_error(
                "editForumTopic",
                StatusCode::BAD_REQUEST,
                400,
                "Bad Request: message thread not found",
            )
            .is_forum_topic_missing()
        );
        assert!(
            api_error(
                "editForumTopic",
                StatusCode::BAD_REQUEST,
                400,
                "Bad Request: TOPIC_NOT_MODIFIED",
            )
            .is_topic_not_modified()
        );
        assert!(
            !api_error(
                "editForumTopic",
                StatusCode::FORBIDDEN,
                403,
                "Forbidden: not enough rights",
            )
            .is_forum_topic_missing()
        );
        assert!(
            !api_error(
                "editForumTopic",
                StatusCode::BAD_REQUEST,
                400,
                "Bad Request: chat not found",
            )
            .is_forum_topic_missing()
        );
        assert!(
            !api_error(
                "sendMessage",
                StatusCode::BAD_REQUEST,
                400,
                "Bad Request: TOPIC_ID_INVALID",
            )
            .is_forum_topic_missing()
        );
    }

    #[test]
    fn serializes_telegram_command_menu_without_slash_prefixes() {
        let commands = vec![
            TelegramBotCommand {
                command: "new".to_string(),
                description: "Create a new session".to_string(),
            },
            TelegramBotCommand {
                command: "stop".to_string(),
                description: "Interrupt the current task".to_string(),
            },
        ];
        let body = serde_json::json!({ "commands": commands });
        assert_eq!(
            body,
            serde_json::json!({
                "commands": [
                    {"command": "new", "description": "Create a new session"},
                    {"command": "stop", "description": "Interrupt the current task"},
                ]
            })
        );
        assert!(
            !body["commands"][0]["command"]
                .as_str()
                .unwrap()
                .starts_with('/')
        );
    }

    #[test]
    fn builds_edit_reply_markup_payload() {
        let body =
            edit_message_reply_markup_body("42", 7, serde_json::json!({"inline_keyboard": []}));

        assert_eq!(
            body,
            serde_json::json!({
                "chat_id": "42",
                "message_id": 7,
                "reply_markup": {"inline_keyboard": []},
            })
        );
    }

    #[test]
    fn builds_send_chat_action_payload() {
        assert_eq!(
            send_chat_action_body("42", "typing"),
            serde_json::json!({
                "chat_id": "42",
                "action": "typing",
            })
        );
    }

    #[test]
    fn builds_empty_message_draft_payload_for_native_thinking() {
        assert_eq!(
            send_message_draft_body(42, 7, ""),
            serde_json::json!({
                "chat_id": 42,
                "draft_id": 7,
                "text": "",
            })
        );
    }

    #[test]
    fn builds_rich_message_draft_payload_for_native_thinking() {
        let rich_message = TelegramInputRichMessage::html("<tg-thinking>Thinking...</tg-thinking>");
        assert_eq!(
            send_rich_message_draft_body(42, 7, &rich_message),
            serde_json::json!({
                "chat_id": 42,
                "draft_id": 7,
                "rich_message": {
                    "html": "<tg-thinking>Thinking...</tg-thinking>",
                },
            })
        );
    }

    #[tokio::test]
    async fn rejects_zero_message_draft_id_before_sending() {
        let api = TelegramApi::new(TelegramSettings::default());

        let error = api
            .send_message_draft(42, 0, "")
            .await
            .expect_err("zero draft id must be rejected locally");

        assert!(error.to_string().contains("must be non-zero"));
    }

    #[tokio::test]
    async fn rejects_zero_rich_message_draft_id_before_sending() {
        let api = TelegramApi::new(TelegramSettings::default());
        let rich_message = TelegramInputRichMessage::html("<tg-thinking>Thinking...</tg-thinking>");

        let error = api
            .send_rich_message_draft(42, 0, &rich_message)
            .await
            .expect_err("zero rich draft id must be rejected locally");

        assert!(error.to_string().contains("must be non-zero"));
    }

    #[test]
    fn message_draft_fallback_only_accepts_capability_errors() {
        assert!(
            api_error("sendMessageDraft", StatusCode::NOT_FOUND, 404, "Not Found",)
                .should_fallback_from_message_draft()
        );
        assert!(
            api_error(
                "sendMessageDraft",
                StatusCode::BAD_REQUEST,
                400,
                "Bad Request: method is available only for private chats",
            )
            .should_fallback_from_message_draft()
        );
        assert!(
            api_error(
                "sendMessageDraft",
                StatusCode::BAD_REQUEST,
                400,
                "Bad Request: message text is empty",
            )
            .should_fallback_from_message_draft()
        );
        assert!(
            !api_error(
                "sendMessageDraft",
                StatusCode::TOO_MANY_REQUESTS,
                429,
                "Too Many Requests",
            )
            .should_fallback_from_message_draft()
        );
        assert!(
            !api_error("sendMessage", StatusCode::NOT_FOUND, 404, "Not Found",)
                .should_fallback_from_message_draft()
        );
    }

    #[test]
    fn rich_message_draft_fallback_only_accepts_capability_errors() {
        assert!(
            api_error(
                "sendRichMessageDraft",
                StatusCode::NOT_FOUND,
                404,
                "Not Found",
            )
            .should_fallback_from_rich_message_draft()
        );
        assert!(
            api_error(
                "sendRichMessageDraft",
                StatusCode::BAD_REQUEST,
                400,
                "Bad Request: unsupported rich message block",
            )
            .should_fallback_from_rich_message_draft()
        );
        assert!(
            !api_error(
                "sendRichMessageDraft",
                StatusCode::TOO_MANY_REQUESTS,
                429,
                "Too Many Requests",
            )
            .should_fallback_from_rich_message_draft()
        );
        assert!(
            !api_error("sendRichMessage", StatusCode::NOT_FOUND, 404, "Not Found",)
                .should_fallback_from_rich_message_draft()
        );
    }

    #[test]
    fn serializes_each_rich_message_content_format_exclusively() {
        let markdown = serde_json::to_value(TelegramInputRichMessage::markdown("**done**"))
            .expect("markdown rich message must serialize");
        assert_eq!(markdown, serde_json::json!({"markdown": "**done**"}));

        let html = serde_json::to_value(TelegramInputRichMessage::html("<b>done</b>"))
            .expect("HTML rich message must serialize");
        assert_eq!(html, serde_json::json!({"html": "<b>done</b>"}));

        let blocks = serde_json::to_value(TelegramInputRichMessage::blocks(vec![
            serde_json::json!({"type": "divider"}),
        ]))
        .expect("block rich message must serialize");
        assert_eq!(blocks, serde_json::json!({"blocks": [{"type": "divider"}]}));
    }

    #[test]
    fn serializes_rich_message_options_and_media() {
        let rich_message = TelegramInputRichMessage::markdown("![result](tg://photo?id=result)")
            .with_media(vec![TelegramInputRichMessageMedia::new(
                "result",
                serde_json::json!({"type": "photo", "media": "file-id"}),
            )])
            .with_rtl(false)
            .with_skip_entity_detection(true);

        assert_eq!(
            serde_json::to_value(rich_message).expect("rich message must serialize"),
            serde_json::json!({
                "markdown": "![result](tg://photo?id=result)",
                "media": [{
                    "id": "result",
                    "media": {"type": "photo", "media": "file-id"},
                }],
                "is_rtl": false,
                "skip_entity_detection": true,
            })
        );
    }

    #[test]
    fn builds_send_rich_message_payload_with_markup() {
        let rich_message = TelegramInputRichMessage::markdown("```shell\npwd\n```");
        let body = send_rich_message_body(
            "42",
            &rich_message,
            Some(serde_json::json!({"inline_keyboard": []})),
        );

        assert_eq!(
            body,
            serde_json::json!({
                "chat_id": "42",
                "rich_message": {"markdown": "```shell\npwd\n```"},
                "reply_markup": {"inline_keyboard": []},
            })
        );
    }

    #[test]
    fn builds_edit_rich_message_payload_without_text_fields() {
        let rich_message =
            TelegramInputRichMessage::html("<pre><code class=\"language-shell\">pwd</code></pre>");
        let body = edit_rich_message_body("42", 7, &rich_message, None);

        assert_eq!(
            body,
            serde_json::json!({
                "chat_id": "42",
                "message_id": 7,
                "rich_message": {
                    "html": "<pre><code class=\"language-shell\">pwd</code></pre>",
                },
            })
        );
        assert!(body.get("text").is_none());
        assert!(body.get("parse_mode").is_none());
    }

    #[test]
    fn rich_message_fallback_only_accepts_capability_and_format_errors() {
        assert!(
            api_error("sendRichMessage", StatusCode::NOT_FOUND, 404, "Not Found",)
                .should_fallback_from_rich_message()
        );
        assert!(
            api_error(
                "sendRichMessage",
                StatusCode::BAD_REQUEST,
                400,
                "Bad Request: rich_message is not supported",
            )
            .should_fallback_from_rich_message()
        );
        assert!(
            api_error(
                "sendRichMessage",
                StatusCode::BAD_REQUEST,
                400,
                "Bad Request: blocks are not supported by this Bot API server",
            )
            .should_fallback_from_rich_message()
        );
        assert!(
            api_error(
                "editMessageText",
                StatusCode::BAD_REQUEST,
                400,
                "Bad Request: can't parse entities",
            )
            .should_fallback_from_rich_message()
        );
        assert!(
            api_error(
                "editMessageText",
                StatusCode::BAD_REQUEST,
                400,
                "Bad Request: message text is empty",
            )
            .should_fallback_from_rich_message()
        );

        assert!(
            !api_error(
                "sendRichMessage",
                StatusCode::BAD_REQUEST,
                400,
                "Bad Request: chat not found",
            )
            .should_fallback_from_rich_message()
        );
        assert!(
            !api_error("editMessageText", StatusCode::NOT_FOUND, 404, "Not Found",)
                .should_fallback_from_rich_message()
        );
        assert!(
            !api_error(
                "sendRichMessage",
                StatusCode::FORBIDDEN,
                403,
                "Forbidden: bot was blocked by the user",
            )
            .should_fallback_from_rich_message()
        );
    }
}
