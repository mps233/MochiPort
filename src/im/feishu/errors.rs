use anyhow::anyhow;
use reqwest::StatusCode;
use serde_json::Value as JsonValue;
use thiserror::Error;

pub const FEISHU_APP_SCOPE_MISSING: i64 = 99991672;
pub const FEISHU_USER_SCOPE_INSUFFICIENT: i64 = 99991679;
pub const FEISHU_TOKEN_INVALID: i64 = 99991668;
pub const FEISHU_TOKEN_EXPIRED: i64 = 99991677;

#[derive(Debug, Clone, Error)]
pub enum FeishuApiError {
    #[error("feishu app scope missing: operation={operation} code={code} message={message}")]
    AppScopeMissing {
        operation: String,
        code: i64,
        message: String,
        body: JsonValue,
    },
    #[error("feishu user scope insufficient: operation={operation} code={code} message={message}")]
    UserScopeInsufficient {
        operation: String,
        code: i64,
        message: String,
        body: JsonValue,
    },
    #[error(
        "feishu user authorization required: operation={operation} code={code} message={message}"
    )]
    UserAuthRequired {
        operation: String,
        code: i64,
        message: String,
        body: JsonValue,
    },
    #[error("feishu request failed: operation={operation} code={code} message={message}")]
    Other {
        operation: String,
        code: i64,
        message: String,
        body: JsonValue,
    },
}

impl FeishuApiError {
    #[allow(dead_code)]
    pub fn code(&self) -> i64 {
        match self {
            FeishuApiError::AppScopeMissing { code, .. }
            | FeishuApiError::UserScopeInsufficient { code, .. }
            | FeishuApiError::UserAuthRequired { code, .. }
            | FeishuApiError::Other { code, .. } => *code,
        }
    }

    #[allow(dead_code)]
    pub fn operation(&self) -> &str {
        match self {
            FeishuApiError::AppScopeMissing { operation, .. }
            | FeishuApiError::UserScopeInsufficient { operation, .. }
            | FeishuApiError::UserAuthRequired { operation, .. }
            | FeishuApiError::Other { operation, .. } => operation,
        }
    }

    #[allow(dead_code)]
    pub fn body(&self) -> &JsonValue {
        match self {
            FeishuApiError::AppScopeMissing { body, .. }
            | FeishuApiError::UserScopeInsufficient { body, .. }
            | FeishuApiError::UserAuthRequired { body, .. }
            | FeishuApiError::Other { body, .. } => body,
        }
    }
}

pub fn classify_feishu_api_error(operation: &str, code: i64, body: &JsonValue) -> FeishuApiError {
    let message = body
        .get("msg")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    match code {
        FEISHU_APP_SCOPE_MISSING => FeishuApiError::AppScopeMissing {
            operation: operation.to_string(),
            code,
            message,
            body: body.clone(),
        },
        FEISHU_USER_SCOPE_INSUFFICIENT => FeishuApiError::UserScopeInsufficient {
            operation: operation.to_string(),
            code,
            message,
            body: body.clone(),
        },
        FEISHU_TOKEN_INVALID | FEISHU_TOKEN_EXPIRED => FeishuApiError::UserAuthRequired {
            operation: operation.to_string(),
            code,
            message,
            body: body.clone(),
        },
        _ => FeishuApiError::Other {
            operation: operation.to_string(),
            code,
            message,
            body: body.clone(),
        },
    }
}

pub fn ensure_feishu_api_success(
    operation: &str,
    status: StatusCode,
    body: &JsonValue,
) -> anyhow::Result<()> {
    if !status.is_success() {
        return Err(anyhow!(
            "feishu request failed: operation={} status={} body={}",
            operation,
            status,
            body
        ));
    }
    let code = body.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    if code != 0 {
        return Err(classify_feishu_api_error(operation, code, body).into());
    }
    Ok(())
}
