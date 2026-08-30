# 认证说明

这份文档记录 MochiPort 当前和 Codex App 的 auth 边界。

## 当前决策

Codex App 的 `auth.json` 由 Codex 自己维护。MochiPort 保留用户已有的官方
OAuth 或 API key 登录，不写入伪造的 ChatGPT JWT，也不写占位 API key。

普通接管只通过 `config.toml` 中的 `ai-gateway` provider 路由模型请求，不会
替 Codex 登录或退出。Remote Control 等依赖 ChatGPT 账号的功能仍要求用户在
Codex 中完成官方登录；模型服务的 API key 不能代替这个账号状态。

## 配置注入

`mochiport configure-codex-app` 会写入：

- `chatgpt_base_url = "http://127.0.0.1:3847/backend-api"`，用于本地 backend fallback 接口。
- 默认 `ai-gateway` provider，地址是 `http://127.0.0.1:3847/ai-gateway/v1`，使用 `requires_openai_auth = true` 和 `supports_standalone_web_search = true`。这样 Codex App 保留账号态（包括 Fast 模式），同时注册原生 `web.run`。
- 如果本地存在 cached curated catalog，则写入本地 `openai-curated` marketplace。
- 固定写入 `features.apps = false`，因为本地没有实现由官方托管的 Apps/Connectors MCP 后端。
- 清理历史插件阻断项，例如 `plugins = false`、`computer_use = false`。
- 清理旧版 CodexHub 生成的 bundled remote plugin 状态。

旧版 Actor Authorization（`requires_openai_auth = false` 加
`x-openai-actor-authorization`）只作为迁移、卸载和清理时的兼容形态识别；当前配置不会再写入该 header。

默认本地 provider 不使用 `experimental_bearer_token`，普通接管也不依赖全局
`CODEX_API_BASE_URL` 环境变量覆盖。

MochiPort 不通过 remote `list` 或 `installed` fallback 发布 `openai-bundled` 插件。包括 `computer-use` 在内的 bundled 插件必须来自 Codex App 自己的本地 `openai-bundled` marketplace。

## 历史兼容

旧版 MochiPort/CodexHub 曾写入 synthetic `chatgptAuthTokens`；某个未发布的
中间版本还写过不带 `auth_mode` 的
`OPENAI_API_KEY = "codexhub-dummy-key"`。这些形态只作为历史 MochiPort-managed
auth 识别。当前配置确实使用本地 `ai-gateway` 时，daemon 启动迁移会优先恢复
已备份的官方 `auth.json`；没有备份时删除 synthetic 占位认证，让 Codex 重新走
正常登录流程。第三方直连 provider 和无关 auth 文件不会被修改。

本地 `/backend-api/ps/plugins/*` fallback 继续保持窄范围：

- 服务 cached `openai-curated` remote catalog/detail。
- 对已经卡在 UI/cache 里的旧 bundled remote ID 提供只读 detail/skill fallback。
- 不允许把 bundled 插件重新放回 remote list/installed 响应。
