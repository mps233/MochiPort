use std::collections::HashMap;

use tauri::{AppHandle, Manager};
use tokio::sync::Mutex;

use super::types::InboundAttachment;

#[derive(Default)]
pub struct ImMediaState {
    pub(crate) inner: Mutex<HashMap<String, Vec<InboundAttachment>>>,
}

pub async fn stage_inbound_attachments<R: tauri::Runtime>(
    app: &AppHandle<R>,
    conversation_key: &str,
    attachments: Vec<InboundAttachment>,
) {
    if attachments.is_empty() {
        return;
    }
    let Some(state) = app.try_state::<ImMediaState>() else {
        return;
    };
    let mut inner = state.inner.lock().await;
    inner
        .entry(conversation_key.to_string())
        .or_default()
        .extend(attachments);
}

pub async fn take_staged_inbound_attachments<R: tauri::Runtime>(
    app: &AppHandle<R>,
    conversation_key: &str,
) -> Vec<InboundAttachment> {
    let Some(state) = app.try_state::<ImMediaState>() else {
        return Vec::new();
    };
    let mut inner = state.inner.lock().await;
    inner.remove(conversation_key).unwrap_or_default()
}

pub fn inbound_attachment_notice(attachments: &[InboundAttachment]) -> String {
    if attachments.is_empty() {
        return "已收到附件。".to_string();
    }
    let has_image = attachments.iter().any(|item| item.kind == "image");
    let has_video = attachments.iter().any(|item| item.kind == "video");
    let has_file = attachments
        .iter()
        .any(|item| item.kind == "file" || item.kind == "text");
    match (has_image, has_file, has_video) {
        (true, false, false) => {
            "已收到图片。请继续发送文字描述，或继续补充图片；发送文字后我会一起处理。".to_string()
        }
        (false, true, false) => {
            "已收到文件。请继续发送文字描述；发送文字后我会结合文件一起处理。".to_string()
        }
        (false, false, true) => {
            "已收到视频。请继续发送文字描述；发送文字后我会结合视频文件路径一起处理。".to_string()
        }
        _ => "已收到附件。请继续发送文字描述；发送文字后我会一起处理。".to_string(),
    }
}
