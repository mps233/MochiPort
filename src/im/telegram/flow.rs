use anyhow::{Context, Result};
use tokio::time::{Duration, sleep};
use tracing::info;

use crate::{
    app_state::SharedState,
    config::TelegramReplyGranularity,
    im::core::{
        approval::{
            ApprovalReplyOutcome, resolve_approval_button_reply, resolve_approval_reply,
            submit_approval_decision,
        },
        i18n::{ImText, im_text_for_state},
        outbound::ImOutboundSender,
        routing::{
            active_turn_for_message, clear_thread_binding, live_thread_for_route,
            remote_client_key_for_thread, route_for_message, turn_in_progress_for_message,
        },
        session::{create_and_bind_thread, resume_and_bind_thread},
        thread::{
            ThreadCreateForm, apply_thread_create_draft_value, create_options_for_field,
            expand_home_prefix, is_approval_reply, load_thread_create_defaults_for_client,
            load_thread_model_settings_choices_for_client, next_thread_routing_request_id,
            normalize_thread_create_field, summarize_thread_cwd, summarize_thread_start_options,
            summarize_thread_status, summarize_thread_title, thread_create_form_from_draft,
            thread_create_help_text, thread_start_options_from_form_for_client,
            thread_start_options_with_current_provider,
        },
        thread_list::{empty_thread_routing_request, load_thread_routing_page},
        turn::{TurnStartOutcome, start_turn_for_route},
    },
    im::events,
    im::telegram::{
        adapter::{TelegramAdapter, TelegramThreadListEntry},
        api::TelegramApi,
        types::TelegramSettings,
        typing as telegram_typing,
    },
    im_runtime::{
        PendingTelegramTurn, RouteTarget, TELEGRAM_QUEUED_TURNS_MAX_COUNT,
        TelegramModelSwitchRequestState, TelegramQueueEnqueueOutcome,
        TelegramThreadSettingsCompatibility, TelegramThreadSettingsDraft,
        TelegramThreadSettingsModelChoice, TelegramThreadSettingsPatch,
        TelegramThreadSettingsPatchValue, TelegramThreadSettingsSpeed, TelegramThreadSettingsStage,
        ThreadRoutingRequestState, ThreadRoutingStage, TurnOrigin,
        next_telegram_model_switch_request_id,
    },
    remote_control_backend,
    types::{
        InboundAction, InboundMessage, ThreadRouteDirection, ThreadSettingsField, now_ms,
        split_telegram_message_target,
    },
};

const TELEGRAM_CREATE_OPTION_PAGE_SIZE: usize = 8;
const TELEGRAM_MODEL_PAGE_SIZE: usize = 8;
const TELEGRAM_THREAD_SETTINGS_APPLY_CONFIRMATION_TIMEOUT: Duration = Duration::from_secs(5);

fn approval_decision_fallback_text(text: ImText, label: &str) -> String {
    text.approval_decision_submitted_label(&text.approval_decision_label(label))
}

fn command_payload(text: &str) -> &str {
    text.find(char::is_whitespace)
        .map(|index| text[index..].trim())
        .unwrap_or_default()
}

#[derive(Debug, PartialEq, Eq)]
enum TelegramThreadRoutingResultDelivery {
    Updated,
    SentAsNew {
        message_id: String,
        update_error: String,
    },
    Undelivered {
        update_error: String,
        fallback_error: String,
    },
}

#[async_trait::async_trait]
trait TelegramThreadRoutingResultSender: Sync {
    async fn send_thread_routing_result(
        &self,
        target: &str,
        title: &str,
        body: &str,
        message_id: Option<&str>,
    ) -> Result<String>;
}

#[async_trait::async_trait]
impl TelegramThreadRoutingResultSender for TelegramAdapter {
    async fn send_thread_routing_result(
        &self,
        target: &str,
        title: &str,
        body: &str,
        message_id: Option<&str>,
    ) -> Result<String> {
        TelegramAdapter::send_thread_routing_result(self, target, title, body, message_id).await
    }
}

async fn deliver_telegram_thread_routing_result(
    sender: &(impl TelegramThreadRoutingResultSender + ?Sized),
    target: &str,
    title: &str,
    body: &str,
    progress_message_id: &str,
) -> TelegramThreadRoutingResultDelivery {
    let update_error = match sender
        .send_thread_routing_result(target, title, body, Some(progress_message_id))
        .await
    {
        Ok(_) => return TelegramThreadRoutingResultDelivery::Updated,
        Err(error) => error.to_string(),
    };

    match sender
        .send_thread_routing_result(target, title, body, None)
        .await
    {
        Ok(message_id) => TelegramThreadRoutingResultDelivery::SentAsNew {
            message_id,
            update_error,
        },
        Err(error) => TelegramThreadRoutingResultDelivery::Undelivered {
            update_error,
            fallback_error: error.to_string(),
        },
    }
}

async fn publish_telegram_thread_routing_result(
    state: &SharedState,
    adapter: &TelegramAdapter,
    chat_id: &str,
    conversation_key: &str,
    thread_id: &str,
    title: &str,
    body: &str,
    progress_message_id: &str,
    success_event_kind: &str,
) {
    let delivery =
        deliver_telegram_thread_routing_result(adapter, chat_id, title, body, progress_message_id)
            .await;
    match delivery {
        TelegramThreadRoutingResultDelivery::Updated => {}
        TelegramThreadRoutingResultDelivery::SentAsNew {
            message_id,
            update_error,
        } => {
            let event_kind = format!("{success_event_kind}_status_update_failed");
            state
                .push_event(
                    "warn",
                    &event_kind,
                    format!(
                        "conversation={conversation_key} thread={thread_id} message={progress_message_id} fallback=sent_as_new fallback_message={message_id} update_err={update_error}"
                    ),
                )
                .await;
        }
        TelegramThreadRoutingResultDelivery::Undelivered {
            update_error,
            fallback_error,
        } => {
            let update_event_kind = format!("{success_event_kind}_status_update_failed");
            state
                .push_event(
                    "warn",
                    &update_event_kind,
                    format!(
                        "conversation={conversation_key} thread={thread_id} message={progress_message_id} fallback=send_new_failed update_err={update_error}"
                    ),
                )
                .await;
            let fallback_event_kind = format!("{success_event_kind}_status_fallback_failed");
            state
                .push_event(
                    "warn",
                    &fallback_event_kind,
                    format!(
                        "conversation={conversation_key} thread={thread_id} fallback_err={fallback_error}"
                    ),
                )
                .await;
        }
    }
}

async fn steer_telegram_turn(
    state: &SharedState,
    adapter: &TelegramAdapter,
    message: &InboundMessage,
    text: &str,
    attachments: &[crate::types::InboundAttachment],
) -> Result<()> {
    let Some((thread_id, turn_id)) = active_turn_for_message(state, message).await else {
        let notice = if turn_in_progress_for_message(state, message).await {
            im_text_for_state(state).telegram_turn_starting_notice()
        } else {
            im_text_for_state(state).no_running_turn()
        };
        adapter.send_text(&message.chat_id, notice).await?;
        return Ok(());
    };
    let Some(remote_client_key) = remote_client_key_for_thread(state, &thread_id).await else {
        adapter
            .send_text(
                &message.chat_id,
                im_text_for_state(state).telegram_steer_failed(),
            )
            .await?;
        return Ok(());
    };
    match remote_control_backend::steer_turn_for_client(
        state,
        &remote_client_key,
        &thread_id,
        &turn_id,
        text,
        attachments,
    )
    .await
    {
        Ok(steered_turn_id) => {
            state
                .push_event(
                    "info",
                    "telegram_turn_steered",
                    format!(
                        "conversation={} thread={} turn={} response_turn={}",
                        message.conversation_key(),
                        thread_id,
                        turn_id,
                        steered_turn_id
                    ),
                )
                .await;
            adapter
                .send_text(
                    &message.chat_id,
                    im_text_for_state(state).telegram_steer_accepted(),
                )
                .await?;
        }
        Err(err) => {
            state
                .push_event(
                    "warn",
                    "telegram_turn_steer_failed",
                    format!(
                        "conversation={} thread={} turn={} err={err}",
                        message.conversation_key(),
                        thread_id,
                        turn_id
                    ),
                )
                .await;
            adapter
                .send_text(
                    &message.chat_id,
                    im_text_for_state(state).telegram_steer_failed(),
                )
                .await?;
        }
    }
    Ok(())
}

pub(crate) async fn start_next_telegram_queued_turn(
    state: &SharedState,
    api: TelegramApi,
    route: &RouteTarget,
) {
    if route.platform != crate::types::ImPlatformKind::Telegram {
        return;
    }
    // Dequeue and turn/start must stay ordered because turn/start is not idempotent.
    let _queue_start_guard = state.telegram_queue_start_ops.lock().await;
    loop {
        let queued = state
            .runtime
            .lock()
            .await
            .take_next_telegram_turn(&route.conversation_key);
        let Some(queued) = queued else {
            return;
        };
        let outcome = start_turn_for_route(
            state,
            route,
            &queued.text,
            &queued.attachments,
            crate::types::now_ms(),
            TurnOrigin::Telegram,
        )
        .await;
        match outcome {
            TurnStartOutcome::Started {
                thread_id: started_thread_id,
                turn_id,
            } => {
                telegram_typing::start_turn(
                    state,
                    api.clone(),
                    &started_thread_id,
                    &turn_id,
                    route,
                )
                .await;
                let adapter = TelegramAdapter::new(api.clone());
                let _ = adapter
                    .send_text(
                        &route.chat_id,
                        im_text_for_state(state).telegram_queue_started(),
                    )
                    .await;
                state
                    .push_event(
                        "info",
                        "telegram_queued_turn_started",
                        format!(
                            "conversation={} thread={} turn={} remaining={}",
                            route.conversation_key,
                            started_thread_id,
                            turn_id,
                            state
                                .runtime
                                .lock()
                                .await
                                .telegram_queue_len(&route.conversation_key)
                        ),
                    )
                    .await;
                return;
            }
            TurnStartOutcome::Busy => {
                state
                    .runtime
                    .lock()
                    .await
                    .requeue_telegram_turn_front(&route.conversation_key, queued);
                return;
            }
            TurnStartOutcome::NoThread | TurnStartOutcome::Stale { .. } => {
                state
                    .runtime
                    .lock()
                    .await
                    .clear_telegram_queue(&route.conversation_key);
                let adapter = TelegramAdapter::new(api.clone());
                let _ = adapter
                    .send_text(
                        &route.chat_id,
                        im_text_for_state(state).stale_thread_unbound(),
                    )
                    .await;
                return;
            }
            TurnStartOutcome::Expired { .. } => {
                let adapter = TelegramAdapter::new(api.clone());
                let _ = adapter
                    .send_text(&route.chat_id, im_text_for_state(state).inbound_expired())
                    .await;
            }
            TurnStartOutcome::Failed { error } => {
                let adapter = TelegramAdapter::new(api.clone());
                let _ = adapter
                    .send_text(
                        &route.chat_id,
                        &im_text_for_state(state).telegram_queue_start_failed(&error.to_string()),
                    )
                    .await;
                state
                    .push_event(
                        "warn",
                        "telegram_queued_turn_start_failed",
                        format!(
                            "conversation={} remaining={} err={error}",
                            route.conversation_key,
                            state
                                .runtime
                                .lock()
                                .await
                                .telegram_queue_len(&route.conversation_key)
                        ),
                    )
                    .await;
            }
        }
    }
}

