# MochiPort macOS SwiftUI 迁移计划

## 1. 文档状态

| 项目 | 内容 |
| --- | --- |
| 状态 | Phase 3 已完成；Phase 2 采用 launchd 基础托管、缺失时首次启动和用户主动安全重启，不再包含 daemon 自动构建、版本切换、候选 runtime、事务 journal 或自动回滚；Phase 4-6 的核心页面已接入真实版本化管理 API，请求日志服务端分页与增强启动完整状态机已完成，剩余重点为诊断导出和发布级无障碍收尾 |
| 最近核验 | 2026-08-18；自动 daemon 构建与运行时切换代码已删除，SwiftPM 134 项通过，Rust 912 项通过（1 项忽略），Xcode 双架构 Debug 构建成功。增强启动 operation 的幂等、并发冲突、取消和失败恢复仍有契约测试覆盖 |
| 目标版本 | 0.5.x 预览阶段 |
| 主平台 | 正式 Xcode App 与 SwiftPM manifest 均以 macOS 26 为 deployment target |
| 主前端 | SwiftUI |
| 核心后端 | 现有 Rust daemon |
| Windows | React/Tauri 客户端，持续维护 |
| Linux | 暂不发布桌面客户端，仅保留 daemon/CLI；未来单独设计客户端 |

本文记录 MochiPort 从旧 Rust/wxDragon 桌面界面迁移到 macOS SwiftUI、Windows React/Tauri 的产品形态和验收边界。

## 2. 决策摘要

MochiPort 的 macOS 客户端改用 SwiftUI 重写，现有 Rust daemon、Telegram、飞书、微信、企业微信、会话管理、远程控制和 AI Gateway 逻辑继续共用。Liquid Glass 是 macOS 客户端的核心产品要求和正式发布门槛，不是功能完成后的装饰性优化。

迁移完成后的平台策略如下：

- macOS：SwiftUI 是唯一持续演进的正式前端，正式包和 SwiftPM 均以 macOS 26 为目标，并以系统原生 Liquid Glass 为旗舰视觉实现。
- Windows：发布 React/Tauri 客户端，界面与 macOS SwiftUI 独立演进。
- Linux：暂不发布桌面包，保留 daemon/CLI；未来另行设计新客户端，不恢复 wxDragon。
- Rust daemon：仍是所有平台的业务事实来源，Swift 代码不得复制 Telegram、飞书、会话或 AI Gateway 业务规则。

旧 Rust/wxDragon GUI 已退役，不再参与 macOS、Windows 或 Linux 的构建与发布。

## 3. 目标与非目标

### 3.1 目标

- 提供符合 macOS 交互习惯的导航、设置、表格、搜索、菜单栏、窗口恢复和系统反馈。
- 将原生 Liquid Glass 的层级、材质、动效和可访问性纳入设计与发布全过程。
- 使用 SwiftUI 的状态驱动模型替代当前 GUI 中大量手动控件更新和定时刷新逻辑。
- 保持现有配置、凭据、会话绑定、Provider、请求日志和更新数据完全兼容。
- 保持 Rust daemon 独立运行，并通过稳定的本地 API 服务 SwiftUI、CLI 和兼容前端。
- 迁移期间始终保留一个可工作的正式版本，并能回滚到 SwiftUI macOS 应用或 Windows React/Tauri 客户端。
- 让 macOS 前端具备独立的单元测试、UI 测试、签名、公证和发布流程。

### 3.2 非目标

- 不用 Swift 重写 Rust daemon 或 IM 平台适配器。
- 不在本次迁移中重做 Telegram、飞书等聊天端的消息样式。
- 不追求 macOS、Windows、Linux 三个平台界面像素一致。
- 不同时开发 WinUI、GTK 或另一套 Linux 原生 GUI。
- 不把 SwiftUI 直接绑定到 Rust 内存结构，也不在第一版引入 Rust/Swift FFI。
- 不借迁移之机重新命名旧数据目录或兼容标识。

## 4. 当前基线

历史桌面界面曾使用 Rust + wxDragon（wxWidgets），现已从仓库和构建链删除。核心后台使用 Rust + Tokio + Axum，本地服务默认监听 `127.0.0.1:3847`。

现有 GUI 覆盖以下能力：

- 总览：Codex App、VS Code、CLI、本地服务、远程控制和 IM 状态。
- Codex：本地接入、卸载、增强启动、可见模型和会话历史。
- AI Gateway：Provider 管理、协议、Base URL、权重、模型和相关开关。
- IM 账号：Telegram、飞书、微信、企业微信的添加、启停、删除和引导。
- 请求日志：列表、清理、请求详情、响应、SSE、错误和内容搜索。
- 桌面能力：菜单栏状态项、主题、语言、网络设置、诊断导出、更新和安全重启。

Rust daemon 已提供 `/api/v1/manage/dashboard`、版本化设置、Codex App、IM 账号、Onboarding、请求日志、Bridge 启停和安全关闭等接口，因此 SwiftUI 可以通过 HTTP API 接入，不需要直接链接 Rust 库。

## 5. 目标架构

```text
MochiPort.app
├── Contents/MacOS/MochiPort                 # SwiftUI 主程序
├── Contents/Helpers/mochiport-daemon         # Rust 可执行文件
├── Contents/Helpers/mochiport-gui-supervisor # GUI 恢复辅助程序
├── Contents/Frameworks/                      # 仅在选择更新框架后加入
├── SwiftUI macOS GUI
│   ├── URLSession API Client
│   ├── Codable API Models
│   ├── App/Window/MenuBar lifecycle
│   └── SwiftUI views and view models
└── MochiPort Rust daemon
    ├── local HTTP/WebSocket API on 127.0.0.1:3847
    ├── Codex remote-control backend
    ├── IM adapters
    ├── AI Gateway
    └── persistent config and state
```

### 5.1 进程模型

第一版继续采用 GUI 与 daemon 分离的双进程模型：

