//! Anthropic Messages 出站 provider。

use axum::{
    body::Body,
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::Response,
};
use serde_json::Value;
use tracing::{debug, error};

use crate::ai_gateway::config::ProviderConfig;
use crate::ai_gateway::context::GatewayContext;
use crate::ai_gateway::error::GatewayError;
use crate::ai_gateway::model::GatewayRequest;
use crate::ai_gateway::request_log::{
    self, RequestLogContext, RequestLogUpdate, ResponsesSseLogStream, UpstreamSseCaptureStream,
};
use crate::ai_gateway::tool_names::ToolNameMap;

use super::{
    apply_total_request_timeout, ensure_success_response, execute_stream_start,
    execute_upstream_request,
};

mod citations;
mod custom_tools;
mod glm_compat;
mod options;
mod request;
mod request_content;
mod request_reasoning;
mod request_tools;
mod response;
mod stream;
mod stream_events;
mod stream_items;
mod stream_message;
mod stream_reasoning;
mod stream_response;
mod stream_state;
mod stream_tools;
mod types;

#[cfg(test)]
mod tests;

use options::{
    AnthropicAuthStyle, AnthropicHeaderStyle, AnthropicProviderOptions, AnthropicProviderProfile,
    AnthropicVersionHeader,
};
use request::build_anthropic_request;
use response::convert_anthropic_response;
use stream::AnthropicSseToResponsesSse;
use types::{
    ANTHROPIC_CLAUDE_CODE_BETA, CLAUDE_CODE_STAINLESS_PACKAGE_VERSION,
    CLAUDE_CODE_STAINLESS_RUNTIME_VERSION, CLAUDE_CODE_USER_AGENT,
};

pub async fn handle(
    client: &reqwest::Client,
    ctx: &GatewayContext,
    request: &GatewayRequest,
    response_model: &str,
    provider: &ProviderConfig,
    log_context: Option<RequestLogContext>,
) -> Result<Response<Body>, GatewayError> {
    let options = AnthropicProviderOptions::from_provider(provider)?;
    let (anthropic_body, tool_name_map) = build_anthropic_request(request, options.profile)?;
    let url = options.messages_url(provider);
    debug!(url = %url, stream = false, "proxying to anthropic messages");

    let upstream_req = build_anthropic_upstream_request(
        client,
        ctx,
        request,
        &anthropic_body,
        provider,
        &options,
    )?;

    if let Some(log_context) = &log_context {
        let update = RequestLogUpdate {
            upstream_request_headers_json: request_log::headers_to_json(upstream_req.headers()),
            upstream_request_body_bytes: request_log::json_body_size_bytes(&anthropic_body),
            upstream_request_json: serde_json::to_string(&anthropic_body).ok(),
            ..RequestLogUpdate::default()
        };
        if let Err(err) = log_context.store.update_record(log_context.log_id, &update) {
            request_log::log_update_error(err);
        }
    }

    let upstream_resp = if request.stream {
        execute_stream_start(
            client,
            upstream_req,
            provider.timeout_secs,
            "anthropic upstream request failed",
        )
        .await?
    } else {
        execute_upstream_request(
            client,
            upstream_req,
            provider.timeout_secs,
            "anthropic upstream request failed",
        )
        .await?
    };
    let upstream_resp = ensure_success_response(&provider.name, upstream_resp).await?;

    if request.stream {
        return handle_stream(
            upstream_resp,
            response_model,
            tool_name_map,
            options.profile,
            log_context,
        )
        .await;
    }

    let anthropic_resp: Value = upstream_resp.json().await.map_err(|e| {
        GatewayError::upstream(StatusCode::BAD_GATEWAY, format!("parse upstream json: {e}"))
    })?;
    let response_obj = convert_anthropic_response(
        &anthropic_resp,
        response_model,
        &tool_name_map,
        options.profile,
    );
    let body_bytes = serde_json::to_vec(&response_obj).unwrap_or_default();

    if let Some(log_context) = &log_context {
        let response_value = serde_json::to_value(&response_obj).unwrap_or_default();
        let update = RequestLogUpdate {
            status: Some(response_obj.status.clone()),
            usage: Some(request_log::usage_from_response_value(&response_value)),
            latency_ms: Some(request_log::elapsed_ms(log_context.started_at)),
            response_json: serde_json::to_string(&response_value).ok(),
            ..RequestLogUpdate::default()
        };
        if let Err(err) = log_context.store.update_record(log_context.log_id, &update) {
            request_log::log_update_error(err);
        }
    }

    let mut response = Response::new(Body::from(body_bytes));
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        HeaderName::from_static("content-type"),
        HeaderValue::from_static("application/json"),
    );
    Ok(response)
}