pub(crate) async fn handle_inbound(
    state: SharedState,
    outbound_tx: ImOutboundSender,
    message: InboundMessage,
) -> Result<()> {
    info!(
        "inbound telegram message chat={} sender={}",
        message.chat_id, message.sender_id
    );
    state
        .push_event(
            "info",
            "telegram_message",
            format!(
                "chat={} sender={} text_len={}",
                message.chat_id,
                message.sender_id,
                message.text.chars().count()
            ),
        )
        .await;

    let config = state.config.lock().await.clone();
    let Some(telegram_config) = config.telegram_account(&message.account_id) else {
        return Ok(());
    };
    let api = TelegramApi::new(TelegramSettings::from_app_config(&telegram_config));
    let adapter = TelegramAdapter::new(api.clone());
    let trimmed = message.text.trim();
    let route = route_for_message(&message);
    let text = im_text_for_state(&state);
    {
        let mut runtime = state.runtime.lock().await;
        runtime.last_route = Some(route.clone());
    }
    if let Some(action) = message.action.clone() {
        return handle_inbound_action(state, outbound_tx, adapter, message, action).await;
    }

    if handle_telegram_thread_create_text_input(&state, &adapter, &message, trimmed).await? {
        return Ok(());
    }

    let command = command(trimmed);
    if let Some(command) = command.as_deref()
        && handle_telegram_thread_create_option_text_reply(
            state.clone(),
            adapter.clone(),
            message.clone(),
            command,
        )
        .await?
    {
        return Ok(());
    }
    if let Some(command) = command.as_deref()
        && is_approval_reply(command)
        && state
            .runtime
            .lock()
            .await
            .has_pending_approvals(&message.conversation_key())
    {
        handle_telegram_approval_text_reply(&state, &outbound_tx, &adapter, &message, command)
            .await?;
        return Ok(());
    }
    if let Some(command) = command.as_deref()
        && handle_telegram_thread_list_text_reply(
            state.clone(),
            adapter.clone(),
            message.clone(),
            command,
        )
        .await?
    {
        return Ok(());
    }
    if let Some(command) = command.as_deref()
        && is_approval_reply(command)
    {
        handle_telegram_approval_text_reply(&state, &outbound_tx, &adapter, &message, command)
            .await?;
        return Ok(());
    }

    match command.as_deref() {
        Some("/help") | Some("/start") => {
            adapter
                .send_text(&message.chat_id, text.telegram_help())
                .await?;
            return Ok(());
        }
        Some("/status") => {
            let thread_id = live_thread_for_route(&state, &route).await;
            let (running, waiting_approval, queued) = {
                let runtime = state.runtime.lock().await;
                let running = thread_id
                    .as_ref()
                    .is_some_and(|thread_id| runtime.turn_in_progress(thread_id));
                let waiting_approval = runtime.current_approval(&route.conversation_key).is_some();
                let queued = runtime.telegram_queue_len(&route.conversation_key);
                (running, waiting_approval, queued)
            };
            let remote_status = remote_control_backend::status_snapshot(&state).await;
            adapter
                .send_text(
                    &message.chat_id,
                    &text.telegram_status(
                        remote_status.connected,
                        thread_id.as_deref(),
                        text.telegram_task_status(running, waiting_approval),
                        queued,
                    ),
                )
                .await?;
            return Ok(());
        }
        Some("/new") => {
            if turn_in_progress_for_message(&state, &message).await {
                adapter
                    .send_text(&message.chat_id, text.telegram_turn_busy_notice())
                    .await?;
                return Ok(());
            }
            let remote_status = remote_control_backend::status_snapshot(&state).await;
            if !remote_status.connected {
                adapter
                    .send_text(&message.chat_id, text.remote_not_connected())
                    .await?;
                return Ok(());
            }
            send_telegram_thread_create_settings(&state, &adapter, &message, None).await?;
            return Ok(());
        }
        Some("/sessions") => {
            if turn_in_progress_for_message(&state, &message).await {
                adapter
                    .send_text(&message.chat_id, text.telegram_turn_busy_notice())
                    .await?;
                return Ok(());
            }
            send_telegram_thread_routing_list(&state, &adapter, &message, None, None, 1).await?;
            return Ok(());
        }
        Some("/model") => {
            send_telegram_thread_settings(&state, &adapter, &message).await?;
            return Ok(());
        }
        Some("/steer") => {
            let payload = command_payload(trimmed);
            if payload.is_empty() && message.attachments.is_empty() {
                adapter
                    .send_text(&message.chat_id, text.telegram_steer_usage())
                    .await?;
                return Ok(());
            }
            steer_telegram_turn(&state, &adapter, &message, payload, &message.attachments).await?;
            return Ok(());
        }
        Some("/queue") => {
            let payload = command_payload(trimmed);
            if payload.is_empty() && message.attachments.is_empty() {
                adapter
                    .send_text(&message.chat_id, text.telegram_queue_usage())
                    .await?;
                return Ok(());
            }
            let outcome = state.runtime.lock().await.enqueue_telegram_turn_if_active(
                &route.conversation_key,
                PendingTelegramTurn {
                    text: payload.to_string(),
                    attachments: message.attachments.clone(),
                },
            );
            match outcome {
                TelegramQueueEnqueueOutcome::Added(position) => {
                    adapter
                        .send_text(&message.chat_id, &text.telegram_queue_added(position))
                        .await?;
                }
                TelegramQueueEnqueueOutcome::Full => {
                    adapter
                        .send_text(
                            &message.chat_id,
                            &text.telegram_queue_full(TELEGRAM_QUEUED_TURNS_MAX_COUNT),
                        )
                        .await?;
                }
                TelegramQueueEnqueueOutcome::NotRunning => {
                    adapter
                        .send_text(&message.chat_id, text.telegram_queue_requires_running())
                        .await?;
                }
            }
            return Ok(());
        }
        Some("/granularity") | Some("/回复") => {
            handle_telegram_reply_granularity_command(
                &state,
                &adapter,
                &message,
                command_payload(trimmed),
            )
            .await?;
            return Ok(());
        }
        Some("/stop") | Some("/s") => {
            let Some((thread_id, turn_id)) = active_turn_for_message(&state, &message).await else {
                let notice = if turn_in_progress_for_message(&state, &message).await {
                    text.telegram_turn_starting_notice()
                } else {
                    text.no_running_turn()
                };
                adapter.send_text(&message.chat_id, notice).await?;
                return Ok(());
            };
            let remote_client_key = remote_client_key_for_thread(&state, &thread_id)
                .await
                .context("bound IM thread is missing remote client key")?;
            let claimed_terminal_notice = state
                .runtime
                .lock()
                .await
                .claim_terminal_notice(&turn_id, true);
            if let Err(err) = remote_control_backend::interrupt_turn_for_client(
                &state,
                &remote_client_key,
                &thread_id,
                &turn_id,
            )
            .await
            {
                if claimed_terminal_notice {
                    state.runtime.lock().await.release_terminal_notice(&turn_id);
                }
                return Err(err);
            }
            remote_control_backend::clear_turn_for_client(
                &state,
                &remote_client_key,
                Some(&turn_id),
            )
            .await;
            telegram_typing::finish_thread(&state, api.clone(), &thread_id, &route).await;
            events::finish_telegram_command_progress_with_api(
                &state,
                api.clone(),
                &thread_id,
                &route,
                &turn_id,
            )
            .await;
            state
                .runtime
                .lock()
                .await
                .mark_turn_completed(&thread_id, Some(&turn_id));
            adapter
                .send_text(&message.chat_id, text.interrupted())
                .await?;
            start_next_telegram_queued_turn(&state, api.clone(), &route).await;
            return Ok(());
        }
        Some("/exit") | Some("/q") => {
            if active_turn_for_message(&state, &message).await.is_none()
                && turn_in_progress_for_message(&state, &message).await
            {
                adapter
                    .send_text(&message.chat_id, text.telegram_turn_starting_notice())
                    .await?;
                return Ok(());
            }
            if let Some((thread_id, turn_id)) = active_turn_for_message(&state, &message).await {
                state
                    .runtime
                    .lock()
                    .await
                    .claim_terminal_notice(&turn_id, true);
                let remote_client_key = remote_client_key_for_thread(&state, &thread_id)
                    .await
                    .context("bound IM thread is missing remote client key")?;
                let _ = remote_control_backend::interrupt_turn_for_client(
                    &state,
                    &remote_client_key,
                    &thread_id,
                    &turn_id,
                )
                .await;
                remote_control_backend::clear_thread_for_client(
                    &state,
                    &remote_client_key,
                    Some(&thread_id),
                )
                .await;
                telegram_typing::finish_thread(&state, api.clone(), &thread_id, &route).await;
                events::finish_telegram_command_progress_with_api(
                    &state,
                    api.clone(),
                    &thread_id,
                    &route,
                    &turn_id,
                )
                .await;
                state
                    .runtime
                    .lock()
                    .await
                    .mark_turn_completed(&thread_id, Some(&turn_id));
            }
            state
                .runtime
                .lock()
                .await
                .clear_telegram_queue(&route.conversation_key);
            clear_thread_binding(&state, &route.conversation_key).await?;
            adapter.send_text(&message.chat_id, text.exited()).await?;
            return Ok(());
        }
        Some(other) => {
            adapter
                .send_text(&message.chat_id, &text.telegram_unknown_command(other))
                .await?;
            return Ok(());
        }
        None => {}
    }

    if active_turn_for_message(&state, &message).await.is_some() {
        if trimmed.is_empty() && !message.attachments.is_empty() {
            let attachment_count = state.runtime.lock().await.hold_pending_attachments(
                &route.conversation_key,
                message.attachments.clone(),
                message.received_at_ms,
            );
            adapter
                .send_text(
                    &message.chat_id,
                    &text.turn_busy_attachments_held(attachment_count),
                )
                .await?;
            return Ok(());
        }
        if !trimmed.is_empty() {
            steer_telegram_turn(&state, &adapter, &message, trimmed, &message.attachments).await?;
        } else {
            adapter
                .send_text(&message.chat_id, text.telegram_turn_busy_notice())
                .await?;
        }
        return Ok(());
    }
    if turn_in_progress_for_message(&state, &message).await {
        adapter
            .send_text(&message.chat_id, text.telegram_turn_starting_notice())
            .await?;
        return Ok(());
    }

    let remote_status = remote_control_backend::status_snapshot(&state).await;
    if !remote_status.connected {
        adapter
            .send_text(&message.chat_id, text.remote_not_connected())
            .await?;
        return Ok(());
    }

    let only_images = trimmed.is_empty()
        && !message.attachments.is_empty()
        && message
            .attachments
            .iter()
            .all(|attachment| attachment.kind == "image");
    if only_images {
        let attachment_count = state.runtime.lock().await.hold_pending_attachments(
            &route.conversation_key,
            message.attachments.clone(),
            message.received_at_ms,
        );
        adapter
            .send_text(&message.chat_id, text.image_description_needed())
            .await?;
        state
            .push_event(
                "info",
                "telegram_image_waiting_for_description",
                format!(
                    "chat={} pending_attachments={attachment_count}",
                    message.chat_id
                ),
            )
            .await;
        return Ok(());
    }

    let pending_attachments = if trimmed.is_empty() {
        Vec::new()
    } else {
        state
            .runtime
            .lock()
            .await
            .take_pending_attachments(&route.conversation_key, message.received_at_ms)
    };
    let mut attachments = message.attachments.clone();
    attachments.extend(pending_attachments.clone());

    let mut outcome = start_turn_for_route(
        &state,
        &route,
        trimmed,
        &attachments,
        message.received_at_ms,
        TurnOrigin::Telegram,
    )
    .await;
    if matches!(&outcome, TurnStartOutcome::NoThread)
        && message.chat_type == crate::types::ChatType::Group
    {
        let project_cwd = {
            let (raw_chat_id, _) = split_telegram_message_target(&message.chat_id);
            state
                .config
                .lock()
                .await
                .telegram_account(&message.account_id)
                .and_then(|config| config.project_group_for_chat(raw_chat_id))
                .map(|group| group.cwd)
        };
        if let Some(cwd) = project_cwd {
            let options = thread_start_options_with_current_provider(
                remote_control_backend::ThreadStartOptions {
                    cwd: Some(cwd),
                    ..Default::default()
                },
            );
            match create_and_bind_thread(&state, &route, options, None).await {
                Ok(thread_id) => {
                    state
                        .push_event(
                            "info",
                            "telegram_topic_thread_created",
                            format!(
                                "conversation={} thread={} project_group=true",
                                route.conversation_key, thread_id
                            ),
                        )
                        .await;
                    outcome = start_turn_for_route(
                        &state,
                        &route,
                        trimmed,
                        &attachments,
                        message.received_at_ms,
                        TurnOrigin::Telegram,
                    )
                    .await;
                }
                Err(error) => outcome = TurnStartOutcome::Failed { error },
            }
        }
    }
    let attachments_to_restore = if matches!(&outcome, TurnStartOutcome::Started { .. }) {
        Vec::new()
    } else if matches!(&outcome, TurnStartOutcome::Expired { .. }) {
        pending_attachments
    } else {
        attachments
    };
    if !attachments_to_restore.is_empty() {
        state.runtime.lock().await.hold_pending_attachments(
            &route.conversation_key,
            attachments_to_restore,
            message.received_at_ms,
        );
    }

    match outcome {
        TurnStartOutcome::Started { thread_id, turn_id } => {
            telegram_typing::start_turn(&state, api.clone(), &thread_id, &turn_id, &route).await;
            state
                .push_event(
                    "info",
                    "telegram_turn_started",
                    format!(
                        "chat={} thread={} turn={turn_id}",
                        message.chat_id, thread_id
                    ),
                )
                .await;
            Ok(())
        }
        TurnStartOutcome::Busy => {
            adapter
                .send_text(&message.chat_id, text.turn_busy_notice())
                .await?;
            Ok(())
        }
        TurnStartOutcome::Expired { thread_id } => {
            adapter
                .send_text(&message.chat_id, text.inbound_expired())
                .await?;
            state
                .push_event(
                    "warn",
                    "telegram_inbound_expired",
                    format!(
                        "chat={} thread={thread_id} message={}",
                        message.chat_id, message.message_id
                    ),
                )
                .await;
            Ok(())
        }
        TurnStartOutcome::NoThread => {
            send_telegram_thread_routing_choice(&state, &adapter, &message, None).await?;
            Ok(())
        }
        TurnStartOutcome::Stale { thread_id } => {
            state
                .push_event(
                    "warn",
                    "telegram_thread_route_stale",
                    format!(
                        "conversation={} thread={} during=turn/start",
                        route.conversation_key, thread_id
                    ),
                )
                .await;
            adapter
                .send_text(&message.chat_id, text.stale_thread_unbound())
                .await?;
            send_telegram_thread_routing_choice(&state, &adapter, &message, None).await
        }
        TurnStartOutcome::Failed { error } => {
            adapter
                .send_text(&message.chat_id, &text.app_message_failed(&error))
                .await?;
            Err(error)
        }
    }
}

