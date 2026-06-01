#[derive(Debug, Clone, Default)]
pub struct TelegramSettings {
    pub bot_token: String,
    pub mention_only: bool,
    pub allowed_chat_ids: Vec<String>,
}

impl TelegramSettings {
    pub fn from_app_config(config: &crate::config::TelegramConfig) -> Self {
        Self {
            bot_token: config.bot_token.clone(),
            mention_only: config.mention_only,
            allowed_chat_ids: config.allowed_chat_ids.clone(),
        }
    }

    pub fn is_configured(&self) -> bool {
        !self.bot_token.trim().is_empty()
    }
}
