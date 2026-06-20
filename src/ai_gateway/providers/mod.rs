pub mod anthropic_messages;
pub mod deepseek_chat;
pub mod openai_responses;

use std::time::Duration;

use axum::http::StatusCode;
use tracing::error;

use crate::ai_gateway::error::GatewayError;

pub(super) fn apply_total_request_timeout(
    builder: reqwest::RequestBuilder,
    timeout_secs: u64,
    stream: bool,
) -> reqwest::RequestBuilder {
    if stream {
        builder
    } else {
        builder.timeout(provider_timeout(timeout_secs))
    }
}

pub(super) async fn execute_stream_start(
    client: &reqwest::Client,
    request: reqwest::Request,
    timeout_secs: u64,
    error_log: &'static str,
) -> Result<reqwest::Response, GatewayError> {
    let response = tokio::time::timeout(provider_timeout(timeout_secs), client.execute(request))
        .await
        .map_err(|_| GatewayError::upstream_timeout())?;
    map_upstream_response(response, error_log)
}

pub(super) fn map_upstream_response(
    response: Result<reqwest::Response, reqwest::Error>,
    error_log: &'static str,
) -> Result<reqwest::Response, GatewayError> {
    response.map_err(|err| {
        if err.is_timeout() {
            GatewayError::upstream_timeout()
        } else {
            error!(error = %err, "{error_log}");
            GatewayError::upstream(StatusCode::BAD_GATEWAY, format!("upstream error: {err}"))
        }
    })
}

pub(super) async fn ensure_success_response(
    provider_name: &str,
    response: reqwest::Response,
) -> Result<reqwest::Response, GatewayError> {
    let upstream_status = response.status();
    if upstream_status.is_success() {
        return Ok(response);
    }

    let status = StatusCode::from_u16(upstream_status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let body_text = response.text().await.unwrap_or_default();
    Err(GatewayError::from_upstream_body(
        status,
        provider_name,
        &body_text,
    ))
}

fn provider_timeout(timeout_secs: u64) -> Duration {
    Duration::from_secs(timeout_secs.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn total_request_timeout_is_not_applied_to_streaming_requests() {
        let client = reqwest::Client::new();

        let streaming_request =
            apply_total_request_timeout(client.get("https://example.com/stream"), 7, true)
                .build()
                .unwrap();
        assert!(streaming_request.timeout().is_none());

        let unary_request =
            apply_total_request_timeout(client.get("https://example.com/json"), 7, false)
                .build()
                .unwrap();
        assert_eq!(unary_request.timeout(), Some(&Duration::from_secs(7)));
    }
}
