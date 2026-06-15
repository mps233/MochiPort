use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use wxdragon::prelude::*;
use wxdragon::widgets::dataview::CustomDataViewVirtualListModel;

use crate::ai_gateway::config::{AiGatewayConfig, ProviderConfig, ProviderType};

use super::api::{ApiClient, ConfigureRequest, DeleteProviderRequest};
use super::text::GuiText;
use super::UiHandles;

pub(super) type AiGwProviderRows = Rc<RefCell<Vec<[String; 4]>>>;
pub(super) type AiGwProviderModel = Rc<RefCell<CustomDataViewVirtualListModel>>;
pub(super) type AiGwActionResultStore = Arc<Mutex<Option<AiGwActionResult>>>;

pub(super) enum AiGwActionResult {
    Save(Result<(), String>),
    Delete(Result<(), String>),
    Toggle(Result<(), String>),
    DefaultProvider(Result<(), String>),
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
    if config.ai_gateway.default_provider == name {
        config.ai_gateway.default_provider = String::new();
    }
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

pub(super) fn save_ai_gw_default_provider(api: &ApiClient, name: &str) -> Result<(), String> {
    let mut config = api.get_app_config()?;
    config.ai_gateway.default_provider = name.to_string();
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

fn ai_gw_provider_list_rows(
    _text: GuiText,
    config: Option<&AiGatewayConfig>,
) -> Vec<[String; 4]> {
    let Some(config) = config else {
        return Vec::new();
    };
    config
        .providers
        .iter()
        .map(|p| {
            [
                p.name.clone(),
                provider_type_display(&p.provider_type),
                p.base_url.clone(),
                masked_ai_gw_key(&p.api_key),
            ]
        })
        .collect()
}

fn provider_type_display(pt: &ProviderType) -> String {
    match pt {
        ProviderType::OpenAiResponses => "OpenAI Responses".to_string(),
        ProviderType::ChatCompletions => "Chat Completions".to_string(),
    }
}

fn masked_ai_gw_key(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        return String::new();
    }
    let suffix: String = value.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
    format!("****{suffix}")
}

pub(super) fn apply_ai_gw_provider_to_form(handles: &UiHandles, provider: &ProviderConfig) {
    super::provider::change_text_value_if_changed(&handles.ai_gw_name, &provider.name);
    let type_str = provider_type_display(&provider.provider_type);
    super::provider::set_combo_value_if_changed(&handles.ai_gw_type, &type_str);
    super::provider::change_text_value_if_changed(&handles.ai_gw_base_url, &provider.base_url);
    super::provider::change_text_value_if_changed(&handles.ai_gw_key, &provider.api_key);
    super::provider::change_text_value_if_changed(
        &handles.ai_gw_timeout,
        &provider.timeout_secs.to_string(),
    );
    let models_str = provider.models.join(", ");
    super::provider::change_text_value_if_changed(&handles.ai_gw_models, &models_str);
}

pub(super) fn ai_gw_provider_from_form(handles: &UiHandles) -> ProviderConfig {
    let name = handles.ai_gw_name.get_value().trim().to_string();
    let type_str = handles.ai_gw_type.get_value();
    let provider_type = if type_str.contains("Chat") {
        ProviderType::ChatCompletions
    } else {
        ProviderType::OpenAiResponses
    };
    let base_url = handles.ai_gw_base_url.get_value().trim().to_string();
    let api_key = handles.ai_gw_key.get_value().trim().to_string();
    let timeout_secs = handles
        .ai_gw_timeout
        .get_value()
        .trim()
        .parse::<u64>()
        .unwrap_or(300);
    let models: Vec<String> = handles
        .ai_gw_models
        .get_value()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    ProviderConfig {
        name,
        provider_type,
        base_url,
        api_key,
        models,
        prompt_cache_retention: None,
        timeout_secs,
    }
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

    handles.ai_gw_save_button.set_label(handles.text.save());
    handles.ai_gw_delete_button.set_label(handles.text.delete());
    set_ai_gw_actions_enabled(handles, true);

    match result {
        AiGwActionResult::Save(Ok(())) => {
            handles
                .ai_gw_catalog
                .set_label(handles.text.ai_gw_saved());
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
        AiGwActionResult::DefaultProvider(Ok(())) => {
            handles.ai_gw_catalog.set_label("");
        }
        AiGwActionResult::DefaultProvider(Err(err)) => {
            handles
                .ai_gw_catalog
                .set_label(&handles.text.ai_gw_save_failed(&err));
            super::show_error(frame, &handles.text.ai_gw_save_failed(&err));
        }
    }
    true
}

pub(super) fn set_ai_gw_actions_enabled(handles: &UiHandles, enabled: bool) {
    handles.ai_gw_save_button.enable(enabled);
    handles.ai_gw_delete_button.enable(enabled);
    handles.ai_gw_new_button.enable(enabled);
    handles.ai_gw_enabled.enable(enabled);
}

pub(super) fn gateway_entry_url(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    format!("{base}/ai-gateway/v1")
}

pub(super) fn refresh_ai_gw_default_provider_combo(
    handles: &UiHandles,
    config: Option<&AiGatewayConfig>,
) {
    let Some(config) = config else {
        return;
    };
    let names: Vec<String> = config.providers.iter().map(|p| p.name.clone()).collect();
    let current = handles.ai_gw_default_provider.get_value();
    handles.ai_gw_default_provider.clear();
    for name in &names {
        handles.ai_gw_default_provider.append(name);
    }
    if names.contains(&config.default_provider) {
        super::provider::set_combo_value_if_changed(
            &handles.ai_gw_default_provider,
            &config.default_provider,
        );
    } else if !current.is_empty() && names.iter().any(|n| n == &current) {
        super::provider::set_combo_value_if_changed(&handles.ai_gw_default_provider, &current);
    }
}
