# codex-remote

[中文说明](README.md)

`codex-remote` is a local Codex App remote-control backend with a Feishu/Lark bridge.

It has one job: after the user opens the GUI, Codex App connects to the local backend, and remote-control messages are bridged to Feishu.

## Quick Start

### 1. Install

Download `Codex Remote.dmg` from GitHub Releases, drag it to Applications, then open it.

If macOS warns that the app was downloaded from the internet, confirm the system prompt. The app does not install startup items and does not run in the background automatically.

Later, use `Help -> Check for Updates` to manually check GitHub Releases for a newer version. The MVP only opens the download page; it does not silently replace the local app.

### 2. Open The App

Open `Codex Remote`. The GUI starts the local backend automatically and stops the backend it started when the GUI exits.

Continue when the status overview shows the local service is running.

### 3. Connect Feishu

On first use, click `扫码使用新机器人` and complete the QR onboarding flow.

After Feishu is connected, normal use does not require scanning again. Scan again only when switching bots.

### 4. Fill Model Info

Open the `Codex 接入` page, click `新增`, then fill your model service settings:

- Provider name
- Third-party Base URL
- API Key

Provider name can be empty. If it is empty but Base URL or API Key is filled, the default provider name is `ai-codex`.

### 5. Enable Provider

Click `保存` to save the current provider only. Click `启用` to save the current provider and make Codex App use it.

Enabling a provider backs up the old config, points Codex App remote control to local `codex-remote`, and writes local auth plus the current model provider.

### 6. Open Codex App

Open Codex App normally, then enable remote control in Codex App.

When connected, `Codex Remote` shows Codex App as connected.

### 7. Use Feishu

Send a message to the bot in Feishu.

If the Feishu chat is not bound to a Codex thread yet, the bot first sends a selection card so you can create a new thread or resume an existing one. After selection, the chat is bridged to that Codex thread.

## Community And Support

The WeChat public account is recommended for technical notes, implementation write-ups, and project updates.

<img src="docs/assets/wechat-public-account.jpg" alt="WeChat public account" width="220">

The WeChat group is for issue feedback, usage discussion, and feature suggestions.

<img src="docs/assets/wechat-group.jpg" alt="AI-Agent technical discussion group" width="260">

## Feishu Commands

Only `/q` is needed in normal use. Follow the card prompts for other actions.

```text
/q         interrupt and clear the current binding
```

Approval cards are updated after selection, so handled approvals are marked visually.

## Clear Codex Access

Click `清除 Codex 接入` in the GUI to remove this project's root Codex routing entries:

- `chatgpt_base_url`
- `model_provider`

## Project Boundary

`codex-remote` only supports the clean Codex App remote-control path.

It does not:

- install a `codex` wrapper
- replace Codex CLI
- launch Codex App through a shim
- install login items or startup agents
- run as a background service automatically
- change Codex model, sandbox, approval policy, cwd, or environment

The local backend starts only when the user opens the GUI or explicitly starts it from development tooling.

## Technical Notes

Runtime path:

```text
Codex App
  |
  | chatgpt_base_url = "http://127.0.0.1:3847/backend-api"
  | user enables remote control in the app
  v
official Codex app-server
  |
  | outbound remote-control websocket
  v
codex-remote local backend
  |
  | Feishu websocket events
  | Feishu message/card APIs
  v
Feishu IM
```

The project implements the official remote-control endpoints:

```text
POST /backend-api/wham/remote/control/server/enroll
GET  /backend-api/wham/remote/control/server
```

Codex remote-control requires a ChatGPT-compatible auth mode. This project writes local `ChatgptAuthTokens` to satisfy Codex App's remote-control account check. API-key-only auth does not start remote control.

Thread binding model:

- Codex app-server remains the source of truth for thread lifecycle and history.
- A Feishu chat binds to one Codex thread at a time.
- If Feishu has not bound a thread yet, the bridge sends a thread list card.
- Resuming a thread from Feishu subscribes to that thread's future remote-control events.
- Feishu-origin turns are tracked by turn id to avoid `userMessage` echo.

## Commands

```text
codex-remote [--config PATH] daemon
codex-remote [--config PATH] status
codex-remote [--config PATH] on
codex-remote [--config PATH] off
codex-remote [--config PATH] configure-codex-app [--codex-home PATH] [--provider-name NAME] [--provider-base-url URL] [--provider-key TOKEN] [--model MODEL]
codex-remote [--config PATH] uninstall-codex-app [--codex-home PATH]
```

`on` / `off` enable or pause the Feishu bridge.

`configure-codex-app` is the CLI equivalent of enabling a provider in the GUI. If model provider config is written, the default provider is `ai-codex` and the default model is `gpt-5.5`.

## Configuration

`config.toml` is for `codex-remote` itself:

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

Codex App config is separate and usually lives at `~/.codex/config.toml`.

See [config.example.toml](config.example.toml) and [docs/configuration.md](docs/configuration.md).

## Development

```powershell
cargo fmt
cargo test
cargo build
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
- `config.toml` stores Feishu `appId` and `appSecret`; do not commit it.
- Codex App `auth.json` and third-party provider keys are local secrets; do not commit them.
- Attachments from Feishu are downloaded to a local state-adjacent `.im/attachments/feishu/` directory.
- Restrict access with `allowedOpenIds` and/or `allowedChatIds` for real usage.
- The bridge can send approval decisions to Codex. Treat Feishu access as equivalent to local Codex approval access.

## More Docs

- [Architecture](docs/architecture.md)
- [Configuration](docs/configuration.md)
- [Auth notes](docs/auth-notes.md)
- [Troubleshooting](docs/troubleshooting.md)

## License

Apache-2.0
