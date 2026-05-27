# codex-remote

[English](README.md)

`codex-remote` 是一个本地 Codex App remote-control backend，用来把 Codex App 的远程控制能力接到飞书 / Lark。

它只做一件事：用户明确启动本地服务后，让 Codex App 连接本机 backend，再把远程控制消息桥接到飞书。

## 快速使用

### 1. 下载或构建

macOS 发布包由 GitHub Actions 生成。下载 `Codex Remote.dmg` 后双击打开 App。

开发环境也可以直接运行：

```powershell
cargo run --features gui --bin codex-remote-gui
```

GUI 依赖 wxDragon，构建 GUI 需要本机有 CMake。daemon、Web 控制台和测试不依赖 GUI。

### 2. 启动本地服务

打开 `Codex Remote.app` 后，点击“启动本地服务”。

本地服务默认监听：

```text
http://127.0.0.1:3847
```

也可以用命令启动 daemon：

```powershell
cargo run -- --config config.toml daemon
```

### 3. 接入飞书

在 GUI 里点击“更换机器人”，按二维码流程接入飞书。

如果已经有飞书机器人凭证，也可以写入 `config.toml`：

```toml
[feishu]
appId = ""
appSecret = ""
mentionOnly = true
allowedOpenIds = []
allowedChatIds = []
```

### 4. 填写模型 provider

在 GUI 的 Codex App 页面填写：

- Provider 名称
- 第三方 Base URL
- API Key

如果 Provider 名称留空，但填写了 Base URL 或 API Key，默认 provider 名称会使用 `codex`。

第三方 key 属于 Codex 的 model provider 配置。`chatgpt_base_url` 只负责 Codex App backend 和 remote-control 流量。

### 5. 写入 Codex App 配置

点击“写入配置”。

它会显式写入 Codex App 的本地配置：

- `chatgpt_base_url = "http://127.0.0.1:3847/backend-api"`
- 本地 `ChatgptAuthTokens`
- 可选 model provider 配置

已有文件会先备份为 `.bak`。daemon 启动本身不会修改 Codex App 配置，只有用户点击按钮或运行配置命令才会写入。

### 6. 打开 Codex App 远程控制

双击启动 Codex App，然后在 Codex App 里启用 remote control。

如果连接成功，GUI 里会看到 Codex App 连接状态变为已连接。也可以检查：

```text
GET http://127.0.0.1:3847/api/remote-control/status
```

期望状态：

```json
{
  "connected": true,
  "initialized": true
}
```

### 7. 在飞书里选择 thread

如果飞书会话还没有绑定 thread，第一条消息不会偷偷创建隐藏状态。bridge 会先发 thread 选择卡片，让用户选择新建 thread 或恢复已有 thread。

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

## 卸载注入

GUI 里点击“卸载注入”即可移除本项目写入 Codex App 的：

- `chatgpt_base_url`
- `model_provider`
- 本地 `ChatgptAuthTokens` auth 文件

命令行等价操作：

```text
codex-remote [--config PATH] uninstall-codex-app [--codex-home PATH]
```

## 项目边界

`codex-remote` 只支持干净的 Codex App remote-control 路径。

它不会：

- 安装 `codex` 包装命令
- 替换 Codex CLI
- 通过 shim 启动 Codex App
- 安装登录项或开机启动项
- 自动常驻后台
- 修改 Codex 的 model、sandbox、approval policy、cwd 或环境变量

本地 backend 只在用户明确点击“启动本地服务”或运行 `daemon` 命令后启动。

## 技术说明

主链路：

```text
Codex App
  |
  | chatgpt_base_url = "http://127.0.0.1:3847/backend-api"
  | 用户在 App 里打开 remote control
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

Codex remote-control 要求 ChatGPT 兼容的 auth mode。这个项目采用本地 `ChatgptAuthTokens` 形态，用来通过 Codex App 的 remote-control 账号检查。API-key-only auth 不能启动 remote-control。

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

`configure-codex-app` 是 GUI“写入配置”的 CLI 等价形式。如果写入模型 provider 配置，默认 provider 是 `codex`，默认模型是 `gpt-5.5`。

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

Codex App 配置是另一份文件，通常在 `~/.codex/config.toml`。

详见 [config.example.toml](config.example.toml) 和 [docs/configuration.md](docs/configuration.md)。

## 开发

```powershell
cargo fmt
cargo test
cargo build
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
- Codex App 的 `auth.json` 和第三方 provider key 都是本地 secret，不要提交
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
