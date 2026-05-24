use std::time::{Duration, Instant};

use anyhow::Result;

use crate::{
    app_state::SharedState,
    im::feishu::{
        FeishuApi,
        renderer::{
            self, FEISHU_CARDKIT_STREAMING_ELEMENT_ID, build_cardkit_streaming_agent_message_card,
            build_cardkit_streaming_reply_card, build_cardkit_streaming_tool_card,
        },
        types::FeishuStreamingCardState,
    },
};

const FEISHU_CARDKIT_THROTTLE_MS: u64 = 100;
const FEISHU_CARDKIT_LONG_GAP_THRESHOLD_MS: u64 = 2000;
const FEISHU_CARDKIT_BATCH_AFTER_GAP_MS: u64 = 300;

fn streaming_card_key(thread_id: &str, item_id: &str) -> String {
    format!("{thread_id}:{item_id}")
}

fn feishu_receive_target(target: &str) -> (&'static str, &str) {
    target
        .strip_prefix("open_id:")
        .map(|open_id| ("open_id", open_id))
        .unwrap_or(("chat_id", target))
}

pub async fn upsert_streaming_card_state(
    state: SharedState,
    api: FeishuApi,
    thread_id: &str,
    item_id: &str,
    kind: &str,
    account_id: &str,
    chat_id: &str,
    delta: &str,
    completed: bool,
) {
    if delta.is_empty() && !completed {
        return;
    }
    let state_key = streaming_card_key(thread_id, item_id);
    {
        let mut runtime = state.runtime.lock().await;
        let entry = runtime
            .feishu_streaming_cards_by_item
            .entry(state_key)
            .or_insert_with(|| FeishuStreamingCardState {
                account_id: account_id.to_string(),
                chat_id: chat_id.to_string(),
                receive_id_type: feishu_receive_target(chat_id).0.to_string(),
                receive_id: feishu_receive_target(chat_id).1.to_string(),
                kind: kind.to_string(),
                message_id: None,
                card_id: None,
                sequence: 0,
                text: String::new(),
                sent_text: String::new(),
                completed: false,
                sending: false,
                dirty: false,
                last_sent_at: None,
            });

        entry.account_id = account_id.to_string();
        entry.chat_id = chat_id.to_string();
        let (receive_id_type, receive_id) = feishu_receive_target(chat_id);
        entry.receive_id_type = receive_id_type.to_string();
        entry.receive_id = receive_id.to_string();
        entry.kind = kind.to_string();
        if completed {
            if !delta.is_empty() {
                entry.text = delta.to_string();
            }
            entry.completed = true;
        } else if kind == "fileSummary" {
            entry.text = delta.to_string();
        } else {
            entry.text.push_str(delta);
        }
        entry.dirty = true;
    }
    spawn_streaming_card_driver(state, api, thread_id, item_id);
}

pub async fn ensure_started_streaming_card_state(
    state: SharedState,
    api: FeishuApi,
    thread_id: &str,
    item_id: &str,
    kind: &str,
    account_id: &str,
    chat_id: &str,
    initial_text: Option<String>,
) {
    let state_key = streaming_card_key(thread_id, item_id);
    {
        let mut runtime = state.runtime.lock().await;
        let entry = runtime
            .feishu_streaming_cards_by_item
            .entry(state_key)
            .or_insert_with(|| FeishuStreamingCardState {
                account_id: account_id.to_string(),
                chat_id: chat_id.to_string(),
                receive_id_type: feishu_receive_target(chat_id).0.to_string(),
                receive_id: feishu_receive_target(chat_id).1.to_string(),
                kind: kind.to_string(),
                message_id: None,
                card_id: None,
                sequence: 0,
                text: String::new(),
                sent_text: String::new(),
                completed: false,
                sending: false,
                dirty: false,
                last_sent_at: None,
            });

        entry.account_id = account_id.to_string();
        entry.chat_id = chat_id.to_string();
        let (receive_id_type, receive_id) = feishu_receive_target(chat_id);
        entry.receive_id_type = receive_id_type.to_string();
        entry.receive_id = receive_id.to_string();
        entry.kind = kind.to_string();
        if let Some(text) = initial_text.filter(|value| !value.trim().is_empty()) {
            if entry.text.trim().is_empty() {
                entry.text = text;
            }
        }
        entry.dirty = true;
    }
    spawn_streaming_card_driver(state, api, thread_id, item_id);
}