async fn create_telegram_thread_for_route(
    state: &SharedState,
    adapter: &TelegramAdapter,
    message: &InboundMessage,
    options: remote_control_backend::ThreadStartOptions,
    request: ThreadRoutingRequestState,
) -> Result<String> {
    let text = im_text_for_state(state);
    let route = route_for_message(message);
    let progress_message_id = adapter
        .send_or_update_text(
            &route.chat_id,
            request.message_id.as_deref(),
            text.creating_new_thread(),
        )
        .await?;
    let mut retry_request = request;
    retry_request.message_id = Some(progress_message_id.clone());
    let thread_id = match create_and_bind_thread(
        state,
        &route,
        options.clone(),
        Some(&retry_request.request_id),
    )
    .await
    {
        Ok(thread_id) => thread_id,
        Err(error) => {
            if retry_request.stage == ThreadRoutingStage::Choice {
                send_telegram_thread_routing_choice(state, adapter, message, Some(retry_request))
                    .await?;
            } else {
                send_telegram_thread_create_settings(state, adapter, message, Some(retry_request))
                    .await?;
            }
            adapter
                .send_text(&route.chat_id, &text.app_message_failed(&error))
                .await?;
            return Err(error);
        }
    };
    publish_telegram_thread_routing_result(
        state,
        adapter,
        &route.chat_id,
        &route.conversation_key,
        &thread_id,
        text.created_new_session_title(),
        &text.created_new_session_body(&thread_id, &summarize_thread_start_options(&options, text)),
        &progress_message_id,
        "telegram_thread_route_created",
    )
    .await;
    state
        .push_event(
            "info",
            "telegram_thread_route_created",
            format!("conversation={} thread={thread_id}", route.conversation_key),
        )
        .await;
    Ok(thread_id)
}

pub(crate) async fn handle_inbound_action(
    state: SharedState,
    outbound_tx: ImOutboundSender,
    adapter: TelegramAdapter,
    message: InboundMessage,
    action: InboundAction,
) -> Result<()> {
    match action {
        InboundAction::ThreadRouteOpen => Ok(()),
        InboundAction::ApprovalDecision {
            request_fingerprint,
            option_index,
        } => {
            handle_telegram_approval_button_reply(
                &state,
                &outbound_tx,
                &adapter,
                &message,
                &request_fingerprint,
                option_index,
            )
            .await?;
            Ok(())
        }
        InboundAction::ThreadRouteChoice { request_id, action } => {
            handle_telegram_thread_route_choice(state, adapter, message, &request_id, &action).await
        }
        InboundAction::ThreadRouteCreateSubmit {
            request_id,
            cwd_choice,
            cwd_custom,
            model,
            effort,
            permission,
        } => {
            let Some(request) =
                checked_telegram_thread_routing_request(&state, &adapter, &message, &request_id)
                    .await?
            else {
                return Ok(());
            };
            let route = route_for_message(&message);
            let remote_client_key = route.remote_client_key.clone();
            let options = match thread_start_options_from_form_for_client(
                &state,
                &remote_client_key,
                ThreadCreateForm {
                    cwd_choice,
                    cwd_custom,
                    model,
                    effort,
                    permission,
                },
            )
            .await
            {
                Ok(options) => options,
                Err(err) => {
                    let text = im_text_for_state(&state);
                    adapter
                        .send_text(&message.chat_id, &text.invalid_create_form(&err))
                        .await?;
                    return Ok(());
                }
            };
            create_telegram_thread_for_route(&state, &adapter, &message, options, request).await?;
            Ok(())
        }
        InboundAction::ThreadRouteCreateDefault { request_id } => {
            let Some(request) =
                checked_telegram_thread_routing_request(&state, &adapter, &message, &request_id)
                    .await?
            else {
                return Ok(());
            };
            let options = thread_start_options_with_current_provider(
                remote_control_backend::ThreadStartOptions::default(),
            );
            create_telegram_thread_for_route(&state, &adapter, &message, options, request).await?;
            Ok(())
        }
        InboundAction::ThreadRouteCreateConfigured { request_id } => {
            let Some(request) =
                checked_telegram_thread_routing_request(&state, &adapter, &message, &request_id)
                    .await?
            else {
                return Ok(());
            };
            let route = route_for_message(&message);
            let remote_client_key = route.remote_client_key.clone();
            let options = match thread_start_options_from_form_for_client(
                &state,
                &remote_client_key,
                thread_create_form_from_draft(&request.create_draft),
            )
            .await
            {
                Ok(options) => options,
                Err(err) => {
                    let text = im_text_for_state(&state);
                    adapter
                        .send_text(&message.chat_id, &text.invalid_create_form(&err))
                        .await?;
                    return Ok(());
                }
            };
            create_telegram_thread_for_route(&state, &adapter, &message, options, request).await?;
            Ok(())
        }
        InboundAction::ThreadRouteCreateEdit { request_id, field } => {
            let Some(request) =
                checked_telegram_thread_routing_request(&state, &adapter, &message, &request_id)
                    .await?
            else {
                return Ok(());
            };
            send_telegram_thread_create_options(&state, &adapter, &message, request, &field, 1)
                .await
        }
        InboundAction::ThreadRouteCreateSetIndex {
            request_id,
            field,
            page,
            index,
        } => {
            let Some(mut request) =
                checked_telegram_thread_routing_request(&state, &adapter, &message, &request_id)
                    .await?
            else {
                return Ok(());
            };
            let Some(field) = normalize_thread_create_field(&field) else {
                let text = im_text_for_state(&state);
                adapter
                    .send_text(&message.chat_id, text.create_option_unavailable())
                    .await?;
                return Ok(());
            };
            let Some(value) = request
                .create_option_values_by_field_page
                .get(field)
                .and_then(|pages| pages.get(page.saturating_sub(1)))
                .and_then(|values| values.get(index))
                .cloned()
            else {
                let text = im_text_for_state(&state);
                adapter
                    .send_text(&message.chat_id, text.create_option_expired())
                    .await?;
                return Ok(());
            };
            apply_thread_create_draft_value(&mut request.create_draft, field, &value)?;
            state
                .runtime
                .lock()
                .await
                .remember_thread_routing_request(request.clone());
            if field == "cwd" && value == "__custom__" {
                send_telegram_thread_create_custom_cwd_prompt(&state, &adapter, &message).await?;
                return Ok(());
            }
            send_telegram_thread_create_settings(&state, &adapter, &message, Some(request)).await
        }
        InboundAction::ThreadRouteCreateSetValue {
            request_id,
            field,
            value,
        } => {
            let Some(mut request) =
                checked_telegram_thread_routing_request(&state, &adapter, &message, &request_id)
                    .await?
            else {
                return Ok(());
            };
            let Some(field) = normalize_thread_create_field(&field) else {
                let text = im_text_for_state(&state);
                adapter
                    .send_text(&message.chat_id, text.create_option_unavailable())
                    .await?;
                return Ok(());
            };
            apply_thread_create_draft_value(&mut request.create_draft, field, &value)?;
            state
                .runtime
                .lock()
                .await
                .remember_thread_routing_request(request.clone());
            if field == "cwd" && value == "__custom__" {
                send_telegram_thread_create_custom_cwd_prompt(&state, &adapter, &message).await?;
                return Ok(());
            }
            send_telegram_thread_create_settings(&state, &adapter, &message, Some(request)).await
        }
        InboundAction::ThreadRouteCreateOptionsPage {
            request_id,
            field,
            direction,
        } => {
            let Some(request) =
                checked_telegram_thread_routing_request(&state, &adapter, &message, &request_id)
                    .await?
            else {
                return Ok(());
            };
            let current_page = request.page.max(1);
            let target_page = match direction {
                ThreadRouteDirection::Prev => current_page.saturating_sub(1).max(1),
                ThreadRouteDirection::Next => current_page.saturating_add(1),
            };
            send_telegram_thread_create_options(
                &state,
                &adapter,
                &message,
                request,
                &field,
                target_page,
            )
            .await
        }
        InboundAction::ThreadRouteResumeSelected {
            request_id,
            thread_id,
        } => {
            handle_telegram_thread_route_resume_selected(
                state,
                adapter,
                message,
                &request_id,
                &thread_id,
            )
            .await
        }
        InboundAction::ThreadRouteResumeIndex {
            request_id,
            page,
            index,
        } => {
            let Some(request) =
                checked_telegram_thread_routing_request(&state, &adapter, &message, &request_id)
                    .await?
            else {
                return Ok(());
            };
            let Some(thread_id) = request
                .thread_ids_by_page
                .get(page.saturating_sub(1))
                .and_then(|thread_ids| thread_ids.get(index))
                .cloned()
            else {
                let text = im_text_for_state(&state);
                adapter
                    .send_text(&message.chat_id, text.thread_selection_expired())
                    .await?;
                return Ok(());
            };
            handle_telegram_thread_route_resume_selected(
                state,
                adapter,
                message,
                &request_id,
                &thread_id,
            )
            .await
        }
        InboundAction::ThreadRouteListPage {
            request_id,
            direction,
        } => {
            handle_telegram_thread_route_list_page(state, adapter, message, &request_id, direction)
                .await
        }
        InboundAction::ThreadSettingsOpenField {
            request_id,
            revision,
            field,
        } => {
            handle_telegram_thread_settings_open_field(
                state,
                adapter,
                message,
                &request_id,
                revision,
                field,
            )
            .await
        }
        InboundAction::ThreadSettingsModelPage {
            request_id,
            revision,
            direction,
        } => {
            handle_telegram_thread_settings_model_page(
                state,
                adapter,
                message,
                &request_id,
                revision,
                direction,
            )
            .await
        }
        InboundAction::ThreadSettingsChooseModel {
            request_id,
            revision,
            page,
            index,
        } => {
            handle_telegram_thread_settings_choose_model(
                state,
                adapter,
                message,
                &request_id,
                revision,
                page,
                index,
            )
            .await
        }
        InboundAction::ThreadSettingsChooseEffort {
            request_id,
            revision,
            index,
        } => {
            handle_telegram_thread_settings_choose_effort(
                state,
                adapter,
                message,
                &request_id,
                revision,
                index,
            )
            .await
        }
        InboundAction::ThreadSettingsChooseSpeed {
            request_id,
            revision,
            fast,
        } => {
            handle_telegram_thread_settings_choose_speed(
                state,
                adapter,
                message,
                &request_id,
                revision,
                fast,
            )
            .await
        }
        InboundAction::ThreadSettingsBack {
            request_id,
            revision,
        } => {
            handle_telegram_thread_settings_back(state, adapter, message, &request_id, revision)
                .await
        }
        InboundAction::ThreadSettingsApply {
            request_id,
            revision,
        } => {
            handle_telegram_thread_settings_apply(state, adapter, message, &request_id, revision)
                .await
        }
        InboundAction::ThreadSettingsCompatibilityConfirm {
            request_id,
            revision,
            accept,
        } => {
            handle_telegram_thread_settings_compatibility_confirmation(
                state,
                adapter,
                message,
                &request_id,
                revision,
                accept,
            )
            .await
        }
        InboundAction::ThreadSettingsCancel {
            request_id,
            revision,
        } => {
            handle_telegram_thread_settings_cancel(state, adapter, message, &request_id, revision)
                .await
        }
    }
}

