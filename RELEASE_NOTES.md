# Codex Remote v0.2.9

本次版本重点加固 Codex App remote-control 连接维护，降低 Computer Use 等高频工具输出场景下 remote-control logical client 丢失后收不到后续消息的概率。

## 更新内容

- 对齐官方 remote-control `pong status=unknown` 语义：保持同一 `client_id` / `stream_id` 重新 `initialize`。
- `unknown` 恢复完成后自动尝试 `thread/resume`，帮助当前 thread 重新加入 app-server listener。
- 新 WebSocket 会话会重新初始化已知 remote clients，并清理对应 ACK cursor，避免重连后 server `seq_id` 从 1 开始时被误判为重复消息。
- 调整 transport ACK 路径，先接管 server envelope 并快速 ACK，再异步分发 IM 侧处理，降低下游渲染/发送对 remote-control 的反压影响。
- 增加 remote-control 时间戳、ACK、`outputDelta` 压力、`unknown` context 诊断日志，方便继续定位 app-server 断开和 backpressure 问题。

## 说明

- 本版本不声称修复 Codex App / Computer Use native pipe 自身问题。
- 如果 app-server 或 Computer Use 运行时自身失败，仍可能看到工具调用失败；但 remote-control 连接维护比 v0.2.8 更稳。
- `cursor` 目前仍是记录和透传基础能力，没有实现完整 backend command replay。

## 验证

- `cargo fmt`
- `cargo test --features gui remote_control_backend::tests`
- `cargo build --release --features gui --bin codex-remote`
