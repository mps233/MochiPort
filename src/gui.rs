use std::{
    cell::RefCell,
    env,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use image::imageops::FilterType;
use qrcode::{Color, QrCode};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use wxdragon::widgets::scrolled_window::ScrollBarConfig;
use wxdragon::{prelude::*, timer::Timer};

#[cfg(target_os = "windows")]
const DEFAULT_BASE_URL: &str = "http://127.0.0.1:3847";
#[cfg(not(target_os = "windows"))]
const DEFAULT_BASE_URL: &str = "http://127.0.0.1:3847";
const DEFAULT_PROVIDER_NAME: &str = "ai-codex";
const CODEX_APP_GUI_UNSUPPORTED: bool = !(cfg!(target_os = "macos") || cfg!(target_os = "windows"));
const PROJECT_HOME_URL: &str = "https://github.com/happy-loki/codex-remote";
const UPDATE_MANIFEST_URL: &str =
    "https://github.com/happy-loki/codex-remote/releases/latest/download/latest.json";
const UPDATE_RELEASE_API_URL: &str =
    "https://api.github.com/repos/happy-loki/codex-remote/releases/latest";
const UPDATE_RELEASE_PAGE_URL: &str = "https://github.com/happy-loki/codex-remote/releases/latest";
const DASHBOARD_REFRESH_INTERVAL_MS: i32 = 2500;
const DASHBOARD_RESULT_POLL_MS: i32 = 100;
const GUI_CONNECT_TIMEOUT: Duration = Duration::from_millis(250);
const GUI_STATUS_TIMEOUT: Duration = Duration::from_millis(650);
const GUI_ACTION_TIMEOUT: Duration = Duration::from_secs(2);
const GUI_CONFIG_TIMEOUT: Duration = Duration::from_secs(15);
const UPDATE_CHECK_TIMEOUT: Duration = Duration::from_secs(8);
const ID_MENU_CLOSE_WINDOW: i32 = 10_001;
const ID_MENU_MINIMIZE: i32 = 10_002;
const ID_MENU_CHECK_UPDATE: i32 = 10_003;

type FrameTimerStore = Rc<RefCell<Option<Timer<Frame>>>>;
type ConfigActionResultStore = Arc<Mutex<Option<ConfigActionResult>>>;

#[derive(Clone)]
struct GuiTimers {
    stores: Rc<RefCell<Vec<FrameTimerStore>>>,
}

impl GuiTimers {
    fn new() -> Self {
        Self {
            stores: Rc::new(RefCell::new(Vec::new())),
        }
    }

    fn track(&self, store: &FrameTimerStore) {
        self.stores.borrow_mut().push(store.clone());
    }

    fn stop_all(&self) {
        let stores = self.stores.borrow().clone();
        for store in stores {
            if let Some(timer) = store.borrow().as_ref() {
                timer.stop();
            }
        }
        self.stores.borrow_mut().clear();
    }
}

pub fn run() {
    if let Err(err) = wxdragon::main(|_| build_ui()) {
        eprintln!("failed to start Codex Remote GUI: {err:?}");
    }
}

fn build_ui() {
    let api = ApiClient::new(default_base_url());
    let gui_timers = GuiTimers::new();

    let frame = Frame::builder()
        .with_title("Codex Remote")
        .with_size(Size::new(1100, 760))
        .build();
    frame.set_icon(&app_icon_bitmap(48));
    install_system_menu(&frame, &gui_timers);
    frame.set_background_color(Colour::rgb(246, 247, 250));
    let _status_bar = StatusBar::builder(&frame)
        .with_fields_count(1)
        .with_status_widths(vec![-1])
        .add_initial_text(0, &format!("版本 {}", env!("CARGO_PKG_VERSION")))
        .build();

    let root = Panel::builder(&frame).build();
    root.set_background_color(Colour::rgb(246, 247, 250));

    let root_sizer = BoxSizer::builder(Orientation::Vertical).build();

    let status_box = StaticBox::builder(&root).with_label("状态概览").build();
    let status_section =
        StaticBoxSizerBuilder::new_with_box(&status_box, Orientation::Vertical).build();
    let status_row = BoxSizer::builder(Orientation::Horizontal).build();
    let codex_status = status_panel(&status_box, "Codex App 控制通道", StatusIconKind::Codex);
    let vscode_status = status_panel(&status_box, "VS Code 插件", StatusIconKind::VsCodeCodex);
    if CODEX_APP_GUI_UNSUPPORTED {
        set_disabled_status_panel(&codex_status, "暂不可用", "当前平台暂不支持 App GUI");
    }
    let service_status = status_panel(&status_box, "本地服务", StatusIconKind::Service);
    let feishu_status = status_panel(&status_box, "飞书", StatusIconKind::Feishu);
    let entry_connector = topology_connector(&status_box);
    let bridge_connector = topology_arrow(&status_box);
    let entry_column = BoxSizer::builder(Orientation::Vertical).build();
    entry_column.add(
        &codex_status.panel,
        1,
        SizerFlag::Expand | SizerFlag::Bottom,
        8,
    );
    entry_column.add(&vscode_status.panel, 1, SizerFlag::Expand, 0);
    status_row.add_sizer(&entry_column, 1, SizerFlag::Expand | SizerFlag::All, 8);
    status_row.add(
        &entry_connector,
        0,
        SizerFlag::AlignCenterVertical | SizerFlag::Left | SizerFlag::Right,
        4,
    );
    status_row.add(
        &service_status.panel,
        1,
        SizerFlag::Expand | SizerFlag::All,
        8,
    );
    status_row.add(
        &bridge_connector,
        0,
        SizerFlag::AlignCenterVertical | SizerFlag::Left | SizerFlag::Right,
        2,
    );
    status_row.add(
        &feishu_status.panel,
        1,
        SizerFlag::Expand | SizerFlag::All,
        8,
    );
    status_section.add_sizer(
        &status_row,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top | SizerFlag::Bottom,
        8,
    );
    root_sizer.add_sizer(
        &status_section,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        14,
    );

    let notebook = Notebook::builder(&root).build();

    let codex_page = ScrolledWindow::builder(&notebook)
        .with_style(ScrolledWindowStyle::VScroll)
        .build();
    codex_page.set_background_color(Colour::rgb(250, 251, 253));
    let codex_sizer = BoxSizer::builder(Orientation::Vertical).build();

    let config_static_box = StaticBox::builder(&codex_page)
        .with_label("Provider 管理")
        .build();
    let config_box =
        StaticBoxSizerBuilder::new_with_box(&config_static_box, Orientation::Vertical).build();
    let config_hint = StaticText::builder(&config_static_box)
        .with_label("选择或填写第三方模型服务，然后写入 Codex App。")
        .build();
    config_hint.set_foreground_color(Colour::rgb(34, 39, 47));
    config_hint.wrap(760);
    config_box.add(
        &config_hint,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top | SizerFlag::Bottom,
        12,
    );

    let new_provider_button = Button::builder(&config_static_box)
        .with_label("新增")
        .build();
    new_provider_button.set_tooltip("清空表单，新增一个 provider");
    let save_provider_button = Button::builder(&config_static_box)
        .with_label("保存")
        .build();
    save_provider_button.set_tooltip("保存或更新当前表单里的 provider");
    let delete_provider_button = Button::builder(&config_static_box)
        .with_label("删除")
        .build();
    delete_provider_button.set_tooltip("删除当前选中的 provider");
    let configure_button = Button::builder(&config_static_box)
        .with_label("启用")
        .build();
    configure_button.set_tooltip("保存并使用这个模型服务");

    let provider_catalog = StaticText::builder(&config_static_box)
        .with_label("正在匹配 ~/.codex/config.toml 里的 provider")
        .build();
    provider_catalog.set_foreground_color(Colour::rgb(103, 111, 124));
    provider_catalog.wrap(980);
    config_box.add(
        &provider_catalog,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
        8,
    );

    let provider_list = ListCtrl::builder(&config_static_box)
        .with_style(ListCtrlStyle::Report | ListCtrlStyle::SingleSel | ListCtrlStyle::HRules)
        .with_size(Size::new(-1, 142))
        .build();
    provider_list.insert_column(0, "名称", ListColumnFormat::Left, 160);
    provider_list.insert_column(1, "Base URL", ListColumnFormat::Left, 420);
    provider_list.insert_column(2, "当前", ListColumnFormat::Left, 90);
    provider_list.insert_column(3, "API Key", ListColumnFormat::Left, 160);
    config_box.add(
        &provider_list,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
        10,
    );

    let provider_actions = BoxSizer::builder(Orientation::Horizontal).build();
    provider_actions.add_stretch_spacer(1);
    provider_actions.add(&new_provider_button, 0, SizerFlag::Right, 8);
    provider_actions.add(&delete_provider_button, 0, SizerFlag::Right, 8);
    provider_actions.add(&configure_button, 0, SizerFlag::Right, 0);
    config_box.add_sizer(
        &provider_actions,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
        10,
    );

    let provider_help = StaticText::builder(&config_static_box)
        .with_label("API Key 已保存时会用星号显示；需要更换时直接输入新 key。")
        .build();
    provider_help.set_foreground_color(Colour::rgb(91, 100, 114));
    provider_help.wrap(980);
    config_box.add(
        &provider_help,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
        8,
    );

    let form = FlexGridSizer::builder(0, 2)
        .with_gap(Size::new(12, 10))
        .build();
    form.add_growable_col(1, 1);
    let provider_name = provider_combo_row(
        &config_static_box,
        &form,
        "Provider 名称",
        DEFAULT_PROVIDER_NAME,
    );
    let provider_base_url = text_field_row(&config_static_box, &form, "Base URL", "");
    let provider_key = text_field_row(&config_static_box, &form, "API Key", "");
    config_box.add_sizer(
        &form,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
        8,
    );
    let save_actions = BoxSizer::builder(Orientation::Horizontal).build();
    save_actions.add_stretch_spacer(1);
    save_actions.add(&save_provider_button, 0, SizerFlag::Right, 0);
    config_box.add_sizer(
        &save_actions,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
        12,
    );
    codex_sizer.add_sizer(
        &config_box,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        10,
    );

    let uninstall_button = Button::builder(&codex_page)
        .with_label("清除 Codex 接入")
        .build();
    uninstall_button.set_tooltip("移除本工具写入的 Codex App 本地接入配置");
    let codex_maintenance_actions = BoxSizer::builder(Orientation::Horizontal).build();
    codex_maintenance_actions.add_stretch_spacer(1);
    codex_maintenance_actions.add(&uninstall_button, 0, SizerFlag::Right, 0);
    codex_sizer.add_stretch_spacer(1);
    codex_sizer.add_sizer(
        &codex_maintenance_actions,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
        20,
    );
    codex_page.set_sizer(codex_sizer, true);
    codex_page.set_scroll_rate(10, 10);
    let codex_best_size = codex_page.get_best_size();
    codex_page.set_scrollbars(ScrollBarConfig {
        pixels_per_unit_x: 10,
        pixels_per_unit_y: 10,
        no_units_x: (codex_best_size.width + 20).max(1) / 10,
        no_units_y: (codex_best_size.height + 80).max(1) / 10,
        x_pos: 0,
        y_pos: 0,
        no_refresh: true,
    });

    let feishu_page = Panel::builder(&notebook)
        .with_style(PanelStyle::TabTraversal)
        .build();
    feishu_page.set_background_color(Colour::rgb(250, 251, 253));
    let feishu_sizer = BoxSizer::builder(Orientation::Vertical).build();

    let feishu_static_box = StaticBox::builder(&feishu_page)
        .with_label("飞书机器人")
        .build();
    let feishu_box =
        StaticBoxSizerBuilder::new_with_box(&feishu_static_box, Orientation::Vertical).build();
    let feishu_state = StaticText::builder(&feishu_static_box)
        .with_label("检测中")
        .build();
    feishu_state.set_foreground_color(Colour::rgb(73, 83, 96));
    feishu_box.add(
        &feishu_state,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        12,
    );

    let feishu_detail = StaticText::builder(&feishu_static_box)
        .with_label("正在读取飞书接入状态")
        .build();
    feishu_detail.set_foreground_color(Colour::rgb(82, 91, 105));
    feishu_detail.wrap(760);
    feishu_box.add(
        &feishu_detail,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        12,
    );

    let divider = StaticLine::builder(&feishu_static_box).build();
    feishu_box.add(
        &divider,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        14,
    );

    let feishu_meta = StaticText::builder(&feishu_static_box)
        .with_label("")
        .build();
    feishu_meta.set_foreground_color(Colour::rgb(103, 111, 124));
    feishu_meta.wrap(760);
    feishu_box.add(
        &feishu_meta,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        12,
    );

    let feishu_buttons = BoxSizer::builder(Orientation::Horizontal).build();
    feishu_buttons.add_stretch_spacer(1);
    let stop_bridge_button = Button::builder(&feishu_static_box)
        .with_label("断开接入")
        .build();
    stop_bridge_button.set_tooltip("暂停飞书桥接，不删除已保存的机器人配置");
    let change_bot_button = Button::builder(&feishu_static_box)
        .with_label("扫码使用新机器人")
        .build();
    change_bot_button.set_tooltip("扫码接入一个新的飞书机器人");
    feishu_buttons.add(&stop_bridge_button, 0, SizerFlag::Right, 8);
    feishu_buttons.add(&change_bot_button, 0, SizerFlag::Right, 0);
    feishu_box.add_sizer(&feishu_buttons, 0, SizerFlag::Expand | SizerFlag::All, 12);
    feishu_sizer.add_sizer(
        &feishu_box,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        14,
    );
    feishu_sizer.add_stretch_spacer(1);
    feishu_page.set_sizer(feishu_sizer, true);

    notebook.add_page(&codex_page, "Codex 接入", true, None);
    notebook.add_page(&feishu_page, "飞书", false, None);

    root_sizer.add(
        &notebook,
        1,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top | SizerFlag::Bottom,
        8,
    );

    root.set_sizer(root_sizer, true);
    let frame_sizer = BoxSizer::builder(Orientation::Vertical).build();
    frame_sizer.add(&root, 1, SizerFlag::Expand, 0);
    frame.set_sizer(frame_sizer, true);

    let handles = UiHandles {
        service_status,
        feishu_status,
        codex_status,
        vscode_status,
        feishu_state,
        feishu_detail,
        feishu_meta,
        change_bot_button,
        stop_bridge_button,
        uninstall_button,
        new_provider_button,
        save_provider_button,
        delete_provider_button,
        configure_button,
        provider_name,
        provider_base_url,
        provider_key,
        provider_list,
        provider_catalog,
    };

    let daemon_child: Rc<RefCell<Option<Child>>> = Rc::new(RefCell::new(None));
    let dashboard_refresh = DashboardRefresh::new();
    let config_action_result: ConfigActionResultStore = Arc::new(Mutex::new(None));
    let config_action_in_flight = Arc::new(AtomicBool::new(false));
    show_dashboard_starting(&handles);
    show_local_codex_app_config_preview(&handles, &api, &dashboard_refresh);

    {
        let handles = handles;
        new_provider_button.on_click(move |_| {
            clear_provider_list_selection(&handles.provider_list);
            set_combo_value_if_changed(&handles.provider_name, "");
            change_text_value_if_changed(&handles.provider_base_url, "");
            change_text_value_if_changed(&handles.provider_key, "");
            handles
                .provider_catalog
                .set_label("填写新 provider 名称、Base URL 和 API Key，然后点击启用。");
            handles.provider_catalog.wrap(980);
            handles.provider_catalog.layout();
        });
    }

    {
        let api = api.clone();
        let dashboard_refresh = dashboard_refresh.clone();
        let provider_name = provider_name;
        let provider_base_url = provider_base_url;
        let provider_key = provider_key;
        let frame = frame;
        let handles = handles;
        let config_action_result = config_action_result.clone();
        let config_action_in_flight = config_action_in_flight.clone();
        save_provider_button.on_click(move |_| {
            if config_action_in_flight.swap(true, Ordering::SeqCst) {
                return;
            }
            handles
                .provider_catalog
                .set_label("正在保存 provider，请稍候...");
            handles.provider_catalog.wrap(980);
            handles.save_provider_button.set_label("保存中...");
            set_actions_enabled(&handles, false);
            frame.refresh(true, None);
            frame.update();

            let (selected_provider, request) = provider_config_request_from_ui(
                &handles,
                &provider_name,
                &provider_base_url,
                &provider_key,
                cached_dashboard_snapshot(&dashboard_refresh).as_ref(),
                false,
            );
            let api = api.clone();
            let config_action_result = config_action_result.clone();
            let config_action_in_flight = config_action_in_flight.clone();
            thread::spawn(move || {
                let outcome = save_codex_provider_and_verify(&api, &request, &selected_provider);
                if let Ok(mut slot) = config_action_result.lock() {
                    slot.replace(ConfigActionResult::Save {
                        provider_name: selected_provider,
                        result: outcome,
                    });
                }
                config_action_in_flight.store(false, Ordering::SeqCst);
            });
        });
    }

    {
        let api = api.clone();
        let dashboard_refresh = dashboard_refresh.clone();
        let provider_name = provider_name;
        let frame = frame;
        let handles = handles;
        let config_action_result = config_action_result.clone();
        let config_action_in_flight = config_action_in_flight.clone();
        delete_provider_button.on_click(move |_| {
            if config_action_in_flight.swap(true, Ordering::SeqCst) {
                return;
            }
            let provider_name = provider_name_from_ui(
                &handles,
                &provider_name,
                cached_dashboard_snapshot(&dashboard_refresh).as_ref(),
            );
            if provider_name.trim().is_empty() {
                config_action_in_flight.store(false, Ordering::SeqCst);
                show_error(&frame, "请先选择或填写要删除的 provider。");
                return;
            }
            if !confirm_delete_provider(&frame, &provider_name) {
                config_action_in_flight.store(false, Ordering::SeqCst);
                return;
            }

            handles
                .provider_catalog
                .set_label("正在删除 provider，请稍候...");
            handles.provider_catalog.wrap(980);
            handles.delete_provider_button.set_label("删除中...");
            set_actions_enabled(&handles, false);
            frame.refresh(true, None);
            frame.update();

            let request = DeleteProviderRequest { provider_name };
            let api = api.clone();
            let config_action_result = config_action_result.clone();
            let config_action_in_flight = config_action_in_flight.clone();
            thread::spawn(move || {
                let outcome = delete_codex_provider_and_verify(&api, &request);
                if let Ok(mut slot) = config_action_result.lock() {
                    slot.replace(ConfigActionResult::Delete(outcome));
                }
                config_action_in_flight.store(false, Ordering::SeqCst);
            });
        });
    }

    {
        let api = api.clone();
        let dashboard_refresh = dashboard_refresh.clone();
        let provider_name = provider_name;
        let provider_base_url = provider_base_url;
        let provider_key = provider_key;
        let frame = frame;
        let handles = handles;
        let config_action_result = config_action_result.clone();
        let config_action_in_flight = config_action_in_flight.clone();
        configure_button.on_click(move |_| {
            if config_action_in_flight.swap(true, Ordering::SeqCst) {
                return;
            }
            handles.provider_catalog.set_label("正在启用，请稍候...");
            handles.provider_catalog.wrap(980);
            handles.configure_button.set_label("启用中...");
            set_actions_enabled(&handles, false);
            frame.refresh(true, None);
            frame.update();

            let (selected_provider, request) = provider_config_request_from_ui(
                &handles,
                &provider_name,
                &provider_base_url,
                &provider_key,
                cached_dashboard_snapshot(&dashboard_refresh).as_ref(),
                true,
            );
            let api = api.clone();
            let config_action_result = config_action_result.clone();
            let config_action_in_flight = config_action_in_flight.clone();
            thread::spawn(move || {
                let outcome = configure_codex_app_and_verify(&api, &request, &selected_provider);
                if let Ok(mut slot) = config_action_result.lock() {
                    slot.replace(ConfigActionResult::Configure {
                        provider_name: selected_provider,
                        result: outcome,
                    });
                }
                config_action_in_flight.store(false, Ordering::SeqCst);
            });
        });
    }

    {
        let api = api.clone();
        let dashboard_refresh = dashboard_refresh.clone();
        let frame = frame;
        let handles = handles;
        uninstall_button.on_click(move |_| {
            if !confirm_uninstall_codex_app_config(&frame) {
                return;
            }

            match api.uninstall_codex_app() {
                Ok(_) => {
                    show_info(
                        &frame,
                        "Codex App 本地接入配置已卸载。请重启 Codex App 以恢复官方连接。",
                    );
                    show_local_codex_app_config_preview(&handles, &api, &dashboard_refresh);
                    schedule_dashboard_refresh(&api, &dashboard_refresh);
                }
                Err(err) => show_error(&frame, &err),
            }
        });
    }

    {
        let handles = handles;
        let dashboard_refresh = dashboard_refresh.clone();
        provider_name.on_selection_changed(move |_| {
            let selected = clean_provider_text(&provider_name.get_value());
            let Some(snapshot) = cached_dashboard_snapshot(&dashboard_refresh) else {
                return;
            };
            if let Some(provider) = find_provider(&snapshot, &selected) {
                apply_provider_to_form(&handles, &provider, true);
            }
        });
    }

    {
        let handles = handles;
        let dashboard_refresh = dashboard_refresh.clone();
        provider_list.on_item_selected(move |event| {
            let index = event.get_item_index();
            if index < 0 {
                return;
            }
            if let Some(snapshot) = cached_dashboard_snapshot(&dashboard_refresh) {
                if let Some(provider) = provider_from_list_row(&snapshot, index as i64) {
                    apply_provider_to_form(&handles, &provider, true);
                    return;
                }
            }
            apply_provider_row_to_form(&handles, &provider_list, index as i64);
        });
    }

    {
        let api = api.clone();
        let dashboard_refresh = dashboard_refresh.clone();
        let frame = frame;
        stop_bridge_button.on_click(move |_| {
            let snapshot = cached_dashboard_snapshot(&dashboard_refresh);
            let feishu_configured = snapshot.as_ref().is_some_and(feishu_configured);
            let bridge_enabled = snapshot.as_ref().is_some_and(bridge_enabled);
            if !feishu_configured {
                show_error(&frame, "请先接入飞书机器人。");
                return;
            }

            let result = if bridge_enabled {
                api.stop_bridge()
            } else {
                api.start_bridge()
            };
            match result {
                Ok(_) => {
                    if bridge_enabled {
                        show_info(&frame, "飞书接入已断开。");
                    } else {
                        show_info(&frame, "飞书接入已恢复。");
                    }
                    schedule_dashboard_refresh(&api, &dashboard_refresh);
                }
                Err(err) => show_error(&frame, &err),
            }
        });
    }

    {
        let api = api.clone();
        let dashboard_refresh = dashboard_refresh.clone();
        let frame = frame;
        change_bot_button.on_click(move |_| {
            show_onboard_dialog(&frame, api.clone());
            schedule_dashboard_refresh(&api, &dashboard_refresh);
        });
    }

    let result_timer_store: FrameTimerStore = Rc::new(RefCell::new(None));
    let result_timer = Timer::new(&frame);
    {
        let handles = handles;
        let dashboard_refresh = dashboard_refresh.clone();
        result_timer.on_tick(move |_| {
            apply_pending_dashboard(&handles, &dashboard_refresh);
        });
    }
    result_timer.start(DASHBOARD_RESULT_POLL_MS, false);
    result_timer_store.borrow_mut().replace(result_timer);
    gui_timers.track(&result_timer_store);

    let config_action_timer_store: FrameTimerStore = Rc::new(RefCell::new(None));
    let config_action_timer = Timer::new(&frame);
    {
        let api = api.clone();
        let handles = handles;
        let frame = frame;
        let dashboard_refresh = dashboard_refresh.clone();
        let config_action_result = config_action_result.clone();
        config_action_timer.on_tick(move |_| {
            apply_pending_config_action(
                &api,
                &handles,
                &frame,
                &dashboard_refresh,
                &config_action_result,
            );
        });
    }
    config_action_timer.start(DASHBOARD_RESULT_POLL_MS, false);
    config_action_timer_store
        .borrow_mut()
        .replace(config_action_timer);
    gui_timers.track(&config_action_timer_store);

    let timer_store: FrameTimerStore = Rc::new(RefCell::new(None));
    let timer = Timer::new(&frame);
    {
        let api = api.clone();
        let dashboard_refresh = dashboard_refresh.clone();
        timer.on_tick(move |_| {
            schedule_dashboard_refresh(&api, &dashboard_refresh);
        });
    }
    timer.start(DASHBOARD_REFRESH_INTERVAL_MS, false);
    timer_store.borrow_mut().replace(timer);
    gui_timers.track(&timer_store);

    start_daemon_for_gui_async(
        &api,
        &handles,
        &frame,
        &daemon_child,
        &dashboard_refresh,
        &gui_timers,
    );

    {
        let api = api.clone();
        let daemon_child = daemon_child.clone();
        let dashboard_refresh = dashboard_refresh.clone();
        let gui_timers = gui_timers.clone();
        let frame = frame;
        frame.on_close(move |_| {
            dashboard_refresh.closing.store(true, Ordering::SeqCst);
            gui_timers.stop_all();
            stop_pending_startup_daemon(&dashboard_refresh);
            stop_daemon_on_exit(&api, &daemon_child);
            frame.destroy();
        });
    }

    frame.centre();
    frame.show(true);
}

fn default_base_url() -> String {
    std::env::var("CODEX_REMOTE_GUI_BASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
}

fn install_system_menu(frame: &Frame, gui_timers: &GuiTimers) {
    let file_menu = Menu::builder()
        .append_item(
            ID_MENU_CLOSE_WINDOW,
            "&Close Window\tCtrl+W",
            "Close this window",
        )
        .append_item(
            ID_MENU_MINIMIZE,
            "Mi&nimize\tCtrl+M",
            "Minimize this window",
        )
        .append_separator()
        .append_item(ID_EXIT, "&Quit Codex Remote\tCtrl+Q", "Quit Codex Remote")
        .build();
    let help_menu = Menu::builder()
        .append_item(
            ID_MENU_CHECK_UPDATE,
            "&Check for Updates",
            "Check GitHub Releases for a newer Codex Remote version",
        )
        .append_separator()
        .append_item(ID_ABOUT, "&About Codex Remote", "About Codex Remote")
        .build();
    let menu_bar = MenuBar::builder()
        .append(file_menu, "&File")
        .append(help_menu, "&Help")
        .build();
    frame.set_menu_bar(menu_bar);

    let frame = *frame;
    let gui_timers = gui_timers.clone();
    let update_check_in_flight = Arc::new(AtomicBool::new(false));
    frame.on_menu_selected(move |event| match event.get_id() {
        ID_EXIT | ID_MENU_CLOSE_WINDOW => frame.close(true),
        ID_MENU_MINIMIZE => frame.iconize(true),
        ID_MENU_CHECK_UPDATE => {
            check_for_updates_async(&frame, &gui_timers, &update_check_in_flight);
        }
        ID_ABOUT => show_about_dialog(&frame),
        _ => event.skip(true),
    });
}

#[derive(Debug)]
struct LatestReleaseInfo {
    version: String,
    release_url: String,
    notes: Option<String>,
}

#[derive(Debug)]
enum UpdateCheckOutcome {
    Newer {
        current_version: String,
        latest_version: String,
        release_url: String,
        notes: Option<String>,
    },
    Current {
        current_version: String,
        latest_version: String,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateManifest {
    version: String,
    #[serde(default, alias = "release_url", alias = "html_url")]
    release_url: Option<String>,
    #[serde(default)]
    notes: Option<String>,
}

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: String,
    body: Option<String>,
}

fn check_for_updates_async(frame: &Frame, gui_timers: &GuiTimers, in_flight: &Arc<AtomicBool>) {
    if in_flight.swap(true, Ordering::SeqCst) {
        show_info(frame, "正在检查更新，请稍候。");
        return;
    }

    let result: Arc<Mutex<Option<Result<UpdateCheckOutcome, String>>>> = Arc::new(Mutex::new(None));
    {
        let result = result.clone();
        thread::spawn(move || {
            let update = check_for_updates();
            if let Ok(mut slot) = result.lock() {
                slot.replace(update);
            }
        });
    }

    let update_timer_store: FrameTimerStore = Rc::new(RefCell::new(None));
    let update_timer = Timer::new(frame);
    {
        let frame = *frame;
        let in_flight = in_flight.clone();
        let update_timer_store = update_timer_store.clone();
        update_timer.on_tick(move |_| {
            let update = result.lock().ok().and_then(|mut slot| slot.take());
            let Some(update) = update else {
                return;
            };

            if let Some(timer) = update_timer_store.borrow().as_ref() {
                timer.stop();
            }
            in_flight.store(false, Ordering::SeqCst);
            show_update_check_result(&frame, update);
        });
    }
    update_timer.start(100, false);
    update_timer_store.borrow_mut().replace(update_timer);
    gui_timers.track(&update_timer_store);
}

fn check_for_updates() -> Result<UpdateCheckOutcome, String> {
    let client = Client::builder()
        .connect_timeout(UPDATE_CHECK_TIMEOUT)
        .timeout(UPDATE_CHECK_TIMEOUT)
        .build()
        .map_err(|err| format!("创建更新检查客户端失败：{err}"))?;

    let release = fetch_update_manifest(&client).or_else(|manifest_err| {
        fetch_github_latest_release(&client).map_err(|api_err| {
            format!(
                "无法读取 GitHub Release 更新信息：{api_err}\nlatest.json 检查结果：{manifest_err}"
            )
        })
    })?;
    build_update_check_outcome(release)
}

fn fetch_update_manifest(client: &Client) -> Result<LatestReleaseInfo, String> {
    let text = fetch_update_text(client, UPDATE_MANIFEST_URL)?;
    let manifest: UpdateManifest =
        serde_json::from_str(&text).map_err(|err| format!("latest.json 无法解析：{err}"))?;
    let release_url = manifest
        .release_url
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| UPDATE_RELEASE_PAGE_URL.to_string());
    Ok(LatestReleaseInfo {
        version: manifest.version,
        release_url,
        notes: manifest.notes,
    })
}

fn fetch_github_latest_release(client: &Client) -> Result<LatestReleaseInfo, String> {
    let text = fetch_update_text(client, UPDATE_RELEASE_API_URL)?;
    let release: GitHubRelease =
        serde_json::from_str(&text).map_err(|err| format!("GitHub Release API 无法解析：{err}"))?;
    Ok(LatestReleaseInfo {
        version: release.tag_name,
        release_url: release.html_url,
        notes: release.body,
    })
}

fn fetch_update_text(client: &Client, url: &str) -> Result<String, String> {
    let response = client
        .get(url)
        .header("User-Agent", "codex-remote")
        .header("Accept", "application/json")
        .send()
        .map_err(|err| {
            if err.is_timeout() {
                format!("{url} 请求超时：{err}")
            } else {
                format!("{url} 请求失败：{err}")
            }
        })?;
    let status = response.status();
    let text = response.text().map_err(|err| err.to_string())?;
    if status.is_success() {
        Ok(text)
    } else {
        Err(format!("{url} 返回 HTTP {status}: {text}"))
    }
}

fn build_update_check_outcome(release: LatestReleaseInfo) -> Result<UpdateCheckOutcome, String> {
    let current_version = env!("CARGO_PKG_VERSION").to_string();
    let latest_version = release.version.trim().to_string();
    if latest_version.is_empty() {
        return Err("GitHub Release 没有版本号。".to_string());
    }

    if is_version_newer(&latest_version, &current_version)? {
        Ok(UpdateCheckOutcome::Newer {
            current_version,
            latest_version,
            release_url: release.release_url,
            notes: release.notes,
        })
    } else {
        Ok(UpdateCheckOutcome::Current {
            current_version,
            latest_version,
        })
    }
}

fn show_update_check_result(parent: &Frame, result: Result<UpdateCheckOutcome, String>) {
    match result {
        Ok(UpdateCheckOutcome::Current {
            current_version,
            latest_version,
        }) => {
            show_info(
                parent,
                &format!(
                    "已是最新版本。\n当前版本：{current_version}\nGitHub 最新版本：{latest_version}"
                ),
            );
        }
        Ok(UpdateCheckOutcome::Newer {
            current_version,
            latest_version,
            release_url,
            notes,
        }) => {
            let notes = update_notes_for_dialog(notes.as_deref());
            let message = format!(
                "发现新版本。\n当前版本：{current_version}\n最新版本：{latest_version}\n\n{notes}\n\n是否打开 GitHub Releases 下载？"
            );
            if confirm_open_update_release(parent, &message) {
                if let Err(err) = open_url_in_browser(&release_url) {
                    show_error(parent, &err);
                }
            }
        }
        Err(err) => {
            show_error(parent, &format!("检查更新失败：{err}"));
        }
    }
}

fn update_notes_for_dialog(notes: Option<&str>) -> String {
    let notes = notes.unwrap_or_default().trim();
    if notes.is_empty() {
        return "Release 页面包含安装包和更新说明。".to_string();
    }
    format!("更新说明：\n{}", truncate_for_dialog(notes, 700))
}

fn truncate_for_dialog(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let mut result = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        result.push_str("\n...");
    }
    result
}

fn is_version_newer(latest: &str, current: &str) -> Result<bool, String> {
    let latest = parse_version_segments(latest)?;
    let current = parse_version_segments(current)?;
    for index in 0..latest.len().max(current.len()) {
        let latest_segment = latest.get(index).copied().unwrap_or_default();
        let current_segment = current.get(index).copied().unwrap_or_default();
        if latest_segment != current_segment {
            return Ok(latest_segment > current_segment);
        }
    }
    Ok(false)
}

fn parse_version_segments(version: &str) -> Result<Vec<u64>, String> {
    let normalized = version
        .trim()
        .trim_start_matches('v')
        .trim_start_matches('V')
        .split(['-', '+'])
        .next()
        .unwrap_or_default();
    let segments = normalized
        .split('.')
        .map(|segment| {
            segment
                .parse::<u64>()
                .map_err(|_| format!("版本号 {version} 无法比较。"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if segments.is_empty() {
        Err(format!("版本号 {version} 无法比较。"))
    } else {
        Ok(segments)
    }
}

#[cfg(test)]
mod update_tests {
    use super::*;

    #[test]
    fn compares_release_versions() {
        assert!(is_version_newer("v0.2.6", "0.2.5").unwrap());
        assert!(is_version_newer("0.3.0", "0.2.99").unwrap());
        assert!(!is_version_newer("v0.2.5", "0.2.5").unwrap());
        assert!(!is_version_newer("v0.2.4", "0.2.5").unwrap());
        assert!(!is_version_newer("v0.2.5-beta.1", "0.2.5").unwrap());
    }
}

fn open_url_in_browser(url: &str) -> Result<(), String> {
    let url = url.trim();
    if url.is_empty() {
        return Err("下载地址为空。".to_string());
    }

    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("cmd");
        command.args(["/C", "start", "", url]);
        command
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("open");
        command.arg(url);
        command
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut command = Command::new("xdg-open");
        command.arg(url);
        command
    };

    command
        .spawn()
        .map(|_| ())
        .map_err(|err| format!("无法打开浏览器：{err}\n下载地址：{url}"))
}

fn restart_daemon_for_gui(api: &ApiClient) -> Result<Child, String> {
    stop_existing_daemon(api);
    let mut child = spawn_daemon()?;
    for _ in 0..40 {
        thread::sleep(Duration::from_millis(250));
        if api.is_online() {
            return Ok(child);
        }
        if let Some(status) = child.try_wait().map_err(|err| err.to_string())? {
            return Err(format!("本地服务启动后退出：{status}"));
        }
    }

    let _ = child.kill();
    let _ = child.wait();
    Err("本地服务已启动，但 10 秒内没有响应。请检查 logs/codex-remote-chain.log。".to_string())
}

fn stop_existing_daemon(api: &ApiClient) {
    if api.is_online() {
        let _ = api.shutdown();
        wait_for_daemon_offline(api, 15);
    }
    stop_daemon_by_port(api);
    wait_for_daemon_offline(api, 15);
}

fn stop_managed_daemon(daemon_child: &Rc<RefCell<Option<Child>>>) {
    if let Some(mut child) = daemon_child.borrow_mut().take() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

struct StartupResult;

fn start_daemon_for_gui_async(
    api: &ApiClient,
    handles: &UiHandles,
    frame: &Frame,
    daemon_child: &Rc<RefCell<Option<Child>>>,
    dashboard_refresh: &DashboardRefresh,
    gui_timers: &GuiTimers,
) {
    if dashboard_refresh
        .daemon_starting
        .swap(true, Ordering::SeqCst)
    {
        return;
    }
    dashboard_refresh.generation.fetch_add(1, Ordering::SeqCst);
    if let Ok(mut result) = dashboard_refresh.result.lock() {
        result.take();
    }
    show_dashboard_starting(handles);

    let result: Arc<Mutex<Option<Result<StartupResult, String>>>> = Arc::new(Mutex::new(None));
    {
        let api = api.clone();
        let closing = dashboard_refresh.closing.clone();
        let pending_startup_child = dashboard_refresh.pending_startup_child.clone();
        let result = result.clone();
        thread::spawn(move || {
            let startup = match restart_daemon_for_gui(&api) {
                Ok(mut child) => {
                    let mut pending_child = pending_startup_child.lock().ok();
                    if closing.load(Ordering::SeqCst) {
                        wait_or_kill_child(&mut child, Duration::from_millis(250));
                    } else if let Some(slot) = pending_child.as_mut() {
                        slot.replace(child);
                    } else {
                        wait_or_kill_child(&mut child, Duration::from_millis(250));
                    }
                    Ok(StartupResult)
                }
                Err(err) => Err(err),
            };
            if let Ok(mut slot) = result.lock() {
                slot.replace(startup);
            }
        });
    }

    let startup_timer_store: FrameTimerStore = Rc::new(RefCell::new(None));
    let startup_timer = Timer::new(frame);
    {
        let api = api.clone();
        let handles = *handles;
        let daemon_child = daemon_child.clone();
        let dashboard_refresh = dashboard_refresh.clone();
        let startup_timer_store = startup_timer_store.clone();
        startup_timer.on_tick(move |_| {
            let startup = result.lock().ok().and_then(|mut slot| slot.take());
            let Some(startup) = startup else {
                return;
            };

            if let Some(timer) = startup_timer_store.borrow().as_ref() {
                timer.stop();
            }

            dashboard_refresh.generation.fetch_add(1, Ordering::SeqCst);
            if let Ok(mut result) = dashboard_refresh.result.lock() {
                result.take();
            }
            dashboard_refresh
                .daemon_starting
                .store(false, Ordering::SeqCst);
            match startup {
                Ok(_) if dashboard_refresh.closing.load(Ordering::SeqCst) => {
                    stop_pending_startup_daemon(&dashboard_refresh);
                }
                Ok(_) => {
                    if let Some(child) = take_pending_startup_daemon(&dashboard_refresh) {
                        replace_managed_daemon(&daemon_child, child);
                    }
                    repair_codex_app_gui_environment_async(&api, &dashboard_refresh);
                }
                Err(_) => {
                    set_actions_enabled(&handles, false);
                }
            }
            schedule_dashboard_refresh(&api, &dashboard_refresh);
        });
    }
    startup_timer.start(100, false);
    startup_timer_store.borrow_mut().replace(startup_timer);
    gui_timers.track(&startup_timer_store);
}

fn repair_codex_app_gui_environment_async(api: &ApiClient, dashboard_refresh: &DashboardRefresh) {
    let api = api.clone();
    let dashboard_refresh = dashboard_refresh.clone();
    thread::spawn(move || {
        let _ = api.repair_codex_app_gui_environment();
        schedule_dashboard_refresh(&api, &dashboard_refresh);
    });
}

fn stop_daemon_on_exit(api: &ApiClient, daemon_child: &Rc<RefCell<Option<Child>>>) {
    let child = daemon_child.borrow_mut().take();

    let _ = api.shutdown();
    if let Some(mut child) = child {
        kill_child(&mut child);
    }
}

fn stop_pending_startup_daemon(dashboard_refresh: &DashboardRefresh) {
    if let Some(mut child) = take_pending_startup_daemon(dashboard_refresh) {
        wait_or_kill_child(&mut child, Duration::from_millis(250));
    }
}

fn take_pending_startup_daemon(dashboard_refresh: &DashboardRefresh) -> Option<Child> {
    dashboard_refresh
        .pending_startup_child
        .lock()
        .ok()
        .and_then(|mut child| child.take())
}

fn wait_or_kill_child(child: &mut Child, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_)) => {
                let _ = child.wait();
                return;
            }
            Ok(None) => thread::sleep(Duration::from_millis(25)),
            Err(_) => return,
        }
    }

    if child.try_wait().ok().flatten().is_none() {
        let _ = child.kill();
    }
    let _ = child.wait();
}

fn kill_child(child: &mut Child) {
    if child.try_wait().ok().flatten().is_none() {
        let _ = child.kill();
    }
    let _ = child.wait();
}

fn wait_for_daemon_offline(api: &ApiClient, attempts: usize) {
    for _ in 0..attempts {
        thread::sleep(Duration::from_millis(100));
        if !api.is_online() {
            break;
        }
    }
}

fn replace_managed_daemon(daemon_child: &Rc<RefCell<Option<Child>>>, child: Child) {
    stop_managed_daemon(daemon_child);
    daemon_child.borrow_mut().replace(child);
}

#[cfg(unix)]
fn stop_daemon_by_port(api: &ApiClient) {
    let Some(port) = api.local_port() else {
        return;
    };
    let Ok(output) = Command::new("lsof")
        .arg("-nP")
        .arg("-iTCP")
        .arg(format!(":{port}"))
        .arg("-sTCP:LISTEN")
        .arg("-F")
        .arg("pc")
        .output()
    else {
        return;
    };
    let mut pid: Option<String> = None;
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if let Some(value) = line.strip_prefix('p') {
            pid = Some(value.to_string());
        } else if let Some(command) = line.strip_prefix('c')
            && command.contains("codex-remote")
        {
            if let Some(pid) = pid.take() {
                let _ = Command::new("kill").arg(pid).status();
            }
        } else if line.starts_with('c') {
            pid = None;
        }
    }
}