1. SwiftUI 应用启动后探测既有 daemon。
2. 若存在匹配的 MochiPort daemon，或可识别的旧 ThreadRelay/CodexHub 兼容 daemon，则直接复用，不因 GUI 与 daemon 版本不同而自动替换或重启。
3. 若不存在，则校验并安装随包 helper，由 launchd 首次启动。
4. 关闭主窗口永远不停止 daemon；默认隐藏到菜单栏，用户可在 Settings 改为退出 GUI，但这仍不等于停止本地服务。
5. 所有停止、重启和升级操作必须校验 daemon PID、instance ID、可执行文件路径和端口归属。

禁止只根据进程名或端口直接杀进程。当前模型请求可能经过 MochiPort AI Gateway，错误关闭 `3847` 会中断正在进行的会话。

GUI 关闭、崩溃和自身更新不得通过父进程退出、进程组、继承管道或临时目录清理带走 daemon。daemon 启动后脱离 GUI 的标准输出和错误管道，日志写入固定文件；退出主界面和停止本地服务必须是两个不同命令。

运行时控制文件与业务数据分开，至少记录管理凭据、daemon 身份和管理租约。现有 `CodexHub/config.toml`、状态、Provider 和请求日志继续由 Rust 兼容读取，本次迁移不搬目录。

实现只需区分四类产品状态：未运行、可复用、由当前安装管理、身份冲突。排空和更新是短暂过程状态，不需要暴露成另一套用户概念。GUI 只能停止自己启动或完成认证接管的 daemon；遇到身份或端口冲突只提供诊断，不结束对方进程。

同一用户数据域只运行一个 daemon。stable 与 preview 都可读取状态并通过 revision 写业务配置，但任一时刻只有持有管理租约的已验证安装可以轮换管理凭据、执行用户主动重启或停止 daemon。账号与凭据、Provider、网络设置、Codex 配置和卸载、日志开关与清理等业务写操作不要求管理租约，但必须校验共享凭据、instance ID 和资源 revision；破坏性操作还要使用显式确认与幂等请求 ID。管理者已退出时，另一安装在重新核验 PID、instance ID、可执行路径和端口归属后可接管；管理者仍存活时必须显式确认接管，失败则退回只读。Phase 0 的控制平面 ADR 冻结共享控制文件、凭据发现与轮换、租约获取与回收、多 GUI 并发写和崩溃恢复规则；Phase 2 不提供版本切换协议。

### 5.2 通信边界

SwiftUI 默认仅通过 loopback HTTP 与 daemon 通信：

- `URLSession` 负责请求。
- `Codable` 类型承载 API 契约。
- 所有修改操作由 daemon 校验并持久化。
- SwiftUI 只维护界面瞬时状态，不保存第二份业务配置。
- 初期使用前台刷新和短轮询；需要实时性的状态统一收敛到一个事件流接口后再启用 SSE 或 WebSocket。

每个写接口应具有明确请求 DTO，避免 SwiftUI 为修改一个字段而回传整份 `AppConfig`。保留 `/api/config` 作为兼容接口，但新前端逐步改用窄接口。

#### 管理 API 安全边界

Loopback 不是鉴权。API 按用途分区：

- `/healthz`：唯一无需管理凭据的管理面端点，只返回 `service`、API 主版本和 `ready`，不返回版本细节、进程身份、路径、配置或运行状态。
- `/api/v1/manage/*`：Dashboard、配置、账号、Onboarding、日志、诊断、关闭和更新接口，全部要求 bearer token。
- `/backend-api/*`、`/ai-gateway/*`：继续遵循 Codex 兼容和模型调用协议，不复用 GUI 管理凭据。

管理凭据按当前用户数据域生成，由 daemon 控制平面维护在仅当前用户可读的共享控制文件中，stable、preview 和桥接版按同一发现协议读取；它不属于某个 App 安装。凭据在首次初始化、可信接管和泄漏恢复时轮换，不得进入 App Bundle、命令行、环境变量、崩溃报告、日志、fixture 或诊断包。GUI 在发送凭据前先确认端口所有者与受信 helper 身份。

这套凭据用于阻止匿名 HTTP 调用、浏览器跨站请求、其他系统用户和意外连错实例，不宣称防住当前用户权限下的恶意进程；若未来需要后者，应另行采用 XPC audit token 等客户端身份机制。管理服务限制 loopback `Host`，不启用 CORS，拒绝浏览器跨站管理请求。所有业务写操作校验凭据、instance ID 和资源 revision；只有上一节列出的 daemon 控制平面操作额外要求管理租约。

Phase 0 先审计全部旧 `/api/*`：除新 `/healthz` 外，旧管理端点必须接入同一鉴权或删除，不再保留第二个匿名健康接口。macOS SwiftUI 与 Windows React/Tauri 只使用版本化 API；Linux daemon/CLI 仅保留必要的兼容接口并必须鉴权。

统一脱敏策略覆盖 HTTP 访问日志、chain log、daemon 启动日志、崩溃报告、fixture 和诊断导出。必须移除或替换 Authorization、Cookie、管理凭据、Provider Key、Bot Token、App Secret、代理用户名/密码、二维码内容、device code、验证码、请求体敏感字段和本地用户标识。脱敏测试使用 canary secret，任何产物中出现原值都视为测试失败。

#### 手动生命周期协议

SwiftUI GUI 更新不重启 daemon。只要 API 兼容，新 GUI 继续复用旧 daemon；若 daemon 源码或随包 helper 已更新，界面仅提示用户手动重启后台服务，不在后台构建、准备或切换版本。

设置页保留受鉴权的用户主动重启操作。重启前由 daemon 检查 AI Gateway 请求、Codex turn、IM 流式消息、最终回复和审批动作等受保护工作项；存在阻塞项时拒绝本次操作，不提供自动重试或强制切换。该操作只重启当前 launchd 配置指向的 daemon，不安装或升级 helper。

### 5.3 实施默认值

以下选择作为 Phase 0 的默认实现。若后续需要改变，应先提交单独的架构决策记录，不在页面开发中临时调整。

