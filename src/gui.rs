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
    time::Duration,
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
const DASHBOARD_REFRESH_INTERVAL_MS: i32 = 2500;
const DASHBOARD_RESULT_POLL_MS: i32 = 100;
const GUI_CONNECT_TIMEOUT: Duration = Duration::from_millis(250);
const GUI_STATUS_TIMEOUT: Duration = Duration::from_millis(650);
const GUI_ACTION_TIMEOUT: Duration = Duration::from_secs(2);
const ID_MENU_CLOSE_WINDOW: i32 = 10_001;
const ID_MENU_MINIMIZE: i32 = 10_002;

type FrameTimerStore = Rc<RefCell<Option<Timer<Frame>>>>;

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

    let frame = Frame::builder()
        .with_title("Codex Remote")
        .with_size(Size::new(1100, 760))
        .build();
    frame.set_icon(&app_icon_bitmap(48));
    install_system_menu(&frame);
    frame.set_background_color(Colour::rgb(246, 247, 250));
    let status_bar = StatusBar::builder(&frame)
        .with_fields_count(3)
        .with_status_widths(vec![-2, -1, -1])
        .add_initial_text(0, "本地服务启动中")
        .add_initial_text(1, "日志写入本地文件")
        .add_initial_text(2, "自动刷新 2.5s")
        .build();

    let root = Panel::builder(&frame).build();
    root.set_background_color(Colour::rgb(246, 247, 250));

    let root_sizer = BoxSizer::builder(Orientation::Vertical).build();

    let header_panel = Panel::builder(&root)
        .with_style(PanelStyle::BorderNone)
        .build();
    header_panel.set_background_color(Colour::rgb(246, 247, 250));
    let header = BoxSizer::builder(Orientation::Horizontal).build();
    let header_copy = BoxSizer::builder(Orientation::Vertical).build();
    let title = StaticText::builder(&header_panel)
        .with_label("Codex Remote")
        .build();
    title.set_foreground_color(Colour::rgb(24, 28, 35));
    header_copy.add(&title, 0, SizerFlag::Bottom, 4);
    let subtitle = StaticText::builder(&header_panel)
        .with_label("本地 remote-control backend + 飞书桥接")
        .build();
    subtitle.set_foreground_color(Colour::rgb(91, 100, 114));
    header_copy.add(&subtitle, 0, SizerFlag::Expand, 0);
    header.add_sizer(&header_copy, 1, SizerFlag::Expand, 0);

    let endpoint = StaticText::builder(&header_panel)
        .with_label(&format!("服务地址 {}", api.base_url))
        .build();
    endpoint.set_foreground_color(Colour::rgb(103, 111, 124));
    header.add(&endpoint, 0, SizerFlag::AlignCenterVertical, 0);
    header_panel.set_sizer(header, true);
    root_sizer.add(
        &header_panel,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        18,
    );

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

    let codex_status_static_box = StaticBox::builder(&codex_page)
        .with_label("Codex App 配置")
        .build();
    let codex_status_box =
        StaticBoxSizerBuilder::new_with_box(&codex_status_static_box, Orientation::Vertical)
            .build();
    let codex_config_state = StaticText::builder(&codex_status_static_box)
        .with_label("正在读取 ~/.codex 配置状态")
        .build();
    codex_config_state.set_foreground_color(Colour::rgb(75, 84, 98));
    codex_config_state.wrap(980);
    codex_status_box.add(
        &codex_config_state,
        0,
        SizerFlag::Expand | SizerFlag::All,
        12,
    );
    codex_sizer.add_sizer(
        &codex_status_box,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        10,
    );

    let config_static_box = StaticBox::builder(&codex_page)
        .with_label("写入 Codex 接入配置")
        .build();
    let config_box =
        StaticBoxSizerBuilder::new_with_box(&config_static_box, Orientation::Vertical).build();
    let config_header = BoxSizer::builder(Orientation::Horizontal).build();
    let config_hint = StaticText::builder(&config_static_box)
        .with_label("填写第三方模型服务，然后写入 Codex App 本地配置。")
        .build();
    config_hint.set_foreground_color(Colour::rgb(34, 39, 47));
    config_hint.wrap(760);
    config_header.add(&config_hint, 1, SizerFlag::AlignCenterVertical, 0);
    let uninstall_button = Button::builder(&config_static_box)
        .with_label("卸载配置")
        .build();
    uninstall_button.set_tooltip("移除本工具写入的 Codex App 本地接入配置");
    config_header.add(
        &uninstall_button,
        0,
        SizerFlag::AlignCenterVertical | SizerFlag::Right,
        8,
    );
    let configure_button = Button::builder(&config_static_box)
        .with_label("写入配置")
        .build();
    configure_button.set_tooltip("写入 Codex Remote 使用的本地 provider 配置");
    config_header.add(&configure_button, 0, SizerFlag::AlignCenterVertical, 0);
    config_box.add_sizer(&config_header, 0, SizerFlag::Expand | SizerFlag::All, 12);

    let provider_help = StaticText::builder(&config_static_box)
        .with_label("Codex App 会使用这里的 Base URL 和 API Key。Provider 可以选择已有配置，也可以输入新名字新建。")
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
    codex_sizer.add_sizer(
        &config_box,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        10,
    );
    codex_sizer.add_stretch_spacer(1);
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
    stop_bridge_button.set_tooltip("停止飞书桥接，不删除已保存的机器人配置");
    let change_bot_button = Button::builder(&feishu_static_box)
        .with_label("更换机器人")
        .build();
    change_bot_button.set_tooltip("重新进入飞书扫码接入流程");
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

    let system_page = Panel::builder(&notebook)
        .with_style(PanelStyle::TabTraversal)
        .build();
    system_page.set_background_color(Colour::rgb(250, 251, 253));
    let system_sizer = BoxSizer::builder(Orientation::Vertical).build();
    let system_static_box = StaticBox::builder(&system_page)
        .with_label("本地服务")
        .build();
    let system_box =
        StaticBoxSizerBuilder::new_with_box(&system_static_box, Orientation::Vertical).build();
    let service_text = StaticText::builder(&system_static_box)
        .with_label("Codex Remote 会在 GUI 打开时接管并重启本地 backend，避免旧版本服务残留；GUI 退出时会关闭本地 backend，并清除本次写入的 Codex App 环境变量。不会安装开机启动项，也不会修改系统常驻服务。")
        .build();
    service_text.set_foreground_color(Colour::rgb(82, 91, 105));
    service_text.wrap(760);
    system_box.add(&service_text, 0, SizerFlag::Expand | SizerFlag::All, 12);
    let refresh_button = Button::builder(&system_static_box)
        .with_label("检测状态")
        .build();
    refresh_button.set_tooltip("立即刷新本地服务、飞书和 Codex App 连接状态");
    let start_daemon_button = Button::builder(&system_static_box)
        .with_label("启动本地服务")
        .build();
    start_daemon_button.set_tooltip("本次会话启动 codex-remote daemon，不安装开机启动项");
    let system_buttons = BoxSizer::builder(Orientation::Horizontal).build();
    system_buttons.add_stretch_spacer(1);
    system_buttons.add(&start_daemon_button, 0, SizerFlag::Right, 8);
    system_buttons.add(&refresh_button, 0, SizerFlag::Right, 0);
    system_box.add_sizer(&system_buttons, 0, SizerFlag::Expand | SizerFlag::All, 12);
    system_sizer.add_sizer(
        &system_box,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        14,
    );
    system_sizer.add_stretch_spacer(1);
    system_page.set_sizer(system_sizer, true);

    notebook.add_page(&codex_page, "Codex 接入", true, None);
    notebook.add_page(&feishu_page, "飞书", false, None);
    notebook.add_page(&system_page, "本地服务", false, None);

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
        status_bar,
        service_status,
        feishu_status,
        codex_status,
        vscode_status,
        feishu_state,
        feishu_detail,
        feishu_meta,
        codex_config_state,
        change_bot_button,
        stop_bridge_button,
        uninstall_button,
        configure_button,
        refresh_button,
        start_daemon_button,
        provider_name,
        provider_base_url,
        provider_key,
        provider_catalog,
    };

    let daemon_child: Rc<RefCell<Option<Child>>> = Rc::new(RefCell::new(None));
    let dashboard_refresh = DashboardRefresh::new();
    let gui_timers = GuiTimers::new();
    show_dashboard_starting(&handles);
    show_local_codex_app_config_preview(&handles, &api);

    {
        let api = api.clone();
        let handles = handles;
        let dashboard_refresh = dashboard_refresh.clone();
        refresh_button.on_click(move |_| {
            handles.status_bar.set_status_text("状态刷新中", 0);
            schedule_dashboard_refresh(&api, &dashboard_refresh);
        });
    }

    {
        let api = api.clone();
        let handles = handles;
        let frame = frame;
        let daemon_child = daemon_child.clone();
        let dashboard_refresh = dashboard_refresh.clone();
        let gui_timers = gui_timers.clone();
        start_daemon_button.on_click(move |_| {
            start_daemon_for_gui_async(
                &api,
                &handles,
                &frame,
                &daemon_child,
                &dashboard_refresh,
                &gui_timers,
            );
        });
    }

    {
        let api = api.clone();
        let dashboard_refresh = dashboard_refresh.clone();
        let provider_name = provider_name;
        let provider_base_url = provider_base_url;
        let provider_key = provider_key;
        let frame = frame;
        configure_button.on_click(move |_| {
            let request = ConfigureRequest {
                provider_name: Some(provider_name.get_value()),
                provider_base_url: Some(provider_base_url.get_value()),
                provider_key: Some(provider_key.get_value()),
                model: None,
            };
            match api.configure_codex_app(&request) {
                Ok(_) => {
                    show_info(
                        &frame,
                        "配置已写入。请重启 Codex App，然后在 App 里打开 remote-control；VS Code 插件也可以接入。",
                    );
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
            let selected = provider_name.get_value();
            let Some(snapshot) = cached_dashboard_snapshot(&dashboard_refresh) else {
                return;
            };
            if let Some(provider) = find_provider(&snapshot, &selected) {
                apply_provider_to_form(&handles, &provider, true);
            }
        });
    }

    {
        let api = api.clone();
        let dashboard_refresh = dashboard_refresh.clone();
        let frame = frame;
        stop_bridge_button.on_click(move |_| match api.stop_bridge() {
            Ok(_) => {
                show_info(&frame, "飞书接入已断开。");
                schedule_dashboard_refresh(&api, &dashboard_refresh);
            }
            Err(err) => show_error(&frame, &err),
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
        let gui_timers = gui_timers.clone();
        frame.on_close(move |_| {
            gui_timers.stop_all();
            stop_daemon_on_exit(&api, &daemon_child);
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

fn install_system_menu(frame: &Frame) {
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
        .append_item(ID_ABOUT, "&About Codex Remote", "About Codex Remote")
        .build();
    let menu_bar = MenuBar::builder()
        .append(file_menu, "&File")
        .append(help_menu, "&Help")
        .build();
    frame.set_menu_bar(menu_bar);

    let frame = *frame;
    frame.on_menu_selected(move |event| match event.get_id() {
        ID_EXIT | ID_MENU_CLOSE_WINDOW => frame.close(true),
        ID_MENU_MINIMIZE => frame.iconize(true),
        ID_ABOUT => show_info(
            &frame,
            "Codex Remote\n本地 remote-control backend + 飞书桥接。",
        ),
        _ => event.skip(true),
    });
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

struct StartupResult {
    child: Child,
}

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
        handles.status_bar.set_status_text("本地服务正在启动", 0);
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
        let result = result.clone();
        thread::spawn(move || {
            let startup = restart_daemon_for_gui(&api).map(|child| StartupResult { child });
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
                Ok(startup) => {
                    replace_managed_daemon(&daemon_child, startup.child);
                    repair_codex_app_gui_environment_async(&api, &dashboard_refresh);
                    handles
                        .status_bar
                        .set_status_text("本地服务已启动，正在读取配置", 0);
                }
                Err(err) => {
                    handles
                        .status_bar
                        .set_status_text(&format!("本地服务启动失败：{err}"), 0);
                    set_actions_enabled(&handles, false);
                    handles.start_daemon_button.enable(true);
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
    let backend_url = api.url("/backend-api");
    let child = daemon_child.borrow_mut().take();

    let api = api.clone();
    thread::spawn(move || {
        clear_codex_app_gui_environment(&backend_url);
        let _ = api.shutdown();
        wait_for_daemon_offline(&api, 3);
        if let Some(mut child) = child {
            if child.try_wait().ok().flatten().is_none() {
                let _ = child.kill();
            }
            let _ = child.wait();
        }
        if api.is_online() {
            stop_daemon_by_port(&api);
        }
    });
}

fn clear_codex_app_gui_environment(backend_url: &str) {
    let _ = crate::codex_app_config::uninstall_gui_environment(backend_url);
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

    fn is_online(&self) -> bool {
        self.get_quick::<serde_json::Value>("/api/status").is_ok()
    }

    fn post_empty<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        let text = self.request_text(self.http.post(self.url(path)))?;
        serde_json::from_str(&text).map_err(|err| format!("{path} 返回数据无法解析：{err}"))
    }

    fn post_json<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, String> {
        let text = self.request_text(self.http.post(self.url(path)).json(body))?;
        serde_json::from_str(&text).map_err(|err| format!("{path} 返回数据无法解析：{err}"))
    }

    fn request_text(&self, request: reqwest::blocking::RequestBuilder) -> Result<String, String> {
        let response = request
            .send()
            .map_err(|err| format!("无法连接本地服务 {}：{err}", self.base_url))?;
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
            Err(err) => {
                return DashboardSnapshot {
                    service_online: false,
                    error: Some(err),
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

        DashboardSnapshot {
            service_online: true,
            error: None,
            config: join_optional(config),
            backend: join_optional(backend),
            remote: join_optional(remote),
            codex_app: join_optional(codex_app),
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
        self.post_json("/api/codex-app/configure", request)
    }

    fn uninstall_codex_app(&self) -> Result<serde_json::Value, String> {
        self.post_empty("/api/codex-app/uninstall")
    }

    fn repair_codex_app_gui_environment(&self) -> Result<serde_json::Value, String> {
        self.post_empty("/api/codex-app/repair-gui-environment")
    }

    fn stop_bridge(&self) -> Result<serde_json::Value, String> {
        self.post_empty("/api/bridge/stop")
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
    status_bar: StatusBar,
    service_status: StatusPanel,
    feishu_status: StatusPanel,
    codex_status: StatusPanel,
    vscode_status: StatusPanel,
    feishu_state: StaticText,
    feishu_detail: StaticText,
    feishu_meta: StaticText,
    codex_config_state: StaticText,
    change_bot_button: Button,
    stop_bridge_button: Button,
    uninstall_button: Button,
    configure_button: Button,
    refresh_button: Button,
    start_daemon_button: Button,
    provider_name: ComboBox,
    provider_base_url: TextCtrl,
    provider_key: TextCtrl,
    provider_catalog: StaticText,
}

#[derive(Clone)]
struct DashboardRefresh {
    in_flight: Arc<AtomicBool>,
    result: Arc<Mutex<Option<(u64, DashboardSnapshot)>>>,
    last_snapshot: Arc<Mutex<Option<DashboardSnapshot>>>,
    daemon_starting: Arc<AtomicBool>,
    generation: Arc<AtomicU64>,
}

impl DashboardRefresh {
    fn new() -> Self {
        Self {
            in_flight: Arc::new(AtomicBool::new(false)),
            result: Arc::new(Mutex::new(None)),
            last_snapshot: Arc::new(Mutex::new(None)),
            daemon_starting: Arc::new(AtomicBool::new(false)),
            generation: Arc::new(AtomicU64::new(0)),
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
    error: Option<String>,
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
    allowed_open_ids: Vec<String>,
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
    codex_home: String,
    configured: bool,
    config_ok: bool,
    auth_ok: bool,
    gui_api_base: GuiApiBaseStatus,
    #[serde(default)]
    remote_control_switch: Option<CodexAppRemoteControlSwitchStatus>,
    provider: Option<CodexAppProviderStatus>,
    #[serde(default)]
    providers: Vec<CodexAppProviderStatus>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexAppRemoteControlSwitchStatus {
    configured: bool,
    error: Option<String>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexAppProviderStatus {
    name: String,
    base_url: Option<String>,
    key: Option<String>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GuiApiBaseStatus {
    #[serde(default)]
    configured: bool,
    #[serde(default)]
    login_issuer_configured: bool,
    value: Option<String>,
    #[serde(default)]
    login_issuer_value: Option<String>,
    error: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConfigureRequest {
    provider_name: Option<String>,
    provider_base_url: Option<String>,
    provider_key: Option<String>,
    model: Option<String>,
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
    handles
        .codex_config_state
        .set_label("正在启动本地服务，界面保持可操作。");
    handles.codex_config_state.wrap(980);
    handles.codex_config_state.layout();
    handles.status_bar.set_status_text("本地服务：启动中", 0);
    handles.status_bar.set_status_text("飞书：等待服务", 1);
    handles.status_bar.set_status_text("Codex App：等待服务", 2);
    set_actions_enabled(handles, false);
    handles.start_daemon_button.enable(false);
}

fn show_local_codex_app_config_preview(handles: &UiHandles, api: &ApiClient) {
    if CODEX_APP_GUI_UNSUPPORTED {
        return;
    }
    let status = crate::codex_app_config::inspect_codex_app_config(None, &api.url("/backend-api"));
    let snapshot = DashboardSnapshot {
        service_online: false,
        codex_app: Some(local_codex_app_status(status)),
        ..DashboardSnapshot::default()
    };
    handles
        .codex_config_state
        .set_label(&codex_app_detail(&snapshot));
    handles.codex_config_state.wrap(980);
    handles.codex_config_state.layout();
    fill_provider_form_if_empty(handles, &snapshot);
}

fn local_codex_app_status(status: crate::codex_app_config::CodexAppConfigStatus) -> CodexAppStatus {
    CodexAppStatus {
        codex_home: status.codex_home.display().to_string(),
        configured: status.configured,
        config_ok: status.config_ok,
        auth_ok: status.auth_ok,
        gui_api_base: GuiApiBaseStatus {
            configured: status.gui_api_base.configured,
            login_issuer_configured: status.gui_api_base.login_issuer_configured,
            value: status.gui_api_base.value,
            login_issuer_value: status.gui_api_base.login_issuer_value,
            error: status.gui_api_base.error,
        },
        remote_control_switch: Some(CodexAppRemoteControlSwitchStatus {
            configured: status.remote_control_switch.configured,
            error: status.remote_control_switch.error,
        }),
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
            "点击“启动本地服务”后再连接 VS Code 插件。",
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
            .set_label("请先启动 codex-remote 后端。");
        handles.feishu_meta.set_label("");
        let config_state = snapshot
            .error
            .as_deref()
            .map(|err| format!("无法读取 Codex App 配置状态。\n本地服务连接错误: {err}"))
            .unwrap_or_else(|| "无法读取 Codex App 配置状态。".to_string());
        handles.codex_config_state.set_label(&config_state);
        handles.codex_config_state.wrap(980);
        handles.codex_config_state.layout();
        handles.status_bar.set_status_text("本地服务：离线", 0);
        handles.status_bar.set_status_text("飞书：不可用", 1);
        handles.status_bar.set_status_text("Codex App：不可用", 2);
        set_actions_enabled(handles, false);
        handles.start_daemon_button.enable(true);
        return;
    }

    set_actions_enabled(handles, true);
    handles.start_daemon_button.enable(false);

    if let Some(status) = &snapshot.status {
        set_status_panel(
            &handles.service_status,
            "运行中",
            &format!("监听 {}", status.bind),
            StateTone::Ok,
        );
        handles
            .status_bar
            .set_status_text(&format!("本地服务：{}", status.bind), 0);
    }

    let feishu_configured = snapshot
        .backend
        .as_ref()
        .map(|backend| backend.feishu_configured)
        .or_else(|| {
            snapshot
                .config
                .as_ref()
                .map(|config| !config.feishu.app_id.is_empty())
        })
        .unwrap_or(false);
    let bridge_enabled = snapshot
        .backend
        .as_ref()
        .map(|backend| backend.enabled)
        .or_else(|| snapshot.config.as_ref().map(|config| config.bridge.enabled))
        .unwrap_or(false);

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
    let (feishu_state, feishu_detail, feishu_tone) = if !feishu_configured {
        (
            "未接入",
            "扫码接入飞书机器人后才会启动飞书桥接。",
            StateTone::Warn,
        )
    } else if !bridge_enabled {
        (
            "已断开",
            "机器人已保存，点击“更换机器人”可重新接入。",
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

    set_status_panel(
        &handles.feishu_status,
        feishu_state,
        feishu_detail,
        feishu_tone,
    );
    handles
        .status_bar
        .set_status_text(&format!("飞书：{feishu_state}"), 1);
    handles.feishu_state.set_label(feishu_state);
    handles
        .feishu_state
        .set_foreground_color(feishu_tone.colour());
    handles.feishu_detail.set_label(feishu_detail);
    handles.feishu_detail.wrap(300);
    handles
        .stop_bridge_button
        .enable(feishu_configured && bridge_enabled);

    let feishu_meta = match (
        &snapshot.config,
        feishu_ws.and_then(|ws| ws.last_error.as_deref()),
    ) {
        (Some(config), Some(err)) if !err.is_empty() => format!(
            "App ID: {}\n允许用户: {}\n最近错误: {err}",
            short_id(&config.feishu.app_id),
            config.feishu.allowed_open_ids.len()
        ),
        (Some(config), _) if !config.feishu.app_id.is_empty() => format!(
            "App ID: {}\n允许用户: {}",
            short_id(&config.feishu.app_id),
            config.feishu.allowed_open_ids.len()
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
        handles
            .status_bar
            .set_status_text("Codex App：当前平台暂不可用", 2);
    } else if codex_control_ready {
        let detail = snapshot
            .remote
            .as_ref()
            .map(codex_remote_detail)
            .unwrap_or_else(|| "Codex App remote-control 已连接。".to_string());
        set_status_panel(&handles.codex_status, "已连接", &detail, StateTone::Ok);
        handles.status_bar.set_status_text("Codex App：已连接", 2);
    } else if remote_connected {
        set_status_panel(
            &handles.codex_status,
            "初始化中",
            "Codex App 已打开控制通道，正在完成 remote-control 初始化。",
            StateTone::Warn,
        );
        handles.status_bar.set_status_text("Codex App：初始化中", 2);
    } else if codex_configured {
        set_status_panel(
            &handles.codex_status,
            "未打开控制",
            "配置已注入，请在 Codex App 里打开“控制这台 Mac”。",
            StateTone::Warn,
        );
        handles
            .status_bar
            .set_status_text("Codex App：未打开控制", 2);
    } else {
        set_status_panel(
            &handles.codex_status,
            "未注入",
            "填写 Base URL 和 API Key 后写入配置。",
            StateTone::Warn,
        );
        handles.status_bar.set_status_text("Codex App：未注入", 2);
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

    handles
        .codex_config_state
        .set_label(&codex_app_detail(snapshot));
    handles.codex_config_state.wrap(980);
    handles.codex_config_state.layout();
    fill_provider_form_if_empty(handles, snapshot);
}

fn fill_provider_form_if_empty(handles: &UiHandles, snapshot: &DashboardSnapshot) {
    let Some(status) = snapshot.codex_app.as_ref() else {
        handles
            .provider_catalog
            .set_label("本地服务运行后会读取 ~/.codex/config.toml 里的 provider。");
        handles.provider_catalog.wrap(980);
        handles.provider_catalog.layout();
        return;
    };
    handles
        .provider_catalog
        .set_label(&provider_catalog_label(status));
    handles.provider_catalog.wrap(980);
    handles.provider_catalog.layout();

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

fn combo_box_items(input: &ComboBox) -> Vec<String> {
    (0..input.get_count())
        .filter_map(|index| input.get_string(index))
        .collect()
}

fn provider_catalog_label(status: &CodexAppStatus) -> String {
    if status.providers.is_empty() {
        if let Some(active) = status.provider.as_ref() {
            return format!(
                "当前配置里有 model_provider: {active}，但没有匹配到 provider 详情。可以继续填写 Base URL 和 API Key 补齐。",
                active = active.name.as_str()
            );
        }
        return "没有在 ~/.codex/config.toml 里找到 provider，默认新建 ai-codex。".to_string();
    }

    let names = status
        .providers
        .iter()
        .map(|provider| provider.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    if let Some(active) = status.provider.as_ref() {
        format!(
            "已匹配 provider: {names}。建议选择当前使用的 {active}，也可以直接输入新名字新建。",
            active = active.name.as_str()
        )
    } else {
        format!("已匹配 provider: {names}。请选择原来使用的 provider，也可以直接输入新名字新建。")
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

fn apply_provider_to_form(handles: &UiHandles, provider: &CodexAppProviderStatus, overwrite: bool) {
    if overwrite || handles.provider_name.get_value().trim().is_empty() {
        set_combo_value_if_changed(&handles.provider_name, &provider.name);
    }
    if overwrite || handles.provider_base_url.get_value().trim().is_empty() {
        change_text_value_if_changed(
            &handles.provider_base_url,
            provider.base_url.as_deref().unwrap_or_default(),
        );
    }
    if overwrite || handles.provider_key.get_value().trim().is_empty() {
        change_text_value_if_changed(
            &handles.provider_key,
            provider.key.as_deref().unwrap_or_default(),
        );
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

fn set_actions_enabled(handles: &UiHandles, enabled: bool) {
    handles.change_bot_button.enable(enabled);
    handles.configure_button.enable(enabled);
    handles.refresh_button.enable(true);
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

fn codex_app_detail(snapshot: &DashboardSnapshot) -> String {
    let Some(status) = &snapshot.codex_app else {
        return "无法读取 ~/.codex 配置状态。".to_string();
    };
    if CODEX_APP_GUI_UNSUPPORTED {
        let mut detail = format!(
            "当前平台 Codex App GUI 自动接入暂不可用，请使用 VS Code 插件接入。\n配置会写入到 {}，用于准备本地 provider；是否可用以 VS Code 插件实际连接结果为准。",
            status.codex_home
        );
        if status.configured {
            detail.push_str("\n当前检测到本地配置已写入。");
        } else {
            detail.push_str("\n当前检测到本地配置尚未写入。");
        }
        return detail;
    }
    if status.configured {
        let mut detail = format!("已注入到 {}", status.codex_home);
        if let Some(value) = &status.gui_api_base.value {
            detail.push_str(&format!("\nGUI backend: {value}"));
        }
        return detail;
    }

    let mut parts = Vec::new();
    if !status.config_ok {
        parts.push("Codex App 还没有写入本地连接配置。".to_string());
    }
    if !status.auth_ok {
        parts.push("本地认证信息还没有准备好，请填写 API Key 后写入配置。".to_string());
    }
    if !status.gui_api_base.configured {
        parts.push("Codex App GUI 还没有指向本地服务，请点击写入配置。".to_string());
    }
    if !status.gui_api_base.login_issuer_configured {
        parts.push("本地授权入口还没有写入，请点击写入配置。".to_string());
    }
    if status
        .remote_control_switch
        .as_ref()
        .is_some_and(|remote_control_switch| !remote_control_switch.configured)
    {
        parts.push("Codex App remote-control 持久化开关还没有写入，请点击写入配置。".to_string());
    }
    if let Some(err) = &status.gui_api_base.error {
        parts.push(format!("检查环境变量时遇到问题: {err}"));
    }
    if let Some(err) = status
        .remote_control_switch
        .as_ref()
        .and_then(|remote_control_switch| remote_control_switch.error.as_ref())
    {
        parts.push(format!("检查 remote-control 开关时遇到问题: {err}"));
    }
    if parts.is_empty() {
        "尚未注入 Codex App 配置。".to_string()
    } else {
        parts.join("\n")
    }
}

fn qr_bitmap(value: &str) -> Option<(Bitmap, i32)> {
    let code = QrCode::new(value.as_bytes()).ok()?;
    const TARGET_PIXELS: usize = 384;
    let quiet_zone = 4usize;
    let cells = code.width() + quiet_zone * 2;
    let module_size = (TARGET_PIXELS / cells).clamp(2, 8);
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

    let dialog = Dialog::builder(parent, "更换飞书机器人")
        .with_size(460, 560)
        .build();
    dialog.set_background_color(Colour::rgb(255, 255, 255));
    let sizer = BoxSizer::builder(Orientation::Vertical).build();

    let title = StaticText::builder(&dialog)
        .with_label("请使用飞书扫码")
        .build();
    title.set_foreground_color(Colour::rgb(21, 25, 31));
    sizer.add(&title, 0, SizerFlag::All, 18);

    if let Some((bitmap, qr_size)) = qr_bitmap(&start.verification_uri_complete) {
        let qr = StaticBitmap::builder(&dialog)
            .with_bitmap(Some(bitmap))
            .with_scale_mode(Some(ScaleMode::None))
            .with_size(Size::new(qr_size, qr_size))
            .build();
        qr.set_min_size(Size::new(qr_size, qr_size));
        sizer.add(
            &qr,
            0,
            SizerFlag::AlignCenterHorizontal | SizerFlag::Top | SizerFlag::Bottom,
            12,
        );
    } else {
        let qr_error = StaticText::builder(&dialog)
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

    let info = StaticText::builder(&dialog)
        .with_label("扫码完成后会自动关闭。")
        .build();
    info.set_foreground_color(Colour::rgb(88, 96, 108));
    info.wrap(424);
    sizer.add(
        &info,
        0,
        SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
        18,
    );

    let buttons = BoxSizer::builder(Orientation::Horizontal).build();
    let close_button = Button::builder(&dialog).with_label("关闭").build();
    buttons.add_stretch_spacer(1);
    buttons.add(&close_button, 1, SizerFlag::Expand, 0);
    sizer.add_sizer(
        &buttons,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
        18,
    );

    dialog.set_sizer(sizer, true);

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

fn short_id(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() <= 18 {
        trimmed.to_string()
    } else {
        format!("{}...", &trimmed[..18])
    }
}
