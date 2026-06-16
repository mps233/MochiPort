//! DeepSeek Chat Completions 出站 provider。
//! 参考 AxonHub `deepseek/outbound.go`。

use axum::{
    body::Body,
    http::{HeaderName, HeaderValue, StatusCode},
    response::Response,
};
use tracing::{debug, error};

use crate::ai_gateway::config::ProviderConfig;
use crate::ai_gateway::context::GatewayContext;
use crate::ai_gateway::error::GatewayError;
use crate::ai_gateway::model::GatewayRequest;
use crate::ai_gateway::transform::chat_to_responses::convert_chat_response;
use crate::ai_gateway::transform::responses_stream::ChatSseToResponsesSse;
use crate::ai_gateway::transform::responses_to_chat::build_chat_request;

/// DeepSeek Chat Completions 出站处理。
pub async fn handle(
    _ctx: &GatewayContext,
    request: &GatewayRequest,
    provider: &ProviderConfig,
) -> Result<Response<Body>, GatewayError> {
    // 1. Responses → Chat Completions 请求转换
    let chat_body = build_chat_request(request, true)
        .map_err(|e| GatewayError::bad_request(format!("transform error: {e}")))?;

    let url = format!(
        "{}/v1/chat/completions",
        provider.base_url.trim_end_matches('/')
    );

    debug!(url = %url, stream = request.stream, "proxying to deepseek chat");

    // 2. 发送上游请求
    let client = reqwest::Client::new();
    let upstream_resp = client
        .post(&url)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {}", provider.api_key))
        .timeout(std::time::Duration::from_secs(provider.timeout_secs))
        .json(&chat_body)
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                GatewayError::upstream_timeout()
            } else {
                error!(error = %e, "deepseek upstream request failed");
                GatewayError::upstream(StatusCode::BAD_GATEWAY, format!("upstream error: {e}"))
            }
        })?;

    let upstream_status = upstream_resp.status();
    if !upstream_status.is_success() {
        let status =
            StatusCode::from_u16(upstream_status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
        let body_text = upstream_resp.text().await.unwrap_or_default();
        return Err(GatewayError::upstream(status, body_text));
    }

    // 3. 流式 vs 非流式
    if request.stream {
        handle_stream(upstream_resp, &request.model).await
    } else {
        handle_non_stream(upstream_resp, &request.model).await
    }
}

/// 非流式：Chat JSON → Responses JSON。
async fn handle_non_stream(
    resp: reqwest::Response,
    model: &str,
) -> Result<Response<Body>, GatewayError> {
    let chat_resp: serde_json::Value = resp.json().await.map_err(|e| {
        GatewayError::upstream(StatusCode::BAD_GATEWAY, format!("parse upstream json: {e}"))
    })?;

    let response_obj = convert_chat_response(&chat_resp, model).map_err(|e| {
        GatewayError::upstream(StatusCode::BAD_GATEWAY, format!("transform error: {e}"))
    })?;

    let body_bytes = serde_json::to_vec(&response_obj).unwrap_or_default();
    let mut response = Response::new(Body::from(body_bytes));
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        HeaderName::from_static("content-type"),
        HeaderValue::from_static("application/json"),
    );
    Ok(response)
}

/// 流式：Chat SSE → Responses SSE（通过状态机转换）。
async fn handle_stream(
    resp: reqwest::Response,
    model: &str,
) -> Result<Response<Body>, GatewayError> {
    let model = model.to_string();
    let byte_stream = resp.bytes_stream();

    let sse_stream = ChatSseToResponsesSse::new(byte_stream, model);

    let body = Body::from_stream(sse_stream);
    let mut response = Response::new(body);
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        HeaderName::from_static("content-type"),
        HeaderValue::from_static("text/event-stream"),
    );
    response.headers_mut().insert(
        HeaderName::from_static("cache-control"),
        HeaderValue::from_static("no-cache"),
    );
    Ok(response)
}
