use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow};
use axum::http::{HeaderMap, HeaderName, HeaderValue, header::CONTENT_TYPE};
use base64::{Engine as _, engine::general_purpose};
use reqwest::{Client, Url};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::chain_log;

use super::types::{
    DEFAULT_WECHAT_API_BASE, WechatGetUpdatesResponse, WechatQrCodeResponse,
    WechatQrStatusResponse, WechatSettings,
};

const QR_LONG_POLL_TIMEOUT_MS: u64 = 35_000;
const LONG_POLL_TIMEOUT_MS: u64 = 35_000;
const API_TIMEOUT_MS: u64 = 15_000;
const CONFIG_TIMEOUT_MS: u64 = 10_000;
const ILINK_APP_ID: &str = "bot";
const MESSAGE_TYPE_BOT: i64 = 2;
const MESSAGE_STATE_FINISH: i64 = 2;
const MESSAGE_ITEM_TYPE_TEXT: i64 = 1;

static WECHAT_CLIENT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone)]
pub struct WechatApi {
    settings: WechatSettings,
    client: Client,
}

impl WechatApi {
    pub fn new(settings: WechatSettings) -> Self {
        Self {
            settings,
            client: Client::new(),
        }
    }

    pub fn settings(&self) -> &WechatSettings {
        &self.settings
    }

    pub fn is_configured(&self) -> bool {
        self.settings.is_configured()
    }

    pub async fn start_qr_login(&self, local_tokens: &[String]) -> Result<WechatQrCodeResponse> {
        let bot_type = self.settings.bot_type();
        let endpoint = format!(
            "ilink/bot/get_bot_qrcode?bot_type={}",
            url_escape(&bot_type)
        );
        self.post_json_at_base(
            DEFAULT_WECHAT_API_BASE,
            &endpoint,
            json!({ "local_token_list": local_tokens }),
            None,
            Duration::from_millis(API_TIMEOUT_MS),
            "wechat_qr_start",
        )
        .await
    }

    pub async fn poll_qr_status(
        &self,
        base_url: &str,
        qrcode: &str,
        verify_code: Option<&str>,
    ) -> Result<WechatQrStatusResponse> {
        let mut endpoint = format!("ilink/bot/get_qrcode_status?qrcode={}", url_escape(qrcode));
        if let Some(verify_code) = verify_code.map(str::trim).filter(|value| !value.is_empty()) {
            endpoint.push_str("&verify_code=");
            endpoint.push_str(&url_escape(verify_code));
        }
        match self
            .get_json_at_base(
                base_url,
                &endpoint,
                Duration::from_millis(QR_LONG_POLL_TIMEOUT_MS),
                "wechat_qr_poll",
            )
            .await
        {
            Ok(response) => Ok(response),
            Err(err) if is_timeout_error(&err) => Ok(WechatQrStatusResponse {
                status: "wait".to_string(),
                bot_token: None,
                ilink_bot_id: None,
                baseurl: None,
                ilink_user_id: None,
                redirect_host: None,
            }),
            Err(err) => Err(err),
        }
    }

    pub async fn get_updates(
        &self,
        get_updates_buf: &str,
        timeout_ms: u64,
    ) -> Result<WechatGetUpdatesResponse> {
        if !self.is_configured() {
            return Err(anyhow!("wechat bot_token is empty"));
        }
        match self
            .post_json(
                "ilink/bot/getupdates",
                json!({
                    "get_updates_buf": get_updates_buf,
                    "base_info": base_info(),
                }),
                Duration::from_millis(timeout_ms.max(1)),
                "wechat_get_updates",
            )
            .await
        {
            Ok(response) => Ok(response),
            Err(err) if is_timeout_error(&err) => Ok(WechatGetUpdatesResponse {
                ret: Some(0),
                msgs: Some(Vec::new()),
                get_updates_buf: Some(get_updates_buf.to_string()),
                ..Default::default()
            }),
            Err(err) => Err(err),
        }
    }

    pub async fn send_text(
        &self,
        to_user_id: &str,
        context_token: Option<&str>,
        text: &str,
    ) -> Result<String> {
        if !self.is_configured() {
            return Err(anyhow!("wechat bot_token is empty"));
        }
        let client_id = next_client_id("codex-remote-wechat");
        let mut msg = json!({
            "from_user_id": "",
            "to_user_id": to_user_id,
            "client_id": client_id,
            "message_type": MESSAGE_TYPE_BOT,
            "message_state": MESSAGE_STATE_FINISH,
            "item_list": [{
                "type": MESSAGE_ITEM_TYPE_TEXT,
                "text_item": { "text": text },
            }],
        });
        if let Some(context_token) = context_token
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            msg["context_token"] = json!(context_token);
        }
        let _: Value = self
            .post_json(
                "ilink/bot/sendmessage",
                json!({
                    "msg": msg,
                    "base_info": base_info(),
                }),
                Duration::from_millis(API_TIMEOUT_MS),
                "wechat_send_message",
            )
            .await?;
        Ok(client_id)
    }

    pub async fn notify_start(&self) -> Result<()> {
        if !self.is_configured() {
            return Ok(());
        }
        let _: Value = self
            .post_json(
                "ilink/bot/msg/notifystart",
                json!({ "base_info": base_info() }),
                Duration::from_millis(CONFIG_TIMEOUT_MS),
                "wechat_notify_start",
            )
            .await?;
        Ok(())
    }

    async fn post_json<T>(
        &self,
        endpoint: &str,
        body: Value,
        timeout: Duration,
        label: &str,
    ) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let base_url = self.settings.api_base_url();
        self.post_json_at_base(
            &base_url,
            endpoint,
            body,
            self.settings.token(),
            timeout,
            label,
        )
        .await
    }

    async fn post_json_at_base<T>(
        &self,
        base_url: &str,
        endpoint: &str,
        body: Value,
        token: Option<&str>,
        timeout: Duration,
        label: &str,
    ) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let url = endpoint_url(base_url, endpoint)?;
        let body_text = serde_json::to_string(&body)?;
        chain_log::write_line(format!(
            "[wechat_api] event=request method=POST label={} url={} body_len={}",
            label,
            redact_url(url.as_str()),
            body_text.len()
        ));
        let response = self
            .client
            .post(url)
            .headers(build_headers(token)?)
            .body(body_text)
            .timeout(timeout)
            .send()
            .await
            .with_context(|| format!("wechat api {label} request failed"))?;
        decode_response(response, label).await
    }

    async fn get_json_at_base<T>(
        &self,
        base_url: &str,
        endpoint: &str,
        timeout: Duration,
        label: &str,
    ) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let url = endpoint_url(base_url, endpoint)?;
        chain_log::write_line(format!(
            "[wechat_api] event=request method=GET label={} url={}",
            label,
            redact_url(url.as_str())
        ));
        let response = self
            .client
            .get(url)
            .headers(build_common_headers()?)
            .timeout(timeout)
            .send()
            .await
            .with_context(|| format!("wechat api {label} request failed"))?;
        decode_response(response, label).await
    }
}