async fn handle_telegram_thread_route_choice(
    state: SharedState,
    adapter: TelegramAdapter,
    message: InboundMessage,
    request_id: &str,
    action: &str,
) -> Result<()> {
    let request =
        checked_telegram_thread_routing_request(&state, &adapter, &message, request_id).await?;
    let Some(request) = request else {
        return Ok(());
    };

    match action {
        "create_new" => {
            send_telegram_thread_create_settings(&state, &adapter, &message, Some(request)).await
        }
        "resume_history" => {
            send_telegram_thread_routing_list(&state, &adapter, &message, Some(request), None, 1)
                .await
        }
        "back" => {
            send_telegram_thread_routing_choice(&state, &adapter, &message, Some(request)).await
        }
        other => {
            let text = im_text_for_state(&state);
            adapter
                .send_text(&message.chat_id, &text.unsupported_thread_action(other))
                .await?;
            Ok(())
        }
    }
}

async fn checked_telegram_thread_routing_request(
    state: &SharedState,
    adapter: &TelegramAdapter,
    message: &InboundMessage,
    request_id: &str,
) -> Result<Option<ThreadRoutingRequestState>> {
    let request = {
        state
            .runtime
            .lock()
            .await
            .thread_routing_request(request_id)
    };
    let Some(request) = request else {
        let text = im_text_for_state(state);
        adapter
            .send_text(&message.chat_id, text.thread_operation_expired())
            .await?;
        return Ok(None);
    };
    if request.conversation_key != message.conversation_key() {
        let text = im_text_for_state(state);
        adapter
            .send_text(&message.chat_id, text.thread_choice_not_current())
            .await?;
        return Ok(None);
    }
    if !callback_targets_current_message(
        request.message_id.as_deref(),
        message.card_message_id.as_deref(),
    ) {
        if let Some(callback_message_id) = message.card_message_id.as_deref() {
            let _ = adapter
                .clear_reply_markup(&message.chat_id, Some(callback_message_id))
                .await;
        }
        let text = im_text_for_state(state);
        adapter
            .send_text(&message.chat_id, text.thread_choice_not_current())
            .await?;
        return Ok(None);
    }
    Ok(Some(request))
}

fn callback_targets_current_message(
    expected_message_id: Option<&str>,
    callback_message_id: Option<&str>,
) -> bool {
    !matches!(
        (expected_message_id, callback_message_id),
        (Some(expected), Some(actual)) if expected != actual
    )
}

async fn send_telegram_thread_settings(
    state: &SharedState,
    adapter: &TelegramAdapter,
    message: &InboundMessage,
) -> Result<()> {
    let text = im_text_for_state(state);
    let route = route_for_message(message);
    let Some(thread_id) = live_thread_for_route(state, &route).await else {
        adapter
            .send_text(
                &message.chat_id,
                text.telegram_model_switch_requires_thread(),
            )
            .await?;
        return Ok(());
    };
    let Some(bound_route) = state.runtime.lock().await.route_for_thread(&thread_id) else {
        adapter
            .send_text(
                &message.chat_id,
                text.telegram_model_switch_requires_thread(),
            )
            .await?;
        return Ok(());
    };
    if bound_route.conversation_key != route.conversation_key {
        adapter
            .send_text(&message.chat_id, text.telegram_model_switch_not_current())
            .await?;
        return Ok(());
    }
    if !remote_control_backend::status_snapshot(state)
        .await
        .connected
    {
        adapter
            .send_text(&message.chat_id, text.remote_not_connected())
            .await?;
        return Ok(());
    }

    let remote_client_key = bound_route.remote_client_key.clone();
    let catalog =
        match load_thread_model_settings_choices_for_client(state, &remote_client_key).await {
            Ok(catalog) => catalog,
            Err(error) => {
                state
                    .push_event(
                        "warn",
                        "telegram_thread_settings_catalog_failed",
                        format!(
                            "conversation={} thread={} client={} err={error}",
                            route.conversation_key, thread_id, remote_client_key
                        ),
                    )
                    .await;
                adapter
                    .send_text(&message.chat_id, &text.telegram_model_list_failed(&error))
                    .await?;
                return Ok(());
            }
        };
    if catalog.is_empty() {
        adapter
            .send_text(&message.chat_id, text.telegram_model_list_empty())
            .await?;
        return Ok(());
    }
    let catalog = catalog
        .into_iter()
        .map(|choice| TelegramThreadSettingsModelChoice {
            supports_fast: choice
                .service_tiers
                .iter()
                .any(|tier| tier.id.eq_ignore_ascii_case("priority")),
            model: choice.model,
            label: choice.label,
            supported_efforts: choice.supported_efforts,
            default_effort: choice.default_effort,
        })
        .collect::<Vec<_>>();

    let (previous_request, observed) = {
        let mut runtime = state.runtime.lock().await;
        let previous_request = runtime
            .current_telegram_model_switch_request(&route.conversation_key)
            .and_then(|request| runtime.clear_telegram_model_switch_request(&request.request_id));
        let observed = runtime.thread_settings_snapshot(&thread_id);
        (previous_request, observed)
    };
    if let Some(previous_request) = previous_request
        && let Some(message_id) = previous_request.message_id.as_deref()
    {
        let _ = adapter
            .clear_reply_markup(&route.chat_id, Some(message_id))
            .await;
    }

    let request = TelegramModelSwitchRequestState {
        request_id: next_telegram_model_switch_request_id(),
        conversation_key: route.conversation_key.clone(),
        account_id: route.account_id.clone(),
        chat_id: route.chat_id.clone(),
        expected_thread_id: thread_id,
        remote_client_key,
        catalog,
        observed,
        draft: TelegramThreadSettingsDraft::default(),
        revision: 1,
        expires_at_ms: now_ms()
            .saturating_add(crate::im_runtime::TELEGRAM_THREAD_SETTINGS_DRAFT_MAX_AGE_MS),
        stage: TelegramThreadSettingsStage::Overview,
        model_page: 1,
        compatibility: None,
        pending_apply: None,
        stale: false,
        message_id: None,
    };
    state
        .runtime
        .lock()
        .await
        .remember_telegram_model_switch_request(request.clone());
    render_telegram_thread_settings(state, adapter, &route.chat_id, request).await
}

async fn checked_telegram_thread_settings_request(
    state: &SharedState,
    adapter: &TelegramAdapter,
    message: &InboundMessage,
    request_id: &str,
    revision: u64,
) -> Result<Option<TelegramModelSwitchRequestState>> {
    let text = im_text_for_state(state);
    let conversation_key = message.conversation_key();
    let request = {
        let mut runtime = state.runtime.lock().await;
        match runtime.telegram_model_switch_request(request_id) {
            Some(request) if request.expires_at_ms <= now_ms() => {
                runtime.clear_telegram_model_switch_request(request_id);
                None
            }
            Some(request)
                if runtime
                    .is_current_telegram_model_switch_request(request_id, &conversation_key)
                    && request.conversation_key == conversation_key
                    && request.revision == revision =>
            {
                Some(request)
            }
            _ => None,
        }
    };
    let Some(request) = request else {
        adapter
            .send_text(&message.chat_id, text.telegram_model_switch_expired())
            .await?;
        return Ok(None);
    };
    if !callback_targets_current_message(
        request.message_id.as_deref(),
        message.card_message_id.as_deref(),
    ) {
        if let Some(callback_message_id) = message.card_message_id.as_deref() {
            let _ = adapter
                .clear_reply_markup(&message.chat_id, Some(callback_message_id))
                .await;
        }
        adapter
            .send_text(&message.chat_id, text.telegram_model_switch_not_current())
            .await?;
        return Ok(None);
    }
    if request.stale {
        state
            .runtime
            .lock()
            .await
            .clear_telegram_model_switch_request(request_id);
        let _ = adapter
            .clear_reply_markup(&message.chat_id, request.message_id.as_deref())
            .await;
        adapter
            .send_text(&message.chat_id, text.telegram_thread_settings_stale())
            .await?;
        return Ok(None);
    }
    let route = route_for_message(message);
    let Some(thread_id) = live_thread_for_route(state, &route).await else {
        adapter
            .send_text(&message.chat_id, text.telegram_model_switch_not_current())
            .await?;
        return Ok(None);
    };
    let Some(bound_route) = state.runtime.lock().await.route_for_thread(&thread_id) else {
        adapter
            .send_text(&message.chat_id, text.telegram_model_switch_not_current())
            .await?;
        return Ok(None);
    };
    if request.expected_thread_id != thread_id
        || request.remote_client_key != bound_route.remote_client_key
        || bound_route.conversation_key != conversation_key
    {
        adapter
            .send_text(&message.chat_id, text.telegram_model_switch_not_current())
            .await?;
        return Ok(None);
    }
    Ok(Some(request))
}

