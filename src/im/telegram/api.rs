use std::path::Path;

use anyhow::{Context, Result, anyhow};
use reqwest::{Client, StatusCode, multipart};
use serde::{Deserialize, Serialize};

use crate::chain_log;

use super::types::TelegramSettings;

const TELEGRAM_API_BASE: &str = "https://api.telegram.org";

#[derive(Debug, Clone)]
pub struct TelegramApi {
    settings: TelegramSettings,
    client: Client,
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

#[derive(Debug, Deserialize)]
pub struct TelegramMessage {
    pub message_id: i64,
    pub from: Option<TelegramUser>,
    pub chat: TelegramChat,
    pub text: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TelegramUser {
    pub id: i64,
    #[serde(default)]
    pub is_bot: bool,
    pub username: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TelegramChat {
    pub id: i64,
    #[serde(rename = "type")]
    pub kind: String,
    pub title: Option<String>,
    pub username: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
}

#[derive(Debug, Serialize)]
pub enum TelegramParseMode {
    #[serde(rename = "MarkdownV2")]
    MarkdownV2,
    #[serde(rename = "HTML")]
    Html,
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
}

impl TelegramApi {
    pub fn new(settings: TelegramSettings) -> Self {
        Self {
            settings,
            client: Client::new(),
        }
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

    pub async fn send_text(&self, chat_id: &str, text: &str) -> Result<i64> {
        let body = serde_json::json!({
            "chat_id": chat_id,
            "text": text,
            "disable_web_page_preview": true,
        });
        let message: TelegramMessage = self.post("sendMessage", &body).await?;
        Ok(message.message_id)
    }

    pub async fn send_text_parse_mode(
        &self,
        chat_id: &str,
        text: &str,
        parse_mode: TelegramParseMode,
    ) -> Result<i64> {
        let body = serde_json::json!({
            "chat_id": chat_id,
            "text": text,
            "parse_mode": parse_mode,
            "disable_web_page_preview": true,
        });
        let message: TelegramMessage = self.post("sendMessage", &body).await?;
        Ok(message.message_id)
    }

    pub async fn send_text_with_reply_markup(
        &self,
        chat_id: &str,
        text: &str,
        reply_markup: serde_json::Value,
    ) -> Result<i64> {
        let body = serde_json::json!({
            "chat_id": chat_id,
            "text": text,
            "disable_web_page_preview": true,
            "reply_markup": reply_markup,
        });
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
        let body = serde_json::json!({
            "chat_id": chat_id,
            "text": text,
            "parse_mode": parse_mode,
            "disable_web_page_preview": true,
            "reply_markup": reply_markup,
        });
        let message: TelegramMessage = self.post("sendMessage", &body).await?;
        Ok(message.message_id)
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
        let body = serde_json::json!({
            "chat_id": chat_id,
            "action": action,
        });
        let _: bool = self.post("sendChatAction", &body).await?;
        Ok(())
    }

    pub async fn get_me(&self) -> Result<TelegramUser> {
        self.post("getMe", &serde_json::json!({})).await
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
        let response = self
            .client
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
        let mut form = multipart::Form::new()
            .text("chat_id", chat_id.to_string())
            .part(field_name.to_string(), part);
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
        let response = self
            .client
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

#[cfg(test)]
mod tests {
    use super::{TelegramResponse, TelegramUpdate};

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
}
