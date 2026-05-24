use serde_json::Value as JsonValue;
use tauri::{AppHandle, Manager};

use super::control::{handle_control_message, resolve_desktop_thread_settings, ImControlOutcome};
use super::session::{resolve_binding_by_thread, resolve_thread_id_for_chat};
use super::settings::resolve_workspace_cwd;
use super::thread::bind_inbound_session;
use super::types::{
    ImChannelKind, ImCodexUserMessageInput, ImDesktopAttachmentInput, InboundMessage,
};
use super::ImCommonSettings;

#[derive(Debug, Clone)]
pub enum ImInboundDispatchOutcome {
    ControlReply { text: String },
    ThreadRoutingRequested,
    TurnStarted,
}

#[derive(Debug, Clone)]
pub struct ImOutboundTextRoute {
    pub dedupe_key: String,
    pub account_id: String,
    pub chat_id: String,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct ImRouteTarget {
    pub dedupe_key: String,
    pub account_id: String,
    pub chat_id: String,
    pub sender_id: String,
}

pub fn extract_user_message_text(item: &JsonValue) -> Option<String> {
    if item.get("type").and_then(|v| v.as_str()) != Some("userMessage") {
        return None;
    }
    item.get("content")
        .and_then(|content| content.as_array())
        .and_then(|items| {
            let mut text = String::new();
            for entry in items {
                if entry.get("type").and_then(|v| v.as_str()) == Some("text") {
                    if let Some(value) = entry.get("text").and_then(|v| v.as_str()) {
                        text.push_str(value);
                    }
                }
            }
            let text = text.trim().to_string();
            if text.is_empty() {
                None
            } else {
                Some(text)
            }
        })
}

pub fn extract_user_message_input(
    item: &JsonValue,
) -> Option<(Option<String>, Vec<ImDesktopAttachmentInput>)> {
    if item.get("type").and_then(|v| v.as_str()) != Some("userMessage") {
        return None;
    }
    let content = item.get("content").and_then(|v| v.as_array())?;
    let mut text = String::new();
    let mut attachments = Vec::new();

    for entry in content {
        match entry.get("type").and_then(|v| v.as_str()) {
            Some("text") => {
                if let Some(value) = entry.get("text").and_then(|v| v.as_str()) {
                    text.push_str(value);
                }
            }
            Some("localImage") => {
                let path = entry
                    .get("path")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|v| !v.is_empty())
                    .map(ToString::to_string);
                if let Some(local_path) = path {
                    attachments.push(ImDesktopAttachmentInput {
                        kind: "image".to_string(),
                        name: None,
                        mime_type: None,
                        local_path: Some(local_path),
                        data_url: None,
                    });
                }
            }
            Some("image") => {
                if let Some(url) = entry
                    .get("url")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|v| !v.is_empty())
                {
                    attachments.push(ImDesktopAttachmentInput {
                        kind: "image".to_string(),
                        name: None,
                        mime_type: None,
                        local_path: None,
                        data_url: Some(url.to_string()),
                    });
                }
            }
            _ => {}
        }
    }

    let text = text.trim().to_string();
    Some(((!text.is_empty()).then_some(text), attachments))
}

pub fn extract_agent_message_text(item: &JsonValue) -> Option<String> {
    if item.get("type").and_then(|v| v.as_str()) != Some("agentMessage") {
        return None;
    }
    item.get("text")
        .and_then(|v| v.as_str())
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
        .or_else(|| {
            item.get("content")
                .and_then(|content| content.as_array())
                .and_then(|items| {
                    items.iter().find_map(|entry| {
                        entry
                            .get("text")
                            .and_then(|v| v.as_str())
                            .map(|text| text.trim().to_string())
                            .filter(|text| !text.is_empty())
                    })
                })
        })
}

pub fn extract_image_view_path(item: &JsonValue) -> Option<&str> {
    if item.get("type").and_then(|v| v.as_str()) != Some("imageView") {
        return None;
    }
    item.get("path")
        .and_then(|v| v.as_str())
        .filter(|v| !v.trim().is_empty())
}

