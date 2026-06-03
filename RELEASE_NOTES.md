# Codex Remote v0.2.8

本次版本重点修复 Telegram 机器人配置体验，并加固 Telegram 私聊入口安全。

## 更新内容

- 修复 Telegram Bot Token 录入窗口按钮不可见的问题。
- Telegram Token 输入框支持回车确认，保存按钮改为默认按钮。
- 加固 Telegram 私聊入口：`allowedChatIds = []` 改为首次私聊自动绑定，后续其它私聊会被拒绝。
- Telegram 群聊仍保持不接入，避免群成员通过 bot 操控宿主本机。
- 更新 README 和配置文档，明确 Telegram `allowedChatIds` 的安全语义。
- 优化日志清理和诊断日志开关，降低大日志文件对 IM 消息链路的影响。
- 优化 remote-control 协议、ACK 和多 IM 会话稳定性。
- 优化 MCP/图片类消息在 Telegram 和微信里的呈现。
- 改进微信 context token 恢复和消息补发逻辑。

## 说明

- Telegram 新用户首次私聊 bot 后，会自动把该私聊 chat id 写入 `allowedChatIds`。
- 如果需要更严格的部署，可以提前手写 `allowedChatIds = ["你的 chat id"]`。
- 已经配置过 `allowedChatIds` 的用户不会被自动覆盖。

## 验证

- `cargo test --features gui im::telegram::polling::tests`
- `cargo test --features gui config::tests`
- `cargo build --release --features gui --bin codex-remote`
