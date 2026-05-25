# Architecture

`codex-remote` bridges three systems:

- Official Codex CLI/TUI
- Official Codex app-server remote-control protocol
- Feishu IM websocket and message APIs

It is not a Codex client replacement. It implements the remote-control backend that official Codex app-server connects to, then adapts those JSON-RPC messages to Feishu.

The design target is strict:

- Codex owns threads, turns, cwd, approvals, tools, and execution semantics.
- `codex-remote` owns only bridge-local transport state.
- Feishu is a remote interaction surface attached to selected Codex threads, not a second source of truth.

## Process Model

With the shim enabled, the user still runs:

```powershell
codex
```

The shim resolves the real Codex binary and starts two official Codex processes:

```text
real-codex -c chatgpt_base_url="http://127.0.0.1:3847/backend-api" app-server --listen ws://127.0.0.1:<temporary-port> --remote-control
real-codex --remote ws://127.0.0.1:<temporary-port> -C <user cwd>
```

`-C <user cwd>` is required for remote TUI mode because official Codex does not infer the remote session cwd from the local process cwd. This is the boundary that prevents `codex-remote`'s repository directory from becoming the Codex project directory.

The daemon runs separately:

```text
codex-remote daemon
```

It owns:

- local web console
- official remote-control backend endpoints
- Feishu websocket listener
- in-memory route/thread/approval/card state

## Remote-Control Backend

The backend exposes the official Codex remote-control paths under `bind`:

```text
POST /backend-api/wham/remote/control/server/enroll
GET  /backend-api/wham/remote/control/server
```

Official Codex app-server connects outbound to those endpoints when started with:

```text
-c chatgpt_base_url="http://127.0.0.1:3847/backend-api" app-server --remote-control
```

Protocol notes:

- Codex sends `ServerEnvelope` values: `server_message`, `server_message_chunk`, `ack`, `pong`.
- `codex-remote` sends `ClientEnvelope` values: `client_message`, `client_message_chunk`, `ack`, `ping`.
- The first client message is JSON-RPC `initialize`; after the initialize response, `codex-remote` sends `initialized`.
- Server envelopes are acknowledged by `seq_id`; chunk acknowledgements include `segment_id`.
- Large outbound client JSON-RPC messages are segmented with the same 100 KiB target used by official Codex.

## Feishu Bridge

The bridge receives Feishu events over Feishu websocket. It handles:

- `im.message.receive_v1`
- `card.action.trigger`

Normal text messages are mapped to Codex input items and sent to the selected Codex thread through `turn/start`. Attachments are downloaded locally and converted into `localImage` or text file-path references.

Outbound Codex events are rendered as Feishu messages/cards:

- thread selection cards
- assistant streaming output
- command/tool cards
- completion cards
- approval cards

The bridge only renders events for threads that are bound to a Feishu conversation.

`userMessage` handling is asymmetric by design:

- TUI-origin `userMessage` items may be rendered to Feishu for a Feishu-bound thread.
- Feishu-origin turns are marked in bridge-local runtime state by `turnId`.
- When Codex later emits `item/completed` for that same `userMessage`, the bridge suppresses it instead of echoing the Feishu message back into the same Feishu chat.

The bridge keeps a Feishu route per Codex thread. A route includes:

```text
conversation_key = feishu:<accountId>:<chatId>
account_id
chat_id
```

## Thread Subscription Model

Feishu does not automatically subscribe to every Codex thread.

The bridge keeps a one-chat-to-one-thread binding and relies on official remote-control thread APIs:

- `thread/list` for historical thread discovery
- `thread/loaded/list` for currently loaded threads
- `thread/resume { excludeTurns: true }` to subscribe to future events of a chosen thread

This is an explicit subscription step, not hidden client logic. Without it, the remote-control backend does not receive future item/turn notifications for arbitrary old threads.

Behavior:

1. Feishu sends a message.
2. If that Feishu conversation is already bound to a live thread, the bridge calls `turn/start`.
3. If it is not bound, the bridge sends a thread-selection card instead of guessing.
4. After the user selects a thread, `codex-remote` calls `thread/resume { excludeTurns: true }`.
5. Future notifications for that thread are then eligible for Feishu rendering.

This keeps the implementation aligned with the official remote-control model instead of inventing a parallel thread store.

## Why a Shim Exists

Manual protocol debugging requires multiple commands:

```powershell
codex-remote daemon
codex -c 'chatgpt_base_url="http://127.0.0.1:3847/backend-api"' app-server --listen ws://127.0.0.1:3849 --remote-control
codex --remote ws://127.0.0.1:3849 -C D:\path\to\project
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

The shim is not the protocol. It is only a launcher convenience so the user can keep typing `codex` instead of manually starting:

- `codex-remote daemon`
- official `codex app-server --remote-control`
- official `codex --remote ... -C <cwd>`

## Approval Handling

Codex app-server sends approval requests as JSON-RPC server requests over remote-control. The bridge stores them as pending approvals.

Important rules:

- Request ids are preserved.
- Feishu card actions answer the original JSON-RPC request id.
- Decision payloads are built from the Codex app-server protocol.
- If `availableDecisions` exists, the bridge uses it.
- Otherwise compatibility decisions mirror Codex TUI behavior.
- Feishu only displays one current approval per conversation.
- Additional approvals remain queued and are sent only after the current approval is resolved.

When a Feishu approval card is selected:

1. The bridge sends `{ "decision": ... }` as the response to the original Codex server request.
2. The original Feishu card is updated to an `已审批` state.
3. The selected option is shown on the card.
4. The next queued approval card is sent, if present.

## Local Web Console

The web console is served from the daemon on `bind`, default:

```text
http://127.0.0.1:3847
```

It provides:

- daemon status
- remote-control status
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

## Auth Experiment References

The repository may keep local reference artifacts used while inspecting Codex auth file shapes.

Those artifacts are for local auth experiments only. They do not change the remote-control architecture above, and they are not required for the normal Feishu bridge path.
