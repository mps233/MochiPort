# codex-remote

[中文说明](README.zh-CN.md)

`codex-remote` is a local remote-control backend for Codex App, with a Feishu/Lark bridge on top. The primary path is:

```text
Codex App
  |
  | reads: chatgpt_base_url = "http://127.0.0.1:3847/backend-api"
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

The project stays a thin bridge. It does not implement a second Codex client, does not own a workspace, and does not change Codex model, sandbox, approval policy, cwd, or environment. Those stay owned by official Codex App/app-server.

## What It Does

- Provides a local ChatGPT backend-shaped base URL at `http://127.0.0.1:3847/backend-api`.
- Implements the official remote-control endpoints:
  - `POST /backend-api/wham/remote/control/server/enroll`
  - `GET /backend-api/wham/remote/control/server`
- Lets Codex App connect to that local backend when `chatgpt_base_url` points to `codex-remote`.
- Renders Codex thread items, assistant output, tool events, turn status, and approval requests to Feishu for Feishu-bound threads.
- Sends Feishu messages back into the selected Codex thread through official app-server JSON-RPC.
- Provides a local web console for Feishu onboarding, bridge status, Codex App setup, and remote-control diagnostics.

## Supported Runtime Shape

The clean Codex App path is config-driven:

```toml
# ~/.codex/config.toml, or the Codex App config home
chatgpt_base_url = "http://127.0.0.1:3847/backend-api"
```

Codex remote-control requires a ChatGPT-compatible auth mode. For this project, the local auth shape is `chatgptAuthTokens`: a local, transparent, ChatGPT-shaped external token record used to satisfy Codex App's remote-control account check. The third-party model key remains in Codex's model provider config; it is not the remote-control identity.

Minimal auth shape:

```json
{
  "auth_mode": "chatgptAuthTokens",
  "OPENAI_API_KEY": null,
  "tokens": {
    "id_token": "<local ChatGPT-shaped JWT>",
    "access_token": "<local ChatGPT-shaped JWT>",
    "refresh_token": "",
    "account_id": "acct_codex_remote_local"
  },
  "last_refresh": "2026-05-26T00:00:00Z"
}
```

Important boundary: API-key-only auth does not pass remote-control startup. Codex will fail with:

```text
remote control requires ChatGPT authentication; API key auth is not supported
```

## Quick Start

Prerequisites:

- Rust stable toolchain
- Codex App installed
- A Feishu/Lark bot app, or the built-in Feishu scan onboarding flow

Start the daemon:

```powershell
cargo run -- --config config.toml daemon
```

Open the local web console:

```text
http://127.0.0.1:3847
```

You can also run the native GUI preview. The GUI does not install login items or start background services by itself; use the `Start Local Service` button or run the daemon command explicitly when needed:

```powershell
cargo run --features gui --bin codex-remote-gui
```

The wxDragon GUI requires CMake on the machine. Without CMake, daemon, web console, and tests still build normally because the GUI dependency is isolated behind the `gui` feature. If CMake was installed through Homebrew but is not visible to the current shell, use `PATH=/opt/homebrew/bin:$PATH cargo run --features gui --bin codex-remote-gui`.

Then:

1. Connect Feishu by scanning the QR code, or fill existing app credentials in `config.toml`.
2. Click `Configure Codex App` in the web console. This writes local `chatgpt_base_url` and `chatgptAuthTokens`, with `.bak` backups for existing files.
3. Open Codex App by double-clicking it.
4. Enable remote control in Codex App.
5. Check `GET http://127.0.0.1:3847/api/remote-control/status`.

Expected remote-control status:

```json
{
  "connected": true,
  "initialized": true
}
```

If a Feishu chat is not yet bound to any thread, the first inbound message does not create hidden client state. The bridge shows a thread-selection card so Feishu can subscribe to an existing working thread or another historical thread first.

## Third-Party Model Key

The third-party key belongs in Codex's model provider config. `chatgpt_base_url` is for Codex App backend and remote-control traffic. `base_url` and `experimental_bearer_token` are for model calls.

## Thread Binding Model

`codex-remote` does not act like a second full Codex client. It only bridges Feishu to specific Codex threads that Feishu has selected.

- Codex app-server remains the source of truth for thread lifecycle and history.
- Feishu chats bind to one Codex thread at a time.
- If Feishu has not bound a thread yet, the bridge sends a thread list card instead of guessing.
- Resuming a thread from Feishu subscribes to that thread's future remote-control events.
- Local Codex-origin `userMessage` items on a Feishu-bound thread can appear in Feishu.
- Feishu-origin turns are marked by turn id, so their `userMessage` completion events are suppressed on the way back to Feishu.

## Runtime Boundary

`codex-remote` only supports the clean Codex App remote-control path. It does not install a `codex` wrapper, replace the Codex CLI, or launch Codex App through a shim. The GUI does not install login items or run as a background service automatically; the backend starts only when the user clicks `Start Local Service` or explicitly runs the `daemon` command.

## macOS App

Release builds are produced by GitHub Actions only. The `release-macos` workflow builds a notarized `Codex Remote.dmg` from a tag or manual workflow run.

The app bundle contains both the GUI and daemon binaries; the GUI starts the bundled daemon only after the user clicks `Start Local Service`. The default config path is `~/Library/Application Support/Codex Remote/config.toml`.

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

`configure-codex-app` is the CLI equivalent of the web console button. It explicitly writes Codex App `config.toml` and `auth.json` with local `chatgpt_base_url` and `chatgptAuthTokens`. Provider options default to `codex` / `gpt-5.5` when model provider fields are supplied. Daemon startup does not modify Codex App config until the user clicks the button or runs this command.

`uninstall-codex-app` removes this project's injected `chatgpt_base_url` and local `ChatgptAuthTokens` auth file.

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

Codex App config is separate and lives in the Codex config home, usually `~/.codex/config.toml`.

See [config.example.toml](config.example.toml) and [docs/configuration.md](docs/configuration.md).

## Feishu Commands

```text
/new       bind the Feishu chat to a new Codex thread
/status    show current binding and runtime status
/s /stop   interrupt the active Codex turn
/q         interrupt and clear the current binding
/y /n      approve or reject the current approval request
/1 /2 /3   select an exact approval card option
```

Approval cards are updated after selection, so handled approvals are marked visually instead of leaving ambiguous old buttons in the chat.

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
