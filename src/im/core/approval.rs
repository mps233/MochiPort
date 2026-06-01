use anyhow::Result;

use crate::{
    app_state::SharedState,
    codex::{approval_decision_by_input, approval_response},
    im::core::thread::approval_reply_hint,
    im_runtime::{ApprovalDecisionOption, PendingApproval},
    remote_control_backend,
    types::InboundMessage,
};

pub(crate) enum ApprovalReplyOutcome {
    Ready {
        conversation_key: String,
        pending: PendingApproval,
        option_index: usize,
        decision: ApprovalDecisionOption,
    },
    NoPending,
    NotCurrent,
    InvalidInput {
        hint: String,
    },
}

pub(crate) async fn resolve_approval_reply(
    state: &SharedState,
    message: &InboundMessage,
    command: &str,
) -> ApprovalReplyOutcome {
    let message_conversation_key = message.conversation_key();
    let pending = {
        let runtime = state.runtime.lock().await;
        if let Some(request_key) = message.approval_request_key.as_deref() {
            runtime.approval_by_request_key_anywhere(request_key)
        } else {
            runtime
                .current_approval(&message_conversation_key)
                .map(|pending| (message_conversation_key.clone(), pending))
        }
    };
    let Some((conversation_key, pending)) = pending else {
        return ApprovalReplyOutcome::NoPending;
    };
    if let Some(request_key) = message.approval_request_key.as_deref() {
        let is_current = state
            .runtime
            .lock()
            .await
            .is_current_approval(&conversation_key, request_key);
        if !is_current {
            return ApprovalReplyOutcome::NotCurrent;
        }
    }
    let Some((option_index, decision)) = approval_decision_by_input(&pending, command) else {
        return ApprovalReplyOutcome::InvalidInput {
            hint: approval_reply_hint(&pending),
        };
    };
    ApprovalReplyOutcome::Ready {
        conversation_key,
        pending,
        option_index,
        decision,
    }
}

pub(crate) async fn submit_approval_decision(
    state: &SharedState,
    pending: &PendingApproval,
    decision: &ApprovalDecisionOption,
) -> Result<Option<(String, PendingApproval)>> {
    let response = approval_response(decision.decision.clone());
    remote_control_backend::send_response(state, pending.request_id.clone(), response).await?;
    let next = state
        .runtime
        .lock()
        .await
        .resolve_approval_request_with_context(&pending.request_id)
        .and_then(|resolved| {
            resolved
                .next_current
                .map(|next| (resolved.conversation_key, next))
        });
    Ok(next)
}
