use std::time::{Duration, Instant};
use std::{fs, path::Path};

use anyhow::{Result, anyhow};
use once_cell::sync::Lazy;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::chain_log;

use super::errors::ensure_feishu_api_success;
use super::types::FeishuSettings;

const FEISHU_API_BASE: &str = "https://open.feishu.cn/open-apis";
const FEISHU_WS_BASE: &str = "https://open.feishu.cn";
const DEFAULT_TOKEN_TTL: Duration = Duration::from_secs(7200);
const TOKEN_REFRESH_SKEW: Duration = Duration::from_secs(120);
static FEISHU_HTTP_CLIENT: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
        .pool_max_idle_per_host(8)
        .tcp_keepalive(Duration::from_secs(60))
        .build()
        .expect("failed to build feishu http client")
});

// Process-wide tenant token cache to avoid fetching a token for every message send.
// Keyed by (app_id, app_secret) since both define the credential pair.
static TENANT_TOKEN_CACHE: Lazy<RwLock<HashMap<String, CachedTenantToken>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));
static TENANT_TOKEN_LOCKS: Lazy<Mutex<HashMap<String, Arc<Mutex<()>>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

fn tenant_token_cache_key(settings: &FeishuSettings) -> Option<String> {
    let app_id = settings
        .app_id
        .as_ref()
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())?;
    let app_secret = settings
        .app_secret
        .as_ref()
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())?;
    Some(format!("{app_id}:{app_secret}"))
}

#[derive(Debug, Clone)]
pub(super) struct CachedTenantToken {
    pub(super) value: String,
    pub(super) refresh_after: Instant,
}

#[derive(Debug, Default, Deserialize)]
pub struct WsClientConfig {
    #[serde(rename = "PingInterval")]
    pub ping_interval: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct WsEndpointResp {
    pub(super) code: i32,
    #[serde(default)]
    pub(super) msg: Option<String>,
    #[serde(default)]
    pub(super) data: Option<WsEndpoint>,
}

#[derive(Debug, Deserialize)]
pub(super) struct WsEndpoint {
    #[serde(rename = "URL")]
    pub(super) url: String,
    #[serde(rename = "ClientConfig")]
    pub(super) client_config: Option<WsClientConfig>,
}

#[derive(Clone)]
pub struct FeishuApi {
    settings: FeishuSettings,
}

impl FeishuApi {
    pub fn new(settings: FeishuSettings) -> Self {
        Self { settings }
    }

    fn http_client(&self) -> &reqwest::Client {
        &FEISHU_HTTP_CLIENT
    }

    pub(super) fn tenant_access_token_url(&self) -> String {
        format!("{FEISHU_API_BASE}/auth/v3/tenant_access_token/internal")
    }

    pub(super) fn ws_endpoint_url(&self) -> String {
        format!("{FEISHU_WS_BASE}/callback/ws/endpoint")
    }

    pub(super) fn oauth_device_authorization_url(&self) -> String {
        "https://accounts.feishu.cn/oauth/v1/device_authorization".to_string()
    }

    pub(super) fn app_registration_url(&self) -> String {
        "https://accounts.feishu.cn/oauth/v1/app/registration".to_string()
    }

    pub(super) fn send_message_url(&self) -> String {
        format!("{FEISHU_API_BASE}/im/v1/messages?receive_id_type=chat_id")
    }

    pub(super) fn send_message_url_for_receive_id_type(&self, receive_id_type: &str) -> String {
        format!("{FEISHU_API_BASE}/im/v1/messages?receive_id_type={receive_id_type}")
    }

    pub(super) fn message_update_url(&self, message_id: &str) -> String {
        format!("{FEISHU_API_BASE}/im/v1/messages/{message_id}")
    }

    pub(super) fn cardkit_create_url(&self) -> String {
        format!("{FEISHU_API_BASE}/cardkit/v1/cards")
    }

    pub(super) fn cardkit_update_url(&self, card_id: &str) -> String {
        format!("{FEISHU_API_BASE}/cardkit/v1/cards/{card_id}")
    }

    pub(super) fn cardkit_settings_url(&self, card_id: &str) -> String {
        format!("{FEISHU_API_BASE}/cardkit/v1/cards/{card_id}/settings")
    }

