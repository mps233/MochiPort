use std::{
    collections::{HashMap, HashSet, VecDeque},
    fmt::Write as _,
    sync::Arc,
    sync::atomic::{AtomicU64, Ordering},
};

use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::Notify;

use crate::{
    chain_log,
    im::feishu::FeishuStreamingCardState,
    types::{ImPlatformKind, InboundAttachment},
};

const PENDING_ATTACHMENTS_MAX_AGE_MS: u128 = 10 * 60 * 1000;
const PENDING_ATTACHMENTS_MAX_COUNT: usize = 8;
pub(crate) const TELEGRAM_QUEUED_TURNS_MAX_COUNT: usize = 8;
const TELEGRAM_COMMENTARY_MAX_ENTRIES: usize = 64;
const TELEGRAM_COMMAND_PROGRESS_MAX_ENTRIES: usize = 128;
const TELEGRAM_COLLAB_PROGRESS_MAX_AGENTS: usize = 64;
const TELEGRAM_COMPLETED_TYPING_HISTORY_MAX: usize = 512;
const TERMINAL_NOTICE_TURN_HISTORY_MAX: usize = 256;
const TELEGRAM_REASONING_MAX_CHARS: usize = 12_000;
const TELEGRAM_PLAN_MAX_STEPS: usize = 64;
const TELEGRAM_DIFF_MAX_PATHS: usize = 128;
const TELEGRAM_WEB_SEARCH_MAX_ENTRIES: usize = 16;
pub(crate) const TELEGRAM_THREAD_SETTINGS_DRAFT_MAX_AGE_MS: u128 = 30 * 60 * 1000;

