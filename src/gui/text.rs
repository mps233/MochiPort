#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GuiLocale {
    ZhCn,
    EnUs,
}

impl GuiLocale {
    pub(super) fn from_code(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "zh-cn" | "zh_cn" | "zh" | "cn" => Some(Self::ZhCn),
            "en-us" | "en_us" | "en" => Some(Self::EnUs),
            _ => None,
        }
    }

    pub(super) fn code(self) -> &'static str {
        match self {
            Self::ZhCn => "zh-CN",
            Self::EnUs => "en-US",
        }
    }
}

impl Default for GuiLocale {
    fn default() -> Self {
        Self::ZhCn
    }
}

#[derive(Clone, Copy)]
pub(super) struct GuiText {
    pub(super) locale: GuiLocale,
}

impl GuiText {
    pub(super) fn new(locale: GuiLocale) -> Self {
        Self { locale }
    }

    pub(super) fn version(self) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("版本 {}", env!("CARGO_PKG_VERSION")),
            GuiLocale::EnUs => format!("Version {}", env!("CARGO_PKG_VERSION")),
        }
    }

    pub(super) fn file_menu(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "文件",
            GuiLocale::EnUs => "&File",
        }
    }

    pub(super) fn close_window(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "关闭窗口\tCtrl+W",
            GuiLocale::EnUs => "&Close Window\tCtrl+W",
        }
    }

    pub(super) fn close_window_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "关闭这个窗口",
            GuiLocale::EnUs => "Close this window",
        }
    }

    pub(super) fn minimize(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "最小化\tCtrl+M",
            GuiLocale::EnUs => "Mi&nimize\tCtrl+M",
        }
    }

    pub(super) fn minimize_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "最小化窗口",
            GuiLocale::EnUs => "Minimize this window",
        }
    }

    pub(super) fn quit(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "退出 Codex Remote\tCtrl+Q",
            GuiLocale::EnUs => "&Quit Codex Remote\tCtrl+Q",
        }
    }

    pub(super) fn language_menu(self) -> &'static str {
        "&Language / 语言"
    }

    pub(super) fn language_zh_cn(self) -> &'static str {
        "中文（简体）"
    }

    pub(super) fn language_en_us(self) -> &'static str {
        "English"
    }

    pub(super) fn language_restart_message(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "语言设置已保存，重启 Codex Remote 后生效。",
            GuiLocale::EnUs => "Language saved. Restart Codex Remote to apply it.",
        }
    }

    pub(super) fn language_save_failed(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "语言设置保存失败",
            GuiLocale::EnUs => "Failed to save language setting",
        }
    }

    pub(super) fn help_menu(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "帮助",
            GuiLocale::EnUs => "&Help",
        }
    }

    pub(super) fn check_updates(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "检查更新",
            GuiLocale::EnUs => "&Check for Updates",
        }
    }

    pub(super) fn check_updates_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "检查 GitHub Releases 是否有新版本",
            GuiLocale::EnUs => "Check GitHub Releases for a newer Codex Remote version",
        }
    }

    pub(super) fn about(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "关于 Codex Remote",
            GuiLocale::EnUs => "&About Codex Remote",
        }
    }

    pub(super) fn status_overview(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "状态概览",
            GuiLocale::EnUs => "Status",
        }
    }

    pub(super) fn codex_control_channel(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "Codex App 控制通道",
            GuiLocale::EnUs => "Codex App Control",
        }
    }

    pub(super) fn vscode_extension(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "VS Code 插件",
            GuiLocale::EnUs => "VS Code Extension",
        }
    }

    pub(super) fn local_service(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "本地服务",
            GuiLocale::EnUs => "Local Service",
        }
    }

    pub(super) fn detecting(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "检测中",
            GuiLocale::EnUs => "Checking",
        }
    }

    pub(super) fn unavailable(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "暂不可用",
            GuiLocale::EnUs => "Unavailable",
        }
    }

    pub(super) fn app_gui_unsupported(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "当前平台暂不支持 App GUI",
            GuiLocale::EnUs => "App GUI is not supported on this platform.",
        }
    }

    pub(super) fn provider_management(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "Provider 管理",
            GuiLocale::EnUs => "Provider Management",
        }
    }

    pub(super) fn add(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "新增",
            GuiLocale::EnUs => "Add",
        }
    }

    pub(super) fn save(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "保存",
            GuiLocale::EnUs => "Save",
        }
    }

    pub(super) fn delete(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "删除",
            GuiLocale::EnUs => "Delete",
        }
    }

    pub(super) fn enable(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "启用",
            GuiLocale::EnUs => "Enable",
        }
    }

    pub(super) fn new_provider_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "清空表单，新增一个 provider",
            GuiLocale::EnUs => "Clear the form and add a provider",
        }
    }

    pub(super) fn save_provider_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "保存或更新当前表单里的 provider",
            GuiLocale::EnUs => "Save or update the provider in the form",
        }
    }

    pub(super) fn delete_provider_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "删除当前选中的 provider",
            GuiLocale::EnUs => "Delete the selected provider",
        }
    }

    pub(super) fn configure_provider_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "保存并使用这个模型服务",
            GuiLocale::EnUs => "Save and use this model provider",
        }
    }

    pub(super) fn provider_catalog_loading(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "正在匹配 ~/.codex/config.toml 里的 provider",
            GuiLocale::EnUs => "Reading providers from ~/.codex/config.toml",
        }
    }

    pub(super) fn provider_name(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "Provider 名称",
            GuiLocale::EnUs => "Provider Name",
        }
    }

    pub(super) fn name(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "名称",
            GuiLocale::EnUs => "Name",
        }
    }

    pub(super) fn current(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "当前",
            GuiLocale::EnUs => "Current",
        }
    }

    pub(super) fn api_key_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "API Key 已保存时会用星号显示；需要更换时直接输入新 key。",
            GuiLocale::EnUs => "Saved API keys are masked. Enter a new key to replace it.",
        }
    }

    pub(super) fn image_generation_feature(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "启用生图工具",
            GuiLocale::EnUs => "Enable image generation",
        }
    }

    pub(super) fn image_generation_feature_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => {
                "写入 ~/.codex/config.toml 的 [features].image_generation；仅用于影响 Codex CLI 和 VS Code 插件。Codex App 本地会话可能使用自己的 feature gate，本开关不能保证干预。"
            }
            GuiLocale::EnUs => {
                "Writes [features].image_generation in ~/.codex/config.toml for Codex CLI and the VS Code extension. Codex App local sessions may use their own feature gates, so this switch cannot reliably control them."
            }
        }
    }

    pub(super) fn image_generation_feature_note(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "仅 VS Code 插件和 Codex CLI 有效",
            GuiLocale::EnUs => "Only affects VS Code extension and Codex CLI",
        }
    }

    pub(super) fn provider_websocket(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "启用 WebSocket",
            GuiLocale::EnUs => "Enable WebSocket",
        }
    }

    pub(super) fn clear_codex_access(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "清除 Codex 接入",
            GuiLocale::EnUs => "Clear Codex Access",
        }
    }

    pub(super) fn clear_codex_access_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "移除本工具写入的 Codex App 本地接入配置",
            GuiLocale::EnUs => "Remove local Codex App access settings written by this tool",
        }
    }

    pub(super) fn codex_tab(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "Codex 接入",
            GuiLocale::EnUs => "Codex",
        }
    }

    pub(super) fn chat_tab(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "聊天工具接入",
            GuiLocale::EnUs => "Chat Integrations",
        }
    }

    pub(super) fn im_access_hint(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => {
                "多个机器人/agent 可以分别管理多个 Codex 会话；暂不支持多个机器人管理同一个会话。例如飞书 1 管理会话 1、飞书 2 管理会话 2、Telegram 1 管理会话 3；并行数量取决于本机能同时承载多少 Codex 任务。"
            }
            GuiLocale::EnUs => {
                "Multiple bots/agents can manage separate Codex sessions. Multiple bots managing the same session is not supported yet. Parallel capacity depends on how many Codex tasks this machine can run."
            }
        }
    }

    pub(super) fn bot_pool(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "聊天工具机器人池",
            GuiLocale::EnUs => "Bot Pool",
        }
    }

    pub(super) fn bot(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "机器人",
            GuiLocale::EnUs => "Bot",
        }
    }

    pub(super) fn platform(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "平台",
            GuiLocale::EnUs => "Platform",
        }
    }

    pub(super) fn state(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "状态",
            GuiLocale::EnUs => "State",
        }
    }

    pub(super) fn account(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "账号",
            GuiLocale::EnUs => "Account",
        }
    }

    pub(super) fn access(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "接入",
            GuiLocale::EnUs => "Access",
        }
    }

    pub(super) fn delete_selected(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "删除选中",
            GuiLocale::EnUs => "Delete Selected",
        }
    }

    pub(super) fn delete_im_account_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "删除当前选中的机器人接入配置",
            GuiLocale::EnUs => "Delete the selected bot integration",
        }
    }

    pub(super) fn add_bot(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "新增机器人",
            GuiLocale::EnUs => "Add Bot",
        }
    }

    pub(super) fn add_feishu_bot(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "添加飞书机器人",
            GuiLocale::EnUs => "Add Feishu Bot",
        }
    }

    pub(super) fn add_feishu_bot_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "扫码接入一个新的飞书机器人",
            GuiLocale::EnUs => "Scan to connect a new Feishu bot",
        }
    }

    pub(super) fn add_telegram_bot(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "添加 Telegram 机器人",
            GuiLocale::EnUs => "Add Telegram Bot",
        }
    }

    pub(super) fn add_telegram_bot_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "填写 Telegram Bot Token 并接入",
            GuiLocale::EnUs => "Enter a Telegram Bot Token and connect it",
        }
    }

    pub(super) fn add_wechat_bot(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "添加微信机器人",
            GuiLocale::EnUs => "Add WeChat Bot",
        }
    }

    pub(super) fn add_wechat_bot_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "使用微信扫码接入机器人",
            GuiLocale::EnUs => "Scan with WeChat to connect the bot",
        }
    }

    pub(super) fn new_provider_prompt(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "填写新 provider 名称、Base URL 和 API Key，然后点击启用。",
            GuiLocale::EnUs => "Enter a provider name, Base URL, and API key, then click Enable.",
        }
    }

    pub(super) fn saving_provider(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "正在保存 provider，请稍候...",
            GuiLocale::EnUs => "Saving provider...",
        }
    }

    pub(super) fn deleting_provider(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "正在删除 provider，请稍候...",
            GuiLocale::EnUs => "Deleting provider...",
        }
    }

    pub(super) fn enabling_provider(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "正在启用，请稍候...",
            GuiLocale::EnUs => "Enabling provider...",
        }
    }

    pub(super) fn save_in_progress(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "保存中...",
            GuiLocale::EnUs => "Saving...",
        }
    }

    pub(super) fn delete_in_progress(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "删除中...",
            GuiLocale::EnUs => "Deleting...",
        }
    }

    pub(super) fn enable_in_progress(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "启用中...",
            GuiLocale::EnUs => "Enabling...",
        }
    }

    pub(super) fn add_in_progress(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "添加中...",
            GuiLocale::EnUs => "Adding...",
        }
    }

    pub(super) fn starting(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "启动中",
            GuiLocale::EnUs => "Starting",
        }
    }

    pub(super) fn starting_backend(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "正在启动本地 backend。",
            GuiLocale::EnUs => "Starting local backend.",
        }
    }

    pub(super) fn waiting_service(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "等待服务",
            GuiLocale::EnUs => "Waiting",
        }
    }

    pub(super) fn service_reads_status(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "服务启动后读取状态",
            GuiLocale::EnUs => "Status loads after service startup.",
        }
    }

    pub(super) fn service_reads_config(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "服务启动后读取配置",
            GuiLocale::EnUs => "Config loads after service startup.",
        }
    }

    pub(super) fn service_vscode_connect(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "服务启动后可连接 VS Code 插件。",
            GuiLocale::EnUs => "VS Code extension can connect after service startup.",
        }
    }

    pub(super) fn startup_failed(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "启动失败",
            GuiLocale::EnUs => "Startup Failed",
        }
    }

    pub(super) fn not_running(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "未运行",
            GuiLocale::EnUs => "Not Running",
        }
    }

    pub(super) fn gui_auto_start_service(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "GUI 会自动启动本地服务；如果一直未运行，请重启 Codex Remote。",
            GuiLocale::EnUs => {
                "The GUI starts the local service automatically. Restart Codex Remote if it stays offline."
            }
        }
    }

    pub(super) fn local_service_not_running(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "本地服务未运行",
            GuiLocale::EnUs => "Local service is not running.",
        }
    }

    pub(super) fn running(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "运行中",
            GuiLocale::EnUs => "Running",
        }
    }

    pub(super) fn listening(self, bind: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("监听 {bind}"),
            GuiLocale::EnUs => format!("Listening on {bind}"),
        }
    }

    pub(super) fn connected(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "已连接",
            GuiLocale::EnUs => "Connected",
        }
    }

    pub(super) fn initializing(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "初始化中",
            GuiLocale::EnUs => "Initializing",
        }
    }

    pub(super) fn codex_initializing(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "Codex App 已打开控制通道，正在完成 remote-control 初始化。",
            GuiLocale::EnUs => {
                "Codex App opened the control channel and is finishing remote-control initialization."
            }
        }
    }

    pub(super) fn control_not_open(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "未打开控制",
            GuiLocale::EnUs => "Control Closed",
        }
    }

    pub(super) fn control_not_open_detail(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "配置已注入，请在 Codex App 里打开“控制这台 Mac”。",
            GuiLocale::EnUs => "Config is injected. Open remote control in Codex App.",
        }
    }

    pub(super) fn not_injected(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "未注入",
            GuiLocale::EnUs => "Not Injected",
        }
    }

    pub(super) fn fill_provider_then_enable(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "填写 Base URL 和 API Key 后点击启用。",
            GuiLocale::EnUs => "Enter Base URL and API key, then click Enable.",
        }
    }

    pub(super) fn can_connect(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "可接入",
            GuiLocale::EnUs => "Ready",
        }
    }

    pub(super) fn vscode_wrapper_detail(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "VS Code 插件可通过 chatgpt.cliExecutable 使用本地 wrapper。",
            GuiLocale::EnUs => {
                "VS Code extension can use the local wrapper through chatgpt.cliExecutable."
            }
        }
    }

    pub(super) fn provider_waiting_service(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "等待本地服务",
            GuiLocale::EnUs => "Waiting for local service",
        }
    }

    pub(super) fn provider_read_after_start(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "启动后读取 ~/.codex/config.toml",
            GuiLocale::EnUs => "Reads ~/.codex/config.toml after startup",
        }
    }

    pub(super) fn not_configured(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "未配置",
            GuiLocale::EnUs => "Not configured",
        }
    }

    pub(super) fn provider_create_on_write(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "未配置，写入时新建",
            GuiLocale::EnUs => "Not configured; created when written",
        }
    }

    pub(super) fn in_use(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "使用中",
            GuiLocale::EnUs => "Active",
        }
    }

    pub(super) fn key_configured(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "已配置",
            GuiLocale::EnUs => "Configured",
        }
    }

    pub(super) fn provider_catalog_after_service(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "本地服务运行后会读取 ~/.codex/config.toml 里的 provider。",
            GuiLocale::EnUs => {
                "Providers are read from ~/.codex/config.toml after the local service starts."
            }
        }
    }

    pub(super) fn no_provider(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "还没有 provider，填写后点击启用。",
            GuiLocale::EnUs => "No providers yet. Fill the form and click Enable.",
        }
    }

    pub(super) fn current_provider(self, name: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("当前 provider: {name}"),
            GuiLocale::EnUs => format!("Current provider: {name}"),
        }
    }

    pub(super) fn saved_providers(self, count: usize) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("已保存 {count} 个 provider，请选择一个使用。"),
            GuiLocale::EnUs => format!("{count} providers saved. Select one to use."),
        }
    }

    pub(super) fn im_waiting_service_row(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "本地服务启动后读取",
            GuiLocale::EnUs => "Loads after local service starts",
        }
    }

    pub(super) fn reading(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "读取中",
            GuiLocale::EnUs => "Loading",
        }
    }

    pub(super) fn reading_bot_list(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "正在读取机器人列表",
            GuiLocale::EnUs => "Loading bot list",
        }
    }

    pub(super) fn not_connected(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "未接入",
            GuiLocale::EnUs => "Not Connected",
        }
    }

    pub(super) fn scan_or_token(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "请扫码或填写 Bot Token",
            GuiLocale::EnUs => "Scan or enter a Bot Token",
        }
    }

    pub(super) fn paused(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "已暂停",
            GuiLocale::EnUs => "Paused",
        }
    }

    pub(super) fn im_connected(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "已接入",
            GuiLocale::EnUs => "Connected",
        }
    }

    pub(super) fn error(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "异常",
            GuiLocale::EnUs => "Error",
        }
    }

    pub(super) fn connecting(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "连接中",
            GuiLocale::EnUs => "Connecting",
        }
    }

    pub(super) fn waiting_connection(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "等待连接",
            GuiLocale::EnUs => "Waiting",
        }
    }

    pub(super) fn bot_saved(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "机器人已保存",
            GuiLocale::EnUs => "Bot saved",
        }
    }

    pub(super) fn name_saved(self, name: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("{name} 已保存"),
            GuiLocale::EnUs => format!("{name} saved"),
        }
    }

    pub(super) fn waiting_bot_connection(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "等待机器人连接",
            GuiLocale::EnUs => "Waiting for bot connection",
        }
    }

    pub(super) fn im_empty_detail(self, platform: &str) -> String {
        match (self.locale, platform) {
            (GuiLocale::ZhCn, "feishu") => "扫码添加飞书机器人".to_string(),
            (GuiLocale::ZhCn, "telegram") => "添加 Telegram Bot Token".to_string(),
            (GuiLocale::ZhCn, "wechat") => "扫码添加微信机器人".to_string(),
            (GuiLocale::ZhCn, _) => "添加机器人".to_string(),
            (GuiLocale::EnUs, "feishu") => "Scan to add a Feishu bot".to_string(),
            (GuiLocale::EnUs, "telegram") => "Add a Telegram Bot Token".to_string(),
            (GuiLocale::EnUs, "wechat") => "Scan to add a WeChat bot".to_string(),
            (GuiLocale::EnUs, _) => "Add a bot".to_string(),
        }
    }

    pub(super) fn bot_fallback(self, platform: &str) -> &'static str {
        match (self.locale, platform) {
            (GuiLocale::ZhCn, "feishu") => "飞书机器人",
            (GuiLocale::ZhCn, "telegram") => "Telegram 机器人",
            (GuiLocale::ZhCn, "wechat") => "微信机器人",
            (GuiLocale::ZhCn, _) => "机器人",
            (GuiLocale::EnUs, "feishu") => "Feishu bot",
            (GuiLocale::EnUs, "telegram") => "Telegram bot",
            (GuiLocale::EnUs, "wechat") => "WeChat bot",
            (GuiLocale::EnUs, _) => "Bot",
        }
    }

    pub(super) fn bot_connecting(self, name: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("{name} 正在连接"),
            GuiLocale::EnUs => format!("{name} connecting"),
        }
    }

    pub(super) fn bot_waiting(self, name: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("{name} 等待连接"),
            GuiLocale::EnUs => format!("{name} waiting"),
        }
    }

    pub(super) fn bot_error(self, name: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("{name} 异常"),
            GuiLocale::EnUs => format!("{name} error"),
        }
    }

    pub(super) fn remote_stale(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "remote-control 心跳失活，等待 Codex App 自动重连。",
            GuiLocale::EnUs => {
                "remote-control heartbeat is stale; waiting for Codex App to reconnect."
            }
        }
    }

    pub(super) fn recent_error(self, err: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("最近错误: {err}"),
            GuiLocale::EnUs => format!("Recent error: {err}"),
        }
    }

    pub(super) fn remote_heartbeat(self, status: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("remote-control 已连接，心跳 {status}。"),
            GuiLocale::EnUs => format!("remote-control connected, heartbeat {status}."),
        }
    }

    pub(super) fn remote_connected_detail(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "remote-control 已连接。",
            GuiLocale::EnUs => "remote-control connected.",
        }
    }

    pub(super) fn codex_remote_connected_detail(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "Codex App remote-control 已连接。",
            GuiLocale::EnUs => "Codex App remote-control connected.",
        }
    }
}
