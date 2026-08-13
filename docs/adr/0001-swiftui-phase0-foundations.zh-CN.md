# ADR 0001：SwiftUI Phase 0 工程与安全边界

- 状态：已接受
- 日期：2026-08-13
- 范围：ThreadRelay macOS SwiftUI 预览版与现有 Rust daemon

## 背景

ThreadRelay 正在从 macOS wxDragon 前端渐进迁移到 SwiftUI。迁移期间，stable、SwiftUI preview 和最后一个桥接版可能同时连接同一用户数据域与同一个 daemon。任何前端生命周期动作都不能误停正在承载 Codex 或消息通道工作的 daemon。

## 决策

### App Sandbox

首个 SwiftUI 正式版暂不启用 App Sandbox。应用需要启动并校验随包 Rust helper、兼容读取既有配置和日志目录、连接 loopback 服务，并与外部 Codex 应用协同。正式发布仍要求 Hardened Runtime、Developer ID 签名和公证。Sandbox 迁移作为独立安全项目评估，不在页面实现中临时开启。

### Universal 构建

macOS App 与 Rust helper 均继续提供 `arm64 + x86_64`。CI 和打包流程必须校验两种架构，不能用 Rosetta 构建结果替代 Intel 原生验证。

### 凭据与旧数据兼容

Rust daemon 是配置和凭据的唯一事实来源。SwiftUI 不复制业务配置、不直接读取 Provider Key 或 Bot Token，也不迁移旧数据目录。新密钥未来只通过窄写接口提交，读接口只返回是否已设置。既有 `Application Support/CodexHub` 兼容读取继续由 Rust 负责。

### 管理 API 鉴权

`/healthz` 是唯一匿名管理面端点，并严格只返回 `service`、`apiMajor`、`ready`。`/api/v1/manage/*` 使用当前用户数据域共享的 bearer credential；该凭据保存在配置目录旁的 `threadrelay-control.json`，不得进入日志、诊断包、崩溃报告、命令行或 App Bundle。Codex/AI Gateway 协议不复用管理凭据。

### Bundle 与版本身份

- stable Bundle ID：`io.github.mps233.threadrelay`
- SwiftUI preview Bundle ID：`io.github.mps233.threadrelay.preview`
- 产品语义版本：根目录 `Cargo.toml` 的 package version
- 构建号：显式 `THREADRELAY_BUILD_NUMBER`
- 最低系统：macOS 13
- stable 与 preview 使用同一签名 Team ID；Team ID 在首次正式签名前由发布环境提供并冻结

旧 `com.codexhub.app` 不能直接覆盖安装为 SwiftUI stable。它先进入最后一个 ThreadRelay wxDragon 桥接版，或与 SwiftUI preview 并行安装。

### Liquid Glass 边界

macOS 26 只在真实导航、控制或浮层重叠处使用系统 `glassEffect`；macOS 13-15 使用系统 `Material` 回退。业务列表、表格、表单和日志内容使用稳定系统表面，不实现自制 blur、shader 或截图材质。

## 控制平面协议

### 共享控制文件

控制文件属于用户数据域，而不是某个 App 安装。它最终承载：

- 管理凭据及其 generation
- 当前 daemon 的 instance ID、PID、启动时间、runtime 路径与哈希
- 当前管理租约的 installation ID、租约 generation 与心跳期限
- 已校验 runtime 列表和 staged candidate
- 最后一次切换/恢复结果

Phase 0 已实现最小凭据字段；新增字段必须向后兼容，旧客户端忽略未知字段。

### 凭据发现与轮换

stable、preview 和桥接版从同一配置目录发现控制文件。凭据只在首次初始化、可信管理接管或明确的泄漏恢复中轮换。轮换采用文件锁、临时文件、`fsync` 和原子替换；成功后 generation 递增。不能因 GUI 普通启动或崩溃自动轮换。

### 唯一管理租约

同一用户数据域任一时刻只有一个安装持有管理租约。只有租约持有者可轮换凭据、staging helper、排空、切换、重启或停止 daemon。读取状态与经过 revision 保护的业务配置写入不要求租约。

租约获取必须校验 daemon PID、instance ID、可执行路径、runtime 哈希和端口归属。租约仍存活时，另一安装只能只读，除非用户明确确认接管。租约过期不等于立即可杀进程；候选安装必须重新完成身份校验后才能接管。

### 多 GUI 并发写

所有业务写请求携带共享凭据、daemon instance ID、资源 revision 和幂等请求 ID。revision 不匹配返回冲突，由 GUI 刷新后重新应用用户意图；禁止回传整份旧配置覆盖未知字段。破坏性操作还要求显式用户确认。

### 崩溃恢复与切换

GUI 关闭或崩溃不停止 daemon。daemon 不依赖 GUI 父进程、管道或临时目录存活。切换 helper 前必须确认受保护工作项已排空；候选 runtime 未在健康窗口内就绪时恢复上一 runtime。遇到身份冲突只提供诊断，不停止未知进程。

## 结果

SwiftUI preview 可以在不接管 daemon 生命周期的前提下独立构建和只读探测。Phase 2 实现完整租约与切换状态机时必须遵循本 ADR；若要改变鉴权、数据域、Bundle 身份或生命周期边界，需要新 ADR。
