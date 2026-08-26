use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use futures_util::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::Duration;

use crate::{
    app_state::{ImAccountProfile, ImAccountRuntimeState, SharedState, im_account_key},
    bridge, chain_log,
    config::{AppConfig, FeishuConfig, TelegramConfig, TelegramProjectGroupConfig},
    im::core::{
        i18n::im_text_for_state, session::bind_thread_to_route, thread::summarize_thread_title,
    },
    im::feishu::{FeishuApi, FeishuSettings},
    im::telegram::{api::TelegramApi, types::TelegramSettings},
    im_runtime::{RouteTarget, route_from_conversation_key},
    remote_control_backend,
    types::{ImPlatformKind, split_telegram_message_target, telegram_message_target},
};

const IM_ACCOUNT_PROFILE_REFRESH_MS: u128 = 60 * 60 * 1_000;
const IM_ACCOUNT_AVATAR_MAX_DATA_BYTES: usize = 512 * 1024;

enum ProfileAccount {
    Feishu(FeishuConfig),
    Telegram(TelegramConfig),
}

pub(super) async fn start_bridge(State(state): State<SharedState>) -> impl IntoResponse {
    {
        let mut config = state.config.lock().await;
        config.bridge.enabled = true;
        if let Err(err) = config.save(&state.config_path) {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "ok": false, "error": err.to_string() })),
            );
        }
    }
    let running = start_bridge_task(
        &state,
        BridgeStartMode::KeepExisting,
        "bridge start requested",
    )
    .await;
    (
        StatusCode::OK,
        Json(json!({ "ok": true, "running": running })),
    )
}

pub(super) async fn stop_bridge(State(state): State<SharedState>) -> impl IntoResponse {
    {
        let mut config = state.config.lock().await;
        config.bridge.enabled = false;
        if let Err(err) = config.save(&state.config_path) {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "ok": false, "error": err.to_string() })),
            );
        }
    }
    stop_bridge_task(&state).await;
    (
        StatusCode::OK,
        Json(json!({ "ok": true, "running": false })),
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SetImChannelEnabledRequest {
    channel: String,
    enabled: bool,
}

pub(super) async fn set_im_channel_enabled(
    State(state): State<SharedState>,
    Json(request): Json<SetImChannelEnabledRequest>,
) -> impl IntoResponse {
    let channel = request.channel.trim().to_ascii_lowercase();
    let should_run = {
        let mut config = state.config.lock().await;
        match channel.as_str() {
            "feishu" => {
                if request.enabled && !feishu_configured(&config) {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({ "ok": false, "error": "Feishu is not configured" })),
                    );
                }
                for account in &mut config.feishu_accounts {
                    account.enabled = request.enabled;
                }
                config.feishu.enabled = request.enabled;
            }
            "telegram" => {
                if request.enabled && !telegram_configured(&config) {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({ "ok": false, "error": "Telegram is not configured" })),
                    );
                }
                for account in &mut config.telegram_accounts {
                    account.enabled = request.enabled;
                }
                config.telegram.enabled = request.enabled;
            }
            "wechat" => {
                if request.enabled && !wechat_configured(&config) {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({ "ok": false, "error": "WeChat is not configured" })),
                    );
                }
                for account in &mut config.wechat_accounts {
                    account.enabled = request.enabled;
                }
                config.wechat.enabled = request.enabled;
            }
            "wecom" => {
                if request.enabled && !wecom_configured(&config) {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({ "ok": false, "error": "WeCom is not configured" })),
                    );
                }
                for account in &mut config.wecom_accounts {
                    account.enabled = request.enabled;
                }
                config.wecom.enabled = request.enabled;
            }
            _ => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "ok": false, "error": "unknown IM channel" })),
                );
            }
        }
        config.bridge.enabled = im_bridge_configured(&config);
        if let Err(err) = config.save(&state.config_path) {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "ok": false, "error": err.to_string() })),
            );
        }
        config.bridge.enabled
    };

    if should_run {
        start_bridge_task(
            &state,
            BridgeStartMode::Restart,
            "bridge restarted after IM channel toggle",
        )
        .await;
    } else {
        stop_bridge_task(&state).await;
    }

    (
        StatusCode::OK,
        Json(json!({
            "ok": true,
            "channel": channel,
            "enabled": request.enabled,
            "running": should_run,
        })),
    )
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ImAccountItem {
    platform: String,
    account_id: String,
    display_name: Option<String>,
    avatar_data: Option<String>,
    enabled: bool,
    configured: bool,
    secret_set: bool,
    connecting: bool,
    polling: bool,
    connected: bool,
    last_error: Option<String>,
    last_event_at_ms: Option<u128>,
    last_inbound_at_ms: Option<u128>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ImAccountsResponse {
    accounts: Vec<ImAccountItem>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ManageImAccountsResponse {
    service: crate::manage_api::ManageStatusResponse,
    accounts: Vec<ImAccountItem>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TelegramProjectGroupAccount {
    account_id: String,
    project_groups: Vec<TelegramProjectGroupConfig>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TelegramProjectGroupsResponse {
    accounts: Vec<TelegramProjectGroupAccount>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UpdateTelegramProjectGroupsRequest {
    account_id: String,
    project_groups: Vec<TelegramProjectGroupConfig>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SyncTelegramTopicsRequest {
    account_id: String,
    chat_id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TelegramTopicSyncItem {
    thread_id: String,
    title: String,
    status: String,
    topic_id: Option<i64>,
    error: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TelegramTopicSyncResponse {
    ok: bool,
    account_id: String,
    chat_id: String,
    total: usize,
    created: usize,
    skipped: usize,
    failed: usize,
    items: Vec<TelegramTopicSyncItem>,
}

pub(super) async fn im_accounts(State(state): State<SharedState>) -> Json<ImAccountsResponse> {
    Json(im_accounts_snapshot(&state).await)
}

pub(super) async fn manage_im_accounts(
    State(state): State<SharedState>,
) -> Json<ManageImAccountsResponse> {
    let accounts = im_accounts_snapshot(&state).await;
    Json(ManageImAccountsResponse {
        service: crate::manage_api::status_snapshot(&state),
        accounts: accounts.accounts,
    })
}

pub(super) async fn manage_telegram_project_groups(
    State(state): State<SharedState>,
) -> Json<TelegramProjectGroupsResponse> {
    let config = state.config.lock().await.clone();
    Json(TelegramProjectGroupsResponse {
        accounts: config
            .effective_telegram_accounts()
            .into_iter()
            .map(|account| TelegramProjectGroupAccount {
                account_id: account.account_id,
                project_groups: account.project_groups,
            })
            .collect(),
    })
}

pub(super) async fn update_telegram_project_groups(
    State(state): State<SharedState>,
    Json(request): Json<UpdateTelegramProjectGroupsRequest>,
) -> impl IntoResponse {
    let account_id = request.account_id.trim().to_string();
    if account_id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": "missing accountId" })),
        );
    }

    let mut project_groups = Vec::with_capacity(request.project_groups.len());
    let mut seen_chat_ids = std::collections::HashSet::new();
    for group in request.project_groups {
        let chat_id = group.chat_id.trim().to_string();
        let project_name = group.project_name.trim().to_string();
        let cwd = group.cwd.trim().to_string();
        if chat_id.is_empty() || project_name.is_empty() || cwd.is_empty() {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "ok": false,
                    "error": "each project group needs chatId, projectName, and cwd"
                })),
            );
        }
        if !seen_chat_ids.insert(chat_id.clone()) {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "ok": false, "error": "project group chatId must be unique" })),
            );
        }
        project_groups.push(TelegramProjectGroupConfig {
            chat_id,
            project_name,
            cwd,
        });
    }

    let mut config = state.config.lock().await;
    config.migrate_legacy_im_accounts();
    let Some(account) = config
        .telegram_accounts
        .iter_mut()
        .find(|account| account.account_id.trim() == account_id)
    else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "ok": false, "error": IM_ACCOUNT_NOT_FOUND_ERROR })),
        );
    };
    account.project_groups = project_groups.clone();
    if config.telegram.account_id.trim() == account_id {
        config.telegram.project_groups = project_groups.clone();
    }
    if let Err(err) = config.save(&state.config_path) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "error": err.to_string() })),
        );
    }

    (
        StatusCode::OK,
        Json(json!({
            "ok": true,
            "accountId": account_id,
            "projectGroups": project_groups,
            "restartRequired": true,
        })),
    )
}

