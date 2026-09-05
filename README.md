<p align="center">
  <img src="packaging/macos/AppIcon.svg" alt="MochiPort logo" width="132">
</p>

<h1 align="center">MochiPort</h1>

<p align="center">
  本地优先的 Codex 会话中继与 AI Gateway
</p>

<p align="center">
  <img src="docs/assets/product/mochiport-hero.png" alt="MochiPort macOS 主界面，展示用量趋势、连接拓扑和消息渠道状态" width="1000">
</p>

<p align="center">
  <a href="README.en.md">English</a>
  ·
  <a href="https://github.com/mps233/MochiPort/releases">下载</a>
  ·
  <a href="https://github.com/mps233/MochiPort/issues">反馈问题</a>
</p>

<p align="center">
  <a href="https://github.com/mps233/MochiPort/releases/latest"><img src="https://img.shields.io/github/v/release/mps233/MochiPort?display_name=tag&style=flat-square" alt="Latest release"></a>
  <a href="https://github.com/mps233/MochiPort/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/mps233/MochiPort/ci.yml?branch=main&style=flat-square&label=CI" alt="CI status"></a>
  <span> <code>v0.5.6</code></span>
</p>

MochiPort 是一个本地优先的 Codex 会话中继。它把 Codex App、Codex VS Code 插件和 Codex CLI 接到 Telegram、飞书、微信或企业微信，让你可以在消息软件里创建会话、跟进任务和处理审批。

MochiPort 还内置 AI Gateway：Codex 只连接一个本地入口，模型请求再按配置转发到 OpenAI、DeepSeek、Grok/xAI、Anthropic/Claude、智谱 GLM 或其它兼容服务。

## 能做什么

- 通过官方 remote-control 接入 Codex App、VS Code 插件和 Codex CLI；优先使用 VS Code 原生 remote-control。仍需兼容旧版插件时，可显式运行 `mochiport vscode-remote-control patch-fallback`，完成后用 `restore-fallback` 恢复；daemon 启动和停止不会自动修改插件，也不安装包装命令。
- 在消息软件中创建、恢复和操作 Codex thread，接收进度并处理审批。
- 在 macOS SwiftUI 或 Windows 客户端中管理模型服务、模型别名、路由、请求日志和消息渠道。
- 查看 Sub2API 账号池的在线状态、倍率和最近命中账号；查询会调用上游用量探测，强制刷新还会调用 billing 探测，可能同步倍率并持久化快照；可在账号池页面逐账号切换是否参与调度，但不会通过 MochiPort 创建、删除或手动编辑账号。

## Telegram 项目群和话题

Telegram 不需要为每个项目、每个会话单独创建机器人。通常只需要：

- 一个 Telegram Bot；
- 每个项目一个群，并把这个群设置为 Forum（论坛）群；
- 群里的每个 Topic（话题）对应一个 Codex 会话。

这样，同一个项目的多个会话可以放在同一个群里，分别使用不同的话题。话题名称会使用 Codex 会话标题，消息也只会回到对应的话题里。以后你在 Codex 客户端改了会话名称，MochiPort 会实时同步 Telegram 话题；你在 Telegram 里改话题名称，也会实时同步回 Codex 会话。

在官方 Codex 客户端新建会话时，只要会话目录匹配某个项目群，MochiPort 会自动创建同名 Topic 并完成绑定。已经绑定过、没有匹配项目群或项目配置有歧义的会话会跳过，不会重复创建；手动同步仍可作为补偿入口。

<table>
  <tr>
    <td align="center"><strong>项目群话题列表</strong><br><img src="docs/assets/product/telegram-mobile-topics.jpg" alt="Telegram 手机端项目群话题列表" width="280"></td>
    <td align="center"><strong>话题内任务进度</strong><br><img src="docs/assets/product/telegram-mobile-topic-progress.jpg" alt="Telegram 手机端话题内 Codex 任务进度" width="280"></td>
  </tr>
  <tr>
    <td align="center"><strong>私聊持续跟进</strong><br><img src="docs/assets/product/telegram-mobile-chat.jpg" alt="Telegram 手机端 Codex 私聊" width="280"></td>
    <td align="center"><strong>任务执行结果</strong><br><img src="docs/assets/product/telegram-mobile-task-result.jpg" alt="Telegram 手机端 Codex 任务执行结果" width="280"></td>
  </tr>
</table>

MochiPort 会记住“哪个话题对应哪个会话”。Codex 会话被归档后，MochiPort 会每隔一段时间检查；确认归档持续约 5 分钟后，会自动删除对应的话题。Codex 会话真正消失后，也会删除对应话题。这个过程不会删除 Codex 会话本身。

