use std::sync::atomic::{AtomicU64, Ordering};

use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use qrcode::{QrCode, render::svg};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::{
    app_state::{SharedState, WechatOnboardSession, WecomOnboardSession},
    im::feishu::{FeishuApi, FeishuSettings},
    im::wechat::{
        api::WechatApi,
        store as wechat_store,
        types::{DEFAULT_WECHAT_API_BASE, WechatSettings},
    },
    im::wecom::onboarding::{self as wecom_onboarding, QrPoll},
};

use super::im_api;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FeishuOnboardStartResponse {
    verification_uri: String,
    verification_uri_complete: String,
    device_code: String,
    expires_in: u64,
    interval: u64,
    qr_svg: String,
}

pub(super) async fn feishu_onboard_start(State(state): State<SharedState>) -> impl IntoResponse {
    let settings = {
        let config = state.config.lock().await;
        FeishuSettings::from_app_config(&config.feishu)
    };
    let api = FeishuApi::new(settings);
    match api.start_app_registration().await {
        Ok(payload) => {
            let verification_uri = payload
                .get("verification_uri")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let verification_uri_complete = payload
                .get("verification_uri_complete")
                .and_then(|v| v.as_str())
                .or_else(|| payload.get("verification_uri").and_then(|v| v.as_str()))
                .unwrap_or_default()
                .to_string();
            let device_code = payload
                .get("device_code")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let expires_in = payload
                .get("expire_in")
                .or_else(|| payload.get("expires_in"))
                .and_then(|v| v.as_u64())
                .unwrap_or(600);
            let interval = payload
                .get("interval")
                .and_then(|v| v.as_u64())
                .unwrap_or(5);
            let qr_svg = build_qr_svg(&verification_uri_complete).unwrap_or_default();
            state
                .push_event("info", "feishu_onboard_started", "scan flow started")
                .await;
            (
                StatusCode::OK,
                Json(json!(FeishuOnboardStartResponse {
                    verification_uri,
                    verification_uri_complete,
                    device_code,
                    expires_in,
                    interval,
                    qr_svg,
                })),
            )
        }
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": err.to_string() })),
        ),
    }
}

struct FeishuPollOutcome {
    done: bool,
    app_id: Option<String>,
    open_id: Option<String>,
    display_name: Option<String>,
    raw: serde_json::Value,
}

