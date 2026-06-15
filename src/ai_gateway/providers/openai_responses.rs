use axum::{
    body::Body,
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::Response,
};
use futures_util::StreamExt;
use serde_json::json;
use tracing::{error, debug};

use crate::ai_gateway::config::ProviderConfig;
use crate::ai_gateway::context::GatewayContext;
use crate::ai_gateway::error::GatewayError;

/// OpenAI Responses API 透传：补齐 cache 字段后代理到上游。
pub async fn passthrough(
    ctx: &GatewayContext,
    mut raw_body: serde_json::Value,
    provider: &ProviderConfig,
) -> Result<Response<Body>, GatewayError> {
    // 1. 补齐 prompt_cache_key
    let existing_key = raw_body
        .get("prompt_cache_key")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if existing_key.is_empty() {
        raw_body["prompt_cache_key"] = json!(ctx.prompt_cache_key);
    }

    // 2. 补齐 prompt_cache_retention
    if let Some(retention) = &provider.prompt_cache_retention {
        if raw_body.get("prompt_cache_retention").is_none() {
            raw_body["prompt_cache_retention"] = json!(retention);
        }
    }

    let is_stream = raw_body
        .get("stream")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // 3. 构建上游请求
    let url = format!(
        "{}/v1/responses",
        provider.base_url.trim_end_matches('/')
    );

    let client = reqwest::Client::new();
    let mut req_builder = client
        .post(&url)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {}", provider.api_key))
        .timeout(std::time::Duration::from_secs(provider.timeout_secs))
        .json(&raw_body);

    // 4. 透传 Codex header
    for (name, value) in ctx.passthrough_headers.iter() {
        if let Ok(v) = value.to_str() {
            req_builder = req_builder.header(name.as_str(), v);
        }
    }

    debug!(url = %url, stream = is_stream, "proxying to openai responses");

    let upstream_resp = req_builder.send().await.map_err(|e| {
        if e.is_timeout() {
            GatewayError::upstream_timeout()
        } else {
            error!(error = %e, "upstream request failed");
            GatewayError::upstream(StatusCode::BAD_GATEWAY, format!("upstream error: {e}"))
        }
    })?;

    let upstream_status = upstream_resp.status();

    // 5. 非 2xx 直接透传错误
    if !upstream_status.is_success() {
        let status =
            StatusCode::from_u16(upstream_status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
        let body_text = upstream_resp.text().await.unwrap_or_default();
        return Err(GatewayError::upstream(status, body_text));
    }

    // 6. 流式：透传 SSE 流
    if is_stream {
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("content-type"),
            HeaderValue::from_static("text/event-stream"),
        );
        headers.insert(
            HeaderName::from_static("cache-control"),
            HeaderValue::from_static("no-cache"),
        );

        let byte_stream = upstream_resp.bytes_stream().map(|result| {
            result.map_err(|e| {
                error!(error = %e, "upstream SSE stream error");
                std::io::Error::new(std::io::ErrorKind::Other, e)
            })
        });
        let body = Body::from_stream(byte_stream);
        let mut response = Response::new(body);
        *response.status_mut() = StatusCode::OK;
        *response.headers_mut() = headers;
        return Ok(response);
    }

    // 7. 非流式：透传 JSON 响应
    let body_bytes = upstream_resp.bytes().await.map_err(|e| {
        GatewayError::upstream(StatusCode::BAD_GATEWAY, format!("read upstream body: {e}"))
    })?;

    let mut response = Response::new(Body::from(body_bytes));
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        HeaderName::from_static("content-type"),
        HeaderValue::from_static("application/json"),
    );
    Ok(response)
}
