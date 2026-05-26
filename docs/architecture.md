# Architecture

`codex-remote` bridges three systems:

- Codex App / official Codex app-server remote-control protocol
- A local ChatGPT backend-shaped base URL
- Feishu IM websocket and message APIs

It is not a Codex client replacement. It implements the remote-control backend that official Codex app-server connects to, then adapts those JSON-RPC messages to Feishu.

The design target is strict:

- Codex owns threads, turns, cwd, approvals, tools, and execution semantics.
- `codex-remote` owns only bridge-local transport state.
- Feishu is a remote interaction surface attached to selected Codex threads, not a second source of truth.

## Process Model

The primary path is Codex App direct connection:

```text
Codex App
  |
  | ~/.codex/config.toml:
  |   chatgpt_base_url = "http://127.0.0.1:3847/backend-api"
  |
  | user enables remote control
  v
official Codex app-server
  |
  | GET /backend-api/wham/remote/control/server
  | outbound websocket
  v
codex-remote daemon
  |
  | Feishu websocket listener
  | Feishu message/card APIs
  v
Feishu IM
```

The daemon runs separately:

```text
codex-remote daemon
```

It owns:

- local web console
- official remote-control backend endpoints
- local ChatGPT backend compatibility endpoints needed by the app
- Feishu websocket listener
- in-memory route/thread/approval/card state

## Remote-Control Backend

The backend exposes the official Codex remote-control paths under `bind`:

```text
POST /backend-api/wham/remote/control/server/enroll
GET  /backend-api/wham/remote/control/server
```

Official Codex app-server connects outbound to those endpoints when Codex App has:

```toml
chatgpt_base_url = "http://127.0.0.1:3847/backend-api"
```

and remote control is enabled.

Protocol notes:

- Codex sends `ServerEnvelope` values: `server_message`, `server_message_chunk`, `ack`, `pong`.
- `codex-remote` sends `ClientEnvelope` values: `client_message`, `client_message_chunk`, `ack`, `ping`.
- The first client message is JSON-RPC `initialize`; after the initialize response, `codex-remote` sends `initialized`.
- Server envelopes are acknowledged by `seq_id`; chunk acknowledgements include `segment_id`.
- Large outbound client JSON-RPC messages are segmented with the same 100 KiB target used by official Codex.

## Local Auth Shape

Remote-control startup is gated by Codex auth, before the websocket reaches `codex-remote`. API-key-only auth is rejected by official Codex app-server.

For this project, the local identity shape is `chatgptAuthTokens`:

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

The JWT only needs the ChatGPT-shaped claims Codex reads locally, especially:

```json
{
  "email": "codex-remote-local@example.local",
  "https://api.openai.com/auth": {
    "chatgpt_account_id": "acct_codex_remote_local",
    "chatgpt_user_id": "user_codex_remote_local",
    "user_id": "user_codex_remote_local",
    "chatgpt_plan_type": "pro",
    "chatgpt_account_is_fedramp": false
  }
}
```

The third-party model key is separate. It belongs in the Codex model provider configuration and is used for model calls, not remote-control enrollment.

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

- Codex-origin `userMessage` items may be rendered to Feishu for a Feishu-bound thread.
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

## Codex App Runtime

`codex-remote` is intentionally scoped to Codex App remote-control. Codex App is launched normally by the user, reads `chatgpt_base_url = "http://127.0.0.1:3847/backend-api"`, and opens the remote-control websocket back to the local daemon. The project does not install a CLI wrapper or start Codex processes on the user's behalf.

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
- Feishu onboarding and bridge on/off
- remote-control status
- Codex App config hints
- recent event log

## State Boundaries

`codex-remote` owns only bridge-local state:

- config path
- Feishu app credentials
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
- model provider keys