名称变更会通过事件实时同步；如果某次事件因断线或重启丢失，仍会每约 5 分钟自动对账补偿。两边同时改名时，后续对账以 Codex 名称为最终结果，避免互相循环覆盖。

在项目群设置里点击“同步 / 转移 Codex 会话到 Telegram Topic”时，如果某个会话原来绑定在私聊，MochiPort 会自动解除私聊绑定、创建项目群 Topic，再绑定回同一个 Codex 会话，不会删除会话本身。同步结果会逐条显示会话标题和跳过或失败原因。

在 Telegram 里手动关闭话题，只表示暂时不让它接收消息，不会删除 Codex 会话；重新打开后可以继续使用。如果手动删除了话题，Telegram 不会单独通知机器人。MochiPort 会在定期对账时探测绑定的 Topic，确认 Topic 确实不存在后清理绑定；你下一次点击“同步 Telegram 话题”时，会为仍存在的 Codex 会话重新创建并绑定 Topic。网络错误或权限不足时不会贸然清理，也不会创建重复 Topic。

## 界面预览

以下截图保留完整的 macOS 窗口边界和投影；示例中的账号、路径和请求内容已做模糊处理。

<table>
  <tr>
    <td align="center"><strong>概览</strong><br><img src="docs/assets/product/mochiport-overview.png" alt="MochiPort 概览" width="480"></td>
    <td align="center"><strong>Codex 接入</strong><br><img src="docs/assets/product/mochiport-codex-access.png" alt="MochiPort Codex 接入引导清单" width="480"></td>
  </tr>
  <tr>
    <td align="center"><strong>AI 网关</strong><br><img src="docs/assets/product/mochiport-ai-gateway.png" alt="MochiPort AI 网关" width="480"></td>
    <td align="center"><strong>消息渠道</strong><br><img src="docs/assets/product/mochiport-channels.png" alt="MochiPort 消息渠道" width="480"></td>
  </tr>
  <tr>
    <td align="center"><strong>会话</strong><br><img src="docs/assets/product/mochiport-sessions.png" alt="MochiPort 会话" width="480"></td>
    <td align="center"><strong>请求日志</strong><br><img src="docs/assets/product/mochiport-request-logs.png" alt="MochiPort 请求日志" width="480"></td>
  </tr>
</table>

## 快速开始

### 1. 安装

