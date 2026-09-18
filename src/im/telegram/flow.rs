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
        i18n::{ImText, im_locale_for_state, im_text_for_state},
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
    im::runtime::{
        PendingTelegramTurn, RouteTarget, TELEGRAM_QUEUED_TURNS_MAX_COUNT,
        TelegramModelSwitchRequestState, TelegramQueueEnqueueOutcome,
        TelegramThreadSettingsCompatibility, TelegramThreadSettingsDraft,
        TelegramThreadSettingsModelChoice, TelegramThreadSettingsPatch,
        TelegramThreadSettingsPatchValue, TelegramThreadSettingsSpeed, TelegramThreadSettingsStage,
        ThreadRoutingRequestState, ThreadRoutingStage, TurnOrigin,
        next_telegram_model_switch_request_id,
    },
    im::telegram::{
        adapter::{TelegramAdapter, TelegramThreadListEntry},
        api::TelegramApi,
        types::TelegramSettings,
        typing as telegram_typing,
    },
    remote_control_backend,
    types::{
        InboundAction, InboundMessage, ThreadRouteDirection, ThreadSettingsField, now_ms,
        split_telegram_message_target,
    },
};

mod approval;
mod commands;
#[cfg(test)]
mod tests;
mod thread_routing;
mod thread_settings;

use approval::*;
use commands::*;
use thread_routing::*;
use thread_settings::*;

// `events.rs` reaches this entry point through `telegram_flow`, so it has to be
// re-exported rather than merely being visible to this module.
pub(crate) use thread_routing::start_next_telegram_queued_turn;

const TELEGRAM_CREATE_OPTION_PAGE_SIZE: usize = 8;
const TELEGRAM_MODEL_PAGE_SIZE: usize = 8;
const TELEGRAM_THREAD_SETTINGS_APPLY_CONFIRMATION_TIMEOUT: Duration = Duration::from_secs(5);

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
    let adapter = TelegramAdapter::with_locale(api.clone(), im_locale_for_state(&state).await);
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
        Some("/granularity") | Some("/reply") | Some("/回复") => {
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

fn callback_targets_current_message(
    expected_message_id: Option<&str>,
    callback_message_id: Option<&str>,
) -> bool {
    !matches!(
            (expected_message_id, callback_message_id),
            (Some(expected), Some(actual)) if expected != actual
    // @@MOD_DECLS@@

        )
}