async fn render_telegram_thread_settings(
    state: &SharedState,
    adapter: &TelegramAdapter,
    target: &str,
    request: TelegramModelSwitchRequestState,
) -> Result<()> {
    let text = im_text_for_state(state);
    let request_id = request.request_id.clone();
    let message_id = adapter
        .send_thread_settings_editor(target, &request, text)
        .await?;
    state
        .runtime
        .lock()
        .await
        .update_telegram_model_switch_request_message_id(&request_id, message_id);
    Ok(())
}

async fn save_telegram_thread_settings_request(
    state: &SharedState,
    request: TelegramModelSwitchRequestState,
) -> bool {
    state
        .runtime
        .lock()
        .await
        .update_telegram_model_switch_request(request)
}

async fn handle_telegram_thread_settings_open_field(
    state: SharedState,
    adapter: TelegramAdapter,
    message: InboundMessage,
    request_id: &str,
    revision: u64,
    field: ThreadSettingsField,
) -> Result<()> {
    let Some(mut request) =
        checked_telegram_thread_settings_request(&state, &adapter, &message, request_id, revision)
            .await?
    else {
        return Ok(());
    };
    request.compatibility = None;
    request.stage = match field {
        ThreadSettingsField::Model => {
            request.model_page = 1;
            TelegramThreadSettingsStage::Model
        }
        ThreadSettingsField::Effort => TelegramThreadSettingsStage::Effort,
        ThreadSettingsField::Speed => TelegramThreadSettingsStage::Speed,
    };
    request.revision = request.revision.saturating_add(1);
    if save_telegram_thread_settings_request(&state, request.clone()).await {
        render_telegram_thread_settings(&state, &adapter, &message.chat_id, request).await?;
    }
    Ok(())
}

async fn handle_telegram_thread_settings_model_page(
    state: SharedState,
    adapter: TelegramAdapter,
    message: InboundMessage,
    request_id: &str,
    revision: u64,
    direction: ThreadRouteDirection,
) -> Result<()> {
    let Some(mut request) =
        checked_telegram_thread_settings_request(&state, &adapter, &message, request_id, revision)
            .await?
    else {
        return Ok(());
    };
    if request.stage != TelegramThreadSettingsStage::Model {
        adapter
            .send_text(
                &message.chat_id,
                im_text_for_state(&state).telegram_model_switch_expired(),
            )
            .await?;
        return Ok(());
    }
    let total_pages = request
        .catalog
        .len()
        .div_ceil(TELEGRAM_MODEL_PAGE_SIZE)
        .max(1);
    request.model_page = match direction {
        ThreadRouteDirection::Prev => request.model_page.saturating_sub(1).max(1),
        ThreadRouteDirection::Next => request.model_page.saturating_add(1).min(total_pages),
    };
    request.revision = request.revision.saturating_add(1);
    if save_telegram_thread_settings_request(&state, request.clone()).await {
        render_telegram_thread_settings(&state, &adapter, &message.chat_id, request).await?;
    }
    Ok(())
}

async fn handle_telegram_thread_settings_choose_model(
    state: SharedState,
    adapter: TelegramAdapter,
    message: InboundMessage,
    request_id: &str,
    revision: u64,
    page: usize,
    index: usize,
) -> Result<()> {
    let Some(mut request) =
        checked_telegram_thread_settings_request(&state, &adapter, &message, request_id, revision)
            .await?
    else {
        return Ok(());
    };
    let model = (request.stage == TelegramThreadSettingsStage::Model && request.model_page == page)
        .then(|| {
            request
                .catalog
                .get((page.saturating_sub(1)) * TELEGRAM_MODEL_PAGE_SIZE + index)
                .map(|choice| choice.model.clone())
        })
        .flatten();
    let Some(model) = model else {
        adapter
            .send_text(
                &message.chat_id,
                im_text_for_state(&state).telegram_model_switch_expired(),
            )
            .await?;
        return Ok(());
    };
    request.draft.model = Some(model);
    request.compatibility = None;
    request.stage = TelegramThreadSettingsStage::Overview;
    request.revision = request.revision.saturating_add(1);
    if save_telegram_thread_settings_request(&state, request.clone()).await {
        render_telegram_thread_settings(&state, &adapter, &message.chat_id, request).await?;
    }
    Ok(())
}

async fn handle_telegram_thread_settings_choose_effort(
    state: SharedState,
    adapter: TelegramAdapter,
    message: InboundMessage,
    request_id: &str,
    revision: u64,
    index: usize,
) -> Result<()> {
    let Some(mut request) =
        checked_telegram_thread_settings_request(&state, &adapter, &message, request_id, revision)
            .await?
    else {
        return Ok(());
    };
    let effort = (request.stage == TelegramThreadSettingsStage::Effort)
        .then(|| {
            thread_settings_selected_model(&request)
                .and_then(|choice| choice.supported_efforts.get(index))
                .cloned()
        })
        .flatten();
    let Some(effort) = effort else {
        adapter
            .send_text(
                &message.chat_id,
                im_text_for_state(&state).telegram_model_switch_expired(),
            )
            .await?;
        return Ok(());
    };
    request.draft.effort = Some(effort);
    request.compatibility = None;
    request.stage = TelegramThreadSettingsStage::Overview;
    request.revision = request.revision.saturating_add(1);
    if save_telegram_thread_settings_request(&state, request.clone()).await {
        render_telegram_thread_settings(&state, &adapter, &message.chat_id, request).await?;
    }
    Ok(())
}

async fn handle_telegram_thread_settings_choose_speed(
    state: SharedState,
    adapter: TelegramAdapter,
    message: InboundMessage,
    request_id: &str,
    revision: u64,
    fast: bool,
) -> Result<()> {
    let Some(mut request) =
        checked_telegram_thread_settings_request(&state, &adapter, &message, request_id, revision)
            .await?
    else {
        return Ok(());
    };
    if request.stage != TelegramThreadSettingsStage::Speed
        || (fast
            && !thread_settings_selected_model(&request).is_some_and(|choice| choice.supports_fast))
    {
        adapter
            .send_text(
                &message.chat_id,
                im_text_for_state(&state).telegram_model_switch_expired(),
            )
            .await?;
        return Ok(());
    }
    request.draft.speed = Some(if fast {
        TelegramThreadSettingsSpeed::Fast
    } else {
        TelegramThreadSettingsSpeed::Standard
    });
    request.compatibility = None;
    request.stage = TelegramThreadSettingsStage::Overview;
    request.revision = request.revision.saturating_add(1);
    if save_telegram_thread_settings_request(&state, request.clone()).await {
        render_telegram_thread_settings(&state, &adapter, &message.chat_id, request).await?;
    }
    Ok(())
}

async fn handle_telegram_thread_settings_back(
    state: SharedState,
    adapter: TelegramAdapter,
    message: InboundMessage,
    request_id: &str,
    revision: u64,
) -> Result<()> {
    let Some(mut request) =
        checked_telegram_thread_settings_request(&state, &adapter, &message, request_id, revision)
            .await?
    else {
        return Ok(());
    };
    request.compatibility = None;
    request.stage = TelegramThreadSettingsStage::Overview;
    request.revision = request.revision.saturating_add(1);
    if save_telegram_thread_settings_request(&state, request.clone()).await {
        render_telegram_thread_settings(&state, &adapter, &message.chat_id, request).await?;
    }
    Ok(())
}

async fn handle_telegram_thread_settings_cancel(
    state: SharedState,
    adapter: TelegramAdapter,
    message: InboundMessage,
    request_id: &str,
    revision: u64,
) -> Result<()> {
    let Some(request) =
        checked_telegram_thread_settings_request(&state, &adapter, &message, request_id, revision)
            .await?
    else {
        return Ok(());
    };
    state
        .runtime
        .lock()
        .await
        .clear_telegram_model_switch_request(request_id);
    let body = im_text_for_state(&state).telegram_thread_settings_cancelled();
    adapter
        .send_or_update_text(&message.chat_id, request.message_id.as_deref(), body)
        .await?;
    let _ = adapter
        .clear_reply_markup(&message.chat_id, request.message_id.as_deref())
        .await;
    Ok(())
}

async fn handle_telegram_thread_settings_apply(
    state: SharedState,
    adapter: TelegramAdapter,
    message: InboundMessage,
    request_id: &str,
    revision: u64,
) -> Result<()> {
    let Some(mut request) =
        checked_telegram_thread_settings_request(&state, &adapter, &message, request_id, revision)
            .await?
    else {
        return Ok(());
    };
    if request.stage != TelegramThreadSettingsStage::Overview || request.pending_apply.is_some() {
        return Ok(());
    }
    let compatibility = thread_settings_compatibility(&request);
    if compatibility.reset_effort || compatibility.reset_speed {
        request.compatibility = Some(compatibility);
        request.stage = TelegramThreadSettingsStage::CompatibilityConfirmation;
        request.revision = request.revision.saturating_add(1);
        if save_telegram_thread_settings_request(&state, request.clone()).await {
            render_telegram_thread_settings(&state, &adapter, &message.chat_id, request).await?;
        }
        return Ok(());
    }
    submit_telegram_thread_settings(state, adapter, message, request, None).await
}

async fn handle_telegram_thread_settings_compatibility_confirmation(
    state: SharedState,
    adapter: TelegramAdapter,
    message: InboundMessage,
    request_id: &str,
    revision: u64,
    accept: bool,
) -> Result<()> {
    let Some(mut request) =
        checked_telegram_thread_settings_request(&state, &adapter, &message, request_id, revision)
            .await?
    else {
        return Ok(());
    };
    if request.stage != TelegramThreadSettingsStage::CompatibilityConfirmation {
        return Ok(());
    }
    let compatibility =
        request
            .compatibility
            .clone()
            .unwrap_or(TelegramThreadSettingsCompatibility {
                reset_effort: false,
                reset_speed: false,
            });
    if !accept {
        request.compatibility = None;
        request.stage = TelegramThreadSettingsStage::Overview;
        request.revision = request.revision.saturating_add(1);
        if save_telegram_thread_settings_request(&state, request.clone()).await {
            render_telegram_thread_settings(&state, &adapter, &message.chat_id, request).await?;
        }
        return Ok(());
    }
    submit_telegram_thread_settings(state, adapter, message, request, Some(compatibility)).await
}