| 事项 | 推荐默认值 | 原因 |
| --- | --- | --- |
| 主程序与 daemon | SwiftUI 主程序 + `Contents/Helpers/mochiport-daemon` | 保留 Rust 核心，并让签名、进程归属和升级边界清晰 |
| macOS 正式包目标 | macOS 26 | 正式 Xcode App 和 SwiftPM manifest 均使用 macOS 26 SDK 与 deployment target |
| Swift 状态模型 | Swift 并发、`@MainActor`、`ObservableObject` | 正式包与 SwiftPM 测试都不依赖更高的 Observation framework |
| 第三方架构库 | 首版不引入 | 优先使用 SwiftUI、Foundation、OSLog、XCTest；更新框架按 Phase 6 决策 |
| App Sandbox | 首个 SwiftUI 正式版不启用 | 当前需要启动 helper、访问既有配置和日志、连接本地服务并协同外部应用；先使用 Hardened Runtime、签名和公证 |
| 凭据存储 | 首版保持现有 Rust 配置兼容 | UI 只向窄写接口提交新密钥，读接口仅返回 `secretSet`；Keychain 迁移作为独立安全项目，避免阻塞界面迁移 |
| 本地管理 API | loopback + 当前用户数据域共享凭据 | 防止匿名 HTTP、浏览器跨站、其他系统用户和连错实例；不把同一用户权限下的恶意进程纳入首版威胁模型，远程控制兼容端点不受影响 |
| CPU 架构 | 继续发布 Universal（arm64 + x86_64） | 当前发布链已经支持两种架构，迁移阶段不额外制造平台回退 |
| 自动更新 | 预览期只保留手动检查和安全切换；Sparkle 2 是否进入首个 stable 在 Phase 6 决定 | 自动更新不阻塞核心 GUI 迁移，采用时仍必须满足签名、排空和回滚门槛 |

SwiftUI 首版不负责把旧 `Application Support/CodexHub` 数据搬到新目录。兼容读取策略继续由 Rust daemon 负责，目录迁移在稳定版之后单独设计。

### 5.4 版本、升级身份与兼容窗口

- UI 产品语义版本和构建号由 `MOCHIPORT_UI_VERSION` / `MOCHIPORT_UI_BUILD_NUMBER` 提供；daemon 版本来自 Rust `Cargo.toml`，构建号由 `MOCHIPORT_DAEMON_BUILD_NUMBER` 提供。
- UI 与 daemon 独立检查和发布更新；构建脚本分别校验 Xcode、daemon `--version`、Info.plist 和发布清单，不再要求两者共享同一构建号。
- `/healthz` 仅返回 `service=threadrelay`、API 主版本和 `ready`；受鉴权运行状态接口另行返回 `productVersion`、`buildNumber`、`apiMajor`、PID、启动时间和 runtime 状态。
- Bundle ID 固定为 `io.github.mps233.threadrelay`，正式签名 Team ID 在 SwiftUI 迁移前后保持一致。
- `swiftui-preview` 使用独立 Bundle ID 和更新 feed，可与 stable GUI 并行安装；两者复用同一兼容 daemon，不各自启动服务。
- 旧 `com.codexhub.app` 数据仍由 Rust 兼容路径读取，不再提供 wxDragon 桥接版。
- 管理 API 面向 macOS SwiftUI、Windows React/Tauri 和未来可能的 Linux 客户端，配置写操作使用窄 DTO 和 revision/ETag。
- Linux CLI 遵循 daemon API 兼容窗口；需要调整时单独发布适配版本。

## 6. macOS 体验原则

### 6.1 基础体验原则

- 使用 `NavigationSplitView` 表达稳定的信息架构，不复制现有顶部状态区加多标签页布局。
- 总览用于快速判断服务是否正常，并提供最常用的恢复动作；配置项进入对应功能页或 Settings。
- 使用系统 `Table`、`List`、`Form`、`Toolbar`、`MenuBarExtra`、`Settings` 和标准 sheet；`Inspector` 按系统版本启用。
- 采用系统语义颜色、材质、控件尺寸和 SF Symbols，不手工模拟跨平台控件外观。
- 支持浅色、深色、增大对比度、减少动态效果和 Dynamic Type 能覆盖的字号变化。
- 破坏性操作使用明确确认，长任务展示可取消进度，错误信息给出下一步操作。
- 列表保留选择、排序、搜索和键盘操作；常用命令进入菜单栏和 Command Menu。
- 主窗口、详情窗口和 Settings 的状态恢复遵循 macOS 习惯。
- 动画只用于解释状态变化，优先短时、可中断的系统动画，不加入装饰性动效。
- 正式 Xcode App 和 SwiftPM 测试 target 均为 macOS 26。`.inspector` 和 Liquid Glass 按系统可用性隔离，兼容路径使用同内容的导航详情页。

### 6.2 Liquid Glass 核心产品要求

Liquid Glass 是 P0 级产品需求。任何核心页面即使功能完整，只要材质层级、控件形态、转场、可读性或旧系统回退未达到本节要求，就不得标记为完成，也不得进入 `stable`。这里追求的是 Apple 原生视觉语言和交互行为，不是把所有区域做成半透明玻璃。

| 界面层级 | 要求 |
| --- | --- |
| 导航与控制层 | macOS 26 及以上优先使用系统 `NavigationSplitView`、toolbar、search、sheet、popover、Inspector 和菜单栏瞬时界面，让系统自动采用原生 Liquid Glass |
| 悬浮与组合控制 | 只有在控制层真实覆盖内容时才使用自定义 glass；相关控件需要共同变形或衔接时使用 `GlassEffectContainer`，并采用系统提供的 glass effect、button style 和 transition API |
| 业务内容层 | `Table`、`List` 行、请求日志、表单、账号与 Provider 内容、总览数据区保持清晰稳定的系统表面，不逐项加玻璃、描边卡片或彩色底板 |

具体约束如下：

- 原生组件优先于自定义 `glassEffect`。能由系统 toolbar、search、sheet、popover 或 sidebar 自动获得的效果，不重复包一层自定义玻璃。
- 默认使用系统常规 glass 变体；clear glass 只允许出现在视觉内容足够丰富、对比度经过验证的场景。MochiPort 以运维信息为主，原则上不使用 clear glass。
- 玻璃只属于导航、控制和临时浮层，不覆盖主要阅读与数据内容。禁止“每个卡片一块玻璃”、多层玻璃嵌套、装饰性渐变、随意染色和常驻高光。
- 内容区采用统一表面与细分隔线形成层级，避免碎片化卡片。颜色主要用于系统强调色、警告和错误，不用颜色填充空间。
- macOS 26 代码使用系统原生 API；业务内容不实现自制 shader、叠层模糊或截图材质。
- 系统开启“降低透明度”“增强对比度”或“减弱动态效果”后，界面必须自动收敛到清晰、稳定且可读的表现；不得依赖透明度、颜色或形变单独表达状态。
- 所有自定义玻璃转场必须短时、可中断，并尊重系统动态效果设置。滚动、调整窗口和切换选择时不能出现材质闪烁、边缘跳变或内容重排。
- Phase 0 先建立总览、请求日志、Settings 和 sheet/popover 的视觉基准；后续页面复用同一套语义层级，不允许各自发明玻璃样式。

