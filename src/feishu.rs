use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow};
use futures_util::{SinkExt, StreamExt};
use prost::Message as ProstMessage;
use reqwest::StatusCode;
use serde::Deserialize;
use tokio::sync::{Mutex, RwLock, mpsc};
use tokio_tungstenite::{connect_async, tungstenite::Message as WsMessage};
use tracing::{info, warn};

use crate::{
    app_state::SharedState,
    config::FeishuConfig,
    types::{ChatType, ImPlatformKind, InboundAttachment, InboundMessage, now_ms},
};

const FEISHU_API_BASE: &str = "https://open.feishu.cn/open-apis";
const FEISHU_WS_BASE: &str = "https://open.feishu.cn";
const DEFAULT_TOKEN_TTL: Duration = Duration::from_secs(7200);
const TOKEN_REFRESH_SKEW: Duration = Duration::from_secs(120);
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(300);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

static HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
static TOKEN_CACHE: OnceLock<RwLock<HashMap<String, CachedTenantToken>>> = OnceLock::new();
static TOKEN_LOCKS: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();
static ATTACHMENT_SEQUENCE: AtomicU64 = AtomicU64::new(1);

fn http_client() -> &'static reqwest::Client {
    HTTP_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .pool_max_idle_per_host(8)
            .tcp_keepalive(Duration::from_secs(60))
            .build()
            .expect("failed to build feishu http client")
    })
}

fn token_cache() -> &'static RwLock<HashMap<String, CachedTenantToken>> {
    TOKEN_CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

fn token_locks() -> &'static Mutex<HashMap<String, Arc<Mutex<()>>>> {
    TOKEN_LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Debug, Clone)]
struct CachedTenantToken {
    value: String,
    refresh_after: Instant,
}

#[derive(Clone, PartialEq, prost::Message)]
struct PbHeader {
    #[prost(string, tag = "1")]
    pub key: String,
    #[prost(string, tag = "2")]
    pub value: String,
}

#[derive(Clone, PartialEq, prost::Message)]
struct PbFrame {
    #[prost(uint64, tag = "1")]
    seq_id: u64,
    #[prost(uint64, tag = "2")]
    log_id: u64,
    #[prost(int32, tag = "3")]
    service: i32,
    #[prost(int32, tag = "4")]
    method: i32,
    #[prost(message, repeated, tag = "5")]
    headers: Vec<PbHeader>,
    #[prost(bytes = "vec", optional, tag = "8")]
    payload: Option<Vec<u8>>,
}

