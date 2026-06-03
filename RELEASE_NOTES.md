# Codex Remote v0.2.7

本次版本聚焦聊天工具接入：新增微信和 Telegram 支持，并支持多个机器人分别管理多个 Codex 会话。

## 更新内容

- 新增微信机器人接入，支持扫码接入、会话创建/恢复、消息转发、审批回复和图片发送。
- 新增 Telegram 机器人接入，支持私聊 Bot Token 配置、会话创建/恢复、inline keyboard 操作和审批回复。
- 支持多个飞书、Telegram、微信机器人同时接入；每个机器人/聊天会话可分别管理自己的 Codex thread。
- 对齐 remote-control 多 stream 协议，多个机器人并行控制多个 thread 时互不覆盖绑定关系。
- 聊天工具接入页改为机器人池管理，展示平台、状态、账号和接入开关，并将新增机器人入口放到更醒目的位置。
- 飞书、Telegram、微信在 turn 进行中会拦截普通输入并提示 `/s` 中断或 `/q` 退出，turn 结束时会发送完成标记。
- 优化 remote-control ACK、重连和日志清理，降低长任务、大输出和多机器人并行时的断流风险。
- macOS DMG 增加 Applications 快捷方式，便于拖拽安装。

## 说明

- 多个机器人可以分别管理多个会话；暂不支持多个机器人管理同一个会话。
- Telegram 当前仅支持与机器人私聊，暂不接入群聊。

## 验证

- `cargo test --features gui --bin codex-remote`
- `cargo build --release --features gui --bin codex-remote`