    pub(super) fn cardkit_element_content_url(&self, card_id: &str, element_id: &str) -> String {
        format!("{FEISHU_API_BASE}/cardkit/v1/cards/{card_id}/elements/{element_id}/content")
    }

    pub(super) fn upload_image_url(&self) -> String {
        format!("{FEISHU_API_BASE}/im/v1/images")
    }

    pub(super) fn upload_file_url(&self) -> String {
        format!("{FEISHU_API_BASE}/im/v1/files")
    }

    pub(super) fn application_info_url(&self, app_id: &str) -> String {
        format!("{FEISHU_API_BASE}/application/v6/applications/{app_id}")
    }

    pub(super) fn file_download_url(&self, message_id: &str, file_key: &str) -> String {
        format!("{FEISHU_API_BASE}/im/v1/messages/{message_id}/resources/{file_key}?type=file")
    }

    pub(super) fn image_resource_download_url(&self, message_id: &str, image_key: &str) -> String {
        format!("{FEISHU_API_BASE}/im/v1/messages/{message_id}/resources/{image_key}?type=image")
    }

    pub async fn request_device_authorization(
        &self,
        scope: Option<&str>,
    ) -> Result<serde_json::Value> {
        let app_id = self
            .settings
            .app_id
            .clone()
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| anyhow!("missing app_id"))?;
        let app_secret = self
            .settings
            .app_secret
            .clone()
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| anyhow!("missing app_secret"))?;

        let mut scope_value = scope.unwrap_or("").trim().to_string();
        if !scope_value
            .split_whitespace()
            .any(|part| part == "offline_access")
        {
            scope_value = if scope_value.is_empty() {
                "offline_access".to_string()
            } else {
                format!("{scope_value} offline_access")
            };
        }

        let basic_auth = {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD.encode(format!("{app_id}:{app_secret}"))
        };

        let body = [
            ("client_id", app_id.as_str()),
            ("scope", scope_value.as_str()),
        ];

        let response = self
            .http_client()
            .post(self.oauth_device_authorization_url())
            .header("Authorization", format!("Basic {basic_auth}"))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("User-Agent", "arthas-desktop")
            .form(&body)
            .send()
            .await?;

