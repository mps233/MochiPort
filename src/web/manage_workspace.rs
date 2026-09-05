use std::collections::{BTreeMap, HashSet};

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    ai_gateway::{
        catalog,
        config::{ProviderConfig, ProviderType, Sub2ApiAdminConfig},
        model_fetch, provider_usage, request_log, sub2api_accounts,
        templates::{self, ProviderTemplate},
    },
    app_state::SharedState,
    config::{LocalConnectionMode, OutboundProxyConfig, OutboundProxyMode},
};

use super::masked_url;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ManageGatewayResponse {
    enabled: bool,
    filter_image_generation_tool: bool,
    request_logging_enabled: bool,
    request_log_details_enabled: bool,
    codex_visible_models: Vec<String>,
    providers: Vec<ManageProviderResponse>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ManageProviderResponse {
    name: String,
    enabled: bool,
    provider_type: ProviderType,
    compatibility: Option<String>,
    base_url: String,
    models_url: Option<String>,
    models: Vec<String>,
    model_aliases: BTreeMap<String, String>,
    prompt_cache_retention: Option<String>,
    weight: u32,
    timeout_secs: u64,
    secret_set: bool,
}

impl From<&ProviderConfig> for ManageProviderResponse {
    fn from(provider: &ProviderConfig) -> Self {
        Self {
            name: provider.name.clone(),
            enabled: provider.enabled,
            provider_type: provider.provider_type.clone(),
            compatibility: provider.compatibility.clone(),
            base_url: masked_url(&provider.base_url),
            models_url: provider.models_url.as_deref().map(masked_url),
            models: provider.models.clone(),
            model_aliases: provider.model_aliases.clone(),
            prompt_cache_retention: provider.prompt_cache_retention.clone(),
            weight: provider.weight,
            timeout_secs: provider.timeout_secs,
            secret_set: !provider.api_key.trim().is_empty(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UpdateGatewayRequest {
    enabled: bool,
    filter_image_generation_tool: bool,
    request_logging_enabled: bool,
    request_log_details_enabled: bool,
    #[serde(default)]
    codex_visible_models: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UpsertProviderRequest {
    original_name: Option<String>,
    name: String,
    enabled: bool,
    provider_type: ProviderType,
    compatibility: Option<String>,
    base_url: String,
    models_url: Option<String>,
    #[serde(default)]
    models: Vec<String>,
    #[serde(default)]
    model_aliases: BTreeMap<String, String>,
    prompt_cache_retention: Option<String>,
    weight: u32,
    timeout_secs: u64,
    api_key: Option<String>,
    #[serde(default)]
    clear_api_key: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DeleteProviderRequest {
    name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ManageSettingsResponse {
    language: Option<String>,
    theme: Option<String>,
    local_connection_mode: LocalConnectionMode,
    bind: String,
    outbound_proxy: ManageOutboundProxyResponse,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ManageOutboundProxyResponse {
    mode: OutboundProxyMode,
    url: String,
    credential_set: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UpdateSettingsRequest {
    language: Option<String>,
    theme: Option<String>,
    local_connection_mode: LocalConnectionMode,
    outbound_proxy_mode: OutboundProxyMode,
    outbound_proxy_url: Option<String>,
}

// Keep the pre-pagination behavior for older clients that omit `limit`.
const REQUEST_LOG_DEFAULT_PAGE_SIZE: usize = 200;
const REQUEST_LOG_MAX_PAGE_SIZE: usize = 500;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RequestLogsQuery {
    limit: Option<usize>,
    cursor: Option<String>,
    query: Option<String>,
    status: Option<String>,
    channel: Option<String>,
    model_id: Option<String>,
    sort: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RequestLogCursorPayload {
    created_at_ms: i64,
    id: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ClearOldRequestLogsRequest {
    days: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct FetchProviderModelsRequest {
    provider_name: Option<String>,
    base_url: String,
    models_url: Option<String>,
    provider_type: ProviderType,
    api_key: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct FetchProviderUsageRequest {
    provider_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UpdateSub2ApiAdminRequest {
    base_url: String,
    admin_api_key: Option<String>,
    #[serde(default)]
    clear_admin_api_key: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct FetchSub2ApiAccountsRequest {
    #[serde(default)]
    force_billing_refresh: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SetSub2ApiAccountSchedulableRequest {
    schedulable: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ManageSub2ApiAdminResponse {
    configured: bool,
    base_url: String,
    secret_set: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderTemplatesResponse {
    templates: Vec<ProviderTemplateResponse>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderTemplateResponse {
    id: &'static str,
    display_name: &'static str,
    provider_type: ProviderType,
    #[serde(skip_serializing_if = "Option::is_none")]
    compatibility: Option<&'static str>,
    base_url: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    models_url: Option<&'static str>,
    models: &'static [&'static str],
}

impl From<&'static ProviderTemplate> for ProviderTemplateResponse {
    fn from(template: &'static ProviderTemplate) -> Self {
        Self {
            id: template.id,
            display_name: template.display_name,
            provider_type: template.provider_type.clone(),
            compatibility: template.compatibility,
            base_url: template.base_url,
            models_url: template.models_url,
            models: template.models,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CodexModelCatalogResponse {
    models: Vec<CodexCatalogModelResponse>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CodexCatalogModelResponse {
    id: String,
    display_name: String,
}

/// 内置服务商模板：纯静态数据，与用户已配置的 provider 完全无关。
pub(super) async fn provider_templates() -> impl IntoResponse {
    Json(ProviderTemplatesResponse {
        templates: templates::provider_templates()
            .iter()
            .map(ProviderTemplateResponse::from)
            .collect(),
    })
}

/// 内置 Codex 模型目录中对 API 可见（可列出）的模型。
pub(super) async fn codex_models_catalog() -> impl IntoResponse {
    Json(CodexModelCatalogResponse {
        models: catalog::visible_catalog_model_options()
            .into_iter()
            .map(|option| CodexCatalogModelResponse {
                id: option.slug,
                display_name: option.display_name,
            })
            .collect(),
    })
}

pub(super) async fn gateway(State(state): State<SharedState>) -> impl IntoResponse {
    let config = state.config.lock().await;
    Json(gateway_snapshot(&config.ai_gateway))
}

pub(super) async fn update_gateway(
    State(state): State<SharedState>,
    Json(request): Json<UpdateGatewayRequest>,
) -> impl IntoResponse {
    let mut config = state.config.lock().await;
    let mut next = config.clone();
    next.ai_gateway.enabled = request.enabled;
    next.ai_gateway.filter_image_generation_tool = request.filter_image_generation_tool;
    next.ai_gateway.request_logging_enabled = request.request_logging_enabled;
    next.ai_gateway.request_log_details_enabled = request.request_log_details_enabled;
    next.ai_gateway.codex_visible_models = normalized_values(request.codex_visible_models);

    if let Err(error) = next.save(&state.config_path) {
        return operation_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    *config = next;
    (
        StatusCode::OK,
        Json(json!({ "ok": true, "gateway": gateway_snapshot(&config.ai_gateway) })),
    )
}

pub(super) async fn upsert_provider(
    State(state): State<SharedState>,
    Json(request): Json<UpsertProviderRequest>,
) -> impl IntoResponse {
    let name = request.name.trim();
    if name.is_empty() || name.len() > 64 {
        return operation_error(StatusCode::BAD_REQUEST, "provider name is required");
    }
    if request.weight == 0 || request.timeout_secs == 0 {
        return operation_error(
            StatusCode::BAD_REQUEST,
            "weight and timeoutSecs must be positive",
        );
    }

    let original_name = non_empty(request.original_name.as_deref()).unwrap_or(name);
    let mut config = state.config.lock().await;
    let mut next = config.clone();
    let existing_index = next
        .ai_gateway
        .providers
        .iter()
        .position(|provider| provider.name == original_name);
    if next
        .ai_gateway
        .providers
        .iter()
        .enumerate()
        .any(|(index, provider)| provider.name == name && Some(index) != existing_index)
    {
        return operation_error(StatusCode::CONFLICT, "provider name already exists");
    }

    let existing_provider = existing_index.map(|index| next.ai_gateway.providers[index].clone());
    let submitted_base_url = normalized_remote_url_text(&request.base_url);
    let base_url = match existing_provider.as_ref() {
        Some(existing) if displayed_remote_url_matches(&existing.base_url, &submitted_base_url) => {
            existing.base_url.clone()
        }
        _ => {
            if let Err(error) = validate_remote_url(&submitted_base_url, "baseUrl") {
                return operation_error(StatusCode::BAD_REQUEST, error);
            }
            submitted_base_url
        }
    };
    let submitted_models_url =
        non_empty_owned(request.models_url).map(|value| normalized_remote_url_text(&value));
    let models_url = match submitted_models_url {
        Some(value)
            if existing_provider
                .as_ref()
                .and_then(|provider| provider.models_url.as_deref())
                .is_some_and(|stored| displayed_remote_url_matches(stored, &value)) =>
        {
            existing_provider
                .as_ref()
                .and_then(|provider| provider.models_url.clone())
        }
        Some(value) => {
            if let Err(error) = validate_remote_url(&value, "modelsUrl") {
                return operation_error(StatusCode::BAD_REQUEST, error);
            }
            Some(value)
        }
        None => None,
    };

    let mut provider = existing_index
        .map(|index| next.ai_gateway.providers[index].clone())
        .unwrap_or_default();
    provider.name = name.to_string();
    provider.enabled = request.enabled;
    provider.provider_type = request.provider_type;
    provider.compatibility = non_empty_owned(request.compatibility);
    provider.base_url = base_url;
    provider.models_url = models_url;
    provider.models = normalized_values(request.models);
    provider.model_aliases = request
        .model_aliases
        .into_iter()
        .filter_map(|(key, value)| {
            let key = key.trim().to_string();
            let value = value.trim().to_string();
            (!key.is_empty() && !value.is_empty()).then_some((key, value))
        })
        .collect();
    provider.prompt_cache_retention = non_empty_owned(request.prompt_cache_retention);
    provider.weight = request.weight;
    provider.timeout_secs = request.timeout_secs;
    if request.clear_api_key {
        provider.api_key.clear();
    } else if let Some(api_key) = non_empty_owned(request.api_key) {
        provider.api_key = api_key;
    }

    if let Some(index) = existing_index {
        next.ai_gateway.providers[index] = provider;
    } else {
        next.ai_gateway.providers.push(provider);
    }
    if let Err(error) = next.save(&state.config_path) {
        return operation_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    *config = next;
    (
        StatusCode::OK,
        Json(json!({ "ok": true, "gateway": gateway_snapshot(&config.ai_gateway) })),
    )
}

pub(super) async fn delete_provider(
    State(state): State<SharedState>,
    Json(request): Json<DeleteProviderRequest>,
) -> impl IntoResponse {
    let name = request.name.trim();
    if name.is_empty() {
        return operation_error(StatusCode::BAD_REQUEST, "provider name is required");
    }
    let mut config = state.config.lock().await;
    let mut next = config.clone();
    let before = next.ai_gateway.providers.len();
    next.ai_gateway
        .providers
        .retain(|provider| provider.name != name);
    if next.ai_gateway.providers.len() == before {
        return operation_error(StatusCode::NOT_FOUND, "provider not found");
    }
    if let Err(error) = next.save(&state.config_path) {
        return operation_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    *config = next;
    (
        StatusCode::OK,
        Json(json!({ "ok": true, "gateway": gateway_snapshot(&config.ai_gateway) })),
    )
}

pub(super) async fn settings(State(state): State<SharedState>) -> impl IntoResponse {
    let config = state.config.lock().await;
    Json(settings_snapshot(&config))
}

pub(super) async fn update_settings(
    State(state): State<SharedState>,
    Json(request): Json<UpdateSettingsRequest>,
) -> impl IntoResponse {
    let language = normalized_language(request.language);
    let theme = normalized_theme(request.theme);
    if language.is_err() || theme.is_err() {
        return operation_error(StatusCode::BAD_REQUEST, "unsupported language or theme");
    }

    let mut config = state.config.lock().await;
    let mut next = config.clone();
    next.language = language.unwrap();
    next.theme = theme.unwrap();
    next.local_connection_mode = request.local_connection_mode;
    next.outbound_proxy.mode = request.outbound_proxy_mode;
    if let Some(url) = request.outbound_proxy_url {
        next.outbound_proxy.url = url.trim().to_string();
    }
    let (outbound_client, sensitive_client) =
        match crate::outbound_http::build_clients(&next.outbound_proxy, next.local_listen_port()) {
            Ok(clients) => clients,
            Err(error) => return operation_error(StatusCode::BAD_REQUEST, error.to_string()),
        };
    if let Err(error) = next.save(&state.config_path) {
        return operation_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    crate::outbound_http::install_with_sensitive(
        outbound_client,
        sensitive_client,
        &next.outbound_proxy,
    );
    *config = next;
    (
        StatusCode::OK,
        Json(json!({ "ok": true, "settings": settings_snapshot(&config) })),
    )
}

pub(super) async fn request_logs(
    State(state): State<SharedState>,
    Query(query): Query<RequestLogsQuery>,
) -> impl IntoResponse {
    let sort = match request_log_sort(query.sort.as_deref()) {
        Ok(sort) => sort,
        Err(message) => return operation_error(StatusCode::BAD_REQUEST, message),
    };
    let cursor = match query.cursor.as_deref().map(decode_request_log_cursor) {
        Some(Ok(cursor)) => Some(cursor),
        Some(Err(message)) => return operation_error(StatusCode::BAD_REQUEST, message),
        None => None,
    };
    let request = request_log::RequestLogQuery {
        limit: query
            .limit
            .unwrap_or(REQUEST_LOG_DEFAULT_PAGE_SIZE)
            .clamp(1, REQUEST_LOG_MAX_PAGE_SIZE),
        cursor,
        query: non_empty_owned(query.query),
        status: non_empty_owned(query.status),
        channel: non_empty_owned(query.channel),
        model_id: non_empty_owned(query.model_id),
        sort,
    };

    match state
        .ai_gateway_request_logs
        .list_page_blocking(request)
        .await
    {
        Ok(page) => {
            let next_cursor = match page.next_cursor.map(encode_request_log_cursor).transpose() {
                Ok(cursor) => cursor,
                Err(()) => {
                    return operation_error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "failed to encode request log cursor",
                    );
                }
            };
            let mut value = serde_json::to_value(page.logs).unwrap_or(Value::Null);
            request_log::redact_value(&mut value);
            (
                StatusCode::OK,
                Json(json!({
                    "logs": value,
                    "nextCursor": next_cursor,
                    "hasMore": page.has_more,
                })),
            )
        }
        Err(error) => operation_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

fn request_log_sort(value: Option<&str>) -> Result<request_log::RequestLogSort, &'static str> {
    match value.unwrap_or("newest") {
        "newest" => Ok(request_log::RequestLogSort::Newest),
        "oldest" => Ok(request_log::RequestLogSort::Oldest),
        _ => Err("unsupported request log sort"),
    }
}

fn decode_request_log_cursor(value: &str) -> Result<request_log::RequestLogCursor, &'static str> {
    if value.is_empty() || value.len() > 256 {
        return Err("invalid request log cursor");
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| "invalid request log cursor")?;
    let payload: RequestLogCursorPayload =
        serde_json::from_slice(&bytes).map_err(|_| "invalid request log cursor")?;
    if payload.id <= 0 {
        return Err("invalid request log cursor");
    }
    Ok(request_log::RequestLogCursor {
        created_at_ms: payload.created_at_ms,
        id: payload.id,
    })
}

fn encode_request_log_cursor(cursor: request_log::RequestLogCursor) -> Result<String, ()> {
    let payload = RequestLogCursorPayload {
        created_at_ms: cursor.created_at_ms,
        id: cursor.id,
    };
    serde_json::to_vec(&payload)
        .map(|bytes| URL_SAFE_NO_PAD.encode(bytes))
        .map_err(|_| ())
}

pub(super) async fn request_log_detail(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match state.ai_gateway_request_logs.get_detail_blocking(id).await {
        Ok(Some(log)) => {
            let mut value = serde_json::to_value(log).unwrap_or(Value::Null);
            request_log::redact_value(&mut value);
            (StatusCode::OK, Json(json!({ "log": value })))
        }
        Ok(None) => operation_error(StatusCode::NOT_FOUND, "request log not found"),
        Err(error) => operation_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

pub(super) async fn clear_request_logs(State(state): State<SharedState>) -> impl IntoResponse {
    let store = state.ai_gateway_request_logs.clone();
    match tokio::task::spawn_blocking(move || store.delete_all()).await {
        Ok(Ok(deleted)) => (
            StatusCode::OK,
            Json(json!({ "ok": true, "deleted": deleted })),
        ),
        Ok(Err(error)) => operation_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
        Err(error) => operation_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("request log cleanup task failed: {error}"),
        ),
    }
}

/// 清理早于 `days` 天的请求日志。兼容端点
/// `DELETE /ai-gateway/request-logs/old?days=3` 在版本化管理 API 中的对等物；
/// `days` 缺省 3，clamp 到 1..=365。
pub(super) async fn clear_old_request_logs(
    State(state): State<SharedState>,
    request: Option<Json<ClearOldRequestLogsRequest>>,
) -> impl IntoResponse {
    let days = request
        .and_then(|Json(request)| request.days)
        .unwrap_or(3)
        .clamp(1, 365);
    let cutoff_ms = request_log::now_ms().saturating_sub(days as i64 * 86_400_000);
    let store = state.ai_gateway_request_logs.clone();
    match tokio::task::spawn_blocking(move || store.delete_older_than(cutoff_ms)).await {
        Ok(Ok(deleted)) => (
            StatusCode::OK,
            Json(json!({ "ok": true, "deleted": deleted })),
        ),
        Ok(Err(error)) => operation_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
        Err(error) => operation_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("request log cleanup task failed: {error}"),
        ),
    }
}

/// 代理端从上游拉取模型列表，复用 `crate::ai_gateway::model_fetch` 的纯逻辑。
///
/// API key 选用顺序：请求里的 `apiKey` → `providerName` 命中现有 provider 时
/// 的已存 key → 无鉴权头。响应对每个候选 URL 记录尝试详情，不回显鉴权信息。
pub(super) async fn fetch_provider_models(
    State(state): State<SharedState>,
    Json(request): Json<FetchProviderModelsRequest>,
) -> impl IntoResponse {
    let base_url = request.base_url.trim().to_string();
    if base_url.is_empty() {
        return operation_error(StatusCode::BAD_REQUEST, "baseUrl is required");
    }

    let provider_name = non_empty(request.provider_name.as_deref());
    let (stored_api_key, stored_compatibility) = {
        let config = state.config.lock().await;
        let existing = provider_name.and_then(|name| {
            config
                .ai_gateway
                .providers
                .iter()
                .find(|provider| provider.name == name)
        });
        (
            existing
                .map(|provider| provider.api_key.trim().to_string())
                .filter(|key| !key.is_empty()),
            existing.and_then(|provider| provider.compatibility.clone()),
        )
    };
    let api_key = non_empty(request.api_key.as_deref())
        .map(str::to_string)
        .or(stored_api_key)
        .unwrap_or_default();

    let fallback_models_url = model_fetch::known_models_url(
        provider_name,
        &request.provider_type,
        stored_compatibility.as_deref(),
        &base_url,
    );
    let candidates = model_fetch::model_list_candidates(
        &base_url,
        request.models_url.as_deref(),
        fallback_models_url.as_deref(),
    );

    let client = crate::outbound_http::get();
    let outcome = model_fetch::fetch_models(
        &client,
        &candidates,
        &api_key,
        model_fetch::MODEL_LIST_FETCH_TIMEOUT,
    )
    .await;

    let (ok, models) = match outcome.models {
        Some(models) => (
            true,
            model_fetch::filter_fetched_models_for_provider(&request.provider_type, models),
        ),
        None => (false, Vec::new()),
    };
    (
        StatusCode::OK,
        Json(json!({ "ok": ok, "models": models, "attempts": outcome.attempts })),
    )
}

/// 查询已保存 Provider 当前 API Key 的余额与计费倍率。
///
/// API Key 只从 daemon 配置读取，既不接受客户端传入，也不会进入响应。
/// Sub2API 的余额与倍率接口，以及 New API 的余额接口会并行探测；
/// 不同协议只使用各自的结果，不会混合额度单位或倍率。
pub(super) async fn fetch_provider_usage(
    State(state): State<SharedState>,
    Json(request): Json<FetchProviderUsageRequest>,
) -> impl IntoResponse {
    let requested_provider_name = request.provider_name.trim();
    if requested_provider_name.is_empty() {
        return operation_error(StatusCode::BAD_REQUEST, "providerName is required");
    }

    let provider = {
        let config = state.config.lock().await;
        config
            .ai_gateway
            .providers
            .iter()
            .find(|provider| provider.name == requested_provider_name)
            .map(|provider| {
                (
                    provider.name.clone(),
                    provider.base_url.clone(),
                    provider.api_key.clone(),
                )
            })
    };
    let Some((provider_name, base_url, api_key)) = provider else {
        return operation_error(StatusCode::NOT_FOUND, "provider not found");
    };
    if api_key.trim().is_empty() {
        return operation_error(
            StatusCode::BAD_REQUEST,
            "provider API key is not configured",
        );
    }
    if base_url.trim().is_empty() {
        return operation_error(
            StatusCode::BAD_REQUEST,
            "provider base URL is not configured",
        );
    }

    let snapshot =
        provider_usage::fetch_provider_usage(&crate::outbound_http::get(), &base_url, &api_key)
            .await;
    (
        StatusCode::OK,
        Json(json!({
            "ok": snapshot.any_available(),
            "providerName": provider_name,
            "usage": snapshot,
        })),
    )
}

/// 查询已保存 Provider API Key 最近一次命中的 Sub2API 上游账号。
///
/// Provider API Key 和 Sub2API 管理密钥都只在 daemon 内使用；响应只包含
/// Provider 名称和账号的非敏感标识信息。
pub(super) async fn fetch_provider_recent_account(
    State(state): State<SharedState>,
    Json(request): Json<FetchProviderUsageRequest>,
) -> impl IntoResponse {
    let requested_provider_name = request.provider_name.trim();
    if requested_provider_name.is_empty() {
        return operation_error(StatusCode::BAD_REQUEST, "providerName is required");
    }

    let provider_and_admin = {
        let config = state.config.lock().await;
        config
            .ai_gateway
            .providers
            .iter()
            .find(|provider| provider.name == requested_provider_name)
            .map(|provider| {
                (
                    provider.name.clone(),
                    provider.api_key.clone(),
                    config.ai_gateway.sub2api_admin.clone(),
                )
            })
    };
    let Some((provider_name, provider_api_key, admin)) = provider_and_admin else {
        return operation_error(StatusCode::NOT_FOUND, "provider not found");
    };
    if provider_api_key.trim().is_empty() {
        return operation_error(
            StatusCode::BAD_REQUEST,
            "provider API key is not configured",
        );
    }
    if !admin.is_configured() {
        return operation_error(StatusCode::BAD_REQUEST, "尚未连接 Sub2API 账号池");
    }

    match sub2api_accounts::fetch_recent_provider_account(
        &crate::outbound_http::get_sensitive(),
        &admin.base_url,
        &admin.admin_api_key,
        &provider_api_key,
    )
    .await
    {
        Ok(account) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "providerName": provider_name,
                "account": account,
            })),
        ),
        Err(error) => operation_error(StatusCode::BAD_GATEWAY, error.to_string()),
    }
}

pub(super) async fn sub2api_admin(State(state): State<SharedState>) -> impl IntoResponse {
    let config = state.config.lock().await;
    Json(sub2api_admin_snapshot(&config.ai_gateway.sub2api_admin))
}

pub(super) async fn update_sub2api_admin(
    State(state): State<SharedState>,
    Json(request): Json<UpdateSub2ApiAdminRequest>,
) -> impl IntoResponse {
    let base_url = normalized_remote_url_text(&request.base_url);
    if let Err(error) = validate_remote_url(&base_url, "baseUrl") {
        return operation_error(StatusCode::BAD_REQUEST, error);
    }
    let existing_key = {
        let config = state.config.lock().await;
        config.ai_gateway.sub2api_admin.admin_api_key.clone()
    };
    let admin_api_key = if request.clear_admin_api_key {
        String::new()
    } else {
        non_empty_owned(request.admin_api_key).unwrap_or(existing_key)
    };
    if admin_api_key.is_empty() {
        return operation_error(StatusCode::BAD_REQUEST, "Sub2API 管理密钥不能为空");
    }
    if let Err(error) = sub2api_accounts::validate_admin_connection(
        &crate::outbound_http::get_sensitive(),
        &base_url,
        &admin_api_key,
    )
    .await
    {
        return operation_error(StatusCode::BAD_GATEWAY, error.to_string());
    }

    let mut config = state.config.lock().await;
    let mut next = config.clone();
    next.ai_gateway.sub2api_admin = Sub2ApiAdminConfig {
        base_url,
        admin_api_key,
    };
    if let Err(error) = next.save(&state.config_path) {
        return operation_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    *config = next;
    (
        StatusCode::OK,
        Json(json!({
            "ok": true,
            "sub2api": sub2api_admin_snapshot(&config.ai_gateway.sub2api_admin),
        })),
    )
}

pub(super) async fn disconnect_sub2api_admin(
    State(state): State<SharedState>,
) -> impl IntoResponse {
    let mut config = state.config.lock().await;
    let mut next = config.clone();
    next.ai_gateway.sub2api_admin = Sub2ApiAdminConfig::default();
    if let Err(error) = next.save(&state.config_path) {
        return operation_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    *config = next;
    (
        StatusCode::OK,
        Json(json!({
            "ok": true,
            "sub2api": sub2api_admin_snapshot(&config.ai_gateway.sub2api_admin),
        })),
    )
}

pub(super) async fn fetch_sub2api_accounts(
    State(state): State<SharedState>,
    Json(request): Json<FetchSub2ApiAccountsRequest>,
) -> impl IntoResponse {
    let admin = {
        let config = state.config.lock().await;
        config.ai_gateway.sub2api_admin.clone()
    };
    if !admin.is_configured() {
        return operation_error(StatusCode::BAD_REQUEST, "尚未连接 Sub2API 账号池");
    }
    match sub2api_accounts::fetch_account_pool(
        &crate::outbound_http::get_sensitive(),
        &admin.base_url,
        &admin.admin_api_key,
        request.force_billing_refresh,
    )
    .await
    {
        Ok(snapshot) => (
            StatusCode::OK,
            Json(json!({ "ok": true, "pool": snapshot })),
        ),
        Err(error) => operation_error(StatusCode::BAD_GATEWAY, error.to_string()),
    }
}

pub(super) async fn set_sub2api_account_schedulable(
    State(state): State<SharedState>,
    Path(account_id): Path<i64>,
    Json(request): Json<SetSub2ApiAccountSchedulableRequest>,
) -> impl IntoResponse {
    if account_id <= 0 {
        return operation_error(
            StatusCode::BAD_REQUEST,
            "accountId must be a positive integer",
        );
    }

    let admin = {
        let config = state.config.lock().await;
        config.ai_gateway.sub2api_admin.clone()
    };
    if !admin.is_configured() {
        return operation_error(StatusCode::BAD_REQUEST, "尚未连接 Sub2API 账号池");
    }

    match sub2api_accounts::set_account_schedulable(
        &crate::outbound_http::get_sensitive(),
        &admin.base_url,
        &admin.admin_api_key,
        account_id,
        request.schedulable,
    )
    .await
    {
        Ok(()) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "accountId": account_id,
                "schedulable": request.schedulable,
            })),
        ),
        Err(error) => operation_error(StatusCode::BAD_GATEWAY, error.to_string()),
    }
}

fn gateway_snapshot(config: &crate::ai_gateway::config::AiGatewayConfig) -> ManageGatewayResponse {
    ManageGatewayResponse {
        enabled: config.enabled,
        filter_image_generation_tool: config.filter_image_generation_tool,
        request_logging_enabled: config.request_logging_enabled,
        request_log_details_enabled: config.request_log_details_enabled,
        codex_visible_models: config.codex_visible_models.clone(),
        providers: config
            .providers
            .iter()
            .map(ManageProviderResponse::from)
            .collect(),
    }
}

fn sub2api_admin_snapshot(config: &Sub2ApiAdminConfig) -> ManageSub2ApiAdminResponse {
    ManageSub2ApiAdminResponse {
        configured: config.is_configured(),
        base_url: masked_url(&config.base_url),
        secret_set: !config.admin_api_key.trim().is_empty(),
    }
}

fn settings_snapshot(config: &crate::config::AppConfig) -> ManageSettingsResponse {
    let proxy = &config.outbound_proxy;
    ManageSettingsResponse {
        language: normalized_language(config.language.clone())
            .ok()
            .flatten()
            .or_else(|| config.language.clone()),
        theme: normalized_theme(config.theme.clone())
            .ok()
            .flatten()
            .or_else(|| config.theme.clone()),
        local_connection_mode: config.local_connection_mode,
        bind: config.bind.clone(),
        outbound_proxy: ManageOutboundProxyResponse {
            mode: proxy.mode,
            url: crate::outbound_http::masked_proxy_url(proxy),
            credential_set: proxy_has_credentials(proxy),
        },
    }
}

fn normalized_values(values: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .filter(|value| seen.insert(value.clone()))
        .collect()
}

fn normalized_theme(value: Option<String>) -> Result<Option<String>, ()> {
    let Some(value) = non_empty_owned(value) else {
        return Ok(None);
    };
    match value.to_ascii_lowercase().as_str() {
        "system" | "auto" => Ok(Some("system".to_string())),
        "light" => Ok(Some("light".to_string())),
        "dark" => Ok(Some("dark".to_string())),
        _ => Err(()),
    }
}

fn normalized_language(value: Option<String>) -> Result<Option<String>, ()> {
    let Some(value) = non_empty_owned(value) else {
        return Ok(None);
    };
    match value.to_ascii_lowercase().replace('_', "-").as_str() {
        "zh" | "zh-cn" | "cn" => Ok(Some("zh-CN".to_string())),
        "en" | "en-us" => Ok(Some("en-US".to_string())),
        _ => Err(()),
    }
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn non_empty_owned(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn validate_remote_url(value: &str, field: &str) -> Result<(), String> {
    let parsed = url::Url::parse(value.trim()).map_err(|_| format!("invalid {field}"))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(format!("invalid {field}"));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(format!("{field} must not contain credentials"));
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(format!("{field} must not contain a query or fragment"));
    }
    Ok(())
}

fn normalized_remote_url_text(value: &str) -> String {
    value.trim().trim_end_matches('/').to_string()
}

fn displayed_remote_url_matches(stored: &str, submitted: &str) -> bool {
    normalized_remote_url_text(&masked_url(stored)) == normalized_remote_url_text(submitted)
}

fn proxy_has_credentials(config: &OutboundProxyConfig) -> bool {
    url::Url::parse(config.url.trim()).is_ok_and(|url| {
        !url.username().is_empty() || url.password().is_some_and(|value| !value.is_empty())
    })
}

fn operation_error(status: StatusCode, error: impl Into<String>) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "ok": false, "error": error.into() })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_log_redaction_covers_nested_json_and_sse_payloads() {
        let mut value = json!({
            "requestHeadersJson": "{\"authorization\":\"Bearer canary\",\"x-debug\":\"ok\"}",
            "requestJson": "{\"input\":\"hello\",\"apiKey\":\"canary-key\"}",
            "upstreamResponseSse": "event: message\ndata: {\"token\":\"canary-token\",\"text\":\"ok\"}\n",
        });

        request_log::redact_value(&mut value);
        let serialized = value.to_string();
        assert!(!serialized.contains("Bearer canary"));
        assert!(!serialized.contains("canary-key"));
        assert!(!serialized.contains("canary-token"));
        assert!(serialized.contains("<redacted>"));
        assert!(serialized.contains("hello"));
        assert!(serialized.contains("ok"));
    }

    #[test]
    fn provider_urls_never_return_embedded_credentials_or_query_tokens() {
        assert_eq!(
            masked_url("https://provider-user:provider-secret@provider.example/v1?api_key=canary"),
            "https://provider.example/v1"
        );
        assert!(
            validate_remote_url(
                "https://provider-user:provider-secret@provider.example/v1",
                "baseUrl"
            )
            .is_err()
        );
        assert!(
            validate_remote_url("https://provider.example/v1?api_key=canary", "baseUrl").is_err()
        );
        assert_eq!(masked_url("https://broken url?api_key=canary"), "<invalid>");
        assert!(displayed_remote_url_matches(
            "https://provider-user:provider-secret@provider.example/v1?api_key=canary",
            "https://provider.example/v1"
        ));
    }

    #[test]
    fn settings_language_aliases_are_saved_in_legacy_compatible_form() {
        assert_eq!(
            normalized_language(Some("en".to_string())),
            Ok(Some("en-US".to_string()))
        );
        assert_eq!(
            normalized_language(Some("zh_cn".to_string())),
            Ok(Some("zh-CN".to_string()))
        );
        assert!(normalized_language(Some("fr".to_string())).is_err());
        assert_eq!(
            normalized_theme(Some("auto".to_string())),
            Ok(Some("system".to_string()))
        );
    }
}