pub(super) async fn sync_telegram_topics(
    State(state): State<SharedState>,
    Json(request): Json<SyncTelegramTopicsRequest>,
) -> impl IntoResponse {
    const PAGE_LIMIT: u32 = 100;
    const MAX_PAGES: usize = 20;

    let account_id = request.account_id.trim().to_string();
    let chat_id = request.chat_id.trim().to_string();
    if account_id.is_empty() || chat_id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": "accountId and chatId are required" })),
        );
    }

    let account = {
        let config = state.config.lock().await;
        config.telegram_account(&account_id)
    };
    let Some(account) = account else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "ok": false, "error": IM_ACCOUNT_NOT_FOUND_ERROR })),
        );
    };
    if !account.is_active() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": "Telegram 账号未启用或未配置" })),
        );
    }
    if account.project_group_for_chat(&chat_id).is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": "请先为这个群组配置项目目录" })),
        );
    }
    let project_cwd = account
        .project_group_for_chat(&chat_id)
        .map(|group| group.cwd.trim().to_string())
        .unwrap_or_default();

    // A sync can take longer than the management client's request timeout.
    // Keep retries serialized so a timed-out client cannot start a second
    // pass that creates duplicate Telegram topics.
    let _sync_guard = state.telegram_topic_sync_ops.lock().await;

    let raw_threads = match remote_control_backend::session_history_threads(
        &state,
        remote_control_backend::default_remote_client_key(),
        PAGE_LIMIT,
        MAX_PAGES,
        false,
    )
    .await
    {
        Ok(threads) => threads,
        Err(err) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "ok": false, "error": err.to_string() })),
            );
        }
    };
    let history_total = raw_threads.len();
    let mut raw_threads = raw_threads
        .into_iter()
        .filter(|thread| thread_belongs_to_project(thread, &project_cwd))
        .collect::<Vec<_>>();
    let filtered = history_total.saturating_sub(raw_threads.len());

    // Do not rely on the server's current list order. The API normally
    // returns newest-first, but the ordering is not part of the binding
    // contract; sort by the session's actual update timestamp instead.
    raw_threads.sort_by(|left, right| {
        thread_updated_at(left)
            .cmp(&thread_updated_at(right))
            .then_with(|| thread_id_for_sort(left).cmp(&thread_id_for_sort(right)))
    });

    let text = im_text_for_state(&state);
    let api = TelegramApi::new(TelegramSettings::from_app_config(&account));
    let mut items = Vec::with_capacity(raw_threads.len());
    let mut created = 0;
    let mut skipped = 0;
    let mut failed = 0;

    for raw_thread in raw_threads {
        let Some(thread_id) = raw_thread
            .get("id")
            .or_else(|| raw_thread.get("threadId"))
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
        else {
            continue;
        };
        let title = raw_thread
            .get("name")
            .or_else(|| raw_thread.get("title"))
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| summarize_thread_title(&raw_thread, text));
        let topic_name = truncate_telegram_topic_name(&title);

        let existing_bindings = {
            let persisted = state.persisted.lock().await;
            persisted
                .im_thread_bindings
                .iter()
                .filter_map(|(conversation_key, bound_thread_id)| {
                    (bound_thread_id == &thread_id)
                        .then(|| route_from_conversation_key(conversation_key))
                        .flatten()
                        .map(|route| (conversation_key.clone(), route))
                })
                .collect::<Vec<_>>()
        };

        let current_topic_bindings = existing_bindings
            .iter()
            .filter(|(_, route)| {
                if route.platform != ImPlatformKind::Telegram || route.account_id != account_id {
                    return false;
                }
                let (bound_chat_id, topic_id) = split_telegram_message_target(&route.chat_id);
                bound_chat_id == chat_id && topic_id.is_some()
            })
            .cloned()
            .collect::<Vec<_>>();
        let current_topic_keys = current_topic_bindings
            .iter()
            .map(|(key, _)| key.as_str())
            .collect::<Vec<_>>();
        let private_bindings = existing_bindings
            .iter()
            .filter(|(_, route)| {
                if route.platform != ImPlatformKind::Telegram || route.account_id != account_id {
                    return false;
                }
                let (bound_chat_id, topic_id) = split_telegram_message_target(&route.chat_id);
                topic_id.is_none()
                    && account
                        .allowed_chat_ids
                        .iter()
                        .any(|allowed| allowed.trim() == bound_chat_id)
            })
            .map(|(key, route)| (key.clone(), route.clone()))
            .collect::<Vec<_>>();
        let mut current_topic_alive = false;
        let mut alive_binding_key = None;
        let mut probe_error = None;
        let mut missing_binding_keys = Vec::new();
        for (conversation_key, route) in &current_topic_bindings {
            let (bound_chat_id, topic_id) = split_telegram_message_target(&route.chat_id);
            let Some(topic_id) = topic_id else {
                continue;
            };
            match api
                .edit_forum_topic(bound_chat_id, topic_id, &topic_name)
                .await
            {
                Ok(true) => {
                    current_topic_alive = true;
                    alive_binding_key = Some(conversation_key.clone());
                }
                Ok(false) => {
                    if !current_topic_alive && probe_error.is_none() {
                        probe_error = Some("Telegram 未确认原 Topic 是否存在".to_string());
                    }
                }
                Err(err)
                    if err
                        .downcast_ref::<crate::im::telegram::api::TelegramApiError>()
                        .is_some_and(|error| error.is_forum_topic_missing()) =>
                {
                    missing_binding_keys.push(conversation_key.clone());
                }
                Err(err) => {
                    state
                        .push_event(
                            "warn",
                            "telegram_topic_probe_failed",
                            format!("conversation={} err={err}", conversation_key),
                        )
                        .await;
                    if !current_topic_alive && probe_error.is_none() {
                        probe_error =
                            Some("无法确认原 Telegram Topic 是否仍存在，已保留原绑定".to_string());
                    }
                }
            }
        }
        if current_topic_alive {
            probe_error = None;
            if let Some(conversation_key) = alive_binding_key.as_deref()
                && let Err(err) =
                    persist_telegram_topic_name(&state, conversation_key, &topic_name).await
            {
                probe_error = Some(format!("保存 Topic 名称失败：{err}"));
            }
        }
        for conversation_key in &missing_binding_keys {
            if let Err(err) = crate::im::core::routing::clear_thread_binding_with_reason(
                &state,
                conversation_key,
                "telegram_topic_missing_during_sync",
            )
            .await
            {
                probe_error = Some(format!("清理已删除 Topic 的绑定失败：{err}"));
                break;
            }
        }
        let has_other_binding = existing_bindings.iter().any(|(conversation_key, _)| {
            !current_topic_keys.contains(&conversation_key.as_str())
                && !missing_binding_keys
                    .iter()
                    .any(|key| key == conversation_key)
                && !private_bindings
                    .iter()
                    .any(|(key, _)| key == conversation_key)
        });
        if let Some(error) = probe_error {
            failed += 1;
            items.push(TelegramTopicSyncItem {
                thread_id,
                title,
                status: "failed".to_string(),
                topic_id: None,
                error: Some(error),
            });
            continue;
        }
        if current_topic_alive || has_other_binding {
            skipped += 1;
            items.push(TelegramTopicSyncItem {
                thread_id,
                title,
                status: "skipped".to_string(),
                topic_id: None,
                error: Some(if current_topic_alive {
                    "已有 Topic".to_string()
                } else {
                    "已有其他 Telegram 对话绑定".to_string()
                }),
            });
            continue;
        }

        let topic = match api.create_forum_topic(&chat_id, &topic_name).await {
            Ok(topic) if topic.message_thread_id > 0 => topic,
            Ok(topic) => {
                failed += 1;
                items.push(TelegramTopicSyncItem {
                    thread_id: thread_id.clone(),
                    title: title.clone(),
                    status: "failed".to_string(),
                    topic_id: None,
                    error: Some(format!(
                        "Telegram 返回了无效 Topic ID：{}",
                        topic.message_thread_id
                    )),
                });
                continue;
            }
            Err(err) => {
                failed += 1;
                items.push(TelegramTopicSyncItem {
                    thread_id: thread_id.clone(),
                    title: title.clone(),
                    status: "failed".to_string(),
                    topic_id: None,
                    error: Some(err.to_string()),
                });
                continue;
            }
        };

        let mut transferred_private_bindings = Vec::new();
        let mut private_transfer_error = None;
        for (conversation_key, private_route) in &private_bindings {
            if let Err(err) = crate::im::core::routing::clear_thread_binding_with_reason(
                &state,
                conversation_key,
                "telegram_topic_transfer_from_private",
            )
            .await
            {
                let error = cleanup_created_topic(
                    &api,
                    &chat_id,
                    topic.message_thread_id,
                    anyhow::anyhow!("已绑定到私聊，自动转移失败：{err}"),
                )
                .await;
                failed += 1;
                items.push(TelegramTopicSyncItem {
                    thread_id: thread_id.clone(),
                    title: title.clone(),
                    status: "failed".to_string(),
                    topic_id: Some(topic.message_thread_id),
                    error: Some(error),
                });
                private_transfer_error = Some(());
                break;
            }
            transferred_private_bindings.push((conversation_key.clone(), private_route.clone()));
        }
        if private_transfer_error.is_some() {
            restore_private_bindings(&state, &thread_id, &transferred_private_bindings).await;
            continue;
        }

        let target = telegram_message_target(&chat_id, Some(topic.message_thread_id));
        let route = RouteTarget {
            platform: ImPlatformKind::Telegram,
            conversation_key: format!("telegram:{account_id}:{target}"),
            account_id: account_id.clone(),
            chat_id: target,
            remote_client_key: String::new(),
        }
        .with_deterministic_remote_client_key();

        if let Err(err) = remote_control_backend::resume_thread_for_client(
            &state,
            &route.remote_client_key,
            &thread_id,
            true,
        )
        .await
        {
            failed += 1;
            let error = cleanup_created_topic(&api, &chat_id, topic.message_thread_id, err).await;
            restore_private_bindings(&state, &thread_id, &transferred_private_bindings).await;
            items.push(TelegramTopicSyncItem {
                thread_id: thread_id.clone(),
                title: title.clone(),
                status: "failed".to_string(),
                topic_id: Some(topic.message_thread_id),
                error: Some(error),
            });
            continue;
        }

        if let Err(err) = bind_thread_to_route(
            &state,
            &route,
            &thread_id,
            None,
            route.remote_client_key.clone(),
        )
        .await
        {
            failed += 1;
            let error = cleanup_created_topic(&api, &chat_id, topic.message_thread_id, err).await;
            restore_private_bindings(&state, &thread_id, &transferred_private_bindings).await;
            items.push(TelegramTopicSyncItem {
                thread_id,
                title,
                status: "failed".to_string(),
                topic_id: Some(topic.message_thread_id),
                error: Some(error),
            });
            continue;
        }

        if let Err(err) =
            persist_telegram_topic_name(&state, &route.conversation_key, &topic_name).await
        {
            state
                .push_event(
                    "warn",
                    "telegram_topic_binding_state_save_failed",
                    format!("thread={} err={err}", thread_id),
                )
                .await;
        }

        let migration_note = if transferred_private_bindings.is_empty() {
            "已创建 Topic".to_string()
        } else {
            "已从私聊转移到项目群，Codex 会话已保留".to_string()
        };
        created += 1;
        items.push(TelegramTopicSyncItem {
            thread_id,
            title,
            status: "created".to_string(),
            topic_id: Some(topic.message_thread_id),
            error: Some(migration_note),
        });
    }

    state
        .push_event(
            "info",
            "telegram_topics_sync_completed",
            format!(
                "account={} chat={} total={} filtered={} created={} skipped={} failed={}",
                account_id,
                chat_id,
                items.len(),
                filtered,
                created,
                skipped,
                failed
            ),
        )
        .await;

    (
        StatusCode::OK,
        Json(json!(TelegramTopicSyncResponse {
            ok: failed == 0,
            account_id,
            chat_id,
            total: items.len(),
            created,
            skipped,
            failed,
            items,
        })),
    )
}

