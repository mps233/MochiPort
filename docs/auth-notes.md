# Auth Notes

This document records the current Codex App auth boundary for MochiPort.

## Current Decision

Codex owns and maintains its `auth.json`. MochiPort preserves the user's existing
official OAuth or API-key login and does not write synthetic ChatGPT JWTs or a
placeholder API key.

Normal takeover routes model requests through the `MochiPort` provider in
`config.toml`. It does not log Codex in or out. Features that require a ChatGPT
account, including upstream remote control, still require the user to sign in
through Codex's official login flow; a model provider key does not satisfy that
account check.

## Config Injection

`mochiport configure-codex-app` writes:

- `chatgpt_base_url = "http://127.0.0.1:3847/backend-api"` for local backend fallback endpoints.
- A default `MochiPort` provider at `http://127.0.0.1:3847/ai-gateway/v1` with `requires_openai_auth = true` and `supports_standalone_web_search = true`. This preserves the Codex App account state, including Fast mode, while registering native `web.run`.
- A local `openai-curated` marketplace entry when the cached curated catalog exists.
- `features.apps = false`, because the host-owned Apps/Connectors MCP backend is not implemented locally.
- Cleanup for legacy plugin-blocking flags such as `plugins = false` and `computer_use = false`.
- Cleanup for old CodexHub-generated bundled remote plugin state.

Older Actor Authorization (`requires_openai_auth = false` plus
`x-openai-actor-authorization`) is recognized only for migration, uninstall, and
cleanup; current configuration no longer writes that header.

The default local provider does not use `experimental_bearer_token` and normal
takeover does not depend on a global `CODEX_API_BASE_URL` environment override.

MochiPort does not publish `openai-bundled` plugins through remote `list` or `installed` fallback. Bundled plugins, including `computer-use`, must come from Codex App's own local `openai-bundled` marketplace.

## Legacy Compatibility

Older MochiPort/CodexHub builds wrote synthetic `chatgptAuthTokens`, and one
in-progress build wrote `OPENAI_API_KEY = "codexhub-dummy-key"` without
`auth_mode`. These shapes are recognized only as legacy MochiPort-managed auth.
When the active configuration is the managed local `MochiPort` provider or a
recognized legacy `ai-gateway`/`ai-codex` shape, an explicit Codex setup action
(the GUI or `mochiport configure-codex-app`) restores the saved official
`auth.json` when available, or removes the synthetic placeholder so Codex can
run its normal login flow. Daemon startup only performs a read-only environment
check and never runs this migration automatically. Direct third-party provider
configurations and unrelated auth files are not changed.

The local `/backend-api/ps/plugins/*` fallback remains narrow:

- It serves cached `openai-curated` remote catalog/detail data.
- It provides read-only detail/skill fallback for old bundled remote IDs already stuck in UI/cache.
- It must not reintroduce bundled plugins into remote list/installed responses.
