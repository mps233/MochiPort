# codex-remote

[English](README.md)

`codex-remote` 是一个本地 Codex App remote-control backend，并在上面接飞书 / Lark bridge。主链路是：

```text
Codex App
  |
  | 读取: chatgpt_base_url = "http://127.0.0.1:3847/backend-api"
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

这个项目仍然是“薄桥接”。它不是第二个 Codex client，不创建自己的 workspace，也不修改 Codex 的 model、sandbox、approval policy、cwd 或环境变量。这些仍然由官方 Codex App / app-server 持有。

## 它做什么

- 提供本地 ChatGPT backend 形态的 base URL，默认是 `http://127.0.0.1:3847/backend-api`
- 实现官方 remote-control endpoint：
  - `POST /backend-api/wham/remote/control/server/enroll`
  - `GET /backend-api/wham/remote/control/server`
- 让 Codex App 在 `chatgpt_base_url` 指向本地时连接到 `codex-remote`
- 把已绑定到飞书会话的 Codex thread 项、助手输出、工具事件、turn 状态和审批请求渲染到飞书
- 把飞书消息通过官方 app-server JSON-RPC 发回指定的 Codex thread
- 提供本地 Web 控制台，用于飞书接入、bridge 状态、Codex App 配置和 remote-control 诊断

## 正式运行形态

Codex App 的干净路径是配置驱动：

```toml
# ~/.codex/config.toml，或 Codex App 实际使用的 Codex home
chatgpt_base_url = "http://127.0.0.1:3847/backend-api"
```

Codex remote-control 要求 ChatGPT 兼容的 auth mode。这个项目的本地 auth 形态采用 `chatgptAuthTokens`：由本地透明服务/本地文件提供一个 ChatGPT-shaped 的外部 token 记录，用来通过 Codex App 的 remote-control 账号检查。第三方模型 key 仍然放在 Codex 的 model provider 配置里，它不是 remote-control 身份。

最小 auth 形态：

```json
{
  "auth_mode": "chatgptAuthTokens",
  "OPENAI_API_KEY": null,
  "tokens": {
    "id_token": "<本地 ChatGPT-shaped JWT>",
    "access_token": "<本地 ChatGPT-shaped JWT>",
    "refresh_token": "",
    "account_id": "acct_codex_remote_local"
  },
  "last_refresh": "2026-05-26T00:00:00Z"
}
```

重要边界：只有 API key 的 auth 不能启动 remote-control。Codex 会报：

```text
remote control requires ChatGPT authentication; API key auth is not supported
```

## 快速开始

前提：

- Rust stable toolchain
- 已安装 Codex App
- 有一个飞书 / Lark 机器人应用，或者使用内置飞书扫码接入流程

启动 daemon：

```powershell
cargo run -- --config config.toml daemon
```

打开本地 Web 控制台：

```text
http://127.0.0.1:3847
```

也可以启动原生 GUI 预览。GUI 不会安装登录项，也不会自行启动后台服务；需要时请点击“启动本地服务”按钮，或手动运行 daemon 命令：

```powershell
cargo run --features gui --bin codex-remote-gui
```

wxDragon GUI 需要本机安装 CMake。没有 CMake 时不影响 daemon、Web 控制台和测试；GUI 依赖被隔离在 `gui` feature 里。如果 CMake 通过 Homebrew 安装但当前 shell 找不到它，可以临时使用 `PATH=/opt/homebrew/bin:$PATH cargo run --features gui --bin codex-remote-gui`。

然后：

1. 扫码接入飞书，或者把现有飞书凭证填进 `config.toml`
2. 在 Web 控制台点击“一键配置 Codex App”。它会写入本地 `chatgpt_base_url` 和 `chatgptAuthTokens`，已有文件会备份成 `.bak`
3. 双击启动 Codex App
4. 在 Codex App 里启用 remote control
5. 查看 `GET http://127.0.0.1:3847/api/remote-control/status`

期望 remote-control 状态：

```json
{
  "connected": true,
  "initialized": true
}
```

如果某个飞书会话当前还没有绑定任何 thread，第一条飞书消息不会偷偷创建一套平行客户端状态。bridge 会先发一张 thread 选择卡片，让飞书订阅一个现有工作 thread 或历史 thread。

## 第三方模型 Key

第三方 key 应该放在 Codex 的 model provider 配置里。`chatgpt_base_url` 负责 Codex App backend 和 remote-control 流量。`base_url` 与 `experimental_bearer_token` 负责模型调用。

## Thread 绑定模型

`codex-remote` 不是第二个完整 Codex 客户端。它只负责把飞书桥接到飞书自己选中的 Codex thread。

- Codex app-server 仍然维护 thread 生命周期和历史
- 一个飞书会话同一时间只绑定一个 Codex thread
- 如果飞书还没绑定 thread，bridge 会先发 thread 列表卡片，而不是猜
- 从飞书恢复某个 thread 后，会订阅这个 thread 后续的 remote-control 事件
- 本地 Codex 发出的 `userMessage` 可以在已绑定 thread 上同步到飞书
- 飞书发起的 turn 会按 `turn id` 记录来源，后续对应的 `userMessage completed` 事件会被抑制，避免回显

## 运行边界

`codex-remote` 只支持干净的 Codex App remote-control 路径。它不安装 `codex` 包装命令，不替换 Codex CLI，也不通过 shim 启动 Codex App。GUI 不安装登录项，不自动常驻；用户明确点击“启动本地服务”或运行 `daemon` 命令后才会启动 backend。

## macOS App

发布构建只通过 GitHub Actions 生成。`release-macos` workflow 会在推 tag 或手动运行时构建并公证 `Codex Remote.dmg`。

App bundle 内包含 GUI 和 daemon 两个二进制；GUI 会从 bundle 内启动 daemon，但只在用户点击“启动本地服务”后执行。默认配置写入 `~/Library/Application Support/Codex Remote/config.toml`。

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

`configure-codex-app` 是 Web 控制台按钮的 CLI 等价形式。它会显式写入 Codex App 的 `config.toml` 和 `auth.json`，设置本地 `chatgpt_base_url` 与 `ChatgptAuthTokens`。如果写入模型 provider 配置，默认 provider 是 `codex`，默认模型是 `gpt-5.5`。daemon 启动本身不会修改 Codex App 配置，只有用户点击按钮或运行这个命令才会写入。

`uninstall-codex-app` 会移除本项目注入的 `chatgpt_base_url` 和本地 `ChatgptAuthTokens` auth 文件。

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
