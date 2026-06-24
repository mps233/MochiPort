# Auth Notes

This document describes the local auth boundary for the Codex App remote-control path.

## Decision

`codexhub` uses a local `chatgptAuthTokens` auth shape for Codex App remote-control identity.

That means:

- Codex App still owns app-server startup and reads its normal Codex home.
- `chatgpt_base_url` points Codex App at the local `codexhub` backend.
- Codex App `auth.json` uses `auth_mode = "chatgptAuthTokens"`.
- The third-party model key stays in Codex model provider config.
- `codexhub` bridges remote-control protocol traffic to Feishu after the app-server connects.

## Why Not API Key Auth

Official Codex remote-control startup rejects API-key-only auth before it connects to the backend:

```text
remote control requires ChatGPT authentication; API key auth is not supported
```

So the model provider key cannot be used as the remote-control identity. It is only for model requests.

## Local `chatgptAuthTokens`

The local auth record is intentionally ChatGPT-shaped because that is what Codex's remote-control gate accepts:

```json
{
  "auth_mode": "chatgptAuthTokens",
  "OPENAI_API_KEY": null,
  "tokens": {
    "id_token": "<local ChatGPT-shaped JWT>",
    "access_token": "<local ChatGPT-shaped JWT>",
    "refresh_token": "",
    "account_id": "acct_codexhub_local"
  },
  "last_refresh": "2026-05-26T00:00:00Z"
}
```

The JWT is local material. Codex reads its claims to find account/user metadata. `codexhub` does not use it to call OpenAI.

## Helper Command

Use:

```powershell
codexhub --config config.toml configure-codex-app
```

This explicitly writes:

- Codex App `config.toml` with `chatgpt_base_url = "http://127.0.0.1:3847/backend-api"`
- Codex App `auth.json` with local `chatgptAuthTokens` auth

Optional provider fields can also be written:

```powershell
codexhub --config config.toml configure-codex-app --provider-name llmx --provider-base-url https://ai.llmx.cloud --provider-key sk-... --model gpt-5.5
```

If provider fields are supplied without `--provider-name`, the helper uses `llmx`.

The command is explicit. The daemon does not modify Codex App config or auth state during startup.

## Runtime Boundary

`codexhub` reads:

- `config.toml`
- local bridge state
- Feishu credentials and bridge settings

Codex App reads:

- Codex App `config.toml`
- Codex App `auth.json`
- model provider settings and keys

After Codex App starts remote-control, `codexhub` only cares about:

- app-server connecting to `/backend-api/wham/remote/control/server`
- remote-control `initialize` / `initialized`
- thread and turn notifications
- approval requests and responses

## Reference Artifacts

The repository may keep local reference artifacts from protocol and auth-shape investigation. They are ignored by git and are not part of daemon startup.
