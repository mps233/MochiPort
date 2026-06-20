use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};

/// AI Gateway 错误类型。
#[derive(Debug)]
pub struct GatewayError {
    pub status: StatusCode,
    pub error_type: String,
    pub code: String,
    pub message: String,
    pub provider: Option<String>,
    pub upstream_status: Option<u16>,
    pub upstream_error_type: Option<String>,
    pub upstream_code: Option<String>,
}

impl GatewayError {
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            error_type: "invalid_request_error".to_string(),
            code: "invalid_request".to_string(),
            message: message.into(),
            provider: None,
            upstream_status: None,
            upstream_error_type: None,
            upstream_code: None,
        }
    }

    pub fn invalid_model(model: &str) -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            error_type: "invalid_model_error".to_string(),
            code: "invalid_model".to_string(),
            message: format!("model '{}' is not configured in any provider", model),
            provider: None,
            upstream_status: None,
            upstream_error_type: None,
            upstream_code: None,
        }
    }

    pub fn not_implemented() -> Self {
        Self {
            status: StatusCode::NOT_IMPLEMENTED,
            error_type: "not_implemented".to_string(),
            code: "not_implemented".to_string(),
            message: "AI Gateway is not yet implemented".into(),
            provider: None,
            upstream_status: None,
            upstream_error_type: None,
            upstream_code: None,
        }
    }

    pub fn upstream(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            error_type: "upstream_error".to_string(),
            code: "upstream_error".to_string(),
            message: message.into(),
            provider: None,
            upstream_status: Some(status.as_u16()),
            upstream_error_type: None,
            upstream_code: None,
        }
    }

    pub fn upstream_provider(
        status: StatusCode,
        provider: impl Into<String>,
        message: impl Into<String>,
        upstream_error_type: Option<String>,
        upstream_code: Option<String>,
    ) -> Self {
        Self {
            status,
            error_type: "upstream_error".to_string(),
            code: upstream_code
                .clone()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "upstream_error".to_string()),
            message: message.into(),
            provider: Some(provider.into()),
            upstream_status: Some(status.as_u16()),
            upstream_error_type,
            upstream_code,
        }
    }

    pub fn from_upstream_body(status: StatusCode, provider: &str, body: &str) -> Self {
        let parsed = serde_json::from_str::<Value>(body).ok();
        let parsed_error = parsed.as_ref().and_then(parse_upstream_error);
        let (message, upstream_error_type, upstream_code) = match parsed_error {
            Some(error) => (error.message, error.error_type, error.code),
            None => {
                let message = body.trim();
                let message = if message.is_empty() {
                    format!("upstream provider returned HTTP {}", status.as_u16())
                } else {
                    message.to_string()
                };
                (message, None, None)
            }
        };

        Self::upstream_provider(
            status,
            provider,
            message,
            upstream_error_type,
            upstream_code,
        )
    }

    pub fn upstream_timeout() -> Self {
        Self {
            status: StatusCode::GATEWAY_TIMEOUT,
            error_type: "upstream_timeout".to_string(),
            code: "upstream_timeout".to_string(),
            message: "upstream provider request timed out".into(),
            provider: None,
            upstream_status: None,
            upstream_error_type: None,
            upstream_code: None,
        }
    }
}

/// 转为 Responses API 格式的 JSON 错误响应。
impl IntoResponse for GatewayError {
    fn into_response(self) -> Response {
        let mut error = serde_json::Map::new();
        error.insert("message".to_string(), Value::String(self.message));
        error.insert("type".to_string(), Value::String(self.error_type));
        error.insert("code".to_string(), Value::String(self.code));
        if let Some(provider) = self.provider {
            error.insert("provider".to_string(), Value::String(provider));
        }
        if let Some(status) = self.upstream_status {
            error.insert(
                "upstream_status".to_string(),
                Value::Number(serde_json::Number::from(status)),
            );
        }
        if let Some(error_type) = self.upstream_error_type {
            error.insert("upstream_type".to_string(), Value::String(error_type));
        }
        if let Some(code) = self.upstream_code {
            error.insert("upstream_code".to_string(), Value::String(code));
        }

        let body = json!({ "error": Value::Object(error) });
        (self.status, Json(body)).into_response()
    }
}

struct ParsedUpstreamError {
    message: String,
    error_type: Option<String>,
    code: Option<String>,
}

fn parse_upstream_error(value: &Value) -> Option<ParsedUpstreamError> {
    let error = value.get("error").unwrap_or(value);
    let message = error
        .get("message")
        .or_else(|| value.get("message"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_string();
    let error_type = error
        .get("type")
        .or_else(|| value.get("type"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let code = error
        .get("code")
        .or_else(|| value.get("code"))
        .and_then(|value| match value {
            Value::String(text) => Some(text.trim().to_string()),
            Value::Number(number) => Some(number.to_string()),
            _ => None,
        })
        .filter(|value| !value.is_empty());

    Some(ParsedUpstreamError {
        message,
        error_type,
        code,
    })
}

#[cfg(test)]
mod tests {
    use axum::response::IntoResponse;
    use serde_json::Value;

    use super::*;

    async fn error_body(error: GatewayError) -> Value {
        let response = error.into_response();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        serde_json::from_slice(&body).expect("json body")
    }

    #[tokio::test]
    async fn parses_openai_style_upstream_error() {
        let error = GatewayError::from_upstream_body(
            StatusCode::BAD_REQUEST,
            "openai",
            r#"{"error":{"message":"bad request","type":"invalid_request_error","code":"bad_param"}}"#,
        );
        let body = error_body(error).await;

        assert_eq!(body["error"]["message"], "bad request");
        assert_eq!(body["error"]["type"], "upstream_error");
        assert_eq!(body["error"]["code"], "bad_param");
        assert_eq!(body["error"]["provider"], "openai");
        assert_eq!(body["error"]["upstream_status"], 400);
        assert_eq!(body["error"]["upstream_type"], "invalid_request_error");
        assert_eq!(body["error"]["upstream_code"], "bad_param");
    }

    #[tokio::test]
    async fn parses_anthropic_style_upstream_error() {
        let error = GatewayError::from_upstream_body(
            StatusCode::TOO_MANY_REQUESTS,
            "anthropic",
            r#"{"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#,
        );
        let body = error_body(error).await;

        assert_eq!(body["error"]["message"], "slow down");
        assert_eq!(body["error"]["code"], "upstream_error");
        assert_eq!(body["error"]["provider"], "anthropic");
        assert_eq!(body["error"]["upstream_status"], 429);
        assert_eq!(body["error"]["upstream_type"], "rate_limit_error");
    }

    #[tokio::test]
    async fn falls_back_to_plain_text_upstream_error() {
        let error = GatewayError::from_upstream_body(
            StatusCode::BAD_GATEWAY,
            "deepseek",
            "gateway unavailable",
        );
        let body = error_body(error).await;

        assert_eq!(body["error"]["message"], "gateway unavailable");
        assert_eq!(body["error"]["code"], "upstream_error");
        assert_eq!(body["error"]["provider"], "deepseek");
        assert_eq!(body["error"]["upstream_status"], 502);
        assert!(body["error"]["upstream_type"].is_null());
    }

    #[tokio::test]
    async fn falls_back_to_status_when_upstream_body_is_empty() {
        let error =
            GatewayError::from_upstream_body(StatusCode::SERVICE_UNAVAILABLE, "openai", "  ");
        let body = error_body(error).await;

        assert_eq!(
            body["error"]["message"],
            "upstream provider returned HTTP 503"
        );
        assert_eq!(body["error"]["upstream_status"], 503);
    }
}