        let status = response.status();
        let payload =
            read_json_response(response, "feishu oauth device authorization", false).await?;
        if !status.is_success() {
            return Err(anyhow!(
                "feishu oauth device authorization failed: status={} body={}",
                status,
                payload
            ));
        }
        if payload.get("error").is_some() {
            return Err(anyhow!(
                "feishu oauth device authorization failed: body={}",
                payload
            ));
        }
        Ok(payload)
    }

    pub async fn start_app_registration(&self) -> Result<serde_json::Value> {
        let response = self
            .http_client()
            .post(self.app_registration_url())
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("User-Agent", "arthas-desktop")
            .form(&[
                ("action", "begin"),
                ("archetype", "PersonalAgent"),
                ("auth_method", "client_secret"),
                ("request_user_info", "open_id"),
            ])
            .send()
            .await?;
        let status = response.status();
        let payload = read_json_response(response, "feishu app registration begin", false).await?;
        if !status.is_success() {
            return Err(anyhow!(
                "feishu app registration begin failed: status={} body={}",
                status,
                payload
            ));
        }
        Ok(payload)
    }

    pub async fn poll_app_registration(&self, device_code: &str) -> Result<serde_json::Value> {
        let response = self
            .http_client()
            .post(self.app_registration_url())
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("User-Agent", "arthas-desktop")
            .form(&[("action", "poll"), ("device_code", device_code)])
            .send()
            .await?;
        let payload = read_json_response(response, "feishu app registration poll", false).await?;
        Ok(payload)
    }

    pub async fn get_application_display_name(&self, app_id: &str) -> Result<Option<String>> {
        let token = self.get_tenant_access_token().await?;
        let response = self
            .http_client()
            .get(self.application_info_url(app_id))
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .query(&[("lang", "zh_cn")])
            .send()
            .await?;

        let status = response.status();
        let payload: serde_json::Value = response.json().await?;
        if !status.is_success() {
            return Err(anyhow!(
                "feishu application info request failed: status={} code={} msg={}",
                status,
                payload.get("code").and_then(|v| v.as_i64()).unwrap_or(-1),
                payload
                    .get("msg")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
            ));
        }
        let code = payload.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
        if code != 0 {
            return Err(anyhow!(
                "feishu application info failed: code={} msg={}",
                code,
                payload
                    .get("msg")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
            ));
        }

        let app = payload
            .get("data")
            .and_then(|data| data.get("app").or(Some(data)))
            .cloned()
            .unwrap_or(serde_json::Value::Null);

        let display_name = app
            .get("i18n_name")
            .and_then(|name| {
                name.get("zh_cn")
                    .and_then(|v| v.as_str())
                    .or_else(|| name.get("en_us").and_then(|v| v.as_str()))
                    .or_else(|| name.get("ja_jp").and_then(|v| v.as_str()))
            })
            .or_else(|| app.get("name").and_then(|v| v.as_str()))
            .or_else(|| app.get("app_name").and_then(|v| v.as_str()))
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        Ok(display_name)
    }

    pub(super) async fn get_tenant_access_token(&self) -> Result<String> {
        let cache_key = tenant_token_cache_key(&self.settings)
            .ok_or_else(|| anyhow!("missing app_id/app_secret"))?;

        // Fast path: cached + still valid.
        {
            let cache = TENANT_TOKEN_CACHE.read().await;
            if let Some(token) = cache.get(&cache_key) {
                if Instant::now() < token.refresh_after {
                    return Ok(token.value.clone());
                }
            }
        }

        // Serialize refresh per credential pair (singleflight).
        let lock = {
            let mut locks = TENANT_TOKEN_LOCKS.lock().await;
            locks
                .entry(cache_key.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _guard = lock.lock().await;

        // Re-check after acquiring the lock.
        {
            let cache = TENANT_TOKEN_CACHE.read().await;
            if let Some(token) = cache.get(&cache_key) {
                if Instant::now() < token.refresh_after {
                    return Ok(token.value.clone());
                }
            }
        }

        let resp = self
            .http_client()
            .post(self.tenant_access_token_url())
            .json(&serde_json::json!({
                "app_id": self.settings.app_id,
                "app_secret": self.settings.app_secret,
            }))
            .send()
            .await?;
        let status = resp.status();
        let body: serde_json::Value = resp.json().await?;
        if !status.is_success() {
            return Err(anyhow!(
                "feishu tenant_access_token request failed: status={} body={}",
                status,
                body
            ));
        }

        let code = body.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
        if code != 0 {
            return Err(anyhow!(
                "feishu tenant_access_token failed: code={} msg={}",
                code,
                body.get("msg")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
            ));
        }

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
        let ttl = Duration::from_secs(ttl);
        let refresh_after = Instant::now()
            + ttl
                .checked_sub(TOKEN_REFRESH_SKEW)
                .unwrap_or(Duration::from_secs(1));

        let mut cache = TENANT_TOKEN_CACHE.write().await;
        cache.insert(
            cache_key,
            CachedTenantToken {
                value: token.clone(),
                refresh_after,
            },
        );
        Ok(token)
    }

    pub async fn get_ws_endpoint(&self) -> Result<(String, WsClientConfig)> {
        let resp = self
            .http_client()
            .post(self.ws_endpoint_url())
            .header("locale", "zh")
            .json(&serde_json::json!({
                "AppID": self.settings.app_id,
                "AppSecret": self.settings.app_secret,
            }))
            .send()
            .await?
            .json::<WsEndpointResp>()
            .await?;

        if resp.code != 0 {
            return Err(anyhow!(
                "feishu ws endpoint failed: code={} msg={}",
                resp.code,
                resp.msg.unwrap_or_default()
            ));
        }
        let endpoint = resp
            .data
            .ok_or_else(|| anyhow!("feishu ws endpoint returned empty data"))?;
        Ok((endpoint.url, endpoint.client_config.unwrap_or_default()))
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
            "content": serde_json::json!({
                "text": text
            }).to_string(),
        });

        let response = self
            .http_client()
            .post(self.send_message_url_for_receive_id_type(receive_id_type))
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        let payload: serde_json::Value = response.json().await?;
        log_feishu_api_response(
            &format!(
                "send_text_message receive_id_type={receive_id_type} receive_id={receive_id} text_len={}",
                text.len()
            ),
            status,
            &payload,
        );
        ensure_feishu_api_success("send_text_message", status, &payload)?;
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

        let response = self
            .http_client()
            .post(self.send_message_url_for_receive_id_type(receive_id_type))
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        let payload: serde_json::Value = response.json().await?;
        log_feishu_api_response(
            &format!(
                "send_interactive_message receive_id_type={receive_id_type} receive_id={receive_id}"
            ),
            status,
            &payload,
        );
        ensure_feishu_api_success("send_interactive_message", status, &payload)?;
        payload
            .get("data")
            .and_then(|v| v.get("message_id"))
            .and_then(|v| v.as_str())
            .map(|v| v.to_string())
            .ok_or_else(|| anyhow!("feishu interactive send missing message_id"))
    }

    pub async fn send_cardkit_message(&self, chat_id: &str, card_id: &str) -> Result<String> {
        self.send_cardkit_message_to("chat_id", chat_id, card_id)
            .await
    }

    pub async fn send_cardkit_message_to(
        &self,
        receive_id_type: &str,
        receive_id: &str,
        card_id: &str,
    ) -> Result<String> {
        let token = self.get_tenant_access_token().await?;
        let body = serde_json::json!({
            "receive_id": receive_id,
            "msg_type": "interactive",
            "content": serde_json::json!({
                "type": "card",
                "data": {
                    "card_id": card_id
                }
            }).to_string(),
        });

        let response = self
            .http_client()
            .post(self.send_message_url_for_receive_id_type(receive_id_type))
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        let payload = read_json_response(response, "feishu send cardkit message", false).await?;
        log_feishu_api_response(
            &format!(
                "send_cardkit_message receive_id_type={receive_id_type} receive_id={receive_id} card_id={card_id}"
            ),
            status,
            &payload,
        );
        ensure_feishu_api_success("send_cardkit_message", status, &payload)?;
        payload
            .get("data")
            .and_then(|v| v.get("message_id"))
            .and_then(|v| v.as_str())
            .map(|v| v.to_string())
            .ok_or_else(|| anyhow!("feishu cardkit send missing message_id"))
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

        let response = self
            .http_client()
            .patch(self.message_update_url(message_id))
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        let payload: serde_json::Value = response.json().await?;
        log_feishu_api_response("update_interactive_message", status, &payload);
        ensure_feishu_api_success("update_interactive_message", status, &payload)?;
        Ok(())
    }

    pub async fn create_cardkit_card(&self, card: &serde_json::Value) -> Result<String> {
        let token = self.get_tenant_access_token().await?;
        let body = serde_json::json!({
            "type": "card_json",
            "data": card.to_string(),
        });
        let response = self
            .http_client()
            .post(self.cardkit_create_url())
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&body)
            .send()
            .await?;
        let status = response.status();
        let payload = read_json_response(response, "feishu cardkit create", false).await?;
        log_feishu_api_response("create_cardkit_card", status, &payload);
        ensure_feishu_api_success("create_cardkit_card", status, &payload)?;
        payload
            .get("data")
            .and_then(|v| v.get("card_id"))
            .and_then(|v| v.as_str())
            .map(|v| v.to_string())
            .ok_or_else(|| anyhow!("feishu cardkit create missing card_id"))
    }

    pub async fn update_cardkit_card(
        &self,
        card_id: &str,
        card: &serde_json::Value,
        sequence: u64,
    ) -> Result<()> {
        let token = self.get_tenant_access_token().await?;
        let body = serde_json::json!({
            "card": {
                "type": "card_json",
                "data": card.to_string(),
            },
            "sequence": sequence,
        });
        let response = self
            .http_client()
            .put(self.cardkit_update_url(card_id))
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&body)
            .send()
            .await?;
        let status = response.status();
        let payload = read_json_response(
            response,
            &format!("feishu cardkit update card_id={card_id} sequence={sequence}"),
            true,
        )
        .await?;
        log_feishu_api_response(
            &format!("update_cardkit_card card_id={card_id} sequence={sequence}"),
            status,
            &payload,
        );
        ensure_feishu_api_success("update_cardkit_card", status, &payload)?;
        Ok(())
    }

    pub async fn set_cardkit_streaming_mode(
        &self,
        card_id: &str,
        streaming_mode: bool,
        sequence: u64,
    ) -> Result<()> {
        let token = self.get_tenant_access_token().await?;
        let body = serde_json::json!({
            "settings": serde_json::json!({
                "streaming_mode": streaming_mode
            }).to_string(),
            "sequence": sequence,
        });
        let response = self
            .http_client()
            .patch(self.cardkit_settings_url(card_id))
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&body)
            .send()
            .await?;
        let status = response.status();
        let payload = read_json_response(
            response,
            &format!(
                "feishu cardkit settings card_id={card_id} streaming_mode={streaming_mode} sequence={sequence}"
            ),
            true,
        )
        .await?;
        log_feishu_api_response(
            &format!(
                "set_cardkit_streaming_mode card_id={card_id} streaming_mode={streaming_mode} sequence={sequence}"
            ),
            status,
            &payload,
        );
        ensure_feishu_api_success("set_cardkit_streaming_mode", status, &payload)?;
        Ok(())
    }

    pub async fn stream_cardkit_element_content(
        &self,
        card_id: &str,
        element_id: &str,
        content: &str,
        sequence: u64,
    ) -> Result<()> {
        let token = self.get_tenant_access_token().await?;
        let body = serde_json::json!({
            "content": content,
            "sequence": sequence,
        });
        let response = self
            .http_client()
            .put(self.cardkit_element_content_url(card_id, element_id))
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&body)
            .send()
            .await?;
        let status = response.status();
        let payload = read_json_response(
            response,
            &format!(
                "feishu cardkit element content card_id={card_id} element_id={element_id} sequence={sequence}"
            ),
            true,
        )
        .await?;
        log_feishu_api_response(
            &format!(
                "stream_cardkit_element_content card_id={card_id} element_id={element_id} sequence={sequence} content_len={}",
                content.len()
            ),
            status,
            &payload,
        );
        ensure_feishu_api_success("stream_cardkit_element_content", status, &payload)?;
        Ok(())
    }

    pub async fn upload_image(&self, local_path: &str) -> Result<String> {
        let token = self.get_tenant_access_token().await?;
        let bytes = fs::read(local_path)?;
        let file_name = Path::new(local_path)
            .file_name()
            .and_then(|v| v.to_str())
            .unwrap_or("image.png")
            .to_string();
        let mime = mime_guess::from_path(&file_name)
            .first_raw()
            .unwrap_or("image/png");
        let part = reqwest::multipart::Part::bytes(bytes)
            .file_name(file_name)
            .mime_str(mime)?;
        let form = reqwest::multipart::Form::new()
            .text("image_type", "message")
            .part("image", part);
        let response = self
            .http_client()
            .post(self.upload_image_url())
            .header("Authorization", format!("Bearer {token}"))
            .multipart(form)
            .send()
            .await?;
        let status = response.status();
        let payload: serde_json::Value = response.json().await?;
        ensure_feishu_api_success("upload_image", status, &payload)?;
        payload
            .get("data")
            .and_then(|v| v.get("image_key"))
            .and_then(|v| v.as_str())
            .map(|v| v.to_string())
            .ok_or_else(|| anyhow!("feishu upload image missing image_key"))
    }

    pub async fn upload_file(&self, local_path: &str, file_name: Option<&str>) -> Result<String> {
        let token = self.get_tenant_access_token().await?;
        let bytes = fs::read(local_path)?;
        let fallback_name = Path::new(local_path)
            .file_name()
            .and_then(|v| v.to_str())
            .unwrap_or("attachment.bin");
        let resolved_name = file_name
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(fallback_name)
            .to_string();
        let mime = mime_guess::from_path(&resolved_name)
            .first_raw()
            .unwrap_or("application/octet-stream");
        let part = reqwest::multipart::Part::bytes(bytes)
            .file_name(resolved_name)
            .mime_str(mime)?;
        let form = reqwest::multipart::Form::new()
            .text("file_type", "stream")
            .part("file", part);
        let response = self
            .http_client()
            .post(self.upload_file_url())
            .header("Authorization", format!("Bearer {token}"))
            .multipart(form)
            .send()
            .await?;
        let status = response.status();
        let payload: serde_json::Value = response.json().await?;
        ensure_feishu_api_success("upload_file", status, &payload)?;
        payload
            .get("data")
            .and_then(|v| v.get("file_key"))
            .and_then(|v| v.as_str())
            .map(|v| v.to_string())
            .ok_or_else(|| anyhow!("feishu upload file missing file_key"))
    }

    pub async fn send_image_message(&self, chat_id: &str, image_key: &str) -> Result<()> {
        let token = self.get_tenant_access_token().await?;
        let body = serde_json::json!({
            "receive_id": chat_id,
            "msg_type": "image",
            "content": serde_json::json!({
                "image_key": image_key
            }).to_string(),
        });
        let response = self
            .http_client()
            .post(self.send_message_url())
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&body)
            .send()
            .await?;
        let status = response.status();
        let payload: serde_json::Value = response.json().await?;
        log_feishu_api_response(
            &format!("send_image_message chat_id={chat_id} image_key={image_key}"),
            status,
            &payload,
        );
        ensure_feishu_api_success("send_image_message", status, &payload)?;
        Ok(())
    }

    pub async fn send_file_message(&self, chat_id: &str, file_key: &str) -> Result<()> {
        let token = self.get_tenant_access_token().await?;
        let body = serde_json::json!({
            "receive_id": chat_id,
            "msg_type": "file",
            "content": serde_json::json!({
                "file_key": file_key
            }).to_string(),
        });
        let response = self
            .http_client()
            .post(self.send_message_url())
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&body)
            .send()
            .await?;
        let status = response.status();
        let payload: serde_json::Value = response.json().await?;
        log_feishu_api_response(
            &format!("send_file_message chat_id={chat_id} file_key={file_key}"),
            status,
            &payload,
        );
        ensure_feishu_api_success("send_file_message", status, &payload)?;
        Ok(())
    }

    pub async fn download_image(
        &self,
        message_id: &str,
        image_key: &str,
    ) -> Result<(Vec<u8>, String)> {
        let token = self.get_tenant_access_token().await?;
        let response = self
            .http_client()
            .get(self.image_resource_download_url(message_id, image_key))
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
        let response = self
            .http_client()
            .get(self.file_download_url(message_id, file_key))
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

fn log_feishu_api_response(
    operation: &str,
    status: reqwest::StatusCode,
    payload: &serde_json::Value,
) {
    let code = payload.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    let msg = payload
        .get("msg")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let message_id = payload
        .get("data")
        .and_then(|v| v.get("message_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let card_id = payload
        .get("data")
        .and_then(|v| v.get("card_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    chain_log::write_line(format!(
        "[feishu_api] event={} operation={} http_status={} code={} msg={} message_id={} card_id={} body={}",
        if status.is_success() && code == 0 {
            "ok"
        } else {
            "failed"
        },
        operation,
        status.as_u16(),
        code,
        msg,
        message_id,
        card_id,
        payload
    ));

    if status.is_success() && code == 0 {
        info!(
            target: "codex_remote::feishu",
            event = "feishu_api_ok",
            operation,
            http_status = status.as_u16(),
            code,
            msg,
            message_id,
            card_id,
            "Feishu API request succeeded"
        );
    } else {
        warn!(
            target: "codex_remote::feishu",
            event = "feishu_api_failed",
            operation,
            http_status = status.as_u16(),
            code,
            msg,
            body = %payload,
            "Feishu API request failed"
        );
    }
}

async fn read_json_response(
    response: reqwest::Response,
    context: &str,
    allow_empty_success_body: bool,
) -> Result<serde_json::Value> {
    let status = response.status();
    let body_text = response.text().await?;
    let trimmed = body_text.trim();

    if trimmed.is_empty() {
        if status.is_success() && allow_empty_success_body {
            return Ok(serde_json::json!({ "code": 0 }));
        }
        return Err(anyhow!("{context} returned empty body: status={status}"));
    }

    serde_json::from_str(trimmed).map_err(|err| {
        anyhow!(
            "{context} returned invalid json: status={} err={} body={}",
            status,
            err,
            trimmed
        )
    })
}
