CodexHub v0.4.3

本版本修复 Codex `view_image` 等工具返回图片后，经 Anthropic Messages 兼容上游转发时，base64 图片偶发被当作普通文本估算 token 的问题。

## Anthropic 工具图片兼容

- 将 Responses `function_call_output` 中的 `input_image` 提升为同一条 Anthropic `role=user` 消息中的独立 `image` block，避免部分 Anthropic-to-Responses 兼容节点把嵌套 base64 当作文本。
- `tool_result` 继续保留文本内容和 `tool_use_id`，不破坏工具调用与结果的配对关系。
- 纯图片工具结果使用简短占位文本，图片仍以标准 Anthropic base64 source 发送。
- 并行工具调用保持所有 `tool_result` 在前、所有图片在后，避免生成不合法的连续工具消息。
- Anthropic 与 GLM Anthropic 兼容渠道统一应用该处理；普通文本工具结果不受影响。
- 保留尾部 prompt cache breakpoint，图片提升不会关闭 Anthropic 缓存。

## 验证

- 使用真实 LLMX 上游和日志中的 1600 x 1600 PNG 完成嵌套结构与提升结构 A/B 验证。
- 新增 Codex 当前 `view_image` 原始 JSON、混合文本图片、纯图片、GLM 和并行图片工具结果测试。
- 全量测试通过。

## Codex App Provider 记录

- 补充 Codex App 自定义 Provider 的两种配置路径、已经解决的能力和仍受前端限制的模型显示问题，便于后续跟进 Codex 更新。
