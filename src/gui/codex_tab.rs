use std::{
    cell::Cell,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use wxdragon::prelude::*;
use wxdragon::widgets::scrolled_window::ScrollBarConfig;

use crate::ai_gateway::catalog::visible_catalog_model_options;
use crate::ai_gateway::config::AiGatewayConfig;

use super::api::{ApiClient, ConfigureRequest};
use super::text::GuiText;
use super::{
    DashboardRefresh, confirm_uninstall_codex_app_config, ensure_service_ready_for_action,
    force_dashboard_refresh, schedule_dashboard_refresh, show_error, show_info,
};

type CodexModelSlugs = Rc<Vec<String>>;
type CodexModelsInitialized = Rc<Cell<bool>>;

pub(super) type CodexActionResultStore = Arc<Mutex<Option<CodexActionResult>>>;

#[derive(Clone)]
pub(super) struct CodexTab {
    pub(super) page: ScrolledWindow,
    text: GuiText,
    provider_image_generation: CheckBox,
    inject_button: Button,
    clear_button: Button,
    save_models_button: Button,
    model_list: CheckListBox,
    model_slugs: CodexModelSlugs,
    models_initialized: CodexModelsInitialized,
}

pub(super) fn create(parent: &Notebook, text: GuiText) -> CodexTab {
    let page = ScrolledWindow::builder(parent)
        .with_style(ScrolledWindowStyle::VScroll)
        .build();
    page.set_background_color(Colour::rgb(250, 251, 253));
    let sizer = BoxSizer::builder(Orientation::Vertical).build();

    let local_config_box = StaticBox::builder(&page)
        .with_label(text.codex_local_config())
        .build();
    local_config_box.set_tooltip(text.codex_local_config_help());
    let local_config_section =
        StaticBoxSizerBuilder::new_with_box(&local_config_box, Orientation::Vertical).build();
    let local_config_hint = StaticText::builder(&local_config_box)
        .with_label(text.codex_local_config_help())
        .build();
    local_config_hint.set_foreground_color(Colour::rgb(103, 111, 124));
    local_config_section.add(
        &local_config_hint,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        10,
    );

    let provider_image_generation = CheckBox::builder(&local_config_box)
        .with_label(text.image_generation_feature())
        .with_value(false)
        .build();
    provider_image_generation.set_tooltip(text.image_generation_feature_help());
    let provider_image_generation_note = StaticText::builder(&local_config_box)
        .with_label(text.image_generation_feature_note())
        .build();
    provider_image_generation_note.set_foreground_color(Colour::rgb(103, 111, 124));
    let provider_image_generation_row = BoxSizer::builder(Orientation::Horizontal).build();
    provider_image_generation_row.add(
        &provider_image_generation,
        0,
        SizerFlag::Right | SizerFlag::AlignCenterVertical,
        8,
    );
    provider_image_generation_row.add(
        &provider_image_generation_note,
        0,
        SizerFlag::AlignCenterVertical,
        0,
    );
    local_config_section.add_sizer(
        &provider_image_generation_row,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        10,
    );

    let inject_button = Button::builder(&local_config_box)
        .with_label(text.inject_codex_access())
        .build();
    inject_button.set_tooltip(text.inject_codex_access_help());
    let clear_button = Button::builder(&local_config_box)
        .with_label(text.clear_codex_access())
        .build();
    clear_button.set_tooltip(text.clear_codex_access_help());
    let local_config_actions = BoxSizer::builder(Orientation::Horizontal).build();
    local_config_actions.add_stretch_spacer(1);
    local_config_actions.add(&inject_button, 0, SizerFlag::Right, 8);
    local_config_actions.add(&clear_button, 0, SizerFlag::Right, 0);
    local_config_section.add_sizer(
        &local_config_actions,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top | SizerFlag::Bottom,
        10,
    );
    sizer.add_sizer(
        &local_config_section,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top | SizerFlag::Bottom,
        12,
    );

    let models_box = StaticBox::builder(&page)
        .with_label(text.codex_visible_models())
        .build();
    models_box.set_tooltip(text.codex_visible_models_help());
    let models_section =
        StaticBoxSizerBuilder::new_with_box(&models_box, Orientation::Vertical).build();
    let models_hint = StaticText::builder(&models_box)
        .with_label(text.codex_visible_models_help())
        .build();
    models_hint.set_foreground_color(Colour::rgb(103, 111, 124));
    models_section.add(
        &models_hint,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        10,
    );

    let model_options = visible_catalog_model_options();
    let model_slugs: CodexModelSlugs = Rc::new(
        model_options
            .iter()
            .map(|model| model.slug.clone())
            .collect(),
    );
    let model_labels = model_options
        .iter()
        .map(|model| {
            if model.display_name == model.slug {
                model.slug.clone()
            } else {
                format!("{} ({})", model.display_name, model.slug)
            }
        })
        .collect();
    let model_list = CheckListBox::builder(&models_box)
        .with_choices(model_labels)
        .with_style(CheckListBoxStyle::AlwaysSB)
        .build();
    model_list.set_min_size(Size::new(360, 170));
    models_section.add(
        &model_list,
        1,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        10,
    );

    let save_models_button = Button::builder(&models_box)
        .with_label(text.save_codex_models())
        .build();
    let models_actions = BoxSizer::builder(Orientation::Horizontal).build();
    models_actions.add_stretch_spacer(1);
    models_actions.add(&save_models_button, 0, SizerFlag::Right, 0);
    models_section.add_sizer(
        &models_actions,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top | SizerFlag::Bottom,
        10,
    );
    sizer.add_sizer(
        &models_section,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
        12,
    );

    page.set_sizer(sizer, true);
    page.set_scroll_rate(10, 10);
    let best_size = page.get_best_size();
    page.set_scrollbars(ScrollBarConfig {
        pixels_per_unit_x: 10,
        pixels_per_unit_y: 10,
        no_units_x: (best_size.width + 20).max(1) / 10,
        no_units_y: (best_size.height + 80).max(1) / 10,
        x_pos: 0,
        y_pos: 0,
        no_refresh: true,
    });

    CodexTab {
        page,
        text,
        provider_image_generation,
        inject_button,
        clear_button,
        save_models_button,
        model_list,
        model_slugs,
        models_initialized: Rc::new(Cell::new(false)),
    }
}

pub(super) fn bind_actions(
    api: &ApiClient,
    frame: &Frame,
    tab: &CodexTab,
    refresh: &DashboardRefresh,
    result: &CodexActionResultStore,
    in_flight: &Arc<AtomicBool>,
) {
    bind_inject_action(api, frame, tab, refresh, result, in_flight);
    bind_save_models_action(api, frame, tab, refresh, result, in_flight);
    bind_clear_action(api, frame, tab, refresh, result, in_flight);
}

pub(super) fn set_actions_enabled(tab: &CodexTab, enabled: bool) {
    tab.provider_image_generation.enable(enabled);
    tab.save_models_button.enable(enabled);
    tab.model_list.enable(enabled);
    tab.inject_button.enable(enabled);
    tab.clear_button.enable(enabled);
}

pub(super) fn refresh_image_generation(tab: &CodexTab, enabled: bool) {
    if !tab.provider_image_generation.has_focus() {
        tab.provider_image_generation.set_value(enabled);
    }
}

pub(super) fn initialize_visible_model_checks(tab: &CodexTab, gateway_config: &AiGatewayConfig) {
    if tab.models_initialized.get() {
        return;
    }
    let selected = gateway_config
        .codex_visible_models
        .iter()
        .map(|model| model.as_str())
        .collect::<std::collections::HashSet<_>>();
    for (index, slug) in tab.model_slugs.iter().enumerate() {
        tab.model_list
            .check(index as u32, selected.contains(slug.as_str()));
    }
    tab.models_initialized.set(true);
}

pub(super) fn apply_pending_action(
    api: &ApiClient,
    tab: &CodexTab,
    text: GuiText,
    frame: &Frame,
    refresh: &DashboardRefresh,
    result: &CodexActionResultStore,
) -> bool {
    let result = result.lock().ok().and_then(|mut slot| slot.take());
    let Some(result) = result else {
        return false;
    };

    tab.inject_button.set_label(text.inject_codex_access());
    tab.clear_button.set_label(text.clear_codex_access());
    tab.save_models_button.set_label(text.save_codex_models());
    tab.inject_button.enable(true);
    tab.clear_button.enable(true);
    tab.save_models_button.enable(true);

    match result {
        CodexActionResult::Inject(Ok(_)) => {
            show_info(frame, text.codex_app_config_injected());
            force_dashboard_refresh(api, refresh);
        }
        CodexActionResult::Inject(Err(err)) => {
            show_error(frame, &err);
            force_dashboard_refresh(api, refresh);
        }
        CodexActionResult::Clear(Ok(_)) => {
            show_info(frame, text.codex_app_config_uninstalled());
            force_dashboard_refresh(api, refresh);
        }
        CodexActionResult::Clear(Err(err)) => {
            show_error(frame, &err);
            force_dashboard_refresh(api, refresh);
        }
        CodexActionResult::SaveModels(Ok(())) => {
            show_info(frame, text.codex_models_saved());
            force_dashboard_refresh(api, refresh);
        }
        CodexActionResult::SaveModels(Err(err)) => {
            show_error(frame, &err);
            force_dashboard_refresh(api, refresh);
        }
    }
    true
}

pub(super) enum CodexActionResult {
    Inject(Result<serde_json::Value, String>),
    Clear(Result<serde_json::Value, String>),
    SaveModels(Result<(), String>),
}

fn bind_inject_action(
    api: &ApiClient,
    frame: &Frame,
    tab: &CodexTab,
    refresh: &DashboardRefresh,
    result: &CodexActionResultStore,
    in_flight: &Arc<AtomicBool>,
) {
    let api = api.clone();
    let refresh = refresh.clone();
    let frame = *frame;
    let tab = tab.clone();
    let result = result.clone();
    let in_flight = in_flight.clone();
    let inject_button = tab.inject_button;
    inject_button.on_click(move |_| {
        if in_flight.swap(true, Ordering::SeqCst) {
            return;
        }
        if !ensure_service_ready_for_action(&api, &frame, &refresh) {
            in_flight.store(false, Ordering::SeqCst);
            return;
        }
        tab.inject_button
            .set_label(tab.text.injecting_codex_access());
        tab.inject_button.enable(false);
        let request = ConfigureRequest {
            provider_name: None,
            provider_base_url: None,
            provider_key: None,
            activate: true,
            image_generation_enabled: Some(tab.provider_image_generation.get_value()),
            supports_websockets: false,
        };
        let thread_api = api.clone();
        let result = result.clone();
        let in_flight = in_flight.clone();
        thread::spawn(move || {
            let outcome = thread_api.configure_codex_app(&request);
            if let Ok(mut slot) = result.lock() {
                slot.replace(CodexActionResult::Inject(outcome));
            }
            in_flight.store(false, Ordering::SeqCst);
        });
        schedule_dashboard_refresh(&api, &refresh);
    });
}

fn bind_save_models_action(
    api: &ApiClient,
    frame: &Frame,
    tab: &CodexTab,
    refresh: &DashboardRefresh,
    result: &CodexActionResultStore,
    in_flight: &Arc<AtomicBool>,
) {
    let api = api.clone();
    let refresh = refresh.clone();
    let frame = *frame;
    let tab = tab.clone();
    let result = result.clone();
    let in_flight = in_flight.clone();
    let save_models_button = tab.save_models_button;
    save_models_button.on_click(move |_| {
        if in_flight.swap(true, Ordering::SeqCst) {
            return;
        }
        if !ensure_service_ready_for_action(&api, &frame, &refresh) {
            in_flight.store(false, Ordering::SeqCst);
            return;
        }
        tab.save_models_button
            .set_label(tab.text.saving_codex_models());
        tab.save_models_button.enable(false);
        let selected_models = selected_visible_models(&tab);
        let thread_api = api.clone();
        let result = result.clone();
        let in_flight = in_flight.clone();
        thread::spawn(move || {
            let outcome = save_visible_models(&thread_api, selected_models);
            if let Ok(mut slot) = result.lock() {
                slot.replace(CodexActionResult::SaveModels(outcome));
            }
            in_flight.store(false, Ordering::SeqCst);
        });
        schedule_dashboard_refresh(&api, &refresh);
    });
}

fn bind_clear_action(
    api: &ApiClient,
    frame: &Frame,
    tab: &CodexTab,
    refresh: &DashboardRefresh,
    result: &CodexActionResultStore,
    in_flight: &Arc<AtomicBool>,
) {
    let api = api.clone();
    let refresh = refresh.clone();
    let frame = *frame;
    let tab = tab.clone();
    let result = result.clone();
    let in_flight = in_flight.clone();
    let clear_button = tab.clear_button;
    clear_button.on_click(move |_| {
        if in_flight.swap(true, Ordering::SeqCst) {
            return;
        }
        if !ensure_service_ready_for_action(&api, &frame, &refresh) {
            in_flight.store(false, Ordering::SeqCst);
            return;
        }
        if !confirm_uninstall_codex_app_config(&frame, tab.text) {
            in_flight.store(false, Ordering::SeqCst);
            return;
        }
        tab.clear_button.set_label(tab.text.clearing_codex_access());
        tab.clear_button.enable(false);
        let thread_api = api.clone();
        let result = result.clone();
        let in_flight = in_flight.clone();
        thread::spawn(move || {
            let outcome = thread_api.uninstall_codex_app();
            if let Ok(mut slot) = result.lock() {
                slot.replace(CodexActionResult::Clear(outcome));
            }
            in_flight.store(false, Ordering::SeqCst);
        });
        schedule_dashboard_refresh(&api, &refresh);
    });
}

fn selected_visible_models(tab: &CodexTab) -> Vec<String> {
    tab.model_slugs
        .iter()
        .enumerate()
        .filter_map(|(index, slug)| {
            tab.model_list
                .is_checked(index as u32)
                .then(|| slug.clone())
        })
        .collect()
}

fn save_visible_models(api: &ApiClient, models: Vec<String>) -> Result<(), String> {
    let mut config = api.get_app_config()?;
    config.ai_gateway.codex_visible_models = models;
    api.save_app_config(&config)?;
    Ok(())
}