#[cfg(not(unix))]
fn stop_daemon_by_port(_api: &ApiClient) {}

fn spawn_daemon() -> Result<Child, String> {
    let mut command = daemon_command()?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| format!("无法启动本地服务：{err}"))
}

fn daemon_command() -> Result<Command, String> {
    let exe = std::env::current_exe().map_err(|err| format!("无法定位当前程序：{err}"))?;
    let mut command = Command::new(exe);
    append_daemon_args(&mut command);
    Ok(command)
}

fn append_daemon_args(command: &mut Command) {
    if let Some(config_path) = daemon_config_path() {
        command.arg("--config").arg(config_path);
    }
    command.arg("daemon");
}

fn daemon_config_path() -> Option<PathBuf> {
    if env::var_os("CODEX_REMOTE_USE_REPO_CONFIG").is_some() {
        return std::env::current_dir()
            .ok()
            .map(|cwd| cwd.join("config.toml"))
            .filter(|path| path.exists())
            .or_else(|| {
                repo_root_from_target_exe()
                    .map(|repo| repo.join("config.toml"))
                    .filter(|path| path.exists())
            });
    }
    Some(app_support_config_path())
}

fn app_support_config_path() -> PathBuf {
    let base = env::var_os("CODEX_REMOTE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join("Library/Application Support/Codex Remote"))
        })
        .or_else(|| env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("config.toml")
}