async fn persist_telegram_topic_name(
    state: &SharedState,
    conversation_key: &str,
    topic_name: &str,
) -> anyhow::Result<()> {
    let mut persisted = state.persisted.lock().await;
    let Some(binding) = persisted
        .telegram_topic_binding_states
        .get_mut(conversation_key)
    else {
        return Ok(());
    };
    binding.topic_name = topic_name.to_string();
    if binding.codex_title.trim().is_empty() {
        binding.codex_title = topic_name.to_string();
    }
    if binding.last_synced_codex_title.trim().is_empty() {
        binding.last_synced_codex_title = binding.codex_title.clone();
    }
    if binding.last_synced_topic_name.trim().is_empty() {
        binding.last_synced_topic_name = topic_name.to_string();
    }
    binding.last_checked_at_ms = crate::types::now_ms();
    let path = state.config.lock().await.state_path.clone();
    persisted.save(&path)
}

async fn cleanup_created_topic(
    api: &TelegramApi,
    chat_id: &str,
    topic_id: i64,
    binding_error: anyhow::Error,
) -> String {
    let binding_error = binding_error.to_string();
    match api.delete_forum_topic(chat_id, topic_id).await {
        Ok(true) => format!("{binding_error}（已清理未绑定的 Topic）"),
        Ok(false) => format!("{binding_error}（清理未绑定的 Topic 未确认成功）"),
        Err(cleanup_error) => {
            format!("{binding_error}（清理未绑定的 Topic 失败：{cleanup_error}）")
        }
    }
}

async fn restore_private_bindings(
    state: &SharedState,
    thread_id: &str,
    bindings: &[(String, RouteTarget)],
) {
    for (conversation_key, route) in bindings {
        if remote_control_backend::resume_thread_for_client(
            state,
            &route.remote_client_key,
            thread_id,
            true,
        )
        .await
        .is_ok()
        {
            let _ = bind_thread_to_route(
                state,
                route,
                thread_id,
                None,
                route.remote_client_key.clone(),
            )
            .await;
        } else {
            state
                .push_event(
                    "warn",
                    "telegram_topic_private_binding_restore_failed",
                    format!("thread={} conversation={}", thread_id, conversation_key),
                )
                .await;
        }
    }
}

fn thread_belongs_to_project(thread: &serde_json::Value, project_cwd: &str) -> bool {
    let project_cwd = project_cwd.trim();
    let Some(thread_cwd) = thread
        .get("cwd")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return false;
    };
    let project_root = Path::new(project_cwd);
    let thread_root = Path::new(thread_cwd);
    !project_cwd.is_empty()
        && (thread_root == project_root || thread_root.starts_with(project_root))
}

fn thread_updated_at(thread: &serde_json::Value) -> i128 {
    [
        "updatedAt",
        "updated_at",
        "lastUpdatedAt",
        "last_updated_at",
    ]
    .iter()
    .find_map(|key| {
        let value = thread.get(*key)?;
        value
            .as_i64()
            .map(i128::from)
            .or_else(|| value.as_u64().map(i128::from))
            .or_else(|| value.as_str().and_then(|value| value.trim().parse().ok()))
    })
    .unwrap_or_default()
}

fn thread_id_for_sort(thread: &serde_json::Value) -> &str {
    thread
        .get("id")
        .or_else(|| thread.get("threadId"))
        .and_then(|value| value.as_str())
        .unwrap_or_default()
}

fn truncate_telegram_topic_name(title: &str) -> String {
    let title = title.trim();
    let title = if title.is_empty() {
        "未命名会话"
    } else {
        title
    };
    title.chars().take(64).collect()
}

pub(super) async fn im_accounts_snapshot(state: &SharedState) -> ImAccountsResponse {
    let config = state.config.lock().await.clone();
    schedule_im_account_profiles_refresh(state, &config);
    let runtime = state.im_accounts.lock().await.clone();
    ImAccountsResponse {
        accounts: im_account_items(state, &config, &runtime),
    }
}

/// API contract with the SwiftUI client: `APIClient.performIMMutation`
/// matches this exact string to tell "the account does not exist" apart from
/// "an older daemon has no versioned account routes" (both are 404).
/// Do not reword without updating the Swift client and its tests.
pub(super) const IM_ACCOUNT_NOT_FOUND_ERROR: &str = "IM account not found";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SetImAccountEnabledRequest {
    platform: String,
    account_id: String,
    enabled: bool,
}