async fn submit_telegram_thread_settings(
    state: SharedState,
    adapter: TelegramAdapter,
    message: InboundMessage,
    request: TelegramModelSwitchRequestState,
    compatibility: Option<TelegramThreadSettingsCompatibility>,
) -> Result<()> {
    let local_patch = thread_settings_patch(&request, compatibility.as_ref());
    if local_patch.is_empty() {
        state
            .runtime
            .lock()
            .await
            .clear_telegram_model_switch_request(&request.request_id);
        let body = im_text_for_state(&state).telegram_thread_settings_no_changes();
        adapter
            .send_or_update_text(&message.chat_id, request.message_id.as_deref(), body)
            .await?;
        let _ = adapter
            .clear_reply_markup(&message.chat_id, request.message_id.as_deref())
            .await;
        return Ok(());
    }
    let claimed = state
        .runtime
        .lock()
        .await
        .claim_telegram_thread_settings_apply(
            &request.request_id,
            request.revision,
            local_patch.clone(),
            now_ms(),
        );
    let Some(request) = claimed else {
        return Ok(());
    };
    let patch = remote_thread_settings_patch(&local_patch);
    match remote_control_backend::update_thread_settings_for_client(
        &state,
        &request.remote_client_key,
        &request.expected_thread_id,
        &patch,
    )
    .await
    {
        Ok(_) => {
            state
                .push_event(
                    "info",
                    "telegram_thread_settings_submitted",
                    format!(
                        "conversation={} thread={} client={} revision={}",
                        request.conversation_key,
                        request.expected_thread_id,
                        request.remote_client_key,
                        request.revision,
                    ),
                )
                .await;
            schedule_telegram_thread_settings_confirmation_timeout(state, adapter, request);
        }
        Err(error) => {
            state
                .runtime
                .lock()
                .await
                .release_telegram_thread_settings_apply(&request.request_id, request.revision);
            state
                .push_event(
                    "warn",
                    "telegram_thread_settings_submit_failed",
                    format!(
                        "conversation={} thread={} client={} err={error}",
                        request.conversation_key,
                        request.expected_thread_id,
                        request.remote_client_key
                    ),
                )
                .await;
            adapter
                .send_text(
                    &message.chat_id,
                    &im_text_for_state(&state).telegram_thread_settings_apply_failed(&error),
                )
                .await?;
        }
    }
    Ok(())
}

fn schedule_telegram_thread_settings_confirmation_timeout(
    state: SharedState,
    adapter: TelegramAdapter,
    request: TelegramModelSwitchRequestState,
) {
    tokio::spawn(async move {
        sleep(TELEGRAM_THREAD_SETTINGS_APPLY_CONFIRMATION_TIMEOUT).await;
        let request = state
            .runtime
            .lock()
            .await
            .take_unconfirmed_telegram_thread_settings_apply(&request.request_id, request.revision);
        let Some(request) = request else {
            return;
        };
        let body = im_text_for_state(&state).telegram_thread_settings_submitted_unconfirmed();
        let _ = adapter
            .send_or_update_text(&request.chat_id, request.message_id.as_deref(), body)
            .await;
        let _ = adapter
            .clear_reply_markup(&request.chat_id, request.message_id.as_deref())
            .await;
    });
}

fn thread_settings_selected_model(
    request: &TelegramModelSwitchRequestState,
) -> Option<&TelegramThreadSettingsModelChoice> {
    let model = request
        .draft
        .model
        .as_deref()
        .or(match &request.observed.model {
            crate::im_runtime::ObservedSetting::Known(Some(model)) => Some(model.as_str()),
            _ => None,
        })?;
    request.catalog.iter().find(|choice| choice.model == model)
}

fn thread_settings_compatibility(
    request: &TelegramModelSwitchRequestState,
) -> TelegramThreadSettingsCompatibility {
    let selected_model = thread_settings_selected_model(request);
    let target_model_changed = request.draft.model.as_deref().is_some_and(|model| {
        !matches!(
            &request.observed.model,
            crate::im_runtime::ObservedSetting::Known(Some(current)) if current == model
        )
    });
    let selected_effort = request
        .draft
        .effort
        .as_deref()
        .or(match &request.observed.effort {
            crate::im_runtime::ObservedSetting::Known(Some(effort)) => Some(effort.as_str()),
            _ => None,
        });
    let selected_fast = matches!(request.draft.speed, Some(TelegramThreadSettingsSpeed::Fast))
        || (request.draft.speed.is_none()
            && matches!(
                &request.observed.service_tier,
                crate::im_runtime::ObservedSetting::Known(Some(tier)) if tier == "priority"
            ));
    TelegramThreadSettingsCompatibility {
        reset_effort: (request.draft.effort.is_some() || target_model_changed)
            && selected_model.is_some_and(|choice| {
                selected_effort.is_some_and(|effort| {
                    !choice.supported_efforts.iter().any(|item| item == effort)
                })
            }),
        reset_speed: (request.draft.speed.is_some() || target_model_changed)
            && selected_fast
            && selected_model.is_some_and(|choice| !choice.supports_fast),
    }
}

fn thread_settings_patch(
    request: &TelegramModelSwitchRequestState,
    compatibility: Option<&TelegramThreadSettingsCompatibility>,
) -> TelegramThreadSettingsPatch {
    let reset_effort = compatibility.is_some_and(|value| value.reset_effort);
    let reset_speed = compatibility.is_some_and(|value| value.reset_speed);
    TelegramThreadSettingsPatch {
        model: thread_settings_string_patch(
            request.draft.model.as_deref(),
            &request.observed.model,
        ),
        effort: if reset_effort {
            TelegramThreadSettingsPatchValue::Clear
        } else {
            thread_settings_string_patch(request.draft.effort.as_deref(), &request.observed.effort)
        },
        service_tier: if reset_speed {
            TelegramThreadSettingsPatchValue::Clear
        } else {
            match request.draft.speed {
                None => TelegramThreadSettingsPatchValue::Unchanged,
                Some(TelegramThreadSettingsSpeed::Fast) => match &request.observed.service_tier {
                    crate::im_runtime::ObservedSetting::Known(Some(value))
                        if value == "priority" =>
                    {
                        TelegramThreadSettingsPatchValue::Unchanged
                    }
                    _ => TelegramThreadSettingsPatchValue::Set("priority".to_string()),
                },
                Some(TelegramThreadSettingsSpeed::Standard) => {
                    match &request.observed.service_tier {
                        crate::im_runtime::ObservedSetting::Known(None) => {
                            TelegramThreadSettingsPatchValue::Unchanged
                        }
                        _ => TelegramThreadSettingsPatchValue::Clear,
                    }
                }
            }
        },
    }
}

fn thread_settings_string_patch(
    selected: Option<&str>,
    observed: &crate::im_runtime::ObservedSetting<String>,
) -> TelegramThreadSettingsPatchValue {
    match selected {
        None => TelegramThreadSettingsPatchValue::Unchanged,
        Some(selected) if matches!(observed, crate::im_runtime::ObservedSetting::Known(Some(value)) if value == selected) => {
            TelegramThreadSettingsPatchValue::Unchanged
        }
        Some(selected) => TelegramThreadSettingsPatchValue::Set(selected.to_string()),
    }
}

fn remote_thread_settings_patch(
    patch: &TelegramThreadSettingsPatch,
) -> remote_control_backend::ThreadSettingsPatch {
    use remote_control_backend::ThreadSettingsPatchValue;

    fn convert(value: &TelegramThreadSettingsPatchValue) -> ThreadSettingsPatchValue {
        match value {
            TelegramThreadSettingsPatchValue::Unchanged => ThreadSettingsPatchValue::Unchanged,
            TelegramThreadSettingsPatchValue::Set(value) => {
                ThreadSettingsPatchValue::Set(value.clone())
            }
            TelegramThreadSettingsPatchValue::Clear => ThreadSettingsPatchValue::Clear,
        }
    }

    remote_control_backend::ThreadSettingsPatch {
        model: convert(&patch.model),
        effort: convert(&patch.effort),
        service_tier: convert(&patch.service_tier),
    }
}

async fn handle_telegram_thread_list_text_reply(
    state: SharedState,
    adapter: TelegramAdapter,
    message: InboundMessage,
    command: &str,
) -> Result<bool> {
    let Some(index) = numeric_command_index(command) else {
        return Ok(false);
    };
    let Some(request) =
        pending_telegram_thread_list_request(&state, &message.conversation_key()).await
    else {
        return Ok(false);
    };
    let page = request.page.max(1);
    let Some(thread_id) = request
        .thread_ids_by_page
        .get(page.saturating_sub(1))
        .and_then(|thread_ids| thread_ids.get(index))
        .cloned()
    else {
        let text = im_text_for_state(&state);
        adapter
            .send_text(&message.chat_id, text.invalid_thread_index_restart())
            .await?;
        return Ok(true);
    };
    handle_telegram_thread_route_resume_selected(
        state,
        adapter,
        message,
        &request.request_id,
        &thread_id,
    )
    .await?;
    Ok(true)
}

async fn handle_telegram_thread_create_option_text_reply(
    state: SharedState,
    adapter: TelegramAdapter,
    message: InboundMessage,
    command: &str,
) -> Result<bool> {
    let Some(index) = numeric_command_index(command) else {
        return Ok(false);
    };
    let Some(mut request) =
        pending_telegram_thread_create_options_request(&state, &message.conversation_key()).await
    else {
        return Ok(false);
    };
    let page = request.page.max(1);
    let Some((field, value)) =
        request
            .create_option_values_by_field_page
            .iter()
            .find_map(|(field, pages)| {
                pages
                    .get(page.saturating_sub(1))
                    .and_then(|values| values.get(index))
                    .cloned()
                    .map(|value| (field.clone(), value))
            })
    else {
        let text = im_text_for_state(&state);
        adapter
            .send_text(&message.chat_id, text.create_option_unavailable())
            .await?;
        return Ok(true);
    };
    apply_thread_create_draft_value(&mut request.create_draft, &field, &value)?;
    state
        .runtime
        .lock()
        .await
        .remember_thread_routing_request(request.clone());
    if field == "cwd" && value == "__custom__" {
        send_telegram_thread_create_custom_cwd_prompt(&state, &adapter, &message).await?;
        return Ok(true);
    }
    send_telegram_thread_create_settings(&state, &adapter, &message, Some(request)).await?;
    Ok(true)
}

async fn pending_telegram_thread_create_options_request(
    state: &SharedState,
    conversation_key: &str,
) -> Option<ThreadRoutingRequestState> {
    state
        .runtime
        .lock()
        .await
        .thread_routing_requests
        .values()
        .filter(|request| request.conversation_key == conversation_key)
        .filter(|request| !request.create_option_values_by_field_page.is_empty())
        .max_by_key(|request| thread_routing_request_rank(&request.request_id))
        .cloned()
}

async fn pending_telegram_thread_list_request(
    state: &SharedState,
    conversation_key: &str,
) -> Option<ThreadRoutingRequestState> {
    state
        .runtime
        .lock()
        .await
        .thread_routing_requests
        .values()
        .filter(|request| request.conversation_key == conversation_key)
        .filter(|request| {
            request
                .thread_ids_by_page
                .get(request.page.saturating_sub(1))
                .is_some_and(|thread_ids| !thread_ids.is_empty())
        })
        .max_by_key(|request| thread_routing_request_rank(&request.request_id))
        .cloned()
}

