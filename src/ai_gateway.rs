pub mod config;
pub mod context;
pub mod error;
pub mod handler;
pub mod model;
pub mod providers;
pub mod router;
pub mod transform;

use axum::{Router, routing::post};

use crate::app_state::SharedState;

/// 构建 AI Gateway 子路由（state 由父 Router 提供）。
pub fn router() -> Router<SharedState> {
    Router::new().route("/v1/responses", post(handler::handle_responses))
}
