# CodexHub v0.3.15

## 改进内容

- 降低 GUI 空闲 CPU 占用。减少主窗口、Codex 页面、请求日志页面的固定轮询频率，后台或未激活页面不再持续高频刷新。
- 仪表盘状态改为单接口聚合读取。GUI 现在通过 `/api/gui/dashboard` 一次拿到配置、运行状态、会话和请求统计，替代此前多个接口反复 fan-out 的轮询方式，减少 daemon 和 GUI 两侧的压力。
- 请求日志页面在未打开时停止刷新。只有日志页可见时才周期性拉取列表，切到其它页面后立即停表，打开详情时的卡顿也随之减少。
- 同步 wxDragon 0.9.17，并保留 CodexHub 所需的本地 vendor 覆盖。该版本包含 WebView 自定义 scheme handler、VirtualList 文本回调、macOS 可访问性辅助、`wxWakeUpIdle` 等更新，为后续事件驱动改造打基础。
- Codex App 快速启动兼容性继续收敛。快速启动现在明确作为可选能力启用，启用时才占用 `localhost:8000` 作为启动阶段辅助入口，关闭后不再设置对应环境变量。
- 补齐 Codex App fast startup 所需的关键 Statsig feature gates，避免加速网关让远程控制入口、手机图标等功能被前端误判为关闭。
- 新增 Codex App 快速启动与 Statsig 构造文档，记录当前实现原理、需要保留的 feature gate、以及后续 Codex App 升级时的核对方法。
- 新增 wxDragon 同步与资源占用优化计划文档，明确后续 CPU/内存治理路径。

## 修复

- 修复快速启动后 Codex App 底部远控手机图标消失的问题。
- 修复快速启动设置勾选后没有立即生效的问题。
- 修复部分 GUI 页面在窗口缩放和频繁刷新时更容易出现卡顿的问题。

## 验证

- `cargo fmt`
- `cargo test`（354 项通过）
- `cargo build --release --features gui --target-dir target\gui-verify`
- 本地重启后采样：GUI 30 秒 CPU 增量为 0，daemon 30 秒 CPU 增量约 0.047 秒；GUI 工作集约 46.8 MB，daemon 工作集约 32.0 MB。

---

有问题可以提 GitHub issue，也可以关注 README 里的公众号后直接发消息给我。
