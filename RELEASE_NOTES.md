# CodexHub v0.3.4

## 改进内容

- 对齐 Anthropic / GLM Anthropic-compatible 出站请求到 Claude Code 请求形态：托管 `Authorization: Bearer`、Claude Code beta、Stainless、`x-app`、`user-agent` 等 header，并固定 Anthropic 上游请求为 HTTP/1.1。
- 修正 Anthropic Messages 请求体映射：工具 `input_schema` 补齐 JSON Schema draft、`properties` 和 `additionalProperties`，web search 保持 Anthropic server tool 形态。
- 将 Anthropic prompt caching 改为适配层生成的 block-level `cache_control`，只标记 `system` text block 和最近的 assistant text block；不再生成顶层 cache、不生成 `ttl`，也不要求 Codex 入参携带 Anthropic 专属参数。
- 改进 AI Gateway provider UI 与 header 处理，减少无关入站 header 对上游请求的影响。
- 停止在配置 Codex App GUI 时写入额外 API 环境变量，避免污染 Codex 客户端运行环境。
- 改进上游 Responses / SSE 请求日志捕获，方便排查第三方渠道问题。
- 统一部分中文界面文案，保留 `AI Gateway` 命名以降低和“大模型接入”表述混用带来的歧义。

## 验证

- `cargo fmt`
- `cargo test --features gui --bin codexhub anthropic`
- `cargo build --release --features gui --bin codexhub`

---

有问题可以提 GitHub issue，也可以关注 README 里的公众号后直接发消息给我。
