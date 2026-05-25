# Configuration

`codex-remote` reads a TOML config file. If no config path is passed, it tries to infer `config.toml` from the current directory or from the repository root when running from `target/debug` or `target/release`.

Use an explicit config path for predictable behavior:

```powershell
codex-remote --config D:\path\to\config.toml daemon
```

## Example

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
binDir = "C:\\Users\\<user>\\AppData\\Local\\codex-remote\\bin"
realCodexPath = "C:\\Users\\<user>\\AppData\\Roaming\\npm\\codex.cmd"
```

Paths relative to the config file are normalized at startup.

## Top-Level Fields

### `bind`

HTTP bind address for the local web console and API.

Default:

```toml
bind = "127.0.0.1:3847"
```

Keep this on localhost. Do not expose it directly to a network.

### `statePath`

Path to the persisted state JSON file.

This stores local bridge state such as Feishu conversation bindings. It should not be committed.

## Feishu

```toml
[feishu]
appId = ""
appSecret = ""
mentionOnly = true
allowedOpenIds = []
allowedChatIds = []
```

### `appId` / `appSecret`

Feishu app credentials. The web onboarding flow can populate these automatically.

Do not commit real credentials.

### `mentionOnly`

When `true`, group messages are ignored unless the bot is mentioned. Direct messages are still accepted.

### `allowedOpenIds`

Optional allowlist of Feishu user `open_id` values.

Empty means no user-level allowlist.

### `allowedChatIds`

Optional allowlist of Feishu chat ids.

Empty means no chat-level allowlist.

## Bridge

```toml
[bridge]
enabled = true
accountId = "default"
sendStreaming = true
```

### `enabled`

Controls whether the shim should connect Codex to Feishu.

When disabled, the shim runs the real Codex directly.

### `accountId`

Local label used to build route keys:

```text
feishu:<accountId>:<chatId>
```

### `sendStreaming`

Controls whether assistant deltas are streamed into Feishu cards.

## Shim

```toml
[shim]
binDir = "..."
realCodexPath = "..."
```

### `binDir`

Directory where the generated `codex` shim is installed.

On Windows the default is:

```text
%LOCALAPPDATA%\codex-remote\bin
```

### `realCodexPath`

Path to the real official Codex executable or command script.

The shim installer tries to discover this automatically from:

- PATH candidates
- common npm global install locations
- configured previous value

If discovery fails, pass it manually:

```powershell
codex-remote --config config.toml install-shim --real-codex C:\path\to\codex.cmd
```

## Feishu App Requirements

For a manually created Feishu app, enable bot messaging and websocket event delivery. Subscribe to:

```text
im.message.receive_v1
card.action.trigger
```

Typical permissions:

```text
im:message
im:message:send_as_bot
im:resource
```

Depending on Feishu app type and tenant policy, additional scopes may be required for card updates or attachment downloads.

## Local Files To Keep Private

These should stay ignored:

```text
config.toml
codex-remote-state.json
*.log
.im/
target/
target-verify/
```

## Reference Auth Artifacts

The repository may also contain local auth-shape reference artifacts.

They are inspection and experiment artifacts only:

- not read by `codex-remote` at runtime
- not a replacement for `config.toml`
- not part of the normal remote-control + Feishu bridge flow
- not documented here as a supported way to bypass official Codex / ChatGPT login

See also:

- [auth-notes.md](auth-notes.md)
- [auth-notes.zh-CN.md](auth-notes.zh-CN.md)
