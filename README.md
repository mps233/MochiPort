# codex-remote

[中文说明](README.zh-CN.md)

`codex-remote` is a local Codex App remote-control backend with a Feishu/Lark bridge.

It has one job: after the user explicitly starts the local service, Codex App connects to the local backend, and remote-control messages are bridged to Feishu.

## Quick Start

### 1. Download Or Build

macOS release builds are produced by GitHub Actions. Download `Codex Remote.dmg`, then open the app.

For development:

```powershell
cargo run --features gui --bin codex-remote-gui
```

The GUI uses wxDragon and requires CMake. Daemon, web console, and tests do not require the GUI feature.

### 2. Start Local Service

Open `Codex Remote.app`, then click `Start Local Service`.

The local service listens on:

```text
http://127.0.0.1:3847
```

You can also start the daemon from the command line:

```powershell
cargo run -- --config config.toml daemon
```

### 3. Connect Feishu

In the GUI, click `Change Bot` and follow the QR onboarding flow.

If you already have Feishu bot credentials, you can write them to `config.toml`:

```toml
[feishu]
appId = ""
appSecret = ""
mentionOnly = true
allowedOpenIds = []
allowedChatIds = []
```

### 4. Fill Model Provider

In the Codex App page, fill:

- Provider name
- Third-party Base URL
- API Key

If provider name is empty but Base URL or API Key is provided, the default provider name is `codex`.

The third-party key belongs to Codex's model provider config. `chatgpt_base_url` is only for Codex App backend and remote-control traffic.

### 5. Write Codex App Config

Click `Write Config`.

This explicitly writes Codex App local config:

- `chatgpt_base_url = "http://127.0.0.1:3847/backend-api"`
- Local `ChatgptAuthTokens`
- Optional model provider config

Existing files are backed up as `.bak`. Starting the daemon does not modify Codex App config; only this button or the matching CLI command writes it.

### 6. Enable Remote Control In Codex App

Open Codex App normally, then enable remote control in the app.

If connected, the GUI shows Codex App as connected. You can also check:

```text
GET http://127.0.0.1:3847/api/remote-control/status
```

Expected status:

```json
{
  "connected": true,
  "initialized": true
}
```

### 7. Select A Thread In Feishu

If a Feishu chat is not bound to a thread yet, the first message does not create hidden client state. The bridge sends a thread-selection card so the user can create a new thread or resume an existing one.

## Feishu Commands

```text
/new       bind the Feishu chat to a new Codex thread
/status    show current binding and runtime status
/s /stop   interrupt the active Codex turn
/q         interrupt and clear the current binding
/y /n      approve or reject the current approval request
/1 /2 /3   select an exact approval card option
```

Approval cards are updated after selection, so handled approvals are marked visually.

## Uninstall Injection

Click `Uninstall Injection` in the GUI to remove this project's Codex App injection:

- `chatgpt_base_url`
- `model_provider`
- local `ChatgptAuthTokens` auth file

CLI equivalent:

```text
codex-remote [--config PATH] uninstall-codex-app [--codex-home PATH]
```

## Project Boundary

`codex-remote` only supports the clean Codex App remote-control path.

It does not:

- install a `codex` wrapper
- replace Codex CLI
- launch Codex App through a shim
- install login items or startup agents
- run as a background service automatically
- change Codex model, sandbox, approval policy, cwd, or environment

The local backend starts only when the user clicks `Start Local Service` or explicitly runs the `daemon` command.

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

`configure-codex-app` is the CLI equivalent of the GUI `Write Config` button. If model provider config is written, the default provider is `codex` and the default model is `gpt-5.5`.

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
