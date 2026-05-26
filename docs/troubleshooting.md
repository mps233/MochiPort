# Troubleshooting

## Check The Daemon

```powershell
Invoke-RestMethod http://127.0.0.1:3847/api/status
```

Expected:

```json
{
  "running": true,
  "feishuWs": {
    "connected": true
  }
}
```

If `feishuWs.connected` is false, check Feishu credentials, websocket subscription, and the event log in the web console.

## Check Remote-Control

```powershell
Invoke-RestMethod http://127.0.0.1:3847/api/remote-control/status
```

Important fields:

- `connected`: official Codex app-server is connected to the remote-control backend.
- `initialized`: the JSON-RPC `initialize` / `initialized` handshake has completed.
- `currentThreadId`: active Codex thread observed from app-server notifications or responses.
- `lastError`: last remote-control websocket error, if any.

If `connected=false`, check the Codex App side:

1. Codex App config contains `chatgpt_base_url = "http://127.0.0.1:3847/backend-api"`.
2. Codex App auth is `chatgptAuthTokens`, not API-key-only auth.
3. The `codex-remote daemon` process is running before remote control is enabled.
4. Remote control is enabled in Codex App.

## API Key Auth Error

If Codex prints:

```text
remote control requires ChatGPT authentication; API key auth is not supported
```

then Codex App never reached the remote-control websocket. The local backend cannot fix this after the fact.

Use a local ChatGPT-shaped external auth record:

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

The third-party model key does not satisfy this check. It belongs in the model provider config and is used later for model calls.

## Feishu Does Not Receive Messages

Check:

1. Daemon status: Feishu websocket connected.
2. Remote-control status: `connected=true` and `initialized=true`.
3. Feishu allowlists: `allowedOpenIds` and `allowedChatIds`.
4. Group chat mention behavior: if `mentionOnly=true`, mention the bot in group chats.
5. Event log: `GET /api/events` or the web console.

## Feishu Messages Do Not Reach Codex

The bridge sends Feishu text to the active Codex thread. It needs:

- remote-control connected and initialized
- an active current thread, or permission to create one through `thread/start`
- the Feishu conversation bound to that thread

If there is no current thread, send a message from Feishu. The bridge will show a thread-selection card or create/bind through the official app-server API, depending on the current runtime state.

## Approval Cards

Expected behavior:

- Feishu shows only one current approval card per conversation.
- Later approvals stay queued.
- After selecting an option, the original card changes to `已审批`.
- The next queued approval card appears after the current one resolves.

If old approvals are still clickable:

- make sure the daemon was rebuilt and restarted
- check whether `card.action.trigger` events are arriving
- check whether Feishu message update API has permission

If clicking an old card says "please handle current approval first", the bridge is preventing out-of-order approval, which is expected.

## Optional CLI Shim

The shim is only for CLI helper flows. Check it with:

```powershell
codex-remote --config config.toml status
```

or:

```powershell
Invoke-RestMethod http://127.0.0.1:3847/api/shim/status
```

Common shim failures:

- real Codex path is not configured
- shim directory is not before official Codex in PATH
- daemon is not running
- Feishu is not configured
- bridge is disabled

To disable bridge mode:

```powershell
codex-remote --config config.toml off
```

To bypass the shim for one terminal:

```powershell
$env:CODEX_REMOTE_DISABLE = "1"
codex
```

## Wrong Project Directory In CLI Shim Mode

This applies to the optional CLI shim path, not the clean Codex App path.

Codex cwd should be the directory where the user ran `codex`.

The shim starts app-server with:

```text
current_dir = user terminal cwd
```

It also starts TUI with:

```text
--remote ws://127.0.0.1:<temporary-port> -C <user terminal cwd>
```

`-C` matters because official remote TUI mode forwards cwd explicitly.

If Codex shows the `codex-remote` repository as cwd, check:

- whether the shim was bypassed
- whether you manually started app-server/TUI from the wrong directory
- whether a stale app-server process is still running

## Manual Protocol Debugging

Use matching app-server and TUI ports:

```powershell
codex-remote --config config.toml daemon
codex -c 'chatgpt_base_url="http://127.0.0.1:3847/backend-api"' app-server --listen ws://127.0.0.1:3849 --remote-control
codex --remote ws://127.0.0.1:3849 -C D:\path\to\project
```

This is for protocol debugging. Codex App should normally connect directly through `chatgpt_base_url`.

## Plugin List Warnings

Warnings such as:

```text
plugin/list featured plugin fetch failed
```

come from official Codex trying to fetch plugin metadata. They are usually unrelated to the Feishu bridge.

## Windows PowerShell Shell Snapshot Warning

Warnings such as:

```text
Failed to create shell snapshot for powershell
```

come from Codex shell snapshot support and are not caused by `codex-remote`.