fn repo_root_from_target_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let profile_dir = exe.parent()?;
    let target_dir = profile_dir.parent()?;
    if target_dir.file_name().and_then(|value| value.to_str()) != Some("target") {
        return None;
    }
    let repo_root = target_dir.parent()?.to_path_buf();
    has_manifest(&repo_root).then_some(repo_root)
}

fn has_manifest(path: &Path) -> bool {
    path.join("Cargo.toml").exists()
}

#[derive(Clone)]
struct ApiClient {
    base_url: String,
    http: Client,
}

impl ApiClient {
    fn new(base_url: String) -> Self {
        let http = Client::builder()
            .connect_timeout(GUI_CONNECT_TIMEOUT)
            .timeout(GUI_ACTION_TIMEOUT)
            .build()
            .expect("build HTTP client");
        Self { base_url, http }
    }

    fn get_quick<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        let text = self.request_text(self.http.get(self.url(path)).timeout(GUI_STATUS_TIMEOUT))?;
        serde_json::from_str(&text).map_err(|err| format!("{path} 返回数据无法解析：{err}"))
    }

    fn get_with_timeout<T: DeserializeOwned>(
        &self,
        path: &str,
        timeout: Duration,
    ) -> Result<T, String> {
        let text = self.request_text(self.http.get(self.url(path)).timeout(timeout))?;
        serde_json::from_str(&text).map_err(|err| format!("{path} 返回数据无法解析：{err}"))
    }

    fn is_online(&self) -> bool {
        self.get_quick::<serde_json::Value>("/api/status").is_ok()
    }

    fn post_empty<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        self.post_empty_with_timeout(path, GUI_ACTION_TIMEOUT)
    }

    fn post_empty_with_timeout<T: DeserializeOwned>(
        &self,
        path: &str,
        timeout: Duration,
    ) -> Result<T, String> {
        let text = self.request_text(self.http.post(self.url(path)).timeout(timeout))?;
        serde_json::from_str(&text).map_err(|err| format!("{path} 返回数据无法解析：{err}"))
    }

    fn post_json<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, String> {
        self.post_json_with_timeout(path, body, GUI_ACTION_TIMEOUT)
    }

    fn post_json_with_timeout<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
        timeout: Duration,
    ) -> Result<T, String> {
        let text = self.request_text(self.http.post(self.url(path)).json(body).timeout(timeout))?;
        serde_json::from_str(&text).map_err(|err| format!("{path} 返回数据无法解析：{err}"))
    }

    fn request_text(&self, request: reqwest::blocking::RequestBuilder) -> Result<String, String> {
        let response = request.send().map_err(|err| {
            if err.is_timeout() {
                format!("本地服务 {} 响应超时：{err}", self.base_url)
            } else if err.is_connect() {
                format!("无法连接本地服务 {}：{err}", self.base_url)
            } else {
                format!("本地服务 {} 请求失败：{err}", self.base_url)
            }
        })?;
        let status = response.status();
        let text = response.text().map_err(|err| err.to_string())?;
        if status.is_success() {
            Ok(text)
        } else {
            Err(format!("HTTP {status}: {text}"))
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url.trim_end_matches('/'), path)
    }

    #[cfg(unix)]
    fn local_port(&self) -> Option<u16> {
        let url = reqwest::Url::parse(&self.base_url).ok()?;
        let host = url.host_str()?;
        matches!(host, "127.0.0.1" | "localhost" | "::1").then_some(url.port_or_known_default()?)
    }

    fn dashboard(&self) -> DashboardSnapshot {
        let status = match self.get_quick::<ServerStatus>("/api/status") {
            Ok(status) => status,
            Err(_err) => {
                return DashboardSnapshot {
                    service_online: false,
                    ..DashboardSnapshot::default()
                };
            }
        };

        let config = self.get_quick_optional_async::<AppConfig>("/api/config");
        let backend = self.get_quick_optional_async::<RemoteControlBackendStatus>(
            "/api/remote-control/backend-status",
        );
        let remote =
            self.get_quick_optional_async::<RemoteControlStatus>("/api/remote-control/status");
        let codex_app = self.get_quick_optional_async::<CodexAppStatus>("/api/codex-app/status");
        let feishu_bot = self.get_quick_optional_async::<FeishuBotStatus>("/api/feishu/bot");

        DashboardSnapshot {
            service_online: true,
            config: join_optional(config),
            backend: join_optional(backend),
            remote: join_optional(remote),
            codex_app: join_optional(codex_app),
            feishu_bot: join_optional(feishu_bot),
            status: Some(status),
        }
    }

    fn get_quick_optional_async<T>(&self, path: &'static str) -> thread::JoinHandle<Option<T>>
    where
        T: DeserializeOwned + Send + 'static,
    {
        let api = self.clone();
        thread::spawn(move || api.get_quick::<T>(path).ok())
    }

    fn configure_codex_app(&self, request: &ConfigureRequest) -> Result<serde_json::Value, String> {
        self.post_json_with_timeout("/api/codex-app/configure", request, GUI_CONFIG_TIMEOUT)
    }

    fn delete_codex_provider(
        &self,
        request: &DeleteProviderRequest,
    ) -> Result<serde_json::Value, String> {
        self.post_json_with_timeout(
            "/api/codex-app/provider/delete",
            request,
            GUI_CONFIG_TIMEOUT,
        )
    }

    fn codex_app_status(&self) -> Result<CodexAppStatus, String> {
        self.get_with_timeout("/api/codex-app/status", GUI_CONFIG_TIMEOUT)
    }

    fn uninstall_codex_app(&self) -> Result<serde_json::Value, String> {
        self.post_empty_with_timeout("/api/codex-app/uninstall", GUI_CONFIG_TIMEOUT)
    }

    fn repair_codex_app_gui_environment(&self) -> Result<serde_json::Value, String> {
        self.post_empty_with_timeout("/api/codex-app/repair-gui-environment", GUI_CONFIG_TIMEOUT)
    }

    fn stop_bridge(&self) -> Result<serde_json::Value, String> {
        self.post_empty("/api/bridge/stop")
    }

    fn start_bridge(&self) -> Result<serde_json::Value, String> {
        self.post_empty("/api/bridge/start")
    }

    fn shutdown(&self) -> Result<serde_json::Value, String> {
        self.post_empty("/api/shutdown")
    }

    fn start_feishu_onboard(&self) -> Result<FeishuOnboardStart, String> {
        self.post_empty("/api/feishu/onboard/start")
    }

    fn poll_feishu_onboard(&self, device_code: &str) -> Result<FeishuOnboardPoll, String> {
        self.post_json(
            "/api/feishu/onboard/poll",
            &serde_json::json!({ "deviceCode": device_code }),
        )
    }
}

