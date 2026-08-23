# MochiPort

[English](README.en.md)

> 当前版本：`0.5.2`

MochiPort 是一个本地优先的 Codex 会话中继。它把本机的 Codex App、Codex VS Code 插件和 Codex CLI 接入 Telegram、飞书、微信或企业微信，让你可以在消息软件里创建和恢复会话、查看任务进度、处理审批，并在本地客户端继续操作同一个 thread。

MochiPort 也内置 AI Gateway：Codex 只需要连接一个本地模型入口，实际请求可以按模型路由到 OpenAI、DeepSeek、Grok/xAI、Anthropic/Claude、智谱 GLM 或其它兼容服务。

## 当前能力

- **本地 Codex 接入**：通过官方 remote-control 路径连接 Codex App、VS Code 插件和 Codex CLI，不替换 Codex 可执行文件，也不安装包装命令。
- **多消息渠道**：支持 Telegram、飞书、微信和企业微信；每个 IM 会话可以绑定一个 Codex thread。
- **Telegram 任务控制**：运行中普通文字默认调整当前任务方向，也可以使用 `/steer`；使用 `/queue` 将后续消息按 FIFO 排队。
- **审批与进度**：Telegram 使用 inline keyboard，飞书/企业微信使用各自的卡片；命令步骤、子代理活动和回复进度会按 turn 聚合。
- **AI Gateway**：支持 OpenAI Responses、DeepSeek Responses、Grok/xAI Responses、Chat Completions 和 Anthropic Messages 协议，并提供模型别名、可见模型、权重路由、超时和请求日志。
- **Sub2API 账号池**：在 GUI 中连接 Sub2API 管理 API，以只读方式查看账号可用性、余额、倍率和最近命中的账号；账号列表默认展开，不会修改 Sub2API 账号池。
- **桌面管理界面**：概览、Codex 接入、AI 网关、消息渠道、会话和请求日志都可以在 MochiPort 中管理。

<p align="center">
  <img src="docs/assets/product/threadrelay-overview.png" alt="MochiPort 当前连接拓扑和消息渠道概览" width="900">
</p>
<p align="center">
  <img src="docs/assets/product/threadrelay-ai-gateway.png" alt="MochiPort 当前 AI 网关模型服务和可见模型界面" width="900">
</p>

## 快速开始

### 1. 准备环境

- macOS、Windows 或 Linux
- Codex App、Codex VS Code 插件或 Codex CLI
- 至少一个模型服务 API Key
- 如果要从消息软件操作 Codex，再准备一个消息渠道账号

Codex remote-control 仍需要 ChatGPT 兼容的认证模式；仅有 API key 的 Codex auth 无法启动 remote-control。MochiPort 的 GUI/CLI 会负责写入本地连接所需的配置，模型服务的 API key 仍由你自己提供。

### 2. 安装并启动