async fn decode_response<T>(response: reqwest::Response, label: &str) -> Result<T>
where
    T: DeserializeOwned,
{
    let status = response.status();
    let text = response
        .text()
        .await
        .with_context(|| format!("wechat api {label} response body read failed"))?;
    chain_log::write_line(format!(
        "[wechat_api] event=response label={} status={} body_len={}",
        label,
        status.as_u16(),
        text.len()
    ));
    if !status.is_success() {
        return Err(anyhow!(
            "wechat api {label} failed: status={} body={}",
            status,
            truncate_log(&text, 300)
        ));
    }
    serde_json::from_str(&text).with_context(|| {
        format!(
            "wechat api {label} response decode failed: {}",
            truncate_log(&text, 300)
        )
    })
}

fn build_headers(token: Option<&str>) -> Result<HeaderMap> {
    let mut headers = build_common_headers()?;
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(
        HeaderName::from_static("authorizationtype"),
        HeaderValue::from_static("ilink_bot_token"),
    );
    headers.insert(
        HeaderName::from_static("x-wechat-uin"),
        HeaderValue::from_str(&random_wechat_uin())?,
    );
    if let Some(token) = token.map(str::trim).filter(|value| !value.is_empty()) {
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&format!("Bearer {token}"))?,
        );
    }
    Ok(headers)
}

fn build_common_headers() -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert(
        HeaderName::from_static("ilink-app-id"),
        HeaderValue::from_static(ILINK_APP_ID),
    );
    headers.insert(
        HeaderName::from_static("ilink-app-clientversion"),
        HeaderValue::from_str(&client_version().to_string())?,
    );
    Ok(headers)
}

fn base_info() -> Value {
    json!({
        "channel_version": env!("CARGO_PKG_VERSION"),
        "bot_agent": format!("CodexRemote/{}", env!("CARGO_PKG_VERSION")),
    })
}

fn endpoint_url(base_url: &str, endpoint: &str) -> Result<Url> {
    let base = if base_url.trim().is_empty() {
        DEFAULT_WECHAT_API_BASE
    } else {
        base_url.trim()
    };
    let base = if base.ends_with('/') {
        base.to_string()
    } else {
        format!("{base}/")
    };
    Url::parse(&base)
        .with_context(|| format!("invalid wechat base url `{base}`"))?
        .join(endpoint)
        .with_context(|| format!("invalid wechat endpoint `{endpoint}`"))
}

fn client_version() -> u32 {
    let parts = env!("CARGO_PKG_VERSION")
        .split('.')
        .map(|part| part.parse::<u32>().unwrap_or(0))
        .collect::<Vec<_>>();
    let major = parts.first().copied().unwrap_or(0) & 0xff;
    let minor = parts.get(1).copied().unwrap_or(0) & 0xff;
    let patch = parts.get(2).copied().unwrap_or(0) & 0xff;
    (major << 16) | (minor << 8) | patch
}

fn random_wechat_uin() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos() as u64)
        .unwrap_or(0);
    let seq = WECHAT_CLIENT_ID.fetch_add(1, Ordering::Relaxed);
    let value = ((now ^ seq.rotate_left(13)) & 0xffff_ffff).to_string();
    general_purpose::STANDARD.encode(value.as_bytes())
}

fn next_client_id(prefix: &str) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis())
        .unwrap_or(0);
    let seq = WECHAT_CLIENT_ID.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{now}-{seq}")
}

fn is_timeout_error(err: &anyhow::Error) -> bool {
    err.downcast_ref::<reqwest::Error>()
        .is_some_and(|err| err.is_timeout())
        || err.to_string().contains("operation timed out")
}

fn url_escape(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

fn redact_url(value: &str) -> String {
    value
        .replace("qrcode=", "qrcode=***")
        .replace("verify_code=", "verify_code=***")
}

fn truncate_log(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut output = text
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect::<String>();
    output.push_str("...");
    output
}

pub(crate) fn default_long_poll_timeout_ms() -> u64 {
    LONG_POLL_TIMEOUT_MS
}