从 [MochiPort Releases](https://github.com/mps233/mochiport/releases) 下载对应平台的程序：

| 平台 | 文件 | 启动方式 |
| --- | --- | --- |
| macOS | `MochiPort-<版本>-build<构建号>-macos-<架构>.dmg` | 拖入 Applications 后打开 |
| Windows | `MochiPort-<版本>-windows-x64.msi` 或 `.zip` | 安装或直接运行 |

打开客户端后，MochiPort 会启动本地 backend。macOS 使用当前用户范围的 LaunchAgent 保持 backend 运行；GUI 退出或崩溃不会停止后台服务，也不会自动重新打开窗口。Linux 桌面包暂不发布，未来将单独设计新的客户端。

### 2. 配置模型

打开 **AI 网关**，添加一个模型服务并填写：

- 协议和 Base URL
- API Key
- 上游模型列表
- 可选的模型别名、权重和超时

Codex 侧只看到 MochiPort 暴露的模型目录。未配置 AI Gateway 时，也可以先完成消息渠道和 Codex 接入。

### 3. 接入 Codex

打开 **Codex 接入**，页面会按进度显示接入引导清单（连接 MochiPort → 选择模型服务 → 检查配置 → 登录 → 桌面控制 → 远程控制）：完成的步骤自动打勾，当前步骤给出操作入口，出现异常的步骤会给出原因和“自动修复”。开启“连接 MochiPort”后，正常启动 Codex App 或 Codex VS Code 插件并开启 remote-control/“控制这台电脑”。remote-control 需要 ChatGPT 兼容的认证模式，仅 API key 认证无法启动；MochiPort 会写入本地连接配置。MochiPort 会通过本地连接读取会话，不需要复制或迁移会话文件。

如果使用 Codex CLI，保持 MochiPort 客户端或 daemon 运行，然后执行：

```bash
codex app-server --listen ws://127.0.0.1:3849 --remote-control
codex --remote ws://127.0.0.1:3849
```

两个命令中的端口必须一致；端口被占用时可以换成其它本地端口。

### 4. 接入消息渠道

打开 **消息渠道**，选择一个或多个渠道：

- **Telegram**：填写 BotFather token；私聊使用 `allowedChatIds` 白名单，留空时会绑定首个私聊，配置的 Forum 项目群按 Topic 分开处理。
- **飞书**：扫码创建机器人，使用 WebSocket 接收事件。
- **微信**：扫码登录微信 iLink Bot。
- **企业微信**：扫码接入 AI Bot，支持私聊和群聊文本。

每个平台可以配置多个账号，并单独启停。实际部署时建议配置 Telegram `allowedChatIds`、飞书 `allowedOpenIds`/`allowedChatIds` 等白名单。

## 常用操作

- **会话**：从当前 Codex App 读取会话，创建或恢复 thread。
- **请求日志**：按状态、渠道和模型筛选请求，查看耗时、用量和错误。
- **Sub2API**：在 **AI 网关 -> 账号** 填写管理地址和 Admin API Key；查询会调用上游用量探测，强制刷新还会调用 billing 探测，可能同步倍率/快照；不会通过 MochiPort 执行账号的创建、删除或手动编辑。
- **网络**：可选系统代理、直连或自定义 HTTP/SOCKS5 代理，只影响 MochiPort 的出站请求。

### Telegram 命令

| 命令 | 作用 |
| --- | --- |
| `/new` | 创建新会话 |
| `/sessions` | 查看并恢复历史会话 |
| `/status` | 查看连接、任务和队列状态 |
| `/steer <内容>` | 调整当前任务方向 |
| `/queue <内容>` | 排到当前任务之后执行 |
| `/stop` | 中断当前任务，保留会话 |
| `/回复 <档位>` | 切换回复颗粒度（摘要 / 标准 / 完整，别名 `/granularity`） |
| `/exit` | 退出会话并清空队列 |
| `/help` | 查看帮助 |

任务运行时直接发送普通文字也会调整当前任务；需要稍后执行的内容使用 `/queue`。

### 回复颗粒度（Telegram）

在 **消息渠道** 展开 Telegram 账号，点击参考卡选择转发详细程度；也可以在 Telegram 里发送 `/回复`（无参数查看当前档位，带参数直接切换）。设置即时生效，无需重启。

| 档位 | 内容 |
| --- | --- |
| 摘要回复 | 只发过程文本（Codex 的说明文字）和最终结果；工具执行、文件修改、推理、计划、搜索全部静默 |
| 标准回复（默认） | 现有行为：所有信息合并进一条聚合气泡，原地更新 |
| 完整回复 | 所有信息逐条独立发送：每条命令执行、文件修改、推理、计划各自一条消息，不合并更新 |

<p align="center">
  <img src="docs/assets/product/mochiport-reply-granularity.png" alt="消息渠道页的回复颗粒度参考卡，三档各带 Telegram 消息效果预览" width="720">
</p>

错误通知和审批卡片不受颗粒度影响，任何档位都会照常送达。

## 安全边界

MochiPort 默认只监听本机：

```text
http://127.0.0.1:3847
```

不要把 daemon 端口暴露到公网。以下内容不要提交到 Git：

- IM Bot token、企业微信 secret、微信 token
- 模型服务 API key
- Sub2API Admin API Key
- Codex 本地认证数据

GUI 中的“恢复原来的设置”会移除 MochiPort 写入的 Codex 连接配置，同时保留后续 Codex 配置变化和会话历史；不会卸载 Codex。VS Code fallback 不会在 daemon 启动或停止时自动修改插件；需要兼容旧版插件时，请显式运行 `mochiport vscode-remote-control patch-fallback`，完成后用 `mochiport vscode-remote-control restore-fallback` 恢复。

## 更多文档

- [配置说明](docs/configuration.md)
- [故障排查](docs/troubleshooting.md)
- [架构说明](docs/architecture.md)
- [Telegram 集成与维护边界](docs/telegram-integration.zh-CN.md)
- [微信集成与已知边界](docs/wechat-integration.zh-CN.md)
- [认证说明](docs/auth-notes.zh-CN.md)
- [构建和交接规则](docs/mochiport-change-handoff.zh-CN.md)
- [发布检查清单](docs/release-checklist.md)

旧版本的 `ThreadRelay`、`CodexHub` 配置目录和环境变量仅在显式 `mochiport migrate-storage` 或只读锁检查中兼容读取；普通启动不会隐式切换到旧目录。新安装请使用 `MochiPort`、`MOCHIPORT_HOME` 和 `mochiport-*`。

## 致谢

MochiPort 源自 [`happy-loki/codexhub`](https://github.com/happy-loki/codexhub)，感谢上游项目及其贡献者提供的基础实现和早期探索。MochiPort 目前由独立维护者继续开发和发布，与上游项目及 OpenAI 没有隶属、背书或官方支持关系。完整的许可证和归属说明见 [NOTICE](NOTICE)。

## 开发

```bash
cargo fmt
cargo test
cargo build --release --bin mochiport
```

许可证：Apache-2.0。上游归属和修改声明见 [NOTICE](NOTICE)。
