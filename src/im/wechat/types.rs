use serde::Deserialize;

pub const DEFAULT_WECHAT_API_BASE: &str = "https://ilinkai.weixin.qq.com";
pub const DEFAULT_WECHAT_BOT_TYPE: &str = "3";

#[derive(Debug, Clone, Default)]
pub struct WechatSettings {
    pub account_id: String,
    pub bot_token: String,
    pub base_url: String,
    pub bot_type: String,
    pub allowed_user_ids: Vec<String>,
}

impl WechatSettings {
    pub fn from_app_config(config: &crate::config::WechatConfig) -> Self {
        Self {
            account_id: config.account_id.clone(),
            bot_token: config.bot_token.clone(),
            base_url: config.base_url.clone(),
            bot_type: config.bot_type.clone(),
            allowed_user_ids: config.allowed_user_ids.clone(),
        }
    }

    pub fn is_configured(&self) -> bool {
        !self.bot_token.trim().is_empty()
    }

    pub fn account_id(&self) -> String {
        non_empty(&self.account_id).unwrap_or_else(|| "wechat".to_string())
    }

    pub fn api_base_url(&self) -> String {
        non_empty(&self.base_url).unwrap_or_else(|| DEFAULT_WECHAT_API_BASE.to_string())
    }

    pub fn bot_type(&self) -> String {
        non_empty(&self.bot_type).unwrap_or_else(|| DEFAULT_WECHAT_BOT_TYPE.to_string())
    }

    pub fn token(&self) -> Option<&str> {
        let token = self.bot_token.trim();
        (!token.is_empty()).then_some(token)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct WechatQrCodeResponse {
    pub qrcode: String,
    pub qrcode_img_content: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WechatQrStatusResponse {
    pub status: String,
    pub bot_token: Option<String>,
    pub ilink_bot_id: Option<String>,
    pub baseurl: Option<String>,
    pub ilink_user_id: Option<String>,
    pub redirect_host: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct WechatGetUpdatesResponse {
    pub ret: Option<i64>,
    pub errcode: Option<i64>,
    pub errmsg: Option<String>,
    pub msgs: Option<Vec<WechatMessage>>,
    pub get_updates_buf: Option<String>,
    pub longpolling_timeout_ms: Option<u64>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct WechatMessage {
    pub seq: Option<i64>,
    pub message_id: Option<i64>,
    pub from_user_id: Option<String>,
    pub to_user_id: Option<String>,
    pub client_id: Option<String>,
    pub create_time_ms: Option<u64>,
    pub session_id: Option<String>,
    pub group_id: Option<String>,
    pub message_type: Option<i64>,
    pub message_state: Option<i64>,
    pub item_list: Option<Vec<WechatMessageItem>>,
    pub context_token: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WechatMessageItem {
    #[serde(rename = "type")]
    pub item_type: Option<i64>,
    pub text_item: Option<WechatTextItem>,
    pub voice_item: Option<WechatVoiceItem>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WechatTextItem {
    pub text: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WechatVoiceItem {
    pub text: Option<String>,
}

fn non_empty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}
