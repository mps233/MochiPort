use std::{
    cell::RefCell,
    env,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    rc::Rc,
    thread,
    time::Duration,
};

use qrcode::{Color, QrCode};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use wxdragon::{prelude::*, timer::Timer};

const DEFAULT_BASE_URL: &str = "http://127.0.0.1:3847";

fn main() {
    if let Err(err) = wxdragon::main(|_| build_ui()) {
        eprintln!("failed to start Codex Remote GUI: {err:?}");
    }
}

fn build_ui() {
    let api = ApiClient::new(default_base_url());

    let frame = Frame::builder()
        .with_title("Codex Remote")
        .with_size(Size::new(920, 640))
        .build();
    frame.set_background_color(Colour::rgb(248, 249, 251));
    let status_bar = StatusBar::builder(&frame)
        .with_fields_count(3)
        .with_status_widths(vec![-2, -1, -1])
        .add_initial_text(0, "本地服务未自动启动")
        .add_initial_text(1, "日志写入本地文件")
        .add_initial_text(2, "自动刷新 2.5s")
        .build();

    let root = Panel::builder(&frame).build();
    root.set_background_color(Colour::rgb(248, 249, 251));

    let root_sizer = BoxSizer::builder(Orientation::Vertical).build();

    let header = BoxSizer::builder(Orientation::Horizontal).build();
    let header_copy = BoxSizer::builder(Orientation::Vertical).build();
    let title = StaticText::builder(&root)
        .with_label("Codex Remote")
        .build();
    title.set_foreground_color(Colour::rgb(24, 28, 35));
    header_copy.add(&title, 0, SizerFlag::Bottom, 4);
    let subtitle = StaticText::builder(&root)
        .with_label("本地 remote-control backend + 飞书桥接")
        .build();
    subtitle.set_foreground_color(Colour::rgb(91, 100, 114));
    header_copy.add(&subtitle, 0, SizerFlag::Expand, 0);
    header.add_sizer(&header_copy, 1, SizerFlag::Expand, 0);

    let endpoint = StaticText::builder(&root)
        .with_label(&format!("服务地址 {}", api.base_url))
        .build();
    endpoint.set_foreground_color(Colour::rgb(103, 111, 124));
    header.add(&endpoint, 0, SizerFlag::AlignCenterVertical, 0);
    root_sizer.add_sizer(
        &header,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        18,
    );

    let status_section = BoxSizer::builder(Orientation::Vertical).build();
    let status_title = section_label(&root, "状态概览");
    status_section.add(&status_title, 0, SizerFlag::Left | SizerFlag::Right, 4);
    let status_row = BoxSizer::builder(Orientation::Horizontal).build();
    let service_status = status_panel(&root, "本地服务");
    let feishu_status = status_panel(&root, "飞书");
    let codex_status = status_panel(&root, "Codex App");
    status_row.add(
        &service_status.panel,
        1,
        SizerFlag::Expand | SizerFlag::All,
        8,
    );
    status_row.add(
        &feishu_status.panel,
        1,
        SizerFlag::Expand | SizerFlag::All,
        8,
    );
    status_row.add(
        &codex_status.panel,
        1,
        SizerFlag::Expand | SizerFlag::All,
        8,
    );
    status_section.add_sizer(&status_row, 0, SizerFlag::Expand | SizerFlag::Top, 6);
    root_sizer.add_sizer(
        &status_section,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        16,
    );

    let notebook = Notebook::builder(&root).build();

    let codex_page = Panel::builder(&notebook).build();
    codex_page.set_background_color(Colour::rgb(255, 255, 255));
    let codex_sizer = BoxSizer::builder(Orientation::Vertical).build();

    let codex_status_title = section_label(&codex_page, "Codex App 状态");
    codex_sizer.add(
        &codex_status_title,
        0,
        SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        14,
    );
    let codex_config_state = StaticText::builder(&codex_page)
        .with_label("正在读取 ~/.codex 配置状态")
        .build();
    codex_config_state.set_foreground_color(Colour::rgb(75, 84, 98));
    codex_config_state.wrap(760);
    codex_sizer.add(
        &codex_config_state,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        22,
    );

    let codex_divider = StaticLine::builder(&codex_page).build();
    codex_sizer.add(
        &codex_divider,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        16,
    );

    let config_header = BoxSizer::builder(Orientation::Horizontal).build();
    let config_title = section_label(&codex_page, "写入 Codex App 配置");
    config_header.add(&config_title, 1, SizerFlag::AlignCenterVertical, 0);
    let uninstall_button = Button::builder(&codex_page).with_label("卸载注入").build();
    uninstall_button.set_tooltip("移除 chatgpt_base_url、model_provider 和本地认证注入");
    let configure_button = Button::builder(&codex_page).with_label("写入配置").build();
    configure_button.set_tooltip("写入 ~/.codex/config.toml 和本地 ChatgptAuthTokens");
    config_header.add(&uninstall_button, 0, SizerFlag::Right, 8);
    config_header.add(&configure_button, 0, SizerFlag::Right, 0);
    codex_sizer.add_sizer(
        &config_header,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        16,
    );
    let config_hint = StaticText::builder(&codex_page)
        .with_label("用于 Codex App remote-control。会写入本地 backend 地址和 ChatgptAuthTokens。")
        .build();
    config_hint.set_foreground_color(Colour::rgb(103, 111, 124));
    codex_sizer.add(
        &config_hint,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        12,
    );

    let form = FlexGridSizer::builder(0, 2)
        .with_gap(Size::new(10, 12))
        .build();
    form.add_growable_col(1, 1);
    let provider_name = text_field_row(&codex_page, &form, "Provider 名称", "");
    let provider_base_url = text_field_row(&codex_page, &form, "第三方 Base URL", "");
    let provider_key = text_field_row(&codex_page, &form, "API Key", "");
    codex_sizer.add_sizer(
        &form,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        16,
    );
    codex_sizer.add_stretch_spacer(1);
    codex_page.set_sizer(codex_sizer, true);

    let feishu_page = Panel::builder(&notebook).build();
    feishu_page.set_background_color(Colour::rgb(255, 255, 255));
    let feishu_sizer = BoxSizer::builder(Orientation::Vertical).build();

    let feishu_title = section_label(&feishu_page, "飞书机器人");
    feishu_sizer.add(
        &feishu_title,
        0,
        SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        22,
    );
    let feishu_state = StaticText::builder(&feishu_page)
        .with_label("检测中")
        .build();
    feishu_state.set_foreground_color(Colour::rgb(73, 83, 96));
    feishu_sizer.add(
        &feishu_state,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        22,
    );

    let feishu_detail = StaticText::builder(&feishu_page)
        .with_label("正在读取飞书接入状态")
        .build();
    feishu_detail.set_foreground_color(Colour::rgb(82, 91, 105));
    feishu_detail.wrap(760);
    feishu_sizer.add(
        &feishu_detail,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        22,
    );

    let divider = StaticLine::builder(&feishu_page).build();
    feishu_sizer.add(
        &divider,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        22,
    );

    let feishu_meta = StaticText::builder(&feishu_page).with_label("").build();
    feishu_meta.set_foreground_color(Colour::rgb(103, 111, 124));
    feishu_meta.wrap(760);
    feishu_sizer.add(
        &feishu_meta,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        22,
    );

    let feishu_buttons = BoxSizer::builder(Orientation::Horizontal).build();
    feishu_buttons.add_stretch_spacer(1);
    let stop_bridge_button = Button::builder(&feishu_page).with_label("断开接入").build();
    stop_bridge_button.set_tooltip("停止飞书桥接，不删除已保存的机器人配置");
    let change_bot_button = Button::builder(&feishu_page)
        .with_label("更换机器人")
        .build();
    change_bot_button.set_tooltip("重新进入飞书扫码接入流程");
    feishu_buttons.add(&stop_bridge_button, 0, SizerFlag::Right, 8);
    feishu_buttons.add(&change_bot_button, 0, SizerFlag::Right, 0);
    feishu_sizer.add_sizer(
        &feishu_buttons,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        22,
    );
    feishu_sizer.add_stretch_spacer(1);
    feishu_page.set_sizer(feishu_sizer, true);

    let system_page = Panel::builder(&notebook).build();
    system_page.set_background_color(Colour::rgb(255, 255, 255));
    let system_sizer = BoxSizer::builder(Orientation::Vertical).build();
    let system_title = section_label(&system_page, "本地服务");
    system_sizer.add(
        &system_title,
        0,
        SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        22,
    );
    let service_text = StaticText::builder(&system_page)
        .with_label("Codex Remote 不会自动常驻或修改系统启动项。需要使用本地 backend 时，请明确点击启动本地服务或手动运行 daemon。日志保留在项目 logs 目录。")
        .build();
    service_text.set_foreground_color(Colour::rgb(82, 91, 105));
    service_text.wrap(760);
    system_sizer.add(
        &service_text,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        22,
    );
    let refresh_button = Button::builder(&system_page).with_label("检测状态").build();
    refresh_button.set_tooltip("立即刷新本地服务、飞书和 Codex App 连接状态");
    let start_daemon_button = Button::builder(&system_page)
        .with_label("启动本地服务")
        .build();
    start_daemon_button.set_tooltip("本次会话启动 codex-remote daemon，不安装开机启动项");
    let system_buttons = BoxSizer::builder(Orientation::Horizontal).build();
    system_buttons.add_stretch_spacer(1);
    system_buttons.add(&start_daemon_button, 0, SizerFlag::Right, 8);
    system_buttons.add(&refresh_button, 0, SizerFlag::Right, 0);
    system_sizer.add_sizer(
        &system_buttons,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        22,
    );
    system_sizer.add_stretch_spacer(1);
    system_page.set_sizer(system_sizer, true);

    notebook.add_page(&codex_page, "Codex App", true, None);
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
        feishu_state,
        feishu_detail,
        feishu_meta,
        codex_config_state,
        change_bot_button,
        stop_bridge_button,
        configure_button,
        refresh_button,
        start_daemon_button,
        uninstall_button,
    };

    refresh_dashboard(&api, &handles);

    {
        let api = api.clone();
        let handles = handles;
        refresh_button.on_click(move |_| refresh_dashboard(&api, &handles));
    }

    {
        let api = api.clone();
        let handles = handles;
        let frame = frame;
        start_daemon_button.on_click(move |_| match start_daemon(&api) {
            Ok(_) => {
                show_info(&frame, "本地服务已启动。");
                refresh_dashboard(&api, &handles);
            }
            Err(err) => show_error(&frame, &err),
        });
    }

    {
        let api = api.clone();
        let handles = handles;
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
                        "配置已写入。请重新打开 Codex App，或在 Codex App 中重新进入远程控制。",
                    );
                    refresh_dashboard(&api, &handles);
                }
                Err(err) => show_error(&frame, &err),
            }
        });
    }

    {
        let api = api.clone();
        let handles = handles;
        let frame = frame;
        uninstall_button.on_click(move |_| match api.uninstall_codex_app() {
            Ok(_) => {
                show_info(&frame, "Codex App 注入配置已卸载。");
                refresh_dashboard(&api, &handles);
            }
            Err(err) => show_error(&frame, &err),
        });
    }

    {
        let api = api.clone();
        let handles = handles;
        let frame = frame;
        stop_bridge_button.on_click(move |_| match api.stop_bridge() {
            Ok(_) => {
                show_info(&frame, "飞书接入已断开。");
                refresh_dashboard(&api, &handles);
            }
            Err(err) => show_error(&frame, &err),
        });
    }

    {
        let api = api.clone();
        let handles = handles;
        let frame = frame;
        change_bot_button.on_click(move |_| {
            show_onboard_dialog(&frame, api.clone());
            refresh_dashboard(&api, &handles);
        });
    }

    let timer_store: Rc<RefCell<Option<Timer<Frame>>>> = Rc::new(RefCell::new(None));
    let timer = Timer::new(&frame);
    {
        let api = api.clone();
        let handles = handles;
        timer.on_tick(move |_| refresh_dashboard(&api, &handles));
    }
    timer.start(2500, false);
    timer_store.borrow_mut().replace(timer);
    std::mem::forget(timer_store);

    frame.centre();
    frame.show(true);
}

