use axum::http::{HeaderMap, HeaderName};

/// 从 HTTP header 提取的请求上下文。
/// 参考 AxonHub `codex/headers.go`。
#[derive(Debug, Clone)]
pub struct GatewayContext {
    pub request_id: String,
    pub session_id: Option<String>,
    pub thread_id: Option<String>,
    pub window_id: Option<String>,
    /// 最终确定的 prompt_cache_key。
    pub prompt_cache_key: String,
    /// 需要合并到上游请求的安全 header。
    pub upstream_headers: HeaderMap,
}

const LIB_MANAGED_HEADERS: &[&str] = &[
    "content-length",
    "transfer-encoding",
    "accept-encoding",
    "host",
];

const SENSITIVE_HEADERS: &[&str] = &[
    "authorization",
    "api-key",
    "x-api-key",
    "x-api-secret",
    "x-api-token",
    "x-goog-api-key",
    "x-google-api-key",
    "cookie",
    "set-cookie",
    "proxy-authorization",
    "www-authenticate",
];

const BLOCKED_HEADERS: &[&str] = &[
    "content-type",
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-connection",
    "te",
    "trailer",
    "trailers",
    "upgrade",
    "x-channel-id",
    "x-project-id",
    "x-real-ip",
    "x-forwarded-for",
    "x-forwarded-proto",
    "x-forwarded-host",
    "x-forwarded-port",
    "x-forwarded-server",
    "accept-language",
    "dnt",
    "origin",
    "referer",
    "sec-fetch-dest",
    "sec-fetch-mode",
    "sec-fetch-site",
    "sec-fetch-user",
    "sec-ch-ua",
    "sec-ch-ua-mobile",
    "sec-ch-ua-platform",
    "ah-trace-id",
    "ah-thread-id",
    "x-initiator",
];

const BLOCKED_HEADER_PREFIXES: &[&str] = &["cf-", "cdn-", "sec-websocket-"];

impl GatewayContext {
    /// 从请求 header 和已解析的 body 提取 GatewayContext。
    pub fn extract(headers: &HeaderMap, body_cache_key: Option<&str>) -> Self {
        let session_id =
            get_header(headers, "session_id").or_else(|| get_header(headers, "session-id"));
        let thread_id = get_header(headers, "thread-id");
        let window_id = get_header(headers, "x-codex-window-id");
        let request_id = get_header(headers, "x-client-request-id")
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        // 从 X-Codex-Turn-Metadata 提取 session_id
        let metadata_session_id = get_header(headers, "x-codex-turn-metadata")
            .and_then(|raw| extract_session_id_from_turn_metadata(&raw));

        // prompt_cache_key 优先级：body → Session_id → session-id → thread-id → metadata → fallback
        let prompt_cache_key = body_cache_key
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .or_else(|| session_id.clone())
            .or_else(|| thread_id.clone())
            .or_else(|| metadata_session_id.clone())
            .unwrap_or_else(|| format!("codexhub:{}", uuid::Uuid::new_v4()));

        let upstream_headers = collect_upstream_headers(headers);

        Self {
            request_id,
            session_id: session_id.or(metadata_session_id),
            thread_id,
            window_id,
            prompt_cache_key,
            upstream_headers,
        }
    }
}

pub fn apply_upstream_headers(
    mut request: reqwest::RequestBuilder,
    headers: &HeaderMap,
) -> reqwest::RequestBuilder {
    for (name, value) in headers.iter() {
        if let Ok(value) = value.to_str() {
            request = request.header(name.as_str(), value);
        }
    }
    request
}

fn collect_upstream_headers(headers: &HeaderMap) -> HeaderMap {
    let mut upstream_headers = HeaderMap::new();
    for (name, value) in headers.iter() {
        if should_forward_header(name) {
            upstream_headers.append(name.clone(), value.clone());
        }
    }
    upstream_headers
}

fn should_forward_header(name: &HeaderName) -> bool {
    let name = name.as_str();
    !contains_header(LIB_MANAGED_HEADERS, name)
        && !contains_header(SENSITIVE_HEADERS, name)
        && !contains_header(BLOCKED_HEADERS, name)
        && !BLOCKED_HEADER_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix))
}

fn contains_header(headers: &[&str], name: &str) -> bool {
    headers
        .iter()
        .any(|blocked| blocked.eq_ignore_ascii_case(name))
}