pub(super) async fn set_im_account_enabled(
    State(state): State<SharedState>,
    Json(request): Json<SetImAccountEnabledRequest>,
) -> impl IntoResponse {
    let platform = request.platform.trim().to_ascii_lowercase();
    let account_id = request.account_id.trim().to_string();
    if account_id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": "missing accountId" })),
        );
    }
    if !is_supported_im_platform(&platform) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": "unknown IM platform" })),
        );
    }
    let should_run = {
        let mut config = state.config.lock().await;
        // Check the effective view before migration so a failed request does
        // not leave a legacy singleton copied into the in-memory account list.
        if !config.has_im_account(&platform, &account_id) {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "ok": false, "error": IM_ACCOUNT_NOT_FOUND_ERROR })),
            );
        }
        let previous_config = config.clone();
        config.migrate_legacy_im_accounts();
        if !config.set_im_account_enabled(&platform, &account_id, request.enabled) {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "ok": false, "error": IM_ACCOUNT_NOT_FOUND_ERROR })),
            );
        }
        set_legacy_im_account_enabled(&mut config, &platform, &account_id, request.enabled);
        config.bridge.enabled = im_bridge_configured(&config);
        if let Err(err) = config.save(&state.config_path) {
            *config = previous_config;
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "ok": false, "error": err.to_string() })),
            );
        }
        config.bridge.enabled
    };
    if should_run {
        start_bridge_task(
            &state,
            BridgeStartMode::Restart,
            "bridge restarted after IM account toggle",
        )
        .await;
    } else {
        stop_bridge_task(&state).await;
    }
    (
        StatusCode::OK,
        Json(
            json!({ "ok": true, "platform": platform, "accountId": account_id, "enabled": request.enabled }),
        ),
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DeleteImAccountRequest {
    platform: String,
    account_id: String,
}

pub(super) async fn delete_im_account(
    State(state): State<SharedState>,
    Json(request): Json<DeleteImAccountRequest>,
) -> impl IntoResponse {
    let platform = request.platform.trim().to_ascii_lowercase();
    let account_id = request.account_id.trim().to_string();
    if account_id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": "missing accountId" })),
        );
    }
    if !is_supported_im_platform(&platform) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": "unknown IM platform" })),
        );
    }
    let should_run = {
        let mut config = state.config.lock().await;
        // Avoid migration and legacy cleanup for a request that cannot remove
        // an account. This keeps a failed delete side-effect free.
        if !config.has_im_account(&platform, &account_id) {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "ok": false, "error": IM_ACCOUNT_NOT_FOUND_ERROR })),
            );
        }
        let previous_config = config.clone();
        config.migrate_legacy_im_accounts();
        let removed = config.remove_im_account(&platform, &account_id);
        if !removed {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "ok": false, "error": IM_ACCOUNT_NOT_FOUND_ERROR })),
            );
        }
        clear_legacy_im_account(&mut config, &platform, &account_id);
        config.bridge.enabled = im_bridge_configured(&config);
        if let Err(err) = config.save(&state.config_path) {
            *config = previous_config;
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "ok": false, "error": err.to_string() })),
            );
        }
        config.bridge.enabled
    };
    clear_im_account_bindings(&state, &platform, &account_id).await;
    if should_run {
        start_bridge_task(
            &state,
            BridgeStartMode::Restart,
            "bridge restarted after IM account deletion",
        )
        .await;
    } else {
        stop_bridge_task(&state).await;
    }
    (
        StatusCode::OK,
        Json(json!({ "ok": true, "platform": platform, "accountId": account_id })),
    )
}

pub(super) async fn stop_bridge_task(state: &SharedState) {
    let mut task = state.bridge_task.lock().await;
    if let Some(handle) = task.take() {
        handle.abort();
    }
    state.runtime.lock().await.invalidate_bridge_generation();
    {
        let mut ws = state.feishu_ws.lock().await;
        ws.connecting = false;
        ws.connected = false;
    }
    {
        let mut wechat = state.wechat.lock().await;
        wechat.polling = false;
        wechat.connected = false;
    }
    {
        let mut telegram = state.telegram.lock().await;
        telegram.polling = false;
        telegram.connected = false;
    }
    {
        let mut accounts = state.im_accounts.lock().await;
        for account in accounts.values_mut() {
            account.connecting = false;
            account.polling = false;
            account.connected = false;
        }
    }
    state
        .push_event("warn", "bridge_stopped", "bridge task aborted")
        .await;
}

#[derive(Clone, Copy)]
pub(super) enum BridgeStartMode {
    KeepExisting,
    Restart,
}

pub(super) async fn start_bridge_task(
    state: &SharedState,
    mode: BridgeStartMode,
    event_message: &'static str,
) -> bool {
    if state.lifecycle_admission.state() != crate::app_state::LifecycleAdmissionState::Active {
        state
            .push_event(
                "info",
                "bridge_start_deferred_during_lifecycle_drain",
                "bridge start deferred while the daemon lifecycle is draining",
            )
            .await;
        return false;
    }
    let config = state.config.lock().await.clone();
    if !config.bridge.enabled {
        state
            .push_event("warn", "bridge_disabled", "bridge disabled by config")
            .await;
        return false;
    }
    if !im_bridge_configured(&config) {
        state
            .push_event(
                "warn",
                "bridge_waiting_for_im_config",
                "bridge is waiting for Feishu, Telegram, WeChat, or WeCom configuration",
            )
            .await;
        return false;
    }

    let restart = matches!(mode, BridgeStartMode::Restart);
    let mut aborted_existing = false;
    {
        let mut task = state.bridge_task.lock().await;
        let running = task
            .as_ref()
            .map(|handle| !handle.is_finished())
            .unwrap_or(false);
        if running && !restart {
            return true;
        }
        if let Some(handle) = task.take()
            && !handle.is_finished()
        {
            handle.abort();
            aborted_existing = true;
        }
        let bridge_state = state.clone();
        *task = Some(tokio::spawn(async move {
            bridge::start_bridge(bridge_state).await;
        }));
    }

    if restart || aborted_existing {
        state.runtime.lock().await.invalidate_bridge_generation();
        let mut ws = state.feishu_ws.lock().await;
        ws.connecting = false;
        ws.connected = false;
        ws.last_error = None;
        let mut wechat = state.wechat.lock().await;
        wechat.polling = false;
        wechat.connected = false;
        wechat.last_error = None;
        let mut telegram = state.telegram.lock().await;
        telegram.polling = false;
        telegram.connected = false;
        telegram.last_error = None;
        let mut accounts = state.im_accounts.lock().await;
        for account in accounts.values_mut() {
            account.connecting = false;
            account.polling = false;
            account.connected = false;
            account.last_error = None;
        }
    }
    state
        .push_event("info", "bridge_start_requested", event_message)
        .await;
    true
}

pub(super) fn feishu_configured(config: &AppConfig) -> bool {
    config
        .effective_feishu_accounts()
        .iter()
        .any(|account| account.is_configured())
}

pub(super) fn telegram_configured(config: &AppConfig) -> bool {
    config
        .effective_telegram_accounts()
        .iter()
        .any(|account| account.is_configured())
}

pub(super) fn wechat_configured(config: &AppConfig) -> bool {
    config
        .effective_wechat_accounts()
        .iter()
        .any(|account| account.is_configured())
}

pub(super) fn wecom_configured(config: &AppConfig) -> bool {
    config
        .effective_wecom_accounts()
        .iter()
        .any(|account| account.is_configured())
}

fn feishu_active(config: &AppConfig) -> bool {
    config
        .effective_feishu_accounts()
        .iter()
        .any(|account| account.is_active())
}

fn telegram_active(config: &AppConfig) -> bool {
    config
        .effective_telegram_accounts()
        .iter()
        .any(|account| account.is_active())
}

fn wechat_active(config: &AppConfig) -> bool {
    config
        .effective_wechat_accounts()
        .iter()
        .any(|account| account.is_active())
}

fn wecom_active(config: &AppConfig) -> bool {
    config
        .effective_wecom_accounts()
        .iter()
        .any(|account| account.is_active())
}

pub(super) fn im_bridge_configured(config: &AppConfig) -> bool {
    feishu_active(config)
        || telegram_active(config)
        || wechat_active(config)
        || wecom_active(config)
}

fn schedule_im_account_profiles_refresh(state: &SharedState, config: &AppConfig) {
    let now = crate::types::now_ms();
    let accounts = profile_accounts(config);
    let stale = state
        .im_account_profiles
        .try_lock()
        .ok()
        .is_some_and(|profiles| {
            accounts.iter().any(|(platform, account_id, account)| {
                let key = im_account_key(*platform, account_id);
                let fresh = profiles
                    .get(&key)
                    .and_then(|profile| profile.avatar_checked_at_ms)
                    .is_some_and(|checked| {
                        now.saturating_sub(checked) < IM_ACCOUNT_PROFILE_REFRESH_MS
                    });
                let configured = match account {
                    ProfileAccount::Feishu(account) => account.is_configured(),
                    ProfileAccount::Telegram(account) => account.is_configured(),
                };
                configured && !fresh
            })
        });
    if !stale
        || state
            .im_account_profile_refresh
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
    {
        return;
    }

    let state = state.clone();
    let config = config.clone();
    tokio::spawn(async move {
        let _reset = ProfileRefreshReset(state.clone());
        refresh_im_account_profiles(&state, &config).await;
    });
}

struct ProfileRefreshReset(SharedState);

impl Drop for ProfileRefreshReset {
    fn drop(&mut self) {
        self.0
            .im_account_profile_refresh
            .store(0, Ordering::Release);
    }
}

fn profile_accounts(config: &AppConfig) -> Vec<(ImPlatformKind, String, ProfileAccount)> {
    config
        .effective_feishu_accounts()
        .into_iter()
        .map(|account| {
            (
                ImPlatformKind::Feishu,
                account.account_id.clone(),
                ProfileAccount::Feishu(account),
            )
        })
        .chain(
            config
                .effective_telegram_accounts()
                .into_iter()
                .map(|account| {
                    (
                        ImPlatformKind::Telegram,
                        account.account_id.clone(),
                        ProfileAccount::Telegram(account),
                    )
                }),
        )
        .collect::<Vec<_>>()
}