fn default_base_url() -> String {
    std::env::var("CODEX_REMOTE_GUI_BASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
}

fn start_daemon(api: &ApiClient) -> Result<(), String> {
    if api.is_online() {
        return Ok(());
    }

    let mut child = spawn_daemon()?;
    for _ in 0..40 {
        thread::sleep(Duration::from_millis(250));
        if api.is_online() {
            std::mem::forget(child);
            return Ok(());
        }
        if let Some(status) = child.try_wait().map_err(|err| err.to_string())? {
            return Err(format!("本地服务启动后退出：{status}"));
        }
    }

    std::mem::forget(child);
    Err("本地服务已启动，但 10 秒内没有响应。请检查 logs/codex-remote-chain.log。".to_string())
}

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
    if let Some(exe) = sibling_daemon_exe() {
        let mut command = Command::new(exe);
        append_daemon_args(&mut command);
        return Ok(command);
    }

    if let Some(exe) = bundled_daemon_exe() {
        let mut command = Command::new(exe);
        append_daemon_args(&mut command);
        return Ok(command);
    }

    if env::var_os("CODEX_REMOTE_USE_REPO_CONFIG").is_some()
        && let Some(repo_root) = repo_root_from_target_exe().or_else(|| repo_root_from_cwd())
    {
        let mut command = Command::new("cargo");
        command.current_dir(repo_root).arg("run").arg("--");
        append_daemon_args(&mut command);
        return Ok(command);
    }

    let mut command = Command::new("codex-remote");
    append_daemon_args(&mut command);
    Ok(command)
}

