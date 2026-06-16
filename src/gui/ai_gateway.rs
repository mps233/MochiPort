use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use wxdragon::prelude::*;
use wxdragon::widgets::dataview::CustomDataViewVirtualListModel;

use crate::ai_gateway::config::{AiGatewayConfig, ProviderConfig, ProviderType};

use super::UiHandles;
use super::api::{ApiClient, ConfigureRequest, DeleteProviderRequest};
use super::text::GuiText;

pub(super) type AiGwProviderRows = Rc<RefCell<Vec<[String; 7]>>>;
pub(super) type AiGwProviderModel = Rc<RefCell<CustomDataViewVirtualListModel>>;
pub(super) type PendingAiGwChannelToggle = Rc<RefCell<Option<(String, bool)>>>;
pub(super) type AiGwActionResultStore = Arc<Mutex<Option<AiGwActionResult>>>;

pub(super) enum AiGwActionResult {
    Save(Result<(), String>),
    Delete(Result<(), String>),
    Toggle(Result<(), String>),
    ChannelToggle(Result<(), String>),
}

pub(super) fn save_ai_gw_provider(api: &ApiClient, provider: ProviderConfig) -> Result<(), String> {
    let mut config = api.get_app_config()?;
    if let Some(existing) = config
        .ai_gateway
        .providers
        .iter_mut()
        .find(|p| p.name == provider.name)
    {
        *existing = provider;
    } else {
        config.ai_gateway.providers.push(provider);
    }
    api.save_app_config(&config)?;
    Ok(())
}

pub(super) fn delete_ai_gw_provider(api: &ApiClient, name: &str) -> Result<(), String> {
    let mut config = api.get_app_config()?;
    config.ai_gateway.providers.retain(|p| p.name != name);
    api.save_app_config(&config)?;
    Ok(())
}

pub(super) fn toggle_ai_gw_enabled(api: &ApiClient, enabled: bool) -> Result<(), String> {
    let mut config = api.get_app_config()?;
    config.ai_gateway.enabled = enabled;
    api.save_app_config(&config)?;

    if enabled {
        let request = ConfigureRequest {
            provider_name: Some("ai-gateway".to_string()),
            provider_base_url: Some(gateway_entry_url(&api.base_url)),
            provider_key: Some(String::new()),
            model: None,
            activate: true,
            image_generation_enabled: None,
            supports_websockets: false,
        };
        api.configure_codex_app(&request)?;
    } else {
        let request = DeleteProviderRequest {
            provider_name: "ai-gateway".to_string(),
        };
        let _ = api.delete_codex_provider(&request);
    }
    Ok(())
}

pub(super) fn set_ai_gw_provider_enabled(
    api: &ApiClient,
    name: &str,
    enabled: bool,
) -> Result<(), String> {
    let mut config = api.get_app_config()?;
    let Some(provider) = config
        .ai_gateway
        .providers
        .iter_mut()
        .find(|provider| provider.name == name)
    else {
        return Err(format!("AI Gateway channel not found: {name}"));
    };
    provider.enabled = enabled;
    api.save_app_config(&config)?;
    Ok(())
}

pub(super) fn refresh_ai_gw_provider_list(handles: &UiHandles, config: Option<&AiGatewayConfig>) {
    let rows = ai_gw_provider_list_rows(handles.text, config);
    let mut current_rows = handles.ai_gw_provider_rows.borrow_mut();
    if *current_rows == rows {
        return;
    }

    let previous_len = current_rows.len();
    let selected_row = handles.ai_gw_provider_list.get_selected_row();
    let new_len = rows.len();
    *current_rows = rows;
    drop(current_rows);

    if previous_len != new_len {
        handles.ai_gw_provider_model.borrow_mut().reset(new_len);
        if let Some(row) = selected_row.filter(|row| *row < new_len) {
            handles.ai_gw_provider_list.select_row(row);
        }
    } else {
        let model = handles.ai_gw_provider_model.borrow();
        for row in 0..new_len {
            model.row_changed(row);
        }
    }
}