/// Poll the Feishu device-code registration once and persist the account on
/// success. Shared by the legacy poll route and the sanitized manage route.
async fn poll_feishu_registration(
    state: &SharedState,
    device_code: &str,
) -> Result<FeishuPollOutcome, (StatusCode, Json<serde_json::Value>)> {
    let settings = {
        let config = state.config.lock().await;
        FeishuSettings::from_app_config(&config.feishu)
    };
    let api = FeishuApi::new(settings);
    let result = api
        .poll_app_registration(device_code)
        .await
        .map_err(|err| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": err.to_string() })),
            )
        })?;
    let app_id = result
        .get("client_id")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let app_secret = result
        .get("client_secret")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let open_id = result
        .get("user_info")
        .and_then(|v| v.get("open_id"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let done = app_id.is_some() && app_secret.is_some();
    let mut display_name = None;
    if let (Some(app_id), Some(app_secret)) = (app_id.clone(), app_secret) {
        display_name =
            persist_feishu_registration(state, &app_id, &app_secret, open_id.as_deref(), None)
                .await?;
    }
    Ok(FeishuPollOutcome {
        done,
        app_id,
        open_id,
        display_name,
        raw: result,
    })
}

/// Persist verified Feishu credentials as an account, refresh the display
/// name, and restart the bridge. Pass `display_name` when the caller already
/// verified the credentials by fetching it, to skip the second lookup.
async fn persist_feishu_registration(
    state: &SharedState,
    app_id: &str,
    app_secret: &str,
    open_id: Option<&str>,
    display_name: Option<String>,
) -> Result<Option<String>, (StatusCode, Json<serde_json::Value>)> {
    let feishu_config = {
        let mut config = state.config.lock().await;
        let previous_config = config.clone();
        config.migrate_legacy_im_accounts();
        config.feishu_accounts.retain(|account| {
            account.account_id.trim() == app_id || account.app_id.trim() != app_id
        });
        let mut account = config.feishu_account(app_id).unwrap_or_default();
        account.enabled = true;
        account.account_id = app_id.to_string();
        account.app_id = app_id.to_string();
        account.app_secret = app_secret.to_string();
        if let Some(open_id) = open_id
            && !account
                .allowed_open_ids
                .iter()
                .any(|existing| existing == open_id)
        {
            account.allowed_open_ids.push(open_id.to_string());
        }
        config.upsert_feishu_account(account.clone());
        let saved_account = account.clone();
        if !config.feishu.is_configured() || config.feishu.app_id == app_id {
            config.feishu = account;
        }
        config.bridge.enabled = true;
        if let Err(err) = config.save(&state.config_path) {
            *config = previous_config;
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": err.to_string() })),
            ));
        }
        saved_account
    };
    let mut display_name = display_name;
    if display_name.is_none() {
        let api = FeishuApi::new(FeishuSettings::from_app_config(&feishu_config));
        display_name = api
            .get_application_display_name(app_id)
            .await
            .ok()
            .flatten();
    }
    if let Some(name) = display_name.clone() {
        let mut config = state.config.lock().await;
        let previous_config = config.clone();
        if let Some(mut account) = config.feishu_account(app_id) {
            account.display_name = name;
            config.upsert_feishu_account(account.clone());
            if config.feishu.app_id == app_id {
                config.feishu = account;
            }
            if config.save(&state.config_path).is_err() {
                *config = previous_config;
            }
        }
    }
    state
        .push_event(
            "info",
            "feishu_onboard_completed",
            format!("app_id={app_id} open_id={}", open_id.unwrap_or_default()),
        )
        .await;
    im_api::start_bridge_task(
        state,
        im_api::BridgeStartMode::Restart,
        "bridge restarted after Feishu onboarding",
    )
    .await;
    Ok(display_name)
}

pub(super) async fn feishu_onboard_poll(
    State(state): State<SharedState>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    let Some(device_code) = payload.get("deviceCode").and_then(|v| v.as_str()) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "missing deviceCode" })),
        );
    };
    match poll_feishu_registration(&state, device_code).await {
        Ok(outcome) => (
            StatusCode::OK,
            Json(json!({
                "done": outcome.done,
                "appId": outcome.app_id,
                "openId": outcome.open_id,
                "displayName": outcome.display_name,
                "error": outcome.raw.get("error").cloned(),
                "errorDescription": outcome.raw.get("error_description").cloned(),
                "raw": outcome.raw,
            })),
        ),
        Err(response) => response,
    }
}

/// Sanitized variant of the Feishu poll for the versioned management API.
/// Unlike the legacy route it never echoes the raw registration payload,
/// which contains the freshly issued app secret.
pub(super) async fn manage_feishu_onboard_poll(
    State(state): State<SharedState>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    let Some(device_code) = payload.get("deviceCode").and_then(|v| v.as_str()) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "missing deviceCode" })),
        );
    };
    match poll_feishu_registration(&state, device_code).await {
        Ok(outcome) => (
            StatusCode::OK,
            Json(json!({
                "done": outcome.done,
                "appId": outcome.app_id,
                "displayName": outcome.display_name,
                "error": outcome
                    .raw
                    .get("error")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                "errorDescription": outcome
                    .raw
                    .get("error_description")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
            })),
        ),
        Err(response) => response,
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ConfigureFeishuAccountRequest {
    app_id: Option<String>,
    app_secret: Option<String>,
}

/// Manual credential onboarding for Feishu on the versioned management API.
/// Credentials are validated against the Feishu open platform before they
/// are persisted; the response never echoes the secret.
pub(super) async fn manage_configure_feishu_account(
    State(state): State<SharedState>,
    Json(request): Json<ConfigureFeishuAccountRequest>,
) -> impl IntoResponse {
    let Some(app_id) = request
        .app_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
    else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": "missing appId" })),
        );
    };
    let Some(app_secret) = request
        .app_secret
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty() && !im_api::is_masked_secret(value))
        .map(str::to_string)
    else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": "missing appSecret" })),
        );
    };
    let candidate = crate::config::FeishuConfig {
        enabled: true,
        app_id: app_id.clone(),
        app_secret: app_secret.clone(),
        ..Default::default()
    };
    let api = FeishuApi::new(FeishuSettings::from_app_config(&candidate));
    let display_name = match tokio::time::timeout(
        std::time::Duration::from_secs(8),
        api.get_application_display_name(&app_id),
    )
    .await
    {
        Ok(Ok(name)) => name,
        Ok(Err(err)) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "ok": false, "error": err.to_string() })),
            );
        }
        Err(_) => {
            return (
                StatusCode::REQUEST_TIMEOUT,
                Json(json!({ "ok": false, "error": "feishu credential check timeout" })),
            );
        }
    };
    match persist_feishu_registration(&state, &app_id, &app_secret, None, display_name).await {
        Ok(display_name) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "platform": "feishu",
                "accountId": app_id,
                "displayName": display_name,
            })),
        ),
        Err(response) => response,
    }
}

