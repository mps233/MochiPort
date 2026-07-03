# CodexHub v0.3.17

## 修复

- 修复 Anthropic / GLM 流式请求上报的 token 用量被放大约 3 倍的严重问题。内层转换器会在 `response.created`、`response.in_progress`、`response.completed` 三个 envelope 事件里各带一份相同的完整 usage 快照，而外层多轮合并逻辑对每个 envelope 都累加一次，导致单轮请求的 `input_tokens` / `cached_tokens` / `cache_creation_tokens` 被乘以 envelope 事件数量（通常 3 倍）。现在只从每轮的终结事件（`response.completed` / `response.incomplete` / `response.failed`）吸收 usage，单轮计一次，多轮 tool_use 仍按轮正确累加。
  - 直接后果：回传给 Codex 的上下文用量虚高（例如真实 ~82k 被报成 ~247k），叠加为回复预留的 `max_tokens` 后提前撞上模型上下文窗口，触发**过早的上下文压缩（CONTEXT CHECKPOINT COMPACTION）**。修复后压缩时机恢复正常。
  - 请求日志中的 token 统计与成本计算此前同样被放大，现已恢复准确。

## 验证

- `cargo fmt`
- `cargo test`（354 项通过，含新增的单轮 usage 去重回归断言）

---

有问题可以提 GitHub issue，也可以关注 README 里的公众号后直接发消息给我。
