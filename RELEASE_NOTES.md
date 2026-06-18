# Codex Remote v0.2.14-ai-gateway-preview.1

这是一个 AI Gateway 临时预览版本，用于验证 Codex App 接入本地 AI Gateway、OpenAI Responses 透传、DeepSeek Chat 协议转换和请求日志调试链路。该功能仍在快速迭代中，不作为稳定正式版本推荐给普通用户自动升级。

## 更新内容

- 新增内置 AI Gateway：Codex 侧可配置 `http://127.0.0.1:3847/ai-gateway/v1` 作为 OpenAI-compatible Responses 入口。
- 支持 OpenAI Responses 透传，补齐 prompt cache key，透传 Codex 关键 header。
- 支持 DeepSeek Chat Completions 协议转换，覆盖文本、reasoning、tool calls、tool result 回填和 SSE 转 Responses SSE。
- 新增 AI Gateway 渠道配置 GUI，可添加 OpenAI / DeepSeek 渠道，维护上游模型列表和 Codex 可见模型白名单。
- 新增 `GET /ai-gateway/v1/models` 模型 catalog 返回和 ETag / `x-models-etag` 刷新机制。
- 新增 Codex 配置注入：维护 `model_providers.ai-gateway`、`model_provider = "ai-gateway"` 和 `chatgpt_base_url`。
- 新增 AI Gateway 请求日志 tab，记录 id、model id、stream、channel、status、tokens、cache、cost、TTFT、latency、created at。
- 请求日志详情支持查看 Codex 原始请求、转换后的上游请求、响应和错误，使用 wxDragon `StyledTextCtrl` 展示 JSON。
- 请求日志支持清理 3 天之前日志和清理全部日志。

## 兼容性说明

- AI Gateway 仍是预览功能，Responses 与 Chat Completions 的转换还在补齐边界 case。
- 当前只固定支持 OpenAI Responses 和 DeepSeek Chat Completions；暂不开放自定义协议选择。
- Codex 可见模型由 `aiGateway.codexVisibleModels` 显式白名单控制，不会自动展示上游渠道所有模型。
- Codex App 的模型列表有本地缓存，保存可见模型后通常会在 5 分钟内刷新；必要时退出 Codex App 并删除 `models_cache.json` 后重启验证。
- 请求日志使用本地 SQLite 存储，默认数据库文件为 `ai-gateway-request-logs.sqlite`。

## 验证

- `cargo fmt`
- `cargo fmt --check`
- `cargo test --release --features gui sqlite_delete -- --nocapture`
- `cargo build --release --features gui --bin codex-remote`
