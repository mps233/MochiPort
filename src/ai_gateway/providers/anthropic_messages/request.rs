use serde_json::{Map, Value, json};

use crate::ai_gateway::error::GatewayError;
use crate::ai_gateway::model::GatewayRequest;
use crate::ai_gateway::tool_names::ToolNameMap;

use super::options::AnthropicProviderProfile;
use super::request_content::build_anthropic_messages;
use super::request_reasoning::insert_reasoning_options;
use super::request_tools::{build_anthropic_tools, convert_tool_choice_to_anthropic};
use super::types::DEFAULT_MAX_TOKENS;

pub(super) fn build_anthropic_request(
    request: &GatewayRequest,
    profile: AnthropicProviderProfile,
) -> Result<(Value, ToolNameMap), GatewayError> {
    let mut tool_name_map = ToolNameMap::default();
    let mut body = Map::new();
    body.insert("model".to_string(), json!(request.model));
    body.insert(
        "max_tokens".to_string(),
        json!(request.max_output_tokens.unwrap_or(DEFAULT_MAX_TOKENS)),
    );

    if let Some(instructions) = request
        .instructions
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        body.insert("system".to_string(), json!(instructions));
    }
    if let Some(temperature) = request.temperature {
        body.insert("temperature".to_string(), json!(temperature));
    }
    if let Some(top_p) = request.top_p {
        body.insert("top_p".to_string(), json!(top_p));
    }
    if request.stream {
        body.insert("stream".to_string(), json!(true));
    }
    insert_reasoning_options(&mut body, profile, request.reasoning.as_ref());
    validate_thinking_budget(&body)?;

    let messages = build_anthropic_messages(&request.input, &mut tool_name_map)?;
    body.insert("messages".to_string(), Value::Array(messages));

    let tools = build_anthropic_tools(request, &mut tool_name_map);
    if !tools.is_empty() {
        body.insert("tools".to_string(), Value::Array(tools));
    }
    if let Some(tool_choice) = &request.tool_choice {
        body.insert(
            "tool_choice".to_string(),
            convert_tool_choice_to_anthropic(tool_choice, &mut tool_name_map),
        );
    }
    insert_prompt_cache_control(&mut body);
    Ok((Value::Object(body), tool_name_map))
}