const WECHAT_ONBOARD_TTL_MS: u128 = 5 * 60_000;
const WECOM_ONBOARD_TTL_MS: u128 = 5 * 60_000;
static ONBOARD_START_GENERATION: AtomicU64 = AtomicU64::new(0);

trait OnboardSessionSnapshot {
    fn generation(&self) -> u64;
    fn session_key(&self) -> &str;
    fn matches_snapshot(&self, expected: &Self) -> bool;
}

impl OnboardSessionSnapshot for WechatOnboardSession {
    fn generation(&self) -> u64 {
        self.generation
    }

    fn session_key(&self) -> &str {
        &self.session_key
    }

    fn matches_snapshot(&self, expected: &Self) -> bool {
        self.generation == expected.generation
            && self.session_key == expected.session_key
            && self.qrcode == expected.qrcode
            && self.started_at_ms == expected.started_at_ms
            && self.current_api_base_url == expected.current_api_base_url
    }
}

impl OnboardSessionSnapshot for WecomOnboardSession {
    fn generation(&self) -> u64 {
        self.generation
    }

    fn session_key(&self) -> &str {
        &self.session_key
    }

    fn matches_snapshot(&self, expected: &Self) -> bool {
        self.generation == expected.generation
            && self.session_key == expected.session_key
            && self.scode == expected.scode
            && self.started_at_ms == expected.started_at_ms
    }
}

fn next_onboard_generation() -> u64 {
    ONBOARD_START_GENERATION.fetch_add(1, Ordering::Relaxed) + 1
}

fn reserve_newer_session<T: OnboardSessionSnapshot>(slot: &mut Option<T>, candidate: T) -> bool {
    if slot
        .as_ref()
        .is_some_and(|current| current.generation() > candidate.generation())
    {
        return false;
    }
    *slot = Some(candidate);
    true
}

fn compare_set_session<T: OnboardSessionSnapshot>(
    slot: &mut Option<T>,
    expected: &T,
    replacement: T,
) -> bool {
    if replacement.generation() != expected.generation()
        || replacement.session_key() != expected.session_key()
        || !slot
            .as_ref()
            .is_some_and(|current| current.matches_snapshot(expected))
    {
        return false;
    }
    *slot = Some(replacement);
    true
}

fn compare_clear_session<T: OnboardSessionSnapshot>(slot: &mut Option<T>, expected: &T) -> bool {
    if !slot
        .as_ref()
        .is_some_and(|current| current.matches_snapshot(expected))
    {
        return false;
    }
    *slot = None;
    true
}

fn restore_session_if_empty<T>(slot: &mut Option<T>, session: T) -> bool {
    if slot.is_some() {
        return false;
    }
    *slot = Some(session);
    true
}

fn invalid_session_response() -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "done": false, "error": "invalid_session" })),
    )
}

fn superseded_start_response() -> (StatusCode, Json<serde_json::Value>) {
    (StatusCode::CONFLICT, Json(json!({ "error": "superseded" })))
}