fn thread_routing_request_rank(request_id: &str) -> u64 {
    request_id
        .strip_prefix("thread-route-")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or_default()
}

async fn handle_telegram_thread_create_text_input(
    state: &SharedState,
    adapter: &TelegramAdapter,
    message: &InboundMessage,
    text: &str,
) -> Result<bool> {
    let Some(mut request) =
        pending_telegram_thread_create_custom_cwd_request(state, &message.conversation_key()).await
    else {
        return Ok(false);
    };
    if text.eq_ignore_ascii_case("/cancel") {
        request.create_draft.cwd_choice = None;
        request.create_draft.cwd_custom = None;
        state
            .runtime
            .lock()
            .await
            .remember_thread_routing_request(request.clone());
        send_telegram_thread_create_settings(state, adapter, message, Some(request)).await?;
        return Ok(true);
    }
    if command(text).is_some() {
        request.create_draft.cwd_choice = None;
        request.create_draft.cwd_custom = None;
        state
            .runtime
            .lock()
            .await
            .remember_thread_routing_request(request);
        return Ok(false);
    }
    let path = text.trim();
    if path.is_empty() {
        send_telegram_thread_create_custom_cwd_prompt(state, adapter, message).await?;
        return Ok(true);
    }
    if !expand_home_prefix(path).is_absolute() {
        let text = im_text_for_state(state);
        adapter
            .send_text(&message.chat_id, text.cwd_must_be_absolute_telegram())
            .await?;
        return Ok(true);
    }
    request.create_draft.cwd_choice = None;
    request.create_draft.cwd_custom = Some(path.to_string());
    state
        .runtime
        .lock()
        .await
        .remember_thread_routing_request(request.clone());
    send_telegram_thread_create_settings(state, adapter, message, Some(request)).await?;
    Ok(true)
}

async fn pending_telegram_thread_create_custom_cwd_request(
    state: &SharedState,
    conversation_key: &str,
) -> Option<ThreadRoutingRequestState> {
    state
        .runtime
        .lock()
        .await
        .thread_routing_requests
        .values()
        .find(|request| {
            request.conversation_key == conversation_key
                && request.create_draft.cwd_choice.as_deref() == Some("__custom__")
                && request.create_draft.cwd_custom.is_none()
        })
        .cloned()
}

async fn send_telegram_thread_create_custom_cwd_prompt(
    state: &SharedState,
    adapter: &TelegramAdapter,
    message: &InboundMessage,
) -> Result<()> {
    let text = im_text_for_state(state);
    let pending =
        pending_telegram_thread_create_custom_cwd_request(state, &message.conversation_key()).await;
    let prompt_message_id = adapter
        .send_or_update_text(
            &message.chat_id,
            pending
                .as_ref()
                .and_then(|request| request.message_id.as_deref()),
            text.custom_cwd_prompt_telegram(),
        )
        .await?;
    if let Some(request) = pending {
        state
            .runtime
            .lock()
            .await
            .update_thread_routing_request_message_id(&request.request_id, prompt_message_id);
    }
    Ok(())
}

async fn send_telegram_thread_routing_choice(
    state: &SharedState,
    adapter: &TelegramAdapter,
    message: &InboundMessage,
    existing_request: Option<ThreadRoutingRequestState>,
) -> Result<()> {
    let route = route_for_message(message);
    let request_id = existing_request
        .as_ref()
        .map(|request| request.request_id.clone())
        .unwrap_or_else(next_thread_routing_request_id);
    let existing_message_id = existing_request
        .as_ref()
        .and_then(|request| request.message_id.as_deref());
    let text = im_text_for_state(state);
    let message_id = adapter
        .send_thread_routing_choice(&route.chat_id, &request_id, existing_message_id, text)
        .await?;
    state
        .runtime
        .lock()
        .await
        .remember_thread_routing_request(empty_thread_routing_request(
            &route, request_id, message_id,
        ));
    Ok(())
}

async fn send_telegram_thread_create_settings(
    state: &SharedState,
    adapter: &TelegramAdapter,
    message: &InboundMessage,
    existing_request: Option<ThreadRoutingRequestState>,
) -> Result<()> {
    let route = route_for_message(message);
    let request_id = existing_request
        .as_ref()
        .map(|request| request.request_id.clone())
        .unwrap_or_else(next_thread_routing_request_id);
    let create_draft = existing_request
        .as_ref()
        .map(|request| request.create_draft.clone())
        .unwrap_or_default();
    let remote_client_key = route.remote_client_key.clone();
    let defaults = load_thread_create_defaults_for_client(state, &remote_client_key).await;
    let im_text = im_text_for_state(state);
    let text = thread_create_help_text(&defaults, &create_draft, im_text);
    let existing_message_id = existing_request
        .as_ref()
        .and_then(|request| request.message_id.as_deref());
    let message_id = adapter
        .send_thread_create_settings(
            &route.chat_id,
            &request_id,
            &text,
            existing_message_id,
            im_text,
        )
        .await?;
    state
        .runtime
        .lock()
        .await
        .remember_thread_routing_request(ThreadRoutingRequestState {
            request_id: request_id.clone(),
            conversation_key: route.conversation_key,
            account_id: route.account_id,
            chat_id: route.chat_id,
            message_id: Some(message_id),
            stage: ThreadRoutingStage::CreateSettings,
            page: 1,
            page_cursors: vec![None],
            thread_ids_by_page: vec![vec![]],
            create_draft,
            create_option_values_by_field_page: Default::default(),
            history_cursor: None,
            history_has_next: false,
        });
    Ok(())
}

