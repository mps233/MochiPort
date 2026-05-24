use codex_app_server_protocol::JSONRPCNotification;
use codex_app_server_protocol::RequestId;
use serde_json::Value as JsonValue;
use tauri::{AppHandle, Emitter, Manager};

use super::{routing::ImRouteTarget, ImChannelKind, PendingApprovalRequest};

fn approval_kind_label(kind: &str) -> &str {
    match kind {
        "command" => "命令执行",
        "fileChange" => "文件修改",
        "review" => "补丁审查",
        other => other,
    }
}

fn concise_command(params: &JsonValue) -> String {
    params
        .get("command")
        .and_then(|v| {
            if let Some(text) = v.as_str() {
                Some(text.to_string())
            } else {
                v.as_array().map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str())
                        .collect::<Vec<_>>()
                        .join(" ")
                })
            }
        })
        .unwrap_or_default()
}

pub fn summarize_approval_request(method: &str, params: &JsonValue) -> Option<(String, String)> {
    match method {
        "item/commandExecution/requestApproval" => {
            let command = concise_command(params);
            let cwd = params.get("cwd").and_then(|v| v.as_str()).unwrap_or("");
            Some((
                "command".to_string(),
                format!("命令：`{command}`\n目录：`{cwd}`"),
            ))
        }
        "item/fileChange/requestApproval" => {
            let reason = params.get("reason").and_then(|v| v.as_str()).unwrap_or("");
            let item_id = params.get("itemId").and_then(|v| v.as_str()).unwrap_or("");
            Some((
                "fileChange".to_string(),
                format!("项目：`{item_id}`\n原因：{reason}"),
            ))
        }
        "applyPatchApproval" => {
            let reason = params.get("reason").and_then(|v| v.as_str()).unwrap_or("");
            Some(("review".to_string(), format!("原因：{reason}")))
        }
        "execCommandApproval" => {
            let command = concise_command(params);
            let cwd = params.get("cwd").and_then(|v| v.as_str()).unwrap_or("");
            Some((
                "review".to_string(),
                format!("命令：`{command}`\n目录：`{cwd}`"),
            ))
        }
        _ => None,
    }
}

pub fn approval_prompt(kind: &str, summary: &str) -> String {
    format!(
        "审批请求：{}\n{}\n\n请回复 /Y 或 /N",
        approval_kind_label(kind),
        summary
    )
}

pub fn pending_approval_from_notification(
    method: &str,
    request_id: &str,
    params: &JsonValue,
) -> Option<PendingApprovalRequest> {
    let (request_kind, summary) = summarize_approval_request(method, params)?;
    Some(PendingApprovalRequest {
        request_id: request_id.to_string(),
        request_kind,
        summary,
    })
}

pub fn approval_thread_id(params: &JsonValue) -> Option<String> {
    params
        .get("threadId")
        .and_then(|v| v.as_str())
        .or_else(|| params.get("conversationId").and_then(|v| v.as_str()))
        .map(|value| value.to_string())
        .filter(|value| !value.trim().is_empty())
}

pub fn approval_notice_text(marker: &str, pending: &PendingApprovalRequest) -> String {
    format!(
        "{}\n{}",
        marker,
        approval_prompt(&pending.request_kind, &pending.summary)
    )
}

pub async fn resolve_approval_route<R: tauri::Runtime>(
    app: &AppHandle<R>,
    channel: ImChannelKind,
    params: &JsonValue,
) -> Option<(String, ImRouteTarget)> {
    let thread_id = approval_thread_id(params)?;
    let route = super::resolve_route_target_by_thread(app, channel, &thread_id).await?;
    Some((thread_id, route))
}

pub fn resolve_approval_decision(
    pending: &PendingApprovalRequest,
    approved: bool,
) -> (JsonValue, &'static str, &'static str) {
    let response = if approved {
        if pending.request_kind == "review" {
            serde_json::json!({ "decision": "approved" })
        } else {
            serde_json::json!({ "decision": "accept" })
        }
    } else if pending.request_kind == "review" {
        serde_json::json!({ "decision": "denied" })
    } else {
        serde_json::json!({ "decision": "decline" })
    };
    let event_decision = if approved { "accept" } else { "decline" };
    let ack_text = if approved {
        "已批准。"
    } else {
        "已拒绝。"
    };
    (response, event_decision, ack_text)
}

pub async fn respond_to_codex_request<R: tauri::Runtime>(
    app: &AppHandle<R>,
    request_id: &str,
    response: JsonValue,
) -> Result<(), String> {
    let codex = app
        .try_state::<crate::codex_integration::CodexState>()
        .ok_or_else(|| "codex_not_initialized".to_string())?;
    let request_id_obj = if let Ok(id) = request_id.parse::<i64>() {
        RequestId::Integer(id)
    } else {
        RequestId::String(request_id.to_string())
    };
    match codex.send_response(request_id_obj, response).await {
        Ok(()) => {
            let _ = app.emit(
                "codex:notification",
                &JSONRPCNotification {
                    method: "serverRequest/resolved".to_string(),
                    params: Some(serde_json::json!({
                        "requestId": request_id,
                    })),
                },
            );
            Ok(())
        }
        Err(err) => Err(err.to_string()),
    }
}
