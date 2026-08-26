#[derive(Debug, Clone, Default)]
pub struct TelegramSettings {
    pub account_id: String,
    pub bot_token: String,
    pub mention_only: bool,
    pub allowed_chat_ids: Vec<String>,
    pub project_groups: Vec<crate::config::TelegramProjectGroupConfig>,
}

impl TelegramSettings {
    pub fn from_app_config(config: &crate::config::TelegramConfig) -> Self {
        Self {
            account_id: config.account_id.clone(),
            bot_token: config.bot_token.clone(),
            mention_only: config.mention_only,
            allowed_chat_ids: config.allowed_chat_ids.clone(),
            project_groups: config.project_groups.clone(),
        }
    }

    pub fn is_configured(&self) -> bool {
        !self.bot_token.trim().is_empty()
    }

    pub fn account_id(&self) -> String {
        let account_id = self.account_id.trim();
        if account_id.is_empty() {
            "telegram".to_string()
        } else {
            account_id.to_string()
        }
    }

    pub fn project_group_for_chat(
        &self,
        chat_id: &str,
    ) -> Option<crate::config::TelegramProjectGroupConfig> {
        let chat_id = chat_id.trim();
        self.project_groups
            .iter()
            .find(|group| group.chat_id.trim() == chat_id && !group.cwd.trim().is_empty())
            .cloned()
    }
}