fn validate_thinking_budget(body: &Map<String, Value>) -> Result<(), GatewayError> {
    let Some(budget_tokens) = body
        .get("thinking")
        .and_then(|thinking| thinking.get("budget_tokens"))
        .and_then(Value::as_i64)
    else {
        return Ok(());
    };
    let max_tokens = body
        .get("max_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(DEFAULT_MAX_TOKENS);
    if budget_tokens >= max_tokens {
        return Err(GatewayError::bad_request(format!(
            "anthropic_messages thinking.budget_tokens ({budget_tokens}) must be less than max_tokens ({max_tokens})"
        )));
    }
    Ok(())
}

/// Anthropic accepts at most 4 explicit `cache_control` breakpoints per request.
const ANTHROPIC_BREAKPOINT_CAP: u8 = 4;

/// Shared breakpoint budget, mirroring OpenCode's `Cache.Breakpoints`. The 4
/// slots are allocated in cache-invalidation order (`tools` -> `system` ->
/// `messages`); tools sit highest in the prefix hierarchy, so when a request
/// would over-mark we keep the tool/system anchors and shed the message-tail
/// one first.
struct Breakpoints {
    remaining: u8,
    dropped: u8,
}

impl Breakpoints {
    fn new() -> Self {
        Self {
            remaining: ANTHROPIC_BREAKPOINT_CAP,
            dropped: 0,
        }
    }

    /// Reserves one slot. Returns false (and records a drop) once the cap is hit
    /// so callers can skip marking instead of tripping a 400 from the API.
    fn take(&mut self) -> bool {
        if self.remaining == 0 {
            self.dropped += 1;
            return false;
        }
        self.remaining -= 1;
        true
    }
}

fn insert_prompt_cache_control(body: &mut Map<String, Value>) {
    // Auto cache policy aligned with OpenCode's default: one breakpoint on the
    // last tool definition, one on the last system block, and one on the latest
    // user message. These positions stay put while a single turn explodes into
    // many assistant/tool round-trips, so the growing prefix keeps hitting the
    // cache. Crucially the marker never lands on assistant/tool_use tails, whose
    // position shifts every turn -- moving a breakpoint forces the previous
    // turn's marked block to lose its cache_control, which empirically breaks
    // the prefix hash and re-writes the whole history.
    let cache_control = anthropic_cache_control();
    let mut budget = Breakpoints::new();

    if let Some(Value::Array(tools)) = body.get_mut("tools") {
        insert_tools_cache_control(tools, &cache_control, &mut budget);
    }
    if let Some(system) = body.get_mut("system") {
        insert_system_cache_control(system, &cache_control, &mut budget);
    }
    if let Some(Value::Array(messages)) = body.get_mut("messages") {
        insert_message_cache_control(messages, &cache_control, &mut budget);
    }

    if budget.dropped > 0 {
        tracing::warn!(
            dropped = budget.dropped,
            cap = ANTHROPIC_BREAKPOINT_CAP,
            "anthropic_messages dropped cache breakpoints exceeding per-request cap"
        );
    }
}

fn insert_tools_cache_control(
    tools: &mut [Value],
    cache_control: &Map<String, Value>,
    budget: &mut Breakpoints,
) {
    // Tools live at the front of the prefix; a breakpoint on the last tool caches
    // the entire tool-definition block as a stable, independently reusable prefix.
    let Some(Value::Object(last)) = tools.last_mut() else {
        return;
    };
    if last.contains_key("cache_control") {
        return;
    }
    if budget.take() {
        last.insert(
            "cache_control".to_string(),
            Value::Object(cache_control.clone()),
        );
    }
}

fn insert_system_cache_control(
    system: &mut Value,
    cache_control: &Map<String, Value>,
    budget: &mut Breakpoints,
) {
    match system {
        Value::String(text) if !text.is_empty() => {
            if !budget.take() {
                return;
            }
            let text = text.clone();
            *system = json!([{
                "type": "text",
                "text": text,
                "cache_control": cache_control,
            }]);
        }
        Value::Array(parts) => {
            // Only mark the last cacheable text block: Anthropic caches the whole
            // prefix up to a breakpoint, so a single breakpoint at the end of the
            // system section covers every earlier block while staying within the
            // 4-breakpoint per-request limit (Codex may emit many system blocks).
            let Some(Value::Object(part)) = parts.iter_mut().rev().find(|part| {
                part.as_object()
                    .map(is_cacheable_anthropic_text_block)
                    .unwrap_or(false)
            }) else {
                return;
            };
            if part.contains_key("cache_control") {
                return;
            }
            if budget.take() {
                part.insert(
                    "cache_control".to_string(),
                    Value::Object(cache_control.clone()),
                );
            }
        }
        _ => {}
    }
}

fn insert_message_cache_control(
    messages: &mut [Value],
    cache_control: &Map<String, Value>,
    budget: &mut Breakpoints,
) {
    // Single breakpoint on the latest `role=="user"` message, matching OpenCode's
    // "latest-user-message" strategy. tool_result messages are role=user, so the
    // agent-loop tail (...tool_result) is covered. Assistant/tool_use tails are
    // deliberately never marked: their index moves every turn, and re-marking a
    // shifting position strips cache_control off the block the previous turn
    // wrote, breaking the prefix hash.
    let Some(index) = messages
        .iter()
        .rposition(|message| message.get("role").and_then(Value::as_str) == Some("user"))
    else {
        return;
    };
    mark_message_breakpoint(&mut messages[index], cache_control, budget);
}

/// Marks the breakpoint block of a message: the last `type=="text"` block when
/// one exists, otherwise the last content block (covers tool_result-only user
/// messages). Idempotent — an existing cache_control is left untouched.
fn mark_message_breakpoint(
    message: &mut Value,
    cache_control: &Map<String, Value>,
    budget: &mut Breakpoints,
) {
    let Some(content) = message.get_mut("content") else {
        return;
    };
    match content {
        Value::String(text) if !text.is_empty() => {
            if !budget.take() {
                return;
            }
            let text = text.clone();
            *content = json!([{
                "type": "text",
                "text": text,
                "cache_control": cache_control,
            }]);
        }
        Value::Array(parts) if !parts.is_empty() => {
            let last_text = parts
                .iter()
                .rposition(|part| part.get("type").and_then(Value::as_str) == Some("text"));
            let index = last_text.unwrap_or(parts.len() - 1);
            let Some(Value::Object(part)) = parts.get_mut(index) else {
                return;
            };
            if part.contains_key("cache_control") {
                return;
            }
            if budget.take() {
                part.insert(
                    "cache_control".to_string(),
                    Value::Object(cache_control.clone()),
                );
            }
        }
        _ => {}
    }
}

fn is_cacheable_anthropic_text_block(block: &Map<String, Value>) -> bool {
    block.get("type").and_then(Value::as_str) == Some("text")
        && block
            .get("text")
            .and_then(Value::as_str)
            .map(|text| !text.is_empty())
            .unwrap_or(false)
}

fn anthropic_cache_control() -> Map<String, Value> {
    let mut cache_control = Map::new();
    cache_control.insert("type".to_string(), json!("ephemeral"));
    cache_control
}
