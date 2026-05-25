# codex-remote

[中文说明](README.zh-CN.md)

`codex-remote` connects official Codex remote-control sessions to Feishu IM. You keep using `codex` in a terminal; Feishu can subscribe to the same official Codex app-server session, send turns into Codex, and receive assistant output, tool events, turn status, and approval requests from that session.

The project is intentionally a thin bridge. It does not implement a second Codex client, does not create its own workspace, and does not change Codex model, sandbox, approval policy, cwd, or environment. Those stay owned by official Codex app-server.

## What It Does

- Provides a local remote-control backend at `http://127.0.0.1:3847/backend-api`.
- Lets official Codex app-server connect through `/backend-api/wham/remote/control/server`.
- Renders Codex thread items, assistant output, tool cards, turn status, and approval requests to Feishu for Feishu-bound threads.
- Sends Feishu messages back into the selected Codex thread through official app-server JSON-RPC.
- Renders Codex approval requests as Feishu interactive cards and answers the original request id.
- Provides a local web page for Feishu onboarding, shim install/uninstall, status, and diagnostics.

## Architecture

```text
user terminal
  |
  | runs: codex
  v
codex shim
  |
  | starts official Codex app-server:
  |   real-codex -c chatgpt_base_url="http://127.0.0.1:3847/backend-api" app-server --listen ws://127.0.0.1:<port> --remote-control
  | starts official Codex TUI:
  |   real-codex --remote ws://127.0.0.1:<port> -C <user cwd>
  v
official Codex app-server
  |
  | outbound remote-control websocket
  v
codex-remote remote-control backend
  |
  | Feishu websocket events
  | Feishu message/card APIs
  v
Feishu IM
```

Key design points:

- The Codex app-server is the source of truth for threads, turns, approvals, tools, and configuration.
- `codex-remote` only implements the official remote-control backend endpoints and Feishu adaptation.
- Feishu inbound messages are translated to official app-server JSON-RPC requests, usually `turn/start`.
- Feishu only sees threads that it explicitly binds to. `codex-remote` does not globally mirror every Codex thread into Feishu.
- Local TUI `userMessage` items can be forwarded to Feishu for Feishu-bound threads, but Feishu-origin turns are not echoed back into the same Feishu chat.
- Codex server requests such as command/file approvals are rendered as Feishu cards, then answered through the original JSON-RPC request id.
- The shim is only a convenience layer. If disabled or misconfigured, it runs the real Codex binary directly.

More details: [docs/architecture.md](docs/architecture.md)

## Quick Start

Prerequisites:

- Rust stable toolchain
- Official Codex CLI installed and working
- A Feishu/Lark bot app, or use the built-in Feishu scan onboarding flow

Build and start the daemon:

```powershell
cargo run -- --config config.toml daemon
```

Open the local web console:

```text
http://127.0.0.1:3847
```

In the web console:

1. Connect Feishu by scanning the QR code, or fill existing app credentials in `config.toml`.
2. Install the Codex shim.
3. Make sure the page shows Feishu websocket connected.

Then open a new terminal and use Codex normally:

```powershell
cd D:\path\to\your\project
codex
```

When the shim is active, Codex connects to Feishu through official remote-control. The TUI still talks to the official app-server, and the app-server talks to `codex-remote`.

If a Feishu chat is not yet bound to any thread, the first inbound message does not blindly create hidden client state. The bridge shows a thread-selection card so Feishu can subscribe to an existing working thread or a different historical thread first.

## Manual Protocol Debugging

For debugging without the shim, start the daemon, then start official Codex app-server with remote-control enabled and `chatgpt_base_url` pointed at the local backend:

```powershell
codex-remote --config D:\path\to\config.toml daemon
codex -c 'chatgpt_base_url="http://127.0.0.1:3847/backend-api"' app-server --listen ws://127.0.0.1:3849 --remote-control
codex --remote ws://127.0.0.1:3849 -C D:\path\to\your\project
```

Manual mode is only for protocol debugging. The intended user path is daemon plus shim, then just `codex`.

## Thread Binding Model

`codex-remote` does not act like a second full Codex client. It only bridges Feishu to specific Codex threads that Feishu has selected.

- Codex app-server remains the source of truth for thread lifecycle and history.
- Feishu chats bind to one Codex thread at a time.
- If Feishu has not bound a thread yet, the bridge sends a thread list card instead of guessing.
- Resuming a thread from Feishu subscribes to that thread's future remote-control events.
- TUI-origin `userMessage` items on a Feishu-bound thread can appear in Feishu.
- Feishu-origin turns are marked by turn id, so their `userMessage` completion events are suppressed on the way back to Feishu.

This is why Feishu can attach to an existing working thread without `codex-remote` becoming a separate workspace owner.

## Commands

```text
codex-remote [--config PATH] daemon
codex-remote [--config PATH] status
codex-remote [--config PATH] on
codex-remote [--config PATH] off
codex-remote [--config PATH] install-shim [--real-codex PATH] [--bin-dir PATH]
codex-remote [--config PATH] uninstall-shim
codex-remote [--config PATH] shim -- [codex args...]
```

`off` keeps the shim installed but makes it pass through directly to the real Codex binary. You can also bypass the shim for a single terminal:

```powershell
$env:CODEX_REMOTE_DISABLE = "1"
codex
```

## Configuration

Example:

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

`config.toml` contains local credentials and is ignored by git. See [config.example.toml](config.example.toml) and [docs/configuration.md](docs/configuration.md).

## Auth Boundary

`codex-remote` does not own Codex authentication.

- The official Codex binary and app-server still own account state and login behavior.
- The supported path is: make official Codex able to start and connect normally first, then attach `codex-remote` through remote-control.
- `codex-remote` does not replace the official ChatGPT login flow.
- `codex-remote` does not promise support for handcrafted, forged, or otherwise unofficial auth material.

If Codex itself cannot start because of account or login state, resolve that in Codex first. The bridge only starts working after the official app-server is already able to run.

More detail: [docs/auth-notes.md](docs/auth-notes.md)

## References

The repository may contain local reference material used during protocol and auth-shape investigation.

Those materials are not part of the normal `codex-remote` runtime path, do not replace the official remote-control flow, and should not be treated as a supported login bypass mechanism.

## Feishu Commands

```text
/new       bind the Feishu chat to a new Codex thread
/status    show current binding and runtime status
/s /stop   interrupt the active Codex turn
/q         interrupt and clear the current binding
/y /n      approve or reject the current approval request
/1 /2 /3   select an exact approval card option
```

Approval cards are updated after selection, so already handled approvals are marked visually instead of leaving ambiguous old buttons in the chat.

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
GET http://127.0.0.1:3847/api/shim/status
GET http://127.0.0.1:3847/api/events
```

## Security Notes

- The daemon binds to `127.0.0.1` by default. Do not expose it publicly.
- `config.toml` stores Feishu `appId` and `appSecret`; do not commit it.
- Attachments from Feishu are downloaded to a local state-adjacent `.im/attachments/feishu/` directory.
- Restrict access with `allowedOpenIds` and/or `allowedChatIds` for real usage.
- The bridge can send approval decisions to Codex. Treat Feishu access as equivalent to local terminal approval access.

## Troubleshooting

See [docs/troubleshooting.md](docs/troubleshooting.md).
