# Codex Remote v0.2.13

本次版本修复 remote-control 多端路由错乱，并改善连接稳定性和桌面状态展示。

## 更新内容

- 支持 Codex App、VS Code 插件、Codex CLI 多 endpoint 并存时按优先级路由：Codex App > VS Code > CLI。
- 新建或恢复 thread 后，IM 会话会绑定当时选中的 endpoint，后续消息、`/s`、`/q` 不再漂移到其他端。
- remote-control 遇到 `unknown` / stale stream 时增加恢复与重新订阅逻辑，降低断流后收不到后续消息的概率。
- 微信链路增加 context token 失效后的低打扰恢复机制，支持用户发送 `!` 或 `?` 刷新 token 且不转发给 Codex。
- 桌面 GUI 状态面板增加 Codex CLI 状态，并压缩状态展示，只保留关键连接状态。
- 调整状态面板局部刷新逻辑，避免稳定状态下反复闪烁，并修复已连接状态颜色没有及时变绿的问题。

## 兼容性说明

- remote CLI 模式建议显式传 `-C`，否则 Codex thread 元数据可能使用 app-server 启动目录。
- Codex App / VS Code / CLI 的 endpoint 识别依赖官方连接 header 和 User-Agent；如果官方后续调整字段，日志会保留诊断信息。

## 验证

- `cargo fmt`
- `cargo check --features gui --bin codex-remote`
- `cargo test --features gui --bin codex-remote`
- `cargo build --release --features gui --bin codex-remote`