impl PbFrame {
    fn header_value<'a>(&'a self, key: &str) -> &'a str {
        self.headers
            .iter()
            .find(|h| h.key == key)
            .map(|h| h.value.as_str())
            .unwrap_or("")
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct WsClientConfig {
    #[serde(rename = "PingInterval")]
    ping_interval: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct WsEndpointResp {
    code: i32,
    #[serde(default)]
    msg: Option<String>,
    #[serde(default)]
    data: Option<WsEndpoint>,
}

#[derive(Debug, Deserialize)]
struct WsEndpoint {
    #[serde(rename = "URL")]
    url: String,
    #[serde(rename = "ClientConfig")]
    client_config: Option<WsClientConfig>,
}

#[derive(Debug, Deserialize)]
struct FeishuWsEvent {
    header: FeishuEventHeader,
    event: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct FeishuEventHeader {
    event_type: String,
}

#[derive(Debug, Deserialize)]
struct MessageReceivePayload {
    sender: EventSender,
    message: EventMessage,
}

#[derive(Debug, Deserialize)]
struct EventSender {
    sender_id: SenderId,
    #[serde(default)]
    sender_type: String,
}

#[derive(Debug, Deserialize, Default)]
struct SenderId {
    open_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EventMessage {
    message_id: String,
    chat_id: String,
    chat_type: String,
    message_type: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    mentions: Vec<serde_json::Value>,
}

#[derive(Clone)]
pub struct FeishuApi {
    settings: FeishuConfig,
}

impl FeishuApi {
    pub fn new(settings: FeishuConfig) -> Self {
        Self { settings }
    }

    async fn get_tenant_access_token(&self) -> Result<String> {
        let app_id = self.settings.app_id.trim();
        let app_secret = self.settings.app_secret.trim();
        if app_id.is_empty() || app_secret.is_empty() {
            return Err(anyhow!("missing feishu app_id/app_secret"));
        }
        let cache_key = format!("{app_id}:{app_secret}");
        {
            let cache = token_cache().read().await;
            if let Some(token) = cache.get(&cache_key) {
                if Instant::now() < token.refresh_after {
                    return Ok(token.value.clone());
                }
            }
        }

        let lock = {
            let mut locks = token_locks().lock().await;
            locks
                .entry(cache_key.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _guard = lock.lock().await;
        {
            let cache = token_cache().read().await;
            if let Some(token) = cache.get(&cache_key) {
                if Instant::now() < token.refresh_after {
                    return Ok(token.value.clone());
                }
            }
        }

        let response = http_client()
            .post(format!(
                "{FEISHU_API_BASE}/auth/v3/tenant_access_token/internal"
            ))
            .json(&serde_json::json!({
                "app_id": app_id,
                "app_secret": app_secret,
            }))
            .send()
            .await
            .context("feishu tenant_access_token request failed")?;
        let status = response.status();
        let body: serde_json::Value = response.json().await?;
        ensure_success("tenant_access_token", status, &body)?;
        let token = body
            .get("tenant_access_token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("missing tenant_access_token"))?
            .to_string();
        let ttl = body
            .get("expire")
            .or_else(|| body.get("expires_in"))
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_TOKEN_TTL.as_secs())
            .max(1);
        let refresh_after = Instant::now()
            + Duration::from_secs(ttl)
                .checked_sub(TOKEN_REFRESH_SKEW)
                .unwrap_or(Duration::from_secs(1));
        token_cache().write().await.insert(
            cache_key,
            CachedTenantToken {
                value: token.clone(),
                refresh_after,
            },
        );
        Ok(token)
    }

    fn file_download_url(message_id: &str, file_key: &str) -> String {
        format!("{FEISHU_API_BASE}/im/v1/messages/{message_id}/resources/{file_key}?type=file")
    }

    fn image_resource_download_url(message_id: &str, image_key: &str) -> String {
        format!("{FEISHU_API_BASE}/im/v1/messages/{message_id}/resources/{image_key}?type=image")
    }

    async fn get_ws_endpoint(&self) -> Result<(String, WsClientConfig)> {
        let response = tokio::time::timeout(
            CONNECT_TIMEOUT,
            http_client()
                .post(format!("{FEISHU_WS_BASE}/callback/ws/endpoint"))
                .header("locale", "zh")
                .json(&serde_json::json!({
                    "AppID": self.settings.app_id,
                    "AppSecret": self.settings.app_secret,
                }))
                .send(),
        )
        .await
        .map_err(|_| anyhow!("feishu ws endpoint request timeout"))??;
        let response = tokio::time::timeout(CONNECT_TIMEOUT, response.json::<WsEndpointResp>())
            .await
            .map_err(|_| anyhow!("feishu ws endpoint response timeout"))??;
        if response.code != 0 {
            return Err(anyhow!(
                "feishu ws endpoint failed: code={} msg={}",
                response.code,
                response.msg.unwrap_or_default()
            ));
        }
        let endpoint = response
            .data
            .ok_or_else(|| anyhow!("feishu ws endpoint returned empty data"))?;
        Ok((endpoint.url, endpoint.client_config.unwrap_or_default()))
    }

    pub async fn start_app_registration() -> Result<serde_json::Value> {
        let response = http_client()
            .post("https://accounts.feishu.cn/oauth/v1/app/registration")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("User-Agent", "codex-remote")
            .form(&[
                ("action", "begin"),
                ("archetype", "PersonalAgent"),
                ("auth_method", "client_secret"),
                ("request_user_info", "open_id"),
            ])
            .send()
            .await?;
        let status = response.status();
        let payload: serde_json::Value = response.json().await?;
        if !status.is_success() {
            return Err(anyhow!(
                "feishu app registration begin failed: status={} body={}",
                status,
                payload
            ));
        }
        Ok(payload)
    }

    pub async fn poll_app_registration(device_code: &str) -> Result<serde_json::Value> {
        let response = http_client()
            .post("https://accounts.feishu.cn/oauth/v1/app/registration")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("User-Agent", "codex-remote")
            .form(&[("action", "poll"), ("device_code", device_code)])
            .send()
            .await?;
        let payload: serde_json::Value = response.json().await?;
        // 飞书在扫码确认前会用 400 + authorization_pending 表示继续等待。
        // 这里保持和 Arthas 一样：poll 接口只返回 payload，由上层判断状态。
        Ok(payload)
    }

    pub async fn get_application_display_name(&self, app_id: &str) -> Result<Option<String>> {
        let token = self.get_tenant_access_token().await?;
        let response = http_client()
            .get(format!(
                "{FEISHU_API_BASE}/application/v6/applications/{app_id}"
            ))
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .query(&[("lang", "zh_cn")])
            .send()
            .await?;
        let status = response.status();
        let payload: serde_json::Value = response.json().await?;
        ensure_success("application_info", status, &payload)?;
        let app = payload
            .get("data")
            .and_then(|data| data.get("app").or(Some(data)))
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        Ok(app
            .get("i18n_name")
            .and_then(|name| {
                name.get("zh_cn")
                    .and_then(|v| v.as_str())
                    .or_else(|| name.get("en_us").and_then(|v| v.as_str()))
                    .or_else(|| name.get("ja_jp").and_then(|v| v.as_str()))
            })
            .or_else(|| app.get("name").and_then(|v| v.as_str()))
            .or_else(|| app.get("app_name").and_then(|v| v.as_str()))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string))
    }

    pub async fn send_text_message(&self, chat_id: &str, text: &str) -> Result<()> {
        self.send_text_message_to("chat_id", chat_id, text).await
    }

    pub async fn send_text_message_to(
        &self,
        receive_id_type: &str,
        receive_id: &str,
        text: &str,
    ) -> Result<()> {
        let token = self.get_tenant_access_token().await?;
        let body = serde_json::json!({
            "receive_id": receive_id,
            "msg_type": "text",
            "content": serde_json::json!({ "text": text }).to_string(),
        });
        let response = http_client()
            .post(format!(
                "{FEISHU_API_BASE}/im/v1/messages?receive_id_type={receive_id_type}"
            ))
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&body)
            .send()
            .await?;
        let status = response.status();
        let body: serde_json::Value = response.json().await?;
        ensure_success("send_text_message", status, &body)?;
        Ok(())
    }

    pub async fn send_interactive_message(
        &self,
        chat_id: &str,
        card: &serde_json::Value,
    ) -> Result<String> {
        self.send_interactive_message_to("chat_id", chat_id, card)
            .await
    }

    pub async fn send_interactive_message_to(
        &self,
        receive_id_type: &str,
        receive_id: &str,
        card: &serde_json::Value,
    ) -> Result<String> {
        let token = self.get_tenant_access_token().await?;
        let body = serde_json::json!({
            "receive_id": receive_id,
            "msg_type": "interactive",
            "content": card.to_string(),
        });
        let response = http_client()
            .post(format!(
                "{FEISHU_API_BASE}/im/v1/messages?receive_id_type={receive_id_type}"
            ))
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&body)
            .send()
            .await?;
        let status = response.status();
        let body: serde_json::Value = response.json().await?;
        ensure_success("send_interactive_message", status, &body)?;
        body.get("data")
            .and_then(|v| v.get("message_id"))
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| anyhow!("feishu interactive send missing message_id"))
    }

    pub async fn update_interactive_message(
        &self,
        message_id: &str,
        card: &serde_json::Value,
    ) -> Result<()> {
        let token = self.get_tenant_access_token().await?;
        let body = serde_json::json!({
            "content": card.to_string(),
        });
        let response = http_client()
            .patch(format!("{FEISHU_API_BASE}/im/v1/messages/{message_id}"))
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&body)
            .send()
            .await?;
        let status = response.status();
        let body: serde_json::Value = response.json().await?;
        ensure_success("update_interactive_message", status, &body)?;
        Ok(())
    }

    pub async fn download_image(
        &self,
        message_id: &str,
        image_key: &str,
    ) -> Result<(Vec<u8>, String)> {
        let token = self.get_tenant_access_token().await?;
        let response = http_client()
            .get(Self::image_resource_download_url(message_id, image_key))
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            return Err(anyhow!("feishu image download failed: status={status}"));
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("image/png")
            .to_string();
        let bytes = response.bytes().await?.to_vec();
        Ok((bytes, content_type))
    }

    pub async fn download_file(&self, message_id: &str, file_key: &str) -> Result<Vec<u8>> {
        let token = self.get_tenant_access_token().await?;
        let response = http_client()
            .get(Self::file_download_url(message_id, file_key))
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            return Err(anyhow!("feishu file download failed: status={status}"));
        }
        Ok(response.bytes().await?.to_vec())
    }
}

