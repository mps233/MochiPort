# Codex Remote v0.2.15

这是一个 AI Gateway 正式功能版本。Codex App 仍然使用原生 Responses 入口，`codex-remote` 在本地负责模型渠道管理、协议转换、请求日志和 Codex 配置写入，让用户可以在 GUI 里接入 OpenAI、DeepSeek、Anthropic/Claude、智谱 GLM 等模型渠道。

有问题可以提 GitHub issue，也可以关注 README 里的公众号后直接发消息给我。

## 重点更新

- AI Gateway 从预览能力升级为正式入口：Codex 请求进入本地 `/ai-gateway/v1/responses`，由 `codex-remote` 选择上游渠道并返回 Codex 可消费的 Responses 结果。
- 新增 Anthropic Messages 协议接入，支持 Claude / Anthropic-compatible 模型的文本、图片、工具调用、思考输出、web search 和 apply_patch 工具转换。
- 新增智谱 GLM Anthropic-compatible profile，兼容 GLM 的 web search 返回差异，避免上游私有搜索字段泄漏到 Codex 应用层。
- DeepSeek / Chat Completions 链路补齐 apply_patch 工具支持，不再简单过滤 Codex 的 apply_patch 能力。
- 新增模型映射能力，解决上游模型名大小写、别名或第三方转发命名不一致的问题，例如把 Codex 侧 `glm-5.2` 映射到上游 `GLM-5.2`。
- 新增 Codex 可见模型管理，用户可以在 GUI 中选择 Codex App 模型列表要展示的模型。
- 新增请求日志增强：记录 Codex 原始请求、发给上游的请求、响应或错误、token、cache、cost、TTFT、latency 和上游请求包大小。
- 请求日志详情优化 JSON 查看和搜索体验，方便排查协议转换、超时、首帧慢和上游错误。
- 新增“过滤生图工具”开关。默认不过滤；打开后 AI Gateway 会从 Codex 请求中移除 `image_generation` 工具，适合不支持生图工具的渠道。
- 新增 Codex 会话管理入口。切换 provider 或接入 AI Gateway 后，可以把旧会话移动到当前入口，让 Codex App 左侧继续看到历史会话。
- Codex 本地配置写入和恢复流程优化：第一次使用时只展示写入入口，写入后才展示“恢复 Codex 原有配置”。
- ChatGPT 登录形态的本地 Codex auth 兼容优化，减少接入 `codex-remote` 后账号身份显示混淆。
- README 更新为 GUI 产品说明，不再引导普通用户手写配置文件。

## 使用提示

- 只使用 AI Gateway 时，不需要接入飞书、微信或 Telegram；IM 接入只在需要远程控制 Codex 时使用。
- Codex App / VS Code 插件常规流程是：下载程序 -> 配置 AI Gateway -> 写入 Codex 配置 -> 重启 Codex。
- Codex CLI 仍需要单独启动 `codex app-server --remote-control` 后再连接本地 TUI。
- 如果 Codex App 模型列表没有立刻刷新，重启 Codex App 通常可以解决。
- 请求日志保存在本地应用数据目录，升级后旧版 exe 目录下的日志数据库会尽量自动迁移。

## 验证

- `cargo fmt`
- `cargo test`
- `cargo build --release --features gui --bin codex-remote`