async fn refresh_im_account_profiles(state: &SharedState, config: &AppConfig) {
    let now = crate::types::now_ms();
    let profiles = state.im_account_profiles.lock().await.clone();
    let tasks = profile_accounts(config).into_iter().filter_map(
        |(platform, account_id, profile_account)| {
            let key = im_account_key(platform, &account_id);
            let fresh = profiles
                .get(&key)
                .and_then(|profile| profile.avatar_checked_at_ms)
                .is_some_and(|checked| now.saturating_sub(checked) < IM_ACCOUNT_PROFILE_REFRESH_MS);
            let configured = match &profile_account {
                ProfileAccount::Feishu(account) => account.is_configured(),
                ProfileAccount::Telegram(account) => account.is_configured(),
            };
            if fresh || !configured {
                return None;
            }
            Some(refresh_one_im_account_profile(
                platform,
                key,
                profile_account,
                now,
            ))
        },
    );

    let mut tasks = stream::iter(tasks).buffer_unordered(4);
    while let Some((key, profile)) = tasks.next().await {
        state.im_account_profiles.lock().await.insert(key, profile);
    }
}

async fn refresh_one_im_account_profile(
    platform: ImPlatformKind,
    key: String,
    profile_account: ProfileAccount,
    now: u128,
) -> (String, ImAccountProfile) {
    let mut profile = ImAccountProfile {
        avatar_checked_at_ms: Some(now),
        ..Default::default()
    };
    match platform {
        ImPlatformKind::Telegram => {
            let ProfileAccount::Telegram(account) = profile_account else {
                return (key, profile);
            };
            let api = TelegramApi::new(TelegramSettings::from_app_config(&account));
            let result = tokio::time::timeout(Duration::from_secs(5), async {
                let user = api.get_me().await?;
                let photos = api.get_user_profile_photos(user.id, 1).await?;
                let photo = photos
                    .photos
                    .first()
                    .and_then(|sizes| sizes.iter().max_by_key(|size| size.width * size.height));
                let Some(photo) = photo else {
                    return Ok::<_, anyhow::Error>(None);
                };
                let file = api.get_file(&photo.file_id).await?;
                let Some(path) = file.file_path else {
                    return Ok(None);
                };
                let bytes = api.download_file(&path).await?;
                let mime = mime_guess::from_path(&path)
                    .first_raw()
                    .filter(|mime| mime.starts_with("image/"))
                    .unwrap_or("image/jpeg")
                    .to_string();
                Ok(Some((bytes, mime)))
            })
            .await;
            if let Ok(Ok(Some((bytes, mime)))) = result {
                profile.avatar_data = image_data_url(bytes, &mime);
                profile.avatar_mime_type = Some(mime);
            }
        }
        ImPlatformKind::Feishu => {
            let ProfileAccount::Feishu(account) = profile_account else {
                return (key, profile);
            };
            let api = FeishuApi::new(FeishuSettings::from_app_config(&account));
            let result = tokio::time::timeout(Duration::from_secs(5), async {
                let info = api.get_application_info(account.app_id.trim()).await?;
                let Some(url) = info.avatar_url else {
                    return Ok::<_, anyhow::Error>(None);
                };
                Ok(Some(api.download_application_avatar(&url).await?))
            })
            .await;
            if let Ok(Ok(Some((bytes, mime)))) = result {
                profile.avatar_data = image_data_url(bytes, &mime);
                profile.avatar_mime_type = Some(mime);
            }
        }
        _ => {}
    }
    (key, profile)
}

fn image_data_url(bytes: Vec<u8>, mime_type: &str) -> Option<String> {
    if bytes.is_empty() || bytes.len() > IM_ACCOUNT_AVATAR_MAX_DATA_BYTES {
        return None;
    }
    let mime = mime_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if !mime.starts_with("image/") {
        return None;
    }
    Some(format!(
        "data:{mime};base64,{}",
        BASE64_STANDARD.encode(bytes)
    ))
}

fn im_account_items(
    state: &SharedState,
    config: &AppConfig,
    runtime: &HashMap<String, ImAccountRuntimeState>,
) -> Vec<ImAccountItem> {
    let profiles = state.im_account_profiles.try_lock().ok();
    let mut accounts = Vec::new();
    for account in config.effective_feishu_accounts() {
        accounts.push(im_account_item(
            profiles.as_deref(),
            ImPlatformKind::Feishu,
            &account.account_id,
            non_empty_string(&account.display_name)
                .or_else(|| non_empty_string(&account.app_id))
                .or_else(|| Some("飞书机器人".to_string())),
            account.enabled,
            account.is_configured(),
            account.is_configured(),
            runtime,
        ));
    }
    for account in config.effective_telegram_accounts() {
        accounts.push(im_account_item(
            profiles.as_deref(),
            ImPlatformKind::Telegram,
            &account.account_id,
            non_empty_string(&account.display_name).or_else(|| Some("Telegram 机器人".to_string())),
            account.enabled,
            account.is_configured(),
            !account.bot_token.trim().is_empty(),
            runtime,
        ));
    }
    for account in config.effective_wechat_accounts() {
        accounts.push(im_account_item(
            profiles.as_deref(),
            ImPlatformKind::Wechat,
            &account.account_id,
            non_empty_string(&account.display_name).or_else(|| Some("微信机器人".to_string())),
            account.enabled,
            account.is_configured(),
            !account.bot_token.trim().is_empty(),
            runtime,
        ));
    }
    for account in config.effective_wecom_accounts() {
        accounts.push(im_account_item(
            profiles.as_deref(),
            ImPlatformKind::Wecom,
            &account.account_id,
            non_empty_string(&account.display_name).or_else(|| Some("企业微信机器人".to_string())),
            account.enabled,
            account.is_configured(),
            !account.secret.trim().is_empty(),
            runtime,
        ));
    }
    accounts
}

fn im_account_item(
    profiles: Option<&HashMap<String, ImAccountProfile>>,
    platform: ImPlatformKind,
    account_id: &str,
    display_name: Option<String>,
    enabled: bool,
    configured: bool,
    secret_set: bool,
    runtime: &HashMap<String, ImAccountRuntimeState>,
) -> ImAccountItem {
    let runtime = runtime.get(&im_account_key(platform, account_id));
    let avatar_data = profiles
        .and_then(|profiles| profiles.get(&im_account_key(platform, account_id)))
        .and_then(|profile| profile.avatar_data.clone());
    ImAccountItem {
        platform: platform.key().to_string(),
        account_id: account_id.to_string(),
        display_name,
        avatar_data,
        enabled,
        configured,
        secret_set,
        connecting: runtime.is_some_and(|state| state.connecting),
        polling: runtime.is_some_and(|state| state.polling),
        connected: runtime.is_some_and(|state| state.connected),
        last_error: runtime.and_then(|state| state.last_error.clone()),
        last_event_at_ms: runtime.and_then(|state| state.last_event_at_ms),
        last_inbound_at_ms: runtime.and_then(|state| state.last_inbound_at_ms),
    }
}

fn set_legacy_im_account_enabled(
    config: &mut AppConfig,
    platform: &str,
    account_id: &str,
    enabled: bool,
) {
    if !legacy_im_account_matches(config, platform, account_id) {
        return;
    }
    match platform {
        "feishu" => config.feishu.enabled = enabled,
        "telegram" => config.telegram.enabled = enabled,
        "wechat" => config.wechat.enabled = enabled,
        "wecom" => config.wecom.enabled = enabled,
        _ => {}
    }
}

fn is_supported_im_platform(platform: &str) -> bool {
    im_platform_from_key(platform).is_some()
}

fn clear_legacy_im_account(config: &mut AppConfig, platform: &str, account_id: &str) {
    if !legacy_im_account_matches(config, platform, account_id) {
        return;
    }
    match platform {
        "feishu" => config.feishu = Default::default(),
        "telegram" => config.telegram = Default::default(),
        "wechat" => config.wechat = Default::default(),
        "wecom" => config.wecom = Default::default(),
        _ => {}
    }
}

fn legacy_im_account_matches(config: &AppConfig, platform: &str, account_id: &str) -> bool {
    match platform {
        "feishu" => {
            config.feishu.account_id.trim() == account_id
                || (config.feishu.account_id.trim().is_empty()
                    && (config.feishu.app_id.trim() == account_id
                        || config.bridge.account_id.trim() == account_id))
        }
        "telegram" => {
            config.telegram.account_id.trim() == account_id
                || (config.telegram.account_id.trim().is_empty() && account_id == "telegram")
        }
        "wechat" => {
            config.wechat.account_id.trim() == account_id
                || (config.wechat.account_id.trim().is_empty() && account_id == "wechat")
        }
        "wecom" => {
            config.wecom.account_id.trim() == account_id
                || (config.wecom.account_id.trim().is_empty() && account_id == "wecom")
        }
        _ => false,
    }
}

