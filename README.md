# codex-remote

`codex-remote` connects a local Codex CLI/TUI session to Feishu IM. You keep using `codex` in a terminal; messages, streaming output, tool events, and approval requests can also be mirrored and controlled from Feishu.

The project is intentionally a thin bridge. It does not replace Codex, does not create its own workspace, and does not change Codex model, sandbox, approval policy, cwd, or environment. Those stay owned by the official Codex CLI/app-server.

## What It Does

- Lets users run `codex` normally in any project directory.
- Mirrors local Codex user messages, assistant output, tool cards, and completion status to Feishu.
- Sends Feishu messages back into the active local Codex thread.
- Supports Feishu image/file attachments by downloading them locally and passing file paths to Codex.
- Renders Codex approval requests as Feishu interactive cards.
- Keeps approval behavior aligned with Codex TUI: one current approval at a time, queued approvals appear only after the current one is resolved.
- Provides a local web page for Feishu onboarding, shim install/uninstall, status, and diagnostics.

## Status

This is an experimental local bridge for developers who already use Codex CLI and Feishu. It currently targets local development and personal/team internal usage rather than hosted multi-tenant deployment.

Tested primarily on Windows. The shim also writes a POSIX shell wrapper, but non-Windows paths need more real-world validation.

## Architecture

```text
user terminal
  |
  | runs: codex
  v
codex shim
  |
  | starts official Codex app-server in the user's current directory
  | starts official Codex TUI with --remote ws://127.0.0.1:3848
  v
codex-remote relay
  |
  | proxies JSON-RPC between TUI and app-server
  | observes notifications and injects Feishu-originated turn/start requests
  v
official Codex app-server

codex-remote bridge
  |
  | Feishu websocket events
  | Feishu message/card APIs
  v
Feishu IM
```

Key design points:

- The Codex app-server is still the source of truth for threads, turns, approvals, tools, and configuration.
- The relay forwards TUI traffic to the real app-server and broadcasts observed server notifications to the Feishu bridge.
- Feishu inbound messages are translated to Codex app-server JSON-RPC requests, usually `turn/start` for the active local thread.
- Codex server requests such as command/file approvals are rendered as Feishu cards, then answered through the original JSON-RPC request id.
- The shim is only a convenience layer. If disabled or misconfigured, it falls back to the real Codex binary.

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
3. Make sure the page shows Feishu websocket connected and relay running.

Then open a new terminal and use Codex normally:

```powershell
cd D:\path\to\your\project
codex
```

When the shim is active, that `codex` command starts:

```text
real-codex app-server --listen ws://127.0.0.1:<temporary-port>
real-codex --remote ws://127.0.0.1:3848
```

Both commands run from the directory where the user typed `codex`, so Codex keeps the correct project cwd.

## Manual Mode

You can use the bridge without installing the shim:

```powershell
codex app-server --listen ws://127.0.0.1:3849
codex-remote --config D:\path\to\config.toml daemon
codex --remote ws://127.0.0.1:3848
```

Manual mode is useful for protocol debugging, but the shim is the recommended user experience.

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

Common usage:

```powershell
codex-remote --config config.toml daemon
codex-remote --config config.toml install-shim
codex-remote --config config.toml off
codex-remote --config config.toml on
codex-remote --config config.toml status
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

[relay]
publicWs = "127.0.0.1:3848"
upstreamWs = "127.0.0.1:3849"

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

## Feishu Features

Supported inbound:

- Text messages
- Images
- Files, audio, and video as local file paths
- Card button callbacks for Codex approvals

Supported outbound:

- Desktop user-message cards
- Streaming assistant cards
- Tool/result cards
- Turn completion cards
- Approval cards

Feishu slash commands:

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

Focused approval tests:

```powershell
cargo test approval
```

Useful status endpoints while the daemon is running:

```text
GET http://127.0.0.1:3847/api/status
GET http://127.0.0.1:3847/api/relay/status
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

Common checks:

```powershell
codex-remote --config config.toml status
Invoke-RestMethod http://127.0.0.1:3847/api/relay/status
Invoke-RestMethod http://127.0.0.1:3847/api/status
```

If `codex` behaves unexpectedly, disable the bridge first:

```powershell
codex-remote --config config.toml off
```

or bypass it:

```powershell
$env:CODEX_REMOTE_DISABLE = "1"
codex
```

## Before Publishing

- Choose and add a license file.
- Remove local logs and generated build directories.
- Confirm `config.toml`, state files, and attachment directories are ignored.
- Replace private paths/screenshots in docs if needed.
- Re-test Feishu onboarding with a clean config.

## License

No license has been selected yet. Add one before publishing to GitHub.
