pub mod catalog;
pub mod config;
pub mod context;
pub mod error;
pub mod handler;
pub mod model;
pub mod providers;
pub mod request_log;
pub mod router;
pub mod transform;

use axum::{
    Router,
    extract::DefaultBodyLimit,
    routing::{get, post},
};

use crate::app_state::SharedState;

/// 构建 AI Gateway 子路由（state 由父 Router 提供）。
pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/v1/responses", post(handler::handle_responses))
        .route("/v1/models", get(handler::handle_models))
        .route("/request-logs", get(handler::handle_request_logs))
        .layer(DefaultBodyLimit::disable())
}