async fn persist_wechat_onboard_account(
    state: &SharedState,
    account_id: &str,
    bot_token: &str,
    base_url: &str,
    user_id: &str,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    let mut config = state.config.lock().await;
    let previous_config = config.clone();
    config.migrate_legacy_im_accounts();
    let token = bot_token.trim().to_string();
    config.wechat_accounts.retain(|account| {
        account.account_id.trim() == account_id || account.bot_token.trim() != token
    });
    let mut account = config.wechat_account(account_id).unwrap_or_default();
    account.enabled = true;
    account.account_id = account_id.to_string();
    account.bot_token = bot_token.to_string();
    if account.display_name.trim().is_empty() {
        account.display_name = "微信机器人".to_string();
    }
    account.base_url = normalize_wechat_base_url(base_url);
    account.user_id = user_id.to_string();
    if !user_id.trim().is_empty() && !account.allowed_user_ids.iter().any(|item| item == user_id) {
        account.allowed_user_ids.push(user_id.to_string());
    }
    config.upsert_wechat_account(account.clone());
    if !config.wechat.is_configured() || config.wechat.account_id == account_id {
        config.wechat = account;
    }
    config.bridge.enabled = true;
    if let Err(err) = config.save(&state.config_path) {
        *config = previous_config;
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "done": false, "error": err.to_string() })),
        ));
    }
    Ok(())
}

async fn persist_wecom_onboard_account(
    state: &SharedState,
    account_id: &str,
    bot_id: &str,
    secret: &str,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    let mut config = state.config.lock().await;
    let previous_config = config.clone();
    config.migrate_legacy_im_accounts();
    let mut account = config.wecom_account(account_id).unwrap_or_default();
    account.enabled = true;
    account.account_id = account_id.to_string();
    account.bot_id = bot_id.to_string();
    account.secret = secret.to_string();
    if account.display_name.trim().is_empty() {
        account.display_name = "企业微信机器人".to_string();
    }
    config.upsert_wecom_account(account.clone());
    if !config.wecom.is_configured() || config.wecom.account_id == account_id {
        config.wecom = account;
    }
    config.bridge.enabled = true;
    if let Err(err) = config.save(&state.config_path) {
        *config = previous_config;
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "done": false, "error": err.to_string() })),
        ));
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WechatOnboardStartResponse {
    session_key: String,
    qrcode_url: String,
    qr_svg: String,
    expires_in: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WechatOnboardPollRequest {
    session_key: String,
    verify_code: Option<String>,
}

pub(super) async fn wechat_onboard_start(State(state): State<SharedState>) -> impl IntoResponse {
    let generation = next_onboard_generation();
    let session_key = format!("wechat-onboard-{}", Uuid::new_v4().simple());
    let reserved_session = WechatOnboardSession {
        generation,
        session_key: session_key.clone(),
        qrcode: String::new(),
        started_at_ms: unix_now_millis(),
        current_api_base_url: DEFAULT_WECHAT_API_BASE.to_string(),
    };
    let reserved = {
        let mut onboard = state.wechat_onboard.lock().await;
        reserve_newer_session(&mut onboard, reserved_session.clone())
    };
    if !reserved {
        return superseded_start_response();
    }
    let config = state.config.lock().await.clone();
    let api = WechatApi::new(WechatSettings::from_app_config(&config.wechat));
    let local_tokens = wechat_store::local_bot_tokens(&state).await;
    match api.start_qr_login(&local_tokens).await {
        Ok(payload) => {
            let qr_svg = build_qr_svg(&payload.qrcode_img_content).unwrap_or_default();
            let session = WechatOnboardSession {
                generation,
                session_key: session_key.clone(),
                qrcode: payload.qrcode,
                started_at_ms: reserved_session.started_at_ms,
                current_api_base_url: DEFAULT_WECHAT_API_BASE.to_string(),
            };
            let installed = {
                let mut onboard = state.wechat_onboard.lock().await;
                compare_set_session(&mut onboard, &reserved_session, session)
            };
            if !installed {
                return superseded_start_response();
            }
            state
                .push_event("info", "wechat_onboard_started", "scan flow started")
                .await;
            (
                StatusCode::OK,
                Json(json!(WechatOnboardStartResponse {
                    session_key,
                    qrcode_url: payload.qrcode_img_content,
                    qr_svg,
                    expires_in: (WECHAT_ONBOARD_TTL_MS / 1000) as u64,
                })),
            )
        }
        Err(err) => {
            let mut onboard = state.wechat_onboard.lock().await;
            compare_clear_session(&mut onboard, &reserved_session);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": err.to_string() })),
            )
        }
    }
}

