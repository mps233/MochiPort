# Architecture

`codex-remote` bridges three systems:

- Official Codex CLI/TUI
- Official Codex app-server JSON-RPC protocol
- Feishu IM websocket and message APIs

It is not a Codex client implementation. It stays between an existing Codex TUI and app-server, forwarding protocol traffic and mirroring selected events to Feishu.

## Process Model

With the shim enabled, the user still runs:

```powershell
codex
```

The shim resolves the real Codex binary and starts two official Codex processes:

```text
real-codex app-server --listen ws://127.0.0.1:<temporary-port>
real-codex --remote ws://127.0.0.1:3848
```

Both are started with `current_dir` set to the directory where the user ran `codex`. This is important: `codex-remote` must not replace the user's project cwd with the bridge repository cwd.

The daemon runs separately:

```text
codex-remote daemon
```

It owns:

- local web console
- relay websocket endpoint
- Feishu websocket listener
- in-memory route/thread/approval/card state

## Relay

The relay exposes a public websocket endpoint, by default:

```text
ws://127.0.0.1:3848
```

The official Codex TUI connects to this endpoint via `--remote`. The relay then connects to the upstream app-server endpoint that the shim registered for the current session.

The relay forwards:

- TUI -> app-server requests and notifications
- app-server -> TUI responses, notifications, and server requests

It also observes protocol traffic:

- tracks active thread id and turn id
- broadcasts server notifications to bridge tasks
- records request method metadata for logging
- filters bridge-owned JSON-RPC responses so they are not sent back to the TUI

Bridge-owned requests use generated numeric ids greater than or equal to `100000`.

## Feishu Bridge

The bridge receives Feishu events over Feishu websocket. It handles:

- `im.message.receive_v1`
- `card.action.trigger`

Normal text messages are mapped to Codex input items and sent to the active Codex thread through `turn/start`. Attachments are downloaded locally and converted into `localImage` or text file-path references.

Outbound Codex events are rendered as Feishu messages/cards:

- local desktop user message
- assistant streaming output
- command/tool cards
- completion cards
- approval cards

The bridge keeps a Feishu route per Codex thread. A route includes:

```text
conversation_key = feishu:<accountId>:<chatId>
account_id
chat_id
```

## Why a Shim Exists

Manual startup requires three commands:

```powershell
codex app-server --listen ws://127.0.0.1:3849
codex-remote daemon
codex --remote ws://127.0.0.1:3848
```

That is too much for daily use. The shim makes the normal command work:

```powershell
codex
```

The shim is deliberately conservative:

- If the bridge is off, it runs the real Codex directly.
- If Feishu is not configured, it runs the real Codex directly.
- If the daemon is unavailable, it runs the real Codex directly.
- If the command is a subcommand such as `login`, `mcp`, `plugin`, `app-server`, or already uses `--remote`, it runs the real Codex directly.

The shim should be transparent when the bridge is not ready.

## Approval Handling

Codex app-server sends approval requests as JSON-RPC server requests. The relay observes these and the bridge stores them as pending approvals.

Important rules:

- Request ids are preserved.
- Feishu card actions answer the original JSON-RPC request id.
- Decision payloads are built from the Codex app-server protocol.
- If `availableDecisions` exists, the bridge uses it.
- If not, fallback decisions mirror Codex TUI behavior.
- Feishu only displays one current approval per conversation.
- Additional approvals remain queued and are sent only after the current approval is resolved.

This mirrors the TUI user experience: there may be multiple pending approvals internally, but the user sees and handles one active prompt at a time.

When a Feishu approval card is selected:

1. The bridge sends `{ "decision": ... }` as the response to the original Codex server request.
2. The original Feishu card is updated to an `已审批` state.
3. The selected option is shown on the card.
4. The next queued approval card is sent, if present.

## Streaming Cards

The bridge supports two outbound card approaches:

- Feishu interactive message updates
- CardKit streaming cards

The runtime stores per-item card state so deltas can update the same message instead of flooding the chat. Completed items are marked complete and no longer updated.

## Local Web Console

The web console is served from the daemon on `bind`, default:

```text
http://127.0.0.1:3847
```

It provides:

- daemon status
- relay status
- shim status
- Feishu scan onboarding
- shim install/uninstall
- bridge on/off
- recent event log

## State Boundaries

`codex-remote` owns only bridge-local state:

- config path
- Feishu app credentials
- shim configuration
- Feishu conversation to Codex thread binding
- pending approvals
- Feishu card ids/message ids
- downloaded attachments

Codex-owned state stays in Codex:

- project cwd
- sandbox policy
- model
- approval policy
- thread data
- tool execution semantics
- MCP configuration