fn join_optional<T>(handle: thread::JoinHandle<Option<T>>) -> Option<T> {
    handle.join().ok().flatten()
}

#[derive(Clone, Copy)]
struct StatusPanel {
    panel: Panel,
    icon: StaticBitmap,
    marker: StaticText,
    title: StaticText,
    state: StaticText,
    detail: StaticText,
    icon_kind: StatusIconKind,
}

#[derive(Clone, Copy)]
enum StatusIconKind {
    Service,
    Feishu,
    Codex,
    VsCodeCodex,
}

#[derive(Clone, Copy)]
struct UiHandles {
    service_status: StatusPanel,
    feishu_status: StatusPanel,
    codex_status: StatusPanel,
    vscode_status: StatusPanel,
    feishu_state: StaticText,
    feishu_detail: StaticText,
    feishu_meta: StaticText,
    change_bot_button: Button,
    stop_bridge_button: Button,
    uninstall_button: Button,
    new_provider_button: Button,
    save_provider_button: Button,
    delete_provider_button: Button,
    configure_button: Button,
    provider_name: ComboBox,
    provider_base_url: TextCtrl,
    provider_key: TextCtrl,
    provider_list: ListCtrl,
    provider_catalog: StaticText,
}

#[derive(Clone)]
struct DashboardRefresh {
    in_flight: Arc<AtomicBool>,
    result: Arc<Mutex<Option<(u64, DashboardSnapshot)>>>,
    last_snapshot: Arc<Mutex<Option<DashboardSnapshot>>>,
    daemon_starting: Arc<AtomicBool>,
    generation: Arc<AtomicU64>,
    closing: Arc<AtomicBool>,
    pending_startup_child: Arc<Mutex<Option<Child>>>,
}

impl DashboardRefresh {
    fn new() -> Self {
        Self {
            in_flight: Arc::new(AtomicBool::new(false)),
            result: Arc::new(Mutex::new(None)),
            last_snapshot: Arc::new(Mutex::new(None)),
            daemon_starting: Arc::new(AtomicBool::new(false)),
            generation: Arc::new(AtomicU64::new(0)),
            closing: Arc::new(AtomicBool::new(false)),
            pending_startup_child: Arc::new(Mutex::new(None)),
        }
    }
}