async fn clear_im_account_bindings(state: &SharedState, platform: &str, account_id: &str) {
    let _binding_guard = state.im_route_binding_ops.lock().await;
    {
        let mut runtime = state.runtime.lock().await;
        let removed = runtime
            .route_by_thread
            .iter()
            .filter(|&(_, route)| {
                route.platform.key() == platform && route.account_id == account_id
            })
            .map(|(thread_id, route)| (thread_id.clone(), route.clone()))
            .collect::<Vec<_>>();
        runtime.route_by_thread.retain(|_, route| {
            !(route.platform.key() == platform && route.account_id == account_id)
        });
        for (thread_id, route) in removed {
            chain_log::write_line(format!(
                "[im_route] level=warn event=unbind_account reason=clear_im_account_bindings thread={} platform={} account={} chat={} conversation={}",
                thread_id,
                route.platform.key(),
                route.account_id,
                route.chat_id,
                route.conversation_key
            ));
        }
    }
    let persisted_cleanup_error = if platform == ImPlatformKind::Telegram.key() {
        let mut persisted = state.persisted.lock().await;
        let previous_len = persisted.im_thread_bindings.len();
        persisted.im_thread_bindings.retain(|conversation_key, _| {
            !crate::im_runtime::route_from_conversation_key(conversation_key).is_some_and(|route| {
                route.platform == ImPlatformKind::Telegram && route.account_id == account_id
            })
        });
        if persisted.im_thread_bindings.len() == previous_len {
            None
        } else {
            let state_path = state.config.lock().await.state_path.clone();
            persisted.save(&state_path).err().map(|err| err.to_string())
        }
    } else {
        None
    };
    if let Some(err) = persisted_cleanup_error {
        state
            .push_event(
                "warn",
                "im_persisted_binding_cleanup_failed",
                format!("platform={platform} account={account_id} err={err}"),
            )
            .await;
    }
    if let Some(kind) = im_platform_from_key(platform) {
        state
            .im_account_profiles
            .lock()
            .await
            .remove(&im_account_key(kind, account_id));
        state
            .im_accounts
            .lock()
            .await
            .remove(&im_account_key(kind, account_id));
    }
}

fn im_platform_from_key(platform: &str) -> Option<ImPlatformKind> {
    ImPlatformKind::from_key(platform)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct FeishuBotStatus {
    configured: bool,
    enabled: bool,
    app_id: Option<String>,
    display_name: Option<String>,
    allowed_open_ids: usize,
    error: Option<String>,
}

pub(super) async fn feishu_bot_status(State(state): State<SharedState>) -> Json<FeishuBotStatus> {
    let config = state.config.lock().await.clone();
    let account = config.effective_feishu_accounts().into_iter().next();
    let app_id = account
        .as_ref()
        .and_then(|account| non_empty_string(&account.app_id));
    let mut display_name = account
        .as_ref()
        .and_then(|account| non_empty_string(&account.display_name));
    let configured = account
        .as_ref()
        .is_some_and(|account| account.is_configured());
    let mut error = None;

    if let Some(account) = account.as_ref()
        && configured
        && display_name.is_none()
    {
        let api = FeishuApi::new(FeishuSettings::from_app_config(account));
        match api
            .get_application_display_name(app_id.as_deref().unwrap_or_default())
            .await
        {
            Ok(Some(name)) => {
                display_name = Some(name.clone());
                let mut config = state.config.lock().await;
                if let Some(mut account) = config.feishu_account(&account.account_id)
                    && account.display_name.trim().is_empty()
                {
                    account.display_name = name;
                    config.upsert_feishu_account(account);
                    if let Err(err) = config.save(&state.config_path) {
                        error = Some(err.to_string());
                    }
                }
            }
            Ok(None) => {}
            Err(err) => error = Some(err.to_string()),
        }
    }

    Json(FeishuBotStatus {
        configured,
        enabled: account.as_ref().is_some_and(|account| account.enabled),
        app_id,
        display_name,
        allowed_open_ids: account
            .as_ref()
            .map(|account| account.allowed_open_ids.len())
            .unwrap_or_default(),
        error,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TelegramBotStatus {
    configured: bool,
    enabled: bool,
    token_set: bool,
    display_name: Option<String>,
    username: Option<String>,
    mention_only: bool,
    allowed_chat_ids: usize,
    polling: bool,
    connected: bool,
    last_error: Option<String>,
    error: Option<String>,
}

pub(super) async fn telegram_bot_status(
    State(state): State<SharedState>,
) -> Json<TelegramBotStatus> {
    let config = state.config.lock().await.clone();
    let telegram = state.telegram.lock().await.clone();
    let account = config.effective_telegram_accounts().into_iter().next();
    let configured = account
        .as_ref()
        .is_some_and(|account| account.is_configured());
    let mut display_name = account
        .as_ref()
        .and_then(|account| non_empty_string(&account.display_name));
    let mut username = None;
    let mut error = None;

    if let Some(account) = account.as_ref()
        && configured
        && display_name.is_none()
    {
        let api = TelegramApi::new(TelegramSettings::from_app_config(account));
        match tokio::time::timeout(std::time::Duration::from_secs(3), api.get_me()).await {
            Ok(Ok(user)) => {
                username = user
                    .username
                    .as_deref()
                    .map(|value| value.trim_start_matches('@').to_string())
                    .filter(|value| !value.is_empty());
                display_name = telegram_user_display_name(&user);
                if let Some(name) = display_name.clone() {
                    let mut config = state.config.lock().await;
                    if let Some(mut account) = config.telegram_account(&account.account_id)
                        && account.display_name.trim().is_empty()
                    {
                        account.display_name = name;
                        config.upsert_telegram_account(account);
                        if let Err(err) = config.save(&state.config_path) {
                            error = Some(err.to_string());
                        }
                    }
                }
            }
            Ok(Err(err)) => error = Some(err.to_string()),
            Err(_) => error = Some("telegram getMe timeout".to_string()),
        }
    }

    Json(TelegramBotStatus {
        configured,
        enabled: account.as_ref().is_some_and(|account| account.enabled),
        token_set: account
            .as_ref()
            .is_some_and(|account| !account.bot_token.trim().is_empty()),
        display_name,
        username,
        mention_only: account.as_ref().is_some_and(|account| account.mention_only),
        allowed_chat_ids: account
            .as_ref()
            .map(|account| account.allowed_chat_ids.len())
            .unwrap_or_default(),
        polling: telegram.polling,
        connected: telegram.connected,
        last_error: telegram.last_error,
        error,
    })
}

fn telegram_user_display_name(user: &crate::im::telegram::api::TelegramUser) -> Option<String> {
    let username = user
        .username
        .as_deref()
        .map(|value| value.trim_start_matches('@'))
        .filter(|value| !value.is_empty());
    let name = [user.first_name.as_deref(), user.last_name.as_deref()]
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join(" ");

    match (name.is_empty(), username) {
        (false, Some(username)) => Some(format!("{name} (@{username})")),
        (false, None) => Some(name),
        (true, Some(username)) => Some(format!("@{username}")),
        (true, None) => None,
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ConfigureTelegramBotRequest {
    bot_token: Option<String>,
    mention_only: Option<bool>,
}

impl ConfigureTelegramBotRequest {
    fn validated_token(&self) -> Option<String> {
        self.bot_token
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty() && !is_masked_secret(value))
            .map(str::to_string)
    }
}

/// Verify a Telegram bot token against getMe, persist the derived account,
/// and restart the bridge. Shared by the legacy `/api/telegram/configure`
/// route and the versioned `/api/v1/manage/im/account/telegram` route.
async fn apply_telegram_token(
    state: &SharedState,
    token: String,
    mention_only: bool,
) -> Result<crate::config::TelegramConfig, (StatusCode, Json<serde_json::Value>)> {
    let mut telegram_config = crate::config::TelegramConfig {
        enabled: true,
        account_id: String::new(),
        bot_token: token,
        display_name: String::new(),
        mention_only,
        allowed_chat_ids: Vec::new(),
        project_groups: Vec::new(),
    };
    let api = TelegramApi::new(TelegramSettings::from_app_config(&telegram_config));
    let user = match tokio::time::timeout(std::time::Duration::from_secs(5), api.get_me()).await {
        Ok(Ok(user)) => user,
        Ok(Err(err)) => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({ "ok": false, "error": err.to_string() })),
            ));
        }
        Err(_) => {
            return Err((
                StatusCode::REQUEST_TIMEOUT,
                Json(json!({ "ok": false, "error": "telegram getMe timeout" })),
            ));
        }
    };
    telegram_config.account_id = format!("tg_{}", user.id);
    telegram_config.display_name = telegram_user_display_name(&user).unwrap_or_else(|| {
        user.username
            .as_deref()
            .map(|value| format!("@{}", value.trim_start_matches('@')))
            .unwrap_or_else(|| format!("Telegram {}", user.id))
    });
    persist_telegram_account(state, &mut telegram_config).await?;
    start_bridge_task(
        state,
        BridgeStartMode::Restart,
        "bridge restarted after Telegram configuration",
    )
    .await;
    Ok(telegram_config)
}

async fn persist_telegram_account(
    state: &SharedState,
    telegram_config: &mut crate::config::TelegramConfig,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    {
        let mut config = state.config.lock().await;
        let previous_config = config.clone();
        config.migrate_legacy_im_accounts();
        if let Some(existing) = config
            .telegram_accounts
            .iter()
            .find(|account| account.account_id.trim() == telegram_config.account_id.trim())
        {
            telegram_config.allowed_chat_ids = existing.allowed_chat_ids.clone();
            telegram_config.project_groups = existing.project_groups.clone();
        }
        let token = telegram_config.bot_token.trim().to_string();
        config.telegram_accounts.retain(|account| {
            account.account_id.trim() == telegram_config.account_id
                || account.bot_token.trim() != token
        });
        config.upsert_telegram_account(telegram_config.clone());
        if !config.telegram.is_configured()
            || config.telegram.account_id.trim() == telegram_config.account_id
        {
            config.telegram = telegram_config.clone();
        }
        config.bridge.enabled = true;
        if let Err(err) = config.save(&state.config_path) {
            *config = previous_config;
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "ok": false, "error": err.to_string() })),
            ));
        }
    }
    Ok(())
}

