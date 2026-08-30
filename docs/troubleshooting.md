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

If `feishuWs.connected` is false, check Feishu credentials, websocket subscription, and `GET /api/events`.

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
2. Codex App is signed in through the official ChatGPT login flow; API-key-only auth does not enable remote control.
3. The `mochiport daemon` process is running before remote control is enabled.
4. Remote control is enabled in Codex App.

## Codex Auth Errors

If Codex prints:

```text
remote control requires ChatGPT authentication; API key auth is not supported
```

then Codex App never reached the remote-control websocket. The local backend cannot fix this after the fact.

Sign in through Codex's official ChatGPT login flow. Do not create or paste a
synthetic `chatgptAuthTokens` record into `auth.json`; MochiPort does not own
that file. The third-party model key belongs in the model provider config and
does not satisfy the remote-control account check.

Older MochiPort builds could leave Codex in external-auth mode and produce:

```text
External auth is active. Use account/login/start (chatgptAuthTokens) to update it or account/logout to clear it.
```

With the local `ai-gateway` active, start the updated MochiPort daemon once so
its legacy migration can restore the saved official auth or remove the old
placeholder. Then fully quit Codex and open it again. If no official auth backup
exists, complete Codex's normal login flow. Direct third-party provider setups
are intentionally not modified by this migration.

## Feishu Does Not Receive Messages

Check:

1. Daemon status: Feishu websocket connected.
2. Remote-control status: `connected=true` and `initialized=true`.
3. Feishu allowlists: `allowedOpenIds` and `allowedChatIds`.
4. Telegram project groups are trusted by explicit `projectGroups` configuration and do not require a mention. Private Telegram chats still use `allowedChatIds`; Feishu and other group channels keep their own mention settings.
5. Event log: `GET /api/events`.

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

To disable bridge mode:

```powershell
mochiport --config config.toml off
```

## Manual Protocol Debugging

Use matching app-server and TUI ports:

```powershell
mochiport --config config.toml daemon
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

come from Codex shell snapshot support and are not caused by MochiPort.