pub async fn complete_existing_item_card(
    state: SharedState,
    api: FeishuApi,
    thread_id: &str,
    item_id: &str,
    kind: &str,
    account_id: &str,
    chat_id: &str,
    text: Option<String>,
) -> bool {
    let state_key = streaming_card_key(thread_id, item_id);
    let updated = {
        let mut runtime = state.runtime.lock().await;
        match runtime.feishu_streaming_cards_by_item.get_mut(&state_key) {
            Some(entry) => {
                entry.account_id = account_id.to_string();
                entry.chat_id = chat_id.to_string();
                let (receive_id_type, receive_id) = feishu_receive_target(chat_id);
                entry.receive_id_type = receive_id_type.to_string();
                entry.receive_id = receive_id.to_string();
                entry.kind = kind.to_string();
                if let Some(text) = text.filter(|value| !value.trim().is_empty()) {
                    entry.text = text;
                }
                entry.completed = true;
                entry.dirty = true;
                true
            }
            None => false,
        }
    };
    if updated {
        spawn_streaming_card_driver(state, api, thread_id, item_id);
    }
    updated
}

fn spawn_streaming_card_driver(state: SharedState, api: FeishuApi, thread_id: &str, item_id: &str) {
    let thread_id = thread_id.to_string();
    let item_id = item_id.to_string();
    tokio::spawn(async move {
        if let Err(err) = drive_streaming_card_state(state.clone(), api, &thread_id, &item_id).await
        {
            state
                .push_event(
                    "error",
                    "feishu_stream_drive_failed",
                    format!("thread={thread_id} item={item_id} err={err}"),
                )
                .await;
        }
    });
}

async fn drive_streaming_card_state(
    state: SharedState,
    api: FeishuApi,
    thread_id: &str,
    item_id: &str,
) -> Result<(), String> {
    let state_key = streaming_card_key(thread_id, item_id);
    loop {
        let mut state_snapshot = {
            let mut runtime = state.runtime.lock().await;
            let Some(current) = runtime.feishu_streaming_cards_by_item.get_mut(&state_key) else {
                return Ok(());
            };
            if current.sending || !current.dirty {
                return Ok(());
            }
            current.sending = true;
            current.dirty = false;
            current.clone()
        };

        if state_snapshot.message_id.is_some() {
            let elapsed = state_snapshot
                .last_sent_at
                .map(|last_sent_at| last_sent_at.elapsed())
                .unwrap_or_else(|| Duration::from_millis(FEISHU_CARDKIT_LONG_GAP_THRESHOLD_MS + 1));

            let wait_duration =
                if elapsed >= Duration::from_millis(FEISHU_CARDKIT_LONG_GAP_THRESHOLD_MS) {
                    Some(Duration::from_millis(FEISHU_CARDKIT_BATCH_AFTER_GAP_MS))
                } else if elapsed < Duration::from_millis(FEISHU_CARDKIT_THROTTLE_MS) {
                    Some(Duration::from_millis(FEISHU_CARDKIT_THROTTLE_MS) - elapsed)
                } else {
                    None
                };

            if let Some(wait_duration) = wait_duration {
                tokio::time::sleep(wait_duration).await;
                let mut runtime = state.runtime.lock().await;
                let Some(current) = runtime.feishu_streaming_cards_by_item.get_mut(&state_key)
                else {
                    return Ok(());
                };
                state_snapshot = current.clone();
                current.dirty = false;
            }
        }

        let send_result = if state_snapshot.kind == "agentMessage" {
            send_agent_message_cardkit(&api, &state_snapshot).await
        } else {
            send_interactive_streaming_card(&api, &state_snapshot).await
        };

        match send_result {
            Ok((message_id, card_id, sequence)) => {
                let mut runtime = state.runtime.lock().await;
                let mut should_remove = false;
                let mut should_continue = false;
                if let Some(current) = runtime.feishu_streaming_cards_by_item.get_mut(&state_key) {
                    if let Some(message_id) = message_id {
                        current.message_id = Some(message_id);
                    }
                    if let Some(card_id) = card_id {
                        current.card_id = Some(card_id);
                    }
                    current.sequence = sequence;
                    current.sent_text = state_snapshot.text.clone();
                    current.last_sent_at = Some(Instant::now());
                    current.sending = false;
                    should_remove =
                        current.completed && !current.dirty && current.sent_text == current.text;
                    should_continue = current.dirty;
                }
                if should_remove {
                    runtime.feishu_streaming_cards_by_item.remove(&state_key);
                    return Ok(());
                }
                if should_continue {
                    continue;
                }
                return Ok(());
            }
            Err(err) => {
                let mut runtime = state.runtime.lock().await;
                if let Some(current) = runtime.feishu_streaming_cards_by_item.get_mut(&state_key) {
                    current.sending = false;
                    current.dirty = true;
                }
                return Err(err.to_string());
            }
        }
    }
}

