use std::path::PathBuf;

use anyhow::{Result, anyhow};
use tokio::sync::mpsc;

use crate::{
    app_state::SharedState,
    chain_log,
    im::{
        core::accounts::ImApiRegistry,
        telegram::{adapter::TelegramAdapter, api::TelegramApi},
        wechat::{adapter::WechatAdapter, api::WechatApi},
    },
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
    api_registry: ImApiRegistry,
    mut receiver: ImOutboundReceiver,
) {
    while let Some(message) = receiver.receiver.recv().await {
        log_outbound_message("worker_dequeue", &message, None);
        if !outbound_channel_enabled(&state, &message.route).await {
            state
                .push_event(
                    "warn",
                    "im_outbound_account_disabled",
                    format!(
                        "platform={} account={} thread={} chat={}",
                        message.route.platform.key(),
                        message.route.account_id,
                        message.thread_id,
                        message.route.chat_id
                    ),
                )
                .await;
            continue;
        }
        match message.route.platform {
            ImPlatformKind::Telegram => {
                let Some(api) = api_registry.telegram_for_route(&message.route) else {
                    log_missing_api(&state, &message).await;
                    continue;
                };
                send_telegram_outbound(&state, &api, message).await;
            }
            ImPlatformKind::Wechat => {
                let Some(api) = api_registry.wechat_for_route(&message.route) else {
                    log_missing_api(&state, &message).await;
                    continue;
                };
                send_wechat_outbound(&state, &api, message).await;
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

async fn outbound_channel_enabled(state: &SharedState, route: &RouteTarget) -> bool {
    let config = state.config.lock().await;
    match route.platform {
        ImPlatformKind::Feishu => config
            .feishu_account(&route.account_id)
            .is_some_and(|account| account.is_active()),
        ImPlatformKind::Telegram => config
            .telegram_account(&route.account_id)
            .is_some_and(|account| account.is_active()),
        ImPlatformKind::Wechat => config
            .wechat_account(&route.account_id)
            .is_some_and(|account| account.is_active()),
    }
}

async fn log_missing_api(state: &SharedState, message: &ImOutboundMessage) {
    state
        .push_event(
            "error",
            "im_outbound_api_missing",
            format!(
                "platform={} account={} thread={} chat={}",
                message.route.platform.key(),
                message.route.account_id,
                message.thread_id,
                message.route.chat_id
            ),
        )
        .await;
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
    log_outbound_message("send_wechat_text_begin", message, Some(text));
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
            log_outbound_result("send_wechat_text_done", message, &message_id);
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
            log_outbound_result("send_wechat_text_failed", message, &err.to_string());
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
    log_outbound_message("send_telegram_text_begin", message, Some(text));
    match adapter.send_text(&message.route.chat_id, text).await {
        Ok(message_id) => {
            log_outbound_result("send_telegram_text_done", message, &message_id);
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
            log_outbound_result("send_telegram_text_failed", message, &err.to_string());
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

fn log_outbound_message(event: &str, message: &ImOutboundMessage, text: Option<&str>) {
    let (payload_kind, text_len, preview) = match (&message.payload, text) {
        (_, Some(text)) => ("text", text.chars().count(), trace_preview(text, 500)),
        (ImOutboundPayload::Text(text), None) => {
            ("text", text.chars().count(), trace_preview(text, 500))
        }
        (
            ImOutboundPayload::Image {
                path,
                caption,
                fallback_text,
            },
            None,
        ) => {
            let image_text = format!(
                "path={} caption={} fallback={}",
                path.display(),
                caption.as_deref().unwrap_or(""),
                fallback_text.as_deref().unwrap_or("")
            );
            (
                "image",
                image_text.chars().count(),
                trace_preview(&image_text, 500),
            )
        }
    };
    chain_log::write_diagnostic_line(format!(
        "[im_trace] event=remote_to_im_outbound_{} platform={} account={} chat={} thread={} item={} type={} kind={:?} payload={} text_len={} preview={}",
        event,
        message.route.platform.key(),
        message.route.account_id,
        message.route.chat_id,
        message.thread_id,
        message.item_id.as_deref().unwrap_or(""),
        message.item_type.as_deref().unwrap_or(""),
        message.kind,
        payload_kind,
        text_len,
        preview
    ));
}

fn log_outbound_result(event: &str, message: &ImOutboundMessage, result: &str) {
    chain_log::write_diagnostic_line(format!(
        "[im_trace] event=remote_to_im_outbound_{} platform={} account={} chat={} thread={} item={} type={} kind={:?} result={}",
        event,
        message.route.platform.key(),
        message.route.account_id,
        message.route.chat_id,
        message.thread_id,
        message.item_id.as_deref().unwrap_or(""),
        message.item_type.as_deref().unwrap_or(""),
        message.kind,
        trace_preview(result, 300)
    ));
}

fn trace_preview(text: &str, limit: usize) -> String {
    let compact = text.replace("\r\n", "\n").replace('\n', "\\n");
    let mut out = String::new();
    for ch in compact.chars().take(limit) {
        out.push(ch);
    }
    if compact.chars().count() > limit {
        out.push_str("...");
    }
    out
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