fn build_anthropic_upstream_request(
    client: &reqwest::Client,
    ctx: &GatewayContext,
    request: &GatewayRequest,
    anthropic_body: &Value,
    provider: &ProviderConfig,
    options: &AnthropicProviderOptions,
) -> Result<reqwest::Request, GatewayError> {
    let req_builder = client
        .post(options.messages_url(provider))
        .json(anthropic_body);
    let req_builder =
        apply_total_request_timeout(req_builder, provider.timeout_secs, request.stream);
    let mut upstream_req = req_builder.build().map_err(|e| {
        error!(error = %e, "build anthropic upstream request failed");
        GatewayError::upstream(
            StatusCode::BAD_GATEWAY,
            format!("build upstream request: {e}"),
        )
    })?;

    *upstream_req.version_mut() = reqwest::Version::HTTP_11;
    apply_anthropic_managed_headers(upstream_req.headers_mut(), ctx, provider, options)?;
    Ok(upstream_req)
}

fn apply_anthropic_managed_headers(
    headers: &mut HeaderMap,
    ctx: &GatewayContext,
    provider: &ProviderConfig,
    options: &AnthropicProviderOptions,
) -> Result<(), GatewayError> {
    headers.clear();
    insert_static_header(headers, "content-type", "application/json");
    apply_anthropic_auth(headers, provider, options)?;
    apply_anthropic_version_header(headers, options);
    apply_anthropic_client_headers(headers, ctx, provider, options)?;
    Ok(())
}

fn apply_anthropic_auth(
    headers: &mut HeaderMap,
    provider: &ProviderConfig,
    options: &AnthropicProviderOptions,
) -> Result<(), GatewayError> {
    match options.auth {
        AnthropicAuthStyle::XApiKey => {
            insert_dynamic_header(headers, "x-api-key", provider.api_key.trim())?;
        }
        AnthropicAuthStyle::Bearer => {
            insert_dynamic_header(
                headers,
                "authorization",
                bearer_authorization(&provider.api_key),
            )?;
        }
    }
    Ok(())
}

fn apply_anthropic_version_header(headers: &mut HeaderMap, options: &AnthropicProviderOptions) {
    match options.version_header {
        AnthropicVersionHeader::Required(version) => {
            insert_static_header(headers, "anthropic-version", version);
        }
    }
}

fn apply_anthropic_client_headers(
    headers: &mut HeaderMap,
    ctx: &GatewayContext,
    provider: &ProviderConfig,
    options: &AnthropicProviderOptions,
) -> Result<(), GatewayError> {
    match options.headers {
        AnthropicHeaderStyle::Plain => {}
        AnthropicHeaderStyle::ClaudeCode => {
            let existing_beta = headers
                .get("anthropic-beta")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default();
            let merged_beta = merge_anthropic_betas(ANTHROPIC_CLAUDE_CODE_BETA, existing_beta);
            insert_dynamic_header(headers, "anthropic-beta", &merged_beta)?;
            insert_static_header(headers, "anthropic-dangerous-direct-browser-access", "true");
            insert_static_header(headers, "x-app", "cli");
            insert_static_header(headers, "accept", "application/json");
            insert_static_header(headers, "accept-encoding", "gzip, deflate, br, zstd");
            insert_static_header(headers, "connection", "keep-alive");
            insert_static_header(headers, "user-agent", CLAUDE_CODE_USER_AGENT);
            insert_dynamic_header(
                headers,
                "x-stainless-timeout",
                provider.timeout_secs.to_string(),
            )?;
            insert_static_header(headers, "x-stainless-retry-count", "0");
            insert_static_header(headers, "x-stainless-runtime", "node");
            insert_static_header(headers, "x-stainless-lang", "js");
            insert_static_header(headers, "x-stainless-arch", stainless_arch());
            insert_static_header(headers, "x-stainless-os", stainless_os());
            insert_static_header(
                headers,
                "x-stainless-package-version",
                CLAUDE_CODE_STAINLESS_PACKAGE_VERSION,
            );
            insert_static_header(
                headers,
                "x-stainless-runtime-version",
                CLAUDE_CODE_STAINLESS_RUNTIME_VERSION,
            );
            let session_id = ctx.session_id.as_deref().unwrap_or(&ctx.prompt_cache_key);
            insert_dynamic_header(headers, "x-claude-code-session-id", session_id)?;
        }
    }
    Ok(())
}