fn append_daemon_args(command: &mut Command) {
    if let Some(config_path) = daemon_config_path() {
        command.arg("--config").arg(config_path);
    }
    command.arg("daemon");
}

fn sibling_daemon_exe() -> Option<PathBuf> {
    let exe_name = if cfg!(windows) {
        "codex-remote.exe"
    } else {
        "codex-remote"
    };
    let path = std::env::current_exe().ok()?.with_file_name(exe_name);
    path.exists().then_some(path)
}

fn bundled_daemon_exe() -> Option<PathBuf> {
    let exe_name = if cfg!(windows) {
        "codex-remote.exe"
    } else {
        "codex-remote"
    };
    let exe = std::env::current_exe().ok()?;
    let macos_dir = exe.parent()?;
    let contents_dir = macos_dir.parent()?;
    if macos_dir.file_name().and_then(|value| value.to_str()) != Some("MacOS") {
        return None;
    }
    let path = contents_dir.join("Resources").join(exe_name);
    path.exists().then_some(path)
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

fn repo_root_from_cwd() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    has_manifest(&cwd).then_some(cwd)
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
            .timeout(Duration::from_secs(2))
            .build()
            .expect("build HTTP client");
        Self { base_url, http }
    }

    fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        let text = self.request_text(self.http.get(self.url(path)))?;
        serde_json::from_str(&text).map_err(|err| format!("{path} 返回数据无法解析：{err}"))
    }

    fn is_online(&self) -> bool {
        self.get::<serde_json::Value>("/api/status").is_ok()
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

    fn dashboard(&self) -> DashboardSnapshot {
        let status = match self.get::<ServerStatus>("/api/status") {
            Ok(status) => status,
            Err(err) => {
                return DashboardSnapshot {
                    service_online: false,
                    error: Some(err),
                    ..DashboardSnapshot::default()
                };
            }
        };

        DashboardSnapshot {
            service_online: true,
            error: None,
            config: self.get::<AppConfig>("/api/config").ok(),
            backend: self
                .get::<RemoteControlBackendStatus>("/api/remote-control/backend-status")
                .ok(),
            remote: self
                .get::<RemoteControlStatus>("/api/remote-control/status")
                .ok(),
            codex_app: self.get::<CodexAppStatus>("/api/codex-app/status").ok(),
            status: Some(status),
        }
    }

    fn configure_codex_app(&self, request: &ConfigureRequest) -> Result<serde_json::Value, String> {
        self.post_json("/api/codex-app/configure", request)
    }

    fn uninstall_codex_app(&self) -> Result<serde_json::Value, String> {
        self.post_empty("/api/codex-app/uninstall")
    }

    fn stop_bridge(&self) -> Result<serde_json::Value, String> {
        self.post_empty("/api/bridge/stop")
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

#[derive(Clone, Copy)]
struct StatusPanel {
    panel: Panel,
    marker: StaticText,
    state: StaticText,
    detail: StaticText,
}

#[derive(Clone, Copy)]
struct UiHandles {
    status_bar: StatusBar,
    service_status: StatusPanel,
    feishu_status: StatusPanel,
    codex_status: StatusPanel,
    feishu_state: StaticText,
    feishu_detail: StaticText,
    feishu_meta: StaticText,
    codex_config_state: StaticText,
    change_bot_button: Button,
    stop_bridge_button: Button,
    configure_button: Button,
    refresh_button: Button,
    start_daemon_button: Button,
    uninstall_button: Button,
}

#[derive(Default)]
struct DashboardSnapshot {
    service_online: bool,
    status: Option<ServerStatus>,
    config: Option<AppConfig>,
    backend: Option<RemoteControlBackendStatus>,
    remote: Option<RemoteControlStatus>,
    codex_app: Option<CodexAppStatus>,
    error: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServerStatus {
    bind: String,
    state_path: String,
    feishu_ws: FeishuWsState,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FeishuWsState {
    connecting: bool,
    connected: bool,
    last_error: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppConfig {
    feishu: FeishuConfig,
    bridge: BridgeConfig,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FeishuConfig {
    app_id: String,
    allowed_open_ids: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BridgeConfig {
    enabled: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteControlBackendStatus {
    enabled: bool,
    feishu_configured: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteControlStatus {
    connected: bool,
    initialized: bool,
    server_name: Option<String>,
    current_thread_id: Option<String>,
    last_error: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexAppStatus {
    codex_home: String,
    configured: bool,
    config_ok: bool,
    auth_ok: bool,
    config_error: Option<String>,
    auth_error: Option<String>,
    gui_api_base: GuiApiBaseStatus,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GuiApiBaseStatus {
    value: Option<String>,
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
    expires_in: u64,
    interval: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FeishuOnboardPoll {
    done: bool,
    app_id: Option<String>,
    open_id: Option<String>,
    display_name: Option<String>,
    error: Option<serde_json::Value>,
    error_description: Option<serde_json::Value>,
}

fn section_label(parent: &Panel, label: &str) -> StaticText {
    let text = StaticText::builder(parent).with_label(label).build();
    text.set_foreground_color(Colour::rgb(30, 35, 43));
    text
}

fn status_panel(parent: &Panel, title: &str) -> StatusPanel {
    let panel = Panel::builder(parent).build();
    panel.set_background_color(Colour::rgb(255, 255, 255));
    panel.set_min_size(Size::new(250, 78));

    let row = BoxSizer::builder(Orientation::Horizontal).build();
    let marker = StaticText::builder(&panel).with_label("●").build();
    marker.set_foreground_color(Colour::rgb(116, 124, 136));
    row.add(&marker, 0, SizerFlag::Top | SizerFlag::Right, 3);

    let text_col = BoxSizer::builder(Orientation::Vertical).build();
    let title_label = StaticText::builder(&panel).with_label(title).build();
    title_label.set_foreground_color(Colour::rgb(91, 100, 114));
    text_col.add(&title_label, 0, SizerFlag::Bottom, 4);

    let state = StaticText::builder(&panel).with_label("检测中").build();
    state.set_foreground_color(Colour::rgb(34, 39, 47));
    text_col.add(&state, 0, SizerFlag::Bottom, 4);

    let detail = StaticText::builder(&panel).with_label("").build();
    detail.set_foreground_color(Colour::rgb(103, 111, 124));
    detail.wrap(220);
    text_col.add(&detail, 0, SizerFlag::Expand, 0);

    row.add_sizer(&text_col, 1, SizerFlag::Expand, 0);
    panel.set_sizer(row, true);
    StatusPanel {
        panel,
        marker,
        state,
        detail,
    }
}

fn text_field_row(parent: &Panel, sizer: &FlexGridSizer, label: &str, value: &str) -> TextCtrl {
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

fn refresh_dashboard(api: &ApiClient, handles: &UiHandles) {
    let snapshot = api.dashboard();
    update_dashboard(handles, &snapshot);
}

fn update_dashboard(handles: &UiHandles, snapshot: &DashboardSnapshot) {
    if !snapshot.service_online {
        set_status_panel(
            &handles.service_status,
            "未运行",
            snapshot.error.as_deref().unwrap_or("无法连接本地服务"),
            StateTone::Error,
        );
        set_status_panel(
            &handles.feishu_status,
            "不可用",
            "本地服务未运行",
            StateTone::Muted,
        );
        set_status_panel(
            &handles.codex_status,
            "不可用",
            "本地服务未运行",
            StateTone::Muted,
        );
        handles.feishu_state.set_label("本地服务未运行");
        handles
            .feishu_detail
            .set_label("请先启动 codex-remote 后端。");
        handles.feishu_meta.set_label("");
        handles
            .codex_config_state
            .set_label("无法读取 Codex App 配置状态。");
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
            &format!("监听 {}，状态文件 {}", status.bind, status.state_path),
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

    let feishu_ws = snapshot.status.as_ref().map(|status| &status.feishu_ws);
    let (feishu_state, feishu_detail, feishu_tone) = if !feishu_configured {
        (
            "未接入",
            "点击“更换机器人”进入扫码接入流程。",
            StateTone::Warn,
        )
    } else if !bridge_enabled {
        (
            "已暂停",
            "机器人已配置，但飞书接入当前关闭。",
            StateTone::Muted,
        )
    } else if feishu_ws.is_some_and(|ws| ws.connected) {
        ("已接入", "飞书长连接正常。", StateTone::Ok)
    } else if feishu_ws.is_some_and(|ws| ws.connecting) {
        ("连接中", "正在连接飞书长连接。", StateTone::Warn)
    } else {
        (
            "等待连接",
            "机器人已配置，等待飞书连接建立。",
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

    let codex_connected = snapshot
        .remote
        .as_ref()
        .map(|remote| remote.connected && remote.initialized)
        .unwrap_or(false);
    let codex_configured = snapshot
        .codex_app
        .as_ref()
        .map(|status| status.configured)
        .unwrap_or(false);

    if codex_connected {
        let detail = snapshot
            .remote
            .as_ref()
            .map(codex_remote_detail)
            .unwrap_or_else(|| "Codex App remote-control 已连接。".to_string());
        set_status_panel(&handles.codex_status, "已连接", &detail, StateTone::Ok);
        handles.status_bar.set_status_text("Codex App：已连接", 2);
    } else if codex_configured {
        set_status_panel(
            &handles.codex_status,
            "等待连接",
            "配置已注入，打开 Codex App 的远程控制后会连接到本机服务。",
            StateTone::Warn,
        );
        handles.status_bar.set_status_text("Codex App：等待连接", 2);
    } else {
        set_status_panel(
            &handles.codex_status,
            "未注入",
            "需要先写入 chatgpt_base_url 和本地 ChatgptAuthTokens。",
            StateTone::Warn,
        );
        handles.status_bar.set_status_text("Codex App：未注入", 2);
    }

    handles
        .codex_config_state
        .set_label(&codex_app_detail(snapshot));
    handles.codex_config_state.wrap(500);
}

fn set_actions_enabled(handles: &UiHandles, enabled: bool) {
    handles.change_bot_button.enable(enabled);
    handles.configure_button.enable(enabled);
    handles.refresh_button.enable(true);
    handles.uninstall_button.enable(enabled);
    handles.stop_bridge_button.enable(enabled);
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
    panel.marker.set_foreground_color(tone.colour());
    panel.state.set_label(state);
    panel.state.set_foreground_color(tone.colour());
    panel.detail.set_label(detail);
    panel.detail.wrap(220);
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
    "Codex App remote-control 已连接。".to_string()
}

fn codex_app_detail(snapshot: &DashboardSnapshot) -> String {
    let Some(status) = &snapshot.codex_app else {
        return "无法读取 ~/.codex 配置状态。".to_string();
    };
    if status.configured {
        let mut detail = format!("已注入到 {}", status.codex_home);
        if let Some(value) = &status.gui_api_base.value {
            detail.push_str(&format!("\n检测到遗留 CODEX_API_BASE_URL: {value}"));
        }
        return detail;
    }

    let mut parts = Vec::new();
    if !status.config_ok {
        parts.push(
            status
                .config_error
                .as_deref()
                .unwrap_or("config.toml 未匹配本地 backend")
                .to_string(),
        );
    }
    if !status.auth_ok {
        parts.push(
            status
                .auth_error
                .as_deref()
                .unwrap_or("auth.json 未使用 ChatgptAuthTokens")
                .to_string(),
        );
    }
    if let Some(err) = &status.gui_api_base.error {
        parts.push(format!("检测遗留 CODEX_API_BASE_URL 失败: {err}"));
    }
    if parts.is_empty() {
        "尚未注入 Codex App 配置。".to_string()
    } else {
        parts.join("\n")
    }
}

fn qr_bitmap(value: &str) -> Option<(Bitmap, i32)> {
    let code = QrCode::new(value.as_bytes()).ok()?;
    let quiet_zone = 4usize;
    let cells = code.width() + quiet_zone * 2;
    let module_size = (420usize / cells).clamp(3, 8);
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
        .with_size(540, 620)
        .build();
    dialog.set_background_color(Colour::rgb(255, 255, 255));
    let sizer = BoxSizer::builder(Orientation::Vertical).build();

    let title = StaticText::builder(&dialog)
        .with_label("扫码接入新的飞书机器人")
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
        .with_label(&format!(
            "有效期约 {} 秒；建议扫码后点击“检查结果”。建议检查间隔 {} 秒。\n{}",
            start.expires_in, start.interval, start.verification_uri_complete
        ))
        .build();
    info.set_foreground_color(Colour::rgb(88, 96, 108));
    info.wrap(480);
    sizer.add(&info, 0, SizerFlag::All, 18);

    let buttons = BoxSizer::builder(Orientation::Horizontal).build();
    let open_button = Button::builder(&dialog).with_label("浏览器打开").build();
    let poll_button = Button::builder(&dialog).with_label("检查结果").build();
    let close_button = Button::builder(&dialog).with_label("关闭").build();
    buttons.add(&open_button, 1, SizerFlag::Expand | SizerFlag::Right, 8);
    buttons.add(&poll_button, 1, SizerFlag::Expand | SizerFlag::Right, 8);
    buttons.add(&close_button, 1, SizerFlag::Expand, 0);
    sizer.add_sizer(
        &buttons,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
        18,
    );

    dialog.set_sizer(sizer, true);

    {
        let url = start.verification_uri_complete.clone();
        open_button.on_click(move |_| {
            launch_default_browser(&url, BrowserLaunchFlags::Default);
        });
    }

    {
        let api = api.clone();
        let device_code = start.device_code.clone();
        let dialog = dialog;
        poll_button.on_click(move |_| match api.poll_feishu_onboard(&device_code) {
            Ok(result) if result.done => {
                let name = result
                    .display_name
                    .or(result.app_id)
                    .unwrap_or_else(|| "飞书机器人".to_string());
                let detail = result
                    .open_id
                    .map(|open_id| format!("{name} 已接入。\nOpen ID: {}", short_id(&open_id)))
                    .unwrap_or_else(|| format!("{name} 已接入。"));
                show_info(&dialog, &detail);
                dialog.end_modal(ID_OK);
            }
            Ok(result) => {
                let detail = result
                    .error_description
                    .or(result.error)
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "飞书尚未返回完成状态。".to_string());
                show_info(&dialog, &detail);
            }
            Err(err) => show_error(&dialog, &err),
        });
    }

    {
        let dialog = dialog;
        close_button.on_click(move |_| dialog.end_modal(ID_CANCEL));
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

fn short_id(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() <= 18 {
        trimmed.to_string()
    } else {
        format!("{}...", &trimmed[..18])
    }
}
