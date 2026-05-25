# codex-remote

[English](README.md)

`codex-remote` 用来把官方 Codex 的 remote-control 会话接到飞书 IM。你仍然在终端里正常使用 `codex`；飞书可以订阅同一个官方 Codex app-server 会话，向 Codex 发起 turn，并接收助手输出、工具事件、turn 状态和审批请求。

这个项目刻意保持“薄桥接”定位。它不是第二个 Codex client，不创建自己的 workspace，也不修改 Codex 的 model、sandbox、approval policy、cwd 或环境变量。这些都仍然由官方 Codex app-server 持有。

## 它做什么

- 提供本地 remote-control backend，地址默认是 `http://127.0.0.1:3847/backend-api`
- 让官方 Codex app-server 通过 `/backend-api/wham/remote/control/server` 连接进来
- 把已绑定到飞书会话的 Codex thread 项、助手输出、工具卡片、turn 状态和审批请求渲染到飞书
- 把飞书消息通过官方 app-server JSON-RPC 发回指定的 Codex thread
- 把 Codex 审批请求渲染成飞书交互卡片，并回填原始 request id
- 提供本地 Web 页面，用于飞书接入、shim 安装/卸载、状态查看和诊断

## 架构

```text
用户终端
  |
  | 运行: codex
  v
codex shim
  |
  | 启动官方 Codex app-server:
  |   real-codex -c chatgpt_base_url="http://127.0.0.1:3847/backend-api" app-server --listen ws://127.0.0.1:<port> --remote-control
  | 启动官方 Codex TUI:
  |   real-codex --remote ws://127.0.0.1:<port> -C <用户当前目录>
  v
官方 Codex app-server
  |
  | outbound remote-control websocket
  v
codex-remote remote-control backend
  |
  | 飞书 websocket 事件
  | 飞书消息 / 卡片 API
  v
飞书 IM
```

关键边界：

- Codex app-server 是 thread、turn、approval、tool、配置的唯一事实来源
- `codex-remote` 只实现官方 remote-control backend 和飞书适配层
- 飞书消息通常会被翻译成官方 app-server 的 `turn/start`
- 飞书不会自动看到所有 thread，只会看到它主动绑定过的 thread
- 本地 TUI 的 `userMessage` 可以同步到已绑定的飞书会话
- 飞书自己发起的 turn，不会再把同一个 `userMessage` 回显回飞书
- shim 只是启动便利层；如果它关闭或配置有误，会直接透传到真实的 Codex 可执行文件

更多细节见 [docs/architecture.md](docs/architecture.md)。

## 快速开始

前提：

- 已安装 Rust stable toolchain
- 已安装并能正常使用官方 Codex CLI
- 有一个飞书 / Lark 机器人应用，或者使用内置飞书扫码接入流程

启动 daemon：

```powershell
cargo run -- --config config.toml daemon
```

打开本地 Web 控制台：

```text
http://127.0.0.1:3847
```

在 Web 控制台里：

1. 扫码接入飞书，或者把现有飞书凭证填进 `config.toml`
2. 安装 Codex shim
3. 确认页面显示飞书 websocket 已连接

然后打开一个新的终端，在任意项目目录里正常运行：

```powershell
cd D:\path\to\your\project
codex
```

当 shim 生效后，Codex 会通过官方 remote-control 接入飞书。TUI 仍然只和官方 app-server 对话，而 app-server 再和 `codex-remote` 对话。

如果某个飞书会话当前还没有绑定任何 thread，第一条飞书消息不会盲猜，也不会偷偷创建一套平行客户端状态。bridge 会先发一张 thread 选择卡片，让飞书订阅一个现有工作 thread 或历史 thread。

## 手工协议调试

如果不使用 shim，而是要直接调协议，可以手工启动：

```powershell
codex-remote --config D:\path\to\config.toml daemon
codex -c 'chatgpt_base_url="http://127.0.0.1:3847/backend-api"' app-server --listen ws://127.0.0.1:3849 --remote-control
codex --remote ws://127.0.0.1:3849 -C D:\path\to\your\project
```

这种模式只适合协议调试。正常用户路径应该是：启动 daemon，安装 shim，然后继续直接用 `codex`。

## Thread 绑定模型

`codex-remote` 不是第二个完整的 Codex 客户端。它只负责把飞书桥接到飞书自己选中的 Codex thread。

- Codex app-server 仍然维护 thread 生命周期和历史
- 一个飞书会话同一时间只绑定一个 Codex thread
- 如果飞书还没绑定 thread，bridge 会先发 thread 列表卡片，而不是猜
- 从飞书恢复某个 thread 后，会订阅这个 thread 后续的 remote-control 事件
- TUI 发出的 `userMessage` 会在已绑定 thread 上同步到飞书
- 飞书发起的 turn 会按 `turn id` 记录来源，后续对应的 `userMessage completed` 事件会被抑制，避免回显

这就是为什么飞书能接入一个已在工作的 thread，但 `codex-remote` 依然不是 workspace owner。

## 命令

```text
codex-remote [--config PATH] daemon
codex-remote [--config PATH] status
codex-remote [--config PATH] on
codex-remote [--config PATH] off
codex-remote [--config PATH] install-shim [--real-codex PATH] [--bin-dir PATH]
codex-remote [--config PATH] uninstall-shim
codex-remote [--config PATH] shim -- [codex args...]
```

`off` 会保留 shim，但让它直接透传到真实 Codex。你也可以只在当前终端临时绕过 shim：

```powershell
$env:CODEX_REMOTE_DISABLE = "1"
codex
```

## 配置

示例：

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

[shim]
binDir = "..."
realCodexPath = "..."
```

`config.toml` 存放本地凭证，默认不会提交到 git。详见 [config.example.toml](config.example.toml) 和 [docs/configuration.md](docs/configuration.md)。

## 认证边界

`codex-remote` 不负责 Codex 的认证本身。

- 官方 Codex binary 和 app-server 仍然负责账号状态与登录行为
- 支持的路径是：先让官方 Codex 自己能够正常启动、正常连接，再通过 remote-control 把 `codex-remote` 接上去
- `codex-remote` 不替代官方 ChatGPT 登录流程
- `codex-remote` 也不承诺支持手工拼装、伪造或其他非官方来源的 auth 材料

如果 Codex 自己因为账号或登录状态无法启动，应该先在 Codex 侧解决，再接 bridge。bridge 只有在官方 app-server 已经能运行的前提下才有意义。

更多说明见 [docs/auth-notes.zh-CN.md](docs/auth-notes.zh-CN.md)。

## 参考文件

仓库里可能会保留一些协议实验和 auth 结构观察时产生的本地参考材料。

这些材料只是研究参考，不属于 `codex-remote` 的正常运行路径，也不能替代正式的 remote-control 流程，更不应被当成受支持的“登录绕过”方案。

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
GET http://127.0.0.1:3847/api/shim/status
GET http://127.0.0.1:3847/api/events
```

## 安全说明

- daemon 默认只绑定 `127.0.0.1`，不要直接暴露到公网
- `config.toml` 里保存飞书 `appId` 和 `appSecret`，不要提交
- 飞书附件会下载到本地状态目录旁边的 `.im/attachments/feishu/`
- 真正使用时建议配置 `allowedOpenIds` 和 / 或 `allowedChatIds`
- bridge 可以替飞书用户向 Codex 提交审批决定，所以飞书访问权限应视为等价于本地终端审批权限

## 排障

见 [docs/troubleshooting.md](docs/troubleshooting.md)。
