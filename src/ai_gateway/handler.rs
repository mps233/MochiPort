use axum::{
    body::Bytes,
    extract::State,
    http::HeaderMap,
    response::IntoResponse,
};
use tracing::info;

use crate::app_state::SharedState;

use super::config::ProviderType;
use super::context::GatewayContext;
use super::error::GatewayError;
use super::model::GatewayRequest;
use super::providers::{deepseek_chat, openai_responses};
use super::router::resolve_provider;

/// POST /ai-gateway/v1/responses
pub async fn handle_responses(
    State(state): State<SharedState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    // 1. 检查 gateway 是否启用
    let config = state.config.lock().await;
    if !config.ai_gateway.enabled {
        return GatewayError::gateway_disabled().into_response();
    }
    let gw_config = config.ai_gateway.clone();
    drop(config);

    // 2. 解析请求 body
    let raw_body: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return GatewayError::bad_request(format!("invalid JSON: {e}")).into_response();
        }
    };
    let request: GatewayRequest = match serde_json::from_value(raw_body.clone()) {
        Ok(r) => r,
        Err(e) => {
            return GatewayError::bad_request(format!("invalid request: {e}")).into_response();
        }
    };

    // 3. 提取上下文
    let body_cache_key = request.prompt_cache_key.as_deref();
    let ctx = GatewayContext::extract(&headers, body_cache_key);

    // 4. 路由到 provider
    let provider = match resolve_provider(&request.model, &gw_config) {
        Ok(p) => p,
        Err(e) => return e.into_response(),
    };

    info!(
        model = %request.model,
        provider = %provider.name,
        provider_type = ?provider.provider_type,
        session_id = ?ctx.session_id,
        prompt_cache_key = %ctx.prompt_cache_key,
        stream = request.stream,
        "ai-gateway request routed"
    );

    // 5. 按 provider_type 分发
    match provider.provider_type {
        ProviderType::OpenAiResponses => {
            match openai_responses::passthrough(&ctx, raw_body, provider).await {
                Ok(resp) => resp.into_response(),
                Err(e) => e.into_response(),
            }
        }
        ProviderType::ChatCompletions => {
            match deepseek_chat::handle(&ctx, &request, provider).await {
                Ok(resp) => resp.into_response(),
                Err(e) => e.into_response(),
            }
        }
    }
}