pub async fn listen_ws(
    api: FeishuApi,
    account_id: String,
    attachment_root: PathBuf,
    tx: mpsc::Sender<InboundMessage>,
    state: SharedState,
) -> Result<()> {
    let (wss_url, client_config) = api.get_ws_endpoint().await?;
    let service_id = wss_url
        .split('?')
        .nth(1)
        .and_then(|qs| {
            qs.split('&')
                .find(|kv| kv.starts_with("service_id="))
                .and_then(|kv| kv.split('=').nth(1))
                .and_then(|v| v.parse::<i32>().ok())
        })
        .unwrap_or(0);
    info!(
        "connecting feishu ws account={} url={}",
        account_id, wss_url
    );
    let (stream, _) = tokio::time::timeout(CONNECT_TIMEOUT, connect_async(&wss_url))
        .await
        .map_err(|_| anyhow!("feishu websocket connect timeout"))??;
    let (mut write, mut read) = stream.split();
    {
        let mut ws = state.feishu_ws.lock().await;
        ws.connecting = false;
        ws.connected = true;
        ws.last_error = None;
    }
    state
        .push_event("info", "feishu_ws_connected", "websocket connected")
        .await;

    let mut ping_secs = client_config.ping_interval.unwrap_or(120).max(10);
    let mut hb_interval = tokio::time::interval(Duration::from_secs(ping_secs));
    let mut timeout_check = tokio::time::interval(Duration::from_secs(10));
    hb_interval.tick().await;
    let mut seq = 0_u64;
    let mut last_recv = Instant::now();

    send_ping(&mut write, service_id, &mut seq).await?;
    loop {
        tokio::select! {
            _ = hb_interval.tick() => {
                send_ping(&mut write, service_id, &mut seq).await?;
            }
            _ = timeout_check.tick() => {
                if last_recv.elapsed() > HEARTBEAT_TIMEOUT {
                    return Err(anyhow!("feishu websocket heartbeat timeout"));
                }
            }
            msg = read.next() => {
                let raw = match msg {
                    Some(Ok(WsMessage::Binary(data))) => {
                        last_recv = Instant::now();
                        data
                    }
                    Some(Ok(WsMessage::Text(text))) => {
                        last_recv = Instant::now();
                        if let Ok(event) = serde_json::from_str::<FeishuWsEvent>(&text) {
                            handle_event(&account_id, &api, &attachment_root, event, tx.clone(), state.clone()).await?;
                        }
                        continue;
                    }
                    Some(Ok(WsMessage::Ping(data))) => {
                        last_recv = Instant::now();
                        write.send(WsMessage::Pong(data)).await?;
                        continue;
                    }
                    Some(Ok(WsMessage::Pong(_))) => {
                        last_recv = Instant::now();
                        continue;
                    }
                    Some(Ok(WsMessage::Close(frame))) => {
                        warn!("feishu websocket closed: {:?}", frame);
                        break;
                    }
                    Some(Ok(_)) => continue,
                    Some(Err(err)) => return Err(anyhow!(err)),
                    None => break,
                };

                let frame = PbFrame::decode(&raw[..])?;
                if frame.method == 0 {
                    if frame.header_value("type") == "pong"
                        && let Some(payload) = &frame.payload
                        && let Ok(cfg) = serde_json::from_slice::<WsClientConfig>(payload)
                        && let Some(secs) = cfg.ping_interval
                    {
                        let secs = secs.max(10);
                        if secs != ping_secs {
                            ping_secs = secs;
                            hb_interval = tokio::time::interval(Duration::from_secs(ping_secs));
                        }
                    }
                    continue;
                }
                if frame.header_value("type") != "event" {
                    continue;
                }
                let mut ack = frame.clone();
                ack.payload = Some(br#"{"code":200,"headers":{},"data":[]}"#.to_vec());
                ack.headers.push(PbHeader {
                    key: "biz_rt".into(),
                    value: "0".into(),
                });
                let _ = write.send(WsMessage::Binary(ack.encode_to_vec().into())).await;
                let payload = frame.payload.unwrap_or_default();
                match serde_json::from_slice::<FeishuWsEvent>(&payload) {
                    Ok(event) => handle_event(&account_id, &api, &attachment_root, event, tx.clone(), state.clone()).await?,
                    Err(err) => warn!("failed to parse feishu event: {}", err),
                }
            }
        }
    }
    Ok(())
}

async fn send_ping<S>(write: &mut S, service_id: i32, seq: &mut u64) -> Result<()>
where
    S: SinkExt<WsMessage> + Unpin,
    <S as futures_util::Sink<WsMessage>>::Error: std::error::Error + Send + Sync + 'static,
{
    *seq = seq.wrapping_add(1);
    let ping = PbFrame {
        seq_id: *seq,
        log_id: 0,
        service: service_id,
        method: 0,
        headers: vec![PbHeader {
            key: "type".into(),
            value: "ping".into(),
        }],
        payload: None,
    };
    write
        .send(WsMessage::Binary(ping.encode_to_vec().into()))
        .await?;
    Ok(())
}

async fn handle_event(
    account_id: &str,
    api: &FeishuApi,
    attachment_root: &Path,
    event: FeishuWsEvent,
    tx: mpsc::Sender<InboundMessage>,
    state: SharedState,
) -> Result<()> {
    if event.header.event_type != "im.message.receive_v1" {
        return Ok(());
    }
    let receive: MessageReceivePayload = serde_json::from_value(event.event)?;
    if receive.sender.sender_type == "app" || receive.sender.sender_type == "bot" {
        return Ok(());
    }
    let sender_id = receive
        .sender
        .sender_id
        .open_id
        .unwrap_or_else(|| "unknown".to_string());
    state
        .push_event(
            "info",
            "feishu_event_received",
            format!(
                "chat={} sender={} type={}",
                receive.message.chat_id, sender_id, receive.message.message_type
            ),
        )
        .await;
    let settings = &api.settings;
    if !settings.allowed_open_ids.is_empty() && !settings.allowed_open_ids.contains(&sender_id) {
        state
            .push_event(
                "warn",
                "feishu_message_ignored",
                format!("sender not allowed: {sender_id}"),
            )
            .await;
        warn!(
            "ignored feishu message from unauthorized sender={}",
            sender_id
        );
        return Ok(());
    }
    if !settings.allowed_chat_ids.is_empty()
        && !settings.allowed_chat_ids.contains(&receive.message.chat_id)
    {
        state
            .push_event(
                "warn",
                "feishu_message_ignored",
                format!("chat not allowed: {}", receive.message.chat_id),
            )
            .await;
        warn!(
            "ignored feishu message from unauthorized chat={}",
            receive.message.chat_id
        );
        return Ok(());
    }
    let text = extract_text_content(&receive.message.content, &receive.message.message_type);
    let attachments = collect_attachments(api, attachment_root, &receive.message).await;
    if text.trim().is_empty() && attachments.is_empty() {
        state
            .push_event(
                "warn",
                "feishu_message_ignored",
                "empty text and no attachment",
            )
            .await;
        return Ok(());
    }
    let mentioned = receive.message.chat_type == "p2p" || !receive.message.mentions.is_empty();
    if receive.message.chat_type != "p2p" && settings.mention_only && !mentioned {
        state
            .push_event(
                "warn",
                "feishu_message_ignored",
                format!("group message without mention: {}", receive.message.chat_id),
            )
            .await;
        warn!(
            "ignored feishu group message without mention chat={}",
            receive.message.chat_id
        );
        return Ok(());
    }
    tx.send(InboundMessage {
        platform: ImPlatformKind::Feishu,
        account_id: account_id.to_string(),
        sender_id,
        chat_id: receive.message.chat_id,
        chat_type: if receive.message.chat_type == "p2p" {
            ChatType::Direct
        } else {
            ChatType::Group
        },
        message_id: receive.message.message_id,
        received_at_ms: now_ms(),
        text,
        mentioned,
        approval_request_key: None,
        action: None,
        card_message_id: None,
        attachments,
    })
    .await
    .map_err(|_| anyhow!("feishu inbound pump closed"))
}

fn extract_text_content(raw_content: &str, message_type: &str) -> String {
    if raw_content.trim().is_empty() {
        return String::new();
    }
    match message_type {
        "text" => serde_json::from_str::<serde_json::Value>(raw_content)
            .ok()
            .and_then(|v| v.get("text").and_then(|v| v.as_str()).map(str::to_string))
            .unwrap_or_else(|| raw_content.to_string()),
        "image" | "file" | "audio" | "video" | "media" | "sticker" => String::new(),
        _ => raw_content.to_string(),
    }
}

async fn collect_attachments(
    api: &FeishuApi,
    attachment_root: &Path,
    message: &EventMessage,
) -> Vec<InboundAttachment> {
    let mut attachments = Vec::new();
    let (image_key, file_key, file_name) =
        parse_media_keys(&message.content, &message.message_type);
    if message.message_type == "image" {
        if let Some(image_key) = image_key {
            match api.download_image(&message.message_id, &image_key).await {
                Ok((bytes, content_type)) => {
                    let preferred_name = format!("image.{}", extension_for_mime(&content_type));
                    match persist_inbound_attachment_bytes(
                        attachment_root,
                        AttachmentKind::Image,
                        &preferred_name,
                        &bytes,
                        Some(content_type),
                        "image",
                        None,
                    )
                    .await
                    {
                        Ok(attachment) => attachments.push(attachment),
                        Err(err) => warn!("failed to persist feishu image attachment: {}", err),
                    }
                }
                Err(err) => warn!("failed to download feishu image attachment: {}", err),
            }
        }
    } else if matches!(
        message.message_type.as_str(),
        "file" | "audio" | "video" | "media" | "sticker"
    ) {
        if let Some(file_key) = file_key {
            let file_name = file_name.unwrap_or_else(|| "attachment.bin".to_string());
            match api.download_file(&message.message_id, &file_key).await {
                Ok(bytes) => {
                    let attachment_kind =
                        if matches!(message.message_type.as_str(), "video" | "media") {
                            AttachmentKind::Video
                        } else {
                            AttachmentKind::File
                        };
                    let attachment_label =
                        if matches!(message.message_type.as_str(), "video" | "media") {
                            "video"
                        } else {
                            "file"
                        };
                    match persist_inbound_attachment_bytes(
                        attachment_root,
                        attachment_kind,
                        &file_name,
                        &bytes,
                        None,
                        attachment_label,
                        Some(file_name.clone()),
                    )
                    .await
                    {
                        Ok(attachment) => attachments.push(attachment),
                        Err(err) => warn!("failed to persist feishu file attachment: {}", err),
                    }
                }
                Err(err) => warn!("failed to download feishu file attachment: {}", err),
            }
        }
    }
    attachments
}

fn parse_media_keys(
    raw_content: &str,
    message_type: &str,
) -> (Option<String>, Option<String>, Option<String>) {
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(raw_content) else {
        return (None, None, None);
    };
    let image_key = parsed
        .get("image_key")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let file_key = parsed
        .get("file_key")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let file_name = parsed
        .get("file_name")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    match message_type {
        "image" => (image_key, None, None),
        "file" => (
            None,
            file_key,
            file_name.or(Some("attachment.bin".to_string())),
        ),
        "audio" => (None, file_key, file_name.or(Some("audio.bin".to_string()))),
        "video" | "media" => (
            image_key,
            file_key,
            file_name.or(Some("media.bin".to_string())),
        ),
        "sticker" => (
            None,
            file_key,
            file_name.or(Some("sticker.bin".to_string())),
        ),
        _ => (None, None, None),
    }
}

#[derive(Debug, Clone, Copy)]
enum AttachmentKind {
    Image,
    File,
    Video,
}

impl AttachmentKind {
    fn as_dir(self) -> &'static str {
        match self {
            Self::Image => "images",
            Self::File => "files",
            Self::Video => "videos",
        }
    }
}