/// 从 X-Codex-Turn-Metadata JSON 中提取 session_id。
fn extract_session_id_from_turn_metadata(raw: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(raw).ok()?;
    parsed
        .get("session_id")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn get_header(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn test_body_cache_key_highest_priority() {
        let headers = HeaderMap::new();
        let ctx = GatewayContext::extract(&headers, Some("body-key-123"));
        assert_eq!(ctx.prompt_cache_key, "body-key-123");
    }

    #[test]
    fn test_session_id_header_priority() {
        let mut headers = HeaderMap::new();
        headers.insert("session_id", HeaderValue::from_static("sess-abc"));
        let ctx = GatewayContext::extract(&headers, None);
        assert_eq!(ctx.prompt_cache_key, "sess-abc");
        assert_eq!(ctx.session_id.as_deref(), Some("sess-abc"));
    }

    #[test]
    fn test_thread_id_fallback() {
        let mut headers = HeaderMap::new();
        headers.insert("thread-id", HeaderValue::from_static("thread-xyz"));
        let ctx = GatewayContext::extract(&headers, None);
        assert_eq!(ctx.prompt_cache_key, "thread-xyz");
    }

    #[test]
    fn test_turn_metadata_session_id() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-codex-turn-metadata",
            HeaderValue::from_static(r#"{"session_id":"meta-sess-1"}"#),
        );
        let ctx = GatewayContext::extract(&headers, None);
        assert_eq!(ctx.prompt_cache_key, "meta-sess-1");
        assert_eq!(ctx.session_id.as_deref(), Some("meta-sess-1"));
    }

    #[test]
    fn test_fallback_generates_uuid() {
        let headers = HeaderMap::new();
        let ctx = GatewayContext::extract(&headers, None);
        assert!(ctx.prompt_cache_key.starts_with("codexhub:"));
    }

    #[test]
    fn test_empty_body_key_skipped() {
        let mut headers = HeaderMap::new();
        headers.insert("session_id", HeaderValue::from_static("sess-1"));
        let ctx = GatewayContext::extract(&headers, Some(""));
        assert_eq!(ctx.prompt_cache_key, "sess-1");
    }

    #[test]
    fn test_upstream_headers_collected() {
        let mut headers = HeaderMap::new();
        headers.insert("x-codex-window-id", HeaderValue::from_static("win-1"));
        headers.insert("x-client-request-id", HeaderValue::from_static("req-1"));
        headers.insert("x-unrelated", HeaderValue::from_static("nope"));
        let ctx = GatewayContext::extract(&headers, Some("key"));
        assert_eq!(
            ctx.upstream_headers.get("x-codex-window-id").unwrap(),
            "win-1"
        );
        assert_eq!(
            ctx.upstream_headers.get("x-client-request-id").unwrap(),
            "req-1"
        );
        assert_eq!(ctx.upstream_headers.get("x-unrelated").unwrap(), "nope");
    }

    #[test]
    fn test_upstream_headers_follow_proxy_filter_rules() {
        let mut headers = HeaderMap::new();
        headers.insert("user-agent", HeaderValue::from_static("Codex/1.0"));
        headers.insert("accept", HeaderValue::from_static("application/json"));
        headers.insert("session_id", HeaderValue::from_static("sess-1"));
        headers.insert("thread-id", HeaderValue::from_static("thread-1"));
        headers.insert("x-codex-window-id", HeaderValue::from_static("win-1"));
        headers.insert("authorization", HeaderValue::from_static("Bearer codex"));
        headers.insert("content-type", HeaderValue::from_static("text/plain"));
        headers.insert("content-length", HeaderValue::from_static("123"));
        headers.insert("accept-encoding", HeaderValue::from_static("gzip"));
        headers.insert("origin", HeaderValue::from_static("https://example.test"));
        headers.insert("cf-ray", HeaderValue::from_static("edge"));
        headers.insert("sec-websocket-key", HeaderValue::from_static("key"));
        headers.insert("x-forwarded-for", HeaderValue::from_static("127.0.0.1"));

        let ctx = GatewayContext::extract(&headers, Some("key"));

        assert_eq!(ctx.upstream_headers.get("user-agent").unwrap(), "Codex/1.0");
        assert_eq!(
            ctx.upstream_headers.get("accept").unwrap(),
            "application/json"
        );
        assert_eq!(ctx.upstream_headers.get("session_id").unwrap(), "sess-1");
        assert_eq!(ctx.upstream_headers.get("thread-id").unwrap(), "thread-1");
        assert_eq!(
            ctx.upstream_headers.get("x-codex-window-id").unwrap(),
            "win-1"
        );
        assert!(ctx.upstream_headers.get("authorization").is_none());
        assert!(ctx.upstream_headers.get("content-type").is_none());
        assert!(ctx.upstream_headers.get("content-length").is_none());
        assert!(ctx.upstream_headers.get("accept-encoding").is_none());
        assert!(ctx.upstream_headers.get("origin").is_none());
        assert!(ctx.upstream_headers.get("cf-ray").is_none());
        assert!(ctx.upstream_headers.get("sec-websocket-key").is_none());
        assert!(ctx.upstream_headers.get("x-forwarded-for").is_none());
    }

    #[test]
    fn test_apply_upstream_headers_keeps_provider_managed_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("user-agent", HeaderValue::from_static("Codex/1.0"));
        headers.insert("accept", HeaderValue::from_static("application/json"));
        headers.insert("authorization", HeaderValue::from_static("Bearer codex"));
        headers.insert("content-type", HeaderValue::from_static("text/plain"));

        let ctx = GatewayContext::extract(&headers, Some("key"));
        let client = reqwest::Client::new();
        let request = apply_upstream_headers(
            client
                .post("https://upstream.example.test/v1/responses")
                .header("content-type", "application/json")
                .header("authorization", "Bearer provider-key")
                .json(&serde_json::json!({"model":"gpt-5.5"})),
            &ctx.upstream_headers,
        )
        .build()
        .unwrap();

        let request_headers = request.headers();
        assert_eq!(
            request_headers.get("authorization").unwrap(),
            "Bearer provider-key"
        );
        assert_eq!(
            request_headers.get("content-type").unwrap(),
            "application/json"
        );
        assert_eq!(request_headers.get("user-agent").unwrap(), "Codex/1.0");
        assert_eq!(request_headers.get("accept").unwrap(), "application/json");
    }

    #[test]
    fn test_invalid_turn_metadata_json() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-codex-turn-metadata",
            HeaderValue::from_static("not-json"),
        );
        headers.insert("thread-id", HeaderValue::from_static("t-1"));
        let ctx = GatewayContext::extract(&headers, None);
        // invalid JSON → skip, fallback to thread-id
        assert_eq!(ctx.prompt_cache_key, "t-1");
    }
}