#[derive(Clone, Default)]
struct DashboardSnapshot {
    service_online: bool,
    status: Option<ServerStatus>,
    config: Option<AppConfig>,
    backend: Option<RemoteControlBackendStatus>,
    remote: Option<RemoteControlStatus>,
    codex_app: Option<CodexAppStatus>,
    feishu_bot: Option<FeishuBotStatus>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServerStatus {
    bind: String,
    feishu_ws: FeishuWsState,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FeishuWsState {
    connecting: bool,
    connected: bool,
    last_error: Option<String>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppConfig {
    feishu: FeishuConfig,
    bridge: BridgeConfig,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FeishuConfig {
    app_id: String,
    display_name: String,
    allowed_open_ids: Vec<String>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FeishuBotStatus {
    configured: bool,
    app_id: Option<String>,
    display_name: Option<String>,
    allowed_open_ids: usize,
    error: Option<String>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BridgeConfig {
    enabled: bool,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteControlBackendStatus {
    enabled: bool,
    feishu_configured: bool,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteControlStatus {
    connected: bool,
    initialized: bool,
    server_name: Option<String>,
    current_thread_id: Option<String>,
    last_error: Option<String>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexAppStatus {
    configured: bool,
    provider: Option<CodexAppProviderStatus>,
    #[serde(default)]
    providers: Vec<CodexAppProviderStatus>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexAppProviderStatus {
    name: String,
    base_url: Option<String>,
    key: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConfigureRequest {
    provider_name: Option<String>,
    provider_base_url: Option<String>,
    provider_key: Option<String>,
    model: Option<String>,
    activate: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeleteProviderRequest {
    provider_name: String,
}

enum ConfigActionResult {
    Configure {
        provider_name: String,
        result: Result<CodexAppStatus, String>,
    },
    Save {
        provider_name: String,
        result: Result<CodexAppStatus, String>,
    },
    Delete(Result<CodexAppStatus, String>),
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct FeishuOnboardStart {
    verification_uri_complete: String,
    device_code: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FeishuOnboardPoll {
    done: bool,
    error: Option<serde_json::Value>,
}

fn status_panel<W: WxWidget>(parent: &W, title: &str, icon_kind: StatusIconKind) -> StatusPanel {
    let panel = Panel::builder(parent)
        .with_style(PanelStyle::BorderStatic)
        .build();
    panel.set_background_color(Colour::rgb(255, 255, 255));
    panel.set_min_size(Size::new(230, 94));

    let row = BoxSizer::builder(Orientation::Horizontal).build();
    let icon = StaticBitmap::builder(&panel)
        .with_bitmap(Some(status_icon_bitmap(icon_kind, 34)))
        .with_scale_mode(Some(ScaleMode::None))
        .with_size(Size::new(34, 34))
        .build();
    icon.set_min_size(Size::new(34, 34));
    row.add_spacer(18);
    row.add(
        &icon,
        0,
        SizerFlag::AlignCenterVertical | SizerFlag::Right,
        16,
    );

    let text_col = BoxSizer::builder(Orientation::Vertical).build();
    text_col.add_stretch_spacer(1);
    let title_row = BoxSizer::builder(Orientation::Horizontal).build();
    let marker = StaticText::builder(&panel).with_label("●").build();
    marker.set_foreground_color(Colour::rgb(116, 124, 136));
    title_row.add(&marker, 0, SizerFlag::Right, 5);
    let title_label = StaticText::builder(&panel).with_label(title).build();
    title_label.set_foreground_color(Colour::rgb(91, 100, 114));
    title_row.add(&title_label, 0, SizerFlag::Bottom, 0);
    text_col.add_sizer(&title_row, 0, SizerFlag::Bottom, 4);

    let state = StaticText::builder(&panel).with_label("检测中").build();
    state.set_foreground_color(Colour::rgb(34, 39, 47));
    text_col.add(&state, 0, SizerFlag::Bottom, 4);

    let detail = StaticText::builder(&panel).with_label("").build();
    detail.set_foreground_color(Colour::rgb(103, 111, 124));
    detail.wrap(250);
    text_col.add(&detail, 0, SizerFlag::Expand, 0);
    text_col.add_stretch_spacer(1);

    row.add_sizer(&text_col, 1, SizerFlag::Expand, 0);
    row.add_spacer(18);
    panel.set_sizer(row, true);
    StatusPanel {
        panel,
        icon,
        marker,
        title: title_label,
        state,
        detail,
        icon_kind,
    }
}

fn topology_connector<W: WxWidget>(parent: &W) -> StaticBitmap {
    let bitmap = topology_connector_bitmap(72, 124);
    let connector = StaticBitmap::builder(parent)
        .with_bitmap(Some(bitmap))
        .with_scale_mode(Some(ScaleMode::None))
        .with_size(Size::new(72, 124))
        .build();
    connector.set_min_size(Size::new(72, 124));
    connector
}

fn topology_arrow<W: WxWidget>(parent: &W) -> StaticBitmap {
    let bitmap = topology_arrow_bitmap(48, 48);
    let arrow = StaticBitmap::builder(parent)
        .with_bitmap(Some(bitmap))
        .with_scale_mode(Some(ScaleMode::None))
        .with_size(Size::new(48, 48))
        .build();
    arrow.set_min_size(Size::new(48, 48));
    arrow
}

fn topology_connector_bitmap(width: usize, height: usize) -> Bitmap {
    let mut canvas = IconCanvas::new_with_size(width, height, [0, 0, 0, 0]);
    let colour = [118, 127, 140, 210];
    let trunk_x = 30usize;
    let top_y = 33usize;
    let mid_y = height / 2;
    let bottom_y = height.saturating_sub(33);
    canvas.draw_line(0, top_y, trunk_x, top_y, 2, colour);
    canvas.draw_line(0, bottom_y, trunk_x, bottom_y, 2, colour);
    canvas.draw_line(trunk_x, top_y, trunk_x, bottom_y, 2, colour);
    canvas.draw_line(trunk_x, mid_y, width.saturating_sub(1), mid_y, 2, colour);
    Bitmap::from_rgba(&canvas.rgba, width as u32, height as u32).expect("topology connector bitmap")
}

fn topology_arrow_bitmap(width: usize, height: usize) -> Bitmap {
    let mut canvas = IconCanvas::new_with_size(width, height, [0, 0, 0, 0]);
    canvas.draw_line(
        0,
        height / 2,
        width.saturating_sub(1),
        height / 2,
        2,
        [118, 127, 140, 210],
    );
    Bitmap::from_rgba(&canvas.rgba, width as u32, height as u32).expect("topology arrow bitmap")
}

fn status_icon_bitmap(kind: StatusIconKind, size: usize) -> Bitmap {
    match kind {
        StatusIconKind::Feishu => {
            return brand_bitmap(
                "feishu-logo.png",
                include_bytes!("../packaging/brand/feishu-logo.png"),
                size,
            );
        }
        StatusIconKind::Codex => {
            return brand_bitmap(
                "codex-app-logo.png",
                include_bytes!("../packaging/brand/codex-app-logo.png"),
                size,
            );
        }
        StatusIconKind::VsCodeCodex => {
            return brand_bitmap(
                "codex-vscode-logo.png",
                include_bytes!("../packaging/brand/codex-vscode-logo.png"),
                size,
            );
        }
        StatusIconKind::Service => {}
    }

    let mut canvas = IconCanvas::new(size, [0, 0, 0, 0]);
    draw_service_icon(&mut canvas);
    Bitmap::from_rgba(&canvas.rgba, size as u32, size as u32).expect("status icon bitmap")
}

fn disabled_status_icon_bitmap(kind: StatusIconKind, size: usize) -> Bitmap {
    match kind {
        StatusIconKind::Feishu => {
            return disabled_brand_bitmap(
                "feishu-logo.png",
                include_bytes!("../packaging/brand/feishu-logo.png"),
                size,
            );
        }
        StatusIconKind::Codex => {
            return disabled_brand_bitmap(
                "codex-app-logo.png",
                include_bytes!("../packaging/brand/codex-app-logo.png"),
                size,
            );
        }
        StatusIconKind::VsCodeCodex => {
            return disabled_brand_bitmap(
                "codex-vscode-logo.png",
                include_bytes!("../packaging/brand/codex-vscode-logo.png"),
                size,
            );
        }
        StatusIconKind::Service => {}
    }

    let mut canvas = IconCanvas::new(size, [0, 0, 0, 0]);
    draw_disabled_service_icon(&mut canvas);
    Bitmap::from_rgba(&canvas.rgba, size as u32, size as u32).expect("disabled status icon bitmap")
}

fn app_icon_bitmap(size: usize) -> Bitmap {
    brand_bitmap(
        "dolphin-rounded-256.png",
        include_bytes!("../packaging/icons/dolphin-rounded-256.png"),
        size,
    )
}

fn brand_bitmap(file_name: &str, bytes: &[u8], size: usize) -> Bitmap {
    let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
        .unwrap_or_else(|err| panic!("failed to load brand image {file_name}: {err}"))
        .resize(size as u32, size as u32, FilterType::Lanczos3)
        .into_rgba8();
    let (width, height) = image.dimensions();
    Bitmap::from_rgba(image.as_raw(), width, height)
        .unwrap_or_else(|| panic!("failed to create bitmap from {file_name}"))
}

fn disabled_brand_bitmap(file_name: &str, bytes: &[u8], size: usize) -> Bitmap {
    let mut image = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
        .unwrap_or_else(|err| panic!("failed to load brand image {file_name}: {err}"))
        .resize(size as u32, size as u32, FilterType::Lanczos3)
        .into_rgba8();
    for pixel in image.pixels_mut() {
        let alpha = pixel[3];
        if alpha == 0 {
            continue;
        }
        let gray =
            ((pixel[0] as u16 * 30 + pixel[1] as u16 * 59 + pixel[2] as u16 * 11) / 100) as u8;
        let soft = (gray as u16 + 180) / 2;
        pixel[0] = soft as u8;
        pixel[1] = soft as u8;
        pixel[2] = soft as u8;
        pixel[3] = ((alpha as u16 * 50) / 100) as u8;
    }
    let (width, height) = image.dimensions();
    Bitmap::from_rgba(image.as_raw(), width, height)
        .unwrap_or_else(|| panic!("failed to create disabled bitmap from {file_name}"))
}

struct IconCanvas {
    width: usize,
    height: usize,
    rgba: Vec<u8>,
}

impl IconCanvas {
    fn new(size: usize, background: [u8; 4]) -> Self {
        Self::new_with_size(size, size, background)
    }

    fn new_with_size(width: usize, height: usize, background: [u8; 4]) -> Self {
        let mut rgba = vec![0; width * height * 4];
        for pixel in rgba.chunks_exact_mut(4) {
            pixel.copy_from_slice(&background);
        }
        Self {
            width,
            height,
            rgba,
        }
    }

    fn fill_circle(&mut self, cx: f32, cy: f32, radius: f32, color: [u8; 4]) {
        let min_x = (cx - radius).floor().max(0.0) as usize;
        let max_x = (cx + radius).ceil().min((self.width - 1) as f32) as usize;
        let min_y = (cy - radius).floor().max(0.0) as usize;
        let max_y = (cy + radius).ceil().min((self.height - 1) as f32) as usize;
        let radius_sq = radius * radius;
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let dx = x as f32 + 0.5 - cx;
                let dy = y as f32 + 0.5 - cy;
                if dx * dx + dy * dy <= radius_sq {
                    self.set_pixel(x, y, color);
                }
            }
        }
    }

    fn fill_rect(&mut self, x: usize, y: usize, width: usize, height: usize, color: [u8; 4]) {
        for yy in y..(y + height).min(self.height) {
            for xx in x..(x + width).min(self.width) {
                self.set_pixel(xx, yy, color);
            }
        }
    }

    fn draw_line(
        &mut self,
        x1: usize,
        y1: usize,
        x2: usize,
        y2: usize,
        thickness: usize,
        color: [u8; 4],
    ) {
        if y1 == y2 {
            let start = x1.min(x2);
            let end = x1.max(x2);
            let y = y1.saturating_sub(thickness / 2);
            self.fill_rect(start, y, end - start + 1, thickness, color);
        } else if x1 == x2 {
            let start = y1.min(y2);
            let end = y1.max(y2);
            let x = x1.saturating_sub(thickness / 2);
            self.fill_rect(x, start, thickness, end - start + 1, color);
        }
    }

    fn fill_round_rect(
        &mut self,
        x: usize,
        y: usize,
        width: usize,
        height: usize,
        radius: usize,
        color: [u8; 4],
    ) {
        let x2 = x + width - 1;
        let y2 = y + height - 1;
        let radius = radius as f32;
        for yy in y..=y2.min(self.height - 1) {
            for xx in x..=x2.min(self.width - 1) {
                let cx = if xx < x + radius as usize {
                    x as f32 + radius
                } else if xx > x2.saturating_sub(radius as usize) {
                    x2 as f32 - radius
                } else {
                    xx as f32
                };
                let cy = if yy < y + radius as usize {
                    y as f32 + radius
                } else if yy > y2.saturating_sub(radius as usize) {
                    y2 as f32 - radius
                } else {
                    yy as f32
                };
                let dx = xx as f32 - cx;
                let dy = yy as f32 - cy;
                if dx * dx + dy * dy <= radius * radius {
                    self.set_pixel(xx, yy, color);
                }
            }
        }
    }

    fn set_pixel(&mut self, x: usize, y: usize, color: [u8; 4]) {
        let offset = (y * self.width + x) * 4;
        self.rgba[offset..offset + 4].copy_from_slice(&color);
    }
}

fn draw_service_icon(canvas: &mut IconCanvas) {
    canvas.fill_circle(17.0, 17.0, 17.0, [229, 247, 239, 255]);
    canvas.fill_round_rect(9, 9, 16, 16, 3, [29, 142, 103, 255]);
    canvas.fill_round_rect(12, 12, 10, 3, 1, [246, 255, 251, 255]);
    canvas.fill_round_rect(12, 17, 10, 3, 1, [246, 255, 251, 255]);
    canvas.fill_rect(12, 22, 3, 2, [246, 255, 251, 255]);
}

fn draw_disabled_service_icon(canvas: &mut IconCanvas) {
    canvas.fill_circle(17.0, 17.0, 17.0, [229, 232, 236, 180]);
    canvas.fill_round_rect(9, 9, 16, 16, 3, [151, 158, 168, 130]);
    canvas.fill_round_rect(12, 12, 10, 3, 1, [247, 248, 250, 180]);
    canvas.fill_round_rect(12, 17, 10, 3, 1, [247, 248, 250, 180]);
    canvas.fill_rect(12, 22, 3, 2, [247, 248, 250, 180]);
}

fn text_field_row<W: WxWidget>(
    parent: &W,
    sizer: &FlexGridSizer,
    label: &str,
    value: &str,
) -> TextCtrl {
    let label_widget = StaticText::builder(parent).with_label(label).build();
    label_widget.set_foreground_color(Colour::rgb(78, 86, 98));
    sizer.add(
        &label_widget,
        0,
        SizerFlag::AlignCenterVertical | SizerFlag::Right,
        0,
    );

    let input = TextCtrl::builder(parent)
        .with_value(value)
        .with_style(TextCtrlStyle::Default)
        .build();
    input.set_min_size(Size::new(420, 30));
    sizer.add(&input, 1, SizerFlag::Expand, 0);
    input
}

fn provider_combo_row<W: WxWidget>(
    parent: &W,
    sizer: &FlexGridSizer,
    label: &str,
    value: &str,
) -> ComboBox {
    let label_widget = StaticText::builder(parent).with_label(label).build();
    label_widget.set_foreground_color(Colour::rgb(78, 86, 98));
    sizer.add(
        &label_widget,
        0,
        SizerFlag::AlignCenterVertical | SizerFlag::Right,
        0,
    );

    let input = ComboBox::builder(parent)
        .with_value(value)
        .with_style(ComboBoxStyle::Default | ComboBoxStyle::ProcessEnter)
        .build();
    input.set_min_size(Size::new(420, 30));
    sizer.add(&input, 1, SizerFlag::Expand, 0);
    input
}

fn schedule_dashboard_refresh(api: &ApiClient, refresh: &DashboardRefresh) -> bool {
    if refresh.in_flight.swap(true, Ordering::SeqCst) {
        return false;
    }

    let api = api.clone();
    let result = refresh.result.clone();
    let in_flight = refresh.in_flight.clone();
    let generation = refresh.generation.load(Ordering::SeqCst);
    thread::spawn(move || {
        let snapshot = api.dashboard();
        if let Ok(mut slot) = result.lock() {
            slot.replace((generation, snapshot));
        }
        in_flight.store(false, Ordering::SeqCst);
    });
    true
}

fn apply_pending_dashboard(handles: &UiHandles, refresh: &DashboardRefresh) -> bool {
    let result = refresh.result.lock().ok().and_then(|mut slot| slot.take());
    let Some((generation, snapshot)) = result else {
        return false;
    };
    if generation != refresh.generation.load(Ordering::SeqCst) {
        return false;
    }

    let daemon_starting = refresh.daemon_starting.load(Ordering::SeqCst);
    update_dashboard(handles, &snapshot, daemon_starting);
    if let Ok(mut last_snapshot) = refresh.last_snapshot.lock() {
        last_snapshot.replace(snapshot);
    }
    true
}

fn cached_dashboard_snapshot(refresh: &DashboardRefresh) -> Option<DashboardSnapshot> {
    refresh
        .last_snapshot
        .lock()
        .ok()
        .and_then(|snapshot| snapshot.clone())
}

fn configure_codex_app_and_verify(
    api: &ApiClient,
    request: &ConfigureRequest,
    selected_provider: &str,
) -> Result<CodexAppStatus, String> {
    api.configure_codex_app(request)?;
    let status = api.codex_app_status()?;
    verify_selected_provider(&status, selected_provider)?;
    Ok(status)
}

fn save_codex_provider_and_verify(
    api: &ApiClient,
    request: &ConfigureRequest,
    selected_provider: &str,
) -> Result<CodexAppStatus, String> {
    api.configure_codex_app(request)?;
    let status = api.codex_app_status()?;
    verify_saved_provider(&status, selected_provider)?;
    Ok(status)
}

fn delete_codex_provider_and_verify(
    api: &ApiClient,
    request: &DeleteProviderRequest,
) -> Result<CodexAppStatus, String> {
    api.delete_codex_provider(request)?;
    let status = api.codex_app_status()?;
    verify_deleted_provider(&status, &request.provider_name)?;
    Ok(status)
}

fn verify_selected_provider(
    status: &CodexAppStatus,
    selected_provider: &str,
) -> Result<(), String> {
    let selected_provider = selected_provider.trim();
    if selected_provider.is_empty() {
        return Ok(());
    }

    let active = status
        .provider
        .as_ref()
        .map(|provider| provider.name.as_str());
    if active == Some(selected_provider) {
        return Ok(());
    }

    Err(format!(
        "配置接口已返回成功，但当前 provider 仍是 {}，期望是 {}。请刷新后再试一次。",
        active.unwrap_or("<未设置>"),
        selected_provider
    ))
}

fn verify_saved_provider(status: &CodexAppStatus, selected_provider: &str) -> Result<(), String> {
    let selected_provider = selected_provider.trim();
    if selected_provider.is_empty() {
        return Err("Provider 名称不能为空。".to_string());
    }

    if provider_rows(status)
        .iter()
        .any(|provider| provider.name == selected_provider)
    {
        return Ok(());
    }

    Err(format!(
        "保存接口已返回成功，但 provider {} 没有出现在配置列表里。请刷新后再试一次。",
        selected_provider
    ))
}

fn verify_deleted_provider(status: &CodexAppStatus, provider_name: &str) -> Result<(), String> {
    if provider_rows(status)
        .iter()
        .any(|provider| provider.name == provider_name)
    {
        return Err(format!(
            "删除接口已返回成功，但 provider {} 仍在配置列表里。请刷新后再试一次。",
            provider_name
        ));
    }
    Ok(())
}

fn apply_pending_config_action(
    api: &ApiClient,
    handles: &UiHandles,
    frame: &Frame,
    refresh: &DashboardRefresh,
    result: &ConfigActionResultStore,
) -> bool {
    let result = result.lock().ok().and_then(|mut slot| slot.take());
    let Some(result) = result else {
        return false;
    };

    handles.configure_button.set_label("启用");
    handles.save_provider_button.set_label("保存");
    handles.delete_provider_button.set_label("删除");
    set_actions_enabled(handles, true);

    match result {
        ConfigActionResult::Save {
            provider_name,
            result: Ok(status),
        } => {
            apply_provider_action_status(handles, refresh, status, &provider_name);
            show_info(frame, "Provider 已保存。需要使用它时再点击启用。");
            schedule_dashboard_refresh(api, refresh);
        }
        ConfigActionResult::Save {
            result: Err(err), ..
        } => {
            show_local_codex_app_config_preview(handles, api, refresh);
            show_error(frame, &err);
        }
        ConfigActionResult::Delete(Ok(status)) => {
            clear_provider_list_selection(&handles.provider_list);
            set_combo_value_if_changed(&handles.provider_name, "");
            change_text_value_if_changed(&handles.provider_base_url, "");
            change_text_value_if_changed(&handles.provider_key, "");
            let snapshot = DashboardSnapshot {
                service_online: true,
                codex_app: Some(status),
                ..DashboardSnapshot::default()
            };
            if let Ok(mut last_snapshot) = refresh.last_snapshot.lock() {
                last_snapshot.replace(snapshot.clone());
            }
            fill_provider_form_if_empty(handles, &snapshot);
            show_info(frame, "Provider 已删除。");
            schedule_dashboard_refresh(api, refresh);
        }
        ConfigActionResult::Delete(Err(err)) => {
            show_local_codex_app_config_preview(handles, api, refresh);
            show_error(frame, &err);
        }
        ConfigActionResult::Configure {
            provider_name,
            result: Ok(status),
        } => {
            apply_provider_action_status(handles, refresh, status, &provider_name);
            show_info(
                frame,
                "已启用。请重启 Codex App，然后在 App 里打开 remote-control；VS Code 插件也可以接入。",
            );
            schedule_dashboard_refresh(api, refresh);
        }
        ConfigActionResult::Configure {
            result: Err(err), ..
        } => {
            show_local_codex_app_config_preview(handles, api, refresh);
            show_error(frame, &err);
        }
    }
    true
}

fn apply_provider_action_status(
    handles: &UiHandles,
    refresh: &DashboardRefresh,
    status: CodexAppStatus,
    provider_name: &str,
) {
    let snapshot = DashboardSnapshot {
        service_online: true,
        codex_app: Some(status),
        ..DashboardSnapshot::default()
    };
    if let Ok(mut last_snapshot) = refresh.last_snapshot.lock() {
        last_snapshot.replace(snapshot.clone());
    }

    if let Some(status) = snapshot.codex_app.as_ref() {
        handles
            .provider_catalog
            .set_label(&provider_catalog_label(status));
        handles.provider_catalog.wrap(980);
        handles.provider_catalog.layout();
        refresh_provider_choices(&handles.provider_name, &status.providers);
        refresh_provider_list(handles, Some(status));
    }

    if let Some(provider) = find_provider(&snapshot, provider_name) {
        apply_provider_to_form(handles, &provider, true);
    } else {
        set_combo_value_if_changed(&handles.provider_name, provider_name);
    }
}

fn show_dashboard_starting(handles: &UiHandles) {
    set_status_panel(
        &handles.service_status,
        "启动中",
        "正在启动本地 backend。",
        StateTone::Warn,
    );
    set_status_panel(
        &handles.feishu_status,
        "等待服务",
        "服务启动后读取飞书状态。",
        StateTone::Muted,
    );
    set_disabled_status_panel(
        &handles.codex_status,
        "等待服务",
        if CODEX_APP_GUI_UNSUPPORTED {
            "当前平台暂不支持 App GUI"
        } else {
            "服务启动后读取配置"
        },
    );
    set_status_panel(
        &handles.vscode_status,
        "等待服务",
        "服务启动后可连接 VS Code 插件。",
        StateTone::Muted,
    );
    handles.feishu_state.set_label("本地服务启动中");
    handles
        .feishu_detail
        .set_label("服务启动完成后会刷新飞书状态。");
    handles.feishu_meta.set_label("");
    set_actions_enabled(handles, false);
}

fn show_local_codex_app_config_preview(
    handles: &UiHandles,
    api: &ApiClient,
    refresh: &DashboardRefresh,
) {
    if CODEX_APP_GUI_UNSUPPORTED {
        return;
    }
    let status = crate::codex_app_config::inspect_codex_app_config(None, &api.url("/backend-api"));
    let snapshot = DashboardSnapshot {
        service_online: false,
        codex_app: Some(local_codex_app_status(status)),
        ..DashboardSnapshot::default()
    };
    if let Ok(mut last_snapshot) = refresh.last_snapshot.lock() {
        last_snapshot.replace(snapshot.clone());
    }
    fill_provider_form_if_empty(handles, &snapshot);
}

fn local_codex_app_status(status: crate::codex_app_config::CodexAppConfigStatus) -> CodexAppStatus {
    CodexAppStatus {
        configured: status.configured,
        provider: status.provider.map(local_codex_app_provider_status),
        providers: status
            .providers
            .into_iter()
            .map(local_codex_app_provider_status)
            .collect(),
    }
}

fn local_codex_app_provider_status(
    provider: crate::codex_app_config::CodexAppProviderStatus,
) -> CodexAppProviderStatus {
    CodexAppProviderStatus {
        name: provider.name,
        base_url: provider.base_url,
        key: provider.key,
    }
}

fn update_dashboard(handles: &UiHandles, snapshot: &DashboardSnapshot, daemon_starting: bool) {
    if !snapshot.service_online {
        if daemon_starting {
            show_dashboard_starting(handles);
            return;
        }
        set_status_panel(
            &handles.service_status,
            "未运行",
            "GUI 会自动启动本地服务；如果一直未运行，请重启 Codex Remote。",
            StateTone::Error,
        );
        set_status_panel(
            &handles.feishu_status,
            "不可用",
            "本地服务未运行",
            StateTone::Muted,
        );
        set_disabled_status_panel(
            &handles.codex_status,
            "不可用",
            if CODEX_APP_GUI_UNSUPPORTED {
                "当前平台暂不支持 App GUI"
            } else {
                "本地服务未运行"
            },
        );
        set_status_panel(
            &handles.vscode_status,
            "不可用",
            "本地服务未运行",
            StateTone::Muted,
        );
        handles.feishu_state.set_label("本地服务未运行");
        handles
            .feishu_detail
            .set_label("请重启 Codex Remote，或查看 logs/codex-remote-chain.log。");
        handles.feishu_meta.set_label("");
        set_actions_enabled(handles, false);
        return;
    }

    set_actions_enabled(handles, true);

    if let Some(status) = &snapshot.status {
        set_status_panel(
            &handles.service_status,
            "运行中",
            &format!("监听 {}", status.bind),
            StateTone::Ok,
        );
    }

    let feishu_configured = feishu_configured(snapshot);
    let bridge_enabled = bridge_enabled(snapshot);

    let remote_connected = snapshot
        .remote
        .as_ref()
        .map(|remote| remote.connected)
        .unwrap_or(false);
    let remote_initialized = snapshot
        .remote
        .as_ref()
        .map(|remote| remote.initialized)
        .unwrap_or(false);
    let codex_control_ready = remote_connected && remote_initialized;
    let codex_configured = snapshot
        .codex_app
        .as_ref()
        .map(|status| status.configured)
        .unwrap_or(false);

    let feishu_ws = snapshot.status.as_ref().map(|status| &status.feishu_ws);
    let feishu_bot_name = feishu_bot_display_name(snapshot);
    let (feishu_state, feishu_detail, feishu_tone) = if !feishu_configured {
        (
            "未接入",
            "扫码接入飞书机器人后才会启动飞书桥接。",
            StateTone::Warn,
        )
    } else if !bridge_enabled {
        (
            "已断开",
            "机器人已保存，点击“重新接入”恢复。",
            StateTone::Muted,
        )
    } else if feishu_ws.is_some_and(|ws| ws.connected) {
        let detail = if codex_control_ready {
            "飞书桥接运行中。"
        } else if remote_connected {
            "飞书桥接运行中，Codex App 控制通道正在初始化。"
        } else {
            "飞书桥接运行中，等待 Codex App 打开“控制这台 Mac”。"
        };
        ("已接入", detail, StateTone::Ok)
    } else if feishu_ws.is_some_and(|ws| ws.connecting) {
        ("连接中", "正在连接飞书。", StateTone::Warn)
    } else {
        (
            "等待连接",
            "机器人已保存，等待飞书桥接启动。",
            StateTone::Warn,
        )
    };

    set_status_panel_title(
        &handles.feishu_status,
        &feishu_status_title(feishu_bot_name.as_deref()),
    );
    set_status_panel(
        &handles.feishu_status,
        feishu_state,
        feishu_detail,
        feishu_tone,
    );
    let feishu_state_label = feishu_state_label(feishu_state, feishu_bot_name.as_deref());
    handles.feishu_state.set_label(&feishu_state_label);
    handles
        .feishu_state
        .set_foreground_color(feishu_tone.colour());
    handles.feishu_detail.set_label(feishu_detail);
    handles.feishu_detail.wrap(300);
    if feishu_configured && bridge_enabled {
        handles.stop_bridge_button.set_label("断开接入");
        handles
            .stop_bridge_button
            .set_tooltip("暂停飞书桥接，不删除已保存的机器人配置");
    } else if feishu_configured {
        handles.stop_bridge_button.set_label("重新接入");
        handles
            .stop_bridge_button
            .set_tooltip("使用已保存的飞书机器人重新接入");
    } else {
        handles.stop_bridge_button.set_label("断开接入");
        handles
            .stop_bridge_button
            .set_tooltip("扫码接入飞书机器人后可断开或重新接入");
    }
    handles.stop_bridge_button.enable(feishu_configured);

    let feishu_meta = match (&snapshot.config, &snapshot.feishu_bot) {
        (Some(config), Some(bot)) if !config.feishu.app_id.is_empty() => {
            let mut lines = Vec::new();
            if let Some(name) = feishu_bot_name.as_deref() {
                lines.push(format!("机器人: {name}"));
            }
            lines.push(format!(
                "App ID: {}",
                short_id(bot.app_id.as_deref().unwrap_or(&config.feishu.app_id))
            ));
            lines.push(format!("允许用户: {}", bot.allowed_open_ids));
            if let Some(err) = feishu_ws.and_then(|ws| ws.last_error.as_deref())
                && !err.is_empty()
            {
                lines.push(format!("最近错误: {err}"));
            } else if let Some(err) = bot.error.as_deref()
                && !err.is_empty()
            {
                lines.push(format!("名称读取失败: {err}"));
            }
            lines.join("\n")
        }
        (Some(config), _) if !config.feishu.app_id.is_empty() => format!(
            "{}App ID: {}\n允许用户: {}{}",
            feishu_bot_name
                .as_deref()
                .map(|name| format!("机器人: {name}\n"))
                .unwrap_or_default(),
            short_id(&config.feishu.app_id),
            config.feishu.allowed_open_ids.len(),
            feishu_ws
                .and_then(|ws| ws.last_error.as_deref())
                .filter(|err| !err.is_empty())
                .map(|err| format!("\n最近错误: {err}"))
                .unwrap_or_default()
        ),
        _ => "未保存飞书机器人凭据。".to_string(),
    };
    handles.feishu_meta.set_label(&feishu_meta);
    handles.feishu_meta.wrap(300);

    if CODEX_APP_GUI_UNSUPPORTED {
        set_disabled_status_panel(
            &handles.codex_status,
            "暂不可用",
            "当前平台暂不支持 App GUI",
        );
    } else if codex_control_ready {
        let detail = snapshot
            .remote
            .as_ref()
            .map(codex_remote_detail)
            .unwrap_or_else(|| "Codex App remote-control 已连接。".to_string());
        set_status_panel(&handles.codex_status, "已连接", &detail, StateTone::Ok);
    } else if remote_connected {
        set_status_panel(
            &handles.codex_status,
            "初始化中",
            "Codex App 已打开控制通道，正在完成 remote-control 初始化。",
            StateTone::Warn,
        );
    } else if codex_configured {
        set_status_panel(
            &handles.codex_status,
            "未打开控制",
            "配置已注入，请在 Codex App 里打开“控制这台 Mac”。",
            StateTone::Warn,
        );
    } else {
        set_status_panel(
            &handles.codex_status,
            "未注入",
            "填写 Base URL 和 API Key 后点击启用。",
            StateTone::Warn,
        );
    }

    if codex_control_ready {
        let detail = snapshot
            .remote
            .as_ref()
            .map(codex_remote_detail)
            .unwrap_or_else(|| "remote-control 已连接。".to_string());
        set_status_panel(&handles.vscode_status, "已连接", &detail, StateTone::Ok);
    } else {
        set_status_panel(
            &handles.vscode_status,
            "可接入",
            "VS Code 插件可通过 chatgpt.cliExecutable 使用本地 wrapper。",
            StateTone::Warn,
        );
    }
}

fn feishu_configured(snapshot: &DashboardSnapshot) -> bool {
    snapshot
        .backend
        .as_ref()
        .map(|backend| backend.feishu_configured)
        .or_else(|| {
            snapshot
                .config
                .as_ref()
                .map(|config| !config.feishu.app_id.is_empty())
        })
        .or_else(|| snapshot.feishu_bot.as_ref().map(|bot| bot.configured))
        .unwrap_or(false)
}

fn bridge_enabled(snapshot: &DashboardSnapshot) -> bool {
    snapshot
        .backend
        .as_ref()
        .map(|backend| backend.enabled)
        .or_else(|| snapshot.config.as_ref().map(|config| config.bridge.enabled))
        .unwrap_or(false)
}

fn fill_provider_form_if_empty(handles: &UiHandles, snapshot: &DashboardSnapshot) {
    let Some(status) = snapshot.codex_app.as_ref() else {
        handles
            .provider_catalog
            .set_label("本地服务运行后会读取 ~/.codex/config.toml 里的 provider。");
        handles.provider_catalog.wrap(980);
        handles.provider_catalog.layout();
        refresh_provider_list(handles, None);
        return;
    };
    handles
        .provider_catalog
        .set_label(&provider_catalog_label(status));
    handles.provider_catalog.wrap(980);
    handles.provider_catalog.layout();
    refresh_provider_list(handles, Some(status));

    if provider_form_has_focus(handles) {
        return;
    }

    refresh_provider_choices(&handles.provider_name, &status.providers);

    let target = status
        .provider
        .as_ref()
        .or_else(|| status.providers.first());
    let current = handles.provider_name.get_value();
    let current = current.trim();
    let provider_values_empty = handles.provider_base_url.get_value().trim().is_empty()
        && handles.provider_key.get_value().trim().is_empty();

    if current.is_empty() {
        if let Some(provider) = target {
            apply_provider_to_form(handles, provider, true);
        } else {
            set_combo_value_if_changed(&handles.provider_name, DEFAULT_PROVIDER_NAME);
        }
    } else if current == DEFAULT_PROVIDER_NAME
        && provider_values_empty
        && let Some(provider) = target
        && provider.name != DEFAULT_PROVIDER_NAME
    {
        apply_provider_to_form(handles, provider, true);
    }

    let selected = handles.provider_name.get_value();
    if let Some(provider) = find_provider(snapshot, &selected) {
        apply_provider_to_form(handles, &provider, false);
    }
}

fn provider_form_has_focus(handles: &UiHandles) -> bool {
    handles.provider_name.has_focus()
        || handles.provider_base_url.has_focus()
        || handles.provider_key.has_focus()
}

fn refresh_provider_choices(input: &ComboBox, providers: &[CodexAppProviderStatus]) {
    let names = provider_choice_names(providers);
    if combo_box_items(input) == names {
        return;
    }

    let current = input.get_value();
    let insertion_point = input.get_insertion_point();
    input.clear();
    for name in names {
        input.append(&name);
    }
    set_combo_value_if_changed(input, &current);
    input.set_insertion_point(insertion_point.min(current.chars().count() as i64));
}

fn refresh_provider_list(handles: &UiHandles, status: Option<&CodexAppStatus>) {
    let rows = provider_list_rows(status);
    if provider_list_matches(&handles.provider_list, &rows) {
        return;
    }

    handles.provider_list.delete_all_items();

    for (index, row_data) in rows.iter().enumerate() {
        let row = handles
            .provider_list
            .insert_item(index as i64, &row_data[0], None);
        handles
            .provider_list
            .set_item_text_by_column(row as i64, 1, row_data[1].as_str());
        handles
            .provider_list
            .set_item_text_by_column(row as i64, 2, row_data[2].as_str());
        handles
            .provider_list
            .set_item_text_by_column(row as i64, 3, row_data[3].as_str());

        if row_data[2] == "使用中" {
            handles.provider_list.ensure_visible(row as i64);
        }
    }
}

fn provider_list_rows(status: Option<&CodexAppStatus>) -> Vec<[String; 4]> {
    let Some(status) = status else {
        return vec![[
            "等待本地服务".to_string(),
            "启动后读取 ~/.codex/config.toml".to_string(),
            String::new(),
            String::new(),
        ]];
    };

    let active_name = status
        .provider
        .as_ref()
        .map(|provider| provider.name.as_str());
    let providers = provider_rows(status);
    if providers.is_empty() {
        return vec![[
            DEFAULT_PROVIDER_NAME.to_string(),
            "未配置，写入时新建".to_string(),
            String::new(),
            "未配置".to_string(),
        ]];
    }

    providers
        .iter()
        .map(|provider| {
            [
                provider.name.clone(),
                provider
                    .base_url
                    .clone()
                    .unwrap_or_else(|| "未配置".to_string()),
                if Some(provider.name.as_str()) == active_name {
                    "使用中".to_string()
                } else {
                    String::new()
                },
                masked_provider_key(provider.key.as_deref()),
            ]
        })
        .collect()
}

fn provider_list_matches(list: &ListCtrl, rows: &[[String; 4]]) -> bool {
    if list.get_item_count() != rows.len() as i32 {
        return false;
    }
    rows.iter().enumerate().all(|(index, row)| {
        (0..4).all(|column| list.get_item_text(index as i64, column) == row[column as usize])
    })
}

fn provider_rows(status: &CodexAppStatus) -> Vec<CodexAppProviderStatus> {
    let mut providers = status.providers.clone();
    if let Some(active) = &status.provider
        && !providers
            .iter()
            .any(|provider| provider.name == active.name)
    {
        providers.insert(0, active.clone());
    }
    providers
}

fn provider_choice_names(providers: &[CodexAppProviderStatus]) -> Vec<String> {
    if providers.is_empty() {
        return vec![DEFAULT_PROVIDER_NAME.to_string()];
    }

    let mut names = Vec::<String>::new();
    for provider in providers {
        if !names.iter().any(|name| name == &provider.name) {
            names.push(provider.name.clone());
        }
    }
    names
}

fn masked_provider_key(value: Option<&str>) -> String {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return "未配置".to_string();
    };
    format!("已配置 {}", masked_secret(value))
}

fn masked_provider_key_input(value: Option<&str>) -> String {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(masked_secret)
        .unwrap_or_default()
}

fn masked_secret(value: &str) -> String {
    let suffix = value
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("****{suffix}")
}

fn provider_key_value_for_config(value: &str) -> Option<String> {
    let value = value.trim();
    if is_placeholder_config_value(value) || is_masked_provider_key(value) {
        None
    } else {
        Some(value.to_string())
    }
}

fn is_masked_provider_key(value: &str) -> bool {
    let value = value.trim();
    value.starts_with("****")
        && value.chars().filter(|ch| *ch == '*').count() >= 4
        && value.chars().any(|ch| ch != '*')
}

fn combo_box_items(input: &ComboBox) -> Vec<String> {
    (0..input.get_count())
        .filter_map(|index| input.get_string(index))
        .collect()
}

fn provider_catalog_label(status: &CodexAppStatus) -> String {
    if status.providers.is_empty() {
        if let Some(active) = status.provider.as_ref() {
            return format!("当前 provider: {active}", active = active.name.as_str());
        }
        return "还没有 provider，填写后点击启用。".to_string();
    }

    if let Some(active) = status.provider.as_ref() {
        format!("当前 provider: {}", active.name)
    } else {
        format!(
            "已保存 {} 个 provider，请选择一个使用。",
            status.providers.len()
        )
    }
}

fn find_provider(
    snapshot: &DashboardSnapshot,
    provider_name: &str,
) -> Option<CodexAppProviderStatus> {
    let provider_name = provider_name.trim();
    if provider_name.is_empty() {
        return None;
    }
    let status = snapshot.codex_app.as_ref()?;
    status
        .providers
        .iter()
        .find(|provider| provider.name == provider_name)
        .cloned()
        .or_else(|| {
            status
                .provider
                .as_ref()
                .filter(|provider| provider.name == provider_name)
                .cloned()
        })
}

fn provider_from_list_row(
    snapshot: &DashboardSnapshot,
    row: i64,
) -> Option<CodexAppProviderStatus> {
    let status = snapshot.codex_app.as_ref()?;
    (row >= 0)
        .then(|| provider_rows(status).get(row as usize).cloned())
        .flatten()
}

fn provider_config_request_from_ui(
    handles: &UiHandles,
    provider_name: &ComboBox,
    provider_base_url: &TextCtrl,
    provider_key: &TextCtrl,
    snapshot: Option<&DashboardSnapshot>,
    activate: bool,
) -> (String, ConfigureRequest) {
    let form_provider = clean_provider_text(&provider_name.get_value());
    let mut selected_provider = form_provider.clone();
    let mut selected_base_url = strip_nul(&provider_base_url.get_value());
    let mut selected_key = strip_nul(&provider_key.get_value());

    let selected_row = handles.provider_list.get_first_selected_item();
    if selected_provider.is_empty() && selected_row >= 0 {
        let row = selected_row as i64;
        if let Some(provider) = snapshot.and_then(|snapshot| provider_from_list_row(snapshot, row))
        {
            selected_provider = provider.name;
            let row_base_url = provider.base_url.unwrap_or_default();

            if selected_provider != form_provider || selected_base_url.trim().is_empty() {
                selected_base_url = row_base_url;
            }

            let row_key = masked_provider_key_input(provider.key.as_deref());
            if selected_provider != form_provider || selected_key.trim().is_empty() {
                selected_key = row_key;
            }
        } else {
            let row_name = clean_provider_text(&handles.provider_list.get_item_text(row, 0));
            if is_real_provider_name(&row_name) {
                selected_provider = row_name;
                let row_base_url =
                    list_base_url_cell_to_input(&handles.provider_list.get_item_text(row, 1));

                if selected_provider != form_provider || selected_base_url.trim().is_empty() {
                    selected_base_url = row_base_url;
                }

                let row_key = list_key_cell_to_input(&handles.provider_list.get_item_text(row, 3));
                if selected_provider != form_provider || selected_key.trim().is_empty() {
                    selected_key = row_key;
                }
            }
        }
    }

    let selected_base_url = config_text_value(&selected_base_url).unwrap_or_default();
    let provider_key = provider_key_value_for_config(&selected_key);
    let request = ConfigureRequest {
        provider_name: Some(selected_provider.clone()),
        provider_base_url: Some(selected_base_url),
        provider_key,
        model: None,
        activate,
    };
    (selected_provider, request)
}

fn provider_name_from_ui(
    handles: &UiHandles,
    provider_name: &ComboBox,
    snapshot: Option<&DashboardSnapshot>,
) -> String {
    let form_provider = clean_provider_text(&provider_name.get_value());
    if !form_provider.is_empty() {
        return form_provider;
    }

    let selected_row = handles.provider_list.get_first_selected_item();
    if selected_row < 0 {
        return String::new();
    }

    snapshot
        .and_then(|snapshot| provider_from_list_row(snapshot, selected_row as i64))
        .map(|provider| provider.name)
        .unwrap_or_else(|| {
            clean_provider_text(&handles.provider_list.get_item_text(selected_row as i64, 0))
        })
}

fn is_real_provider_name(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty() && value != "等待本地服务"
}

fn apply_provider_row_to_form(handles: &UiHandles, list: &ListCtrl, row: i64) {
    let name = clean_provider_text(&list.get_item_text(row, 0));
    let base_url = list_base_url_cell_to_input(&list.get_item_text(row, 1));
    let key = list_key_cell_to_input(&list.get_item_text(row, 3));
    if is_real_provider_name(&name) {
        set_combo_value_if_changed(&handles.provider_name, &name);
    }
    change_text_value_if_changed(&handles.provider_base_url, &base_url);
    change_text_value_if_changed(&handles.provider_key, &key);
}

fn list_base_url_cell_to_input(value: &str) -> String {
    let value = strip_nul(value);
    let value = value.trim();
    if is_placeholder_config_value(value) {
        String::new()
    } else {
        value.to_string()
    }
}

fn list_key_cell_to_input(value: &str) -> String {
    let value = strip_nul(value);
    let value = value.trim();
    if is_placeholder_config_value(value) {
        return String::new();
    }
    value.strip_prefix("已配置 ").unwrap_or(value).to_string()
}

fn clean_provider_text(value: &str) -> String {
    value
        .chars()
        .filter(|ch| !ch.is_control())
        .collect::<String>()
        .trim()
        .to_string()
}

fn strip_nul(value: &str) -> String {
    value.chars().filter(|ch| *ch != '\0').collect()
}

fn config_text_value(value: &str) -> Option<String> {
    let value = strip_nul(value).trim().to_string();
    (!is_placeholder_config_value(&value)).then_some(value)
}

fn is_placeholder_config_value(value: &str) -> bool {
    let value = value.trim();
    value.is_empty() || value.contains("未配")
}

fn apply_provider_to_form(handles: &UiHandles, provider: &CodexAppProviderStatus, overwrite: bool) {
    if overwrite || handles.provider_name.get_value().trim().is_empty() {
        set_combo_value_if_changed(&handles.provider_name, &provider.name);
    }
    if overwrite || handles.provider_base_url.get_value().trim().is_empty() {
        let base_url = provider
            .base_url
            .as_deref()
            .and_then(config_text_value)
            .unwrap_or_default();
        change_text_value_if_changed(&handles.provider_base_url, &base_url);
    }
    if overwrite || handles.provider_key.get_value().trim().is_empty() {
        let key = provider
            .key
            .as_deref()
            .and_then(config_text_value)
            .map(|value| masked_secret(&value))
            .unwrap_or_default();
        change_text_value_if_changed(&handles.provider_key, &key);
    }
}

fn set_combo_value_if_changed(input: &ComboBox, value: &str) {
    if input.get_value() == value {
        return;
    }
    input.set_value(value);
}

fn change_text_value_if_changed(input: &TextCtrl, value: &str) {
    if input.get_value() == value {
        return;
    }
    input.change_value(value);
}

fn clear_provider_list_selection(list: &ListCtrl) {
    loop {
        let selected = list.get_first_selected_item();
        if selected < 0 {
            break;
        }
        if !list.set_item_state(
            selected as i64,
            ListItemState::None,
            ListItemState::Selected,
        ) {
            break;
        }
    }
}

fn set_actions_enabled(handles: &UiHandles, enabled: bool) {
    handles.change_bot_button.enable(enabled);
    handles.configure_button.enable(enabled);
    handles.new_provider_button.enable(enabled);
    handles.save_provider_button.enable(enabled);
    handles.delete_provider_button.enable(enabled);
    handles.stop_bridge_button.enable(enabled);
    handles.uninstall_button.enable(enabled);
}

#[derive(Clone, Copy)]
enum StateTone {
    Ok,
    Warn,
    Error,
    Muted,
}

impl StateTone {
    fn colour(self) -> Colour {
        match self {
            StateTone::Ok => Colour::rgb(28, 127, 89),
            StateTone::Warn => Colour::rgb(169, 104, 24),
            StateTone::Error => Colour::rgb(185, 55, 55),
            StateTone::Muted => Colour::rgb(102, 110, 122),
        }
    }
}

fn set_status_panel(panel: &StatusPanel, state: &str, detail: &str, tone: StateTone) {
    if panel.state.get_label() == state && panel.detail.get_label() == detail {
        return;
    }

    let title_colour = Colour::rgb(91, 100, 114);
    panel.panel.set_background_color(Colour::rgb(255, 255, 255));
    if panel.title.get_foreground_color() != title_colour {
        panel
            .icon
            .set_bitmap(&status_icon_bitmap(panel.icon_kind, 34));
    }
    panel.title.set_foreground_color(title_colour);
    panel.marker.set_foreground_color(tone.colour());
    panel.state.set_label(state);
    panel.state.set_foreground_color(tone.colour());
    panel.detail.set_label(detail);
    panel
        .detail
        .set_foreground_color(Colour::rgb(103, 111, 124));
    panel.detail.wrap(220);
}

fn set_status_panel_title(panel: &StatusPanel, title: &str) {
    if panel.title.get_label() != title {
        panel.title.set_label(title);
    }
}

fn set_disabled_status_panel(panel: &StatusPanel, state: &str, detail: &str) {
    if panel.state.get_label() == state && panel.detail.get_label() == detail {
        return;
    }

    let muted = Colour::rgb(145, 151, 160);
    panel.panel.set_background_color(Colour::rgb(242, 244, 247));
    if panel.title.get_foreground_color() != muted {
        panel
            .icon
            .set_bitmap(&disabled_status_icon_bitmap(panel.icon_kind, 34));
    }
    panel.title.set_foreground_color(muted);
    panel.marker.set_foreground_color(muted);
    panel.state.set_label(state);
    panel.state.set_foreground_color(muted);
    panel.detail.set_label(detail);
    panel.detail.set_foreground_color(muted);
    panel.detail.wrap(190);
}

fn feishu_bot_display_name(snapshot: &DashboardSnapshot) -> Option<String> {
    snapshot
        .feishu_bot
        .as_ref()
        .and_then(|bot| bot.display_name.as_deref())
        .or_else(|| {
            snapshot
                .config
                .as_ref()
                .map(|config| config.feishu.display_name.as_str())
        })
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

fn feishu_status_title(bot_name: Option<&str>) -> String {
    bot_name
        .map(|name| format!("飞书：{name}"))
        .unwrap_or_else(|| "飞书".to_string())
}

fn feishu_state_label(state: &str, bot_name: Option<&str>) -> String {
    bot_name
        .map(|name| format!("{state} · {name}"))
        .unwrap_or_else(|| state.to_string())
}

fn codex_remote_detail(remote: &RemoteControlStatus) -> String {
    if let Some(thread_id) = &remote.current_thread_id {
        return format!("当前 thread: {}", short_id(thread_id));
    }
    if let Some(server_name) = &remote.server_name {
        return format!("设备: {server_name}");
    }
    if let Some(err) = &remote.last_error {
        return format!("最近错误: {err}");
    }
    "remote-control 已连接。".to_string()
}

fn qr_bitmap(value: &str) -> Option<(Bitmap, i32)> {
    let code = QrCode::new(value.as_bytes()).ok()?;
    const TARGET_PIXELS: usize = 560;
    let quiet_zone = 4usize;
    let cells = code.width() + quiet_zone * 2;
    let module_size = (TARGET_PIXELS / cells).clamp(3, 12);
    let image_size = cells * module_size;
    let mut rgba = vec![255u8; image_size * image_size * 4];

    for y in 0..image_size {
        for x in 0..image_size {
            let cell_x = x / module_size;
            let cell_y = y / module_size;
            let dark = cell_x >= quiet_zone
                && cell_y >= quiet_zone
                && cell_x < quiet_zone + code.width()
                && cell_y < quiet_zone + code.width()
                && code[(cell_x - quiet_zone, cell_y - quiet_zone)] == Color::Dark;

            let offset = (y * image_size + x) * 4;
            let value = if dark { 0 } else { 255 };
            rgba[offset] = value;
            rgba[offset + 1] = value;
            rgba[offset + 2] = value;
            rgba[offset + 3] = 255;
        }
    }

    Bitmap::from_rgba(&rgba, image_size as u32, image_size as u32)
        .map(|bitmap| (bitmap, image_size as i32))
}

fn show_onboard_dialog(parent: &Frame, api: ApiClient) {
    let start = match api.start_feishu_onboard() {
        Ok(start) => start,
        Err(err) => {
            show_error(parent, &err);
            return;
        }
    };

    let dialog = Dialog::builder(parent, "扫码使用新机器人")
        .with_style(DialogStyle::DefaultDialogStyle | DialogStyle::ResizeBorder)
        .with_size(660, 760)
        .build();
    dialog.set_min_size(Size::new(560, 660));
    dialog.set_background_color(Colour::rgb(255, 255, 255));

    let panel = Panel::builder(&dialog).build();
    panel.set_background_color(Colour::rgb(255, 255, 255));
    let sizer = BoxSizer::builder(Orientation::Vertical).build();

    let title = StaticText::builder(&panel)
        .with_label("请使用飞书扫码")
        .build();
    title.set_foreground_color(Colour::rgb(21, 25, 31));
    sizer.add(&title, 0, SizerFlag::All, 18);

    if let Some((bitmap, qr_size)) = qr_bitmap(&start.verification_uri_complete) {
        let qr_panel = Panel::builder(&panel).build();
        qr_panel.set_background_color(Colour::rgb(255, 255, 255));
        qr_panel.set_min_size(Size::new(500, 500));

        let qr = StaticBitmap::builder(&qr_panel)
            .with_bitmap(Some(bitmap))
            .with_scale_mode(Some(ScaleMode::AspectFit))
            .with_size(Size::new(qr_size.max(500), qr_size.max(500)))
            .build();
        qr.set_min_size(Size::new(500, 500));

        let qr_sizer = BoxSizer::builder(Orientation::Vertical).build();
        qr_sizer.add(&qr, 1, SizerFlag::Expand | SizerFlag::All, 0);
        qr_panel.set_sizer(qr_sizer, true);

        sizer.add(
            &qr_panel,
            1,
            SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
            12,
        );
    } else {
        let qr_error = StaticText::builder(&panel)
            .with_label("二维码生成失败，请使用浏览器打开链接。")
            .build();
        qr_error.set_foreground_color(Colour::rgb(185, 55, 55));
        sizer.add(
            &qr_error,
            0,
            SizerFlag::AlignCenterHorizontal | SizerFlag::Top | SizerFlag::Bottom,
            80,
        );
    }

    let fallback_link = HyperlinkCtrl::builder(&panel)
        .with_label("扫码失败？打开飞书确认链接")
        .with_url(&start.verification_uri_complete)
        .build();
    sizer.add(
        &fallback_link,
        0,
        SizerFlag::AlignCenterHorizontal | SizerFlag::Bottom,
        12,
    );

    let info = StaticText::builder(&panel)
        .with_label("扫码完成后会自动关闭。")
        .build();
    info.set_foreground_color(Colour::rgb(88, 96, 108));
    info.wrap(600);
    sizer.add(
        &info,
        0,
        SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
        18,
    );

    let buttons = BoxSizer::builder(Orientation::Horizontal).build();
    let close_button = Button::builder(&panel).with_label("关闭").build();
    buttons.add_stretch_spacer(1);
    buttons.add(&close_button, 1, SizerFlag::Expand, 0);
    sizer.add_sizer(
        &buttons,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
        18,
    );

    panel.set_sizer(sizer, true);
    let dialog_sizer = BoxSizer::builder(Orientation::Vertical).build();
    dialog_sizer.add(&panel, 1, SizerFlag::Expand, 0);
    dialog.set_sizer(dialog_sizer, true);
    dialog.center();

    let timer = Timer::new(&dialog);
    {
        let api = api.clone();
        let device_code = start.device_code.clone();
        let dialog = dialog;
        timer.on_tick(move |_| match api.poll_feishu_onboard(&device_code) {
            Ok(result) if result.done => {
                dialog.end_modal(ID_OK);
            }
            Ok(result) => {
                if is_feishu_onboard_pending(result.error.as_ref()) {
                    info.set_label("扫码完成后会自动关闭。");
                } else if result.error.is_some() {
                    info.set_label("接入失败，请关闭后重试。");
                }
            }
            Err(_) => {
                info.set_label("接入失败，请关闭后重试。");
            }
        });
    }
    timer.start(1500, false);

    {
        let dialog = dialog;
        close_button.on_click(move |_| dialog.end_modal(ID_CANCEL));
    }

    dialog.show_modal();
    timer.stop();
    dialog.destroy();
}

fn is_feishu_onboard_pending(error: Option<&serde_json::Value>) -> bool {
    matches!(
        error.and_then(|value| value.as_str()),
        Some("authorization_pending" | "slow_down")
    )
}

fn show_about_dialog(parent: &Frame) {
    let dialog = Dialog::builder(parent, "About Codex Remote")
        .with_style(DialogStyle::DefaultDialogStyle)
        .with_size(520, 260)
        .build();
    dialog.set_icon(&app_icon_bitmap(48));
    dialog.set_background_color(Colour::rgb(255, 255, 255));

    let panel = Panel::builder(&dialog).build();
    panel.set_background_color(Colour::rgb(255, 255, 255));
    let sizer = BoxSizer::builder(Orientation::Vertical).build();

    let title = StaticText::builder(&panel)
        .with_label(&format!("Codex Remote {}", env!("CARGO_PKG_VERSION")))
        .build();
    title.set_foreground_color(Colour::rgb(21, 25, 31));
    sizer.add(
        &title,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        18,
    );

    let description = StaticText::builder(&panel)
        .with_label("本地 remote-control backend + 飞书桥接。")
        .build();
    description.set_foreground_color(Colour::rgb(88, 96, 108));
    description.wrap(460);
    sizer.add(
        &description,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        18,
    );

    let link = HyperlinkCtrl::builder(&panel)
        .with_label(PROJECT_HOME_URL)
        .with_url(PROJECT_HOME_URL)
        .build();
    sizer.add(
        &link,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        18,
    );

    let buttons = BoxSizer::builder(Orientation::Horizontal).build();
    let close_button = Button::builder(&panel).with_label("关闭").build();
    buttons.add_stretch_spacer(1);
    buttons.add(&close_button, 0, SizerFlag::AlignLeft, 0);
    sizer.add_sizer(&buttons, 0, SizerFlag::Expand | SizerFlag::All, 18);

    panel.set_sizer(sizer, true);
    let dialog_sizer = BoxSizer::builder(Orientation::Vertical).build();
    dialog_sizer.add(&panel, 1, SizerFlag::Expand, 0);
    dialog.set_sizer(dialog_sizer, true);
    dialog.center();

    {
        let dialog = dialog;
        close_button.on_click(move |_| dialog.end_modal(ID_OK));
    }

    dialog.show_modal();
    dialog.destroy();
}

fn show_info(parent: &dyn WxWidget, message: &str) {
    MessageDialog::builder(parent, message, "Codex Remote")
        .with_style(MessageDialogStyle::OK | MessageDialogStyle::IconInformation)
        .build()
        .show_modal();
}

fn show_error(parent: &dyn WxWidget, message: &str) {
    MessageDialog::builder(parent, message, "Codex Remote")
        .with_style(MessageDialogStyle::OK | MessageDialogStyle::IconError)
        .build()
        .show_modal();
}

fn confirm_open_update_release(parent: &dyn WxWidget, message: &str) -> bool {
    MessageDialog::builder(parent, message, "Codex Remote 更新")
        .with_style(MessageDialogStyle::YesNo | MessageDialogStyle::IconQuestion)
        .build()
        .show_modal()
        == ID_YES
}

fn confirm_uninstall_codex_app_config(parent: &dyn WxWidget) -> bool {
    MessageDialog::builder(
        parent,
        "卸载会移除本工具写入的 chatgpt_base_url、本地认证信息和 Codex App 环境变量。确认继续？",
        "卸载 Codex App 配置",
    )
    .with_style(MessageDialogStyle::YesNo | MessageDialogStyle::IconQuestion)
    .build()
    .show_modal()
        == ID_YES
}

fn confirm_delete_provider(parent: &dyn WxWidget, provider_name: &str) -> bool {
    MessageDialog::builder(
        parent,
        &format!("删除 provider `{provider_name}`？如果它正在使用中，也会取消当前 provider 设置。"),
        "删除 Provider",
    )
    .with_style(MessageDialogStyle::YesNo | MessageDialogStyle::IconQuestion)
    .build()
    .show_modal()
        == ID_YES
}

fn short_id(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() <= 18 {
        trimmed.to_string()
    } else {
        format!("{}...", &trimmed[..18])
    }
}