实施和评审以 Apple 的 [Liquid Glass 技术概览](https://developer.apple.com/documentation/TechnologyOverviews/liquid-glass)与[材质设计指南](https://developer.apple.com/design/human-interface-guidelines/materials)为规范来源。

## 7. 信息架构与页面设计

### 7.1 主窗口与导航

主窗口使用 `NavigationSplitView` 的 Sidebar + Content 两栏；macOS 26 可在列表页额外打开系统 Inspector。不把旧 Rust GUI 的标签页原样搬过来，也不把 Inspector 当作第三个导航栏。

```text
┌──────────────┬───────────────────────────────────┐  ┌──────────────┐
│ Sidebar      │ Content                           │  │ Inspector    │
│ 总览          │ 当前功能的状态、列表、表单或导航详情       │  │ macOS 26+    │
│ 工作区        │                                   │  │ 列表页按需显示 │
│  Codex       │                                   │  └──────────────┘
│  会话         │                                   │
│ 连接          │                                   │
│  消息通道      │                                   │
│  AI Gateway  │                                   │
│  请求日志      │                                   │
└──────────────┴───────────────────────────────────┘
```

Sidebar 只保留六个一级入口：

| 分组 | 入口 | 作用 |
| --- | --- | --- |
| 顶部 | 总览 | 判断系统是否可用，处理当前最重要的问题 |
| 工作区 | Codex | 管理本地接入、增强启动和可见模型 |
| 工作区 | 会话 | 搜索历史会话并移动 Provider |
| 连接 | 消息通道 | 管理 Telegram、飞书、微信和企业微信账号 |
| 连接 | AI Gateway | 管理模型 Provider、协议、权重和 Gateway 开关 |
| 连接 | 请求日志 | 查询模型请求、错误、耗时和协议转换详情 |

网络、外观、语言、更新、诊断和高级本地连接进入独立 `Settings` 场景，不占 Sidebar。About 与检查更新进入应用菜单；常驻服务状态同时出现在总览和菜单栏，不额外创建“服务”页面。

窗口默认宽度约 1040、最小宽度约 760。macOS 26 且空间足够时，列表类页面可在 Content 右侧显示 Inspector；空间不足时，选择行后在 Content 内导航到详情并提供返回。Sidebar 可折叠，具体宽度由原型验证。

### 7.2 信息与操作层级

所有页面共享同一信息层级，但只在内容需要时使用 Detail：

1. Sidebar 决定当前功能，不显示频繁变化的明细数字。
2. Toolbar 放页面标题、搜索、筛选和一个主要动作，例如“添加账号”或“添加 Provider”。
3. Content 放可扫描的状态、列表或表单，是主要工作区。
4. Detail/Inspector 只用于有“选择对象”关系的会话、账号、Provider 和请求日志；总览与 Codex 保持单一 Content，不制造空详情栏。
5. Sheet、popover 和 alert 只承载短暂流程；需要扫码、分步验证或较多字段的 Onboarding 使用 sheet，确认危险操作使用 alert。

主要动作每页最多一个，位于 toolbar 尾部。添加类命令优先使用系统 `+` 图标并提供 tooltip、菜单和键盘等价路径；只有需要强调文字结果的命令才用 prominent 文本按钮。行内只保留高频、可逆操作；编辑、复制诊断等次要操作进入 toolbar、Detail 或 context menu。删除账号、清空日志、恢复配置和立即停止 daemon 等破坏性操作不得与主要动作并排，也不得仅靠红色表达风险。

### 7.3 核心页面

| 页面 | Content | Detail / 流程 | 主要动作 |
| --- | --- | --- | --- |
| 总览 | 顶部显示整体结论和首要恢复动作；下方依次为本地服务、执行端、消息通道、AI Gateway 四个统一 section。执行端逐行显示 Codex App、VS Code、CLI 及 remote-control 状态 | 问题行就地展开原因、最近更新时间和诊断入口；正常状态不展示冗长说明 | 未运行或崩溃时显示“启动/重新启动”；受管服务的“安全重启/停止”放 section 更多菜单；冲突时只允许重试与诊断 |
| Codex | 用分组表单展示本地接入、增强启动和可见模型；当前状态始终靠近对应控制 | “写入配置”和“增强启动”留在各自 section；修复放在接入状态旁；“恢复原配置”和卸载放 section 更多菜单，确认后进入可取消进度 sheet | toolbar 不放会随状态变成危险命令的主按钮 |
| 会话 | 可搜索、可排序的会话列表，显示项目、时间、执行端和当前 Provider | macOS 26 的 Inspector 显示完整路径、绑定状态和 Provider 移动控件；窄窗口导航到详情页 | 默认没有全局主要动作；操作针对所选会话 |
| 消息通道 | 按平台分组的账号列表，显示名称、连接状态和最近错误；行尾开关直接启停，失败时回滚；不为四个平台各建一级页面 | 添加账号使用 Onboarding sheet；选中账号后在 macOS 26 Inspector 或窄窗口详情页编辑，显式“保存”；删除放更多菜单并确认 | 添加账号 |
| AI Gateway | Provider 列表展示启用状态、协议、权重和模型数量；行尾开关直接启停，失败时回滚；Gateway 全局开关放在列表上方紧凑控制行 | macOS 26 Inspector 或窄窗口详情页编辑 Base URL、密钥状态、模型映射和高级选项，显式“保存”；密钥只写不回显，删除放更多菜单并确认 | 添加 Provider |
| 请求日志 | 全高 `Table`，toolbar 提供搜索、筛选和日志开关；状态、耗时、模型等列支持排序 | macOS 26 使用 Inspector 按“摘要、Codex 请求、上游请求、响应、事件、错误”分区并按需加载；窄窗口导航到详情页，并可另开详情窗口 | 默认没有创建动作；“清空日志”放更多菜单 |

总览不是第二套设置页。它只回答三件事：MochiPort 能不能工作、问题在哪里、下一步该做什么。正常项目以紧凑行呈现；只有异常项目展开解释和动作，避免状态卡片铺满窗口。

列表类页面的 Detail 规则保持一致：macOS 26 且空间足够时使用系统 Inspector；空间不足时进入同内容的导航详情页。总览和 Codex 从不显示 Inspector。独立日志窗口是额外工作方式。

会话列表支持多选。单选时 Inspector/详情页显示会话信息；选中一个或多个会话时，toolbar 出现“移动 Provider”动作，popover 选择目标并预览数量，确认后执行。部分失败时保留失败项选择并列出原因，已成功项不回滚。

AI Gateway 的运行开关与 Provider 列表放在该页面。请求日志页的控制行依次放“记录请求”和“记录详情”，后者依赖前者，关闭请求日志时自动禁用但保留详情偏好；“过滤图像生成工具”属于 Gateway 高级设置，不混入日志 toolbar。

请求日志采用服务端分页和虚拟化 `Table`，底部显示当前范围、总数、上一页和下一页，筛选变化回到第一页。详情 toolbar 提供复制、查找、敏感字段显示/遮罩和在独立窗口打开；默认遮罩敏感字段，JSON 与事件正文使用等宽字体。

### 7.4 Settings、菜单栏与辅助窗口

`Settings` 使用系统 Settings 场景，分为四个标签：

- 通用：语言、外观、关闭主窗口后的行为；可选隐藏到菜单栏或退出 GUI，两者都不停止 daemon。
- 网络：系统代理、直连、自定义代理。
- 本地服务：本地连接模式，以及明确停止受管 daemon 的独立危险操作；只有现有 daemon 支持的连接参数才展示，高级项默认收起。
- 更新与诊断：检查当前安装所属 feed 的更新、版本、导出诊断和打开日志目录。stable 与 preview 是不同 Bundle ID，不在设置中互相切换。

菜单栏只提供当前整体状态、打开 MochiPort、启动或查看本地服务、检查更新和退出。它不是缩小版主窗口，不在菜单栏编辑账号、Provider 或网络设置。点击异常状态应打开主窗口并定位到相关页面；`MenuBarExtra` 无法可靠深链时至少打开总览并聚焦首要问题。

应用命令保持精简并使用系统惯例：File 提供关闭窗口；View 提供显示 Sidebar、显示 Inspector（支持页面才启用）和刷新；Edit 使用系统查找命令聚焦当前列表或详情搜索；Window 提供主窗口与日志详情窗口；Help 提供帮助、隐私说明和检查更新；Settings 使用系统快捷键。删除、移动 Provider、停止服务等对象命令同时提供 toolbar/context menu 入口，首版不自创全局快捷键。

扫码和多步连接使用可取消 sheet。Onboarding 共享“选择平台 -> 提供凭据或扫码 -> 等待验证 -> 完成”四步骨架：Telegram 在凭据步输入 Token；飞书和企业微信按 daemon 能力显示扫码或凭据；微信在扫码后可插入验证码步骤。每一步都能返回或取消；二维码/验证码过期留在当前步刷新，失败留在当前步重试，成功后关闭 sheet 并选中新账号。平台特有字段在 Phase 3 原型中确定，不扩散到主窗口信息架构。

请求日志的大文本可以从 Inspector 打开独立详情窗口，方便并排比较和搜索。除 Settings、About 和日志详情外，首版不增加多窗口工作流。

### 7.5 页面状态与反馈

- 首次加载保留页面骨架和稳定尺寸，不用空白窗口或全屏 spinner。
- 无数据时说明缺少什么，并只给一个下一步动作；不把“暂无数据”伪装成错误。
- 后台刷新保留旧内容并标记更新时间；只有首次加载失败才替换整个内容区。
- 错误显示在受影响对象附近，并提供重试、打开设置或查看诊断等具体动作；同一错误不同时弹 alert、banner 和通知。
- 长任务在原位置显示阶段和取消入口，完成后就地更新；不为普通成功操作弹 modal。
- daemon 离线时保留可读取的最后状态，但禁用需要写入的动作，并明确数据可能过期。
- daemon 身份或端口冲突时，总览显示阻塞性问题，只提供“重试检查”和“查看诊断”；不得提供停止未知进程。API 不兼容时说明需要更新 GUI 或 daemon，并禁用不受支持的写操作。
- 列表选择、Sidebar 位置、Inspector 开关和搜索条件按场景恢复；敏感输入和临时错误不恢复。

### 7.6 Liquid Glass 在层级中的落点

Sidebar、toolbar、search、sheet、popover、Inspector 和菜单栏瞬时界面属于导航与控制层，在 macOS 26 交给系统呈现原生 Liquid Glass。Content 中的表格、列表行、表单、日志文本和状态 section 使用稳定内容表面与系统分隔线，不再套玻璃卡片。

悬浮控制只有在真实覆盖可滚动内容且系统组件不能表达时才允许自定义 `glassEffect`。同组悬浮控件需要共同变形时才使用 `GlassEffectContainer`；首版不为了展示技术效果创造悬浮按钮或自定义 morph 动画。

## 8. API 准备

API 按页面渐进准备：公共安全与版本骨架在 Phase 0 完成；每个页面的端点和 fixture 在该页面实现前完成。

- [x] 固化已实现页面的版本化 API 约定；Dashboard、生命周期和 IM 账号接口已有脱敏 fixture，后续页面继续按 Phase 4-6 补齐。
- [ ] 将新增写操作收敛为窄 DTO；凭据只写，读取只返回是否已设置；所有写操作定义校验、幂等性和统一错误格式。
- [ ] 将管理 API 与 Codex/AI Gateway 协议分区，完成本地鉴权、浏览器跨站防护和统一脱敏。
- [ ] 返回 API 版本、能力和状态 revision，让 GUI 能识别不兼容 daemon 并避免多页面竞态。
- [x] 请求日志使用服务端分页、筛选和排序，详情按需加载。
- [ ] 用当前及兼容版本 fixture 覆盖旧数据目录、配置字段和跨平台客户端，不要求旧 GUI 在 Phase 0 全量迁移。

API 完成标准：Swift 端不需要导入或复制 Rust 内部实现类型，只依据公开 JSON 契约即可完成对应页面。

## 9. 分阶段实施

### Phase 0：工程骨架与设计基线

当前状态：基础骨架、版本化管理 API、SwiftUI 视觉基线和 CI 验证已完成；helper 生命周期、完整租约状态机、桥接版验证、旧管理端点全面收口和 UI 自动化仍留在后续工作。下列清单只标记已经有代码与验证证据的项目。

交付物：

- [x] 创建 `macos/ThreadRelay` SwiftUI 工程，正式 Xcode 与 SwiftPM deployment target 均为 macOS 26。
- [x] 固化 stable/preview Bundle ID、Cargo 版本来源、构建号和 Debug/Release scheme。
- [ ] 补齐图标、签名能力和正式包签名验证。
- [x] 使用 Swift 并发、`@MainActor` 和 `ObservableObject` 建立 `APIClient`、错误模型、依赖注入和只读 fixture 加载能力。
- [x] 建立 Rust helper 的包内复制、嵌套签名和版本一致性校验。
- [x] 记录 App Sandbox 暂缓、Universal 构建、凭据兼容和 API 认证四项架构决策。
- [ ] 冻结并测试控制平面 ADR：共享凭据的发现与轮换、唯一管理租约、stable/preview 仲裁、接管、并发写和崩溃恢复。
- [x] 实现共享管理凭据、唯一匿名 `/healthz` 和受鉴权 API v1 骨架；已用路由测试验证 bearer 校验和脱敏 dashboard。
- [ ] 完成管理 API 的浏览器 Host/CORS 防护，并将其余 legacy 管理端点接入同一鉴权或删除。
- [x] 固化 stable/preview Bundle ID、版本来源和 API 主版本。
- [ ] 冻结 Team ID 和完整 API 兼容窗口，并完成正式签名验证。
- [x] 删除旧 Rust/wxDragon GUI、wxDragon vendor、Linux AppImage workflow 和旧 Linux 桌面入口。
- [x] 建立语义化设计 token，但颜色和材质优先引用系统值。
- [x] 建立 Liquid Glass 使用边界；macOS 26 使用原生 API，不实现仿制玻璃渲染器。
- [x] 为总览、请求日志、Settings 和 sheet/popover 制作可运行的视觉基准，并提供不接触真实 daemon 的 `ThreadRelayPreview` fixture scheme（工程内部名称保留）。
- [ ] 完成 macOS 26 浅色/深色的截图和可读性评审。
- [x] 创建主导航、Settings、About、菜单命令和占位页面。
- [x] 增加 Swift 单元测试 target。
- [ ] 增加 UI 测试 target 和真实交互自动化。
- [x] 在 CI 中加入 `xcodebuild build` 和 `xcodebuild test`，暂不替换正式产物。

进入 Phase 1 前只冻结会影响兼容性或返工成本的决策：helper 身份与生命周期、本地 API 鉴权、凭据事实来源、Bundle/版本身份、更新回滚、Universal 架构范围和 macOS 26 SwiftPM 构建路径。内部目录、字段命名和等待阈值可在对应 ADR 与测试中继续收敛。

验收：SwiftUI 空壳能独立构建、签名和启动；API v1 安全骨架与 fixture 测试通过；不会停止或修改当前 Rust daemon；视觉基准在 macOS 26 浅色/深色完成评审，没有玻璃覆盖业务内容或自制仿制材质。

### Phase 1：只读总览

交付物：

- [x] 对接 `/healthz` 与 `/api/v1/manage/dashboard`；旧 `/api/status` 仅保留给 CLI 状态探测和兼容检查，旧 GUI 聚合端点已删除。
- [x] 展示 daemon、Codex App、VS Code、CLI、remote-control、Telegram、飞书、微信、企业微信和 AI Gateway 状态，完整对应第 7.3 节总览的四个 section。
- [x] 支持加载、空、离线、部分失败和陈旧数据状态。
- [x] 支持手动刷新和统一自动刷新，窗口不可见时降低刷新频率。
- [x] 建立脱敏诊断摘要复制和打开日志目录入口。

验收：使用固定 fixture 时 SwiftUI 与 daemon 状态分类结果一致；连续运行只读预览版 24 小时，不产生第二个 daemon、不写业务配置、不触发 daemon 重启，由预览版主动终止的受保护工作项数为 0。

当前验证边界：fixture、API 契约、SwiftPM、Xcode 与 Rust 测试已通过；真实 daemon 的 24 小时长稳验证仍未勾选。隔离端口冒烟发现 Rust daemon 启动会同步 Codex App 环境，因此在 Phase 2 拆分“只读探测”和“受管启动”前，不把启动隔离实例计为 Phase 1 只读验收完成。

### Phase 2：daemon 生命周期与菜单栏

当前状态：正式 App 已内嵌 Universal Rust daemon，并通过 LaunchAgent 维持 daemon 与 GUI supervisor。SwiftUI 会复用已经运行的 daemon，只有在服务不存在时才安装并首次启动随包 helper；普通启动和版本不一致都不会替换或重启现有 daemon。设置页保留用户主动的安全重启，受保护工作存在时由 daemon 拒绝操作。自动 runtime staging、切换 journal、候选 hold、失败回滚和 GUI 驱动的版本恢复均已删除。

2026-08-16 的现场只读证据为 GUI build 422 复用正式 daemon build 410，升级期间 daemon PID 70418 未变化；`/healthz`、`/api/v1/manage/dashboard`、`/api/v1/manage/lifecycle`、`/api/v1/manage/codex/status`、`/api/v1/manage/settings`、`/api/v1/manage/im/accounts`、`/api/v1/manage/gateway` 和 `/api/v1/manage/request-logs` 均返回 200。该证据证明新 GUI 可以读取并继续使用兼容的旧 daemon；不兼容或无法识别 build 的 daemon 必须提示用户手动更新后台服务。

交付物：

- [x] 实现现有 daemon 探测、精确 PID、instance、路径、参数和环境校验、启动及受保护关闭。
- [x] 将 Rust daemon 作为独立可执行文件嵌入 App Bundle。
- [x] 服务不存在时校验并安装随包 helper，由 launchd 首次启动；服务已存在时不复制或替换 helper。
- [x] 按 Phase 0 控制平面 ADR 使用和轮换共享管理凭据，落实唯一管理租约与可信接管；控制文件使用稳定锁与原子替换，接管和泄漏恢复均通过 generation、daemon 身份快照和幂等 request ID 防止竞态。
- [x] 实现 daemon 管理租约、受保护工作检查，以及只针对当前 daemon 的用户主动安全重启。
- [x] 实现 `MenuBarExtra`：打开主窗口、服务状态、检查更新、退出。
- [x] 处理重复启动和 GUI 崩溃恢复，GUI 生命周期不影响 launchd 管理的 daemon。
- [x] 完成固定 fixture 与旧 daemon 的只读 API 兼容验证；GUI build 422 复用正式 daemon build 410 时主要管理路由均返回 200，daemon PID 在 GUI 升级期间未变化。
- [ ] 在隔离环境验证用户主动重启只重启当前 daemon，存在受保护工作时可靠拒绝，且不会升级或切换版本。
- [x] 实现“关闭窗口后隐藏或退出 GUI”的偏好；两种行为都不停止 daemon，停止受管服务保留为独立危险操作。

验收：正常启动、重复启动和 GUI 崩溃不会误杀、替换或重启现有 daemon；GUI 关闭造成的受保护工作项主动终止数为 0。服务缺失时可完成首次启动，用户主动重启存在阻塞项时明确拒绝，版本不一致时只提示手动更新后台服务。此阶段不代表已接入自动更新。

### Phase 3：IM 账号与 Onboarding

当前状态：账号列表、基础管理，以及 Telegram、飞书、微信和企业微信的新增 Onboarding 已接入 SwiftUI。

交付物：

- [x] 迁移账号列表、平台筛选、启停和删除。
- [x] 迁移 Telegram Token 配置。
- [x] 迁移飞书扫码或凭据引导。
- [x] 迁移微信扫码、验证码和过期处理。
- [x] 迁移企业微信引导。
- [x] 通过 daemon 的受保护窄写接口提交账号启停、删除和新增凭据；界面不回显已保存密钥，首版不迁移到 Keychain。
- [x] 对连接中、轮询中、已连接、未配置和错误状态使用统一语义。

本阶段实现范围：账号读取、启停、删除、Telegram/飞书凭据和三种扫码流程全部位于要求 bearer 鉴权的 `/api/v1/manage/im/*` 命名空间；旧 daemon 缺少接口时，界面明确显示“后台服务需要更新”。删除不存在账号不会触发旧配置迁移副作用，未知平台返回 400；legacy 单例账号在迁移前后保持同一账号 ID，删除后不会因单例残留而复活。

共享 Onboarding sheet 采用“选择平台 → 提供凭据或扫码 → 等待验证 → 完成”骨架：Telegram Token 由 daemon 通过 getMe 校验；飞书支持设备码扫码和 App ID/App Secret 手动验证，版本化轮询响应不回显包含 App Secret 的原始注册载荷；微信扫码可插入验证码步骤，并在二维码过期或验证码受限时原地重试；企业微信使用扫码轮询。所有密钥均只写不回显，验证失败留在当前步骤，二维码过期原地刷新，账号启停失败回滚界面开关状态。

验收：四个平台的新增、启停、删除、失败重试和重启恢复均通过，旧配置无需重新录入。

### Phase 4：Codex 与会话管理

交付物：

- [x] 迁移 Codex App 接入、修复和卸载操作。
- [x] 迁移增强启动及其预检、等待、取消和失败恢复。
- [x] 迁移可见模型配置。
- [x] 迁移会话历史、搜索、状态和 Provider 移动操作。
- [x] 为需要关闭 Codex App 的动作提供明确说明和可取消流程。

当前状态：SwiftUI 已通过受保护的 `/api/v1/manage/codex/*` 和 `/api/v1/manage/sessions*` 路由读取脱敏状态并执行接入、修复、卸载、模型刷新、增强启动和会话 Provider 移动。增强启动已由 daemon 提供可查询的 operation 状态机，覆盖准备、启动、等待 App、注入、完成、失败和取消阶段；支持 requestId 幂等、并发冲突、服务端取消、超时、失败恢复和旧版 daemon 的同步接口回退。SwiftUI 会在页面重建后恢复并继续轮询运行中的 operation，并对旧版取消能力给出明确说明。

验收：不会破坏 `~/.codex/config.toml` 中用户无关配置；每个写操作都有前后状态验证和失败回滚提示。

### Phase 5：AI Gateway 与请求日志

交付物：

- [x] 迁移 Provider 列表、启停、添加、编辑、删除、协议和权重。
- [x] 迁移图像生成过滤、请求日志和详情日志开关。
- [x] 迁移请求日志分页列表、筛选、搜索、排序和清理。
- [x] 迁移请求详情的 Codex 请求、上游请求、SSE、响应和错误视图。
- [x] 大文本详情采用按需加载，支持复制、查找、等宽显示和敏感字段遮罩。
- [x] 对清空日志等操作增加不可逆确认和执行结果反馈。

当前状态：Provider 和网关开关已可真实读写，API Key 只写不回显；请求日志使用稳定游标在服务端完成分页、组合筛选、字面量搜索和正反排序，SwiftUI 支持旧 daemon 单页降级、筛选防抖、翻页去重和竞态隔离；旧 daemon 不返回分页元数据时，客户端会在旧版最多返回的 200 条记录内本地完成筛选和排序。详情继续同页按需脱敏加载，并支持复制、查找、惰性逐行渲染、不可逆清空确认和删除数量反馈。

验收：使用接近现有真实上限的日志集和大详情 fixture，分页、筛选、搜索、滚动与按需详情没有可感知主线程卡顿或无界内存增长；具体基准由 Phase 5 在真实设备测量后固化，日志开关与 daemon 状态一致。

### Phase 6：设置、更新与诊断

交付物：

- [x] 使用 SwiftUI `Settings` 迁移语言、外观、本地连接和出站代理设置。
- [ ] 迁移诊断导出、打开日志和版本信息。
- [x] 先交付手动检查更新：发现新版本、展示发布说明并打开签名下载页，不在 App 内下载或替换程序。
- [ ] 若 Phase 6 决定接入 Sparkle 2，再补 appcast、EdDSA、通道隔离、防降级、原位安装、排空和失败回滚；这些要求不适用于手动检查分支。
- [ ] 完成主菜单、快捷键、About、Help 和隐私说明。
- [ ] 完成 VoiceOver 标签、键盘导航、增大对比度与减少动态效果检查。
- [ ] 完成 Liquid Glass 全页面审查，验证降低透明度、增强对比度和减弱动态效果三种系统设置，并清理碎片化卡片、重复材质和非必要自定义 glass。

当前状态：Settings 已接入服务消息语言、即时外观、本地连接模式和脱敏出站代理设置，并提供运行实例、日志目录、版本、诊断摘要复制和 GitHub 最新 Release 检查。完整诊断 ZIP 导出、帮助/隐私、菜单与无障碍发布审查仍待完成。

验收：手动分支能准确发现版本并打开正确的签名下载页，旧 App 和 daemon 不受影响。若采用 Sparkle，再验证上一正式版原位升级，以及下载损坏、签名不符、网络中断、新 GUI/helper 启动失败时保留旧 App、兼容 daemon 和原配置；计划升级造成的受保护工作项主动终止数为 0。

### Phase 7：正式切换与清理

RC 准备：

- [x] macOS release workflow 构建 SwiftUI App 和 Rust daemon 双产物 Bundle。
- [ ] 版本号由单一来源驱动 Cargo、Xcode 和更新清单，CI 校验三者一致。
- [ ] 生成 Universal 包，并验证 Apple Silicon 与 Intel 两种架构。
- [ ] 运行完整迁移、升级、回滚和长时间稳定性测试。
- [x] 更新 README、架构、故障排查和平台支持等级。
- [ ] 验证 SwiftUI App 更新不重启兼容 daemon；helper 更新由用户在明确提示后手动重启后台服务生效。
- [ ] 确认 macOS legacy 管理端点已删除；保留给 Windows/Linux 客户端的端点全部鉴权，唯一匿名管理面端点仍是 `/healthz`。
- [ ] 完成第 6.2 节 Liquid Glass 发布评审，归档核心页面和无障碍模式的基准截图与交互测试结果。

切换：SwiftUI 已作为 macOS stable，Windows 使用 React/Tauri；Rust daemon/CLI 独立构建。Linux 桌面发布暂停，未来客户端另行设计。

## 10. 测试与发布策略

### 10.1 自动化测试

- Rust：继续运行 `cargo fmt`、`cargo check` 和完整测试集。
- API：为 Swift 使用的每个端点增加 Rust 契约测试和稳定 JSON fixture。
- Swift：对 API 解码、ViewModel 状态机、配置校验和 daemon 管理增加单元测试。
- SwiftUI：对首次启动、离线、账号管理、Provider 管理、请求日志和设置增加 UI 测试。
- Bundle：校验嵌入 daemon 的架构、执行权限、签名、Entitlements、版本和哈希。
- 发布：从已安装旧版本执行真实升级，不只验证全新安装。
- 安全：为未认证访问、伪造 Origin、错误 Host、令牌轮换、PID 重用、helper 身份冲突和 canary secret 泄漏增加负向测试。
- 构建与运行：SwiftPM 和正式 Xcode App 均在 macOS 26 验证菜单栏、Settings、日志详情和 `Inspector`。
- 视觉：在 macOS 26 覆盖浅色、深色、降低透明度、增强对比度和减弱动态效果；截图用于发现层级和布局回归。

### 10.2 发布前手工场景

- 旧数据启动后账号、凭据、绑定、Provider 和日志完整；GUI 重启不要求重新连接。
- 既有 daemon、端口冲突、GUI/daemon 崩溃以及休眠/网络切换均按第 5 节规则工作且不误杀或自动替换进程；GUI 关闭造成的受保护工作项主动终止数为 0。
- Codex、四种消息通道、Provider、会话移动、请求日志和设置完成各自主路径与失败恢复。
- 浅色、深色、VoiceOver、键盘和长文本可用；macOS 26 使用原生 Liquid Glass，降低透明度/增强对比度/减弱动态效果可用。
- 升级、校验失败、启动失败和回滚符合 Phase 6 选择的更新方案。

### 10.3 发布通道

迁移期间只维护两个通道：

- `stable`：当前 SwiftUI macOS 与 React/Tauri Windows 正式版。
- `swiftui-preview`：独立 Bundle ID 和更新 feed，兼作内部测试与日常使用验证。

SwiftUI 未通过退出条件前，不覆盖 `stable` 更新清单。

## 11. 旧 GUI 退役结果

- Rust/wxDragon GUI 源码和 vendor 已删除，不再提供 `gui` Cargo feature 或 `mochiport gui` 命令。
- macOS 发布只包含 SwiftUI App 与 Rust daemon；Windows 发布只包含 React/Tauri App 与 Rust daemon。
- Linux AppImage 发布 workflow 和桌面入口已删除；Linux daemon/CLI 可继续按需构建，未来客户端不复用 wxDragon。

## 12. 实施规则

- 每个 Phase 拆成 API/后台、SwiftUI、测试/打包等小提交；Phase 结束点必须可独立构建、验收和回滚。
- SwiftUI 页面完成前先补齐其所需 API 和契约测试。
- 不在同一提交中同时重构 Rust 业务逻辑和重写对应 SwiftUI 页面。
- SwiftUI 预览版不得写入另一套配置目录；始终复用现有用户数据策略。
- 所有 daemon 关闭动作必须可审计，并记录目标 PID、instance ID、可执行路径和结果。
- 每次完成并验证 macOS 改动后，构建正式 `.app`，启动该包，并确认运行路径、版本和 `/healthz.service`；桥接期同时验证旧 `/api/status.service`。
- 不删除用户数据，不自动清理旧 CodexHub 目录；目录独立迁移另行立项。