static TELEGRAM_MODEL_SWITCH_REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TelegramCommentaryEntry {
    pub item_id: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TelegramCommentarySnapshot {
    pub turn_id: String,
    pub segment: u64,
    pub message_id: Option<String>,
    pub entries: Vec<TelegramCommentaryEntry>,
    pub dropped_entries: usize,
}

#[derive(Debug, Clone)]
struct TelegramCommentaryState {
    turn_id: String,
    active_segment: u64,
    segments: HashMap<u64, TelegramCommentarySegmentState>,
    completed: bool,
}

#[derive(Debug, Clone, Default)]
struct TelegramCommentarySegmentState {
    message_id: Option<String>,
    entries: Vec<TelegramCommentaryEntry>,
    dropped_entries: usize,
}

#[derive(Debug, Clone)]
struct PendingAttachments {
    attachments: Vec<InboundAttachment>,
    received_at_ms: u128,
}

#[derive(Debug, Clone)]
pub(crate) struct PendingTelegramTurn {
    pub text: String,
    pub attachments: Vec<InboundAttachment>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TelegramQueueEnqueueOutcome {
    Added(usize),
    Full,
    NotRunning,
}

#[derive(Debug, Clone)]
pub struct RouteTarget {
    pub platform: ImPlatformKind,
    pub conversation_key: String,
    pub account_id: String,
    pub chat_id: String,
    pub remote_client_key: String,
}

#[derive(Debug, Clone)]
pub struct PendingApproval {
    pub request_id: Value,
    pub request_kind: String,
    #[allow(dead_code)]
    pub method: String,
    #[allow(dead_code)]
    pub params: Value,
    pub summary: String,
    pub decisions: Vec<ApprovalDecisionOption>,
    pub message_id: Option<String>,
    pub remote_client_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ApprovalDecisionOption {
    pub label: String,
    pub decision: Value,
}

#[derive(Debug, Clone)]
pub struct ResolvedApproval {
    pub conversation_key: String,
    #[allow(dead_code)]
    pub approval: PendingApproval,
    pub was_current: bool,
    pub next_current: Option<PendingApproval>,
}

#[derive(Debug, Clone)]
pub struct ThreadRoutingRequestState {
    pub request_id: String,
    pub conversation_key: String,
    #[allow(dead_code)]
    pub account_id: String,
    pub chat_id: String,
    pub message_id: Option<String>,
    pub stage: ThreadRoutingStage,
    pub page: usize,
    pub page_cursors: Vec<Option<String>>,
    pub thread_ids_by_page: Vec<Vec<String>>,
    pub create_draft: ThreadCreateDraftState,
    pub create_option_values_by_field_page: HashMap<String, Vec<Vec<String>>>,
    #[allow(dead_code)]
    pub history_cursor: Option<String>,
    pub history_has_next: bool,
}

/// A setting value observed from Codex. `Unknown` is intentionally distinct
/// from `Known(None)`: the latter means Codex explicitly has no override.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ObservedSetting<T> {
    #[default]
    Unknown,
    Known(Option<T>),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ThreadSettingsSnapshot {
    pub model: ObservedSetting<String>,
    pub effort: ObservedSetting<String>,
    pub service_tier: ObservedSetting<String>,
}

impl ThreadSettingsSnapshot {
    pub fn merge_from(&mut self, update: &Self) {
        merge_observed_setting(&mut self.model, &update.model);
        merge_observed_setting(&mut self.effort, &update.effort);
        merge_observed_setting(&mut self.service_tier, &update.service_tier);
    }

    pub fn from_protocol_value(value: &Value) -> Self {
        let value = value.get("thread").unwrap_or(value);
        Self {
            model: observed_string(value, &["model"]),
            effort: observed_string(value, &["reasoningEffort", "effort"]),
            service_tier: observed_string(value, &["serviceTier", "service_tier"]),
        }
    }
}

fn merge_observed_setting<T: Clone>(target: &mut ObservedSetting<T>, update: &ObservedSetting<T>) {
    if !matches!(update, ObservedSetting::Unknown) {
        *target = update.clone();
    }
}

fn observed_string(value: &Value, keys: &[&str]) -> ObservedSetting<String> {
    for key in keys {
        if let Some(value) = value.get(*key) {
            return ObservedSetting::Known(
                value
                    .as_str()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
            );
        }
    }
    ObservedSetting::Unknown
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TelegramThreadSettingsModelChoice {
    pub model: String,
    pub label: String,
    pub supported_efforts: Vec<String>,
    pub default_effort: Option<String>,
    pub supports_fast: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TelegramThreadSettingsSpeed {
    Standard,
    Fast,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TelegramThreadSettingsDraft {
    pub model: Option<String>,
    pub effort: Option<String>,
    pub speed: Option<TelegramThreadSettingsSpeed>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum TelegramThreadSettingsPatchValue {
    #[default]
    Unchanged,
    Set(String),
    Clear,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TelegramThreadSettingsPatch {
    pub model: TelegramThreadSettingsPatchValue,
    pub effort: TelegramThreadSettingsPatchValue,
    pub service_tier: TelegramThreadSettingsPatchValue,
}

impl TelegramThreadSettingsPatch {
    pub fn is_empty(&self) -> bool {
        matches!(&self.model, TelegramThreadSettingsPatchValue::Unchanged)
            && matches!(&self.effort, TelegramThreadSettingsPatchValue::Unchanged)
            && matches!(
                &self.service_tier,
                TelegramThreadSettingsPatchValue::Unchanged
            )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TelegramThreadSettingsStage {
    #[default]
    Overview,
    Model,
    Effort,
    Speed,
    CompatibilityConfirmation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramThreadSettingsCompatibility {
    pub reset_effort: bool,
    pub reset_speed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingTelegramThreadSettingsApply {
    pub patch: TelegramThreadSettingsPatch,
    pub submitted_at_ms: u128,
}

/// One staged Telegram settings editor for an already-bound Codex thread.
/// Model ids remain in server-side state; callback payloads only contain
/// request id, revision and indexes or fixed tokens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramModelSwitchRequestState {
    pub request_id: String,
    pub conversation_key: String,
    pub account_id: String,
    pub chat_id: String,
    pub expected_thread_id: String,
    pub remote_client_key: String,
    pub catalog: Vec<TelegramThreadSettingsModelChoice>,
    pub observed: ThreadSettingsSnapshot,
    pub draft: TelegramThreadSettingsDraft,
    pub revision: u64,
    pub expires_at_ms: u128,
    pub stage: TelegramThreadSettingsStage,
    pub model_page: usize,
    pub compatibility: Option<TelegramThreadSettingsCompatibility>,
    pub pending_apply: Option<PendingTelegramThreadSettingsApply>,
    pub stale: bool,
    pub message_id: Option<String>,
}

pub fn next_telegram_model_switch_request_id() -> String {
    let value = TELEGRAM_MODEL_SWITCH_REQUEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("thread-model-{value}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadRoutingStage {
    Choice,
    ResumeList,
    CreateSettings,
    CreateOptions,
}

#[derive(Debug, Clone, Default)]
pub struct ThreadCreateDraftState {
    pub cwd_choice: Option<String>,
    pub cwd_custom: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub permission: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnOrigin {
    Feishu,
    Telegram,
    Wechat,
    Wecom,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThreadTurnState {
    Starting,
    Running(String),
}

#[derive(Debug, Default)]
pub struct RuntimeState {
    pub bridge_generation: u64,
    pub current_turn_by_thread: HashMap<String, String>,
    pub starting_turn_by_thread: HashSet<String>,
    pub turn_started_at_by_thread: HashMap<String, u128>,
    pub turn_finished_at_by_thread: HashMap<String, u128>,
    pub turn_origin_by_id: HashMap<String, TurnOrigin>,
    terminal_notice_failed_by_turn: HashMap<String, bool>,
    terminal_notice_turn_order: VecDeque<String>,
    terminal_status_fallback_by_thread: HashMap<String, TerminalStatusFallbackState>,
    next_terminal_status_fallback_token: u64,
    pub last_sent_text_by_route: HashMap<String, String>,
    pub route_by_thread: HashMap<String, RouteTarget>,
    pub last_route: Option<RouteTarget>,
    pub pending_approvals_by_conversation: HashMap<String, Vec<PendingApproval>>,
    pub pending_approval_request_keys: HashSet<String>,
    pub feishu_streaming_cards_by_item: HashMap<String, FeishuStreamingCardState>,
    telegram_typing_by_item: HashMap<String, TelegramTypingState>,
    telegram_typing_suspended_threads: HashSet<String>,
    telegram_completed_typing_keys: HashSet<String>,
    telegram_completed_typing_order: VecDeque<String>,
    telegram_command_progress_by_thread: HashMap<String, TelegramCommandProgressState>,
    telegram_commentary_by_thread: HashMap<String, TelegramCommentaryState>,
    next_telegram_typing_generation: i64,
    pub wecom_streams_by_thread: HashMap<String, WecomStreamState>,
    pub thread_routing_requests: HashMap<String, ThreadRoutingRequestState>,
    telegram_model_switch_requests: HashMap<String, TelegramModelSwitchRequestState>,
    telegram_model_switch_request_by_conversation: HashMap<String, String>,
    thread_settings_by_thread: HashMap<String, ThreadSettingsSnapshot>,
    pending_attachments_by_conversation: HashMap<String, PendingAttachments>,
    pending_telegram_turns_by_conversation: HashMap<String, VecDeque<PendingTelegramTurn>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TerminalStatusFallbackState {
    turn_id: String,
    token: u64,
    event_driver_started: bool,
    server_driver_started: bool,
}

#[derive(Debug, Clone)]
struct TelegramTypingState {
    generation: i64,
    sending: bool,
    dirty: bool,
    finished: bool,
    revision: u64,
    last_attempt_at_ms: u128,
    wake_driver: Arc<Notify>,
    completed: Arc<Notify>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TelegramCommandProgressStatus {
    Running,
    Interrupted,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TelegramCommandProgressEntryKind {
    Command,
    McpTool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TelegramCommandProgressEntry {
    pub item_id: String,
    pub kind: TelegramCommandProgressEntryKind,
    pub command: String,
    pub status: TelegramCommandProgressStatus,
    pub exit_code: Option<i64>,
    pub duration_ms: Option<u64>,
    pub failure_output: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TelegramWebSearchProgressEntry {
    pub item_id: String,
    pub summary: String,
    pub blocks: Vec<Value>,
    pub fallback_markdown: String,
}

#[derive(Debug, Clone)]
pub(crate) struct TelegramCommandProgressSnapshot {
    pub turn_id: String,
    pub revision: u64,
    pub message_id: Option<String>,
    pub entries: Vec<TelegramCommandProgressEntry>,
    pub dropped_entries: usize,
    pub retry_count: usize,
    pub retry_error: Option<String>,
    pub reasoning_summary: Option<String>,
    pub plan_explanation: Option<String>,
    pub plan: Vec<TelegramPlanStep>,
    pub diff_summary: Option<TelegramDiffSummary>,
    pub web_searches: Vec<TelegramWebSearchProgressEntry>,
    pub dropped_web_searches: usize,
    pub collab: Option<TelegramCollabProgressSnapshot>,
    pub completed: bool,
    pub failed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TelegramPlanStep {
    pub step: String,
    pub status: TelegramPlanStepStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TelegramPlanStepStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TelegramDiffFileSummary {
    pub path: String,
    pub additions: usize,
    pub deletions: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TelegramDiffSummary {
    pub file_count: usize,
    pub additions: usize,
    pub deletions: usize,
    pub files: Vec<TelegramDiffFileSummary>,
    pub paths: Vec<String>,
    pub omitted_paths: usize,
}

#[derive(Debug, Clone)]
struct TelegramCommandProgressState {
    turn_id: String,
    revision: u64,
    message_id: Option<String>,
    entries: Vec<TelegramCommandProgressEntry>,
    dropped_entries: usize,
    retry_count: usize,
    retry_error: Option<String>,
    reasoning_item_id: Option<String>,
    reasoning_summary_index: Option<i64>,
    reasoning_summary: String,
    reasoning_completed: bool,
    plan_explanation: Option<String>,
    plan: Vec<TelegramPlanStep>,
    diff_summary: Option<TelegramDiffSummary>,
    web_searches: Vec<TelegramWebSearchProgressEntry>,
    dropped_web_searches: usize,
    collab_entries: Vec<TelegramCollabProgressEntry>,
    collab_dropped_entries: usize,
    completed: bool,
    failed: bool,
    dirty: bool,
    sending: bool,
    in_flight_revision: Option<u64>,
    cleanup_after_delivery: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TelegramCollabProgressStatus {
    Running,
    Responded,
    Succeeded,
    Failed,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TelegramCollabProgressUpdate {
    pub agent_id: String,
    pub name: Option<String>,
    pub status: Option<TelegramCollabProgressStatus>,
    pub detail: Option<String>,
    pub occurred_at_ms: u128,
    pub create_if_missing: bool,
    pub restart: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TelegramCollabProgressEntry {
    pub agent_id: String,
    pub name: String,
    pub status: TelegramCollabProgressStatus,
    pub detail: Option<String>,
    pub started_at_ms: u128,
    pub updated_at_ms: u128,
}

#[derive(Debug, Clone)]
pub(crate) struct TelegramCollabProgressSnapshot {
    pub entries: Vec<TelegramCollabProgressEntry>,
    pub dropped_entries: usize,
    pub completed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TelegramTypingSendAction {
    Continue,
    Stop,
}

#[derive(Debug, Clone)]
pub struct WecomStreamState {
    pub req_id: String,
    pub stream_id: String,
    pub content: String,
    pub sent_content: String,
    pub finished: bool,
    pub sending: bool,
    pub dirty: bool,
    pub delivered: bool,
    pub cleanup_after_delivery: bool,
    pub revision: u64,
}

impl RuntimeState {
    /// Return the counts that make a daemon lifecycle operation unsafe to
    /// commit immediately.  The maps themselves stay private so the
    /// management API cannot accidentally mutate bridge state while taking a
    /// snapshot.
    pub fn protected_work_item_counts(&self) -> (usize, usize, usize) {
        let codex_turns = self
            .current_turn_by_thread
            .keys()
            .chain(self.starting_turn_by_thread.iter())
            .collect::<HashSet<_>>()
            .len()
            + self
                .pending_telegram_turns_by_conversation
                .values()
                .map(VecDeque::len)
                .sum::<usize>();
        let im_streams = self.feishu_streaming_cards_by_item.len()
            + self
                .telegram_typing_by_item
                .values()
                .filter(|typing| !typing.finished)
                .count()
            + self
                .wecom_streams_by_thread
                .values()
                .filter(|stream| !stream.finished || !stream.delivered)
                .count();
        let pending_approvals = self.pending_approval_request_keys.len();
        (codex_turns, im_streams, pending_approvals)
    }

    pub fn start_bridge_generation(&mut self) -> u64 {
        self.bridge_generation = self.bridge_generation.saturating_add(1);
        self.feishu_streaming_cards_by_item.clear();
        self.clear_all_telegram_typing();
        self.telegram_command_progress_by_thread.clear();
        self.telegram_commentary_by_thread.clear();
        self.terminal_notice_failed_by_turn.clear();
        self.terminal_notice_turn_order.clear();
        self.terminal_status_fallback_by_thread.clear();
        self.wecom_streams_by_thread.clear();
        self.telegram_model_switch_requests.clear();
        self.telegram_model_switch_request_by_conversation.clear();
        self.thread_settings_by_thread.clear();
        self.pending_attachments_by_conversation.clear();
        self.pending_telegram_turns_by_conversation.clear();
        self.bridge_generation
    }

    pub fn invalidate_bridge_generation(&mut self) {
        self.bridge_generation = self.bridge_generation.saturating_add(1);
        self.feishu_streaming_cards_by_item.clear();
        self.clear_all_telegram_typing();
        self.telegram_command_progress_by_thread.clear();
        self.telegram_commentary_by_thread.clear();
        self.terminal_notice_failed_by_turn.clear();
        self.terminal_notice_turn_order.clear();
        self.terminal_status_fallback_by_thread.clear();
        self.wecom_streams_by_thread.clear();
        self.telegram_model_switch_requests.clear();
        self.telegram_model_switch_request_by_conversation.clear();
        self.thread_settings_by_thread.clear();
        self.pending_attachments_by_conversation.clear();
        self.pending_telegram_turns_by_conversation.clear();
    }

    pub fn is_bridge_generation(&self, generation: u64) -> bool {
        self.bridge_generation == generation
    }

    pub fn bind_route(&mut self, thread_id: &str, route: RouteTarget) {
        self.last_route = Some(route.clone());
        let previous = self
            .route_by_thread
            .insert(thread_id.to_string(), route.clone());
        if previous.as_ref().is_none_or(|previous| {
            previous.conversation_key != route.conversation_key
                || previous.remote_client_key != route.remote_client_key
        }) {
            self.clear_telegram_model_switch_requests_for_conversation(&route.conversation_key);
            if let Some(previous) = previous.as_ref() {
                self.clear_telegram_model_switch_requests_for_conversation(
                    &previous.conversation_key,
                );
            }
        }
        log_route_bind(thread_id, &route, previous.as_ref());
    }

    pub fn unbind_routes_for_conversation_with_reason(
        &mut self,
        conversation_key: &str,
        reason: &str,
    ) -> Vec<String> {
        let entries = self
            .route_by_thread
            .iter()
            .filter(|&(_, route)| route.conversation_key == conversation_key)
            .map(|(thread_id, route)| (thread_id.clone(), route.clone()))
            .collect::<Vec<_>>();
        for (thread_id, route) in &entries {
            self.route_by_thread.remove(thread_id);
            if let Some(turn_id) = self.current_turn_by_thread.remove(thread_id) {
                self.turn_origin_by_id.remove(&turn_id);
            }
            self.starting_turn_by_thread.remove(thread_id);
            self.turn_started_at_by_thread.remove(thread_id);
            self.turn_finished_at_by_thread.remove(thread_id);
            self.clear_telegram_typing_for_thread(thread_id);
            self.telegram_command_progress_by_thread.remove(thread_id);
            self.telegram_commentary_by_thread.remove(thread_id);
            self.terminal_status_fallback_by_thread.remove(thread_id);
            self.thread_settings_by_thread.remove(thread_id);
            self.pending_telegram_turns_by_conversation
                .remove(&route.conversation_key);
            log_route_unbind("unbind_conversation", reason, thread_id, route);
        }
        self.clear_telegram_model_switch_requests_for_conversation(conversation_key);
        entries
            .into_iter()
            .map(|(thread_id, _)| thread_id)
            .collect()
    }

    pub fn mark_turn_started(&mut self, thread_id: &str, turn_id: &str) {
        self.starting_turn_by_thread.remove(thread_id);
        self.terminal_status_fallback_by_thread.remove(thread_id);
        if self
            .telegram_command_progress_by_thread
            .get(thread_id)
            .is_some_and(|progress| progress.turn_id != turn_id)
        {
            self.telegram_command_progress_by_thread.remove(thread_id);
        }
        if self
            .telegram_commentary_by_thread
            .get(thread_id)
            .is_some_and(|commentary| commentary.turn_id != turn_id)
        {
            self.telegram_commentary_by_thread.remove(thread_id);
        }
        self.current_turn_by_thread
            .insert(thread_id.to_string(), turn_id.to_string());
        self.turn_started_at_by_thread
            .insert(thread_id.to_string(), crate::types::now_ms());
    }

    pub fn try_mark_turn_starting(&mut self, thread_id: &str) -> Result<(), ThreadTurnState> {
        if let Some(turn_id) = self.current_turn_by_thread.get(thread_id) {
            return Err(ThreadTurnState::Running(turn_id.clone()));
        }
        if self.starting_turn_by_thread.contains(thread_id) {
            return Err(ThreadTurnState::Starting);
        }
        self.starting_turn_by_thread.insert(thread_id.to_string());
        Ok(())
    }

    pub fn clear_turn_starting(&mut self, thread_id: &str) {
        self.starting_turn_by_thread.remove(thread_id);
    }

    pub fn message_is_stale_for_latest_turn(&self, thread_id: &str, received_at_ms: u128) -> bool {
        received_at_ms > 0
            && (self
                .turn_finished_at_by_thread
                .get(thread_id)
                .is_some_and(|finished_at_ms| received_at_ms < *finished_at_ms)
                || self
                    .turn_started_at_by_thread
                    .get(thread_id)
                    .is_some_and(|started_at_ms| received_at_ms < *started_at_ms))
    }

    pub fn remember_turn_origin(&mut self, turn_id: &str, origin: TurnOrigin) {
        self.turn_origin_by_id.insert(turn_id.to_string(), origin);
    }

    pub(crate) fn claim_terminal_notice(&mut self, turn_id: &str, failed: bool) -> bool {
        if let Some(previous_failed) = self.terminal_notice_failed_by_turn.get_mut(turn_id) {
            // A late failure must still be visible after an optimistic success notice.
            if failed && !*previous_failed {
                *previous_failed = true;
                return true;
            }
            return false;
        }
        self.terminal_notice_failed_by_turn
            .insert(turn_id.to_string(), failed);
        self.terminal_notice_turn_order
            .push_back(turn_id.to_string());
        while self.terminal_notice_turn_order.len() > TERMINAL_NOTICE_TURN_HISTORY_MAX {
            if let Some(oldest) = self.terminal_notice_turn_order.pop_front() {
                self.terminal_notice_failed_by_turn.remove(&oldest);
            }
        }
        true
    }

    pub(crate) fn release_terminal_notice(&mut self, turn_id: &str) {
        if self
            .terminal_notice_failed_by_turn
            .remove(turn_id)
            .is_none()
        {
            return;
        }
        self.terminal_notice_turn_order
            .retain(|current| current != turn_id);
    }

    pub fn turn_origin(&self, turn_id: &str) -> Option<TurnOrigin> {
        self.turn_origin_by_id.get(turn_id).copied()
    }

    pub(crate) fn current_turn_id(&self, thread_id: &str) -> Option<&str> {
        self.current_turn_by_thread
            .get(thread_id)
            .map(String::as_str)
    }

    pub(crate) fn turn_in_progress(&self, thread_id: &str) -> bool {
        self.current_turn_by_thread.contains_key(thread_id)
            || self.starting_turn_by_thread.contains(thread_id)
    }

    #[cfg(test)]
    pub(crate) fn register_terminal_status_fallback(
        &mut self,
        thread_id: &str,
        turn_id: &str,
    ) -> bool {
        self.register_terminal_status_fallback_token(thread_id, turn_id)
            .is_some_and(|(_, should_start_server_driver)| should_start_server_driver)
    }

    /// Register the long-lived server-side safety timer for a turn.
    ///
    /// A repeated status notification reuses the existing token instead of
    /// creating another latch. The boolean identifies whether the caller is
    /// responsible for spawning the server-side timer.
    pub(crate) fn register_terminal_status_fallback_token(
        &mut self,
        thread_id: &str,
        turn_id: &str,
    ) -> Option<(u64, bool)> {
        if self.current_turn_id(thread_id) != Some(turn_id) {
            return None;
        }
        if let Some(existing) = self.terminal_status_fallback_by_thread.get_mut(thread_id)
            && existing.turn_id == turn_id
        {
            let should_start_server_driver = !existing.server_driver_started;
            existing.server_driver_started = true;
            return Some((existing.token, should_start_server_driver));
        }
        let token = self.next_terminal_status_fallback_token();
        self.terminal_status_fallback_by_thread.insert(
            thread_id.to_string(),
            TerminalStatusFallbackState {
                turn_id: turn_id.to_string(),
                token,
                event_driver_started: false,
                server_driver_started: true,
            },
        );
        Some((token, true))
    }

    /// Start the IM-side timer using the same latch as the server safety timer.
    ///
    /// When the server registered the latch first, this marks the existing
    /// entry as owned by the IM driver.  If the IM path is the first observer,
    /// it creates the entry itself.  Only one IM driver can be started.
    pub(crate) fn start_terminal_status_fallback(
        &mut self,
        thread_id: &str,
        turn_id: &str,
    ) -> Option<u64> {
        if self.current_turn_id(thread_id) != Some(turn_id) {
            return None;
        }
        if let Some(existing) = self.terminal_status_fallback_by_thread.get_mut(thread_id) {
            if existing.turn_id != turn_id || existing.event_driver_started {
                return None;
            }
            existing.event_driver_started = true;
            return Some(existing.token);
        }
        let token = self.next_terminal_status_fallback_token();
        self.terminal_status_fallback_by_thread.insert(
            thread_id.to_string(),
            TerminalStatusFallbackState {
                turn_id: turn_id.to_string(),
                token,
                event_driver_started: true,
                server_driver_started: false,
            },
        );
        Some(token)
    }

    fn next_terminal_status_fallback_token(&mut self) -> u64 {
        self.next_terminal_status_fallback_token = self
            .next_terminal_status_fallback_token
            .wrapping_add(1)
            .max(1);
        self.next_terminal_status_fallback_token
    }

    pub(crate) fn cancel_terminal_status_fallback(&mut self, thread_id: &str, turn_id: &str) {
        if self
            .terminal_status_fallback_by_thread
            .get(thread_id)
            .is_some_and(|current| current.turn_id == turn_id)
        {
            self.terminal_status_fallback_by_thread.remove(thread_id);
        }
    }

    #[cfg(test)]
    pub(crate) fn terminal_status_fallback_matches(&self, thread_id: &str, turn_id: &str) -> bool {
        self.terminal_status_fallback_by_thread
            .get(thread_id)
            .is_some_and(|current| current.turn_id == turn_id)
    }

    pub(crate) fn terminal_status_fallback_matches_token(
        &self,
        thread_id: &str,
        turn_id: &str,
        token: u64,
    ) -> bool {
        self.terminal_status_fallback_by_thread
            .get(thread_id)
            .is_some_and(|current| current.turn_id == turn_id && current.token == token)
    }

    /// Atomically consume a fallback latch for the matching current turn.
    pub(crate) fn claim_terminal_status_fallback(
        &mut self,
        thread_id: &str,
        turn_id: &str,
        token: u64,
    ) -> bool {
        if self.current_turn_id(thread_id) != Some(turn_id)
            || !self.terminal_status_fallback_matches_token(thread_id, turn_id, token)
        {
            return false;
        }
        self.terminal_status_fallback_by_thread.remove(thread_id);
        true
    }

    pub fn should_skip_duplicate_text(&self, route_key: &str, text: &str) -> bool {
        self.last_sent_text_by_route
            .get(route_key)
            .map(|last| last == text)
            .unwrap_or(false)
    }

    pub fn remember_sent_text(&mut self, route_key: &str, text: &str) {
        self.last_sent_text_by_route
            .insert(route_key.to_string(), text.to_string());
    }

    pub(crate) fn append_telegram_commentary(
        &mut self,
        thread_id: &str,
        turn_id: &str,
        item_id: &str,
        text: String,
    ) -> Option<TelegramCommentarySnapshot> {
        if self.current_turn_id(thread_id) != Some(turn_id) {
            return None;
        }
        let commentary = self
            .telegram_commentary_by_thread
            .entry(thread_id.to_string())
            .or_insert_with(|| TelegramCommentaryState {
                turn_id: turn_id.to_string(),
                active_segment: 0,
                segments: HashMap::new(),
                completed: false,
            });
        if commentary.turn_id != turn_id {
            *commentary = TelegramCommentaryState {
                turn_id: turn_id.to_string(),
                active_segment: 0,
                segments: HashMap::new(),
                completed: false,
            };
        }
        if commentary.completed {
            return None;
        }
        let active_segment = commentary.active_segment;
        let segment = commentary.segments.entry(active_segment).or_default();

        match segment
            .entries
            .iter_mut()
            .find(|entry| entry.item_id == item_id)
        {
            Some(entry) if entry.text == text => return None,
            Some(entry) => entry.text = text,
            None => segment.entries.push(TelegramCommentaryEntry {
                item_id: item_id.to_string(),
                text,
            }),
        }
        if segment.entries.len() > TELEGRAM_COMMENTARY_MAX_ENTRIES {
            let excess = segment.entries.len() - TELEGRAM_COMMENTARY_MAX_ENTRIES;
            segment.entries.drain(..excess);
            segment.dropped_entries = segment.dropped_entries.saturating_add(excess);
        }
        Some(telegram_commentary_snapshot(
            &commentary.turn_id,
            active_segment,
            segment,
        ))
    }

    pub(crate) fn remember_telegram_commentary_delivery(
        &mut self,
        thread_id: &str,
        turn_id: &str,
        segment: u64,
        message_id: String,
    ) {
        let Some(commentary) = self.telegram_commentary_by_thread.get_mut(thread_id) else {
            return;
        };
        if commentary.turn_id == turn_id
            && let Some(commentary_segment) = commentary.segments.get_mut(&segment)
        {
            commentary_segment.message_id = Some(message_id);
        }
    }

    pub(crate) fn telegram_commentary_delivery_target(
        &self,
        thread_id: &str,
        turn_id: &str,
        segment: u64,
    ) -> Option<Option<String>> {
        let commentary = self.telegram_commentary_by_thread.get(thread_id)?;
        if commentary.turn_id != turn_id {
            return None;
        }
        commentary
            .segments
            .get(&segment)
            .map(|commentary_segment| commentary_segment.message_id.clone())
    }

    pub(crate) fn start_new_telegram_commentary_segment(
        &mut self,
        thread_id: &str,
        turn_id: &str,
    ) -> bool {
        let commentary = self
            .telegram_commentary_by_thread
            .entry(thread_id.to_string())
            .or_insert_with(|| TelegramCommentaryState {
                turn_id: turn_id.to_string(),
                active_segment: 0,
                segments: HashMap::new(),
                completed: false,
            });
        if commentary.turn_id != turn_id || commentary.completed {
            return false;
        }
        commentary.active_segment = commentary.active_segment.saturating_add(1);
        true
    }

    pub fn start_telegram_typing(&mut self, thread_id: &str, item_id: &str) -> Option<i64> {
        let key = telegram_typing_key(thread_id, item_id);
        if self.telegram_typing_suspended_threads.contains(thread_id)
            || self.telegram_completed_typing_keys.contains(&key)
        {
            return None;
        }
        let thread_prefix = telegram_typing_thread_prefix(thread_id);
        if !self.telegram_typing_by_item.contains_key(&key)
            && self
                .telegram_typing_by_item
                .keys()
                .any(|active_key| active_key.starts_with(&thread_prefix))
        {
            return None;
        }
        if !self.telegram_typing_by_item.contains_key(&key) {
            self.next_telegram_typing_generation =
                self.next_telegram_typing_generation.saturating_add(1);
            if self.next_telegram_typing_generation <= 0 {
                self.next_telegram_typing_generation = 1;
            }
            self.telegram_typing_by_item.insert(
                key.clone(),
                TelegramTypingState {
                    generation: self.next_telegram_typing_generation,
                    sending: false,
                    dirty: false,
                    finished: false,
                    revision: 0,
                    last_attempt_at_ms: 0,
                    wake_driver: Arc::new(Notify::new()),
                    completed: Arc::new(Notify::new()),
                },
            );
        }
        let typing = self.telegram_typing_by_item.get_mut(&key)?;
        if typing.sending {
            return None;
        }
        typing.sending = true;
        typing.dirty = true;
        typing.revision = typing.revision.saturating_add(1);
        Some(typing.generation)
    }

    #[cfg(test)]
    pub fn finish_telegram_typing(
        &mut self,
        thread_id: &str,
        item_id: &str,
    ) -> Option<(i64, bool, Arc<Notify>)> {
        let key = telegram_typing_key(thread_id, item_id);
        self.remember_completed_telegram_typing(&key);
        let typing = self.telegram_typing_by_item.get_mut(&key)?;
        typing.dirty = true;
        typing.finished = true;
        typing.revision = typing.revision.saturating_add(1);
        let should_start = !typing.sending;
        typing.sending = true;
        // Wake a driver that may be sleeping in a retry backoff even when a
        // fresh completion driver will be spawned for this state.
        typing.wake_driver.notify_one();
        Some((typing.generation, should_start, typing.completed.clone()))
    }

    pub fn finish_telegram_typing_for_thread(
        &mut self,
        thread_id: &str,
    ) -> Vec<(String, i64, bool)> {
        self.stop_telegram_typing_for_thread(thread_id, true, None)
    }

    pub fn suspend_telegram_typing_for_persistent_message(
        &mut self,
        thread_id: &str,
        terminal: bool,
        turn_id: Option<&str>,
    ) -> Vec<(String, i64, bool)> {
        self.telegram_typing_suspended_threads
            .insert(thread_id.to_string());
        self.stop_telegram_typing_for_thread(thread_id, terminal, turn_id)
    }

    pub fn resume_telegram_typing_after_persistent_message(&mut self, thread_id: &str) {
        self.telegram_typing_suspended_threads.remove(thread_id);
    }

    fn stop_telegram_typing_for_thread(
        &mut self,
        thread_id: &str,
        terminal: bool,
        terminal_turn_id: Option<&str>,
    ) -> Vec<(String, i64, bool)> {
        let terminal_turn_id = if terminal {
            terminal_turn_id.map(str::to_string).or_else(|| {
                self.current_turn_by_thread
                    .get(thread_id)
                    .map(String::to_string)
            })
        } else {
            None
        };
        if let Some(turn_id) = terminal_turn_id {
            self.remember_completed_telegram_typing(&telegram_typing_key(
                thread_id,
                &format!("turn:{turn_id}"),
            ));
        }
        let prefix = telegram_typing_thread_prefix(thread_id);
        let mut item_ids = self
            .telegram_typing_by_item
            .keys()
            .filter_map(|key| key.strip_prefix(&prefix).map(str::to_string))
            .collect::<Vec<_>>();
        item_ids.sort();

        let mut finishing = Vec::with_capacity(item_ids.len());
        for item_id in item_ids {
            let key = telegram_typing_key(thread_id, &item_id);
            if terminal {
                self.remember_completed_telegram_typing(&key);
            }
            let Some(typing) = self.telegram_typing_by_item.get_mut(&key) else {
                continue;
            };
            typing.dirty = true;
            typing.finished = true;
            typing.revision = typing.revision.saturating_add(1);
            let should_start = !typing.sending;
            typing.sending = true;
            typing.wake_driver.notify_one();
            finishing.push((item_id, typing.generation, should_start));
        }
        finishing
    }

    pub fn take_telegram_typing_snapshot(
        &mut self,
        thread_id: &str,
        item_id: &str,
        generation: i64,
        attempted_at_ms: u128,
    ) -> Option<(bool, u64)> {
        let typing = self
            .telegram_typing_by_item
            .get_mut(&telegram_typing_key(thread_id, item_id))?;
        if typing.generation != generation {
            return None;
        }
        if !typing.sending || !typing.dirty {
            typing.sending = false;
            return None;
        }
        typing.dirty = false;
        typing.last_attempt_at_ms = attempted_at_ms.max(1);
        Some((typing.finished, typing.revision))
    }

    pub fn telegram_typing_send_delay_ms(
        &self,
        thread_id: &str,
        item_id: &str,
        generation: i64,
        now_ms: u128,
        throttle_ms: u128,
    ) -> Option<u64> {
        let typing = self
            .telegram_typing_by_item
            .get(&telegram_typing_key(thread_id, item_id))?;
        if typing.generation != generation || !typing.sending || !typing.dirty {
            return None;
        }
        if typing.finished {
            return Some(0);
        }
        if typing.last_attempt_at_ms == 0 {
            return Some(0);
        }
        let remaining = throttle_ms
            .saturating_sub(now_ms.saturating_sub(typing.last_attempt_at_ms))
            .min(u128::from(u64::MAX));
        Some(remaining as u64)
    }

    pub fn telegram_typing_wait_for_update(
        &self,
        thread_id: &str,
        item_id: &str,
        generation: i64,
        now_ms: u128,
        heartbeat_ms: u128,
    ) -> Option<(Arc<Notify>, u64)> {
        let typing = self
            .telegram_typing_by_item
            .get(&telegram_typing_key(thread_id, item_id))?;
        if typing.generation != generation || !typing.sending || typing.dirty || typing.finished {
            return None;
        }
        let delay_ms = if typing.last_attempt_at_ms == 0 {
            heartbeat_ms
        } else {
            heartbeat_ms.saturating_sub(now_ms.saturating_sub(typing.last_attempt_at_ms))
        }
        .min(u128::from(u64::MAX));
        Some((typing.wake_driver.clone(), delay_ms as u64))
    }

    pub fn telegram_typing_wake_driver(
        &self,
        thread_id: &str,
        item_id: &str,
        generation: i64,
    ) -> Option<Arc<Notify>> {
        let typing = self
            .telegram_typing_by_item
            .get(&telegram_typing_key(thread_id, item_id))?;
        (typing.generation == generation && typing.sending).then(|| typing.wake_driver.clone())
    }

    pub fn telegram_typing_is_active(
        &self,
        thread_id: &str,
        item_id: &str,
        generation: i64,
    ) -> bool {
        self.telegram_typing_by_item
            .get(&telegram_typing_key(thread_id, item_id))
            .is_some_and(|typing| typing.generation == generation)
    }

    #[cfg(test)]
    pub fn telegram_typing_item_is_active(&self, thread_id: &str, item_id: &str) -> bool {
        self.telegram_typing_by_item
            .contains_key(&telegram_typing_key(thread_id, item_id))
    }

    pub fn cancel_telegram_typing_generation(
        &mut self,
        thread_id: &str,
        item_id: &str,
        generation: i64,
    ) -> bool {
        let key = telegram_typing_key(thread_id, item_id);
        if !self
            .telegram_typing_by_item
            .get(&key)
            .is_some_and(|typing| typing.generation == generation)
        {
            return false;
        }
        self.remember_completed_telegram_typing(&key);
        self.remove_telegram_typing(&key);
        true
    }

    pub fn mark_telegram_typing_renewal_due(
        &mut self,
        thread_id: &str,
        item_id: &str,
        generation: i64,
    ) -> bool {
        let Some(typing) = self
            .telegram_typing_by_item
            .get_mut(&telegram_typing_key(thread_id, item_id))
        else {
            return false;
        };
        if typing.generation != generation || !typing.sending {
            return false;
        }
        if typing.finished {
            return true;
        }
        if !typing.dirty {
            typing.dirty = true;
            typing.revision = typing.revision.saturating_add(1);
        }
        true
    }

    /// Force an active typing/thinking driver to send its next heartbeat now.
    ///
    /// Native Telegram drafts can be hidden when a regular message arrives.
    /// Waking the driver lets callers restore the indicator immediately after
    /// that message is delivered instead of waiting for the normal heartbeat.
    #[cfg(test)]
    pub fn wake_telegram_typing(&mut self, thread_id: &str, item_id: &str) -> bool {
        let Some(typing) = self
            .telegram_typing_by_item
            .get_mut(&telegram_typing_key(thread_id, item_id))
        else {
            return false;
        };
        if typing.finished {
            return false;
        }
        if !typing.dirty {
            typing.dirty = true;
            typing.revision = typing.revision.saturating_add(1);
        }
        typing.wake_driver.notify_one();
        true
    }

    pub fn wake_telegram_typing_for_thread(&mut self, thread_id: &str) -> usize {
        let prefix = telegram_typing_thread_prefix(thread_id);
        let mut woken = 0;
        for (key, typing) in &mut self.telegram_typing_by_item {
            if !key.starts_with(&prefix) || typing.finished {
                continue;
            }
            if !typing.dirty {
                typing.dirty = true;
                typing.revision = typing.revision.saturating_add(1);
            }
            typing.wake_driver.notify_one();
            woken += 1;
        }
        woken
    }

    pub fn complete_telegram_typing_send(
        &mut self,
        thread_id: &str,
        item_id: &str,
        generation: i64,
        revision: u64,
        succeeded: bool,
    ) -> TelegramTypingSendAction {
        let key = telegram_typing_key(thread_id, item_id);
        let Some(typing) = self.telegram_typing_by_item.get_mut(&key) else {
            return TelegramTypingSendAction::Stop;
        };
        if typing.generation != generation {
            return TelegramTypingSendAction::Stop;
        }
        if !succeeded {
            if typing.finished {
                if let Some(typing) = self.telegram_typing_by_item.remove(&key) {
                    typing.completed.notify_one();
                }
            } else {
                typing.sending = false;
                typing.dirty = true;
            }
            return TelegramTypingSendAction::Stop;
        }
        if typing.dirty || typing.revision != revision {
            return TelegramTypingSendAction::Continue;
        }
        if typing.finished {
            if let Some(typing) = self.telegram_typing_by_item.remove(&key) {
                typing.completed.notify_one();
            }
            return TelegramTypingSendAction::Stop;
        }
        TelegramTypingSendAction::Continue
    }

    pub fn clear_telegram_typing_for_thread(&mut self, thread_id: &str) {
        let prefix = telegram_typing_thread_prefix(thread_id);
        let keys = self
            .telegram_typing_by_item
            .keys()
            .filter(|key| key.starts_with(&prefix))
            .cloned()
            .collect::<Vec<_>>();
        for key in keys {
            self.remember_completed_telegram_typing(&key);
            self.remove_telegram_typing(&key);
        }
    }

    fn clear_all_telegram_typing(&mut self) {
        self.telegram_typing_suspended_threads.clear();
        let keys = self
            .telegram_typing_by_item
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for key in keys {
            self.remember_completed_telegram_typing(&key);
            self.remove_telegram_typing(&key);
        }
    }

    fn remove_telegram_typing(&mut self, key: &str) {
        if let Some(typing) = self.telegram_typing_by_item.remove(key) {
            typing.wake_driver.notify_one();
            typing.completed.notify_one();
        }
    }

    fn remember_completed_telegram_typing(&mut self, key: &str) {
        if !self.telegram_completed_typing_keys.insert(key.to_string()) {
            return;
        }
        self.telegram_completed_typing_order
            .push_back(key.to_string());
        while self.telegram_completed_typing_order.len() > TELEGRAM_COMPLETED_TYPING_HISTORY_MAX {
            if let Some(oldest) = self.telegram_completed_typing_order.pop_front() {
                self.telegram_completed_typing_keys.remove(&oldest);
            }
        }
    }

    pub(crate) fn upsert_telegram_command_progress(
        &mut self,
        thread_id: &str,
        turn_id: &str,
        entry: TelegramCommandProgressEntry,
        deliver_update: bool,
    ) -> Option<TelegramCommandProgressSnapshot> {
        let progress = self
            .telegram_command_progress_by_thread
            .entry(thread_id.to_string())
            .or_insert_with(|| telegram_command_progress_state(turn_id));
        if progress.turn_id != turn_id {
            *progress = telegram_command_progress_state(turn_id);
        }
        if progress.completed {
            return None;
        }

        let (changed, new_entry) = match progress
            .entries
            .iter_mut()
            .find(|current| current.item_id == entry.item_id)
        {
            Some(current) if *current == entry => (false, false),
            Some(current) => {
                *current = entry;
                (true, false)
            }
            None => {
                progress.entries.push(entry);
                if progress.entries.len() > TELEGRAM_COMMAND_PROGRESS_MAX_ENTRIES {
                    let excess = progress.entries.len() - TELEGRAM_COMMAND_PROGRESS_MAX_ENTRIES;
                    progress.entries.drain(..excess);
                    progress.dropped_entries = progress.dropped_entries.saturating_add(excess);
                }
                (true, true)
            }
        };
        if !changed {
            return None;
        }
        progress.revision = progress.revision.saturating_add(1);
        progress.dirty = true;
        // A newly discovered command must be visible immediately even after
        // the aggregate message has already been created.  Existing entries
        // remain coalesced until completion (or an explicit final flush).
        if deliver_update || new_entry || progress.message_id.is_none() {
            Some(telegram_command_progress_snapshot(progress))
        } else {
            None
        }
    }

    pub(crate) fn upsert_telegram_web_search_progress(
        &mut self,
        thread_id: &str,
        turn_id: &str,
        entry: TelegramWebSearchProgressEntry,
    ) -> Option<TelegramCommandProgressSnapshot> {
        if self.current_turn_id(thread_id) != Some(turn_id) {
            return None;
        }
        let progress = self
            .telegram_command_progress_by_thread
            .entry(thread_id.to_string())
            .or_insert_with(|| telegram_command_progress_state(turn_id));
        if progress.turn_id != turn_id {
            *progress = telegram_command_progress_state(turn_id);
        }
        if progress.completed {
            return None;
        }

        match progress
            .web_searches
            .iter_mut()
            .find(|current| current.item_id == entry.item_id)
        {
            Some(current) if *current == entry => return None,
            Some(current) => *current = entry,
            None => {
                progress.web_searches.push(entry);
                if progress.web_searches.len() > TELEGRAM_WEB_SEARCH_MAX_ENTRIES {
                    let excess = progress.web_searches.len() - TELEGRAM_WEB_SEARCH_MAX_ENTRIES;
                    progress.web_searches.drain(..excess);
                    progress.dropped_web_searches =
                        progress.dropped_web_searches.saturating_add(excess);
                }
            }
        }
        progress.revision = progress.revision.saturating_add(1);
        progress.dirty = true;
        Some(telegram_command_progress_snapshot(progress))
    }

    pub(crate) fn record_telegram_retry(
        &mut self,
        thread_id: &str,
        turn_id: &str,
        error: Option<String>,
    ) -> Option<TelegramCommandProgressSnapshot> {
        if self.current_turn_id(thread_id) != Some(turn_id) {
            return None;
        }
        let progress = self
            .telegram_command_progress_by_thread
            .entry(thread_id.to_string())
            .or_insert_with(|| telegram_command_progress_state(turn_id));
        if progress.turn_id != turn_id {
            *progress = telegram_command_progress_state(turn_id);
        }
        if progress.completed {
            return None;
        }

        progress.retry_count = progress.retry_count.saturating_add(1);
        if let Some(error) = error.filter(|value| !value.trim().is_empty()) {
            progress.retry_error = Some(error);
        }
        progress.revision = progress.revision.saturating_add(1);
        progress.dirty = true;
        Some(telegram_command_progress_snapshot(progress))
    }

    /// Cache reasoning deltas without touching Telegram for every token-sized
    /// notification. The completed reasoning item flushes this buffer into the
    /// aggregate progress message.
    pub(crate) fn append_telegram_reasoning_delta(
        &mut self,
        thread_id: &str,
        turn_id: &str,
        item_id: &str,
        summary_index: i64,
        delta: &str,
    ) {
        if delta.is_empty() || self.current_turn_id(thread_id) != Some(turn_id) {
            return;
        }
        let progress = self
            .telegram_command_progress_by_thread
            .entry(thread_id.to_string())
            .or_insert_with(|| telegram_command_progress_state(turn_id));
        if progress.turn_id != turn_id || progress.completed {
            return;
        }
        if progress.reasoning_item_id.as_deref() != Some(item_id) {
            progress.reasoning_item_id = Some(item_id.to_string());
            progress.reasoning_summary_index = Some(summary_index);
            progress.reasoning_summary.clear();
            progress.reasoning_completed = false;
        } else if progress
            .reasoning_summary_index
            .is_some_and(|current| summary_index < current)
        {
            return;
        } else if progress.reasoning_summary_index != Some(summary_index) {
            progress.reasoning_summary_index = Some(summary_index);
            progress.reasoning_summary.clear();
        }
        append_bounded_text(
            &mut progress.reasoning_summary,
            delta,
            TELEGRAM_REASONING_MAX_CHARS,
        );
        progress.revision = progress.revision.saturating_add(1);
        progress.dirty = true;
    }

    pub(crate) fn complete_telegram_reasoning(
        &mut self,
        thread_id: &str,
        turn_id: &str,
        item_id: &str,
        summary: Option<String>,
    ) -> Option<TelegramCommandProgressSnapshot> {
        if self.current_turn_id(thread_id) != Some(turn_id) {
            return None;
        }
        let progress = self
            .telegram_command_progress_by_thread
            .get_mut(thread_id)?;
        if progress.turn_id != turn_id || progress.completed {
            return None;
        }
        let mut content_changed = false;
        if let Some(summary) = summary.filter(|value| !value.trim().is_empty()) {
            content_changed = progress.reasoning_item_id.as_deref() != Some(item_id)
                || progress.reasoning_summary != summary;
            progress.reasoning_item_id = Some(item_id.to_string());
            progress.reasoning_summary_index = None;
            progress.reasoning_summary = summary;
        } else if progress.reasoning_item_id.as_deref() != Some(item_id) {
            return None;
        }
        if progress.reasoning_summary.trim().is_empty() {
            return None;
        }
        let completion_changed = !progress.reasoning_completed;
        if !content_changed && !completion_changed {
            return None;
        }
        progress.reasoning_completed = true;
        progress.revision = progress.revision.saturating_add(1);
        progress.dirty = true;
        Some(telegram_command_progress_snapshot(progress))
    }

    pub(crate) fn update_telegram_plan(
        &mut self,
        thread_id: &str,
        turn_id: &str,
        explanation: Option<String>,
        plan: Vec<TelegramPlanStep>,
        deliver_update: bool,
    ) -> Option<TelegramCommandProgressSnapshot> {
        if self.current_turn_id(thread_id) != Some(turn_id) {
            return None;
        }
        let progress = self
            .telegram_command_progress_by_thread
            .entry(thread_id.to_string())
            .or_insert_with(|| telegram_command_progress_state(turn_id));
        if progress.turn_id != turn_id || progress.completed {
            return None;
        }
        let explanation = explanation.filter(|value| !value.trim().is_empty());
        let plan = plan
            .into_iter()
            .take(TELEGRAM_PLAN_MAX_STEPS)
            .collect::<Vec<_>>();
        if progress.plan_explanation == explanation && progress.plan == plan {
            return None;
        }
        progress.plan_explanation = explanation;
        progress.plan = plan;
        if progress.message_id.is_none() && !telegram_command_progress_has_content(progress) {
            progress.dirty = false;
            return None;
        }
        progress.revision = progress.revision.saturating_add(1);
        progress.dirty = true;
        deliver_update.then(|| telegram_command_progress_snapshot(progress))
    }

    pub(crate) fn update_telegram_diff(
        &mut self,
        thread_id: &str,
        turn_id: &str,
        diff_summary: Option<TelegramDiffSummary>,
        deliver_update: bool,
    ) -> Option<TelegramCommandProgressSnapshot> {
        if self.current_turn_id(thread_id) != Some(turn_id) {
            return None;
        }
        let progress = self
            .telegram_command_progress_by_thread
            .entry(thread_id.to_string())
            .or_insert_with(|| telegram_command_progress_state(turn_id));
        if progress.turn_id != turn_id || progress.completed {
            return None;
        }
        let Some(diff_summary) = diff_summary else {
            if progress.diff_summary.take().is_some() {
                progress.revision = progress.revision.saturating_add(1);
                progress.dirty = true;
                return deliver_update.then(|| telegram_command_progress_snapshot(progress));
            }
            return None;
        };
        let diff_summary = limit_telegram_diff_summary(diff_summary);
        if progress.diff_summary.as_ref() == Some(&diff_summary) {
            return None;
        }
        progress.diff_summary = Some(diff_summary);
        progress.revision = progress.revision.saturating_add(1);
        progress.dirty = true;
        deliver_update.then(|| telegram_command_progress_snapshot(progress))
    }

    /// Flush a file-change completion into the aggregate message. Turn-level
    /// diff notifications are cached first; the item fallback is only used
    /// when no aggregate diff has arrived yet.
    pub(crate) fn complete_telegram_file_change(
        &mut self,
        thread_id: &str,
        turn_id: &str,
        fallback: Option<TelegramDiffSummary>,
    ) -> Option<TelegramCommandProgressSnapshot> {
        if self.current_turn_id(thread_id) != Some(turn_id) {
            return None;
        }
        let progress = self
            .telegram_command_progress_by_thread
            .entry(thread_id.to_string())
            .or_insert_with(|| telegram_command_progress_state(turn_id));
        if progress.turn_id != turn_id || progress.completed {
            return None;
        }
        if progress.diff_summary.is_none()
            && let Some(fallback) = fallback
        {
            progress.diff_summary = Some(limit_telegram_diff_summary(fallback));
            progress.revision = progress.revision.saturating_add(1);
            progress.dirty = true;
        }
        progress
            .dirty
            .then(|| telegram_command_progress_snapshot(progress))
    }

    pub(crate) fn finish_telegram_command_progress(
        &mut self,
        thread_id: &str,
        turn_id: &str,
    ) -> Option<TelegramCommandProgressSnapshot> {
        self.finish_telegram_command_progress_with_outcome(thread_id, turn_id, false)
    }

    pub(crate) fn finish_telegram_command_progress_with_outcome(
        &mut self,
        thread_id: &str,
        turn_id: &str,
        failed: bool,
    ) -> Option<TelegramCommandProgressSnapshot> {
        let progress = self
            .telegram_command_progress_by_thread
            .get_mut(thread_id)?;
        if progress.turn_id != turn_id || !telegram_command_progress_has_content(progress) {
            return None;
        }
        let interrupted = progress
            .entries
            .iter_mut()
            .filter(|entry| entry.status == TelegramCommandProgressStatus::Running)
            .map(|entry| {
                entry.status = TelegramCommandProgressStatus::Interrupted;
            })
            .count();
        let now_ms = crate::types::now_ms();
        let interrupted_collab = progress
            .collab_entries
            .iter_mut()
            .filter(|entry| entry.status == TelegramCollabProgressStatus::Running)
            .map(|entry| {
                entry.status = TelegramCollabProgressStatus::Interrupted;
                entry.updated_at_ms = entry.updated_at_ms.max(now_ms).max(entry.started_at_ms);
            })
            .count();
        if failed {
            progress.failed = true;
        }
        if !progress.completed || interrupted > 0 || interrupted_collab > 0 || failed {
            progress.completed = true;
            progress.revision = progress.revision.saturating_add(1);
            progress.dirty = true;
        }
        progress
            .dirty
            .then(|| telegram_command_progress_snapshot(progress))
    }

    pub(crate) fn claim_telegram_command_progress_delivery(
        &mut self,
        thread_id: &str,
        turn_id: &str,
    ) -> Option<TelegramCommandProgressSnapshot> {
        let current_turn_id = self.current_turn_id(thread_id).map(str::to_string);
        let progress = self
            .telegram_command_progress_by_thread
            .get_mut(thread_id)?;
        if progress.turn_id != turn_id
            || progress.sending
            || !progress.dirty
            || (current_turn_id.as_deref() != Some(turn_id) && !progress.cleanup_after_delivery)
        {
            return None;
        }
        progress.sending = true;
        claim_next_telegram_command_progress_snapshot(progress)
    }

    pub(crate) fn telegram_command_progress_delivery_is_current(
        &self,
        thread_id: &str,
        turn_id: &str,
        revision: u64,
    ) -> bool {
        let Some(progress) = self.telegram_command_progress_by_thread.get(thread_id) else {
            return false;
        };
        progress.turn_id == turn_id
            && progress.sending
            && progress.in_flight_revision == Some(revision)
            && (self.current_turn_id(thread_id) == Some(turn_id) || progress.cleanup_after_delivery)
    }

    pub(crate) fn complete_telegram_command_progress_delivery(
        &mut self,
        thread_id: &str,
        turn_id: &str,
        revision: u64,
        message_id: String,
    ) -> Option<TelegramCommandProgressSnapshot> {
        let mut cleanup = false;
        let next = {
            let progress = self
                .telegram_command_progress_by_thread
                .get_mut(thread_id)?;
            if progress.turn_id != turn_id
                || !progress.sending
                || progress.in_flight_revision != Some(revision)
            {
                return None;
            }
            progress.message_id = Some(message_id);
            progress.in_flight_revision = None;
            if progress.dirty {
                claim_next_telegram_command_progress_snapshot(progress)
            } else {
                progress.sending = false;
                cleanup = progress.completed && progress.cleanup_after_delivery;
                None
            }
        };
        if cleanup {
            self.telegram_command_progress_by_thread.remove(thread_id);
        }
        next
    }

    pub(crate) fn fail_telegram_command_progress_delivery(
        &mut self,
        thread_id: &str,
        turn_id: &str,
        revision: u64,
    ) -> bool {
        let Some(progress) = self.telegram_command_progress_by_thread.get_mut(thread_id) else {
            return false;
        };
        if progress.turn_id != turn_id
            || !progress.sending
            || progress.in_flight_revision != Some(revision)
        {
            return false;
        }
        progress.in_flight_revision = None;
        progress.dirty = true;
        if progress.completed {
            true
        } else {
            progress.sending = false;
            false
        }
    }

    pub(crate) fn retry_telegram_command_progress_delivery(
        &mut self,
        thread_id: &str,
        turn_id: &str,
    ) -> Option<TelegramCommandProgressSnapshot> {
        let progress = self
            .telegram_command_progress_by_thread
            .get_mut(thread_id)?;
        if progress.turn_id != turn_id
            || !progress.sending
            || progress.in_flight_revision.is_some()
            || !progress.dirty
        {
            return None;
        }
        claim_next_telegram_command_progress_snapshot(progress)
    }

    #[cfg(test)]
    pub(crate) fn remember_telegram_command_progress_delivery(
        &mut self,
        thread_id: &str,
        turn_id: &str,
        revision: u64,
        message_id: String,
    ) {
        let Some(progress) = self.telegram_command_progress_by_thread.get_mut(thread_id) else {
            return;
        };
        if progress.turn_id != turn_id {
            return;
        }
        progress.message_id = Some(message_id);
        if progress.revision == revision {
            progress.dirty = false;
        }
    }

    pub(crate) fn clear_telegram_command_progress_for_thread(&mut self, thread_id: &str) {
        self.telegram_command_progress_by_thread.remove(thread_id);
    }

    pub(crate) fn upsert_telegram_collab_task_progress(
        &mut self,
        thread_id: &str,
        turn_id: &str,
        updates: Vec<TelegramCollabProgressUpdate>,
    ) -> Option<TelegramCommandProgressSnapshot> {
        if updates.is_empty() || self.current_turn_id(thread_id) != Some(turn_id) {
            return None;
        }
        let progress = self
            .telegram_command_progress_by_thread
            .entry(thread_id.to_string())
            .or_insert_with(|| telegram_command_progress_state(turn_id));
        if progress.turn_id != turn_id {
            *progress = telegram_command_progress_state(turn_id);
        }
        if progress.completed
            || !merge_telegram_collab_progress_updates(
                &mut progress.collab_entries,
                &mut progress.collab_dropped_entries,
                updates,
            )
        {
            return None;
        }
        progress.revision = progress.revision.saturating_add(1);
        progress.dirty = true;
        Some(telegram_command_progress_snapshot(progress))
    }

    pub fn hold_pending_attachments(
        &mut self,
        conversation_key: &str,
        attachments: Vec<InboundAttachment>,
        received_at_ms: u128,
    ) -> usize {
        let entry = self
            .pending_attachments_by_conversation
            .entry(conversation_key.to_string())
            .or_insert_with(|| PendingAttachments {
                attachments: Vec::new(),
                received_at_ms,
            });
        if received_at_ms.saturating_sub(entry.received_at_ms) > PENDING_ATTACHMENTS_MAX_AGE_MS {
            entry.attachments.clear();
        }
        entry.received_at_ms = received_at_ms;
        entry.attachments.extend(attachments);
        if entry.attachments.len() > PENDING_ATTACHMENTS_MAX_COUNT {
            let excess = entry.attachments.len() - PENDING_ATTACHMENTS_MAX_COUNT;
            entry.attachments.drain(..excess);
        }
        entry.attachments.len()
    }

    pub fn take_pending_attachments(
        &mut self,
        conversation_key: &str,
        received_at_ms: u128,
    ) -> Vec<InboundAttachment> {
        self.pending_attachments_by_conversation
            .remove(conversation_key)
            .filter(|entry| {
                received_at_ms.saturating_sub(entry.received_at_ms)
                    <= PENDING_ATTACHMENTS_MAX_AGE_MS
            })
            .map(|entry| entry.attachments)
            .unwrap_or_default()
    }

    pub(crate) fn enqueue_telegram_turn(
        &mut self,
        conversation_key: &str,
        turn: PendingTelegramTurn,
    ) -> Option<usize> {
        let queue = self
            .pending_telegram_turns_by_conversation
            .entry(conversation_key.to_string())
            .or_default();
        if queue.len() >= TELEGRAM_QUEUED_TURNS_MAX_COUNT {
            return None;
        }
        queue.push_back(turn);
        Some(queue.len())
    }

    pub(crate) fn enqueue_telegram_turn_if_active(
        &mut self,
        conversation_key: &str,
        turn: PendingTelegramTurn,
    ) -> TelegramQueueEnqueueOutcome {
        let Some(thread_id) = self.route_by_thread.iter().find_map(|(thread_id, route)| {
            (route.conversation_key == conversation_key).then(|| thread_id.clone())
        }) else {
            return TelegramQueueEnqueueOutcome::NotRunning;
        };
        if !self.turn_in_progress(&thread_id) {
            return TelegramQueueEnqueueOutcome::NotRunning;
        }
        match self.enqueue_telegram_turn(conversation_key, turn) {
            Some(position) => TelegramQueueEnqueueOutcome::Added(position),
            None => TelegramQueueEnqueueOutcome::Full,
        }
    }

    pub(crate) fn take_next_telegram_turn(
        &mut self,
        conversation_key: &str,
    ) -> Option<PendingTelegramTurn> {
        let queue = self
            .pending_telegram_turns_by_conversation
            .get_mut(conversation_key)?;
        let turn = queue.pop_front();
        if queue.is_empty() {
            self.pending_telegram_turns_by_conversation
                .remove(conversation_key);
        }
        turn
    }

    pub(crate) fn requeue_telegram_turn_front(
        &mut self,
        conversation_key: &str,
        turn: PendingTelegramTurn,
    ) {
        let queue = self
            .pending_telegram_turns_by_conversation
            .entry(conversation_key.to_string())
            .or_default();
        queue.push_front(turn);
    }

    pub(crate) fn telegram_queue_len(&self, conversation_key: &str) -> usize {
        self.pending_telegram_turns_by_conversation
            .get(conversation_key)
            .map(VecDeque::len)
            .unwrap_or(0)
    }

    pub(crate) fn clear_telegram_queue(&mut self, conversation_key: &str) {
        self.pending_telegram_turns_by_conversation
            .remove(conversation_key);
    }

    pub fn mark_turn_completed(&mut self, thread_id: &str, turn_id: Option<&str>) -> bool {
        let current_turn_id = self
            .current_turn_by_thread
            .get(thread_id)
            .map(String::as_str);
        if let Some(turn_id) = turn_id
            && current_turn_id != Some(turn_id)
        {
            self.turn_origin_by_id.remove(turn_id);
            return false;
        }
        self.terminal_status_fallback_by_thread.remove(thread_id);
        self.starting_turn_by_thread.remove(thread_id);
        let completed_turn_id = self.current_turn_by_thread.remove(thread_id);
        if let Some(turn_id) = turn_id.or(completed_turn_id.as_deref()) {
            self.turn_origin_by_id.remove(turn_id);
        }
        self.clear_telegram_typing_for_thread(thread_id);
        let retain_command_progress =
            if let Some(progress) = self.telegram_command_progress_by_thread.get_mut(thread_id) {
                if progress.completed && progress.sending {
                    progress.cleanup_after_delivery = true;
                    true
                } else {
                    false
                }
            } else {
                false
            };
        if !retain_command_progress {
            self.clear_telegram_command_progress_for_thread(thread_id);
        }
        if let Some(turn_id) = turn_id.or(completed_turn_id.as_deref())
            && let Some(commentary) = self.telegram_commentary_by_thread.get_mut(thread_id)
            && commentary.turn_id == turn_id
        {
            commentary.completed = true;
        }
        self.turn_finished_at_by_thread
            .insert(thread_id.to_string(), crate::types::now_ms());
        true
    }

    pub fn route_for_thread(&self, thread_id: &str) -> Option<RouteTarget> {
        self.route_by_thread.get(thread_id).cloned()
    }

    pub fn has_pending_approvals(&self, conversation_key: &str) -> bool {
        self.pending_approvals_by_conversation
            .get(conversation_key)
            .is_some_and(|approvals| !approvals.is_empty())
    }

    pub fn push_approval(&mut self, conversation_key: String, approval: PendingApproval) -> bool {
        let request_key = approval.request_key();
        if !self.pending_approval_request_keys.insert(request_key) {
            return false;
        }
        self.pending_approvals_by_conversation
            .entry(conversation_key)
            .or_default()
            .push(approval);
        true
    }

    pub fn current_approval(&self, conversation_key: &str) -> Option<PendingApproval> {
        self.pending_approvals_by_conversation
            .get(conversation_key)
            .and_then(|approvals| approvals.first())
            .cloned()
    }

    pub fn is_current_approval(&self, conversation_key: &str, request_key: &str) -> bool {
        self.current_approval(conversation_key)
            .is_some_and(|approval| approval.request_key() == request_key)
    }

    pub fn approval_by_request_key_anywhere(
        &self,
        request_key: &str,
    ) -> Option<(String, PendingApproval)> {
        self.pending_approvals_by_conversation
            .iter()
            .find_map(|(conversation_key, approvals)| {
                approvals
                    .iter()
                    .find(|approval| approval.request_key() == request_key)
                    .cloned()
                    .map(|approval| (conversation_key.clone(), approval))
            })
    }

    pub fn remember_approval_message_id(&mut self, request_id: &Value, message_id: String) -> bool {
        let request_key = approval_request_key(request_id);
        for approvals in self.pending_approvals_by_conversation.values_mut() {
            if let Some(approval) = approvals
                .iter_mut()
                .find(|approval| approval.request_key() == request_key)
            {
                approval.message_id = Some(message_id);
                return true;
            }
        }
        false
    }

    #[cfg(test)]
    pub fn resolve_approval_request(&mut self, request_id: &Value) -> Option<PendingApproval> {
        self.resolve_approval_request_with_context(request_id)
            .map(|resolved| resolved.approval)
    }

    pub fn resolve_approval_request_with_context(
        &mut self,
        request_id: &Value,
    ) -> Option<ResolvedApproval> {
        let request_key = approval_request_key(request_id);
        self.pending_approval_request_keys.remove(&request_key);

        let mut resolved = None;
        let mut empty_key = None;
        for (conversation_key, approvals) in &mut self.pending_approvals_by_conversation {
            if let Some(index) = approvals
                .iter()
                .position(|approval| approval.request_key() == request_key)
            {
                let approval = approvals.remove(index);
                let was_current = index == 0;
                let next_current = was_current.then(|| approvals.first().cloned()).flatten();
                if approvals.is_empty() {
                    empty_key = Some(conversation_key.clone());
                }
                resolved = Some(ResolvedApproval {
                    conversation_key: conversation_key.clone(),
                    approval,
                    was_current,
                    next_current,
                });
                break;
            }
        }
        if let Some(conversation_key) = empty_key {
            self.pending_approvals_by_conversation
                .remove(&conversation_key);
        }
        resolved
    }

    pub fn remember_thread_routing_request(&mut self, request: ThreadRoutingRequestState) {
        // A conversation can have only one active interactive menu. Opening
        // a thread-routing menu invalidates any model picker that was shown
        // before it, so numeric replies cannot be consumed by the wrong flow.
        self.clear_telegram_model_switch_requests_for_conversation(&request.conversation_key);
        self.thread_routing_requests
            .insert(request.request_id.clone(), request);
    }

    pub fn thread_routing_request(&self, request_id: &str) -> Option<ThreadRoutingRequestState> {
        self.thread_routing_requests.get(request_id).cloned()
    }

    pub fn update_thread_routing_request_message_id(
        &mut self,
        request_id: &str,
        message_id: String,
    ) -> bool {
        let Some(request) = self.thread_routing_requests.get_mut(request_id) else {
            return false;
        };
        request.message_id = Some(message_id);
        true
    }

    pub fn clear_thread_routing_request(
        &mut self,
        request_id: &str,
    ) -> Option<ThreadRoutingRequestState> {
        self.thread_routing_requests.remove(request_id)
    }

    pub fn clear_thread_routing_requests_for_conversation(&mut self, conversation_key: &str) {
        let request_ids = self
            .thread_routing_requests
            .iter()
            .filter(|(_, request)| request.conversation_key == conversation_key)
            .map(|(request_id, _)| request_id.clone())
            .collect::<Vec<_>>();
        for request_id in request_ids {
            self.thread_routing_requests.remove(&request_id);
        }
    }

    /// Remember the latest staged thread-settings editor for a conversation.
    /// Replacing it removes older request ids so delayed callbacks cannot
    /// mutate a newer draft.
    pub fn remember_telegram_model_switch_request(
        &mut self,
        request: TelegramModelSwitchRequestState,
    ) {
        // Keep model and thread-routing menus mutually exclusive within one
        // Telegram conversation.
        self.clear_thread_routing_requests_for_conversation(&request.conversation_key);
        if let Some(previous) = self
            .telegram_model_switch_requests
            .remove(&request.request_id)
            && self
                .telegram_model_switch_request_by_conversation
                .get(&previous.conversation_key)
                .is_some_and(|current_id| current_id == &request.request_id)
        {
            self.telegram_model_switch_request_by_conversation
                .remove(&previous.conversation_key);
        }
        self.clear_telegram_model_switch_requests_for_conversation(&request.conversation_key);
        self.telegram_model_switch_request_by_conversation
            .insert(request.conversation_key.clone(), request.request_id.clone());
        self.telegram_model_switch_requests
            .insert(request.request_id.clone(), request);
    }

    pub fn telegram_model_switch_request(
        &self,
        request_id: &str,
    ) -> Option<TelegramModelSwitchRequestState> {
        self.telegram_model_switch_requests.get(request_id).cloned()
    }

    pub fn current_telegram_model_switch_request(
        &self,
        conversation_key: &str,
    ) -> Option<TelegramModelSwitchRequestState> {
        let request_id = self
            .telegram_model_switch_request_by_conversation
            .get(conversation_key)?;
        self.telegram_model_switch_requests.get(request_id).cloned()
    }

    pub fn thread_settings_snapshot(&self, thread_id: &str) -> ThreadSettingsSnapshot {
        self.thread_settings_by_thread
            .get(thread_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn observe_thread_settings(
        &mut self,
        thread_id: &str,
        update: ThreadSettingsSnapshot,
    ) -> TelegramThreadSettingsObservation {
        let previous = self.thread_settings_snapshot(thread_id);
        let mut merged = previous.clone();
        merged.merge_from(&update);
        self.thread_settings_by_thread
            .insert(thread_id.to_string(), merged.clone());

        let Some(request_id) =
            self.telegram_model_switch_requests
                .iter()
                .find_map(|(request_id, request)| {
                    (request.expected_thread_id == thread_id).then(|| request_id.clone())
                })
        else {
            return TelegramThreadSettingsObservation::None;
        };
        let Some(request) = self.telegram_model_switch_requests.get_mut(&request_id) else {
            return TelegramThreadSettingsObservation::None;
        };
        request.observed = merged.clone();
        if let Some(pending) = request.pending_apply.as_ref()
            && telegram_thread_settings_patch_matches(&pending.patch, &merged)
        {
            let request = self
                .clear_telegram_model_switch_request(&request_id)
                .expect("matching request must remain registered");
            return TelegramThreadSettingsObservation::Confirmed(request);
        }
        if !request.stale
            && telegram_thread_settings_draft_was_changed_externally(
                &request.draft,
                &previous,
                &update,
            )
        {
            request.stale = true;
            request.pending_apply = None;
            request.compatibility = None;
            request.stage = TelegramThreadSettingsStage::Overview;
            request.revision = request.revision.saturating_add(1);
            return TelegramThreadSettingsObservation::Stale(request.clone());
        }
        TelegramThreadSettingsObservation::None
    }

    pub fn update_telegram_model_switch_request(
        &mut self,
        request: TelegramModelSwitchRequestState,
    ) -> bool {
        if !self.is_current_telegram_model_switch_request(
            &request.request_id,
            &request.conversation_key,
        ) {
            return false;
        }
        self.telegram_model_switch_requests
            .insert(request.request_id.clone(), request);
        true
    }

    pub fn claim_telegram_thread_settings_apply(
        &mut self,
        request_id: &str,
        revision: u64,
        patch: TelegramThreadSettingsPatch,
        submitted_at_ms: u128,
    ) -> Option<TelegramModelSwitchRequestState> {
        let request = self.telegram_model_switch_requests.get_mut(request_id)?;
        if request.revision != revision || request.pending_apply.is_some() || request.stale {
            return None;
        }
        request.pending_apply = Some(PendingTelegramThreadSettingsApply {
            patch,
            submitted_at_ms,
        });
        Some(request.clone())
    }

    pub fn take_unconfirmed_telegram_thread_settings_apply(
        &mut self,
        request_id: &str,
        revision: u64,
    ) -> Option<TelegramModelSwitchRequestState> {
        let request = self.telegram_model_switch_requests.get(request_id)?;
        if request.revision != revision || request.pending_apply.is_none() {
            return None;
        }
        self.clear_telegram_model_switch_request(request_id)
    }

    pub fn release_telegram_thread_settings_apply(
        &mut self,
        request_id: &str,
        revision: u64,
    ) -> Option<TelegramModelSwitchRequestState> {
        let request = self.telegram_model_switch_requests.get_mut(request_id)?;
        if request.revision != revision || request.pending_apply.is_none() {
            return None;
        }
        request.pending_apply = None;
        Some(request.clone())
    }

    pub fn update_telegram_model_switch_request_message_id(
        &mut self,
        request_id: &str,
        message_id: String,
    ) -> bool {
        let Some(request) = self.telegram_model_switch_requests.get_mut(request_id) else {
            return false;
        };
        request.message_id = Some(message_id);
        true
    }

    /// Return whether a picker request is still the latest one for a
    /// conversation. Callers should additionally compare its expected thread
    /// and remote client key with the current route before mutating a thread.
    pub fn is_current_telegram_model_switch_request(
        &self,
        request_id: &str,
        conversation_key: &str,
    ) -> bool {
        self.telegram_model_switch_request_by_conversation
            .get(conversation_key)
            .is_some_and(|current_id| current_id == request_id)
    }

    pub fn clear_telegram_model_switch_request(
        &mut self,
        request_id: &str,
    ) -> Option<TelegramModelSwitchRequestState> {
        let request = self.telegram_model_switch_requests.remove(request_id)?;
        if self
            .telegram_model_switch_request_by_conversation
            .get(&request.conversation_key)
            .is_some_and(|current_id| current_id == request_id)
        {
            self.telegram_model_switch_request_by_conversation
                .remove(&request.conversation_key);
        }
        Some(request)
    }

    pub fn clear_telegram_model_switch_requests_for_conversation(
        &mut self,
        conversation_key: &str,
    ) {
        let request_ids = self
            .telegram_model_switch_requests
            .iter()
            .filter(|(_, request)| request.conversation_key == conversation_key)
            .map(|(request_id, _)| request_id.clone())
            .collect::<Vec<_>>();
        for request_id in request_ids {
            self.telegram_model_switch_requests.remove(&request_id);
        }
        self.telegram_model_switch_request_by_conversation
            .remove(conversation_key);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TelegramThreadSettingsObservation {
    None,
    Confirmed(TelegramModelSwitchRequestState),
    Stale(TelegramModelSwitchRequestState),
}

fn telegram_thread_settings_patch_matches(
    patch: &TelegramThreadSettingsPatch,
    snapshot: &ThreadSettingsSnapshot,
) -> bool {
    patch_value_matches(&patch.model, &snapshot.model)
        && patch_value_matches(&patch.effort, &snapshot.effort)
        && patch_value_matches(&patch.service_tier, &snapshot.service_tier)
}

fn patch_value_matches(
    patch: &TelegramThreadSettingsPatchValue,
    observed: &ObservedSetting<String>,
) -> bool {
    match patch {
        TelegramThreadSettingsPatchValue::Unchanged => true,
        TelegramThreadSettingsPatchValue::Set(expected) => {
            matches!(observed, ObservedSetting::Known(Some(actual)) if actual == expected)
        }
        TelegramThreadSettingsPatchValue::Clear => {
            matches!(observed, ObservedSetting::Known(None))
        }
    }
}

fn telegram_thread_settings_draft_was_changed_externally(
    draft: &TelegramThreadSettingsDraft,
    previous: &ThreadSettingsSnapshot,
    update: &ThreadSettingsSnapshot,
) -> bool {
    draft.model.is_some() && observed_setting_changed(&previous.model, &update.model)
        || draft.effort.is_some() && observed_setting_changed(&previous.effort, &update.effort)
        || draft.speed.is_some()
            && observed_setting_changed(&previous.service_tier, &update.service_tier)
}

fn observed_setting_changed<T: PartialEq>(
    previous: &ObservedSetting<T>,
    update: &ObservedSetting<T>,
) -> bool {
    matches!(previous, ObservedSetting::Known(_))
        && matches!(update, ObservedSetting::Known(_))
        && previous != update
}

fn merge_telegram_collab_progress_updates(
    entries: &mut Vec<TelegramCollabProgressEntry>,
    dropped_entries: &mut usize,
    updates: Vec<TelegramCollabProgressUpdate>,
) -> bool {
    let mut changed = false;
    for update in updates {
        let timestamp = update.occurred_at_ms.max(1);
        if let Some(entry) = entries
            .iter_mut()
            .find(|entry| entry.agent_id == update.agent_id)
        {
            if timestamp < entry.updated_at_ms {
                continue;
            }
            let mut entry_changed = false;
            let mut clear_stale_detail = false;
            if let Some(name) = update.name.filter(|name| !name.trim().is_empty())
                && entry.name != name
            {
                entry.name = name;
                entry_changed = true;
            }
            if let Some(status) = update.status {
                let should_change = if entry.status == status {
                    false
                } else if update.restart && status == TelegramCollabProgressStatus::Running {
                    true
                } else {
                    match entry.status {
                        TelegramCollabProgressStatus::Running => true,
                        TelegramCollabProgressStatus::Responded => matches!(
                            status,
                            TelegramCollabProgressStatus::Succeeded
                                | TelegramCollabProgressStatus::Failed
                        ),
                        TelegramCollabProgressStatus::Succeeded
                        | TelegramCollabProgressStatus::Interrupted => {
                            status == TelegramCollabProgressStatus::Failed
                        }
                        TelegramCollabProgressStatus::Failed => false,
                    }
                };
                clear_stale_detail = (entry.status == status || should_change)
                    && (status == TelegramCollabProgressStatus::Succeeded
                        || (update.restart && status == TelegramCollabProgressStatus::Running));
                if should_change {
                    if status == TelegramCollabProgressStatus::Running {
                        entry.started_at_ms = timestamp;
                    }
                    entry.status = status;
                    entry_changed = true;
                }
            }
            if let Some(detail) = update.detail.filter(|detail| !detail.trim().is_empty()) {
                if entry.detail.as_deref() != Some(detail.as_str()) {
                    entry.detail = Some(detail);
                    entry_changed = true;
                }
            } else if clear_stale_detail && entry.detail.take().is_some() {
                entry_changed = true;
            }
            if entry_changed {
                entry.updated_at_ms = entry.updated_at_ms.max(timestamp).max(entry.started_at_ms);
                changed = true;
            }
            continue;
        }

        if !update.create_if_missing {
            continue;
        }
        let Some(name) = update.name.filter(|name| !name.trim().is_empty()) else {
            continue;
        };
        entries.push(TelegramCollabProgressEntry {
            agent_id: update.agent_id,
            name,
            status: update
                .status
                .unwrap_or(TelegramCollabProgressStatus::Running),
            detail: update.detail.filter(|detail| !detail.trim().is_empty()),
            started_at_ms: timestamp,
            updated_at_ms: timestamp,
        });
        if entries.len() > TELEGRAM_COLLAB_PROGRESS_MAX_AGENTS {
            let excess = entries.len() - TELEGRAM_COLLAB_PROGRESS_MAX_AGENTS;
            entries.drain(..excess);
            *dropped_entries = dropped_entries.saturating_add(excess);
        }
        changed = true;
    }
    changed
}

fn telegram_command_progress_snapshot(
    progress: &TelegramCommandProgressState,
) -> TelegramCommandProgressSnapshot {
    TelegramCommandProgressSnapshot {
        turn_id: progress.turn_id.clone(),
        revision: progress.revision,
        message_id: progress.message_id.clone(),
        entries: progress.entries.clone(),
        dropped_entries: progress.dropped_entries,
        retry_count: progress.retry_count,
        retry_error: progress.retry_error.clone(),
        reasoning_summary: progress
            .reasoning_completed
            .then(|| progress.reasoning_summary.clone())
            .filter(|value| !value.trim().is_empty()),
        plan_explanation: progress.plan_explanation.clone(),
        plan: progress.plan.clone(),
        diff_summary: progress.diff_summary.clone(),
        web_searches: progress.web_searches.clone(),
        dropped_web_searches: progress.dropped_web_searches,
        collab: (!progress.collab_entries.is_empty()).then(|| TelegramCollabProgressSnapshot {
            entries: progress.collab_entries.clone(),
            dropped_entries: progress.collab_dropped_entries,
            completed: progress.completed,
        }),
        completed: progress.completed,
        failed: progress.failed,
    }
}

fn telegram_command_progress_state(turn_id: &str) -> TelegramCommandProgressState {
    TelegramCommandProgressState {
        turn_id: turn_id.to_string(),
        revision: 0,
        message_id: None,
        entries: Vec::new(),
        dropped_entries: 0,
        retry_count: 0,
        retry_error: None,
        reasoning_item_id: None,
        reasoning_summary_index: None,
        reasoning_summary: String::new(),
        reasoning_completed: false,
        plan_explanation: None,
        plan: Vec::new(),
        diff_summary: None,
        web_searches: Vec::new(),
        dropped_web_searches: 0,
        collab_entries: Vec::new(),
        collab_dropped_entries: 0,
        completed: false,
        failed: false,
        dirty: false,
        sending: false,
        in_flight_revision: None,
        cleanup_after_delivery: false,
    }
}

fn telegram_command_progress_has_content(progress: &TelegramCommandProgressState) -> bool {
    !progress.entries.is_empty()
        || progress.retry_count > 0
        || (progress.reasoning_completed && !progress.reasoning_summary.trim().is_empty())
        || progress.plan_explanation.is_some()
        || !progress.plan.is_empty()
        || progress.diff_summary.is_some()
        || !progress.web_searches.is_empty()
        || !progress.collab_entries.is_empty()
}

fn claim_next_telegram_command_progress_snapshot(
    progress: &mut TelegramCommandProgressState,
) -> Option<TelegramCommandProgressSnapshot> {
    if !progress.sending || progress.in_flight_revision.is_some() || !progress.dirty {
        return None;
    }
    progress.dirty = false;
    progress.in_flight_revision = Some(progress.revision);
    Some(telegram_command_progress_snapshot(progress))
}

fn telegram_commentary_snapshot(
    turn_id: &str,
    segment: u64,
    commentary: &TelegramCommentarySegmentState,
) -> TelegramCommentarySnapshot {
    TelegramCommentarySnapshot {
        turn_id: turn_id.to_string(),
        segment,
        message_id: commentary.message_id.clone(),
        entries: commentary.entries.clone(),
        dropped_entries: commentary.dropped_entries,
    }
}

fn append_bounded_text(target: &mut String, delta: &str, max_chars: usize) {
    target.push_str(delta);
    let overflow = target.chars().count().saturating_sub(max_chars);
    if overflow == 0 {
        return;
    }
    *target = target.chars().skip(overflow).collect();
}

fn limit_telegram_diff_summary(mut summary: TelegramDiffSummary) -> TelegramDiffSummary {
    summary.files.truncate(TELEGRAM_DIFF_MAX_PATHS);
    summary.paths.truncate(TELEGRAM_DIFF_MAX_PATHS);
    let visible_files = summary.files.len().max(summary.paths.len());
    summary.omitted_paths = summary
        .omitted_paths
        .max(summary.file_count.saturating_sub(visible_files));
    summary
}

fn telegram_typing_key(thread_id: &str, item_id: &str) -> String {
    format!("{}{item_id}", telegram_typing_thread_prefix(thread_id))
}

fn telegram_typing_thread_prefix(thread_id: &str) -> String {
    format!("{thread_id}\u{1f}")
}

impl RouteTarget {
    pub fn deterministic_remote_client_key_for(
        platform: ImPlatformKind,
        account_id: &str,
        chat_id: &str,
    ) -> String {
        let source = format!(
            "{}:{}:{}",
            platform.key(),
            account_id.trim(),
            chat_id.trim()
        );
        let digest = Sha256::digest(source.as_bytes());
        let mut suffix = String::with_capacity(16);
        for byte in digest.iter().take(8) {
            let _ = write!(&mut suffix, "{byte:02x}");
        }
        format!("im:{}:{suffix}", platform.key())
    }

    pub fn deterministic_remote_client_key(&self) -> String {
        Self::deterministic_remote_client_key_for(self.platform, &self.account_id, &self.chat_id)
    }

    pub fn with_deterministic_remote_client_key(mut self) -> Self {
        self.remote_client_key = self.deterministic_remote_client_key();
        self
    }
}

fn log_route_bind(thread_id: &str, route: &RouteTarget, previous: Option<&RouteTarget>) {
    match previous {
        Some(previous) => chain_log::write_line(format!(
            "[im_route] level=warn event=bind_overwrite thread={} platform={} account={} chat={} conversation={} previous_platform={} previous_account={} previous_chat={} previous_conversation={}",
            thread_id,
            route.platform.key(),
            route.account_id,
            route.chat_id,
            route.conversation_key,
            previous.platform.key(),
            previous.account_id,
            previous.chat_id,
            previous.conversation_key
        )),
        None => chain_log::write_diagnostic_lazy(|| {
            format!(
                "[im_route] event=bind thread={} platform={} account={} chat={} conversation={}",
                thread_id,
                route.platform.key(),
                route.account_id,
                route.chat_id,
                route.conversation_key
            )
        }),
    }
}

fn log_route_unbind(event: &str, reason: &str, thread_id: &str, route: &RouteTarget) {
    chain_log::write_line(format!(
        "[im_route] level=warn event={} reason={} thread={} platform={} account={} chat={} conversation={}",
        event,
        reason,
        thread_id,
        route.platform.key(),
        route.account_id,
        route.chat_id,
        route.conversation_key
    ));
}

impl PendingApproval {
    pub fn request_key(&self) -> String {
        approval_request_key(&self.request_id)
    }
}

pub fn approval_request_key(request_id: &Value) -> String {
    match request_id {
        Value::Number(value) => format!("number:{value}"),
        Value::String(value) => format!("string:{value}"),
        other => format!("json:{other}"),
    }
}

pub fn approval_request_fingerprint(request_key: &str) -> String {
    let digest = Sha256::digest(request_key.as_bytes());
    let mut fingerprint = String::with_capacity(16);
    for byte in digest.iter().take(8) {
        let _ = write!(&mut fingerprint, "{byte:02x}");
    }
    fingerprint
}

pub fn route_from_conversation_key(conversation_key: &str) -> Option<RouteTarget> {
    let mut parts = conversation_key.splitn(3, ':');
    let channel = parts.next()?;
    let platform = match channel {
        "feishu" => ImPlatformKind::Feishu,
        "telegram" => ImPlatformKind::Telegram,
        "wechat" => ImPlatformKind::Wechat,
        "wecom" => ImPlatformKind::Wecom,
        _ => return None,
    };
    let account_id = parts.next()?.to_string();
    let chat_id = parts.next()?.to_string();
    Some(
        RouteTarget {
            platform,
            conversation_key: conversation_key.to_string(),
            account_id,
            chat_id,
            remote_client_key: String::new(),
        }
        .with_deterministic_remote_client_key(),
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::types::{ImPlatformKind, InboundAttachment};

    use super::{
        ObservedSetting, PENDING_ATTACHMENTS_MAX_AGE_MS, PendingApproval, PendingTelegramTurn,
        RouteTarget, RuntimeState, TELEGRAM_QUEUED_TURNS_MAX_COUNT,
        TELEGRAM_THREAD_SETTINGS_DRAFT_MAX_AGE_MS, TelegramCollabProgressStatus,
        TelegramCollabProgressUpdate, TelegramCommandProgressEntry,
        TelegramCommandProgressEntryKind, TelegramCommandProgressStatus, TelegramDiffFileSummary,
        TelegramDiffSummary, TelegramModelSwitchRequestState, TelegramPlanStep,
        TelegramPlanStepStatus, TelegramQueueEnqueueOutcome, TelegramThreadSettingsModelChoice,
        TelegramThreadSettingsObservation, TelegramThreadSettingsPatch,
        TelegramThreadSettingsPatchValue, TelegramThreadSettingsStage, TelegramTypingSendAction,
        TelegramWebSearchProgressEntry, ThreadRoutingRequestState, ThreadRoutingStage,
        ThreadSettingsSnapshot, ThreadTurnState, TurnOrigin, route_from_conversation_key,
    };

    fn approval(id: i64) -> PendingApproval {
        PendingApproval {
            request_id: json!(id),
            request_kind: "command".to_string(),
            method: "item/commandExecution/requestApproval".to_string(),
            params: json!({
                "threadId": "thread",
                "turnId": "turn",
                "itemId": "item",
                "command": "test",
                "cwd": "D:\\test"
            }),
            summary: "command: `test`".to_string(),
            decisions: vec![],
            message_id: None,
            remote_client_key: None,
        }
    }

    fn image_attachment(index: usize) -> InboundAttachment {
        InboundAttachment {
            kind: "image".to_string(),
            name: Some(format!("image-{index}.png")),
            mime_type: Some("image/png".to_string()),
            text_hint: None,
            local_path: Some(format!("C:\\temp\\image-{index}.png")),
        }
    }

    fn command_progress_entry(
        item_id: &str,
        command: &str,
        status: TelegramCommandProgressStatus,
    ) -> TelegramCommandProgressEntry {
        TelegramCommandProgressEntry {
            item_id: item_id.to_string(),
            kind: TelegramCommandProgressEntryKind::Command,
            command: command.to_string(),
            status,
            exit_code: None,
            duration_ms: None,
            failure_output: None,
        }
    }

    fn web_search_progress_entry(item_id: &str, query: &str) -> TelegramWebSearchProgressEntry {
        TelegramWebSearchProgressEntry {
            item_id: item_id.to_string(),
            summary: format!("搜索 · {query} · 1 条结果"),
            blocks: vec![json!({
                "type": "paragraph",
                "text": format!("关键词：{query}"),
            })],
            fallback_markdown: format!("🔎 搜索\n\n关键词：`{query}`\n结果：1 条"),
        }
    }

    fn collab_progress_update(
        agent_id: &str,
        name: Option<&str>,
        status: Option<TelegramCollabProgressStatus>,
        occurred_at_ms: u128,
    ) -> TelegramCollabProgressUpdate {
        TelegramCollabProgressUpdate {
            agent_id: agent_id.to_string(),
            name: name.map(str::to_string),
            status,
            detail: None,
            occurred_at_ms,
            create_if_missing: name.is_some(),
            restart: false,
        }
    }

    #[test]
    fn pending_attachments_accumulate_until_description_arrives() {
        let mut runtime = RuntimeState::default();
        let route = "feishu:default:chat-1";

        assert_eq!(
            runtime.hold_pending_attachments(route, vec![image_attachment(1)], 1_000),
            1
        );
        assert_eq!(
            runtime.hold_pending_attachments(route, vec![image_attachment(2)], 2_000),
            2
        );

        let attachments = runtime.take_pending_attachments(route, 3_000);
        assert_eq!(attachments.len(), 2);
        assert_eq!(attachments[0].name.as_deref(), Some("image-1.png"));
        assert_eq!(attachments[1].name.as_deref(), Some("image-2.png"));
        assert!(runtime.take_pending_attachments(route, 4_000).is_empty());
    }

    #[test]
    fn pending_attachments_keep_only_the_latest_eight_images() {
        let mut runtime = RuntimeState::default();
        let route = "feishu:default:chat-1";
        let attachments = (0..10).map(image_attachment).collect();

        assert_eq!(
            runtime.hold_pending_attachments(route, attachments, 1_000),
            8
        );

        let attachments = runtime.take_pending_attachments(route, 2_000);
        assert_eq!(attachments.len(), 8);
        assert_eq!(attachments[0].name.as_deref(), Some("image-2.png"));
        assert_eq!(attachments[7].name.as_deref(), Some("image-9.png"));
    }

    #[test]
    fn pending_attachments_expire_after_ten_minutes() {
        let mut runtime = RuntimeState::default();
        let route = "feishu:default:chat-1";
        runtime.hold_pending_attachments(route, vec![image_attachment(1)], 1_000);

        assert!(
            runtime
                .take_pending_attachments(route, 1_000 + PENDING_ATTACHMENTS_MAX_AGE_MS + 1)
                .is_empty()
        );
    }

    #[test]
    fn telegram_turn_queue_is_fifo_and_bounded() {
        let mut runtime = RuntimeState::default();
        let route = "telegram:default:chat-1";

        for index in 0..TELEGRAM_QUEUED_TURNS_MAX_COUNT {
            assert_eq!(
                runtime.enqueue_telegram_turn(
                    route,
                    PendingTelegramTurn {
                        text: format!("message-{index}"),
                        attachments: Vec::new(),
                    }
                ),
                Some(index + 1)
            );
        }
        assert!(
            runtime
                .enqueue_telegram_turn(
                    route,
                    PendingTelegramTurn {
                        text: "overflow".to_string(),
                        attachments: Vec::new(),
                    }
                )
                .is_none()
        );

        for index in 0..TELEGRAM_QUEUED_TURNS_MAX_COUNT {
            assert_eq!(
                runtime
                    .take_next_telegram_turn(route)
                    .expect("queued turn")
                    .text,
                format!("message-{index}")
            );
        }
        assert_eq!(runtime.telegram_queue_len(route), 0);
    }

    #[test]
    fn telegram_queue_enqueue_is_atomic_with_turn_state() {
        let mut runtime = RuntimeState::default();
        let route = RouteTarget {
            platform: ImPlatformKind::Telegram,
            conversation_key: "telegram:default:chat-1".to_string(),
            account_id: "default".to_string(),
            chat_id: "chat-1".to_string(),
            remote_client_key: "im:telegram:default:chat-1".to_string(),
        };
        runtime.bind_route("thread", route.clone());
        let queued = |text: &str| PendingTelegramTurn {
            text: text.to_string(),
            attachments: Vec::new(),
        };

        assert_eq!(
            runtime.enqueue_telegram_turn_if_active(&route.conversation_key, queued("idle")),
            TelegramQueueEnqueueOutcome::NotRunning
        );

        runtime.try_mark_turn_starting("thread").unwrap();
        assert_eq!(
            runtime.enqueue_telegram_turn_if_active(&route.conversation_key, queued("starting")),
            TelegramQueueEnqueueOutcome::Added(1)
        );

        runtime.mark_turn_started("thread", "turn");
        assert_eq!(
            runtime.enqueue_telegram_turn_if_active(&route.conversation_key, queued("running")),
            TelegramQueueEnqueueOutcome::Added(2)
        );

        runtime.mark_turn_completed("thread", Some("turn"));
        assert_eq!(
            runtime.enqueue_telegram_turn_if_active(&route.conversation_key, queued("finished")),
            TelegramQueueEnqueueOutcome::NotRunning
        );
        assert_eq!(runtime.telegram_queue_len(&route.conversation_key), 2);
    }

    #[test]
    fn telegram_busy_requeue_never_drops_an_existing_turn() {
        let mut runtime = RuntimeState::default();
        let route = "telegram:default:chat-1";
        runtime.enqueue_telegram_turn(
            route,
            PendingTelegramTurn {
                text: "original-front".to_string(),
                attachments: Vec::new(),
            },
        );
        let original = runtime
            .take_next_telegram_turn(route)
            .expect("original queued turn");

        for index in 0..TELEGRAM_QUEUED_TURNS_MAX_COUNT {
            assert!(
                runtime
                    .enqueue_telegram_turn(
                        route,
                        PendingTelegramTurn {
                            text: format!("concurrent-{index}"),
                            attachments: Vec::new(),
                        },
                    )
                    .is_some()
            );
        }

        runtime.requeue_telegram_turn_front(route, original);

        assert_eq!(
            runtime.telegram_queue_len(route),
            TELEGRAM_QUEUED_TURNS_MAX_COUNT + 1
        );
        assert_eq!(
            runtime
                .take_next_telegram_turn(route)
                .expect("requeued original turn")
                .text,
            "original-front"
        );
        for index in 0..TELEGRAM_QUEUED_TURNS_MAX_COUNT {
            assert_eq!(
                runtime
                    .take_next_telegram_turn(route)
                    .expect("concurrently queued turn")
                    .text,
                format!("concurrent-{index}")
            );
        }
    }

    #[test]
    fn clearing_a_route_also_clears_telegram_turn_queue() {
        let mut runtime = RuntimeState::default();
        let route = RouteTarget {
            platform: ImPlatformKind::Telegram,
            conversation_key: "telegram:default:chat-1".to_string(),
            account_id: "default".to_string(),
            chat_id: "chat-1".to_string(),
            remote_client_key: "im:telegram:default:chat-1".to_string(),
        };
        runtime.bind_route("thread", route.clone());
        runtime.enqueue_telegram_turn(
            &route.conversation_key,
            PendingTelegramTurn {
                text: "queued".to_string(),
                attachments: Vec::new(),
            },
        );

        runtime.unbind_routes_for_conversation_with_reason(&route.conversation_key, "test");

        assert_eq!(runtime.telegram_queue_len(&route.conversation_key), 0);
    }

    #[test]
    fn telegram_command_progress_reuses_one_message_and_deduplicates_items() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn");

        let first = runtime
            .upsert_telegram_command_progress(
                "thread",
                "turn",
                command_progress_entry(
                    "item-1",
                    "cargo check",
                    TelegramCommandProgressStatus::Running,
                ),
                false,
            )
            .expect("first command should create a message");
        assert_eq!(first.message_id, None);
        runtime.remember_telegram_command_progress_delivery(
            "thread",
            "turn",
            first.revision,
            "77".to_string(),
        );

        let second = runtime
            .upsert_telegram_command_progress(
                "thread",
                "turn",
                command_progress_entry(
                    "item-2",
                    "cargo test",
                    TelegramCommandProgressStatus::Running,
                ),
                false,
            )
            .expect("new running command should edit the aggregate immediately");
        assert_eq!(second.message_id.as_deref(), Some("77"));
        assert_eq!(second.entries.len(), 2);
        runtime.remember_telegram_command_progress_delivery(
            "thread",
            "turn",
            second.revision,
            "77".to_string(),
        );
        let completed = runtime
            .upsert_telegram_command_progress(
                "thread",
                "turn",
                command_progress_entry(
                    "item-2",
                    "cargo test",
                    TelegramCommandProgressStatus::Succeeded,
                ),
                true,
            )
            .expect("completion should edit the aggregate");
        assert_eq!(completed.message_id.as_deref(), Some("77"));
        assert_eq!(completed.entries.len(), 2);

        runtime.remember_telegram_command_progress_delivery(
            "thread",
            "turn",
            completed.revision,
            "77".to_string(),
        );
        assert!(
            runtime
                .upsert_telegram_command_progress(
                    "thread",
                    "turn",
                    command_progress_entry(
                        "item-2",
                        "cargo test",
                        TelegramCommandProgressStatus::Succeeded,
                    ),
                    true,
                )
                .is_none()
        );
    }

    #[test]
    fn telegram_web_search_progress_reuses_one_message_and_deduplicates_items() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn");

        let first = runtime
            .upsert_telegram_web_search_progress(
                "thread",
                "turn",
                web_search_progress_entry("search-1", "rust"),
            )
            .expect("first search should create progress");
        assert_eq!(first.message_id, None);
        assert_eq!(first.web_searches.len(), 1);
        runtime.remember_telegram_command_progress_delivery(
            "thread",
            "turn",
            first.revision,
            "77".to_string(),
        );

        let second = runtime
            .upsert_telegram_web_search_progress(
                "thread",
                "turn",
                web_search_progress_entry("search-2", "telegram"),
            )
            .expect("second search should edit progress");
        assert_eq!(second.message_id.as_deref(), Some("77"));
        assert_eq!(second.web_searches.len(), 2);
        assert!(
            runtime
                .upsert_telegram_web_search_progress(
                    "thread",
                    "turn",
                    web_search_progress_entry("search-2", "telegram"),
                )
                .is_none()
        );
    }

    #[test]
    fn telegram_command_progress_allows_only_one_in_flight_snapshot() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn");

        let created = runtime
            .upsert_telegram_command_progress(
                "thread",
                "turn",
                command_progress_entry(
                    "item",
                    "cargo check",
                    TelegramCommandProgressStatus::Running,
                ),
                true,
            )
            .expect("initial progress should be dirty");
        let claimed = runtime
            .claim_telegram_command_progress_delivery("thread", "turn")
            .expect("initial progress should be claimed");
        assert_eq!(claimed.revision, created.revision);
        assert!(
            runtime
                .claim_telegram_command_progress_delivery("thread", "turn")
                .is_none()
        );
        assert!(runtime.telegram_command_progress_delivery_is_current(
            "thread",
            "turn",
            claimed.revision
        ));
        assert!(!runtime.telegram_command_progress_delivery_is_current(
            "thread",
            "turn",
            claimed.revision.saturating_add(1)
        ));
    }

    #[test]
    fn telegram_command_progress_queues_latest_revision_behind_delivery() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn");

        let first = runtime
            .upsert_telegram_command_progress(
                "thread",
                "turn",
                command_progress_entry(
                    "item",
                    "cargo check",
                    TelegramCommandProgressStatus::Running,
                ),
                true,
            )
            .expect("initial progress should be dirty");
        let claimed = runtime
            .claim_telegram_command_progress_delivery("thread", "turn")
            .expect("initial progress should be claimed");

        let updated = runtime
            .upsert_telegram_command_progress(
                "thread",
                "turn",
                command_progress_entry(
                    "item",
                    "cargo check",
                    TelegramCommandProgressStatus::Succeeded,
                ),
                true,
            )
            .expect("new state should remain dirty while the first send is in flight");
        assert!(updated.revision > first.revision);
        assert!(runtime.telegram_command_progress_delivery_is_current(
            "thread",
            "turn",
            claimed.revision
        ));

        let next = runtime
            .complete_telegram_command_progress_delivery(
                "thread",
                "turn",
                claimed.revision,
                "42".to_string(),
            )
            .expect("latest revision should be claimed after the first send");
        assert_eq!(next.revision, updated.revision);
        assert_eq!(next.message_id.as_deref(), Some("42"));
        assert!(
            runtime
                .complete_telegram_command_progress_delivery(
                    "thread",
                    "turn",
                    claimed.revision,
                    "stale".to_string(),
                )
                .is_none()
        );
    }

    #[test]
    fn telegram_commentary_reuses_delivery_and_deduplicates_items() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn");

        let first = runtime
            .append_telegram_commentary("thread", "turn", "item-1", "first update".to_string())
            .expect("first commentary");
        assert_eq!(first.message_id, None);
        assert_eq!(first.entries.len(), 1);

        runtime.remember_telegram_commentary_delivery("thread", "turn", 0, "42".to_string());
        assert!(
            runtime
                .append_telegram_commentary("thread", "turn", "item-1", "first update".to_string(),)
                .is_none()
        );

        let second = runtime
            .append_telegram_commentary("thread", "turn", "item-2", "second update".to_string())
            .expect("second commentary");
        assert_eq!(second.message_id.as_deref(), Some("42"));
        assert_eq!(second.entries.len(), 2);
        assert_eq!(
            runtime.telegram_commentary_delivery_target("thread", "turn", 0),
            Some(Some("42".to_string()))
        );
    }

    #[test]
    fn telegram_commentary_compaction_starts_a_new_delivery_segment() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn");
        let before = runtime
            .append_telegram_commentary("thread", "turn", "item-1", "before".to_string())
            .expect("commentary before compaction");
        runtime.remember_telegram_commentary_delivery(
            "thread",
            "turn",
            before.segment,
            "42".to_string(),
        );

        assert!(runtime.start_new_telegram_commentary_segment("thread", "turn"));
        assert_eq!(
            runtime.telegram_commentary_delivery_target("thread", "turn", before.segment),
            Some(Some("42".to_string()))
        );

        let after = runtime
            .append_telegram_commentary("thread", "turn", "item-2", "after".to_string())
            .expect("commentary after compaction");
        assert_eq!(after.segment, before.segment + 1);
        assert_eq!(after.message_id, None);
        assert_eq!(after.entries.len(), 1);
        assert_eq!(after.entries[0].text, "after");

        runtime.remember_telegram_commentary_delivery(
            "thread",
            "turn",
            before.segment,
            "stale".to_string(),
        );
        assert_eq!(
            runtime.telegram_commentary_delivery_target("thread", "turn", before.segment),
            Some(Some("stale".to_string()))
        );
        assert_eq!(
            runtime.telegram_commentary_delivery_target("thread", "turn", after.segment),
            Some(None)
        );
    }

    #[test]
    fn telegram_commentary_supports_repeated_compaction_without_crossing_deliveries() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn");

        let first = runtime
            .append_telegram_commentary("thread", "turn", "first", "first".to_string())
            .expect("first segment");
        assert!(runtime.start_new_telegram_commentary_segment("thread", "turn"));
        let second = runtime
            .append_telegram_commentary("thread", "turn", "second", "second".to_string())
            .expect("second segment");
        assert!(runtime.start_new_telegram_commentary_segment("thread", "turn"));
        let third = runtime
            .append_telegram_commentary("thread", "turn", "third", "third".to_string())
            .expect("third segment");

        for (segment, message_id) in [
            (first.segment, "10"),
            (second.segment, "20"),
            (third.segment, "30"),
        ] {
            runtime.remember_telegram_commentary_delivery(
                "thread",
                "turn",
                segment,
                message_id.to_string(),
            );
        }

        assert_eq!(
            runtime.telegram_commentary_delivery_target("thread", "turn", first.segment),
            Some(Some("10".to_string()))
        );
        assert_eq!(
            runtime.telegram_commentary_delivery_target("thread", "turn", second.segment),
            Some(Some("20".to_string()))
        );
        assert_eq!(
            runtime.telegram_commentary_delivery_target("thread", "turn", third.segment),
            Some(Some("30".to_string()))
        );
    }

    #[test]
    fn telegram_commentary_compaction_before_first_update_starts_segment_one() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn");

        assert!(runtime.start_new_telegram_commentary_segment("thread", "turn"));
        let after = runtime
            .append_telegram_commentary("thread", "turn", "item", "after".to_string())
            .expect("commentary after compaction");

        assert_eq!(after.segment, 1);
        assert_eq!(after.entries[0].text, "after");
        assert_eq!(
            runtime.telegram_commentary_delivery_target("thread", "turn", 0),
            None
        );
    }

    #[test]
    fn telegram_commentary_rejects_stale_turn_and_clears_on_replacement() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "old-turn");
        runtime
            .append_telegram_commentary("thread", "old-turn", "item-1", "old update".to_string())
            .expect("old commentary");

        runtime.mark_turn_started("thread", "new-turn");
        assert_eq!(
            runtime.telegram_commentary_delivery_target("thread", "old-turn", 0),
            None
        );
        assert!(
            runtime
                .append_telegram_commentary(
                    "thread",
                    "old-turn",
                    "late-item",
                    "late update".to_string(),
                )
                .is_none()
        );
        assert!(
            runtime
                .append_telegram_commentary(
                    "thread",
                    "new-turn",
                    "item-2",
                    "new update".to_string(),
                )
                .is_some()
        );
    }

    #[test]
    fn telegram_commentary_is_frozen_when_the_turn_completes() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn");
        runtime
            .append_telegram_commentary("thread", "turn", "item-1", "update".to_string())
            .expect("commentary");
        runtime.remember_telegram_commentary_delivery("thread", "turn", 0, "42".to_string());

        assert!(runtime.mark_turn_completed("thread", Some("turn")));
        assert_eq!(
            runtime.telegram_commentary_delivery_target("thread", "turn", 0),
            Some(Some("42".to_string()))
        );
        assert!(
            runtime
                .append_telegram_commentary(
                    "thread",
                    "turn",
                    "late-item",
                    "late update".to_string(),
                )
                .is_none()
        );
    }

    #[test]
    fn telegram_command_progress_rejects_old_delivery_after_turn_replacement() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "old-turn");
        let first = runtime
            .upsert_telegram_command_progress(
                "thread",
                "old-turn",
                command_progress_entry(
                    "item",
                    "cargo check",
                    TelegramCommandProgressStatus::Running,
                ),
                true,
            )
            .expect("initial progress should be dirty");
        let claimed = runtime
            .claim_telegram_command_progress_delivery("thread", "old-turn")
            .expect("initial progress should be claimed");
        assert_eq!(claimed.revision, first.revision);

        runtime.mark_turn_completed("thread", Some("old-turn"));
        runtime.mark_turn_started("thread", "new-turn");
        assert!(!runtime.telegram_command_progress_delivery_is_current(
            "thread",
            "old-turn",
            claimed.revision
        ));
        assert!(
            runtime
                .complete_telegram_command_progress_delivery(
                    "thread",
                    "old-turn",
                    claimed.revision,
                    "stale".to_string(),
                )
                .is_none()
        );
        assert!(runtime.telegram_command_progress_by_thread.is_empty());
    }

    #[test]
    fn telegram_command_progress_cleans_terminal_state_after_final_delivery() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn");
        runtime
            .upsert_telegram_command_progress(
                "thread",
                "turn",
                command_progress_entry(
                    "item",
                    "cargo check",
                    TelegramCommandProgressStatus::Running,
                ),
                true,
            )
            .expect("initial progress should be dirty");
        let first = runtime
            .claim_telegram_command_progress_delivery("thread", "turn")
            .expect("initial progress should be claimed");
        let final_snapshot = runtime
            .finish_telegram_command_progress("thread", "turn")
            .expect("terminal progress should be dirty");
        assert!(final_snapshot.completed);
        assert!(runtime.mark_turn_completed("thread", Some("turn")));
        assert!(
            runtime
                .telegram_command_progress_by_thread
                .contains_key("thread")
        );

        let queued = runtime
            .complete_telegram_command_progress_delivery(
                "thread",
                "turn",
                first.revision,
                "42".to_string(),
            )
            .expect("terminal revision should follow the initial delivery");
        assert!(queued.completed);
        assert_eq!(queued.message_id.as_deref(), Some("42"));
        assert!(
            runtime
                .telegram_command_progress_by_thread
                .contains_key("thread")
        );
        assert!(
            runtime
                .complete_telegram_command_progress_delivery(
                    "thread",
                    "turn",
                    queued.revision,
                    "42".to_string(),
                )
                .is_none()
        );
        assert!(runtime.telegram_command_progress_by_thread.is_empty());
    }

    #[test]
    fn telegram_command_progress_keeps_terminal_failure_retryable_before_cleanup() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn");
        runtime
            .upsert_telegram_command_progress(
                "thread",
                "turn",
                command_progress_entry(
                    "item",
                    "cargo test",
                    TelegramCommandProgressStatus::Running,
                ),
                true,
            )
            .expect("initial progress should be dirty");
        let first = runtime
            .claim_telegram_command_progress_delivery("thread", "turn")
            .expect("initial progress should be claimed");
        runtime
            .finish_telegram_command_progress("thread", "turn")
            .expect("terminal progress should be queued");

        assert!(runtime.fail_telegram_command_progress_delivery("thread", "turn", first.revision));
        assert!(runtime.mark_turn_completed("thread", Some("turn")));
        let retry = runtime
            .retry_telegram_command_progress_delivery("thread", "turn")
            .expect("terminal progress should remain retryable");
        assert!(retry.completed);
        assert!(
            runtime
                .complete_telegram_command_progress_delivery(
                    "thread",
                    "turn",
                    retry.revision,
                    "43".to_string(),
                )
                .is_none()
        );
        assert!(runtime.telegram_command_progress_by_thread.is_empty());
    }

    #[test]
    fn telegram_command_progress_can_reclaim_retryable_active_failure() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn");
        runtime
            .upsert_telegram_command_progress(
                "thread",
                "turn",
                command_progress_entry(
                    "item",
                    "cargo test",
                    TelegramCommandProgressStatus::Running,
                ),
                true,
            )
            .expect("initial progress should be dirty");
        let first = runtime
            .claim_telegram_command_progress_delivery("thread", "turn")
            .expect("initial progress should be claimed");

        assert!(!runtime.fail_telegram_command_progress_delivery("thread", "turn", first.revision));
        let retry = runtime
            .claim_telegram_command_progress_delivery("thread", "turn")
            .expect("retryable API failures should be able to reclaim the snapshot");
        assert_eq!(retry.revision, first.revision);
    }

    #[test]
    fn telegram_command_progress_ignores_duplicate_detail_events() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn");

        runtime.append_telegram_reasoning_delta("thread", "turn", "reasoning", 0, "Review");
        let reasoning = runtime
            .complete_telegram_reasoning("thread", "turn", "reasoning", None)
            .expect("reasoning should be flushed");
        assert!(
            runtime
                .complete_telegram_reasoning(
                    "thread",
                    "turn",
                    "reasoning",
                    Some("Review".to_string()),
                )
                .is_none()
        );

        let plan = vec![TelegramPlanStep {
            step: "Inspect".to_string(),
            status: TelegramPlanStepStatus::InProgress,
        }];
        let planned = runtime
            .update_telegram_plan("thread", "turn", None, plan.clone(), true)
            .expect("plan should be dirty");
        assert!(planned.revision > reasoning.revision);
        assert!(
            runtime
                .update_telegram_plan("thread", "turn", None, plan, true)
                .is_none()
        );

        let diff = TelegramDiffSummary {
            file_count: 1,
            additions: 2,
            deletions: 1,
            files: vec![TelegramDiffFileSummary {
                path: "src/lib.rs".to_string(),
                additions: 2,
                deletions: 1,
            }],
            paths: vec!["src/lib.rs".to_string()],
            omitted_paths: 0,
        };
        let diff_snapshot = runtime
            .update_telegram_diff("thread", "turn", Some(diff.clone()), true)
            .expect("diff should be dirty");
        assert!(diff_snapshot.revision > planned.revision);
        assert!(
            runtime
                .update_telegram_diff("thread", "turn", Some(diff), true)
                .is_none()
        );
    }

    #[test]
    fn telegram_command_progress_final_flush_and_turn_cleanup() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn-1");
        let started = runtime
            .upsert_telegram_command_progress(
                "thread",
                "turn-1",
                command_progress_entry(
                    "item",
                    "cargo test",
                    TelegramCommandProgressStatus::Running,
                ),
                false,
            )
            .expect("started command");
        runtime.remember_telegram_command_progress_delivery(
            "thread",
            "turn-1",
            started.revision,
            "42".to_string(),
        );

        let final_snapshot = runtime
            .finish_telegram_command_progress("thread", "turn-1")
            .expect("final progress update");
        assert!(final_snapshot.completed);
        assert_eq!(final_snapshot.message_id.as_deref(), Some("42"));
        assert_eq!(
            final_snapshot.entries[0].status,
            TelegramCommandProgressStatus::Interrupted
        );
        assert!(
            runtime
                .finish_telegram_command_progress("thread", "turn-2")
                .is_none()
        );

        runtime.mark_turn_completed("thread", Some("turn-1"));
        assert!(runtime.telegram_command_progress_by_thread.is_empty());
    }

    #[test]
    fn telegram_turn_details_share_the_command_progress_message() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn");

        runtime.append_telegram_reasoning_delta("thread", "turn", "reasoning-1", 0, "first");
        runtime.append_telegram_reasoning_delta("thread", "turn", "reasoning-1", 0, " second");
        assert!(
            runtime
                .complete_telegram_reasoning("thread", "turn", "reasoning-1", None)
                .is_some()
        );
        let plan_snapshot = runtime
            .update_telegram_plan(
                "thread",
                "turn",
                Some("Implement and verify".to_string()),
                vec![TelegramPlanStep {
                    step: "Implement".to_string(),
                    status: TelegramPlanStepStatus::InProgress,
                }],
                true,
            )
            .expect("plan should create the aggregate");
        assert_eq!(
            plan_snapshot.reasoning_summary.as_deref(),
            Some("first second")
        );
        runtime.remember_telegram_command_progress_delivery(
            "thread",
            "turn",
            plan_snapshot.revision,
            "99".to_string(),
        );

        let diff = TelegramDiffSummary {
            file_count: 2,
            additions: 3,
            deletions: 1,
            files: vec![
                TelegramDiffFileSummary {
                    path: "src/a.rs".to_string(),
                    additions: 2,
                    deletions: 1,
                },
                TelegramDiffFileSummary {
                    path: "src/b.rs".to_string(),
                    additions: 1,
                    deletions: 0,
                },
            ],
            paths: vec!["src/a.rs".to_string(), "src/b.rs".to_string()],
            omitted_paths: 0,
        };
        assert!(
            runtime
                .update_telegram_diff("thread", "turn", Some(diff.clone()), false)
                .is_none()
        );
        let file_snapshot = runtime
            .complete_telegram_file_change("thread", "turn", None)
            .expect("file change should flush cached diff");
        assert_eq!(file_snapshot.message_id.as_deref(), Some("99"));
        assert_eq!(file_snapshot.diff_summary, Some(diff));

        let duplicate_revision = file_snapshot.revision;
        assert!(
            runtime
                .update_telegram_diff("thread", "turn", file_snapshot.diff_summary.clone(), false,)
                .is_none()
        );
        let final_snapshot = runtime
            .finish_telegram_command_progress("thread", "turn")
            .expect("details-only turn should flush at completion");
        assert!(final_snapshot.completed);
        assert!(final_snapshot.revision > duplicate_revision);
    }

    #[test]
    fn telegram_reasoning_keeps_only_the_latest_summary_index() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn");

        runtime.append_telegram_reasoning_delta("thread", "turn", "reasoning", 0, "older summary");
        runtime.append_telegram_reasoning_delta("thread", "turn", "reasoning", 1, "**latest");
        runtime.append_telegram_reasoning_delta("thread", "turn", "reasoning", 1, " summary**");
        runtime.append_telegram_reasoning_delta("thread", "turn", "reasoning", 0, " stale replay");

        let snapshot = runtime
            .complete_telegram_reasoning("thread", "turn", "reasoning", None)
            .expect("latest reasoning summary");
        assert_eq!(
            snapshot.reasoning_summary.as_deref(),
            Some("**latest summary**")
        );
    }

    #[test]
    fn telegram_turn_details_ignore_stale_turns_and_empty_diff() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "current");
        assert!(
            runtime
                .update_telegram_plan("thread", "old", None, Vec::new(), true,)
                .is_none()
        );
        assert!(
            runtime
                .update_telegram_diff("thread", "current", None, true)
                .is_none()
        );
        assert!(
            runtime
                .complete_telegram_file_change(
                    "thread",
                    "old",
                    Some(TelegramDiffSummary {
                        file_count: 1,
                        additions: 1,
                        deletions: 0,
                        files: vec![TelegramDiffFileSummary {
                            path: "old.rs".to_string(),
                            additions: 1,
                            deletions: 0,
                        }],
                        paths: vec!["old.rs".to_string()],
                        omitted_paths: 0,
                    }),
                )
                .is_none()
        );
    }

    #[test]
    fn telegram_collab_progress_shares_command_delivery_and_message() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn");

        let command = runtime
            .upsert_telegram_command_progress(
                "thread",
                "turn",
                command_progress_entry(
                    "cmd-1",
                    "cargo check",
                    TelegramCommandProgressStatus::Running,
                ),
                true,
            )
            .expect("command should create the aggregate");
        let first = runtime
            .claim_telegram_command_progress_delivery("thread", "turn")
            .expect("first aggregate revision should be claimable");
        assert_eq!(first.revision, command.revision);

        let collab = runtime
            .upsert_telegram_collab_task_progress(
                "thread",
                "turn",
                vec![collab_progress_update(
                    "agent-secret",
                    Some("api_review"),
                    Some(TelegramCollabProgressStatus::Running),
                    1_000,
                )],
            )
            .expect("collaboration update should dirty the same aggregate");
        assert!(collab.revision > first.revision);
        assert_eq!(collab.collab.as_ref().unwrap().entries.len(), 1);
        assert!(
            runtime
                .claim_telegram_command_progress_delivery("thread", "turn")
                .is_none()
        );

        let next = runtime
            .complete_telegram_command_progress_delivery(
                "thread",
                "turn",
                first.revision,
                "88".to_string(),
            )
            .expect("queued collaboration state should follow the first send");
        assert_eq!(next.message_id.as_deref(), Some("88"));
        assert_eq!(
            next.collab.as_ref().unwrap().entries[0].status,
            TelegramCollabProgressStatus::Running
        );
        assert!(
            runtime
                .complete_telegram_command_progress_delivery(
                    "thread",
                    "turn",
                    first.revision,
                    "stale".to_string(),
                )
                .is_none()
        );

        let completed = runtime
            .upsert_telegram_collab_task_progress(
                "thread",
                "turn",
                vec![collab_progress_update(
                    "agent-secret",
                    None,
                    Some(TelegramCollabProgressStatus::Succeeded),
                    43_000,
                )],
            )
            .expect("agent completion should edit the aggregate");
        assert_eq!(
            completed.collab.as_ref().unwrap().entries[0].status,
            TelegramCollabProgressStatus::Succeeded
        );

        let final_snapshot = runtime
            .complete_telegram_command_progress_delivery(
                "thread",
                "turn",
                next.revision,
                "88".to_string(),
            )
            .expect("latest state should be sent after the queued update");
        assert_eq!(final_snapshot.message_id.as_deref(), Some("88"));
        assert_eq!(
            final_snapshot.collab.as_ref().unwrap().entries[0].updated_at_ms,
            43_000
        );
    }

    #[test]
    fn telegram_collab_progress_merges_by_agent_and_ignores_stale_updates() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn");

        let initial = runtime
            .upsert_telegram_collab_task_progress(
                "thread",
                "turn",
                vec![collab_progress_update(
                    "agent-a",
                    Some("initial_name"),
                    Some(TelegramCollabProgressStatus::Running),
                    1_000,
                )],
            )
            .expect("agent should create progress");
        let claimed = runtime
            .claim_telegram_command_progress_delivery("thread", "turn")
            .expect("initial progress should be claimable");
        assert_eq!(claimed.revision, initial.revision);
        assert!(
            runtime
                .complete_telegram_command_progress_delivery(
                    "thread",
                    "turn",
                    claimed.revision,
                    "90".to_string(),
                )
                .is_none()
        );

        let mut newest_update = collab_progress_update(
            "agent-a",
            Some("current_name"),
            Some(TelegramCollabProgressStatus::Succeeded),
            3_000,
        );
        newest_update.detail = Some("current detail".to_string());
        let newest = runtime
            .upsert_telegram_collab_task_progress("thread", "turn", vec![newest_update])
            .expect("newer event should update progress");
        let collab = newest.collab.as_ref().unwrap();
        assert_eq!(collab.entries[0].name, "current_name");
        assert_eq!(
            collab.entries[0].status,
            TelegramCollabProgressStatus::Succeeded
        );
        assert_eq!(collab.entries[0].detail.as_deref(), Some("current detail"));
        assert_eq!(collab.entries[0].updated_at_ms, 3_000);

        let mut stale_update = collab_progress_update(
            "agent-a",
            Some("stale_name"),
            Some(TelegramCollabProgressStatus::Failed),
            2_000,
        );
        stale_update.detail = Some("stale detail".to_string());
        assert!(
            runtime
                .upsert_telegram_collab_task_progress("thread", "turn", vec![stale_update])
                .is_none()
        );
    }

    #[test]
    fn telegram_collab_progress_restart_and_terminal_states_are_monotonic() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn");

        let mut started = collab_progress_update(
            "agent-a",
            Some("review"),
            Some(TelegramCollabProgressStatus::Running),
            1_000,
        );
        started.detail = Some("temporary error".to_string());
        runtime
            .upsert_telegram_collab_task_progress("thread", "turn", vec![started])
            .expect("agent should create progress");
        let claimed = runtime
            .claim_telegram_command_progress_delivery("thread", "turn")
            .expect("initial state should be claimable");
        assert!(
            runtime
                .complete_telegram_command_progress_delivery(
                    "thread",
                    "turn",
                    claimed.revision,
                    "92".to_string(),
                )
                .is_none()
        );

        let succeeded = runtime
            .upsert_telegram_collab_task_progress(
                "thread",
                "turn",
                vec![collab_progress_update(
                    "agent-a",
                    None,
                    Some(TelegramCollabProgressStatus::Succeeded),
                    2_000,
                )],
            )
            .expect("success should update progress");
        assert_eq!(succeeded.collab.as_ref().unwrap().entries[0].detail, None);
        let succeeded_claim = runtime
            .claim_telegram_command_progress_delivery("thread", "turn")
            .expect("success should be claimable");
        runtime.complete_telegram_command_progress_delivery(
            "thread",
            "turn",
            succeeded_claim.revision,
            "92".to_string(),
        );

        let mut restarted = collab_progress_update(
            "agent-a",
            None,
            Some(TelegramCollabProgressStatus::Running),
            4_000,
        );
        restarted.restart = true;
        let restarted = runtime
            .upsert_telegram_collab_task_progress("thread", "turn", vec![restarted])
            .expect("newer restart should reopen the agent");
        assert_eq!(
            restarted.collab.as_ref().unwrap().entries[0].status,
            TelegramCollabProgressStatus::Running
        );
        assert_eq!(
            restarted.collab.as_ref().unwrap().entries[0].started_at_ms,
            4_000
        );
        assert_eq!(restarted.collab.as_ref().unwrap().entries[0].detail, None);

        let running = runtime
            .upsert_telegram_collab_task_progress(
                "thread",
                "turn",
                vec![collab_progress_update(
                    "agent-b",
                    Some("running_agent"),
                    Some(TelegramCollabProgressStatus::Running),
                    5_000,
                )],
            )
            .expect("second agent should be visible");
        assert_eq!(running.collab.as_ref().unwrap().entries.len(), 2);

        let terminal = runtime
            .upsert_telegram_collab_task_progress(
                "thread",
                "turn",
                vec![
                    collab_progress_update(
                        "agent-c",
                        Some("completed_agent"),
                        Some(TelegramCollabProgressStatus::Succeeded),
                        6_000,
                    ),
                    collab_progress_update(
                        "agent-d",
                        Some("failed_agent"),
                        Some(TelegramCollabProgressStatus::Failed),
                        7_000,
                    ),
                ],
            )
            .expect("terminal agents should be retained in order");
        assert_eq!(terminal.collab.as_ref().unwrap().entries.len(), 4);
        assert_eq!(
            terminal.collab.as_ref().unwrap().entries[2].status,
            TelegramCollabProgressStatus::Succeeded
        );
        assert_eq!(
            terminal.collab.as_ref().unwrap().entries[3].status,
            TelegramCollabProgressStatus::Failed
        );
        let mut stale_terminal = collab_progress_update(
            "agent-c",
            None,
            Some(TelegramCollabProgressStatus::Running),
            8_000,
        );
        stale_terminal.restart = false;
        assert!(
            runtime
                .upsert_telegram_collab_task_progress("thread", "turn", vec![stale_terminal])
                .is_none()
        );
        let final_snapshot = runtime
            .finish_telegram_command_progress("thread", "turn")
            .expect("turn finish should interrupt running agents");
        assert!(final_snapshot.completed);
        assert_eq!(
            final_snapshot.collab.as_ref().unwrap().entries[0].status,
            TelegramCollabProgressStatus::Interrupted
        );
        assert_eq!(
            final_snapshot.collab.as_ref().unwrap().entries[1].status,
            TelegramCollabProgressStatus::Interrupted
        );
        assert_eq!(
            final_snapshot.collab.as_ref().unwrap().entries[2].status,
            TelegramCollabProgressStatus::Succeeded
        );
        assert_eq!(
            final_snapshot.collab.as_ref().unwrap().entries[3].status,
            TelegramCollabProgressStatus::Failed
        );
    }

    #[test]
    fn telegram_collab_progress_terminal_delivery_survives_turn_cleanup_and_retry() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn");
        runtime
            .upsert_telegram_collab_task_progress(
                "thread",
                "turn",
                vec![collab_progress_update(
                    "agent-a",
                    Some("review"),
                    Some(TelegramCollabProgressStatus::Running),
                    1_000,
                )],
            )
            .expect("agent should create progress");
        let first = runtime
            .claim_telegram_command_progress_delivery("thread", "turn")
            .expect("initial state should be claimable");
        let final_state = runtime
            .finish_telegram_command_progress("thread", "turn")
            .expect("terminal state should be queued");
        assert!(final_state.completed);
        assert!(runtime.mark_turn_completed("thread", Some("turn")));
        assert!(
            runtime
                .telegram_command_progress_by_thread
                .contains_key("thread")
        );

        assert!(runtime.fail_telegram_command_progress_delivery("thread", "turn", first.revision));
        let retry = runtime
            .retry_telegram_command_progress_delivery("thread", "turn")
            .expect("terminal state should remain retryable");
        assert!(retry.completed);
        assert_eq!(
            retry.collab.as_ref().unwrap().entries[0].status,
            TelegramCollabProgressStatus::Interrupted
        );
        assert!(
            runtime
                .complete_telegram_command_progress_delivery(
                    "thread",
                    "turn",
                    retry.revision,
                    "101".to_string(),
                )
                .is_none()
        );
        assert!(runtime.telegram_command_progress_by_thread.is_empty());
    }

    #[test]
    fn telegram_retry_progress_counts_attempts_and_reuses_delivery() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn");

        let first = runtime
            .record_telegram_retry(
                "thread",
                "turn",
                Some("503 Service Unavailable".to_string()),
            )
            .expect("first retry should create progress");
        assert_eq!(first.retry_count, 1);
        assert_eq!(first.message_id, None);
        runtime.remember_telegram_command_progress_delivery(
            "thread",
            "turn",
            first.revision,
            "77".to_string(),
        );

        let second = runtime
            .record_telegram_retry(
                "thread",
                "turn",
                Some("503 Service Unavailable".to_string()),
            )
            .expect("second retry should update progress");
        assert_eq!(second.retry_count, 2);
        assert_eq!(second.message_id.as_deref(), Some("77"));

        runtime.remember_telegram_command_progress_delivery(
            "thread",
            "turn",
            second.revision,
            "77".to_string(),
        );
        let final_snapshot = runtime
            .finish_telegram_command_progress_with_outcome("thread", "turn", true)
            .expect("retry-only progress must receive a terminal update");
        assert!(final_snapshot.completed);
        assert!(final_snapshot.failed);
        assert_eq!(final_snapshot.retry_count, 2);
        assert_eq!(final_snapshot.message_id.as_deref(), Some("77"));
    }

    #[test]
    fn telegram_retry_after_turn_completion_does_not_recreate_progress() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn");
        assert!(
            runtime
                .record_telegram_retry("thread", "turn", None)
                .is_some()
        );

        assert!(runtime.mark_turn_completed("thread", Some("turn")));
        assert!(runtime.telegram_command_progress_by_thread.is_empty());

        assert!(
            runtime
                .record_telegram_retry(
                    "thread",
                    "turn",
                    Some("503 Service Unavailable".to_string()),
                )
                .is_none()
        );
        assert!(runtime.telegram_command_progress_by_thread.is_empty());
    }

    #[test]
    fn terminal_notice_deduplicates_equal_outcomes_and_prioritizes_failure() {
        let mut runtime = RuntimeState::default();

        assert!(runtime.claim_terminal_notice("turn-1", false));
        assert!(!runtime.claim_terminal_notice("turn-1", false));
        assert!(runtime.claim_terminal_notice("turn-1", true));
        assert!(!runtime.claim_terminal_notice("turn-1", true));
        assert!(!runtime.claim_terminal_notice("turn-1", false));

        assert!(runtime.claim_terminal_notice("turn-2", true));
        assert!(!runtime.claim_terminal_notice("turn-2", false));

        runtime.release_terminal_notice("turn-2");
        assert!(runtime.claim_terminal_notice("turn-2", false));
    }

    #[test]
    fn stale_turn_completion_does_not_clear_the_new_turn() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "old-turn");
        runtime.mark_turn_started("thread", "new-turn");

        assert!(!runtime.mark_turn_completed("thread", Some("old-turn")));
        assert_eq!(runtime.current_turn_id("thread"), Some("new-turn"));

        assert!(runtime.mark_turn_completed("thread", Some("new-turn")));
        assert_eq!(runtime.current_turn_id("thread"), None);
    }

    #[test]
    fn terminal_status_fallback_is_scoped_to_one_turn() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn-1");

        assert!(runtime.register_terminal_status_fallback("thread", "turn-1"));
        assert!(runtime.terminal_status_fallback_matches("thread", "turn-1"));
        runtime.cancel_terminal_status_fallback("thread", "other-turn");
        assert!(runtime.terminal_status_fallback_matches("thread", "turn-1"));
        runtime.cancel_terminal_status_fallback("thread", "turn-1");
        assert!(!runtime.terminal_status_fallback_matches("thread", "turn-1"));

        assert!(runtime.register_terminal_status_fallback("thread", "turn-1"));
        runtime.mark_turn_started("thread", "turn-2");
        assert!(!runtime.terminal_status_fallback_matches("thread", "turn-1"));
        assert!(!runtime.register_terminal_status_fallback("thread", "turn-1"));
    }

    #[test]
    fn terminal_status_fallback_token_is_single_owner_across_drivers() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn-1");

        let (server_token, inserted) = runtime
            .register_terminal_status_fallback_token("thread", "turn-1")
            .expect("server registration");
        assert!(inserted);
        let (same_token, inserted_again) = runtime
            .register_terminal_status_fallback_token("thread", "turn-1")
            .expect("duplicate registration should reuse latch");
        assert_eq!(same_token, server_token);
        assert!(!inserted_again);

        assert_eq!(
            runtime.start_terminal_status_fallback("thread", "turn-1"),
            Some(server_token)
        );
        assert_eq!(
            runtime.start_terminal_status_fallback("thread", "turn-1"),
            None
        );
        assert!(runtime.terminal_status_fallback_matches_token("thread", "turn-1", server_token));

        assert!(runtime.claim_terminal_status_fallback("thread", "turn-1", server_token));
        assert!(!runtime.claim_terminal_status_fallback("thread", "turn-1", server_token));
    }

    #[test]
    fn server_driver_starts_after_im_created_the_shared_latch() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn-1");

        let token = runtime
            .start_terminal_status_fallback("thread", "turn-1")
            .expect("IM driver should create latch");
        let (same_token, should_start_server) = runtime
            .register_terminal_status_fallback_token("thread", "turn-1")
            .expect("server driver should reuse latch");
        assert_eq!(same_token, token);
        assert!(should_start_server);
        let (_, should_start_server_again) = runtime
            .register_terminal_status_fallback_token("thread", "turn-1")
            .expect("server driver registration");
        assert!(!should_start_server_again);
    }

    #[test]
    fn failed_command_progress_snapshot_is_marked_failed() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn");
        runtime
            .upsert_telegram_command_progress(
                "thread",
                "turn",
                command_progress_entry(
                    "item",
                    "cargo test",
                    TelegramCommandProgressStatus::Succeeded,
                ),
                true,
            )
            .expect("command progress");

        let snapshot = runtime
            .finish_telegram_command_progress_with_outcome("thread", "turn", true)
            .expect("failed terminal snapshot");
        assert!(snapshot.failed);
        assert!(snapshot.completed);
    }

    #[test]
    fn starting_a_new_turn_discards_stale_command_progress() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread", "turn-1");
        runtime
            .upsert_telegram_command_progress(
                "thread",
                "turn-1",
                command_progress_entry(
                    "old-item",
                    "old command",
                    TelegramCommandProgressStatus::Succeeded,
                ),
                true,
            )
            .expect("old command");

        runtime.mark_turn_started("thread", "turn-2");

        assert!(runtime.telegram_command_progress_by_thread.is_empty());
        let next = runtime
            .upsert_telegram_command_progress(
                "thread",
                "turn-2",
                command_progress_entry(
                    "new-item",
                    "new command",
                    TelegramCommandProgressStatus::Succeeded,
                ),
                true,
            )
            .expect("new command");
        assert_eq!(next.turn_id, "turn-2");
        assert_eq!(next.entries.len(), 1);
        assert_eq!(next.entries[0].command, "new command");
    }

    #[test]
    fn telegram_typing_deltas_do_not_schedule_extra_actions() {
        let mut runtime = RuntimeState::default();

        let generation = runtime
            .start_telegram_typing("thread", "item")
            .expect("first typing update");
        let (finished, revision) = runtime
            .take_telegram_typing_snapshot("thread", "item", generation, 1_000)
            .expect("first typing snapshot");
        assert!(!finished);
        assert!(runtime.start_telegram_typing("thread", "item").is_none());
        assert!(runtime.start_telegram_typing("thread", "item").is_none());
        assert_eq!(
            runtime.complete_telegram_typing_send("thread", "item", generation, revision, true),
            TelegramTypingSendAction::Continue
        );
        assert!(
            runtime
                .telegram_typing_send_delay_ms("thread", "item", generation, 1_100, 300)
                .is_none()
        );
        let (_, delay_ms) = runtime
            .telegram_typing_wait_for_update("thread", "item", generation, 1_100, 4_000)
            .expect("typing should wait for renewal");
        assert_eq!(delay_ms, 3_900);
    }

    #[test]
    fn telegram_typing_renews_after_four_seconds() {
        let mut runtime = RuntimeState::default();
        let generation = runtime
            .start_telegram_typing("thread", "item")
            .expect("typing update");
        let (_, revision) = runtime
            .take_telegram_typing_snapshot("thread", "item", generation, 1_000)
            .expect("initial typing snapshot");
        assert_eq!(
            runtime.complete_telegram_typing_send("thread", "item", generation, revision, true),
            TelegramTypingSendAction::Continue
        );

        let (_, delay_ms) = runtime
            .telegram_typing_wait_for_update("thread", "item", generation, 1_100, 4_000)
            .expect("typing should wait for an update or renewal");
        assert_eq!(delay_ms, 3_900);
        assert!(runtime.mark_telegram_typing_renewal_due("thread", "item", generation));
        assert_eq!(
            runtime.telegram_typing_send_delay_ms("thread", "item", generation, 5_000, 300),
            Some(0)
        );
        let (finished, renewal_revision) = runtime
            .take_telegram_typing_snapshot("thread", "item", generation, 5_000)
            .expect("typing renewal snapshot");
        assert!(!finished);
        assert!(renewal_revision > revision);
    }

    #[test]
    fn telegram_typing_can_be_woken_after_a_regular_message_delivery() {
        let mut runtime = RuntimeState::default();
        let generation = runtime
            .start_telegram_typing("thread", "turn:turn-1")
            .expect("typing update");
        let (_, initial_revision) = runtime
            .take_telegram_typing_snapshot("thread", "turn:turn-1", generation, 1_000)
            .expect("initial typing snapshot");
        assert_eq!(
            runtime.complete_telegram_typing_send(
                "thread",
                "turn:turn-1",
                generation,
                initial_revision,
                true,
            ),
            TelegramTypingSendAction::Continue
        );

        assert!(runtime.wake_telegram_typing("thread", "turn:turn-1"));
        assert_eq!(
            runtime.telegram_typing_send_delay_ms("thread", "turn:turn-1", generation, 1_001, 300),
            Some(299)
        );
        let (finished, renewal_revision) = runtime
            .take_telegram_typing_snapshot("thread", "turn:turn-1", generation, 1_300)
            .expect("woken typing snapshot");
        assert!(!finished);
        assert!(renewal_revision > initial_revision);
    }

    #[test]
    fn telegram_typing_wakes_only_the_active_driver_for_its_thread() {
        let mut runtime = RuntimeState::default();
        let turn_generation = runtime
            .start_telegram_typing("thread", "turn:turn-1")
            .expect("turn typing");
        let other_generation = runtime
            .start_telegram_typing("other-thread", "turn:other-turn")
            .expect("other typing");
        for (thread_id, item_id, generation) in [
            ("thread", "turn:turn-1", turn_generation),
            ("other-thread", "turn:other-turn", other_generation),
        ] {
            let (_, revision) = runtime
                .take_telegram_typing_snapshot(thread_id, item_id, generation, 1_000)
                .expect("initial typing snapshot");
            assert_eq!(
                runtime
                    .complete_telegram_typing_send(thread_id, item_id, generation, revision, true,),
                TelegramTypingSendAction::Continue
            );
        }

        assert_eq!(runtime.wake_telegram_typing_for_thread("thread"), 1);
        assert_eq!(
            runtime.telegram_typing_send_delay_ms(
                "thread",
                "turn:turn-1",
                turn_generation,
                1_001,
                300,
            ),
            Some(299)
        );
        assert!(
            runtime
                .telegram_typing_send_delay_ms(
                    "other-thread",
                    "turn:other-turn",
                    other_generation,
                    1_001,
                    300,
                )
                .is_none()
        );
    }

    #[test]
    fn telegram_typing_allows_only_one_draft_driver_per_thread() {
        let mut runtime = RuntimeState::default();
        let turn_generation = runtime
            .start_telegram_typing("thread", "turn:turn-1")
            .expect("turn typing");
        assert!(
            runtime
                .start_telegram_typing("thread", "agent-item")
                .is_none()
        );

        let finishing = runtime.finish_telegram_typing_for_thread("thread");

        assert_eq!(
            finishing,
            vec![("turn:turn-1".to_string(), turn_generation, false)]
        );
        let (finished, revision) = runtime
            .take_telegram_typing_snapshot("thread", "turn:turn-1", turn_generation, 1_001)
            .expect("finished typing snapshot");
        assert!(finished);
        assert_eq!(
            runtime.complete_telegram_typing_send(
                "thread",
                "turn:turn-1",
                turn_generation,
                revision,
                true,
            ),
            TelegramTypingSendAction::Stop
        );
        assert!(
            runtime
                .start_telegram_typing("thread", "turn:turn-1")
                .is_none()
        );
    }

    #[test]
    fn telegram_approval_barrier_pauses_then_allows_the_turn_to_resume() {
        let mut runtime = RuntimeState::default();
        let generation = runtime
            .start_telegram_typing("thread", "turn:turn-1")
            .expect("turn typing");

        let pausing =
            runtime.suspend_telegram_typing_for_persistent_message("thread", false, Some("turn-1"));
        assert_eq!(
            pausing,
            vec![("turn:turn-1".to_string(), generation, false)]
        );
        assert!(
            runtime
                .start_telegram_typing("thread", "turn:turn-1")
                .is_none()
        );
        let (finished, revision) = runtime
            .take_telegram_typing_snapshot("thread", "turn:turn-1", generation, 1_001)
            .expect("paused typing snapshot");
        assert!(finished);
        assert_eq!(
            runtime.complete_telegram_typing_send(
                "thread",
                "turn:turn-1",
                generation,
                revision,
                true,
            ),
            TelegramTypingSendAction::Stop
        );
        assert!(
            runtime
                .start_telegram_typing("thread", "turn:turn-1")
                .is_none()
        );

        runtime.resume_telegram_typing_after_persistent_message("thread");
        assert!(
            runtime
                .start_telegram_typing("thread", "turn:turn-1")
                .is_some()
        );
    }

    #[test]
    fn telegram_final_barrier_blocks_late_deltas_for_the_completed_turn() {
        let mut runtime = RuntimeState::default();
        let generation = runtime
            .start_telegram_typing("thread", "turn:turn-1")
            .expect("turn typing");

        let finishing =
            runtime.suspend_telegram_typing_for_persistent_message("thread", true, Some("turn-1"));
        assert_eq!(
            finishing,
            vec![("turn:turn-1".to_string(), generation, false)]
        );
        let (finished, revision) = runtime
            .take_telegram_typing_snapshot("thread", "turn:turn-1", generation, 1_001)
            .expect("finished typing snapshot");
        assert!(finished);
        assert_eq!(
            runtime.complete_telegram_typing_send(
                "thread",
                "turn:turn-1",
                generation,
                revision,
                true,
            ),
            TelegramTypingSendAction::Stop
        );

        runtime.resume_telegram_typing_after_persistent_message("thread");
        assert!(
            runtime
                .start_telegram_typing("thread", "turn:turn-1")
                .is_none()
        );
        assert!(
            runtime
                .start_telegram_typing("thread", "turn:turn-2")
                .is_some()
        );
    }

    #[test]
    fn telegram_final_barrier_tombstones_a_turn_without_an_existing_driver() {
        let mut runtime = RuntimeState::default();

        assert!(
            runtime
                .suspend_telegram_typing_for_persistent_message("thread", true, Some("turn-1"),)
                .is_empty()
        );
        runtime.resume_telegram_typing_after_persistent_message("thread");

        assert!(
            runtime
                .start_telegram_typing("thread", "turn:turn-1")
                .is_none()
        );
    }

    #[test]
    fn telegram_typing_can_be_woken_during_retry_backoff() {
        let mut runtime = RuntimeState::default();
        let generation = runtime
            .start_telegram_typing("thread", "turn:turn-1")
            .expect("typing update");
        let (_, failed_revision) = runtime
            .take_telegram_typing_snapshot("thread", "turn:turn-1", generation, 1_000)
            .expect("typing snapshot");
        assert_eq!(
            runtime.complete_telegram_typing_send(
                "thread",
                "turn:turn-1",
                generation,
                failed_revision,
                false,
            ),
            TelegramTypingSendAction::Stop
        );

        assert!(runtime.wake_telegram_typing("thread", "turn:turn-1"));
        assert_eq!(
            runtime.start_telegram_typing("thread", "turn:turn-1"),
            Some(generation)
        );
        let (finished, retry_revision) = runtime
            .take_telegram_typing_snapshot("thread", "turn:turn-1", generation, 1_001)
            .expect("woken retry snapshot");
        assert!(!finished);
        assert!(retry_revision > failed_revision);
    }

    #[test]
    fn telegram_typing_is_removed_after_internal_completion() {
        let mut runtime = RuntimeState::default();
        let generation = runtime
            .start_telegram_typing("thread", "item")
            .expect("typing update");
        let (_, first_revision) = runtime
            .take_telegram_typing_snapshot("thread", "item", generation, 1_000)
            .expect("initial typing snapshot");
        assert_eq!(
            runtime.complete_telegram_typing_send(
                "thread",
                "item",
                generation,
                first_revision,
                true,
            ),
            TelegramTypingSendAction::Continue
        );

        let (finished_generation, should_start, _completed) = runtime
            .finish_telegram_typing("thread", "item")
            .expect("typing completion");
        assert_eq!(finished_generation, generation);
        assert!(!should_start);
        assert_eq!(
            runtime.telegram_typing_send_delay_ms("thread", "item", generation, 1_001, 300),
            Some(0)
        );
        let (finished, final_revision) = runtime
            .take_telegram_typing_snapshot("thread", "item", generation, 1_001)
            .expect("internal completion snapshot");
        assert!(finished);
        assert_eq!(
            runtime.complete_telegram_typing_send(
                "thread",
                "item",
                generation,
                final_revision,
                true,
            ),
            TelegramTypingSendAction::Stop
        );
        assert!(runtime.finish_telegram_typing("thread", "item").is_none());
        assert!(runtime.start_telegram_typing("thread", "item").is_none());
    }

    #[test]
    fn telegram_typing_failure_can_restart_on_a_later_delta() {
        let mut runtime = RuntimeState::default();
        let generation = runtime
            .start_telegram_typing("thread", "item")
            .expect("typing update");
        let (_, failed_revision) = runtime
            .take_telegram_typing_snapshot("thread", "item", generation, 1_000)
            .expect("typing snapshot");

        assert_eq!(
            runtime.complete_telegram_typing_send(
                "thread",
                "item",
                generation,
                failed_revision,
                false,
            ),
            TelegramTypingSendAction::Stop
        );
        assert_eq!(
            runtime.start_telegram_typing("thread", "item"),
            Some(generation)
        );
        assert_eq!(
            runtime.telegram_typing_send_delay_ms("thread", "item", generation, 1_100, 300),
            Some(200)
        );
        let (finished, restarted_revision) = runtime
            .take_telegram_typing_snapshot("thread", "item", generation, 1_300)
            .expect("restarted typing snapshot");
        assert!(!finished);
        assert!(restarted_revision > failed_revision);
    }

    #[test]
    fn clearing_a_thread_tombstones_typing_before_a_retry_can_revive_it() {
        let mut runtime = RuntimeState::default();
        let generation = runtime
            .start_telegram_typing("thread", "turn:turn-1")
            .expect("typing update");
        let (_, revision) = runtime
            .take_telegram_typing_snapshot("thread", "turn:turn-1", generation, 1_000)
            .expect("typing snapshot");
        assert_eq!(
            runtime.complete_telegram_typing_send(
                "thread",
                "turn:turn-1",
                generation,
                revision,
                false,
            ),
            TelegramTypingSendAction::Stop
        );

        runtime.clear_telegram_typing_for_thread("thread");

        assert!(!runtime.telegram_typing_item_is_active("thread", "turn:turn-1"));
        assert!(
            runtime
                .start_telegram_typing("thread", "turn:turn-1")
                .is_none()
        );
    }

    #[test]
    fn cancelling_typing_requires_the_current_generation() {
        let mut runtime = RuntimeState::default();
        let generation = runtime
            .start_telegram_typing("thread", "item")
            .expect("typing update");

        assert!(!runtime.cancel_telegram_typing_generation(
            "thread",
            "item",
            generation.saturating_add(1)
        ));
        assert!(runtime.telegram_typing_item_is_active("thread", "item"));
        assert!(runtime.cancel_telegram_typing_generation("thread", "item", generation));
        assert!(!runtime.telegram_typing_item_is_active("thread", "item"));
        assert!(runtime.start_telegram_typing("thread", "item").is_none());
    }

    #[test]
    fn telegram_completed_item_rejects_late_typing_without_a_prior_state() {
        let mut runtime = RuntimeState::default();

        assert!(
            runtime
                .finish_telegram_typing("thread", "completed-item")
                .is_none()
        );
        assert!(
            runtime
                .start_telegram_typing("thread", "completed-item")
                .is_none()
        );
        assert!(
            runtime
                .start_telegram_typing("thread", "new-item")
                .is_some()
        );
    }

    #[tokio::test]
    async fn telegram_typing_completion_notifies_the_persistent_message_waiter() {
        let mut runtime = RuntimeState::default();
        let generation = runtime
            .start_telegram_typing("thread", "item")
            .expect("typing update");
        let (_, first_revision) = runtime
            .take_telegram_typing_snapshot("thread", "item", generation, 1_000)
            .expect("initial typing snapshot");
        assert_eq!(
            runtime.complete_telegram_typing_send(
                "thread",
                "item",
                generation,
                first_revision,
                true,
            ),
            TelegramTypingSendAction::Continue
        );
        let (_, should_start, completed) = runtime
            .finish_telegram_typing("thread", "item")
            .expect("typing completion");
        assert!(!should_start);
        assert!(runtime.mark_telegram_typing_renewal_due("thread", "item", generation));
        let (finished, final_revision) = runtime
            .take_telegram_typing_snapshot("thread", "item", generation, 1_001)
            .expect("internal completion snapshot");
        assert!(finished);
        assert_eq!(
            runtime.complete_telegram_typing_send(
                "thread",
                "item",
                generation,
                final_revision,
                true,
            ),
            TelegramTypingSendAction::Stop
        );

        tokio::time::timeout(std::time::Duration::from_millis(50), completed.notified())
            .await
            .expect("typing completion should wake the persistent message sender");
    }

    #[tokio::test]
    async fn telegram_typing_failure_during_finish_does_not_hang() {
        let mut runtime = RuntimeState::default();
        let generation = runtime
            .start_telegram_typing("thread", "item")
            .expect("typing update");
        let (_, in_flight_revision) = runtime
            .take_telegram_typing_snapshot("thread", "item", generation, 1_000)
            .expect("in-flight typing snapshot");
        let (_, should_start, completed) = runtime
            .finish_telegram_typing("thread", "item")
            .expect("typing completion");
        assert!(!should_start);

        assert_eq!(
            runtime.complete_telegram_typing_send(
                "thread",
                "item",
                generation,
                in_flight_revision,
                false,
            ),
            TelegramTypingSendAction::Stop
        );
        assert!(!runtime.telegram_typing_item_is_active("thread", "item"));
        tokio::time::timeout(std::time::Duration::from_millis(50), completed.notified())
            .await
            .expect("typing failure during finish should still wake the final sender");
    }

    #[tokio::test]
    async fn telegram_typing_finish_restarts_a_stopped_driver() {
        let mut runtime = RuntimeState::default();
        let generation = runtime
            .start_telegram_typing("thread", "item")
            .expect("typing update");
        let (_, failed_revision) = runtime
            .take_telegram_typing_snapshot("thread", "item", generation, 1_000)
            .expect("typing snapshot");
        assert_eq!(
            runtime.complete_telegram_typing_send(
                "thread",
                "item",
                generation,
                failed_revision,
                false,
            ),
            TelegramTypingSendAction::Stop
        );

        let (_, should_start, completed) = runtime
            .finish_telegram_typing("thread", "item")
            .expect("typing completion");
        assert!(should_start);
        let (finished, final_revision) = runtime
            .take_telegram_typing_snapshot("thread", "item", generation, 1_001)
            .expect("internal completion snapshot");
        assert!(finished);
        assert_eq!(
            runtime.complete_telegram_typing_send(
                "thread",
                "item",
                generation,
                final_revision,
                true,
            ),
            TelegramTypingSendAction::Stop
        );
        tokio::time::timeout(std::time::Duration::from_millis(50), completed.notified())
            .await
            .expect("completion should restart a stopped typing driver and wake the final sender");
    }

    #[test]
    fn completing_a_turn_clears_its_single_telegram_typing_driver() {
        let mut runtime = RuntimeState::default();
        let first = runtime
            .start_telegram_typing("thread", "item-1")
            .expect("first typing state");
        assert!(runtime.start_telegram_typing("thread", "item-2").is_none());
        runtime
            .start_telegram_typing("other-thread", "item")
            .expect("other typing state");

        runtime.mark_turn_completed("thread", None);

        assert!(
            runtime
                .take_telegram_typing_snapshot("thread", "item-1", first, 1_000)
                .is_none()
        );
        assert_eq!(runtime.telegram_typing_by_item.len(), 1);
    }

    #[test]
    fn approval_request_lifecycle_survives_replay_until_resolved() {
        let mut runtime = RuntimeState::default();
        let route = "feishu:default:open_id:ou_test".to_string();

        assert!(runtime.push_approval(route.clone(), approval(7)));
        assert!(!runtime.push_approval(route.clone(), approval(7)));
        assert_eq!(
            runtime
                .pending_approvals_by_conversation
                .get(&route)
                .map(Vec::len),
            Some(1)
        );

        runtime.resolve_approval_request(&json!(7));
        assert!(runtime.push_approval(route, approval(7)));
    }

    #[test]
    fn approval_reply_targets_current_request() {
        let mut runtime = RuntimeState::default();
        let route = "feishu:default:open_id:ou_test".to_string();

        assert!(runtime.push_approval(route.clone(), approval(1)));
        assert!(runtime.push_approval(route.clone(), approval(2)));

        let current = runtime
            .current_approval(&route)
            .expect("current approval should exist");
        assert_eq!(current.request_id, json!(1));

        let resolved = runtime
            .resolve_approval_request_with_context(&json!(1))
            .expect("current approval should resolve");
        assert_eq!(resolved.approval.request_id, json!(1));
        assert!(resolved.was_current);
        assert_eq!(
            resolved
                .next_current
                .expect("queued approval should become current")
                .request_id,
            json!(2)
        );

        let remaining = runtime
            .current_approval(&route)
            .expect("queued approval should remain until resolved");
        assert_eq!(remaining.request_id, json!(2));
    }

    #[test]
    fn approval_can_be_resolved_by_request_key_without_chat_key() {
        let mut runtime = RuntimeState::default();
        let route = "feishu:default:open_id:ou_test".to_string();
        assert!(runtime.push_approval(route.clone(), approval(42)));

        let (found_route, pending) = runtime
            .approval_by_request_key_anywhere("number:42")
            .expect("approval should be found globally");
        assert_eq!(found_route, route);
        assert_eq!(pending.request_id, json!(42));
    }

    #[test]
    fn turn_origin_is_removed_when_turn_completes() {
        let mut runtime = RuntimeState::default();
        runtime.mark_turn_started("thread-1", "turn-1");
        runtime.remember_turn_origin("turn-1", TurnOrigin::Feishu);

        assert_eq!(runtime.turn_origin("turn-1"), Some(TurnOrigin::Feishu));

        runtime.mark_turn_completed("thread-1", Some("turn-1"));

        assert_eq!(runtime.turn_origin("turn-1"), None);
    }

    #[test]
    fn turn_starting_blocks_parallel_starts_and_expires_old_messages() {
        let mut runtime = RuntimeState::default();
        assert!(runtime.try_mark_turn_starting("thread-1").is_ok());
        assert_eq!(
            runtime.try_mark_turn_starting("thread-1"),
            Err(ThreadTurnState::Starting)
        );

        let before_turn = 1;
        runtime.mark_turn_started("thread-1", "turn-1");
        assert_eq!(
            runtime.try_mark_turn_starting("thread-1"),
            Err(ThreadTurnState::Running("turn-1".to_string()))
        );

        runtime.mark_turn_completed("thread-1", Some("turn-1"));
        assert!(runtime.message_is_stale_for_latest_turn("thread-1", before_turn));
        assert!(runtime.try_mark_turn_starting("thread-1").is_ok());
    }

    #[test]
    fn route_from_conversation_key_preserves_platform() {
        let feishu =
            route_from_conversation_key("feishu:default:open_id:ou_test").expect("feishu route");
        assert_eq!(feishu.platform, ImPlatformKind::Feishu);
        assert_eq!(feishu.account_id, "default");
        assert_eq!(feishu.chat_id, "open_id:ou_test");
        assert!(feishu.remote_client_key.starts_with("im:feishu:"));

        let telegram =
            route_from_conversation_key("telegram:bot:chat:123").expect("telegram route");
        assert_eq!(telegram.platform, ImPlatformKind::Telegram);
        assert_eq!(telegram.account_id, "bot");
        assert_eq!(telegram.chat_id, "chat:123");
        assert!(telegram.remote_client_key.starts_with("im:telegram:"));

        let wecom =
            route_from_conversation_key("wecom:corp-bot:group:room-1").expect("wecom route");
        assert_eq!(wecom.platform, ImPlatformKind::Wecom);
        assert_eq!(wecom.account_id, "corp-bot");
        assert_eq!(wecom.chat_id, "group:room-1");
        assert!(wecom.remote_client_key.starts_with("im:wecom:"));

        assert!(route_from_conversation_key("slack:team:channel").is_none());
    }

    #[test]
    fn im_remote_client_key_is_deterministic_and_route_scoped() {
        let feishu_key = RouteTarget::deterministic_remote_client_key_for(
            ImPlatformKind::Feishu,
            "default",
            "chat-1",
        );
        assert_eq!(
            feishu_key,
            RouteTarget::deterministic_remote_client_key_for(
                ImPlatformKind::Feishu,
                "default",
                "chat-1"
            )
        );
        assert!(feishu_key.starts_with("im:feishu:"));
        assert_ne!(
            feishu_key,
            RouteTarget::deterministic_remote_client_key_for(
                ImPlatformKind::Wechat,
                "default",
                "chat-1"
            )
        );
        assert_ne!(
            feishu_key,
            RouteTarget::deterministic_remote_client_key_for(
                ImPlatformKind::Feishu,
                "default",
                "chat-2"
            )
        );
    }

    fn model_switch_request(
        request_id: &str,
        conversation_key: &str,
        thread_id: &str,
    ) -> TelegramModelSwitchRequestState {
        TelegramModelSwitchRequestState {
            request_id: request_id.to_string(),
            conversation_key: conversation_key.to_string(),
            account_id: "bot".to_string(),
            chat_id: "chat".to_string(),
            expected_thread_id: thread_id.to_string(),
            remote_client_key: "im:telegram:bot:chat".to_string(),
            catalog: vec![TelegramThreadSettingsModelChoice {
                model: "gpt-test".to_string(),
                label: "GPT Test".to_string(),
                supported_efforts: vec!["medium".to_string()],
                default_effort: Some("medium".to_string()),
                supports_fast: true,
            }],
            observed: Default::default(),
            draft: Default::default(),
            revision: 1,
            expires_at_ms: crate::types::now_ms() + TELEGRAM_THREAD_SETTINGS_DRAFT_MAX_AGE_MS,
            stage: TelegramThreadSettingsStage::Overview,
            model_page: 1,
            compatibility: None,
            pending_apply: None,
            stale: false,
            message_id: None,
        }
    }

    fn thread_routing_request(
        request_id: &str,
        conversation_key: &str,
    ) -> ThreadRoutingRequestState {
        ThreadRoutingRequestState {
            request_id: request_id.to_string(),
            conversation_key: conversation_key.to_string(),
            account_id: "bot".to_string(),
            chat_id: "chat".to_string(),
            message_id: None,
            stage: ThreadRoutingStage::Choice,
            page: 1,
            page_cursors: vec![None],
            thread_ids_by_page: vec![vec![]],
            create_draft: Default::default(),
            create_option_values_by_field_page: Default::default(),
            history_cursor: None,
            history_has_next: false,
        }
    }

    #[test]
    fn telegram_model_switch_request_lifecycle_invalidates_older_menu() {
        let mut runtime = RuntimeState::default();
        let first = model_switch_request("model-1", "telegram:bot:chat", "thread-1");
        runtime.remember_telegram_model_switch_request(first);

        assert!(runtime.is_current_telegram_model_switch_request("model-1", "telegram:bot:chat"));
        assert!(
            runtime.update_telegram_model_switch_request_message_id("model-1", "101".to_string())
        );
        assert_eq!(
            runtime
                .telegram_model_switch_request("model-1")
                .and_then(|request| request.message_id),
            Some("101".to_string())
        );

        runtime.remember_telegram_model_switch_request(model_switch_request(
            "model-2",
            "telegram:bot:chat",
            "thread-1",
        ));
        assert!(runtime.telegram_model_switch_request("model-1").is_none());
        assert!(!runtime.is_current_telegram_model_switch_request("model-1", "telegram:bot:chat"));
        assert!(runtime.is_current_telegram_model_switch_request("model-2", "telegram:bot:chat"));

        let cleared = runtime
            .clear_telegram_model_switch_request("model-2")
            .expect("current model picker should clear");
        assert_eq!(cleared.expected_thread_id, "thread-1");
        assert!(!runtime.is_current_telegram_model_switch_request("model-2", "telegram:bot:chat"));
    }

    #[test]
    fn telegram_thread_settings_drafts_are_cleared_when_route_is_rebound_but_not_turn_start() {
        let mut runtime = RuntimeState::default();
        runtime.remember_telegram_model_switch_request(model_switch_request(
            "model-1",
            "telegram:bot:chat",
            "thread-1",
        ));
        runtime.bind_route(
            "thread-1",
            RouteTarget {
                platform: ImPlatformKind::Telegram,
                conversation_key: "telegram:bot:chat".to_string(),
                account_id: "bot".to_string(),
                chat_id: "chat".to_string(),
                remote_client_key: "im:telegram:bot:chat".to_string(),
            },
        );
        assert!(runtime.telegram_model_switch_request("model-1").is_none());

        runtime.remember_telegram_model_switch_request(model_switch_request(
            "model-2",
            "telegram:bot:chat",
            "thread-1",
        ));
        runtime
            .try_mark_turn_starting("thread-1")
            .expect("thread should be idle");
        assert!(runtime.telegram_model_switch_request("model-2").is_some());
    }

    #[test]
    fn matching_thread_settings_echo_confirms_pending_apply() {
        let mut runtime = RuntimeState::default();
        runtime.observe_thread_settings(
            "thread-1",
            ThreadSettingsSnapshot {
                model: ObservedSetting::Known(Some("gpt-old".to_string())),
                effort: ObservedSetting::Known(Some("low".to_string())),
                service_tier: ObservedSetting::Known(None),
            },
        );
        let mut request = model_switch_request("model-1", "telegram:bot:chat", "thread-1");
        request.draft.model = Some("gpt-test".to_string());
        runtime.remember_telegram_model_switch_request(request);
        let pending = runtime
            .claim_telegram_thread_settings_apply(
                "model-1",
                1,
                TelegramThreadSettingsPatch {
                    model: TelegramThreadSettingsPatchValue::Set("gpt-test".to_string()),
                    ..Default::default()
                },
                1,
            )
            .expect("apply should be claimed");
        assert!(pending.pending_apply.is_some());

        let outcome = runtime.observe_thread_settings(
            "thread-1",
            ThreadSettingsSnapshot {
                model: ObservedSetting::Known(Some("gpt-test".to_string())),
                ..Default::default()
            },
        );
        assert!(matches!(
            outcome,
            TelegramThreadSettingsObservation::Confirmed(_)
        ));
        assert!(runtime.telegram_model_switch_request("model-1").is_none());
    }

    #[test]
    fn initial_settings_observation_does_not_stale_a_draft() {
        let mut runtime = RuntimeState::default();
        let mut request = model_switch_request("model-1", "telegram:bot:chat", "thread-1");
        request.draft.effort = Some("medium".to_string());
        runtime.remember_telegram_model_switch_request(request);

        let outcome = runtime.observe_thread_settings(
            "thread-1",
            ThreadSettingsSnapshot {
                effort: ObservedSetting::Known(Some("low".to_string())),
                ..Default::default()
            },
        );

        assert!(matches!(outcome, TelegramThreadSettingsObservation::None));
        assert!(
            runtime
                .telegram_model_switch_request("model-1")
                .is_some_and(|request| !request.stale)
        );
    }

    #[test]
    fn external_change_to_a_drafted_field_marks_the_draft_stale() {
        let mut runtime = RuntimeState::default();
        runtime.observe_thread_settings(
            "thread-1",
            ThreadSettingsSnapshot {
                effort: ObservedSetting::Known(Some("low".to_string())),
                ..Default::default()
            },
        );
        let mut request = model_switch_request("model-1", "telegram:bot:chat", "thread-1");
        request.draft.effort = Some("medium".to_string());
        runtime.remember_telegram_model_switch_request(request);

        let outcome = runtime.observe_thread_settings(
            "thread-1",
            ThreadSettingsSnapshot {
                effort: ObservedSetting::Known(Some("high".to_string())),
                ..Default::default()
            },
        );
        assert!(matches!(
            outcome,
            TelegramThreadSettingsObservation::Stale(_)
        ));
        assert!(
            runtime
                .telegram_model_switch_request("model-1")
                .is_some_and(|request| request.stale)
        );
    }

    #[test]
    fn telegram_model_switch_requests_are_cleared_from_the_previous_conversation_on_rebind() {
        let mut runtime = RuntimeState::default();
        runtime.bind_route(
            "thread-1",
            RouteTarget {
                platform: ImPlatformKind::Telegram,
                conversation_key: "telegram:bot:old-chat".to_string(),
                account_id: "bot".to_string(),
                chat_id: "old-chat".to_string(),
                remote_client_key: "im:telegram:bot:old-chat".to_string(),
            },
        );
        runtime.remember_telegram_model_switch_request(model_switch_request(
            "model-old",
            "telegram:bot:old-chat",
            "thread-1",
        ));

        runtime.bind_route(
            "thread-1",
            RouteTarget {
                platform: ImPlatformKind::Telegram,
                conversation_key: "telegram:bot:new-chat".to_string(),
                account_id: "bot".to_string(),
                chat_id: "new-chat".to_string(),
                remote_client_key: "im:telegram:bot:new-chat".to_string(),
            },
        );

        assert!(runtime.telegram_model_switch_request("model-old").is_none());
    }

    #[test]
    fn telegram_model_and_thread_routing_menus_are_mutually_exclusive() {
        let mut runtime = RuntimeState::default();
        let conversation_key = "telegram:bot:chat";

        runtime
            .remember_thread_routing_request(thread_routing_request("route-1", conversation_key));
        runtime.remember_telegram_model_switch_request(model_switch_request(
            "model-1",
            conversation_key,
            "thread-1",
        ));
        assert!(runtime.thread_routing_request("route-1").is_none());

        runtime
            .remember_thread_routing_request(thread_routing_request("route-2", conversation_key));
        assert!(runtime.telegram_model_switch_request("model-1").is_none());
        assert!(runtime.thread_routing_request("route-2").is_some());
    }
}
