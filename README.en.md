# MochiPort

[中文说明](README.md)

MochiPort is a local-first agent session relay. It connects local Codex App, the Codex VS Code extension, and Codex CLI sessions to Telegram, Feishu, WeChat, and WeCom while providing session management and an AI Gateway.

This project is derived from [`happy-loki/codexhub`](https://github.com/happy-loki/codexhub) and is now independently maintained by the MochiPort maintainers. MochiPort is not affiliated with or endorsed by the upstream authors or OpenAI.

## Product Preview

| Feature | Description |
| --- | --- |
| Remote and local side by side | Use Feishu, WeChat, and Telegram to control local Codex App, the Codex VS Code extension, and Codex CLI. The same Codex session can stay synchronized between IM and local clients. |
| Local Codex access | Does not modify Codex frontend code. Connect Codex App, the VS Code extension, and Codex CLI through the local backend. |
| Codex session management | Read sessions directly from the current Codex App over its remote-control connection and manage them in the GUI. No session files need to be copied or migrated; change a session's provider only when needed. |
| Manage Codex sessions from IM | Use the native Codex remote-control protocol to create and resume Codex sessions from IM. |
| Built-in AI Gateway | Keep Codex App on its native Responses entry while routing model calls to OpenAI, DeepSeek, Anthropic/Claude, Zhipu GLM, or compatible providers from the local GUI. |

<p align="center">
  <img src="docs/assets/product/threadrelay-overview.png" alt="Current MochiPort connection topology and messaging channel overview" width="900">
</p>
<p align="center">
  <img src="docs/assets/product/threadrelay-ai-gateway.png" alt="Current MochiPort AI Gateway providers and visible models" width="900">
</p>

AI Gateway is a local model entry built into `mochiport`. Codex App keeps sending normal Responses-style requests, while `mochiport` routes them to the provider you configured and converts the result back into the shape Codex expects. Providers, visible models, model aliases, request logs, and image-generation-tool filtering are managed in the GUI.

## Quick Start

The main MochiPort flow is: download the app -> configure a model provider -> connect Telegram, Feishu, WeChat, or WeCom -> connect Codex -> start using Codex from the messaging app. If you only want to use the AI Gateway from the local Codex App or VS Code extension, you can skip messaging channels. Codex CLI still requires starting its own app-server in step 7.

### 0. Prerequisites

- macOS, Windows, or Linux device
- Codex App, the Codex VS Code extension, or Codex CLI
- No ChatGPT account and no acceleration network required
- At least one model API key: OpenAI Responses, DeepSeek, Anthropic/Claude, Zhipu GLM, or another compatible provider
- At least one messaging channel: Telegram, Feishu, WeChat, or WeCom; this is the main entry point for using Codex from chat

### 1. Install

Download the appropriate package from [MochiPort Releases](https://github.com/mps233/mochiport/releases). On macOS, open `MochiPort-<version>-macos-<architecture>.dmg` and drag MochiPort to Applications. On Windows, install `MochiPort-<version>-windows-x64.msi` or run `MochiPort.exe` from the ZIP package. On Linux, download `MochiPort-<version>-linux-x86_64.AppImage`, make it executable, then open it.

If macOS warns that the app was downloaded from the internet, confirm the system prompt. The macOS client installs per-user LaunchAgents that keep the local backend running and recover the GUI after an abnormal exit; a normal GUI quit does not reopen the window. If your Linux desktop does not mark the AppImage as executable automatically, run `chmod +x MochiPort-*-linux-x86_64.AppImage` once.

Later, use `Help -> Check for Updates` to manually check GitHub Releases for a newer version. The Rust desktop client can download, verify, and launch the platform installer after confirmation; the SwiftUI macOS client opens the verified release page/update flow. Neither client silently replaces the local app.

### 2. Open The App

Open `MochiPort`. The GUI ensures that the local backend is running. On macOS, the LaunchAgent-managed backend continues running after the GUI exits.

Continue when the status overview shows the local service is running.

### 3. Connect A Messaging Channel (Required For The Main Flow)

Open the `消息接入` page and choose one channel:

- Feishu: click `扫码使用新机器人` and complete QR onboarding.
- Telegram: paste the BotFather token and click `保存并接入`. Private chats support text, image/file input, in-place menu and approval updates, aggregated command and subagent progress per turn, and streamed agent reply drafts; group chats are ignored.
- WeChat: click `扫码连接微信` and confirm in WeChat.
- WeCom: click `添加企业微信机器人` and confirm by scanning with WeCom. Direct/group text, streaming and final replies, image/file transfer, initial/history thread selection cards, and interactive approval template cards are supported.

After a channel is connected, the `IM 通道` status panel becomes available. The main flow only needs one channel; normal use does not require scanning or entering the token again unless you switch bots.

### 4. Configure AI Gateway

Open the `Codex 接入` page and add a model provider in the AI Gateway area. The GUI includes common provider templates, and you can also enter provider details manually:

- Provider name
- Provider type
- Third-party Base URL
- API Key
- Model list

If the upstream model name differs from the name you want to expose in Codex, use `Edit Model Aliases`. For example, the upstream model can be `GLM-5.2` while Codex shows `glm-5.2`.

If a provider rejects Codex's image generation tool, enable `Filter image generation tool`. It takes effect immediately and removes `image_generation` from outgoing AI Gateway requests.

### 5. Let MochiPort Take Over Codex

Turn on `连接 MochiPort` on the `Codex 接入` page. This single switch starts the local MochiPort connection for Codex App and the Codex VS Code extension.

Turning the switch off first restores the Codex connection from before setup, then stops MochiPort for Codex. The restore action is shown only after Codex config has been written.

### 6. Open Codex

Open Codex App or the Codex VS Code extension normally, then enable remote-control / control this computer.

When connected, `MochiPort` shows the Codex control channel as connected.

MochiPort reads the current Codex App's local sessions through its remote-control connection. No import, file copy, or session migration is required. The list can be empty when Codex App is not running, remote-control is disconnected, or a session has not been written to the Codex state database yet.

You do not need to see a remote device list in Codex App's connection settings. This project uses a local backend plus IM bridge. If the `MochiPort` status overview is normal, you can use it directly from the connected IM channel.

If Codex App, the Codex VS Code extension, and Codex CLI are connected to `MochiPort` at the same time, new or resumed IM sessions choose the execution endpoint by fixed priority: Codex App > Codex VS Code extension > Codex CLI. After a session is bound, later messages keep using the selected endpoint until the IM session exits or binds again.

### 7. Use Codex CLI

If you want Codex CLI to work with Feishu / Telegram / WeChat, you do not need to replace the `codex` command or install a wrapper. Use the same three-step flow on macOS, Windows, and Linux.

1. Open the `MochiPort` desktop app, finish IM channel setup and Codex access, and keep it running.

2. Open a terminal in the project directory and start Codex app-server:

```text
codex app-server --listen ws://127.0.0.1:3849 --remote-control
```

3. Open another terminal in the same project directory and connect the local Codex TUI:

```text
codex --remote ws://127.0.0.1:3849
```

After that, you can message the bot from IM, and you can also keep using the same Codex app-server from local Codex TUI. If port `3849` is already in use, choose another local port, but keep the addresses in step 2 and step 3 identical.

### 8. Use IM

Send a message to the bot in Feishu, a Telegram private chat, WeChat, or WeCom.

If the IM chat is not bound to a Codex thread yet, the bot first asks you to create a new thread or resume an existing one. After selection, the chat is bridged to that Codex thread.

The WeChat path depends on a context token issued by the WeChat client. During long tasks or when the phone client has been inactive for a while, the token may expire and the local backend may temporarily be unable to send messages. If this happens, send `!` or `?` in WeChat to refresh the token. These activation messages are only used to recover the send path and are not forwarded to Codex.

## Network and Proxy

The Network menu provides three outbound modes: use the system proxy, connect directly, or use a custom HTTP/SOCKS5 proxy. This setting only affects requests MochiPort sends to model providers, WeChat, Telegram, Feishu HTTP APIs, and update endpoints. It does not modify macOS `launchctl`, Windows user environment variables, or networking for other applications.

For a local Clash or V2Ray proxy, select the custom proxy option and enter `http://127.0.0.1:7890` or `socks5://127.0.0.1:1080`. The setting applies immediately while the daemon is running. Loopback communication between the GUI, Codex App, VS Code, and MochiPort does not use this outbound proxy.

TUN and Network Extension VPNs operate below the HTTP proxy layer. If such a VPN intercepts loopback traffic, exclude `localhost`, `127.0.0.1`, and `::1` in the VPN application.

## AI Gateway

AI Gateway solves one practical problem: Codex expects its native model entry, but users often want to use more model providers. After providers are configured in the GUI, Codex App still sees a normal model list; `mochiport` handles provider routing and protocol conversion locally.

Current highlights:

- OpenAI Responses providers for native or compatible Responses services.
- DeepSeek Responses providers for the native DeepSeek `/v1/responses` API, including hosted web search, function tools, and `apply_patch`.
- DeepSeek Chat / Chat Completions providers retain the existing conversion path back to Codex-compatible Responses output.
- Anthropic Messages providers for Claude / Anthropic-compatible models, including text, images, tool calls, thinking output, and web search conversion.
- Zhipu GLM through the Anthropic-compatible path, including GLM web search normalization.
- Model aliases for case differences, provider-specific names, and third-party relay names.
- Codex visible model selection.
- Request logs with original Codex request, upstream request, response or error, tokens, cache usage, cost, latency, TTFT, and request body size.
- Image generation tool filtering, disabled by default.

All of this is configured from the GUI. Users do not need to hand-edit config files.

## Community And Support

For questions or feedback, open an issue in [MochiPort Issues](https://github.com/mps233/mochiport/issues).

## IM Commands

Telegram shows the standard commands in the bot menu. `/s` and `/q` remain available as compatibility aliases.

```text
/new       create a new session
/sessions  resume a previous session
/status    show connection, task, and queue status
/steer     steer the current task
/queue     run one message after the current task
/stop      interrupt the current task (alias: /s)
/exit      exit the current session (alias: /q)
/help      show command help
```

While a task is running, ordinary text is sent as a direction update; you can also use
`/steer your new direction`. Use `/queue text to run later` to add a message to the
current conversation's FIFO queue. Queued messages start automatically after the current
task finishes.

Use the buttons or numbers shown in approval and session messages.

Approval prompts are updated after selection where the platform supports it.

## Restore Codex Config

Click `Restore Previous Settings` in the GUI to restore the Codex connection from before setup. After restore, Codex App no longer sends model requests through MochiPort.

This does not uninstall Codex and does not delete Codex session history.

## Project Boundary

`mochiport` only supports the clean official Codex remote-control path.

It does not:

- install a `codex` wrapper
- replace Codex CLI
- launch Codex App through a shim
- change Codex model, sandbox, approval policy, cwd, or environment

The macOS client installs only per-user LaunchAgents for MochiPort itself, not a system-wide service. They keep the local backend running and recover the GUI only after an abnormal exit; a normal GUI quit stays quit.

## Technical Notes

Runtime path:

```text
Codex App / Codex VS Code extension / Codex CLI app-server
  |
  | chatgpt_base_url = "http://127.0.0.1:3847/backend-api"
  | user enables remote control, or starts codex app-server --remote-control
  v
official Codex app-server
  |
  | outbound remote-control websocket
  v
MochiPort local backend
  |
  | Feishu websocket events
  | Feishu message/card APIs
  | Telegram long polling
  | Telegram Bot API
  | WeChat iLink long polling
  | WeChat sendmessage API
  | WeCom AI Bot WebSocket / aibot_send_msg
  v
IM channel
```

The project implements the official remote-control endpoints:

```text
POST /backend-api/wham/remote/control/server/enroll
GET  /backend-api/wham/remote/control/server
```

Codex remote-control requires a ChatGPT-compatible auth mode. This project writes local `ChatgptAuthTokens` to satisfy Codex App's remote-control account check. API-key-only auth does not start remote control.

Thread binding model:

- Codex app-server remains the source of truth for thread lifecycle and history.
- One IM chat binds to one Codex thread at a time.
- If the IM chat has not bound a thread yet, the bridge asks whether to create or resume a thread.
- Resuming a thread from IM subscribes to that thread's future remote-control events.
- IM-origin turns are tracked by turn id to avoid `userMessage` echo.

## Development

```powershell
cargo fmt
cargo test
cargo build --release --features gui --bin mochiport
```

Useful status endpoints while the daemon is running:

```text
GET http://127.0.0.1:3847/api/status
GET http://127.0.0.1:3847/api/remote-control/status
GET http://127.0.0.1:3847/api/remote-control/backend-status
GET http://127.0.0.1:3847/api/events
```

## Security Notes

- The daemon binds to `127.0.0.1` by default. Do not expose it publicly.
- Locally saved IM tokens, model API keys, and Codex auth data are secrets; do not commit them.
- Attachments from Feishu and Telegram are downloaded to local state-adjacent `.im/attachments/feishu/` and `.im/attachments/telegram/` directories.
- Restrict access with `allowedOpenIds` and/or `allowedChatIds` for real usage.
- The bridge can send approval decisions to Codex. Treat Feishu / Telegram / WeChat / WeCom access as equivalent to local Codex approval access.

## More Docs

- [Architecture](docs/architecture.md)
- [Telegram integration and maintenance boundaries](docs/telegram-integration.zh-CN.md)
- [WeChat integration and known limitations](docs/wechat-integration.zh-CN.md)
- [Auth notes](docs/auth-notes.md)
- [Troubleshooting](docs/troubleshooting.md)

## Independent Maintenance And Compatibility

- New installations use the MochiPort name, the `MochiPort` config directory, and the `mochiport` command.
- Existing ThreadRelay and CodexHub users continue to load the legacy config directories, `THREADRELAY_*` and `CODEXHUB_*` environment variables, preserving IM credentials, session bindings, and provider configuration.
- Releases and update metadata are published only from [`mps233/mochiport`](https://github.com/mps233/mochiport); maintainers can still selectively merge upstream changes.
- ThreadRelay `0.5.0` and `0.5.1` still point at the retired update URL. There is no compatibility repository, so those users must install one MochiPort release manually before future update checks follow the new repository.

## License

Apache-2.0. See [NOTICE](NOTICE) for upstream attribution and modification notices, and [packaging/THIRD_PARTY_LICENSES.txt](packaging/THIRD_PARTY_LICENSES.txt) for third-party assets.