async fn send_telegram_thread_create_options(
    state: &SharedState,
    adapter: &TelegramAdapter,
    message: &InboundMessage,
    mut request: ThreadRoutingRequestState,
    field: &str,
    page: usize,
) -> Result<()> {
    let Some(field) = normalize_thread_create_field(field) else {
        let text = im_text_for_state(state);
        adapter
            .send_text(&message.chat_id, text.create_option_unavailable())
            .await?;
        return Ok(());
    };
    let remote_client_key = route_for_message(message).remote_client_key;
    let defaults = load_thread_create_defaults_for_client(state, &remote_client_key).await;
    let text = im_text_for_state(state);
    let (title, body, options) =
        create_options_for_field(&defaults, &request.create_draft, field, text)?;
    let total_pages = options
        .len()
        .div_ceil(TELEGRAM_CREATE_OPTION_PAGE_SIZE)
        .max(1);
    let page = page.clamp(1, total_pages);
    let start = (page - 1) * TELEGRAM_CREATE_OPTION_PAGE_SIZE;
    let end = (start + TELEGRAM_CREATE_OPTION_PAGE_SIZE).min(options.len());
    let page_options = options[start..end]
        .iter()
        .map(|(_, option)| option.clone())
        .collect::<Vec<_>>();
    let value_pages = options
        .chunks(TELEGRAM_CREATE_OPTION_PAGE_SIZE)
        .map(|chunk| {
            chunk
                .iter()
                .map(|(value, _)| value.clone())
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    request.create_option_values_by_field_page.clear();
    request
        .create_option_values_by_field_page
        .insert(field.to_string(), value_pages);
    request.stage = ThreadRoutingStage::CreateOptions;
    request.page = page;
    let message_id = adapter
        .send_thread_create_options(
            &request.chat_id,
            &request.request_id,
            field,
            &title,
            &body,
            &page_options,
            page,
            page > 1,
            page < total_pages,
            request.message_id.as_deref(),
            text,
        )
        .await?;
    request.message_id = Some(message_id);
    state
        .runtime
        .lock()
        .await
        .remember_thread_routing_request(request.clone());
    state
        .push_event(
            "info",
            "telegram_thread_create_options_sent",
            format!(
                "conversation={} field={} page={page} options={}",
                request.conversation_key,
                field,
                options.len()
            ),
        )
        .await;
    Ok(())
}

async fn send_telegram_thread_routing_list(
    state: &SharedState,
    adapter: &TelegramAdapter,
    message: &InboundMessage,
    existing_request: Option<ThreadRoutingRequestState>,
    cursor: Option<&str>,
    page: usize,
) -> Result<()> {
    let route = route_for_message(message);
    let existing_message_id = existing_request
        .as_ref()
        .and_then(|request| request.message_id.as_deref());
    let loaded_page =
        match load_thread_routing_page(state, &route, existing_request.as_ref(), cursor, page, 8)
            .await
        {
            Ok(page) => page,
            Err(err) => {
                state
                    .push_event(
                        "error",
                        "telegram_thread_list_failed",
                        format!("conversation={} err={err}", route.conversation_key),
                    )
                    .await;
                adapter
                    .send_or_update_text(
                        &route.chat_id,
                        existing_message_id,
                        im_text_for_state(state).list_load_failed(),
                    )
                    .await?;
                return Ok(());
            }
        };
    let telegram_entries = loaded_page
        .entries
        .iter()
        .map(|entry| TelegramThreadListEntry {
            title: entry.title.clone(),
            state: entry.state.clone(),
            cwd: entry.cwd.clone(),
        })
        .collect::<Vec<_>>();

    let text = im_text_for_state(state);
    let body = text.thread_list_body_telegram(loaded_page.model_provider_filter.as_deref());
    let message_id = adapter
        .send_thread_list(
            &route.chat_id,
            &loaded_page.request_id,
            text.thread_list_title_telegram(),
            &body,
            &telegram_entries,
            loaded_page.page,
            loaded_page.page > 1,
            loaded_page.next_cursor.is_some(),
            existing_message_id,
            text,
        )
        .await?;
    state
        .runtime
        .lock()
        .await
        .remember_thread_routing_request(loaded_page.into_request(
            &route,
            message_id,
            existing_request.as_ref(),
            cursor,
        ));
    Ok(())
}

async fn handle_telegram_thread_route_list_page(
    state: SharedState,
    adapter: TelegramAdapter,
    message: InboundMessage,
    request_id: &str,
    direction: ThreadRouteDirection,
) -> Result<()> {
    let Some(request) =
        checked_telegram_thread_routing_request(&state, &adapter, &message, request_id).await?
    else {
        return Ok(());
    };
    let target_page = match direction {
        ThreadRouteDirection::Prev => request.page.saturating_sub(1).max(1),
        ThreadRouteDirection::Next => request.page.saturating_add(1),
    };
    let cursor = request
        .page_cursors
        .get(target_page.saturating_sub(1))
        .cloned()
        .flatten();
    send_telegram_thread_routing_list(
        &state,
        &adapter,
        &message,
        Some(request),
        cursor.as_deref(),
        target_page,
    )
    .await
}

async fn handle_telegram_thread_route_resume_selected(
    state: SharedState,
    adapter: TelegramAdapter,
    message: InboundMessage,
    request_id: &str,
    thread_id: &str,
) -> Result<()> {
    let Some(request) =
        checked_telegram_thread_routing_request(&state, &adapter, &message, request_id).await?
    else {
        return Ok(());
    };
    let text = im_text_for_state(&state);
    let progress_message_id = adapter
        .send_or_update_text(
            &message.chat_id,
            request.message_id.as_deref(),
            &text.subscribing_thread(thread_id),
        )
        .await?;
    let route = route_for_message(&message);
    let thread = match resume_and_bind_thread(&state, &route, thread_id, Some(request_id)).await {
        Ok(thread) => thread,
        Err(error) => {
            let mut retry_request = request;
            retry_request.message_id = Some(progress_message_id);
            send_telegram_thread_routing_choice(&state, &adapter, &message, Some(retry_request))
                .await?;
            adapter
                .send_text(&message.chat_id, &text.app_message_failed(&error))
                .await?;
            return Err(error);
        }
    };
    let body = text.subscribed_session_body(
        thread_id,
        &summarize_thread_title(&thread, text),
        &summarize_thread_cwd(&thread, text),
        &summarize_thread_status(&thread, text),
    );
    publish_telegram_thread_routing_result(
        &state,
        &adapter,
        &route.chat_id,
        &route.conversation_key,
        thread_id,
        text.subscribed_session_title(),
        &body,
        &progress_message_id,
        "telegram_thread_route_resumed",
    )
    .await;
    state
        .push_event(
            "info",
            "telegram_thread_route_resumed",
            format!("conversation={} thread={thread_id}", route.conversation_key),
        )
        .await;
    Ok(())
}

async fn handle_telegram_approval_text_reply(
    state: &SharedState,
    outbound_tx: &ImOutboundSender,
    adapter: &TelegramAdapter,
    message: &InboundMessage,
    command: &str,
) -> Result<bool> {
    handle_telegram_approval_outcome(
        state,
        outbound_tx,
        adapter,
        message,
        resolve_approval_reply(state, message, command).await,
    )
    .await
}

async fn handle_telegram_approval_button_reply(
    state: &SharedState,
    outbound_tx: &ImOutboundSender,
    adapter: &TelegramAdapter,
    message: &InboundMessage,
    request_fingerprint: &str,
    option_index: usize,
) -> Result<bool> {
    handle_telegram_approval_outcome(
        state,
        outbound_tx,
        adapter,
        message,
        resolve_approval_button_reply(state, message, request_fingerprint, option_index).await,
    )
    .await
}

async fn handle_telegram_approval_outcome(
    state: &SharedState,
    outbound_tx: &ImOutboundSender,
    adapter: &TelegramAdapter,
    message: &InboundMessage,
    outcome: ApprovalReplyOutcome,
) -> Result<bool> {
    match outcome {
        ApprovalReplyOutcome::Ready {
            conversation_key,
            pending,
            option_index,
            decision,
        } => {
            let approval_message_id = pending
                .message_id
                .clone()
                .or_else(|| message.card_message_id.clone());
            let next = submit_approval_decision(state, &pending, &decision).await?;
            let text = im_text_for_state(state);
            let resolved = adapter
                .update_resolved_approval(
                    &message.chat_id,
                    approval_message_id.as_deref(),
                    &pending,
                    option_index,
                    &decision.label,
                    text,
                )
                .await;
            let update_succeeded = match resolved {
                Ok(updated) => updated,
                Err(err) => {
                    state
                        .push_event(
                            "warn",
                            "telegram_approval_update_failed",
                            format!(
                                "conversation={} request_id={} message={} err={err}",
                                conversation_key,
                                pending.request_id,
                                approval_message_id.as_deref().unwrap_or("")
                            ),
                        )
                        .await;
                    false
                }
            };
            if !update_succeeded {
                adapter
                    .send_text(
                        &message.chat_id,
                        &approval_decision_fallback_text(text, &decision.label),
                    )
                    .await?;
            }
            state
                .push_event(
                    "info",
                    "telegram_approval_decision_sent",
                    format!(
                        "conversation={} request_id={} option={} label={}",
                        conversation_key, pending.request_id, option_index, decision.label
                    ),
                )
                .await;
            if let Some((conversation_key, next_approval)) = next {
                events::send_next_approval(state, outbound_tx, &conversation_key, &next_approval)
                    .await?;
            }
        }
        ApprovalReplyOutcome::NoPending => {
            let text = im_text_for_state(state);
            let _ = adapter
                .clear_reply_markup(&message.chat_id, message.card_message_id.as_deref())
                .await;
            adapter
                .send_text(&message.chat_id, text.no_pending_approval())
                .await?;
        }
        ApprovalReplyOutcome::NotCurrent => {
            let text = im_text_for_state(state);
            let _ = adapter
                .clear_reply_markup(&message.chat_id, message.card_message_id.as_deref())
                .await;
            adapter
                .send_text(&message.chat_id, text.approval_not_current())
                .await?;
        }
        ApprovalReplyOutcome::InvalidInput { hint } => {
            let text = im_text_for_state(state);
            adapter
                .send_text(&message.chat_id, &text.invalid_approval_reply(&hint))
                .await?;
        }
    }
    Ok(true)
}

/// `/回复 <档位>`：切换本账号的回复颗粒度并持久化；无参数时显示当前档位与可选项。
async fn handle_telegram_reply_granularity_command(
    state: &SharedState,
    adapter: &TelegramAdapter,
    message: &InboundMessage,
    payload: &str,
) -> Result<()> {
    let text = im_text_for_state(state);
    let value = payload.trim();
    if value.is_empty() {
        let current = state
            .config
            .lock()
            .await
            .telegram_reply_granularity(&message.account_id);
        adapter
            .send_text(&message.chat_id, &text.telegram_granularity_status(current))
            .await?;
        return Ok(());
    }
    let Some(granularity) = TelegramReplyGranularity::parse(value) else {
        adapter
            .send_text(&message.chat_id, text.telegram_granularity_unknown())
            .await?;
        return Ok(());
    };
    let update = {
        let mut config = state.config.lock().await;
        match config.telegram_account(&message.account_id) {
            None => None,
            Some(mut account) => {
                account.reply_granularity = granularity;
                config.upsert_telegram_account(account);
                Some(config.save(&state.config_path).err())
            }
        }
    };
    let Some(save_error) = update else {
        adapter
            .send_text(&message.chat_id, text.telegram_granularity_unknown())
            .await?;
        return Ok(());
    };
    if let Some(err) = save_error {
        state
            .push_event(
                "error",
                "telegram_granularity_save_failed",
                format!("chat={} err={err}", message.chat_id),
            )
            .await;
    }
    adapter
        .send_text(
            &message.chat_id,
            &text.telegram_granularity_set(granularity),
        )
        .await?;
    Ok(())
}

pub(crate) fn command(text: &str) -> Option<String> {
    let first = text.split_whitespace().next()?.trim();
    if !first.starts_with('/') {
        return None;
    }
    let command = first
        .split_once('@')
        .map(|(command, _)| command)
        .unwrap_or(first)
        .to_ascii_lowercase();
    Some(command)
}

pub(crate) fn numeric_command_index(command: &str) -> Option<usize> {
    let number = command.strip_prefix('/')?.parse::<usize>().ok()?;
    number.checked_sub(1)
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, sync::Mutex};

    use anyhow::{Result, anyhow};

    use crate::im::core::i18n::ImText;

    use super::{
        TelegramThreadRoutingResultDelivery, TelegramThreadRoutingResultSender,
        approval_decision_fallback_text, callback_targets_current_message,
        deliver_telegram_thread_routing_result,
    };

    struct ScriptedThreadRoutingResultSender {
        results: Mutex<VecDeque<Result<String>>>,
        message_ids: Mutex<Vec<Option<String>>>,
    }

    impl ScriptedThreadRoutingResultSender {
        fn new(results: Vec<Result<String>>) -> Self {
            Self {
                results: Mutex::new(results.into()),
                message_ids: Mutex::new(Vec::new()),
            }
        }

        fn message_ids(&self) -> Vec<Option<String>> {
            self.message_ids.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl TelegramThreadRoutingResultSender for ScriptedThreadRoutingResultSender {
        async fn send_thread_routing_result(
            &self,
            _target: &str,
            _title: &str,
            _body: &str,
            message_id: Option<&str>,
        ) -> Result<String> {
            self.message_ids
                .lock()
                .unwrap()
                .push(message_id.map(str::to_string));
            self.results
                .lock()
                .unwrap()
                .pop_front()
                .expect("missing scripted Telegram result")
        }
    }

    #[test]
    fn rejects_callback_from_replaced_routing_message() {
        assert!(!callback_targets_current_message(Some("10"), Some("9")));
    }

    #[test]
    fn accepts_current_callback_and_text_replies() {
        assert!(callback_targets_current_message(Some("10"), Some("10")));
        assert!(callback_targets_current_message(Some("10"), None));
    }

    #[test]
    fn approval_failure_fallback_uses_the_localized_decision_label() {
        assert_eq!(
            approval_decision_fallback_text(ImText::zh_cn(), "Yes, proceed"),
            "已提交：允许执行"
        );
    }

    #[test]
    fn command_parser_normalizes_standard_commands_and_legacy_aliases() {
        assert_eq!(
            super::command("/STOP@MochiPort extra"),
            Some("/stop".to_string())
        );
        assert_eq!(super::command("/s"), Some("/s".to_string()));
        assert_eq!(super::command("/q"), Some("/q".to_string()));
        assert_eq!(super::command("/1"), Some("/1".to_string()));
        assert_eq!(super::command("status"), None);
        assert_eq!(super::command("/queue hello"), Some("/queue".to_string()));
        assert_eq!(
            super::command("/steer@MochiPort new direction"),
            Some("/steer".to_string())
        );
        assert_eq!(super::command_payload("/queue hello world"), "hello world");
        assert_eq!(super::command_payload("/queue@MochiPort hello"), "hello");
    }

    #[tokio::test]
    async fn final_routing_result_updates_progress_message_without_fallback() {
        let sender = ScriptedThreadRoutingResultSender::new(vec![Ok("10".to_string())]);

        let delivery =
            deliver_telegram_thread_routing_result(&sender, "chat", "title", "body", "10").await;

        assert_eq!(delivery, TelegramThreadRoutingResultDelivery::Updated);
        assert_eq!(sender.message_ids(), vec![Some("10".to_string())]);
    }

    #[tokio::test]
    async fn final_routing_result_falls_back_to_new_message_after_update_failure() {
        let sender = ScriptedThreadRoutingResultSender::new(vec![
            Err(anyhow!("edit failed")),
            Ok("11".to_string()),
        ]);

        let delivery =
            deliver_telegram_thread_routing_result(&sender, "chat", "title", "body", "10").await;

        assert_eq!(
            delivery,
            TelegramThreadRoutingResultDelivery::SentAsNew {
                message_id: "11".to_string(),
                update_error: "edit failed".to_string(),
            }
        );
        assert_eq!(sender.message_ids(), vec![Some("10".to_string()), None]);
    }

    #[tokio::test]
    async fn final_routing_result_swallows_both_delivery_failures() {
        let sender = ScriptedThreadRoutingResultSender::new(vec![
            Err(anyhow!("edit failed")),
            Err(anyhow!("send failed")),
        ]);

        let delivery =
            deliver_telegram_thread_routing_result(&sender, "chat", "title", "body", "10").await;

        assert_eq!(
            delivery,
            TelegramThreadRoutingResultDelivery::Undelivered {
                update_error: "edit failed".to_string(),
                fallback_error: "send failed".to_string(),
            }
        );
        assert_eq!(sender.message_ids(), vec![Some("10".to_string()), None]);
    }
}
