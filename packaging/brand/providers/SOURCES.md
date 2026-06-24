# Brand Assets

Brand SVGs are copied from `@lobehub/icons` version `5.8.0` when the package contains a matching icon. The package is distributed under the MIT license.

- Codex: `references/lobehub-icons/es/Codex/components/Color.js` -> `packaging/brand/codex.svg`
- OpenAI: `references/lobehub-icons/es/OpenAI/components/Mono.js`
- OpenAI badge: `packaging/brand/openai-badge.svg`; based on the OpenAI icon above, with a light badge background so status icons remain visible in dark theme.
- DeepSeek: `references/lobehub-icons/es/DeepSeek/components/Color.js`
- Anthropic: `references/lobehub-icons/es/Anthropic/components/Mono.js`
- Zhipu: `references/lobehub-icons/es/Zhipu/components/Color.js`

Lucide UI icons are copied from Lucide Icons and covered by `packaging/brand/LICENSE.lucide-icons`.

- Codex CLI terminal: Lucide `terminal` -> `packaging/brand/codex-cli.svg`
- Local service: Lucide `server` -> `packaging/brand/service-server.svg`

Fallback local assets:

- Telegram: `packaging/brand/telegram-logo.svg`; `@lobehub/icons` 5.8.0 has no Telegram component.
- WeChat: `packaging/brand/wechat-logo.svg`; `@lobehub/icons` 5.8.0 has no WeChat/Wechat component.
- Feishu: `packaging/brand/feishu-logo.png`; `@lobehub/icons` 5.8.0 has no Feishu/Lark component.
- App icon: `packaging/icons/dolphin-rounded-256.png`; this is the CodexHub application icon.

The `references/` directory is intentionally ignored by git, so extracted and fallback assets live under `packaging/brand/` or `packaging/icons/` for compile-time embedding in the GUI.
