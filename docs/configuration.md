# Configuration

There are two separate config surfaces:

- MochiPort config, usually this repository's `config.toml`
- Codex App config, usually `~/.codex/config.toml`

Do not mix them. MochiPort stores IM channel and bridge settings. Codex App stores model provider, auth, and `chatgpt_base_url`.

## MochiPort Config

Use an explicit config path for predictable behavior:

```powershell
mochiport --config D:\path\to\config.toml daemon
```

Without `--config`, installed builds use the current MochiPort application-data directory
(`MochiPort/config.toml`). `MOCHIPORT_HOME` can point at a specific data directory. There is no
implicit repository-local or legacy-directory fallback; use an explicit `--config` path for
development or a test fixture.

Historical `ThreadRelay` and `CodexHub` directories, plus `THREADRELAY_HOME` /
`CODEXHUB_HOME`, are migration inputs only. Ordinary startup never selects them. To move one
legacy directory atomically, choose the destination with `MOCHIPORT_HOME` and run the explicit
command (or pass `--from`):

```powershell
$env:MOCHIPORT_HOME = "D:\path\to\MochiPort"
mochiport migrate-storage --from "D:\path\to\ThreadRelay"
```

Migrations never merge an existing MochiPort directory. New automation and new installations
should use the `MOCHIPORT_*` variables and `mochiport` names.

Example:

```toml
bind = "127.0.0.1:3847"
statePath = "mochiport-state.json"

[outboundProxy]
mode = "system"
url = ""

[feishu]
appId = ""
appSecret = ""
mentionOnly = true
allowedOpenIds = []
allowedChatIds = []

[telegram]
botToken = ""
allowedChatIds = []
projectGroups = []

[wechat]
accountId = "wechat"
botToken = ""
baseUrl = ""
userId = ""
botType = "3"
allowedUserIds = []

[bridge]
enabled = true
accountId = "default"
sendStreaming = true
```

Paths relative to the config file are normalized at startup.

### `bind`

HTTP bind address for the local backend API and remote-control websocket.

Default:

```toml
bind = "127.0.0.1:3847"
```

Keep this on localhost. Do not expose it directly to a network.

### `outboundProxy`

Controls only requests that MochiPort sends to external services such as model providers,
WeChat, Telegram, Feishu HTTP APIs, and update endpoints. It does not change the operating
system proxy or the environment of other applications.

```toml
[outboundProxy]
mode = "system" # system | direct | custom
url = ""
```

- `system` follows the operating system proxy and proxy environment variables.
- `direct` disables proxy discovery for MochiPort HTTP requests.
- `custom` uses `url` as an explicit HTTP, HTTPS, SOCKS5, or SOCKS5H proxy.

Example for a local Clash mixed port:

```toml
[outboundProxy]
mode = "custom"
url = "http://127.0.0.1:7890"
```

The desktop GUI exposes the same setting under `Network` and applies it immediately while the
daemon is running. Local GUI-to-daemon requests always bypass proxies. A VPN implemented as a TUN or Network Extension may still route traffic below
the HTTP proxy layer; configure loopback exclusions in that VPN when necessary.

### `statePath`

Path to the persisted state JSON file.

This stores local bridge state such as IM conversation bindings. It should not be committed.

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

Feishu app credentials. The desktop GUI onboarding flow can populate these automatically.

Do not commit real credentials.

### `mentionOnly`

When `true`, group messages are ignored unless the bot is mentioned. Direct messages are still accepted.

### `allowedOpenIds`

Optional allowlist of Feishu user `open_id` values.

Empty means no user-level allowlist.

### `allowedChatIds`

Optional allowlist of Feishu chat ids.

Empty means no chat-level allowlist.

## Telegram

```toml
[telegram]
botToken = ""
allowedChatIds = []
projectGroups = []
```

### `botToken`

Telegram Bot token from BotFather. `bot_token` is also accepted for hand-written config.

This is the Telegram Bot API flow: create one bot with BotFather and add it to the project groups that MochiPort should control. It does not require Telegram `api_id`, `api_hash`, phone login, or an MTProto user session.

Private chats continue to use `allowedChatIds`. Forum groups are enabled explicitly through `projectGroups`; other groups are ignored.

`mentionOnly` is kept for compatibility with the existing Telegram onboarding. Explicit `projectGroups` are treated as trusted project groups, so their messages are accepted without requiring an `@机器人` mention. Private chats still follow `allowedChatIds`.

### `allowedChatIds`

Allowlist of Telegram private chat ids as strings.

Empty means "bind the first private chat". After the first private Telegram message is accepted, MochiPort writes that chat id into `allowedChatIds` and rejects other private chats.

For stricter setup, prefill this list before starting the bridge:

```toml
allowedChatIds = ["123456789"]
```

### `projectGroups`

Each configured forum group represents one project. Enable forum mode, add the bot as an administrator, then add a mapping like this:

```toml
projectGroups = [
  { chatId = "-1001234567890", projectName = "MochiPort", cwd = "/Users/you/src/mochiport" },
  { chatId = "-1009876543210", projectName = "CellularBridge", cwd = "/Users/you/src/CellularBridge" },
]
```

The first message in the group automatically creates a Topic named from that message and binds a Codex thread using the configured project directory. Messages in an existing Topic continue in that Topic's thread. The Topic id is part of the conversation route, so multiple Topics in the same project can run and receive replies independently. The bot needs permission to manage topics; otherwise MochiPort sends a setup hint and ignores the message.

Treat a configured project group as trusted: every member who can send messages there can ask Codex to operate on the mapped project directory.

## WeChat

```toml
[wechat]
accountId = "wechat"
botToken = ""
baseUrl = ""
userId = ""
botType = "3"
allowedUserIds = []
```

