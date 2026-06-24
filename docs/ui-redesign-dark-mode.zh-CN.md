# GUI 视觉改造 + 暗色模式

## 背景（为什么做）

`codexhub` 的 GUI 是原生 wxWidgets（wxdragon）。当前观感偏「丑」的根因不是框架，而是**没有设计系统**：

- 全项目 ~98 处 `Colour::rgb(...)` 硬编码，散落在 `src/gui.rs` 与 `src/gui/*.rs`，其中**近 30 种几乎一样的脏灰**（103/111/124、91/100/114、88/96/108、78/86/98…）——对比度低、色系不统一。
- 一次都没用过自绘能力（`on_paint` / `custom_widget!` / `AutoBufferedPaintDC` 在 `src/` 里 0 命中）；按钮全是原生灰控件，section 全是 `PanelStyle::BorderStatic` 生硬线框。
- 没有暗色模式；自绘位图（拓扑连接线 `[118,127,140,210]`、服务图标绿色）把颜色烤死在 RGBA 里，暗色下无法适配。

目标产出：①一套集中式设计 token（配色/间距/字号/圆角），统一全部页面观感；②暗色模式（跟随系统 / 强制亮 / 强制暗）；③用自绘圆角按钮替掉最显眼的原生灰按钮。约束：不破坏现有功能、`cargo build --features gui` 通过、改动可分阶段验证。

## 复用的现有模式（不重造）

- **配置持久化**：`src/config.rs` `AppConfig`（TOML，`#[serde(default, camelCase)]`），`load_or_default` / `save`。已有 `language: Option<String>` 先例 → 照搬加 `theme`。
- **偏好→GUI 接线先例**：`src/gui.rs:1372` 启动读 `config.language` 解析 locale；`install_system_menu`（1386）建语言子菜单；`handle_language_selected`（2851）存配置后弹「需重启」提示。**主题切换完全复刻这套**。
- **语义色先例**：`src/gui/widgets.rs:645` `StateTone{Ok,Warn,Error,Muted}` + `colour()` —— 扩展成 theme 取色。
- **自绘位图先例**：`src/gui/widgets.rs:492` `IconCanvas`（RGBA 画布）+ `topology_connector_bitmap`/`status_icon_bitmap` 经 `StaticBitmap` 入界面。
- **wxdragon 能力（vendored 已确认）**：`set_appearance(Appearance::{System,Light,Dark})`（`app.rs:379`，prelude 已导出）、`get_system_appearance()`、`is_system_dark_mode()`；DC 自绘 `draw_rounded_rectangle`/`set_brush`/`set_pen`/`gradient_fill_linear`/`set_clipping_region`/`draw_text`；`custom_widget!` 宏 + `AutoBufferedPaintDC` + `BackgroundStyle::Paint`；`Window` trait 的 `set_background_color`/`set_foreground_color`/`refresh`/`layout` 所有控件可用。
- **关键约束**：`set_appearance` 必须在 `Frame::builder()`（`src/gui.rs:162`）**之前**调用，建窗后改返回 `CannotChange`。

## 设计决策

1. **主题切换 = 存配置 + 提示重启**（与语言切换一致）。理由：`set_appearance` 必须建窗前调用，运行时切换原生控件不可靠；重启方案诚实、低风险、与现有 UX 统一。运行时实时切换列为未来项，不在本次范围。
2. **原生控件暗色交给 `set_appearance`**（wxWidgets 3.3 在 Win/macOS 自动处理 ListCtrl/TextCtrl/Notebook 等），**自绘面板与文字色交给 theme token**，两边取同一套语义色保持一致。
3. **集中式 token**：新增 `src/gui/theme.rs`，亮/暗各一套；全项目 `Colour::rgb(...)` 替换为 `theme.<token>`。

## 改动方案（分阶段，每阶段可独立编译验证）

### 阶段 1 — 主题基础设施（基础，必做）
- 新增 `src/gui/theme.rs`：
  - `struct Theme`：palette（`bg_app, bg_card, bg_card_alt, border, divider, ink_primary, ink_secondary, ink_muted, accent, accent_hover, on_accent` + 语义 `ok/warn/error/info` 及其柔和底色）、spacing 常量（`SPACE_XS..XL`）、`RADIUS`、字号/字体 helper。
  - `Theme::light()` / `Theme::dark()`（亮色沿用当前观感但收敛成约 6 档灰 + 1 主色；暗色新配）。
  - `enum ThemeMode{System,Light,Dark}` + `code()/from_code()`（仿 `GuiLocale`）；`resolve(mode)->Appearance` 与 `is_dark()`。
  - 全局取色：`thread_local!`/`OnceCell` 持有启动时解析好的 `Theme`，`theme::current()` 返回。GUI 单线程、启动构建一次，安全。
- `src/config.rs`：`AppConfig` 加 `theme: Option<String>`（`skip_serializing_if=Option::is_none`，仿 `language`）。

### 阶段 2 — 启动接线 + 主题菜单
- `src/gui.rs run()`（150）：建 `Frame` 前读 `config.theme` → `ThemeMode` → `set_appearance(...)` + 初始化 `theme::current()`。
- `install_system_menu`：在语言子菜单旁加「主题」子菜单（系统/亮/暗，单选打勾），仿 `handle_language_selected` 写 `handle_theme_selected`（存 `config.theme` + 弹重启提示，复用 `text.language_restart_message` 同款文案，新增 i18n 串）。
- `src/gui/text.rs`：加主题菜单相关 i18n 文案（中/英）。