async fn send_agent_message_cardkit(
    api: &FeishuApi,
    state_snapshot: &FeishuStreamingCardState,
) -> Result<(Option<String>, Option<String>, u64)> {
    let (card_id, message_id, next_sequence) =
        ensure_cardkit_streaming_card(api, state_snapshot).await?;
    if state_snapshot.completed {
        let final_card = build_streaming_card(&state_snapshot.kind, &state_snapshot.text, true);
        api.update_cardkit_card(&card_id, &final_card, next_sequence)
            .await?;
        api.set_cardkit_streaming_mode(&card_id, false, next_sequence.saturating_add(1))
            .await?;
        Ok((message_id, Some(card_id), next_sequence.saturating_add(1)))
    } else if state_snapshot.text.trim().is_empty() {
        Ok((message_id, Some(card_id), next_sequence))
    } else {
        api.stream_cardkit_element_content(
            &card_id,
            FEISHU_CARDKIT_STREAMING_ELEMENT_ID,
            &state_snapshot.text,
            next_sequence,
        )
        .await?;
        Ok((message_id, Some(card_id), next_sequence))
    }
}

async fn ensure_cardkit_streaming_card(
    api: &FeishuApi,
    state_snapshot: &FeishuStreamingCardState,
) -> Result<(String, Option<String>, u64)> {
    if let Some(card_id) = state_snapshot.card_id.clone() {
        return Ok((
            card_id,
            state_snapshot.message_id.clone(),
            state_snapshot.sequence.saturating_add(1),
        ));
    }
    let card = build_cardkit_streaming_card(&state_snapshot.kind);
    let card_id = api.create_cardkit_card(&card).await?;
    let message_id = api
        .send_cardkit_message_to(
            &state_snapshot.receive_id_type,
            &state_snapshot.receive_id,
            &card_id,
        )
        .await?;
    Ok((card_id, Some(message_id), 1))
}

async fn send_interactive_streaming_card(
    api: &FeishuApi,
    state_snapshot: &FeishuStreamingCardState,
) -> Result<(Option<String>, Option<String>, u64)> {
    let card = build_streaming_card(
        &state_snapshot.kind,
        &state_snapshot.text,
        state_snapshot.completed,
    );
    match state_snapshot.message_id.as_deref() {
        Some(message_id) => {
            api.update_interactive_message(message_id, &card).await?;
            Ok((Some(message_id.to_string()), None, state_snapshot.sequence))
        }
        None => {
            let message_id = api
                .send_interactive_message_to(
                    &state_snapshot.receive_id_type,
                    &state_snapshot.receive_id,
                    &card,
                )
                .await?;
            Ok((Some(message_id), None, state_snapshot.sequence))
        }
    }
}

fn build_streaming_card(kind: &str, text: &str, completed: bool) -> serde_json::Value {
    match kind {
        "plan" => renderer::build_streaming_plan_card(text, completed),
        "reasoning" => renderer::build_streaming_reasoning_card(text, completed),
        "commandExecution" => renderer::build_streaming_command_card(text, completed),
        "fileChange" => renderer::build_streaming_file_change_card(text, completed),
        "fileSummary" => renderer::build_streaming_file_summary_card(text, completed),
        "mcpToolCall" => renderer::build_streaming_mcp_tool_card(text, completed),
        _ => renderer::build_streaming_reply_card(text, completed),
    }
}

fn build_cardkit_streaming_card(kind: &str) -> serde_json::Value {
    match kind {
        "agentMessage" => build_cardkit_streaming_agent_message_card(),
        "commandExecution" => build_cardkit_streaming_tool_card("命令执行", "lime", "状态：执行中"),
        "fileChange" => build_cardkit_streaming_tool_card("文件变更", "green", "状态：处理中"),
        "fileSummary" => build_cardkit_streaming_reply_card(),
        _ => build_cardkit_streaming_reply_card(),
    }
}
