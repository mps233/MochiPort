# 认证说明

这份文档说明 Codex App remote-control 路径里的本地 auth 边界。

## 当前决策

`codex-remote` 使用本地 `chatgptAuthTokens` auth 形态作为 Codex App remote-control 身份。

含义是：

- Codex App 仍然负责 app-server 启动，并读取它正常的 Codex home
- `chatgpt_base_url` 指向本地 `codex-remote` backend
- Codex App 的 `auth.json` 使用 `auth_mode = "chatgptAuthTokens"`
- 第三方模型 key 仍然放在 Codex model provider 配置里
- app-server 连上 remote-control 后，`codex-remote` 再把协议流量桥到飞书

## 为什么不能只用 API Key

官方 Codex remote-control 启动时会在连接 backend 之前拒绝纯 API key auth：

```text
remote control requires ChatGPT authentication; API key auth is not supported
```

所以模型 provider key 不能当 remote-control 身份。它只负责后续模型请求。

## 本地 `chatgptAuthTokens`

本地 auth 记录刻意做成 ChatGPT-shaped，因为这是 Codex remote-control 检查接受的形态：

```json
{
  "auth_mode": "chatgptAuthTokens",
  "OPENAI_API_KEY": null,
  "tokens": {
    "id_token": "<本地 ChatGPT-shaped JWT>",
    "access_token": "<本地 ChatGPT-shaped JWT>",
    "refresh_token": "",
    "account_id": "acct_codex_remote_local"
  },
  "last_refresh": "2026-05-26T00:00:00Z"
}
```

这个 JWT 是本地材料。Codex 读取它的 claims 来拿 account/user 元数据。`codex-remote` 不会用它去请求 OpenAI。

## 辅助命令

使用：

```powershell
codex-remote --config config.toml configure-codex-app
```

它会显式写入：

- Codex App `config.toml`：`chatgpt_base_url = "http://127.0.0.1:3847/backend-api"`
- Codex App `auth.json`：本地 `chatgptAuthTokens` auth

也可以顺手写第三方 provider 配置：

```powershell
codex-remote --config config.toml configure-codex-app --provider-name llmx --provider-base-url https://ai.llmx.cloud --provider-key sk-... --model gpt-5.5
```

如果写 provider 字段但不传 `--provider-name`，helper 默认使用 `llmx`。

这个命令只在直接调用时执行。daemon 启动不会偷偷修改 Codex App 配置或 auth 状态。

## 运行边界

`codex-remote` 读取：

- `config.toml`
- 本地 bridge 状态
- 飞书凭证和 bridge 配置

Codex App 读取：

- Codex App `config.toml`
- Codex App `auth.json`
- model provider 配置和 key

当 Codex App 启动 remote-control 之后，`codex-remote` 真正关心的是：

- app-server 是否连到了 `/backend-api/wham/remote/control/server`
- remote-control 的 `initialize` / `initialized`
- thread / turn 通知
- approval 请求与响应

## 参考材料

仓库里可能会保留一些协议和 auth 结构研究时留下的本地参考材料。它们被 git 忽略，不参与 daemon 启动。
