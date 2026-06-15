use axum::http::HeaderMap;

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
    /// 需要透传到上游的 Codex header。
    pub passthrough_headers: HeaderMap,
}

/// Codex 需要透传的 header 列表。
const PASSTHROUGH_HEADER_NAMES: &[&str] = &[
    "x-codex-turn-metadata",
    "x-codex-window-id",
    "x-client-request-id",
    "x-codex-beta-features",
];

impl GatewayContext {
    /// 从请求 header 和已解析的 body 提取 GatewayContext。
    pub fn extract(headers: &HeaderMap, body_cache_key: Option<&str>) -> Self {
        let session_id = get_header(headers, "session_id")
            .or_else(|| get_header(headers, "session-id"));
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
            .unwrap_or_else(|| format!("codex-remote:{}", uuid::Uuid::new_v4()));

        // 收集透传 header
        let mut passthrough_headers = HeaderMap::new();
        for name in PASSTHROUGH_HEADER_NAMES {
            if let Some(value) = headers.get(*name) {
                if let Ok(header_name) = axum::http::header::HeaderName::from_bytes(name.as_bytes())
                {
                    passthrough_headers.insert(header_name, value.clone());
                }
            }
        }

        Self {
            request_id,
            session_id: session_id.or(metadata_session_id),
            thread_id,
            window_id,
            prompt_cache_key,
            passthrough_headers,
        }
    }
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
        assert!(ctx.prompt_cache_key.starts_with("codex-remote:"));
    }

    #[test]
    fn test_empty_body_key_skipped() {
        let mut headers = HeaderMap::new();
        headers.insert("session_id", HeaderValue::from_static("sess-1"));
        let ctx = GatewayContext::extract(&headers, Some(""));
        assert_eq!(ctx.prompt_cache_key, "sess-1");
    }

    #[test]
    fn test_passthrough_headers_collected() {
        let mut headers = HeaderMap::new();
        headers.insert("x-codex-window-id", HeaderValue::from_static("win-1"));
        headers.insert("x-client-request-id", HeaderValue::from_static("req-1"));
        headers.insert("x-unrelated", HeaderValue::from_static("nope"));
        let ctx = GatewayContext::extract(&headers, Some("key"));
        assert_eq!(
            ctx.passthrough_headers.get("x-codex-window-id").unwrap(),
            "win-1"
        );
        assert_eq!(
            ctx.passthrough_headers.get("x-client-request-id").unwrap(),
            "req-1"
        );
        assert!(ctx.passthrough_headers.get("x-unrelated").is_none());
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