fn ai_gw_provider_list_rows(text: GuiText, config: Option<&AiGatewayConfig>) -> Vec<[String; 7]> {
    let Some(config) = config else {
        return Vec::new();
    };
    config
        .providers
        .iter()
        .map(|p| {
            [
                p.enabled.to_string(),
                p.name.clone(),
                provider_service_display(text, p),
                provider_type_display(&p.provider_type),
                provider_models_display(&p.models),
                p.base_url.clone(),
                p.timeout_secs.to_string(),
            ]
        })
        .collect()
}

fn provider_service_display(text: GuiText, provider: &ProviderConfig) -> String {
    match provider_service_index(provider) {
        1 => "DeepSeek".to_string(),
        0 => "OpenAI".to_string(),
        _ => text.ai_gw_service_custom().to_string(),
    }
}

pub(super) fn provider_service_index(provider: &ProviderConfig) -> i32 {
    let name = provider.name.to_ascii_lowercase();
    let base_url = provider.base_url.to_ascii_lowercase();
    if name.contains("deepseek") || base_url.contains("deepseek") {
        1
    } else if name.contains("openai") || base_url.contains("openai") {
        0
    } else {
        2
    }
}

pub(super) fn provider_type_display(pt: &ProviderType) -> String {
    match pt {
        ProviderType::OpenAiResponses => "OpenAI Responses".to_string(),
        ProviderType::ChatCompletions => "Chat Completions".to_string(),
    }
}

fn provider_models_display(models: &[String]) -> String {
    if models.is_empty() {
        return String::new();
    }
    models.join(" / ")
}

pub(super) fn apply_pending_ai_gw_action(
    handles: &UiHandles,
    frame: &Frame,
    result_store: &AiGwActionResultStore,
) -> bool {
    let result = result_store.lock().ok().and_then(|mut slot| slot.take());
    let Some(result) = result else {
        return false;
    };

    handles
        .ai_gw_delete_button
        .set_label(handles.text.ai_gw_delete_channel());
    set_ai_gw_actions_enabled(handles, true);

    match result {
        AiGwActionResult::Save(Ok(())) => {
            handles.ai_gw_catalog.set_label(handles.text.ai_gw_saved());
            super::show_info(frame, handles.text.ai_gw_saved());
        }
        AiGwActionResult::Save(Err(err)) => {
            handles
                .ai_gw_catalog
                .set_label(&handles.text.ai_gw_save_failed(&err));
            super::show_error(frame, &handles.text.ai_gw_save_failed(&err));
        }
        AiGwActionResult::Delete(Ok(())) => {
            handles
                .ai_gw_catalog
                .set_label(handles.text.ai_gw_deleted());
            super::show_info(frame, handles.text.ai_gw_deleted());
        }
        AiGwActionResult::Delete(Err(err)) => {
            handles
                .ai_gw_catalog
                .set_label(&handles.text.ai_gw_save_failed(&err));
            super::show_error(frame, &handles.text.ai_gw_save_failed(&err));
        }
        AiGwActionResult::Toggle(Ok(())) => {
            handles
                .ai_gw_catalog
                .set_label(handles.text.ai_gw_restart_codex_hint());
            super::show_info(frame, handles.text.ai_gw_restart_codex_hint());
        }
        AiGwActionResult::Toggle(Err(err)) => {
            handles
                .ai_gw_catalog
                .set_label(&handles.text.ai_gw_save_failed(&err));
            super::show_error(frame, &handles.text.ai_gw_save_failed(&err));
        }
        AiGwActionResult::ChannelToggle(Ok(())) => {
            handles.ai_gw_catalog.set_label("");
        }
        AiGwActionResult::ChannelToggle(Err(err)) => {
            handles
                .ai_gw_catalog
                .set_label(&handles.text.ai_gw_save_failed(&err));
            super::show_error(frame, &handles.text.ai_gw_save_failed(&err));
        }
    }
    true
}

pub(super) fn set_ai_gw_actions_enabled(handles: &UiHandles, enabled: bool) {
    handles.ai_gw_delete_button.enable(enabled);
    handles.ai_gw_new_button.enable(enabled);
    handles.ai_gw_edit_button.enable(enabled);
    handles.ai_gw_enabled.enable(enabled);
    handles.ai_gw_provider_list.enable(enabled);
}

pub(super) fn gateway_entry_url(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    format!("{base}/ai-gateway/v1")
}