pub(super) async fn configure_telegram_bot(
    State(state): State<SharedState>,
    Json(request): Json<ConfigureTelegramBotRequest>,
) -> impl IntoResponse {
    let Some(token) = request.validated_token() else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": "missing botToken" })),
        );
    };
    match apply_telegram_token(&state, token, request.mention_only.unwrap_or(false)).await {
        Ok(account) => (
            StatusCode::OK,
            Json(json!({ "ok": true, "configured": true, "accountId": account.account_id })),
        ),
        Err(response) => response,
    }
}

/// Versioned management variant of the Telegram token onboarding. The token
/// is write-only: the response only confirms the derived account identity and
/// never echoes the credential.
pub(super) async fn manage_configure_telegram_account(
    State(state): State<SharedState>,
    Json(request): Json<ConfigureTelegramBotRequest>,
) -> impl IntoResponse {
    let Some(token) = request.validated_token() else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": "missing botToken" })),
        );
    };
    match apply_telegram_token(&state, token, request.mention_only.unwrap_or(false)).await {
        Ok(account) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "platform": "telegram",
                "accountId": account.account_id,
                "displayName": account.display_name,
            })),
        ),
        Err(response) => response,
    }
}

pub(super) fn is_masked_secret(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty() && trimmed.chars().all(|ch| ch == '*')
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WechatBotStatus {
    configured: bool,
    enabled: bool,
    display_name: Option<String>,
    account_id: Option<String>,
    base_url: Option<String>,
    user_id: Option<String>,
    allowed_user_ids: usize,
    polling: bool,
    connected: bool,
    last_error: Option<String>,
    last_event_at_ms: Option<u128>,
    last_inbound_at_ms: Option<u128>,
}

pub(super) async fn wechat_bot_status(State(state): State<SharedState>) -> Json<WechatBotStatus> {
    let config = state.config.lock().await.clone();
    let wechat = state.wechat.lock().await.clone();
    let account = config.effective_wechat_accounts().into_iter().next();
    Json(WechatBotStatus {
        configured: account
            .as_ref()
            .is_some_and(|account| account.is_configured()),
        enabled: account.as_ref().is_some_and(|account| account.enabled),
        display_name: account
            .as_ref()
            .and_then(|account| non_empty_string(&account.display_name))
            .or_else(|| account.is_some().then(|| "微信机器人".to_string())),
        account_id: account
            .as_ref()
            .and_then(|account| non_empty_string(&account.account_id)),
        base_url: account
            .as_ref()
            .and_then(|account| non_empty_string(&account.base_url)),
        user_id: account
            .as_ref()
            .and_then(|account| non_empty_string(&account.user_id)),
        allowed_user_ids: account
            .as_ref()
            .map(|account| account.allowed_user_ids.len())
            .unwrap_or_default(),
        polling: wechat.polling,
        connected: wechat.connected,
        last_error: wechat.last_error,
        last_event_at_ms: wechat.last_event_at_ms,
        last_inbound_at_ms: wechat.last_inbound_at_ms,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WecomBotStatus {
    configured: bool,
    enabled: bool,
    display_name: Option<String>,
    account_id: Option<String>,
    bot_id: Option<String>,
    connecting: bool,
    connected: bool,
    last_error: Option<String>,
    last_event_at_ms: Option<u128>,
    last_inbound_at_ms: Option<u128>,
}

pub(super) async fn wecom_bot_status(State(state): State<SharedState>) -> Json<WecomBotStatus> {
    let config = state.config.lock().await.clone();
    let account = config.effective_wecom_accounts().into_iter().next();
    let runtime = account.as_ref().and_then(|account| {
        let key = im_account_key(ImPlatformKind::Wecom, &account.account_id);
        state
            .im_accounts
            .try_lock()
            .ok()
            .and_then(|items| items.get(&key).cloned())
    });
    Json(WecomBotStatus {
        configured: account
            .as_ref()
            .is_some_and(|account| account.is_configured()),
        enabled: account.as_ref().is_some_and(|account| account.enabled),
        display_name: account
            .as_ref()
            .and_then(|account| non_empty_string(&account.display_name)),
        account_id: account
            .as_ref()
            .and_then(|account| non_empty_string(&account.account_id)),
        bot_id: account
            .as_ref()
            .and_then(|account| non_empty_string(&account.bot_id)),
        connecting: runtime.as_ref().is_some_and(|runtime| runtime.connecting),
        connected: runtime.as_ref().is_some_and(|runtime| runtime.connected),
        last_error: runtime
            .as_ref()
            .and_then(|runtime| runtime.last_error.clone()),
        last_event_at_ms: runtime
            .as_ref()
            .and_then(|runtime| runtime.last_event_at_ms),
        last_inbound_at_ms: runtime.and_then(|runtime| runtime.last_inbound_at_ms),
    })
}

fn non_empty_string(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::{app_state::AppState, im_runtime::RouteTarget, store::PersistedState};

    fn telegram_account(
        account_id: &str,
        token: &str,
        enabled: bool,
        allowed_chat_ids: &[&str],
    ) -> crate::config::TelegramConfig {
        crate::config::TelegramConfig {
            enabled,
            account_id: account_id.to_string(),
            bot_token: token.to_string(),
            display_name: format!("Telegram {account_id}"),
            mention_only: false,
            allowed_chat_ids: allowed_chat_ids
                .iter()
                .map(|chat_id| (*chat_id).to_string())
                .collect(),
            project_groups: Vec::new(),
        }
    }

    fn state_with_config_path(config_path: std::path::PathBuf, config: AppConfig) -> SharedState {
        AppState::new(config_path, config, None, None)
    }

    #[test]
    fn telegram_topic_name_matches_title_and_respects_telegram_limit() {
        assert_eq!(
            truncate_telegram_topic_name("  修复启动流程  "),
            "修复启动流程"
        );
        assert_eq!(truncate_telegram_topic_name(""), "未命名会话");
        assert_eq!(
            truncate_telegram_topic_name(&"x".repeat(80))
                .chars()
                .count(),
            64
        );
    }

    #[test]
    fn telegram_topic_sync_filters_to_project_root_and_children() {
        let thread = |cwd: &str| json!({ "cwd": cwd });
        assert!(thread_belongs_to_project(
            &thread("/Users/miaopasi/codexhub"),
            "/Users/miaopasi/codexhub"
        ));
        assert!(thread_belongs_to_project(
            &thread("/Users/miaopasi/codexhub/macos"),
            "/Users/miaopasi/codexhub"
        ));
        assert!(!thread_belongs_to_project(
            &thread("/Users/miaopasi/codexhub-old"),
            "/Users/miaopasi/codexhub"
        ));
        assert!(!thread_belongs_to_project(
            &thread("/Users/miaopasi/da-2/CellularBridge"),
            "/Users/miaopasi/codexhub"
        ));
        assert!(!thread_belongs_to_project(
            &json!({}),
            "/Users/miaopasi/codexhub"
        ));
    }

    #[test]
    fn telegram_topic_sync_sorts_by_actual_update_time() {
        let mut threads = vec![
            json!({"id": "new", "updatedAt": 300}),
            json!({"id": "old", "updated_at": 100}),
            json!({"id": "middle", "updatedAt": 200}),
        ];
        threads.sort_by(|left, right| {
            thread_updated_at(left)
                .cmp(&thread_updated_at(right))
                .then_with(|| thread_id_for_sort(left).cmp(&thread_id_for_sort(right)))
        });
        assert_eq!(
            threads.iter().map(thread_id_for_sort).collect::<Vec<_>>(),
            vec!["old", "middle", "new"]
        );
    }

    #[test]
    fn telegram_topic_sync_update_time_accepts_numeric_strings_and_missing_values() {
        assert_eq!(thread_updated_at(&json!({"updatedAt": "42"})), 42);
        assert_eq!(thread_updated_at(&json!({"id": "legacy"})), 0);
    }

    #[test]
    fn legacy_im_account_match_is_shared_by_toggle_and_clear() {
        let mut config = AppConfig::default();
        config.feishu.app_id = "app_42".to_string();
        config.feishu.enabled = true;

        assert!(legacy_im_account_matches(&config, "feishu", "app_42"));
        set_legacy_im_account_enabled(&mut config, "feishu", "app_42", false);
        assert!(!config.feishu.enabled);

        config.feishu.enabled = true;
        clear_legacy_im_account(&mut config, "feishu", "app_42");
        assert!(!config.feishu.is_configured());

        config.feishu.app_id = "app_42".to_string();
        config.feishu.enabled = true;
        set_legacy_im_account_enabled(&mut config, "feishu", "other", false);
        assert!(config.feishu.enabled);
    }

    #[tokio::test]
    async fn reconfiguring_telegram_account_preserves_allowed_chat_ids() {
        let temp_dir = tempdir().expect("temp dir");
        let config_path = temp_dir.path().join("config.toml");
        let existing = telegram_account("tg_42", "old-token", true, &["100", "200"]);
        let mut config = AppConfig::default();
        config.state_path = temp_dir.path().join("state.json");
        config.telegram = existing.clone();
        config.telegram_accounts = vec![existing];
        config.bridge.enabled = true;
        let state = state_with_config_path(config_path.clone(), config);
        let mut replacement = telegram_account("tg_42", "new-token", true, &[]);
        replacement.display_name = "Updated bot".to_string();
        replacement.mention_only = true;

        persist_telegram_account(&state, &mut replacement)
            .await
            .expect("persist Telegram account");

        assert_eq!(replacement.allowed_chat_ids, ["100", "200"]);
        let in_memory = state.config.lock().await.clone();
        assert_eq!(in_memory.telegram.allowed_chat_ids, ["100", "200"]);
        assert_eq!(in_memory.telegram.bot_token, "new-token");
        assert_eq!(in_memory.telegram_accounts.len(), 1);
        assert_eq!(
            in_memory.telegram_accounts[0].allowed_chat_ids,
            ["100", "200"]
        );
        assert_eq!(in_memory.telegram_accounts[0].bot_token, "new-token");
        let persisted = AppConfig::load_or_default(&config_path).expect("load persisted config");
        assert_eq!(
            persisted.telegram_accounts[0].allowed_chat_ids,
            ["100", "200"]
        );
    }

    #[tokio::test]
    async fn telegram_configure_rolls_back_in_memory_config_when_save_fails() {
        let temp_dir = tempdir().expect("temp dir");
        let unwritable_config_path = temp_dir.path().join("config-directory");
        std::fs::create_dir(&unwritable_config_path).expect("create config directory");
        let existing = telegram_account("tg_42", "old-token", false, &["100"]);
        let mut config = AppConfig::default();
        config.state_path = temp_dir.path().join("state.json");
        config.telegram = existing.clone();
        config.telegram_accounts = vec![existing];
        config.bridge.enabled = false;
        let state = state_with_config_path(unwritable_config_path, config);
        let mut replacement = telegram_account("tg_42", "new-token", true, &[]);

        let result = persist_telegram_account(&state, &mut replacement).await;

        assert!(result.is_err());
        let config = state.config.lock().await;
        assert!(!config.bridge.enabled);
        assert!(!config.telegram.enabled);
        assert_eq!(config.telegram.bot_token, "old-token");
        assert_eq!(config.telegram.allowed_chat_ids, ["100"]);
        assert_eq!(config.telegram_accounts.len(), 1);
        assert!(!config.telegram_accounts[0].enabled);
        assert_eq!(config.telegram_accounts[0].bot_token, "old-token");
        assert_eq!(config.telegram_accounts[0].allowed_chat_ids, ["100"]);
    }

    #[tokio::test]
    async fn toggling_im_account_rolls_back_in_memory_config_when_save_fails() {
        let temp_dir = tempdir().expect("temp dir");
        let unwritable_config_path = temp_dir.path().join("config-directory");
        std::fs::create_dir(&unwritable_config_path).expect("create config directory");
        let existing = telegram_account("tg_42", "token", true, &["100"]);
        let mut config = AppConfig::default();
        config.state_path = temp_dir.path().join("state.json");
        config.telegram = existing.clone();
        config.telegram_accounts = vec![existing];
        config.bridge.enabled = true;
        let state = state_with_config_path(unwritable_config_path, config);

        let response = set_im_account_enabled(
            State(state.clone()),
            Json(SetImAccountEnabledRequest {
                platform: "telegram".to_string(),
                account_id: "tg_42".to_string(),
                enabled: false,
            }),
        )
        .await
        .into_response();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let config = state.config.lock().await;
        assert!(config.bridge.enabled);
        assert!(config.telegram.enabled);
        assert!(config.telegram_accounts[0].enabled);
    }

    #[tokio::test]
    async fn deleting_im_account_rolls_back_in_memory_config_when_save_fails() {
        let temp_dir = tempdir().expect("temp dir");
        let unwritable_config_path = temp_dir.path().join("config-directory");
        std::fs::create_dir(&unwritable_config_path).expect("create config directory");
        let existing = telegram_account("tg_42", "token", true, &["100"]);
        let mut config = AppConfig::default();
        config.state_path = temp_dir.path().join("state.json");
        config.telegram = existing.clone();
        config.telegram_accounts = vec![existing];
        config.bridge.enabled = true;
        let state = state_with_config_path(unwritable_config_path, config);

        let response = delete_im_account(
            State(state.clone()),
            Json(DeleteImAccountRequest {
                platform: "telegram".to_string(),
                account_id: "tg_42".to_string(),
            }),
        )
        .await
        .into_response();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let config = state.config.lock().await;
        assert!(config.bridge.enabled);
        assert!(config.telegram.is_configured());
        assert_eq!(config.telegram.account_id, "tg_42");
        assert_eq!(config.telegram_accounts.len(), 1);
        assert_eq!(config.telegram_accounts[0].account_id, "tg_42");
    }

    #[tokio::test]
    async fn deleting_telegram_account_clears_only_its_saved_bindings() {
        let temp_dir = tempdir().expect("temp dir");
        let mut config = AppConfig::default();
        config.state_path = temp_dir.path().join("state.json");
        let state = AppState::new(
            temp_dir.path().join("config.toml"),
            config.clone(),
            None,
            None,
        );
        let first_route = RouteTarget {
            platform: ImPlatformKind::Telegram,
            conversation_key: "telegram:first:41".to_string(),
            account_id: "first".to_string(),
            chat_id: "41".to_string(),
            remote_client_key: "im:telegram:first".to_string(),
        };
        let second_route = RouteTarget {
            platform: ImPlatformKind::Telegram,
            conversation_key: "telegram:second:42".to_string(),
            account_id: "second".to_string(),
            chat_id: "42".to_string(),
            remote_client_key: "im:telegram:second".to_string(),
        };
        {
            let mut runtime = state.runtime.lock().await;
            runtime.bind_route("thread-41", first_route);
            runtime.bind_route("thread-42", second_route);
        }
        {
            let mut persisted = state.persisted.lock().await;
            persisted.im_thread_bindings = HashMap::from([
                ("telegram:first:41".to_string(), "thread-41".to_string()),
                ("telegram:second:42".to_string(), "thread-42".to_string()),
            ]);
            persisted.save(&config.state_path).expect("save bindings");
        }

        clear_im_account_bindings(&state, "telegram", "first").await;

        assert_eq!(
            state
                .runtime
                .lock()
                .await
                .route_by_thread
                .keys()
                .cloned()
                .collect::<Vec<_>>(),
            vec!["thread-42".to_string()]
        );
        let expected = HashMap::from([("telegram:second:42".to_string(), "thread-42".to_string())]);
        assert_eq!(state.persisted.lock().await.im_thread_bindings, expected);
        assert_eq!(
            PersistedState::load(&config.state_path).im_thread_bindings,
            expected
        );
    }
}
