use anyhow::Error;

use crate::{
    app_state::SharedState,
    im::core::routing::{
        clear_thread_binding_with_reason, is_stale_thread_error, live_thread_for_route,
        persist_thread_binding,
    },
    im_runtime::{RouteTarget, TurnOrigin},
    remote_control_backend,
    types::InboundAttachment,
};

pub(crate) enum TurnStartOutcome {
    Started { thread_id: String, turn_id: String },
    NoThread,
    Stale { thread_id: String },
    Failed { error: Error },
}

pub(crate) fn turn_busy_notice(_thread_id: &str, _turn_id: &str) -> &'static str {
    "任务还在进行中，打断 /s，退出会话 /q。"
}

pub(crate) fn turn_completed_notice() -> &'static str {
    "✅ 已完成"
}

pub(crate) async fn start_turn_for_route(
    state: &SharedState,
    route: &RouteTarget,
    text: &str,
    attachments: &[InboundAttachment],
    origin: TurnOrigin,
) -> TurnStartOutcome {
    let Some(thread_id) = live_thread_for_route(state, route).await else {
        return TurnStartOutcome::NoThread;
    };
    if let Err(error) = persist_thread_binding(state, route, &thread_id).await {
        return TurnStartOutcome::Failed { error };
    }

    match remote_control_backend::start_turn_for_client(
        state,
        &route.conversation_key,
        &thread_id,
        text,
        attachments,
    )
    .await
    {
        Ok(turn_id) => {
            state
                .runtime
                .lock()
                .await
                .mark_turn_started(&thread_id, &turn_id);
            state
                .runtime
                .lock()
                .await
                .remember_turn_origin(&turn_id, origin);
            TurnStartOutcome::Started { thread_id, turn_id }
        }
        Err(error) if is_stale_thread_error(&error) => {
            let _ =
                clear_thread_binding_with_reason(state, &route.conversation_key, "stale_thread")
                    .await;
            TurnStartOutcome::Stale { thread_id }
        }
        Err(error) => TurnStartOutcome::Failed { error },
    }
}