### 阶段 3 — 用 token 替换硬编码色（核心，统一观感 + 暗色生效）
- 机械替换 `src/gui.rs` 与 `src/gui/*.rs` 全部 `Colour::rgb(...)` → `theme.<token>`，按用途归并：背景白(255³)→`bg_card`；脏灰群→`ink_secondary/ink_muted/border`；状态色→语义色。
- 重点文件：`src/gui/widgets.rs`（状态卡 86–230、`set_status_panel`/`set_im_channel_row`/`set_disabled_status_panel` 664–740、`StateTone::colour`）、`src/gui.rs`（顶部状态区、各 tab 背景 285/444/632…）、`request_log_detail.rs`、`session_history.rs`、`codex_tab.rs`、`onboarding.rs`。
- `IconCanvas` 自绘位图改为按 theme 取色重新生成（拓扑连接线、服务图标），暗色下可读。
- 把刺眼红警告（如微信 token 警告）改为语义 `warn` 柔和底色 + 文字。

### 阶段 4 — 自绘圆角按钮（现代化观感，最显眼的提升）
- 在 `src/gui/widgets.rs`（或新增 `src/gui/controls.rs`）用 `custom_widget!` + `AutoBufferedPaintDC` 实现圆角按钮：`primary/secondary/ghost` 变体 + hover/press（参考 `wxDragon/examples/rust/custom_widget/src/anim_fill_button.rs`），颜色取自 theme。
- 先替换主页（状态概览页）最显眼的动作按钮；其余 tab 渐进替换。统一 section 由 `BorderStatic` 硬线框改为 theme 卡片（圆角 + 柔边 + `bg_card`）。

### 阶段 5 — 间距/字号统一 + 验证
- 用 spacing/type token 统一主页与各 tab 的边距与标题/正文/辅助文字层级。
- 编译并实测亮/暗双模。

## 关键文件
- 新增：`src/gui/theme.rs`（必要时 `src/gui/controls.rs`）
- 改：`src/config.rs`、`src/gui.rs`、`src/gui/widgets.rs`、`src/gui/text.rs`，以及各 tab 模块（`request_log_detail.rs`/`session_history.rs`/`codex_tab.rs`/`onboarding.rs`/`request_logs.rs`/`im_accounts.rs`/`ai_gateway.rs`/`daemon.rs`/`update.rs`）做颜色替换
- 文档：本文件

## 验证
- `cargo fmt && cargo build --release --features gui --bin codexhub`
- 运行 GUI：默认（system）观感统一、无脏灰；菜单切「暗」并重启 → 整体暗色、文字可读、自绘位图与原生控件色系一致；切「亮」重启回亮色。
- `cargo test`（确认 config 序列化/反序列化含新 `theme` 字段不破坏现有用例）。
- 逐页目检：主页状态区 / Codex 接入 / 聊天工具接入 / 请求日志 / 会话历史 在亮暗两态都正常。

## 实现说明（已落地）

- **自绘按钮** `src/gui/controls.rs`：`ThemeButton` 用 `AutoBufferedPaintDC` 自绘圆角 + hover/press，变体 `Primary / Secondary / Ghost / Danger`，颜色全部取自 theme token。为保持和原生 `Button` 一样的 `Copy` 句柄语义（调用点到处按值传递），交互状态存放在 thread-local 注册表，按 panel 句柄查找；`ThemeButton` 本身只是 `{ panel }` 的 Copy 句柄。暴露 `on_click(move |_| ..)` / `enable` / `set_label`，并经 `Deref<Target=Panel>` 复用 `set_tooltip` 与 sizer 插入，所以调用点改动极小。
- **替换范围**：主窗口 / 页面 / 工具栏按钮全部换为自绘（AI Gateway 增删改、IM 接入、请求日志清理、模型操作、会话历史工具栏、日志搜索条等）。
- **保持原生**：模态对话框的 OK/Cancel（`with_id(ID_OK/ID_CANCEL)` + `set_default`）保留原生，以保住 Enter=默认 / Esc=取消 / 模态关闭语义；它们在暗色下由 `set_appearance` 自动主题化。
- **字号层次**：对话框/区块标题套用 `theme::font(TextRole::Title)`（加粗+略大），其余正文沿用系统默认，尊重系统 DPI。`theme::font` 必须经 `Font::builder()` 构造——`Font::new()` 是无效字体，`set_font` 后布局读取点大小会触发 wxWidgets 断言（`GetFractionalPointSize: invalid font`）。
- **状态卡片扁平化**：`build_status_panel` / `im_channel_row` 去掉原生 `BorderStatic` 蚀刻边框，靠白卡片与加深后的 `bg_app` 灰背景色差分层（亮色 `bg_app` 调到 237/240/245）。

### 已知小问题 / 后续
- 自绘按钮注册表只增不删：对话框反复打开会有极小泄漏（每个自绘按钮一条 `Rc`），后续可在 panel 销毁时清理。
- 暗色下部分 brand logo 位图自带浅色背景，观感略突兀（用户已确认暂不处理）。
- spacing token 已定义，但本轮未做跨页边距大改（现有间距尚可，避免不可目检的回归）。

## 范围与风险
- 不改任何业务逻辑、协议、IM/AI Gateway 行为，纯视觉层。
- 风险：暗色下原生控件（尤其 Linux/GTK 的 ListCtrl 表头）可能不完全跟随；策略是依赖 `set_appearance` 并对自绘部分兜底，验证阶段重点看列表/输入框。
- 运行时实时切主题（免重启）为未来项，不在本次范围。