pub fn extract_turn_reply_text(params: &JsonValue) -> Option<String> {
    params
        .get("turn")
        .and_then(|turn| turn.get("items"))
        .and_then(|items| items.as_array())
        .and_then(|items| items.iter().rev().find_map(extract_agent_message_text))
        .map(|text: String| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

fn current_desktop_thread_and_cwd<R: tauri::Runtime>(
    app: &AppHandle<R>,
) -> impl std::future::Future<Output = (Option<String>, Option<String>)> + '_ {
    async move {
        let inner = super::desktop_binding_snapshot(app).await;
        (inner.thread_id, inner.cwd)
    }
}

pub async fn routed_thread_has_active_desktop_turn<R: tauri::Runtime>(
    app: &AppHandle<R>,
    thread_id: &str,
) -> bool {
    let Some(active_desktop_thread_id) = super::active_desktop_thread_id(app).await else {
        return false;
    };
    if thread_id != active_desktop_thread_id {
        return false;
    }
    super::active_desktop_turn_busy(app).await
}

pub async fn resolve_inbound_target_thread<R: tauri::Runtime>(
    app: &AppHandle<R>,
    channel: ImChannelKind,
    account_id: &str,
    chat_id: &str,
) -> Result<String, String> {
    if let Some(bound_thread_id) =
        resolve_thread_id_for_chat(app, channel, account_id, chat_id).await
    {
        return Ok(bound_thread_id);
    }

    let (active_desktop_thread_id, _) = current_desktop_thread_and_cwd(app).await;
    if let Some(active_thread_id) = active_desktop_thread_id.as_deref() {
        if super::thread_has_in_progress_turn(app, active_thread_id).await {
            return Err("desktop_thread_busy".to_string());
        }
    }
    active_desktop_thread_id.ok_or_else(|| "no_active_desktop_thread".to_string())
}

fn inbound_attachment_inputs(message: &InboundMessage) -> Vec<JsonValue> {
    let mut inputs = Vec::new();
    for attachment in &message.attachments {
        let Some(local_path) = attachment
            .local_path
            .as_ref()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        match attachment.kind.as_str() {
            "image" => inputs.push(serde_json::json!({
                "type": "localImage",
                "path": local_path,
            })),
            "file" | "text" | "video" => inputs.push(serde_json::json!({
                "type": "text",
                "text": format!("File: {}", local_path),
            })),
            _ => {}
        }
    }
    inputs
}

pub async fn dispatch_inbound_message<R: tauri::Runtime>(
    app: &AppHandle<R>,
    message: &InboundMessage,
    origin: &str,
) -> Result<ImInboundDispatchOutcome, String> {
    if let Some(outcome) = handle_control_message(
        app,
        message.channel,
        &message.account_id,
        &message.chat_id,
        &message.sender_id,
        &message.message_id,
        &message.text,
    )
    .await?
    {
        return match outcome {
            ImControlOutcome::Reply { text } => Ok(ImInboundDispatchOutcome::ControlReply { text }),
            ImControlOutcome::RequestThreadRouting => {
                Ok(ImInboundDispatchOutcome::ThreadRoutingRequested)
            }
        };
    }

    let conversation_key = message.conversation_key();
    if !message.attachments.is_empty() {
        super::stage_inbound_attachments(app, &conversation_key, message.attachments.clone()).await;
        if message.text.trim().is_empty() {
            return Ok(ImInboundDispatchOutcome::ControlReply {
                text: super::inbound_attachment_notice(&message.attachments),
            });
        }
    }

    let thread_id =
        resolve_inbound_target_thread(app, message.channel, &message.account_id, &message.chat_id)
            .await?;
    if routed_thread_has_active_desktop_turn(app, &thread_id).await {
        return Err("desktop_thread_busy".to_string());
    }
    let settings = resolve_desktop_thread_settings(app).await?;
    let mut merged = super::take_staged_inbound_attachments(app, &conversation_key).await;
    if merged.is_empty() {
        merged = message.attachments.clone();
    }
    let extra_inputs = inbound_attachment_inputs(&InboundMessage {
        attachments: merged,
        ..message.clone()
    });
    start_turn_with_settings(
        app,
        &thread_id,
        &settings,
        &message.text,
        Some(origin),
        extra_inputs,
        None,
    )
    .await?;

    bind_inbound_session(
        app,
        message.channel,
        &message.account_id,
        &message.sender_id,
        &message.chat_id,
        &message.message_id,
        &message.text,
        &thread_id,
    )
    .await;

    super::set_remote_binding(
        app,
        Some(message.channel),
        Some(message.account_id.clone()),
        Some(message.chat_id.clone()),
        Some(message.sender_id.clone()),
    )
    .await;

    Ok(ImInboundDispatchOutcome::TurnStarted)
}

fn turn_sandbox_policy_for_settings(
    cwd: Option<&str>,
    approval_policy: Option<&str>,
    sandbox_mode: Option<&str>,
) -> JsonValue {
    match sandbox_mode.unwrap_or_default() {
        "danger-full-access" => serde_json::json!({ "type": "dangerFullAccess" }),
        "workspace-write" => {
            if let Some(cwd) = cwd.filter(|value| !value.trim().is_empty()) {
                serde_json::json!({
                    "type": "workspaceWrite",
                    "writableRoots": [cwd],
                    "readOnlyAccess": { "type": "fullAccess" },
                    "networkAccess": true,
                    "excludeTmpdirEnvVar": false,
                    "excludeSlashTmp": false
                })
            } else {
                serde_json::json!({ "type": "workspaceWrite" })
            }
        }
        _ => match approval_policy.unwrap_or_default() {
            "never" => serde_json::json!({ "type": "dangerFullAccess" }),
            "on-request" | "untrusted" => {
                if let Some(cwd) = cwd.filter(|value| !value.trim().is_empty()) {
                    serde_json::json!({
                        "type": "workspaceWrite",
                        "writableRoots": [cwd],
                        "readOnlyAccess": { "type": "fullAccess" },
                        "networkAccess": true,
                        "excludeTmpdirEnvVar": false,
                        "excludeSlashTmp": false
                    })
                } else {
                    serde_json::json!({ "type": "workspaceWrite" })
                }
            }
            _ => serde_json::json!({ "type": "readOnly" }),
        },
    }
}

pub async fn start_turn_with_settings<R: tauri::Runtime>(
    app: &AppHandle<R>,
    thread_id: &str,
    settings: &ImCommonSettings,
    text: &str,
    origin: Option<&str>,
    extra_inputs: Vec<JsonValue>,
    collaboration_mode: Option<JsonValue>,
) -> Result<Option<String>, String> {
    let resolved_cwd = resolve_workspace_cwd(app).await;
    let sandbox_policy = turn_sandbox_policy_for_settings(
        resolved_cwd.as_deref(),
        settings.approval_policy.as_deref(),
        settings.sandbox_mode.as_deref(),
    );
    let codex = app
        .try_state::<crate::codex_integration::CodexState>()
        .ok_or_else(|| "codex_not_initialized".to_string())?;
    let mut input_items = vec![serde_json::json!({
        "type": "text",
        "text": text,
        "text_elements": []
    })];
    input_items.extend(extra_inputs);
    let mut request = serde_json::json!({
        "threadId": thread_id,
        "cwd": resolved_cwd,
        "approvalPolicy": settings.approval_policy.clone(),
        "approvalsReviewer": settings.approvals_reviewer.clone(),
        "sandboxPolicy": sandbox_policy,
        "model": settings.model.clone(),
        "effort": settings.reasoning_effort.clone(),
        "input": input_items
    });
    if let Some(collaboration_mode) = collaboration_mode.as_ref() {
        request["collaborationMode"] = collaboration_mode.clone();
    }
    if let Some(origin) = origin.filter(|v| !v.trim().is_empty()) {
        request["metadata"] = serde_json::json!({
            "arthasOrigin": origin
        });
    }
    let response = codex
        .send_request("turn/start".to_string(), Some(request))
        .await
        .map_err(|e| e.to_string())?;
    let started_turn_id = response
        .get("turn")
        .and_then(|v| v.get("id"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    if let Some(turn_id) = started_turn_id.as_deref() {
        if let Some(origin) = origin.filter(|v| !v.trim().is_empty()) {
            crate::im::core::note_turn_origin(app, turn_id, origin).await;
        }
        if let Some(mode) = collaboration_mode
            .as_ref()
            .and_then(|value| value.get("mode"))
            .and_then(|value| value.as_str())
        {
            crate::im::core::note_turn_mode(app, turn_id, mode).await;
        }
        crate::im::core::mark_turn_started(app, thread_id, turn_id).await;
        if crate::im::core::is_active_desktop_thread(app, thread_id).await {
            let desktop = crate::im::core::desktop_binding_snapshot(app).await;
            crate::im::core::thread::bind_desktop_thread(
                app,
                Some(thread_id.to_string()),
                desktop.thread_name,
                desktop.cwd,
                desktop.pending_plan_implement_turn_id,
                desktop.selected_model,
                desktop.selected_effort,
            )
            .await;
        }
    }
    Ok(started_turn_id)
}

pub async fn start_plan_implementation_turn<R: tauri::Runtime>(
    app: &AppHandle<R>,
    thread_id: &str,
    origin: Option<&str>,
) -> Result<Option<String>, String> {
    let settings = super::control::resolve_desktop_thread_settings(app).await?;
    let mut request_settings = settings.clone();
    request_settings.route_tag = None;
    let collaboration_mode = serde_json::json!({
        "mode": "default",
        "settings": {
            "model": request_settings
                .model
                .clone()
                .unwrap_or_else(|| "gpt-5.2-codex".to_string()),
            "reasoning_effort": request_settings.reasoning_effort.clone(),
            "developer_instructions": serde_json::Value::Null
        }
    });
    start_turn_with_settings(
        app,
        thread_id,
        &request_settings,
        "Implement the plan.",
        origin,
        Vec::new(),
        Some(collaboration_mode),
    )
    .await
}

pub async fn route_codex_text_notification<R: tauri::Runtime>(
    app: &AppHandle<R>,
    channel: ImChannelKind,
    method: &str,
    params: &JsonValue,
) -> Option<ImOutboundTextRoute> {
    let thread_id = params.get("threadId").and_then(|v| v.as_str())?;
    let binding = resolve_binding_by_thread(app, channel, thread_id).await?;
    let text = match method {
        "item/completed" => params
            .get("item")
            .and_then(extract_agent_message_text)
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty())?,
        "turn/completed" | "codex/event/turn_completed" => extract_turn_reply_text(params)?,
        _ => return None,
    };
    Some(ImOutboundTextRoute {
        dedupe_key: binding.conversation_key,
        account_id: binding.account_id,
        chat_id: binding.chat_id,
        text,
    })
}

pub async fn route_codex_user_message_notification<R: tauri::Runtime>(
    app: &AppHandle<R>,
    channel: ImChannelKind,
    params: &JsonValue,
) -> Option<ImCodexUserMessageInput> {
    let thread_id = params.get("threadId").and_then(|v| v.as_str())?;
    let turn_id = params.get("turnId").and_then(|v| v.as_str());
    let origin = super::runtime::resolve_turn_origin(app, thread_id, turn_id).await?;
    if origin != "desktop" {
        return None;
    }
    if resolve_binding_by_thread(app, channel, thread_id)
        .await
        .is_none()
    {
        return None;
    }
    let (text, attachments) = params.get("item").and_then(extract_user_message_input)?;
    if text.is_none() && attachments.is_empty() {
        return None;
    }
    Some(ImCodexUserMessageInput {
        thread_id: thread_id.to_string(),
        text,
        attachments,
    })
}

pub async fn resolve_route_target_by_thread<R: tauri::Runtime>(
    app: &AppHandle<R>,
    channel: ImChannelKind,
    thread_id: &str,
) -> Option<ImRouteTarget> {
    let binding = resolve_binding_by_thread(app, channel, thread_id).await?;
    Some(ImRouteTarget {
        dedupe_key: binding.conversation_key,
        account_id: binding.account_id,
        chat_id: binding.chat_id,
        sender_id: binding.sender_id,
    })
}