从 [MochiPort Releases](https://github.com/mps233/mochiport/releases) 下载对应平台的程序：

| 平台 | 文件 | 启动方式 |
| --- | --- | --- |
| macOS | `MochiPort-<版本>-macos-<架构>.dmg` | 拖入 Applications 后打开 |
| Windows | `MochiPort-<版本>-windows-x64.msi` 或 `.zip` | 安装或直接运行 |
| Linux | `MochiPort-<版本>-linux-x86_64.AppImage` | `chmod +x` 后运行 |

打开 GUI 后，MochiPort 会启动本地 backend。macOS 使用当前用户范围的 LaunchAgent 保持 backend 运行；正常退出 GUI 不会自动重新打开窗口。

### 3. 接入消息渠道

进入 **消息渠道** 页面，选择并接入一个或多个渠道：

- **Telegram**：填入 BotFather token。当前只处理私聊，群聊消息和群聊按钮会被忽略；支持文本、图片/文件、会话创建/恢复、审批、草稿流式回复以及任务/子代理进度。
- **飞书**：扫码创建机器人，使用 WebSocket 接收事件和消息卡片。
- **微信**：扫码登录微信 iLink Bot，使用长轮询收发消息。
- **企业微信**：扫码接入 AI Bot，支持私聊/群聊文本、流式回复、图片/文件、会话选择和审批卡片。

可以为同一平台配置多个账号，并在页面中单独启停或删除。Telegram 的 `allowedChatIds`、飞书的 `allowedOpenIds`/`allowedChatIds` 等白名单建议在实际使用时配置。

### 4. 连接 Codex

在 **Codex 接入** 页面打开“连接 MochiPort”，然后正常启动 Codex App 或 Codex VS Code 插件，并开启 remote-control/“控制这台电脑”。MochiPort 会通过本地 remote-control 读取会话列表，不需要复制或迁移会话文件。

如果多个 Codex 客户端同时连接，新的 IM 会话按固定优先级选择执行端：Codex App > Codex VS Code 插件 > Codex CLI；绑定后会继续使用原执行端，直到退出或重新绑定。

### 5. 使用 Codex CLI

保持 MochiPort GUI 运行，在项目目录启动 app-server：

```bash
codex app-server --listen ws://127.0.0.1:3849 --remote-control
```

再在同一目录连接本地 TUI：

```bash
codex --remote ws://127.0.0.1:3849
```

端口 `3849` 可以替换，但两个命令中的地址必须一致。连接完成后，CLI 和消息软件可以继续操作同一个 app-server。

## Telegram 使用方式

Telegram 使用 Bot API long polling。Bot 菜单会自动显示规范命令；旧的 `/s`、`/q` 仍可用，但不会出现在菜单中。

### 任务运行语义

- 没有运行任务时，直接发送普通文字会启动一条新的 turn。
- 有任务运行时，直接发送普通文字会调用 `turn/steer`，把它作为当前任务的新方向追加进去。
- 需要明确表达时使用 `/steer <新的方向>`。
- 使用 `/queue <要稍后执行的内容>` 将消息加入当前会话的 FIFO 队列，每个会话最多 8 条；没有运行任务时，机器人会提示直接发送消息。
- 当前任务完成、失败或被停止后，队列会自动执行下一条。
- `/stop` 只中断当前任务并保留队列；`/exit` 会中断任务、清空队列并解除会话绑定。

### 命令

| 命令 | 作用 |
| --- | --- |
| `/help` | 查看帮助；`/start` 也可作为入口 |
| `/new` | 创建新的 Codex 会话 |
| `/sessions` | 查看并恢复历史会话 |
| `/status` | 查看连接、当前会话、任务状态和排队数量 |
| `/steer <内容>` | 调整正在运行的任务方向 |
| `/queue <内容>` | 将一条消息排到当前任务之后 |
| `/stop` | 中断当前任务并保留会话；兼容别名 `/s` |
| `/exit` | 退出当前会话并清空队列；兼容别名 `/q` |

会话选择和审批优先使用消息中的按钮。按钮不可用时，按消息提示发送编号或审批回复（例如 `/1`、`/y`、`/n`）。

## AI Gateway 与 Sub2API

### AI Gateway

进入 **AI 网关** 页面添加模型服务。每个 provider 可以配置协议、Base URL、API Key、模型列表、模型别名、权重和超时；Codex 侧只看到 MochiPort 暴露的模型目录。

当前支持：

- OpenAI Responses
- DeepSeek Responses
- Grok/xAI Responses
- Chat Completions
- Anthropic Messages（可用于 Claude 和兼容 GLM 的配置）

可选能力包括 Codex 可见模型白名单、同模型多 provider 的优先级/稳定路由、prompt cache 设置、请求日志摘要与详情，以及移除不被上游支持的 `image_generation` 工具。具体 web search、工具调用和思考输出能力取决于 provider 协议与上游服务。

### Sub2API 账号池

在 **AI 网关 -> 账号** 中填写 Sub2API 管理地址和 Admin API Key。MochiPort 只读取管理 API：

- 展示账号在线/可用状态、上游余额和倍率；
- 展示最近一次实际命中的账号；
- 支持手动刷新，并在短时间内复用上次结果；
- 不使用 Admin API Key 代替模型 provider key；
- 不创建、删除、编辑或切换 Sub2API 账号。

管理密钥只保存在本机，界面不会回显。未配置 Sub2API 时，AI Gateway 的模型服务仍可独立使用。

## 网络、配置与安全

MochiPort 默认只监听本机：

```text
http://127.0.0.1:3847
```

不要把 daemon 端口直接暴露到公网。GUI 的“网络”设置只影响 MochiPort 发往模型服务、Telegram、飞书、微信和更新地址的出站请求，可选系统代理、直连或自定义 HTTP/SOCKS5 代理；GUI、Codex 和 daemon 的回环通信不使用这个出站代理。

手写配置时，建议从 [`config.example.toml`](config.example.toml) 开始。MochiPort 配置和 Codex App 配置是两套文件，不要混用；字段说明见 [`docs/configuration.md`](docs/configuration.md)。

以下内容都是 secret，不要提交到 Git：

- IM Bot token、企业微信 secret 和微信 token
- 模型 provider API key
- Sub2API Admin API Key
- Codex 本地认证数据

飞书和 Telegram 附件会写入状态目录旁边的 `.im/attachments/`。bridge 可以代 IM 用户向 Codex 提交审批决定，因此消息渠道的访问权限应按本机 Codex 审批权限管理。

## 恢复与项目边界

GUI 中的“恢复原来的设置”会恢复 MochiPort 写入前的 Codex 连接方式，不会卸载 Codex，也不会删除会话历史。

MochiPort 不会：

- 替换 Codex App、Codex CLI 或 VS Code 插件的原始可执行文件；
- 安装 `codex` 包装命令或 shim；
- 代替 Codex 管理模型、沙箱、审批策略、工作目录或环境变量；
- 把本地 daemon 自动切换到其它 runtime，或把它暴露成系统级服务。

## 诊断与开发

daemon 运行时可以检查：

```text
GET http://127.0.0.1:3847/api/status
GET http://127.0.0.1:3847/api/remote-control/status
GET http://127.0.0.1:3847/api/remote-control/backend-status
GET http://127.0.0.1:3847/api/events
```

常用开发命令：

```bash
cargo fmt
cargo test
cargo build --release --features gui --bin mochiport
```

故障排查可以从 [`docs/troubleshooting.md`](docs/troubleshooting.md) 开始；架构说明见 [`docs/architecture.md`](docs/architecture.md)。

## 更多文档

- [配置说明](docs/configuration.md)
- [Telegram 集成与维护边界](docs/telegram-integration.zh-CN.md)
- [微信集成与已知边界](docs/wechat-integration.zh-CN.md)
- [认证说明](docs/auth-notes.zh-CN.md)
- [MochiPort 构建和交接规则](docs/threadrelay-change-handoff.zh-CN.md)
- [发布检查清单](docs/release-checklist.md)

## 兼容说明

MochiPort 会优先使用新的 `MochiPort` 配置目录、`MOCHIPORT_HOME` 和 `mochiport-*` 状态文件，同时兼容读取旧 `ThreadRelay`、`CodexHub` 配置目录，`THREADRELAY_*`、`CODEXHUB_*` 环境变量及少量旧标识。这些旧值仅用于迁移已有凭据、会话绑定和 provider 配置，不代表当前产品名称，也不应手动重命名。

旧 ThreadRelay `0.5.0`/`0.5.1` 客户端仍指向原更新地址，且项目不提供旧仓库兼容更新通道。现有用户需要从 [MochiPort Releases](https://github.com/mps233/mochiport/releases) 手动安装一次，之后更新检查会继续使用 MochiPort 仓库。

## License

Apache-2.0。上游归属和修改声明见 [NOTICE](NOTICE)，第三方资产说明见 [packaging/THIRD_PARTY_LICENSES.txt](packaging/THIRD_PARTY_LICENSES.txt)。
