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

## Check The Relay

```powershell
Invoke-RestMethod http://127.0.0.1:3847/api/relay/status
```

Important fields:

- `running`: relay listener is started
- `tuiConnected`: official Codex TUI is connected to the relay
- `upstreamConnected`: relay is connected to official Codex app-server
- `currentThreadId`: active Codex thread observed by the relay

If `tuiConnected=false`, start or restart Codex:

```powershell
codex
```

or in manual mode:

```powershell
codex --remote ws://127.0.0.1:3848
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

The shim starts both app-server and TUI with:

```text
current_dir = user terminal cwd
```

If Codex shows the `codex-remote` repository as cwd, check:

- whether you manually started `codex app-server` from the wrong directory
- whether the command is bypassing the shim
- whether a stale app-server process is still running

Restart sequence:

```powershell
codex-remote --config config.toml off
codex-remote --config config.toml on
```

Then open a new terminal in the project directory and run `codex`.

## Feishu Does Not Receive Messages

Check:

1. Daemon status: Feishu websocket connected.
2. Relay status: `tuiConnected=true` and `currentThreadId` is set.
3. Feishu allowlists: `allowedOpenIds` and `allowedChatIds`.
4. Group chat mention behavior: if `mentionOnly=true`, mention the bot in group chats.
5. Event log: `GET /api/events` or the web console.

## Feishu Messages Do Not Reach Codex

The bridge sends Feishu text to the active local thread. It needs:

- a connected TUI
- an active current thread
- the Feishu conversation bound to that thread

If there is no current thread, send a message from the local Codex TUI first or start a new Codex session.

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

## App-Server Connection Errors

Example:

```text
failed to connect upstream app-server `ws://127.0.0.1:<port>`
```

Usually means the app-server process exited or was started on a different port.

With shim mode, close the Codex terminal and start `codex` again.

Manual mode must use matching ports:

```powershell
codex app-server --listen ws://127.0.0.1:3849
codex --remote ws://127.0.0.1:3848
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

## Clean Build Artifacts Before Publishing

```powershell
cargo clean
Remove-Item -Recurse -Force target-verify -ErrorAction SilentlyContinue
Remove-Item *.log -ErrorAction SilentlyContinue
```

Do not remove local `config.toml` unless you no longer need your credentials.
