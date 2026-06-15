use super::config::{AiGatewayConfig, ProviderConfig};
use super::error::GatewayError;

/// 根据 model 名选择 provider，找不到则返回错误。
pub fn resolve_provider<'a>(
    model: &str,
    config: &'a AiGatewayConfig,
) -> Result<&'a ProviderConfig, GatewayError> {
    config
        .select_provider(model)
        .ok_or_else(|| GatewayError::invalid_model(model))
}