WeChat config is normally written by the GUI QR onboarding flow. The implementation follows the OpenClaw WeChat bot path: QR login through `https://ilinkai.weixin.qq.com`, bot type `3`, long polling through `ilink/bot/getupdates`, and text replies through `ilink/bot/sendmessage`.

### `accountId`

Local label for the WeChat bot account. It is used in route keys and persisted state.

### `botToken`

WeChat bot token returned by QR onboarding. Do not commit real tokens.

### `baseUrl`

WeChat iLink API base URL. Leave empty unless the QR flow returns a redirected host.

### `userId`

The WeChat user id returned by onboarding. It is stored for display and allowlist defaults.

### `botType`

Current bot type. The default is `3`.

### `allowedUserIds`

Optional allowlist of WeChat user ids.

Empty means no user-level allowlist.

## WeCom (Enterprise WeChat)

```toml
[wecom]
enabled = true
accountId = "wecom"
botId = ""
secret = ""
displayName = "企业微信机器人"
websocketUrl = "wss://openws.work.weixin.qq.com"
allowedUserIds = []
allowedChatIds = []
```

The GUI QR flow normally writes `botId` and `secret`. MochiPort then subscribes to the official WeCom AI Bot WebSocket and supports direct/group text, streaming and final replies, initial/history thread routing cards, image/file input and output, and interactive approval template cards. Empty allowlists accept all users and chats. Keep `secret` private.

## Bridge

```toml
[bridge]
enabled = true
accountId = "default"
sendStreaming = true
```

### `enabled`

Controls whether the IM bridge should run.

When disabled, Feishu and WeCom websocket listening, Telegram polling, and WeChat polling stop, and IM messages are not forwarded to Codex.

### `accountId`

Local label used to build route keys:

```text
feishu:<accountId>:<chatId>
telegram:<accountId>:<chatId>
wechat:<accountId>:<userId>
wecom:<accountId>:<userId-or-groupChatId>
```

### `sendStreaming`

Controls whether assistant deltas are streamed into Feishu cards.

## Codex App Config

Codex App must point ChatGPT backend traffic at the local daemon:

```toml
chatgpt_base_url = "http://127.0.0.1:3847/backend-api"
```

This belongs in the Codex App config home, usually:

```text
~/.codex/config.toml
```

Third-party model provider keys stay in the Codex model provider section. Example:

```toml
model_provider = "llmx"
model = "gpt-5.5"

chatgpt_base_url = "http://127.0.0.1:3847/backend-api"

[model_providers.llmx]
name = "llmx"
base_url = "https://ai.llmx.cloud"
wire_api = "responses"
requires_openai_auth = true
experimental_bearer_token = "your-third-party-key"
```

`chatgpt_base_url` is not the model API base URL. It is the ChatGPT backend-shaped URL used by Codex App features such as remote-control enrollment.
MochiPort preserves unrelated Codex App settings. Its compatibility setup only manages the local backend/provider, required bundled plugin entries, the local curated marketplace, telemetry defaults, and `features.apps = false` because the host-owned Apps MCP backend is not implemented locally.

When MochiPort injects its default local AI Gateway provider, Codex keeps its existing official account state while model requests use the standalone web-search capability:

```toml
model_provider = "MochiPort"
web_search = "live"

[model_providers.MochiPort]
name = "MochiPort"
base_url = "http://127.0.0.1:3847/ai-gateway/v1"
wire_api = "responses"
requires_openai_auth = true
supports_websockets = false
supports_standalone_web_search = true
```

The provider key and identity are both `MochiPort`. Because that identity is not `OpenAI`, OpenAI-only private-state behavior is not enabled accidentally. `supports_standalone_web_search` lets current Codex builds expose native `web.run` for this custom provider while preserving the account state used by Codex App. The default local provider does not need a dummy bearer token, and normal takeover does not depend on a global `CODEX_API_BASE_URL` override. The model catalog still comes from MochiPort's `/models` endpoint. Older `ai-gateway`/`ai-codex` and Actor Authorization configurations remain recognized for migration and cleanup only.

## Codex App Auth

Codex App owns `auth.json`. Sign in through Codex's official OAuth or API-key
flow; MochiPort preserves that file and does not generate ChatGPT-shaped JWTs.
Remote control still requires a supported ChatGPT account and may reject
API-key-only auth before its websocket connects. A third-party model provider
key controls model calls only and does not replace the Codex account login.

When you explicitly configure Codex App (from the desktop GUI or with
`mochiport configure-codex-app`), MochiPort recognizes auth placeholders written by
older versions only when the active configuration is the managed local `MochiPort`
provider or a recognized legacy `ai-gateway`/`ai-codex` shape. It restores the saved
official auth file if one exists, or removes the legacy placeholder so Codex can
present its normal login flow. Daemon startup only inspects the Codex environment and
does not perform this migration. Direct third-party provider configurations are not
altered.

The desktop GUI provides Codex App configuration controls that write the local Codex App config for you.

The CLI equivalent is:

```powershell
mochiport --config config.toml configure-codex-app
```

Optional provider fields:

```powershell
mochiport --config config.toml configure-codex-app --provider-name llmx --provider-base-url https://ai.llmx.cloud --provider-key sk-... --model gpt-5.5
```

When provider fields are supplied without `--provider-name`, `llmx` is used as the provider name.

The daemon does not modify Codex App config on startup. It writes these files only when the desktop GUI or CLI command is used.

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
mochiport-state.json
*.log
.im/
target/
target-verify/
reference/
```

Do not commit Codex App `auth.json`, third-party provider keys, Feishu credentials, open ids, or chat ids.
