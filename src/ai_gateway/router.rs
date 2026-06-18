use super::config::{AiGatewayConfig, ProviderConfig};
use super::error::GatewayError;

/// 根据 model 名选择 provider，找不到则返回错误。
pub fn resolve_provider<'a>(
    model: &str,
    session_id: Option<&str>,
    config: &'a AiGatewayConfig,
) -> Result<&'a ProviderConfig, GatewayError> {
    config
        .select_provider_for_session(model, session_id)
        .ok_or_else(|| GatewayError::invalid_model(model))
}