fn insert_static_header(headers: &mut HeaderMap, name: &'static str, value: &'static str) {
    headers.insert(
        HeaderName::from_static(name),
        HeaderValue::from_static(value),
    );
}

fn insert_dynamic_header(
    headers: &mut HeaderMap,
    name: &'static str,
    value: impl AsRef<str>,
) -> Result<(), GatewayError> {
    let value = HeaderValue::from_str(value.as_ref()).map_err(|e| {
        GatewayError::upstream(
            StatusCode::BAD_GATEWAY,
            format!("invalid Anthropic header '{name}': {e}"),
        )
    })?;
    headers.insert(HeaderName::from_static(name), value);
    Ok(())
}

fn bearer_authorization(api_key: &str) -> String {
    let api_key = api_key.trim();
    if let Some(token) = strip_bearer_prefix(api_key) {
        format!("Bearer {}", token.trim())
    } else {
        format!("Bearer {api_key}")
    }
}

fn strip_bearer_prefix(value: &str) -> Option<&str> {
    let prefix_len = "bearer ".len();
    if value
        .get(..prefix_len)
        .map(|prefix| prefix.eq_ignore_ascii_case("bearer "))
        .unwrap_or(false)
    {
        Some(&value[prefix_len..])
    } else {
        None
    }
}

fn merge_anthropic_betas(required: &str, inbound: &str) -> String {
    let mut merged = Vec::new();
    for beta in required.split(',').chain(inbound.split(',')) {
        let beta = beta.trim();
        if !beta.is_empty() && !merged.contains(&beta) {
            merged.push(beta);
        }
    }
    merged.join(",")
}

fn stainless_arch() -> &'static str {
    #[cfg(target_arch = "x86_64")]
    {
        "x64"
    }
    #[cfg(target_arch = "aarch64")]
    {
        "arm64"
    }
    #[cfg(all(not(target_arch = "x86_64"), not(target_arch = "aarch64")))]
    {
        std::env::consts::ARCH
    }
}

fn stainless_os() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "Windows"
    }
    #[cfg(target_os = "macos")]
    {
        "MacOS"
    }
    #[cfg(target_os = "linux")]
    {
        "Linux"
    }
    #[cfg(all(
        not(target_os = "windows"),
        not(target_os = "macos"),
        not(target_os = "linux")
    ))]
    {
        std::env::consts::OS
    }
}

async fn handle_stream(
    resp: reqwest::Response,
    model: &str,
    tool_name_map: ToolNameMap,
    profile: AnthropicProviderProfile,
    log_context: Option<RequestLogContext>,
) -> Result<Response<Body>, GatewayError> {
    let upstream_bytes = resp.bytes_stream();
    let body = if let Some(log_context) = log_context {
        let captured_upstream = UpstreamSseCaptureStream::new(upstream_bytes, log_context.clone());
        let sse_stream = AnthropicSseToResponsesSse::new(
            captured_upstream,
            model.to_string(),
            tool_name_map,
            profile,
        );
        Body::from_stream(ResponsesSseLogStream::new(sse_stream, log_context))
    } else {
        let sse_stream = AnthropicSseToResponsesSse::new(
            upstream_bytes,
            model.to_string(),
            tool_name_map,
            profile,
        );
        Body::from_stream(sse_stream)
    };

    let mut headers = HeaderMap::new();
    headers.insert(
        HeaderName::from_static("content-type"),
        HeaderValue::from_static("text/event-stream"),
    );
    headers.insert(
        HeaderName::from_static("cache-control"),
        HeaderValue::from_static("no-cache"),
    );
    headers.insert(
        HeaderName::from_static("connection"),
        HeaderValue::from_static("keep-alive"),
    );

    let mut response = Response::new(body);
    *response.status_mut() = StatusCode::OK;
    *response.headers_mut() = headers;
    Ok(response)
}
