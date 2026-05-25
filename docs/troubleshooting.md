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

If `connected=false`, start or restart Codex from a project directory:

```powershell
codex
```

## Check The Shim

```powershell
codex-remote --config config.toml status
```

or:

```powershell
Invoke-RestMethod http://127.0.0.1:3847/api/shim/status
```

Common failures:

- real Codex path is not configured
- shim directory is not before official Codex in PATH
- daemon is not running
- Feishu is not configured
- bridge is disabled

## Disable The Bridge

If local Codex behavior is confusing, disable bridge mode:

```powershell
codex-remote --config config.toml off
```

Then run:

```powershell
codex
```

The shim should pass through to the real official Codex command.

To bypass for one terminal:

```powershell
$env:CODEX_REMOTE_DISABLE = "1"
codex
```

## Wrong Project Directory

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

Then open a new terminal in the project directory and run `codex`.

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

If there is no current thread, send a message from Feishu. That lets the bridge create or bind the Codex thread through the official app-server API.

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

## Manual Protocol Debugging

Use matching app-server and TUI ports:

```powershell
codex-remote --config config.toml daemon
codex -c 'chatgpt_base_url="http://127.0.0.1:3847/backend-api"' app-server --listen ws://127.0.0.1:3849 --remote-control
codex --remote ws://127.0.0.1:3849 -C D:\path\to\project
```

## Plugin List Warnings

Warnings such as:

```text
plugin/list featured plugin fetch failed
```

come from the official Codex app-server trying to fetch plugin metadata. They are usually unrelated to the Feishu bridge.

## Windows PowerShell Shell Snapshot Warning

Warnings such as:

```text
Failed to create shell snapshot for powershell
```

come from Codex shell snapshot support and are not caused by `codex-remote`.