pub(super) async fn wechat_onboard_poll(
    State(state): State<SharedState>,
    Json(request): Json<WechatOnboardPollRequest>,
) -> impl IntoResponse {
    let session = {
        let onboard = state.wechat_onboard.lock().await;
        onboard.clone()
    };
    let Some(session) = session else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "done": false, "error": "missing_session" })),
        );
    };
    if session.session_key != request.session_key {
        return invalid_session_response();
    }
    if unix_now_millis().saturating_sub(session.started_at_ms) > WECHAT_ONBOARD_TTL_MS {
        let cleared = {
            let mut onboard = state.wechat_onboard.lock().await;
            compare_clear_session(&mut onboard, &session)
        };
        if !cleared {
            return invalid_session_response();
        }
        state
            .push_event(
                "warn",
                "wechat_onboard_expired",
                "local onboarding session expired",
            )
            .await;
        return (
            StatusCode::OK,
            Json(json!({ "done": false, "status": "expired", "error": "expired" })),
        );
    }

    let config = state.config.lock().await.clone();
    let api = WechatApi::new(WechatSettings::from_app_config(&config.wechat));
    let result = match api
        .poll_qr_status(
            &session.current_api_base_url,
            &session.qrcode,
            request.verify_code.as_deref(),
        )
        .await
    {
        Ok(result) => result,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "done": false, "error": err.to_string() })),
            );
        }
    };

    if result.status == "scaned_but_redirect" {
        if let Some(redirect_host) = result
            .redirect_host
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            let mut redirected_session = session.clone();
            redirected_session.current_api_base_url = normalize_wechat_base_url(redirect_host);
            let updated = {
                let mut onboard = state.wechat_onboard.lock().await;
                compare_set_session(&mut onboard, &session, redirected_session)
            };
            if !updated {
                return invalid_session_response();
            }
        }
        return (
            StatusCode::OK,
            Json(json!({ "done": false, "status": result.status })),
        );
    }

    if result.status == "confirmed" {
        let Some(bot_token) = result
            .bot_token
            .clone()
            .filter(|value| !value.trim().is_empty())
        else {
            return (
                StatusCode::OK,
                Json(
                    json!({ "done": false, "status": result.status, "error": "missing_bot_token" }),
                ),
            );
        };
        let account_id = result
            .ilink_bot_id
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| {
                if config.wechat.account_id.trim().is_empty() {
                    "wechat".to_string()
                } else {
                    config.wechat.account_id.clone()
                }
            });
        let base_url = result
            .baseurl
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| session.current_api_base_url.clone());
        let user_id = result.ilink_user_id.clone().unwrap_or_default();
        let claimed = {
            let mut onboard = state.wechat_onboard.lock().await;
            compare_clear_session(&mut onboard, &session)
        };
        if !claimed {
            return invalid_session_response();
        }
        let resolved_account_id = if account_id.trim().is_empty() {
            "wechat".to_string()
        } else {
            account_id.clone()
        };
        if let Err(response) = persist_wechat_onboard_account(
            &state,
            &resolved_account_id,
            &bot_token,
            &base_url,
            &user_id,
        )
        .await
        {
            let mut onboard = state.wechat_onboard.lock().await;
            restore_session_if_empty(&mut onboard, session);
            return response;
        }
        state
            .push_event(
                "info",
                "wechat_onboard_completed",
                format!("account={} user={}", account_id, user_id),
            )
            .await;
        im_api::start_bridge_task(
            &state,
            im_api::BridgeStartMode::Restart,
            "bridge restarted after WeChat onboarding",
        )
        .await;
        return (
            StatusCode::OK,
            Json(json!({
                "done": true,
                "status": result.status,
                "accountId": account_id,
                "userId": user_id,
            })),
        );
    }

    if result.status == "binded_redirect" {
        let cleared = {
            let mut onboard = state.wechat_onboard.lock().await;
            compare_clear_session(&mut onboard, &session)
        };
        if !cleared {
            return invalid_session_response();
        }
        state
            .push_event(
                "info",
                "wechat_onboard_already_connected",
                "already connected",
            )
            .await;
        return (
            StatusCode::OK,
            Json(json!({
                "done": true,
                "alreadyConnected": true,
                "status": result.status,
            })),
        );
    }

    if result.status == "expired" {
        let cleared = {
            let mut onboard = state.wechat_onboard.lock().await;
            compare_clear_session(&mut onboard, &session)
        };
        if !cleared {
            return invalid_session_response();
        }
        state
            .push_event(
                "warn",
                "wechat_onboard_expired",
                "upstream QR status expired",
            )
            .await;
    }

    (
        StatusCode::OK,
        Json(json!({
            "done": false,
            "status": result.status,
            "needVerifyCode": result.status == "need_verifycode",
            "error": match result.status.as_str() {
                "expired" => Some("expired"),
                "verify_code_blocked" => Some("verify_code_blocked"),
                _ => None,
            },
        })),
    )
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WecomOnboardStartResponse {
    session_key: String,
    qrcode_url: String,
    qr_svg: String,
    expires_in: u64,
    interval: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WecomOnboardPollRequest {
    session_key: String,
}

pub(super) async fn wecom_onboard_start(State(state): State<SharedState>) -> impl IntoResponse {
    let generation = next_onboard_generation();
    let session_key = format!("wecom-onboard-{}", Uuid::new_v4().simple());
    let reserved_session = WecomOnboardSession {
        generation,
        session_key: session_key.clone(),
        scode: String::new(),
        started_at_ms: unix_now_millis(),
    };
    let reserved = {
        let mut onboard = state.wecom_onboard.lock().await;
        reserve_newer_session(&mut onboard, reserved_session.clone())
    };
    if !reserved {
        return superseded_start_response();
    }
    let http_client = crate::outbound_http::get();
    match wecom_onboarding::start(&http_client).await {
        Ok(qr) => {
            let qr_svg = build_qr_svg(&qr.auth_url).unwrap_or_default();
            let session = WecomOnboardSession {
                generation,
                session_key: session_key.clone(),
                scode: qr.scode,
                started_at_ms: reserved_session.started_at_ms,
            };
            let installed = {
                let mut onboard = state.wecom_onboard.lock().await;
                compare_set_session(&mut onboard, &reserved_session, session)
            };
            if !installed {
                return superseded_start_response();
            }
            state
                .push_event("info", "wecom_onboard_started", "scan flow started")
                .await;
            (
                StatusCode::OK,
                Json(json!(WecomOnboardStartResponse {
                    session_key,
                    qrcode_url: qr.auth_url,
                    qr_svg,
                    expires_in: (WECOM_ONBOARD_TTL_MS / 1000) as u64,
                    interval: 3,
                })),
            )
        }
        Err(err) => {
            let mut onboard = state.wecom_onboard.lock().await;
            compare_clear_session(&mut onboard, &reserved_session);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": err.to_string() })),
            )
        }
    }
}

