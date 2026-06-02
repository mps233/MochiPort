use std::collections::HashMap;

use crate::{
    im::{feishu::FeishuApi, telegram::api::TelegramApi, wechat::api::WechatApi},
    im_runtime::RouteTarget,
    types::ImPlatformKind,
};

#[derive(Clone, Default)]
pub(crate) struct ImApiRegistry {
    pub feishu: HashMap<String, FeishuApi>,
    pub telegram: HashMap<String, TelegramApi>,
    pub wechat: HashMap<String, WechatApi>,
}

impl ImApiRegistry {
    pub(crate) fn feishu_for_route(&self, route: &RouteTarget) -> Option<FeishuApi> {
        (route.platform == ImPlatformKind::Feishu)
            .then(|| self.feishu.get(&route.account_id).cloned())
            .flatten()
    }

    pub(crate) fn telegram_for_route(&self, route: &RouteTarget) -> Option<TelegramApi> {
        (route.platform == ImPlatformKind::Telegram)
            .then(|| self.telegram.get(&route.account_id).cloned())
            .flatten()
    }

    pub(crate) fn wechat_for_route(&self, route: &RouteTarget) -> Option<WechatApi> {
        (route.platform == ImPlatformKind::Wechat)
            .then(|| self.wechat.get(&route.account_id).cloned())
            .flatten()
    }
}
