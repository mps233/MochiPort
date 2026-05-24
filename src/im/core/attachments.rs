use std::fs;
use std::path::{Path, PathBuf};

use tauri::AppHandle;
use uuid::Uuid;

use super::settings::resolve_workspace_cwd;
use super::types::ImChannelKind;
use super::InboundAttachment;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImAttachmentKind {
    Image,
    File,
    Video,
}

impl ImAttachmentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Image => "images",
            Self::File => "files",
            Self::Video => "videos",
        }
    }
}

pub async fn im_attachment_root<R: tauri::Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    let workspace = resolve_workspace_cwd(app)
        .await
        .ok_or_else(|| "workspace_missing_bug".to_string())?;
    let dir = PathBuf::from(workspace).join(".im").join("attachments");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

pub async fn im_attachment_dir<R: tauri::Runtime>(
    app: &AppHandle<R>,
    channel: ImChannelKind,
    kind: ImAttachmentKind,
) -> Result<PathBuf, String> {
    let dir = im_attachment_root(app)
        .await?
        .join(channel.as_str())
        .join(kind.as_str());
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

pub async fn allocate_attachment_path<R: tauri::Runtime>(
    app: &AppHandle<R>,
    channel: ImChannelKind,
    kind: ImAttachmentKind,
    preferred_name: &str,
) -> Result<PathBuf, String> {
    let dir = im_attachment_dir(app, channel, kind).await?;
    let file_name = build_attachment_file_name(preferred_name);
    Ok(dir.join(file_name))
}

pub async fn persist_inbound_attachment_bytes<R: tauri::Runtime>(
    app: &AppHandle<R>,
    channel: ImChannelKind,
    kind: ImAttachmentKind,
    preferred_name: &str,
    bytes: &[u8],
    mime_type: Option<String>,
    attachment_kind: &str,
    display_name: Option<String>,
) -> Result<InboundAttachment, String> {
    let path = allocate_attachment_path(app, channel, kind, preferred_name).await?;
    fs::write(&path, bytes).map_err(|e| e.to_string())?;
    Ok(InboundAttachment {
        kind: attachment_kind.to_string(),
        name: display_name.or_else(|| {
            path.file_name()
                .and_then(|v| v.to_str())
                .map(|v| v.to_string())
        }),
        mime_type,
        text_hint: None,
        local_path: Some(path.to_string_lossy().to_string()),
    })
}

pub fn inbound_attachment_from_local_path(
    attachment_kind: &str,
    local_path: &Path,
    display_name: Option<String>,
    mime_type: Option<String>,
) -> InboundAttachment {
    InboundAttachment {
        kind: attachment_kind.to_string(),
        name: display_name.or_else(|| {
            local_path
                .file_name()
                .and_then(|v| v.to_str())
                .map(|v| v.to_string())
        }),
        mime_type,
        text_hint: None,
        local_path: Some(local_path.to_string_lossy().to_string()),
    }
}

fn build_attachment_file_name(preferred_name: &str) -> String {
    let source = preferred_name.trim();
    let sanitized = sanitize_file_name(if source.is_empty() {
        "attachment.bin"
    } else {
        source
    });
    let stem = Path::new(&sanitized)
        .file_stem()
        .and_then(|v| v.to_str())
        .filter(|v| !v.trim().is_empty())
        .unwrap_or("attachment");
    let ext = Path::new(&sanitized)
        .extension()
        .and_then(|v| v.to_str())
        .filter(|v| !v.trim().is_empty());
    match ext {
        Some(ext) => format!("{}-{}.{}", stem, Uuid::new_v4().simple(), ext),
        None => format!("{}-{}", stem, Uuid::new_v4().simple()),
    }
}

fn sanitize_file_name(name: &str) -> String {
    name.chars()
        .map(|ch| match ch {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect()
}
