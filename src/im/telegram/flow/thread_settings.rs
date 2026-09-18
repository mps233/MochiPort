//! Telegram Thread settings: the `/model`-style settings card, its callbacks, and the resulting Codex patch submission.
//!
//! Moved verbatim out of `flow.rs`; no behavior changes.

use super::*;

pub(super) async fn send_telegram_thread_settings(
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
            .saturating_add(crate::im::runtime::TELEGRAM_THREAD_SETTINGS_DRAFT_MAX_AGE_MS),
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

pub(super) async fn handle_telegram_thread_settings_open_field(
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

pub(super) async fn handle_telegram_thread_settings_model_page(
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

pub(super) async fn handle_telegram_thread_settings_choose_model(
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

pub(super) async fn handle_telegram_thread_settings_choose_effort(
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

pub(super) async fn handle_telegram_thread_settings_choose_speed(
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

pub(super) async fn handle_telegram_thread_settings_back(
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

pub(super) async fn handle_telegram_thread_settings_cancel(
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

pub(super) async fn handle_telegram_thread_settings_apply(
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

pub(super) async fn handle_telegram_thread_settings_compatibility_confirmation(
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
            crate::im::runtime::ObservedSetting::Known(Some(model)) => Some(model.as_str()),
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
            crate::im::runtime::ObservedSetting::Known(Some(current)) if current == model
        )
    });
    let selected_effort = request
        .draft
        .effort
        .as_deref()
        .or(match &request.observed.effort {
            crate::im::runtime::ObservedSetting::Known(Some(effort)) => Some(effort.as_str()),
            _ => None,
        });
    let selected_fast = matches!(request.draft.speed, Some(TelegramThreadSettingsSpeed::Fast))
        || (request.draft.speed.is_none()
            && matches!(
                &request.observed.service_tier,
                crate::im::runtime::ObservedSetting::Known(Some(tier)) if tier == "priority"
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
                    crate::im::runtime::ObservedSetting::Known(Some(value))
                        if value == "priority" =>
                    {
                        TelegramThreadSettingsPatchValue::Unchanged
                    }
                    _ => TelegramThreadSettingsPatchValue::Set("priority".to_string()),
                },
                Some(TelegramThreadSettingsSpeed::Standard) => {
                    match &request.observed.service_tier {
                        crate::im::runtime::ObservedSetting::Known(None) => {
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
    observed: &crate::im::runtime::ObservedSetting<String>,
) -> TelegramThreadSettingsPatchValue {
    match selected {
        None => TelegramThreadSettingsPatchValue::Unchanged,
        Some(selected) if matches!(observed, crate::im::runtime::ObservedSetting::Known(Some(value)) if value == selected) => {
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
