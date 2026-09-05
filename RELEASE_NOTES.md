# MochiPort v0.5.6

本版本完成 MochiPort 身份与遗留路径清理，并为账号池带来上游余额、站点名称与调度管理能力；同时将 CI 构建链路迁移至 Xcode 27。

## 核心改动

- 统一 daemon、AI Gateway、bridge、配置和本地存储的 MochiPort 身份；旧 ThreadRelay/CodexHub 数据仅通过显式 `mochiport migrate-storage` 迁移，或以兼容读取方式处理。
- 移除 GUI supervisor、安全重启、自动 runtime 切换、回滚与恢复流程。正常启动不再因为版本不一致替换运行中的 daemon。
- 保留 Codex 官方认证，不再写入伪造 JWT 或占位 API Key；remote-control 状态改为按连接隔离。
- 将旧版 IM 单账号状态迁移为账号数组，并规范化和持久化账号 ID。
- Windows 会话改为读取 daemon 的权威快照，不再在本地执行 provider 转移或模拟回写。

## 账号池与 AI 网关

- 新增上游余额与倍率探测：经 Sub2API 官方管理员备份导出读取账号凭据（仅内存使用，不落盘），按站点直探余额；Sub2API 系站点读取使用快照，One API 系站点按额度与用量推算。
- 新增站点自报名展示：从各站点公开接口读取运营者自定义的站点名称，并在探测到模板默认名时自动回退域名显示。
- 新增账号调度开关：可在账号池页面逐个或按站点批量切换账号是否参与调度；变更采用乐观更新，失败自动回滚。
- 额度 Dock 在上游余额不可用时回退展示 Provider 自身的订阅与用量状态。

## 账号池界面

- 页面布局改版：迷你统计卡、玻璃卡面与统一的状态色语义（红色仅表示错误或耗尽）。
- 统计卡支持点击筛选（全部/可用/异常），行内一键打开站点面板，组展开状态跨会话保留。
- 多账号站点合并为可展开分组，展开采用平滑手风琴动画。

## 客户端与发布

- macOS、Windows 客户端与内置 daemon 版本号为 `0.5.6`。
- 账号调度开关在 macOS 与 Windows 客户端均可用。
- CI 的 SwiftUI 构建与 macOS 发布流程迁移至 Xcode 27 镜像，修复旧版 actool 编译 AppIcon 崩溃。

## 验证

- Rust 全量测试、格式检查与全目标检查通过（1074 项）。
- macOS Swift 测试通过（169 项）。
- Windows TypeScript 类型检查与 Playwright 账号池一致性测试通过。
