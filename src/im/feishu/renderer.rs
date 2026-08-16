const DEFAULT_CARD_TEMPLATE: &str = "blue";
const APPROVAL_CARD_TEMPLATE: &str = "orange";
pub const FEISHU_CARDKIT_STREAMING_ELEMENT_ID: &str = "streaming_content";
const FEISHU_STREAMING_COMMAND_COMMAND_CHARS: usize = 600;
const FEISHU_STREAMING_COMMAND_OUTPUT_CHARS: usize = 2400;
const FEISHU_STREAMING_COMMAND_META_CHARS: usize = 320;

mod approval;
mod common;
mod item_cards;
mod item_summary;
mod items;
mod markdown;
mod media;
mod streaming;
mod threads;

#[allow(unused_imports)]
pub use approval::{build_approval_card, build_resolved_approval_card};
pub use items::{build_item_card, item_markdown_summary};
pub use markdown::normalize_cardkit_streaming_markdown;
pub use media::{build_image_generation_result_card, build_image_view_result_card};
pub use streaming::{
    build_cardkit_streaming_agent_message_card, build_cardkit_streaming_reply_card,
    build_cardkit_streaming_tool_card, build_streaming_command_card,
    build_streaming_file_change_card, build_streaming_file_summary_card,
    build_streaming_mcp_tool_card, build_streaming_plan_card, build_streaming_reasoning_card,
    build_streaming_reply_card, build_turn_completed_card, build_turn_terminal_mark_card,
};
pub use threads::{
    FeishuThreadListEntry, FeishuThreadRoutingAction, build_thread_create_settings_card,
    build_thread_list_card, build_thread_routing_choice_card, build_thread_routing_result_card,
};
