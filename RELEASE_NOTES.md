CodexHub v0.3.29

本次版本重点更新 AI Gateway 的 Grok/xAI 接入和 Codex 模型目录：

- 新增 Grok/xAI Responses provider 类型，可在 AI Gateway GUI 中直接选择 Grok 渠道，默认使用 `https://api.x.ai/v1`。
- 修复 Grok Responses 连续对话时的 reasoning replay 兼容问题，避免无效 `encrypted_content` 触发上游解码错误。
- 更新 Codex 可见模型目录，新增 `gpt-5.6-sol`、`gpt-5.6-terra`、`gpt-5.6-luna` 和 `grok-4.5`，不再加入 `gpt-5.2`。
- 同步 IM 侧模型 fallback 列表，确保微信、飞书、Telegram 等入口也能看到新的 5.6 与 Grok 模型选项。
- 新增 Grok provider 图标与来源文档，保持 GUI 渠道选择页和品牌资源一致。
