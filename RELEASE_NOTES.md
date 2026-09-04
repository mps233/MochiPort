# MochiPort v0.5.6

本版本完成从上游 Fork 遗留兼容路径到 MochiPort 原生实现的清理，减少隐式迁移、自动接管和旧身份回退。

## 核心改动

- 统一 daemon、AI Gateway、bridge、配置和本地存储的 MochiPort 身份；旧 ThreadRelay/CodexHub 数据仅通过显式 `mochiport migrate-storage` 迁移，或以兼容读取方式处理。
- 移除 GUI supervisor、安全重启、自动 runtime 切换、回滚与恢复流程。正常启动不再因为版本不一致替换运行中的 daemon。
- 保留 Codex 官方认证，不再写入伪造 JWT 或占位 API Key；remote-control 状态改为按连接隔离。
- 将 VS Code 兼容补丁限制为显式 CLI 操作，daemon 启停不再自动修改或恢复扩展。
- 将旧版 IM 单账号状态迁移为账号数组，并规范化和持久化账号 ID。
- Windows 会话改为读取 daemon 的权威快照，不再在本地执行 provider 转移或模拟回写。

## 客户端与发布

- macOS 正式 App 与内置 daemon 升级至 `0.5.6`，UI build 为 `486`，daemon build 为 `486`。
- 更新 macOS 发布工作流，移除已废弃 GUI supervisor 的签名步骤。

## 验证

- Rust 全量测试、格式检查、全目标检查和差异检查均已通过。
- Windows 前端构建与 Playwright 测试已通过。
- macOS Swift 测试已通过。
