# CodexHub v0.3.13

## 改进内容

- 大幅降低后台 daemon 的 CPU 占用。AI 网关请求日志的列表查询此前会读取排在大字段之后的 `upstream_request_body_bytes` 列，而每行通常携带数百 KB 的请求/响应 JSON，导致 SQLite 必须逐行遍历溢出页，单次查询约 170ms；仪表盘又每 1.5 秒轮询一次，使 daemon 长期占用约一个核心的 12%。本版为列表查询新增覆盖索引（`CREATE INDEX IF NOT EXISTS`，旧库下次启动时自动补建），查询从约 170ms 降到不足 1ms，daemon 空闲 CPU 回落到接近 0。
- 修复请求日志「打开详情卡顿」的回退问题。上一版将 GUI 改为事件驱动后，请求日志的列表、详情与清理结果渲染被遗漏接入空闲事件，只剩 1.5 秒的兜底定时器在刷新，双击日志后详情最长要等约 1.5 秒才出现。本版让这些后台加载完成时立即唤醒空闲事件并即时渲染，恢复秒开手感，定时器仅作兜底。

## 已知问题

- 在 CodexHub 模式下，Codex App 插件页点击 `computer-use` 进入详情页可能显示「未找到插件」。这是 Codex App 前端对 bundled 本地插件详情的展示行为，`computer-use` 功能本身可正常使用，不影响实际调用。

## 验证

- `cargo check --features gui --bin codexhub`
- `cargo test --bin codexhub request_log`（13 项通过）
- 在真实 819MB 请求日志库上实测：列表查询经覆盖索引由约 55ms（Python）/170ms（daemon）降到 0.4ms，执行计划切换为 `USING COVERING INDEX`。

---

有问题可以提 GitHub issue，也可以关注 README 里的公众号后直接发消息给我。
