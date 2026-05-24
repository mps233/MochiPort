pub mod active;
mod adapter;
pub mod approval;
pub mod approval_state;
pub mod attachments;
pub mod codex_dispatch;
pub mod commands;
pub mod control;
pub mod desktop;
pub mod dispatch_queue;
pub mod media;
pub mod outbound;
pub mod remote;
pub mod routing;
pub mod runtime;
pub mod session;
pub mod settings;
pub mod thread;
pub mod traits;
pub mod types;

pub use active::{current_active_binding, set_active_binding, ImActiveState};
pub use approval::{
    approval_notice_text, approval_prompt, pending_approval_from_notification,
    resolve_approval_decision, resolve_approval_route, respond_to_codex_request,
};
pub use approval_state::{
    approval_request_ids_for_route, first_pending_approval, pop_pending_approval,
    push_pending_approval, ImApprovalState,
};
pub use attachments::{
    allocate_attachment_path, inbound_attachment_from_local_path, persist_inbound_attachment_bytes,
    ImAttachmentKind,
};
pub use codex_dispatch::{dispatch_codex_notification, dispatch_codex_request};
pub use control::{handle_control_message, ImControlOutcome};
pub use desktop::ImDesktopState;
pub use dispatch_queue::{
    enqueue_notification as enqueue_im_notification, enqueue_request as enqueue_im_request,
    init_im_dispatch_queue, ImDispatchQueueState,
};
pub use media::{
    inbound_attachment_notice, stage_inbound_attachments, take_staged_inbound_attachments,
};
pub use outbound::{remember_sent_text, should_skip_duplicate_text, ImOutboundState};
pub use remote::{
    current_remote_binding, set_remote_binding, set_remote_binding_for_route, ImRemoteState,
};
pub use routing::{
    dispatch_inbound_message, extract_agent_message_text, extract_image_view_path,
    extract_turn_reply_text, resolve_inbound_target_thread, resolve_route_target_by_thread,
    route_codex_text_notification, route_codex_user_message_notification,
    routed_thread_has_active_desktop_turn, start_plan_implementation_turn,
    ImInboundDispatchOutcome,
};
pub use runtime::{
    active_desktop_thread_id, active_desktop_turn_busy, clear_all_runtime, clear_channel_runtime,
    clear_turn_mode, clear_turn_origin, clear_turn_plan, desktop_binding_snapshot,
    get_thread_runtime_state, is_active_desktop_thread, mark_thread_status, mark_turn_completed,
    mark_turn_has_plan, mark_turn_started, note_turn_mode, note_turn_origin, resolve_turn_mode,
    thread_has_in_progress_turn, turn_has_plan, update_thread_runtime_state, ImRuntimeState,
};
pub use session::{
    bind_route_to_thread, build_conversation_key, resolve_conversation_key_by_thread,
    resolve_thread_id_for_chat, ImSessionState,
};
pub use settings::{
    should_sync_message_kind, thread_sandbox_for_settings, ImCommonSettings, ImSyncMessageKind,
    ImSyncSettings,
};
pub use thread::{
    bind_active_desktop_thread_to_route, bind_desktop_thread,
    bind_desktop_thread_with_active_route, bind_inbound_session, fetch_thread_list_page,
    start_thread, sync_active_desktop_thread_name,
};
pub use types::{
    ImChannelKind, ImChatType, ImDesktopAttachmentInput, ImPendingThreadRoute, ImThreadListEntry,
    ImThreadRouteMode, ImTurnTerminalState, InboundAttachment, InboundMessage, OutboundAttachment,
    OutboundMessage, PendingApprovalRequest,
};
