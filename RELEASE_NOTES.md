# MochiPort v0.5.5

本版本重点完善 Codex 会话与 Telegram Topic 的联动，并更新 macOS 客户端体验。

## Telegram Topic

- 在 Codex App 创建新会话时，自动按项目目录匹配项目群并创建同名 Topic。
- 自动绑定新 Topic 与 Codex 会话，支持多个 Codex 连接之间的来源隔离。
- 避免重复创建和孤儿 Topic，桥接重启或来源连接失效时不会误绑定到其他项目。
- Telegram 多张图片会作为相册发送，减少同一轮消息的刷屏。

## macOS 客户端

- 调整 macOS 浅色主题和 mascot，使主界面在浅色外观下更清晰统一。
- 更新正式 App 与内置 daemon 版本为 `0.5.5 (build 461)`。

## 验证

- 已通过 Rust 格式检查、锁定依赖测试和差异检查。
- 已完成 macOS arm64/x86_64 daemon 构建、通用二进制合并和 Release App 组装验证。
