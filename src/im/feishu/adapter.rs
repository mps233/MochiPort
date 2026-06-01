use anyhow::Result;

use crate::im::core::thread::ThreadCreateDefaults;
use crate::im_runtime::PendingApproval;

use super::{FeishuApi, renderer};

#[derive(Clone)]
pub struct FeishuAdapter {
    api: FeishuApi,
}

impl FeishuAdapter {
    pub fn new(api: FeishuApi) -> Self {
        Self { api }
    }

    pub async fn send_text(&self, target: &str, text: &str) -> Result<()> {
        if let Some(open_id) = target.strip_prefix("open_id:") {
            self.api
                .send_text_message_to("open_id", open_id, text)
                .await
        } else {
            self.api.send_text_message(target, text).await
        }
    }

    pub async fn send_interactive(&self, target: &str, card: &serde_json::Value) -> Result<String> {
        if let Some(open_id) = target.strip_prefix("open_id:") {
            self.api
                .send_interactive_message_to("open_id", open_id, card)
                .await
        } else {
            self.api.send_interactive_message(target, card).await
        }
    }

    pub async fn update_interactive(
        &self,
        message_id: &str,
        card: &serde_json::Value,
    ) -> Result<()> {
        self.api.update_interactive_message(message_id, card).await
    }

    pub async fn send_or_update_interactive(
        &self,
        target: &str,
        message_id: Option<&str>,
        card: &serde_json::Value,
    ) -> Result<String> {
        if let Some(message_id) = message_id {
            self.update_interactive(message_id, card).await?;
            Ok(message_id.to_string())
        } else {
            self.send_interactive(target, card).await
        }
    }

    pub async fn update_resolved_approval(
        &self,
        pending: &PendingApproval,
        option_index: usize,
        decision_label: &str,
    ) -> Result<()> {
        let Some(message_id) = pending.message_id.as_deref() else {
            return Ok(());
        };
        let card = renderer::build_resolved_approval_card(
            approval_kind_label(&pending.request_kind),
            &pending.summary,
            decision_label,
            option_index,
        );
        self.update_interactive(message_id, &card).await
    }

    pub async fn send_approval(&self, target: &str, approval: &PendingApproval) -> Result<String> {
        let request_key = approval.request_key();
        let card = renderer::build_approval_card(
            approval_kind_label(&approval.request_kind),
            &approval.summary,
            &approval.decisions,
            &request_key,
        );
        self.send_interactive(target, &card).await
    }

    pub async fn send_thread_routing_choice(
        &self,
        target: &str,
        request_id: &str,
        message_id: Option<&str>,
    ) -> Result<String> {
        let card = thread_routing_choice_card(request_id, None);
        self.send_or_update_interactive(target, message_id, &card)
            .await
    }

    pub async fn update_thread_routing_choice_selected(
        &self,
        request_id: &str,
        message_id: Option<&str>,
        selected_action: &str,
    ) -> Result<()> {
        let Some(message_id) = message_id else {
            return Ok(());
        };
        let card = thread_routing_choice_card(request_id, Some(selected_action));
        self.update_interactive(message_id, &card).await
    }

    pub async fn send_thread_list(
        &self,
        target: &str,
        request_id: &str,
        title: &str,
        body: &str,
        entries: &[renderer::FeishuThreadListEntry],
        page: usize,
        has_prev: bool,
        has_next: bool,
        message_id: Option<&str>,
    ) -> Result<String> {
        let card = renderer::build_thread_list_card(
            request_id, title, body, entries, page, has_prev, has_next,
        );
        self.send_or_update_interactive(target, message_id, &card)
            .await
    }

    pub async fn send_thread_create_settings(
        &self,
        target: &str,
        request_id: &str,
        defaults: &ThreadCreateDefaults,
        message_id: Option<&str>,
    ) -> Result<String> {
        let card = renderer::build_thread_create_settings_card(request_id, defaults);
        self.send_or_update_interactive(target, message_id, &card)
            .await
    }

    pub async fn send_thread_routing_result(
        &self,
        target: &str,
        title: &str,
        body: &str,
        message_id: Option<&str>,
    ) -> Result<String> {
        let card = renderer::build_thread_routing_result_card(title, body);
        self.send_or_update_interactive(target, message_id, &card)
            .await
    }

    pub async fn send_turn_completed(&self, target: &str, reply_text: &str) -> Result<String> {
        let card = renderer::build_turn_completed_card(reply_text);
        self.send_interactive(target, &card).await
    }
}

fn thread_routing_choice_card(
    request_id: &str,
    selected_action: Option<&str>,
) -> serde_json::Value {
    let resolved = selected_action.is_some();
    renderer::build_thread_routing_choice_card(
        "未绑定会话",
        "当前飞书会话没有可直接使用的活跃 Codex thread。请选择新建会话，或显式恢复一个历史会话。",
        &[
            renderer::FeishuThreadRoutingAction {
                label: "创建新会话".to_string(),
                description: "创建一个新的 Codex thread，并接入后续消息。".to_string(),
                value: serde_json::json!({
                    "kind": "thread_route_choice",
                    "requestId": request_id,
                    "action": "create_new"
                }),
                primary: true,
                selected: selected_action == Some("create_new"),
                resolved,
            },
            renderer::FeishuThreadRoutingAction {
                label: "恢复历史会话".to_string(),
                description: "查看 Codex App 当前可恢复的历史 thread 列表。".to_string(),
                value: serde_json::json!({
                    "kind": "thread_route_choice",
                    "requestId": request_id,
                    "action": "resume_history"
                }),
                primary: false,
                selected: selected_action == Some("resume_history"),
                resolved,
            },
        ],
    )
}

pub fn approval_kind_label(kind: &str) -> &'static str {
    match kind {
        "command" => "命令执行",
        "fileChange" => "文件修改",
        "review" => "补丁审查",
        _ => "操作审批",
    }
}