pub(super) async fn wecom_onboard_poll(
    State(state): State<SharedState>,
    Json(request): Json<WecomOnboardPollRequest>,
) -> impl IntoResponse {
    let Some(session) = state.wecom_onboard.lock().await.clone() else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "done": false, "error": "missing_session" })),
        );
    };
    if session.session_key != request.session_key {
        return invalid_session_response();
    }
    if unix_now_millis().saturating_sub(session.started_at_ms) > WECOM_ONBOARD_TTL_MS {
        let cleared = {
            let mut onboard = state.wecom_onboard.lock().await;
            compare_clear_session(&mut onboard, &session)
        };
        if !cleared {
            return invalid_session_response();
        }
        return (
            StatusCode::OK,
            Json(json!({ "done": false, "status": "expired", "error": "expired" })),
        );
    }

    let http_client = crate::outbound_http::get();
    match wecom_onboarding::poll(&http_client, &session.scode).await {
        Ok(QrPoll::Pending(status)) => {
            if status == "expired" {
                let cleared = {
                    let mut onboard = state.wecom_onboard.lock().await;
                    compare_clear_session(&mut onboard, &session)
                };
                if !cleared {
                    return invalid_session_response();
                }
            }
            (
                StatusCode::OK,
                Json(json!({ "done": false, "status": status })),
            )
        }
        Ok(QrPoll::Success { bot_id, secret }) => {
            let account_id = bot_id.clone();
            let claimed = {
                let mut onboard = state.wecom_onboard.lock().await;
                compare_clear_session(&mut onboard, &session)
            };
            if !claimed {
                return invalid_session_response();
            }
            if let Err(response) =
                persist_wecom_onboard_account(&state, &account_id, &bot_id, &secret).await
            {
                let mut onboard = state.wecom_onboard.lock().await;
                restore_session_if_empty(&mut onboard, session);
                return response;
            }
            state
                .push_event(
                    "info",
                    "wecom_onboard_completed",
                    format!("account={account_id}"),
                )
                .await;
            im_api::start_bridge_task(
                &state,
                im_api::BridgeStartMode::Restart,
                "bridge restarted after WeCom onboarding",
            )
            .await;
            (
                StatusCode::OK,
                Json(json!({ "done": true, "status": "success", "accountId": account_id })),
            )
        }
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "done": false, "error": err.to_string() })),
        ),
    }
}