async fn persist_inbound_attachment_bytes(
    root: &Path,
    storage_kind: AttachmentKind,
    preferred_name: &str,
    bytes: &[u8],
    mime_type: Option<String>,
    attachment_kind: &str,
    display_name: Option<String>,
) -> Result<InboundAttachment> {
    let dir = root.join("feishu").join(storage_kind.as_dir());
    tokio::fs::create_dir_all(&dir)
        .await
        .with_context(|| format!("failed to create attachment dir {}", dir.display()))?;
    let path = dir.join(build_attachment_file_name(preferred_name));
    tokio::fs::write(&path, bytes)
        .await
        .with_context(|| format!("failed to write attachment {}", path.display()))?;
    Ok(InboundAttachment {
        kind: attachment_kind.to_string(),
        name: display_name.or_else(|| {
            path.file_name()
                .and_then(|v| v.to_str())
                .map(str::to_string)
        }),
        mime_type,
        text_hint: None,
        local_path: Some(path.to_string_lossy().to_string()),
    })
}

fn build_attachment_file_name(preferred_name: &str) -> String {
    let source = preferred_name.trim();
    let sanitized = sanitize_file_name(if source.is_empty() {
        "attachment.bin"
    } else {
        source
    });
    let path = Path::new(&sanitized);
    let stem = path
        .file_stem()
        .and_then(|v| v.to_str())
        .filter(|v| !v.trim().is_empty())
        .unwrap_or("attachment");
    match path
        .extension()
        .and_then(|v| v.to_str())
        .filter(|v| !v.trim().is_empty())
    {
        Some(ext) => format!("{}-{}.{}", stem, attachment_suffix(), ext),
        None => format!("{}-{}", stem, attachment_suffix()),
    }
}

fn attachment_suffix() -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_millis())
        .unwrap_or_default();
    let seq = ATTACHMENT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("{millis:x}-{seq:x}")
}

fn sanitize_file_name(name: &str) -> String {
    name.chars()
        .map(|ch| match ch {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect()
}

fn extension_for_mime(content_type: &str) -> &'static str {
    if content_type.contains("jpeg") {
        "jpg"
    } else if content_type.contains("webp") {
        "webp"
    } else if content_type.contains("gif") {
        "gif"
    } else {
        "png"
    }
}

fn ensure_success(operation: &str, status: StatusCode, body: &serde_json::Value) -> Result<()> {
    if !status.is_success() {
        return Err(anyhow!(
            "feishu {operation} failed: status={} body={}",
            status,
            body
        ));
    }
    let code = body.get("code").and_then(|v| v.as_i64()).unwrap_or(0);
    if code != 0 {
        return Err(anyhow!(
            "feishu {operation} failed: code={} msg={} body={}",
            code,
            body.get("msg").and_then(|v| v.as_str()).unwrap_or_default(),
            body
        ));
    }
    Ok(())
}
