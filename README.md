# codex-remote

[English](README.en.md)

## 产品预览

本地 GUI 负责启动 backend、接入飞书、写入 Codex App 和 Codex VS Code 插件所需的本地配置。

<p align="center">
  <img src="docs/assets/product/codex-remote-gui.png" alt="Codex Remote GUI 状态和配置界面" width="900">
</p>

同一个 Codex thread 可以继续在 Codex App 里查看，飞书发起的会话和工具结果会同步回来。

<p align="center">
  <img src="docs/assets/product/codex-app-chat.png" alt="Codex App 会话同步和图片结果" width="900">
</p>

在飞书移动端也可以直接收到 Codex 的图片、命令和审批结果。

<p align="center">
  <img src="docs/assets/product/feishu-mobile-image.jpg" alt="飞书移动端展示 Codex 图片结果" width="520">
</p>

`codex-remote` 是一个本地 Codex remote-control backend，用来把 Codex App 和 Codex VS Code 插件的远程控制能力接到飞书 / Lark。

它只做一件事：用户明确打开本地 GUI 后，让 Codex 客户端连接本机 backend，再把 remote-control 消息桥接到飞书。

当前支持 macOS / Windows 上的 Codex App，以及 Codex VS Code 插件。本项目只写入本地配置和必要的 GUI 环境入口，不替换 Codex App、Codex CLI 或 VS Code 插件的原始可执行文件。

## 快速使用

### 0. 前置条件

- macOS 或 Windows 设备
- Codex App 或 Codex VS Code 插件
- 不需要 ChatGPT 账号，也不需要“加速网络”
- 一个可以访问 GPT-5.x 兼容模型的第三方 key
- 已安装飞书

### 1. 安装

从 GitHub Releases 下载 `Codex Remote.dmg`，拖到 Applications 后打开。

第一次打开时，如果 macOS 提示来自互联网，按系统提示确认即可。Windows 直接运行 release 包里的 `codex-remote.exe`。这个 App 不会安装开机启动项，也不会自动常驻后台。

### 2. 启动本地服务

打开 `Codex Remote`。GUI 会自动启动本地 backend，并在退出时关闭本次启动的 backend。

看到“本地服务：运行中”后继续下一步。

### 3. 接入飞书

第一次使用时，点击“更换机器人”，按二维码流程完成飞书接入。

接入成功后，飞书状态会显示为已连接。之后正常使用不需要反复扫码，只有更换机器人时才需要重新扫码。

### 4. 填写模型信息

切到 “Codex 接入” 页面，填写你的模型服务信息：

- Provider 名称
- 第三方 Base URL
- API Key

Provider 名称可以留空。留空时，如果填写了 Base URL 或 API Key，默认会使用 `ai-codex`。

### 5. 写入 Codex 配置

点击“写入配置”。

这个按钮只改本地 Codex 配置，并会先备份旧文件。它会让 Codex App 和 Codex VS Code 插件的 remote-control 连接到本机 `codex-remote`，同时写入本地认证信息和可选的模型 provider 配置。

在 macOS 上会通过 `launchctl` 写入本次用户会话的 `CODEX_API_BASE_URL` 和 `CODEX_APP_SERVER_LOGIN_ISSUER`。在 Windows 上会写入当前用户的同名环境变量，并在 GUI 退出或卸载配置时按当前值清理。

### 6. 打开 Codex

正常启动 Codex App 或 Codex VS Code 插件，并打开 remote-control / 控制这台电脑。

连接成功后，`Codex Remote` 里会看到 Codex 控制通道变为已连接。

不需要在 Codex App 的“连接”设置页里看到远程连接设备列表。这个项目走的是本地 backend + 飞书 bridge，只要 `Codex Remote` 首页的“本地服务”“飞书”“Codex 控制通道”三个状态都是绿色，就可以直接在飞书里使用。

### 7. 在飞书里开始使用

在飞书里给机器人发消息。

如果当前飞书会话还没有绑定 Codex thread，机器人会先发一张选择卡片，让你新建 thread 或恢复已有 thread。选择后，后续对话就会进入对应的 Codex thread。

## 交流与支持

推荐关注公众号，后续会更新技术干货、实践记录和项目进展。

<img src="docs/assets/wechat-public-account.jpg" alt="微信公众号" width="220">

微信群主要用于反馈问题、交流使用体验和提出功能建议。

<img src="docs/assets/wechat-group.jpg" alt="AI-Agent 技术交流群" width="260">

## 飞书命令

```text
/new       把当前飞书会话重新绑定到新的 Codex thread
/status    查看当前绑定和运行状态
/s /stop   中断当前正在运行的 Codex turn
/q         中断并清除当前绑定
/y /n      通过或拒绝当前审批
/1 /2 /3   选择审批卡片里的具体选项
```