fn build_qr_svg(content: &str) -> anyhow::Result<String> {
    let code = QrCode::new(content.as_bytes())?;
    Ok(code
        .render::<svg::Color>()
        .min_dimensions(220, 220)
        .dark_color(svg::Color("#20242a"))
        .light_color(svg::Color("#ffffff"))
        .build())
}

fn unix_now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

fn normalize_wechat_base_url(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        return DEFAULT_WECHAT_API_BASE.to_string();
    }
    if value.starts_with("http://") || value.starts_with("https://") {
        value.trim_end_matches('/').to_string()
    } else {
        format!("https://{}", value.trim_end_matches('/'))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{app_state::AppState, config::AppConfig};
    use tempfile::TempDir;

    fn wechat_session(key: &str, qrcode: &str, base_url: &str) -> WechatOnboardSession {
        wechat_session_with_generation(1, key, qrcode, base_url)
    }

    fn wechat_session_with_generation(
        generation: u64,
        key: &str,
        qrcode: &str,
        base_url: &str,
    ) -> WechatOnboardSession {
        WechatOnboardSession {
            generation,
            session_key: key.to_string(),
            qrcode: qrcode.to_string(),
            started_at_ms: 1,
            current_api_base_url: base_url.to_string(),
        }
    }

    fn wecom_session(key: &str, scode: &str) -> WecomOnboardSession {
        wecom_session_with_generation(1, key, scode)
    }

    fn wecom_session_with_generation(
        generation: u64,
        key: &str,
        scode: &str,
    ) -> WecomOnboardSession {
        WecomOnboardSession {
            generation,
            session_key: key.to_string(),
            scode: scode.to_string(),
            started_at_ms: 1,
        }
    }

    fn state_with_unwritable_config() -> (SharedState, TempDir) {
        let temp = tempfile::tempdir().expect("temp dir");
        let config_path = temp.path().join("config-directory");
        std::fs::create_dir(&config_path).expect("create config directory");
        let mut config = AppConfig::default();
        config.state_path = temp.path().join("state.json");
        config.bridge.enabled = false;
        (AppState::new(config_path, config, None, None), temp)
    }

    #[test]
    fn in_flight_wechat_poll_cannot_replace_or_clear_a_new_session() {
        let s1 = wechat_session("s1", "qr-1", "https://api-1.example");
        let s2 = wechat_session("s2", "qr-2", "https://api-2.example");
        let stale_result = wechat_session("s1", "qr-1", "https://redirect.example");
        let mut slot = Some(s1);
        let in_flight_s1 = slot.clone().expect("S1 should be active");

        slot = Some(s2.clone());

        assert!(!compare_set_session(&mut slot, &in_flight_s1, stale_result));
        assert!(!compare_clear_session(&mut slot, &in_flight_s1));
        assert!(
            slot.as_ref()
                .is_some_and(|current| current.matches_snapshot(&s2))
        );
    }

    #[test]
    fn slower_wechat_start_cannot_replace_a_newer_qr_session() {
        let older = wechat_session_with_generation(1, "older", "", "https://api.example");
        let newer = wechat_session_with_generation(2, "newer", "qr-new", "https://api.example");
        let older_result =
            wechat_session_with_generation(1, "older", "qr-old", "https://api.example");
        let mut slot = Some(newer.clone());

        assert!(!reserve_newer_session(&mut slot, older.clone()));
        assert!(!compare_set_session(&mut slot, &older, older_result));
        assert!(
            slot.as_ref()
                .is_some_and(|current| current.matches_snapshot(&newer))
        );
    }

    #[test]
    fn older_poll_for_same_wechat_session_cannot_overwrite_newer_snapshot() {
        let in_flight_snapshot = wechat_session("s1", "qr-1", "https://api.example");
        let newer_snapshot = wechat_session("s1", "qr-1", "https://new.example");
        let stale_result = wechat_session("s1", "qr-1", "https://old.example");
        let mut slot = Some(newer_snapshot.clone());

        assert!(!compare_set_session(
            &mut slot,
            &in_flight_snapshot,
            stale_result
        ));
        assert!(
            slot.as_ref()
                .is_some_and(|current| current.matches_snapshot(&newer_snapshot))
        );
    }

    #[test]
    fn in_flight_wecom_poll_cannot_clear_a_new_session() {
        let s1 = wecom_session("s1", "scode-1");
        let s2 = wecom_session("s2", "scode-2");
        let mut slot = Some(s1);
        let in_flight_s1 = slot.clone().expect("S1 should be active");

        slot = Some(s2.clone());

        assert!(!compare_clear_session(&mut slot, &in_flight_s1));
        assert!(
            slot.as_ref()
                .is_some_and(|current| current.matches_snapshot(&s2))
        );
    }

    #[test]
    fn slower_wecom_start_cannot_replace_a_newer_qr_session() {
        let older = wecom_session_with_generation(1, "older", "");
        let newer = wecom_session_with_generation(2, "newer", "scode-new");
        let older_result = wecom_session_with_generation(1, "older", "scode-old");
        let mut slot = Some(newer.clone());

        assert!(!reserve_newer_session(&mut slot, older.clone()));
        assert!(!compare_set_session(&mut slot, &older, older_result));
        assert!(
            slot.as_ref()
                .is_some_and(|current| current.matches_snapshot(&newer))
        );
    }

    #[test]
    fn failed_persistence_restore_does_not_replace_a_new_session() {
        let s1 = wechat_session("s1", "qr-1", "https://api-1.example");
        let s2 = wechat_session("s2", "qr-2", "https://api-2.example");
        let mut slot = Some(s2.clone());

        assert!(!restore_session_if_empty(&mut slot, s1));
        assert!(
            slot.as_ref()
                .is_some_and(|current| current.matches_snapshot(&s2))
        );
    }

    #[tokio::test]
    async fn feishu_persistence_rolls_back_in_memory_config_when_save_fails() {
        let (state, _temp) = state_with_unwritable_config();

        let result = persist_feishu_registration(
            &state,
            "app-id",
            "app-secret",
            Some("open-id"),
            Some("Feishu bot".to_string()),
        )
        .await;

        assert!(result.is_err());
        let config = state.config.lock().await;
        assert!(config.feishu_accounts.is_empty());
        assert!(!config.feishu.is_configured());
        assert!(!config.bridge.enabled);
    }

    #[tokio::test]
    async fn wechat_persistence_rolls_back_in_memory_config_when_save_fails() {
        let (state, _temp) = state_with_unwritable_config();

        let result = persist_wechat_onboard_account(
            &state,
            "wechat-account",
            "bot-token",
            "https://wechat.example",
            "user-id",
        )
        .await;

        assert!(result.is_err());
        let config = state.config.lock().await;
        assert!(config.wechat_accounts.is_empty());
        assert!(!config.wechat.is_configured());
        assert!(!config.bridge.enabled);
    }

    #[tokio::test]
    async fn wecom_persistence_rolls_back_in_memory_config_when_save_fails() {
        let (state, _temp) = state_with_unwritable_config();

        let result =
            persist_wecom_onboard_account(&state, "wecom-account", "bot-id", "secret").await;

        assert!(result.is_err());
        let config = state.config.lock().await;
        assert!(config.wecom_accounts.is_empty());
        assert!(!config.wecom.is_configured());
        assert!(!config.bridge.enabled);
    }
}
