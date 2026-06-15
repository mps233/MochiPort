use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;

/// AI Gateway 错误类型。
#[derive(Debug)]
pub struct GatewayError {
    pub status: StatusCode,
    pub error_type: &'static str,
    pub code: &'static str,
    pub message: String,
}

impl GatewayError {
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            error_type: "invalid_request_error",
            code: "invalid_request",
            message: message.into(),
        }
    }

    pub fn invalid_model(model: &str) -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            error_type: "invalid_model_error",
            code: "invalid_model",
            message: format!("model '{}' is not configured in any provider", model),
        }
    }

    pub fn not_implemented() -> Self {
        Self {
            status: StatusCode::NOT_IMPLEMENTED,
            error_type: "not_implemented",
            code: "not_implemented",
            message: "AI Gateway is not yet implemented".into(),
        }
    }

    pub fn gateway_disabled() -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            error_type: "service_unavailable",
            code: "gateway_disabled",
            message: "AI Gateway is not enabled".into(),
        }
    }

    pub fn upstream(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            error_type: "upstream_error",
            code: "upstream_error",
            message: message.into(),
        }
    }

    pub fn upstream_timeout() -> Self {
        Self {
            status: StatusCode::GATEWAY_TIMEOUT,
            error_type: "upstream_timeout",
            code: "upstream_timeout",
            message: "upstream provider request timed out".into(),
        }
    }
}

/// 转为 Responses API 格式的 JSON 错误响应。
impl IntoResponse for GatewayError {
    fn into_response(self) -> Response {
        let body = json!({
            "error": {
                "message": self.message,
                "type": self.error_type,
                "code": self.code,
            }
        });
        (self.status, Json(body)).into_response()
    }
}
