# CodexHub 待办事项

## Anthropic 工具结果图片兼容模式

状态：待处理，当前未实现。

### 问题现象

- 请求日志 `9753` 使用 Anthropic Messages、Opus-4.8 和 `ai.llmx.cloud`。
- CodexHub 正确把 Codex 工具输出图片转换为 Claude Code/Anthropic 原生结构：
  `tool_result.content[].image.source.data`。
- 图片为 1600 x 1600 PNG，base64 长度为 5,261,816 字符。
- 上游返回未缓存输入 1,315,798 token，几乎等于 base64 长度除以 4，说明兼容网关把嵌套图片按文本计算或处理。
- 日志 `9879` 使用相同渠道、模型和 Claude Code headers，但图片位于普通 `user.content[].image`，约 4.8 MB 的 base64 只产生 18,720 总输入 token。
- 旧日志 `3879` 已被日志保留策略清理，当前无法重新读取原始请求。

### 已确认结论

- Claude Code 的 Read、MCP、浏览器截图等工具可以在 `tool_result.content` 中返回 image block。
- 当前 CodexHub 的嵌套结构符合 Claude Code/Anthropic Messages 语义，不应全局修改。
- `9753` 与 `9879` 的 Claude Code headers 一致，问题不是缺少 `anthropic-beta`、`x-app`、`User-Agent` 或 session header。
- 问题更可能发生在 Anthropic -> Responses 或其他内部协议的兼容网关转换中。
- `references/sub2api-main/backend/internal/pkg/apicompat/anthropic_to_responses.go` 已实现正确的桥接方式：从 tool result 提取图片，将文本保留为 function output，并把图片作为独立 user image 发送。

### 建议实现

增加 Provider 级兼容模式，不按域名硬编码：

```toml
compatibility = "anthropic_responses_bridge"
```

行为：

1. 默认 `anthropic` profile 保持原生 `tool_result.content[].image`。
2. `anthropic_responses_bridge` profile 将 tool result 中的图片提升为同一 user message 的并列 image block。
3. tool result 中的文本继续留在原 `tool_result`。
4. image-only tool result 写入简短占位文本，例如 `Image output attached below.`。
5. 保持 `tool_use_id`、消息顺序、图片顺序和工具调用配对不变。
6. 多个并行 tool result 必须继续合并为合法的单条 user message，不能产生不合法的连续 user/tool 消息。
7. 普通文本 tool result、官方 Anthropic 和正确支持 Claude Code 的渠道不受影响。
8. 记录本次提升的图片数量，方便日志排查。

### 测试清单

- image-only `function_call_output`。
- 文本和图片混合的 `function_call_output`。
- 多张图片。
- 多个并行 tool call/tool result。
- custom tool call output。
- 普通文本 output 保持原样。
- 默认 Anthropic profile 保持嵌套图片。
- 兼容 profile 输出并列 user image。
- cache-control 不得破坏图片结构或 tool result 配对。
- 使用与日志 `9753` 等价的 1600 x 1600 图片请求对 LLMX 做 A/B 验证。

### 相关代码

- `src/ai_gateway/providers/anthropic_messages/request_content.rs`
- `src/ai_gateway/providers/anthropic_messages/options.rs`
- `src/ai_gateway/providers/anthropic_messages/request.rs`
- `src/ai_gateway/providers/anthropic_messages/tests.rs`
- `references/sub2api-main/backend/internal/pkg/apicompat/anthropic_to_responses.go`
- `references/sub2api-main/backend/internal/pkg/apicompat/anthropic_responses_test.go`
