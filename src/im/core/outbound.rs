use std::path::PathBuf;

use anyhow::{Result, anyhow};
use tokio::sync::mpsc;

use crate::{
    app_state::SharedState,
    im::telegram::{adapter::TelegramAdapter, api::TelegramApi},
    im::wechat::{adapter::WechatAdapter, api::WechatApi},
    im_runtime::RouteTarget,
    types::ImPlatformKind,
};

#[derive(Clone)]
pub(crate) struct ImOutboundSender {
    sender: mpsc::UnboundedSender<ImOutboundMessage>,
}

pub(crate) struct ImOutboundReceiver {
    receiver: mpsc::UnboundedReceiver<ImOutboundMessage>,
}

#[derive(Debug, Clone)]
pub(crate) struct ImOutboundMessage {
    pub thread_id: String,
    pub route: RouteTarget,
    pub item_id: Option<String>,
    pub item_type: Option<String>,
    pub kind: ImOutboundKind,
    pub payload: ImOutboundPayload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImOutboundKind {
    TurnReply,
    Item,
    ImageItem,
}

#[derive(Debug, Clone)]
pub(crate) enum ImOutboundPayload {
    Text(String),
    Image {
        path: PathBuf,
        caption: Option<String>,
        fallback_text: Option<String>,
    },
}

pub(crate) fn channel() -> (ImOutboundSender, ImOutboundReceiver) {
    let (sender, receiver) = mpsc::unbounded_channel();
    (ImOutboundSender { sender }, ImOutboundReceiver { receiver })
}

impl ImOutboundSender {
    pub(crate) fn enqueue(&self, message: ImOutboundMessage) -> Result<()> {
        self.sender
            .send(message)
            .map_err(|_| anyhow!("IM outbound queue is closed"))
    }
}

pub(crate) async fn run_worker(
    state: SharedState,
    telegram_api: TelegramApi,
    wechat_api: WechatApi,
    mut receiver: ImOutboundReceiver,
) {
    while let Some(message) = receiver.receiver.recv().await {
        if !outbound_channel_enabled(&state, message.route.platform).await {
            state
                .push_event(
                    "warn",
                    "im_outbound_channel_disabled",
                    format!(
                        "platform={} thread={} chat={}",
                        message.route.platform.key(),
                        message.thread_id,
                        message.route.chat_id
                    ),
                )
                .await;
            continue;
        }
        match message.route.platform {
            ImPlatformKind::Telegram => {
                send_telegram_outbound(&state, &telegram_api, message).await;
            }
            ImPlatformKind::Wechat => {
                send_wechat_outbound(&state, &wechat_api, message).await;
            }
            ImPlatformKind::Feishu => {
                state
                    .push_event(
                        "warn",
                        "im_outbound_unsupported",
                        format!(
                            "platform=feishu thread={} chat={} kind={:?}",
                            message.thread_id, message.route.chat_id, message.kind
                        ),
                    )
                    .await;
            }
        }
    }
    state
        .push_event(
            "warn",
            "im_outbound_worker_stopped",
            "outbound queue closed",
        )
        .await;
}

async fn outbound_channel_enabled(state: &SharedState, platform: ImPlatformKind) -> bool {
    let config = state.config.lock().await;
    match platform {
        ImPlatformKind::Feishu => config.feishu.enabled,
        ImPlatformKind::Telegram => config.telegram.enabled,
        ImPlatformKind::Wechat => config.wechat.enabled,
    }
}

async fn send_telegram_outbound(
    state: &SharedState,
    telegram_api: &TelegramApi,
    message: ImOutboundMessage,
) {
    let adapter = TelegramAdapter::new(telegram_api.clone());
    match &message.payload {
        ImOutboundPayload::Text(text) => {
            send_telegram_text(state, &adapter, &message, text).await;
        }
        ImOutboundPayload::Image {
            path,
            caption,
            fallback_text,
        } => {
            send_telegram_image(
                state,
                &adapter,
                &message,
                path.clone(),
                caption.as_deref(),
                fallback_text.as_deref(),
            )
            .await;
        }
    }
}

async fn send_wechat_outbound(
    state: &SharedState,
    wechat_api: &WechatApi,
    message: ImOutboundMessage,
) {
    let adapter = WechatAdapter::new(wechat_api.clone());
    match &message.payload {
        ImOutboundPayload::Text(text) => {
            send_wechat_text(state, &adapter, &message, text).await;
        }
        ImOutboundPayload::Image {
            path,
            caption,
            fallback_text,
        } => {
            send_wechat_image(
                state,
                &adapter,
                &message,
                path.clone(),
                caption.as_deref(),
                fallback_text.as_deref(),
            )
            .await;
        }
    }
}

async fn send_wechat_text(
    state: &SharedState,
    adapter: &WechatAdapter,
    message: &ImOutboundMessage,
    text: &str,
) {
    let event_begin = match message.kind {
        ImOutboundKind::TurnReply => "wechat_turn_send_begin",
        ImOutboundKind::Item | ImOutboundKind::ImageItem => "wechat_item_send_begin",
    };
    let event_done = match message.kind {
        ImOutboundKind::TurnReply => "wechat_turn_completed_sent",
        ImOutboundKind::Item | ImOutboundKind::ImageItem => "wechat_item_sent",
    };
    state
        .push_event(
            "info",
            event_begin,
            format!(
                "thread={} item={} type={} peer={} text_len={}",
                message.thread_id,
                message.item_id.as_deref().unwrap_or(""),
                message.item_type.as_deref().unwrap_or(""),
                message.route.chat_id,
                text.chars().count()
            ),
        )
        .await;
    match adapter
        .send_text(
            state,
            &message.route.account_id,
            &message.route.chat_id,
            text,
        )
        .await
    {
        Ok(message_id) => {
            state
                .push_event(
                    "info",
                    event_done,
                    format!(
                        "thread={} item={} type={} peer={} message={}",
                        message.thread_id,
                        message.item_id.as_deref().unwrap_or(""),
                        message.item_type.as_deref().unwrap_or(""),
                        message.route.chat_id,
                        message_id
                    ),
                )
                .await;
        }
        Err(err) => {
            let event_failed = match message.kind {
                ImOutboundKind::TurnReply => "wechat_turn_completed_failed",
                ImOutboundKind::Item | ImOutboundKind::ImageItem => "wechat_item_failed",
            };
            state
                .push_event(
                    "error",
                    event_failed,
                    format!(
                        "thread={} item={} type={} peer={} err={}",
                        message.thread_id,
                        message.item_id.as_deref().unwrap_or(""),
                        message.item_type.as_deref().unwrap_or(""),
                        message.route.chat_id,
                        err
                    ),
                )
                .await;
        }
    }
}

async fn send_wechat_image(
    state: &SharedState,
    adapter: &WechatAdapter,
    message: &ImOutboundMessage,
    path: PathBuf,
    caption: Option<&str>,
    fallback_text: Option<&str>,
) {
    state
        .push_event(
            "info",
            "wechat_image_send_begin",
            format!(
                "thread={} item={} type={} peer={} path={} caption_len={}",
                message.thread_id,
                message.item_id.as_deref().unwrap_or(""),
                message.item_type.as_deref().unwrap_or(""),
                message.route.chat_id,
                path.display(),
                caption.map(|value| value.chars().count()).unwrap_or(0)
            ),
        )
        .await;
    match adapter
        .send_image_path(
            state,
            &message.route.account_id,
            &message.route.chat_id,
            &path,
            caption,
            fallback_text,
        )
        .await
    {
        Ok(message_id) => {
            state
                .push_event(
                    "info",
                    "wechat_image_item_sent",
                    format!(
                        "thread={} item={} type={} peer={} message={}",
                        message.thread_id,
                        message.item_id.as_deref().unwrap_or(""),
                        message.item_type.as_deref().unwrap_or(""),
                        message.route.chat_id,
                        message_id
                    ),
                )
                .await;
        }
        Err(err) => {
            state
                .push_event(
                    "error",
                    "wechat_image_send_failed",
                    format!(
                        "thread={} item={} type={} path={} err={}",
                        message.thread_id,
                        message.item_id.as_deref().unwrap_or(""),
                        message.item_type.as_deref().unwrap_or(""),
                        path.display(),
                        err
                    ),
                )
                .await;
        }
    }
}

async fn send_telegram_text(
    state: &SharedState,
    adapter: &TelegramAdapter,
    message: &ImOutboundMessage,
    text: &str,
) {
    let event_begin = match message.kind {
        ImOutboundKind::TurnReply => "telegram_turn_send_begin",
        ImOutboundKind::Item | ImOutboundKind::ImageItem => "telegram_item_send_begin",
    };
    let event_done = match message.kind {
        ImOutboundKind::TurnReply => "telegram_turn_completed_sent",
        ImOutboundKind::Item | ImOutboundKind::ImageItem => "telegram_item_sent",
    };
    state
        .push_event(
            "info",
            event_begin,
            format!(
                "thread={} item={} type={} chat={} text_len={}",
                message.thread_id,
                message.item_id.as_deref().unwrap_or(""),
                message.item_type.as_deref().unwrap_or(""),
                message.route.chat_id,
                text.chars().count()
            ),
        )
        .await;
    match adapter.send_text(&message.route.chat_id, text).await {
        Ok(message_id) => {
            state
                .push_event(
                    "info",
                    event_done,
                    format!(
                        "thread={} item={} type={} chat={} message={}",
                        message.thread_id,
                        message.item_id.as_deref().unwrap_or(""),
                        message.item_type.as_deref().unwrap_or(""),
                        message.route.chat_id,
                        message_id
                    ),
                )
                .await;
        }
        Err(err) => {
            let event_failed = match message.kind {
                ImOutboundKind::TurnReply => "telegram_turn_completed_failed",
                ImOutboundKind::Item | ImOutboundKind::ImageItem => "telegram_item_failed",
            };
            state
                .push_event(
                    "error",
                    event_failed,
                    format!(
                        "thread={} item={} type={} chat={} err={}",
                        message.thread_id,
                        message.item_id.as_deref().unwrap_or(""),
                        message.item_type.as_deref().unwrap_or(""),
                        message.route.chat_id,
                        err
                    ),
                )
                .await;
        }
    }
}

async fn send_telegram_image(
    state: &SharedState,
    adapter: &TelegramAdapter,
    message: &ImOutboundMessage,
    path: PathBuf,
    caption: Option<&str>,
    fallback_text: Option<&str>,
) {
    state
        .push_event(
            "info",
            "telegram_image_send_begin",
            format!(
                "thread={} item={} type={} chat={} path={} caption_len={}",
                message.thread_id,
                message.item_id.as_deref().unwrap_or(""),
                message.item_type.as_deref().unwrap_or(""),
                message.route.chat_id,
                path.display(),
                caption.map(|value| value.chars().count()).unwrap_or(0)
            ),
        )
        .await;
    match adapter
        .send_image_path(&message.route.chat_id, &path, caption)
        .await
    {
        Ok(message_id) => {
            state
                .push_event(
                    "info",
                    "telegram_image_item_sent",
                    format!(
                        "thread={} item={} type={} chat={} message={}",
                        message.thread_id,
                        message.item_id.as_deref().unwrap_or(""),
                        message.item_type.as_deref().unwrap_or(""),
                        message.route.chat_id,
                        message_id
                    ),
                )
                .await;
        }
        Err(err) => {
            state
                .push_event(
                    "warn",
                    "telegram_image_send_failed",
                    format!(
                        "thread={} item={} type={} path={} err={}",
                        message.thread_id,
                        message.item_id.as_deref().unwrap_or(""),
                        message.item_type.as_deref().unwrap_or(""),
                        path.display(),
                        err
                    ),
                )
                .await;
            if let Some(fallback_text) = fallback_text {
                send_telegram_text(state, adapter, message, fallback_text).await;
            }
        }
    }
}
