# 认证说明

这份文档记录 CodexHub 当前和 Codex App 的 auth 边界。

## 当前决策

CodexHub 把 Codex App `auth.json` 写成本地 ChatGPT-shaped token 形态：

```json
{
  "auth_mode": "chatgptAuthTokens",
  "OPENAI_API_KEY": null,
  "tokens": {
    "id_token": "<本地 ChatGPT-shaped JWT>",
    "access_token": "<本地 ChatGPT-shaped JWT>",
    "refresh_token": "",
    "account_id": "acct_codexhub_local"
  },
  "last_refresh": "2026-06-29T00:00:00Z"
}
```

正常初始化不要切到纯 API key auth。新版本 Codex App 在纯 API key 模式下可以显示插件，但上游 remote-control 会在连接 CodexHub 之前拒绝 API key auth。

## 配置注入

`codexhub configure-codex-app` 会写入：

- `chatgpt_base_url = "http://127.0.0.1:3847/backend-api"`，用于本地 backend fallback 接口。
- 默认 `ai-gateway` provider，地址是 `http://127.0.0.1:3847/ai-gateway/v1`。
- `experimental_bearer_token = "dummy-token"`，所以模型请求仍然通过 provider 走 CodexHub。
- 如果本地存在 cached curated catalog，则写入本地 `openai-curated` marketplace。
- 清理历史插件阻断项，例如 `apps = false`、`plugins = false`、`computer_use = false`。
- 清理旧版 CodexHub 生成的 bundled remote plugin 状态。

CodexHub 不通过 remote `list` 或 `installed` fallback 发布 `openai-bundled` 插件。包括 `computer-use` 在内的 bundled 插件必须来自 Codex App 自己的本地 `openai-bundled` marketplace。

## 历史兼容

某个未发布的中间版本曾经写过不带 `auth_mode` 的 `OPENAI_API_KEY = "codexhub-dummy-key"`。当前代码只把这种形态作为卸载/清理时的旧 CodexHub-managed auth 识别对象；它不是目标 auth 形态。

本地 `/backend-api/ps/plugins/*` fallback 继续保持窄范围：

- 服务 cached `openai-curated` remote catalog/detail。
- 对已经卡在 UI/cache 里的旧 bundled remote ID 提供只读 detail/skill fallback。
- 不允许把 bundled 插件重新放回 remote list/installed 响应。