审批卡片在选择后会高亮并标记为已处理，避免聊天里堆了很多卡片后分不清哪些已经操作过。

## 卸载配置

GUI 里点击“卸载配置”即可移除本项目写入 Codex 的：

- `chatgpt_base_url`
- `model_provider`
- 本地 `ChatgptAuthTokens` auth 文件
- `CODEX_API_BASE_URL` / `CODEX_APP_SERVER_LOGIN_ISSUER` GUI 环境入口

## 项目边界

`codex-remote` 只支持干净的 Codex remote-control 路径。

它不会：

- 安装 `codex` 包装命令
- 替换 Codex CLI
- 通过 shim 启动 Codex App
- 安装登录项或开机启动项
- 自动常驻后台
- 替换 Codex App、Codex CLI 或 VS Code 插件的原始可执行文件

本地 backend 只会在用户明确打开 GUI 或主动从开发工具启动时运行。

## 技术说明

主链路：

```text
Codex App / Codex VS Code 插件
  |
  | chatgpt_base_url = "http://127.0.0.1:3847/backend-api"
  | 用户打开 remote-control
  v
官方 Codex app-server
  |
  | outbound remote-control websocket
  v
codex-remote 本地 backend
  |
  | 飞书 websocket 事件
  | 飞书消息 / 卡片 API
  v
飞书 IM
```

本项目实现官方 remote-control endpoint：

```text
POST /backend-api/wham/remote/control/server/enroll
GET  /backend-api/wham/remote/control/server
```

Codex remote-control 要求 ChatGPT 兼容的 auth mode。这个项目采用本地 `ChatgptAuthTokens` 形态，用来通过 Codex 客户端的 remote-control 账号检查。API-key-only auth 不能启动 remote-control。

Thread 绑定模型：

- Codex app-server 仍然维护 thread 生命周期和历史
- 一个飞书会话同一时间只绑定一个 Codex thread
- 如果飞书还没绑定 thread，bridge 会先发 thread 列表卡片
- 从飞书恢复某个 thread 后，会订阅这个 thread 后续的 remote-control 事件
- 飞书发起的 turn 会按 turn id 记录来源，避免 userMessage 回显

## 命令

```text
codex-remote [--config PATH] daemon
codex-remote [--config PATH] status
codex-remote [--config PATH] on
codex-remote [--config PATH] off
codex-remote [--config PATH] configure-codex-app [--codex-home PATH] [--provider-name NAME] [--provider-base-url URL] [--provider-key TOKEN] [--model MODEL]
codex-remote [--config PATH] uninstall-codex-app [--codex-home PATH]
```

`on` / `off` 用来启用或暂停飞书 bridge。

`configure-codex-app` 是 GUI“写入配置”的 CLI 等价形式。如果写入模型 provider 配置，默认 provider 是 `ai-codex`，默认模型是 `gpt-5.5`。

## 配置

`config.toml` 是 `codex-remote` 自己的配置：

```toml
bind = "127.0.0.1:3847"
statePath = "codex-remote-state.json"

[feishu]
appId = ""
appSecret = ""
mentionOnly = true
allowedOpenIds = []
allowedChatIds = []

[bridge]
enabled = true
accountId = "default"
sendStreaming = true
```

Codex 客户端配置是另一份文件，通常在 `~/.codex/config.toml`。

详见 [config.example.toml](config.example.toml) 和 [docs/configuration.md](docs/configuration.md)。

## 开发

```powershell
cargo fmt
cargo test
cargo build --release --features gui --bin codex-remote
```

daemon 运行时常用状态接口：

```text
GET http://127.0.0.1:3847/api/status
GET http://127.0.0.1:3847/api/remote-control/status
GET http://127.0.0.1:3847/api/remote-control/backend-status
GET http://127.0.0.1:3847/api/events
```

## 安全说明

- daemon 默认只绑定 `127.0.0.1`，不要直接暴露到公网
- `config.toml` 里保存飞书 `appId` 和 `appSecret`，不要提交
- Codex 的 `auth.json` 和第三方 provider key 都是本地 secret，不要提交
- 飞书附件会下载到本地状态目录旁边的 `.im/attachments/feishu/`
- 真正使用时建议配置 `allowedOpenIds` 和 / 或 `allowedChatIds`
- bridge 可以替飞书用户向 Codex 提交审批决定，所以飞书访问权限应视为等价于本地 Codex 审批权限

## 更多文档

- [架构](docs/architecture.md)
- [配置](docs/configuration.md)
- [认证说明](docs/auth-notes.zh-CN.md)
- [排障](docs/troubleshooting.md)

## License

Apache-2.0
