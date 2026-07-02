# wxDragon 同步与资源占用优化执行计划

本文档用于指导两件事的落地顺序:

1. 先同步 `wxDragon` 最新代码。
2. 再基于证据解决 CodexHub GUI 的 CPU 与内存占用问题。

## 背景

当前 CodexHub GUI 使用 `wxdragon`，`Cargo.toml` 中声明版本为 `0.9.16`，但实际构建通过 `[patch.crates-io]` 指向仓库内的 `vendor/wxdragon`:

```toml
[patch.crates-io]
wxdragon = { path = "vendor/wxdragon/rust/wxdragon" }
wxdragon-sys = { path = "vendor/wxdragon/rust/wxdragon-sys" }
wxdragon-macros = { path = "vendor/wxdragon/rust/wxdragon-macros" }
```

因此，同步 wxDragon 时必须更新 vendor 目录，而不是只改 crates.io 版本号。

用户反馈的资源占用问题已经观察到一个明确来源: GUI 空闲时仍然高频刷新 dashboard 与请求日志列表，带来 HTTP 请求、SQLite 查询、内存分配和 chain log I/O。同步 wxDragon 可以提供更好的事件驱动能力和构建修复，但它不是资源占用问题的唯一根因。

## 当前工作区注意事项

当前已有一个未提交的 GUI 轮询降频改动:

```text
M src/gui.rs
```

在同步 wxDragon 前，必须先隔离该改动，避免 vendor 大改和业务优化混在一起。

推荐顺序:

1. 单独提交当前 `src/gui.rs` 资源占用初步修复；或
2. 临时 `git stash push -- src/gui.rs`，等 wxDragon 同步完成后再恢复。

默认推荐单独提交，因为它已经通过:

```powershell
cargo fmt --check
cargo test
git diff --check
```

## 阶段一: 同步 wxDragon

### 目标

将 CodexHub vendor 内的 wxDragon 更新到当前上游最新代码，并确认 GUI 仍可构建、运行、基础交互正常。

### 重点关注的 wxDragon 更新

新功能:

- `wxWakeUpIdle` 包装: 支持后台线程完成后唤醒 UI idle 事件。
- DataView `select` / `unselect` / `ensure_visible`: 有利于表格刷新后恢复选择和定位。
- VirtualList text callback: 后续可用于减少大型列表的数据复制。
- WebView custom URI handler: 当前不是 CodexHub 资源占用问题的关键路径。

构建与平台:

- `wxdragon-sys` reqwest TLS 从 `rustls` 切换到 `native-tls`。
- 动态检测 Visual Studio generator。
- Windows ARM64 workflow 覆盖。
- ARM64 MSVC library search path 修复。
- Linux iconv link path 修复。

### 操作步骤

1. 确认上游最新提交。

```powershell
git -C D:\rust_demo\wxDragon fetch --all --tags
git -C D:\rust_demo\wxDragon status --short
git -C D:\rust_demo\wxDragon log -1 --oneline
```

如果本地 `D:\rust_demo\wxDragon` 不是最新:

```powershell
git -C D:\rust_demo\wxDragon pull --ff-only
```

2. 记录同步前 vendor 状态。

```powershell
git status --short
git diff --stat -- vendor/wxdragon
```

3. 同步 vendor。

同步范围至少包括:

```text
vendor/wxdragon/Cargo.toml
vendor/wxdragon/Cargo.lock
vendor/wxdragon/rust/wxdragon
vendor/wxdragon/rust/wxdragon-sys
vendor/wxdragon/rust/wxdragon-macros
```

如果 wxDragon 上游还有 native 绑定、C/C++ shim、构建脚本或资源文件变更，也需要一起同步。

4. 更新锁文件。

因为 CodexHub 使用 path patch，通常运行:

```powershell
cargo update -p wxdragon -p wxdragon-sys -p wxdragon-macros
```

如果 Cargo 不接受多个 `-p`，分开运行。

5. 编译与测试。

```powershell
cargo fmt --check
cargo test
cargo build --release --features gui
```

如 release exe 正在运行导致 Windows 文件锁冲突，先关闭当前 CodexHub GUI/daemon，或只跑 debug 编译确认 API，再由用户手动 build release。

6. GUI 冒烟验证。

必须覆盖:

- 主窗口启动。
- tab 切换。
- Codex App tab 的初始化、清理、fast startup checkbox。
- AI Gateway provider 表格显示与开关。
- 请求日志 tab 切入、刷新、双击详情。
- 窗口 resize，无明显重影。
- 更新检查弹窗。
- Windows 托盘菜单。

7. 单独提交。

建议提交信息:

```text
chore: sync wxDragon vendor
```

## 阶段二: CPU 与内存占用优化

### 目标

减少 GUI 和 daemon 在空闲、长时间运行、请求日志打开、会话运行中的 CPU、内存和磁盘日志 I/O。

### 基线采集

每次优化前后都用相同场景采集指标。

场景:

1. 空闲 5 分钟。
2. 打开请求日志 tab 5 分钟。
3. Codex 会话运行中。
4. 长时间运行 30-60 分钟。

采集指标:

- `codexhub.exe` GUI 和 daemon 的 CPU 增量。
- Working Set。
- Private Memory。
- Thread count。
- Handle count。
- `codexhub-chain.log` 增长速度。
- 最近 1000 行 HTTP path 分布。
- `/ai-gateway/request-logs` 查询耗时。

示例命令:

```powershell
$p1 = @(Get-Process -Name codexhub -ErrorAction SilentlyContinue | Select-Object Id,ProcessName,CPU,WorkingSet64,PrivateMemorySize64,Threads,HandleCount,Path)
Start-Sleep -Seconds 60
$p2 = @(Get-Process -Name codexhub -ErrorAction SilentlyContinue | Select-Object Id,ProcessName,CPU,WorkingSet64,PrivateMemorySize64,Threads,HandleCount,Path)
foreach ($p in $p2) {
  $old = $p1 | Where-Object { $_.Id -eq $p.Id } | Select-Object -First 1
  $delta = if ($old) { $p.CPU - $old.CPU } else { 0 }
  [pscustomobject]@{
    ProcessName = $p.ProcessName
    Id = $p.Id
    CpuDeltaSec = [math]::Round($delta, 3)
    CpuPctOneCore = [math]::Round(($delta / 60 * 100), 1)
    WorkingSetMB = [math]::Round($p.WorkingSet64 / 1MB, 1)
    PrivateMB = [math]::Round($p.PrivateMemorySize64 / 1MB, 1)
    Threads = $p.Threads.Count
    Handles = $p.HandleCount
    Path = $p.Path
  }
}
```

HTTP path 分布:

```powershell
$log = 'D:\rust_demo\codex-remote\target\release\logs\codexhub-chain.log'
$lines = Get-Content -Path $log -Tail 1000
$paths = $lines | ForEach-Object { if ($_ -match 'path=([^ ]+)') { $matches[1] } }
$paths | Group-Object | Sort-Object Count -Descending | Select-Object -First 20 Count,Name
```

### 优化优先级

#### P0: 去掉不必要的常驻刷新

已确认的热点:

- dashboard timer 固定刷新多个接口。
- 请求日志 tab 未打开时仍刷新日志列表。

落地方向:

- 用户操作后强制刷新。
- tab 切换后按需刷新。
- 空闲兜底刷新降频。
- 请求日志只在 tab 激活时自动刷新。

#### P1: 拆轻量 dashboard API

当前 `ApiClient::dashboard()` 会拉:

- `/api/status`
- `/api/remote-control/status`
- `/api/codex-app/status`
- `/api/im/accounts`
- `/api/config`

其中 `/api/config` 返回完整配置，包含大量 provider/model 配置。dashboard 多数时候只需要少量字段。

落地方向:

- 新增轻量接口，例如 `/api/gui/dashboard`。
- 或将 `/api/config` 替换为只返回 AI Gateway GUI 所需状态的接口。
- 配置保存、provider 开关等用户动作后再强制刷新完整配置。

#### P2: 引入后端事件流

目前 GUI 只能主动 polling daemon 状态。更彻底的方式是让 daemon push 状态变化。

候选方案:

- SSE: `/api/events/stream`
- 本地 WebSocket
- long-poll `/api/events/wait?cursor=...`

优先考虑 SSE 或 long-poll，避免引入过多 GUI 线程复杂度。

事件类型:

- `service_started`
- `config_changed`
- `remote_control_changed`
- `im_account_changed`
- `request_log_changed`
- `codex_app_status_changed`

GUI 收到事件后只刷新对应区域。

#### P3: 请求日志列表内存优化

现状:

- 请求日志列表最多拉取 200 条。
- DataView model 使用内存 rows。

后续可以结合 wxDragon 新的 VirtualList text callback:

- 不再为每个 cell 长期保存所有格式化字符串。
- 只保存原始 entry 或 id，cell 显示时按需格式化。
- 请求日志详情按需加载，不进入列表内存。

#### P4: 日志 I/O 控制

release 默认 `logging.diagnostic=false`，但用户开启 diagnostic 时，高频 GUI 请求会显著放大日志 I/O。

落地方向:

- 先减少请求源头。
- 必要时对成功的 GUI polling GET 做低价值日志降噪。
- error/warn/timeout 保持强制记录。

### 验收标准

空闲 5 分钟:

- GUI + daemon `codexhub.exe` CPU 接近 0。
- 单进程长期低于 1% 单核。
- `codexhub-chain.log` 不再持续刷 GUI 状态接口。
- Working Set 和 Private Memory 稳定，无持续线性增长。

请求日志 tab 打开:

- 切入后立即加载。
- 停留时低频刷新。
- 双击详情仍然快速。
- 切出后停止自动拉 `/ai-gateway/request-logs`。

会话运行中:

- 流式输出不受影响。
- request log TTFT、latency、cache 数据正常。
- remote control 状态不丢。

构建:

- `cargo fmt --check` 通过。
- `cargo test` 通过。
- Windows release GUI build 通过。

## 提交拆分

建议至少拆成两个提交:

1. `chore: sync wxDragon vendor`
2. `fix: reduce GUI idle CPU and memory usage`

如果引入事件流，建议单独提交:

3. `feat: add GUI event stream for daemon status updates`

这样 release note 可清晰说明:

- GUI 框架同步。
- 空闲资源占用下降。
- 请求日志页刷新策略改变。
- 后续事件驱动基础能力。

## 回滚策略

wxDragon 同步失败:

- 回滚 vendor 同步提交。
- 保留业务侧 CPU/内存优化提交。

CPU/内存优化出现 UI 状态延迟:

- 临时把 dashboard 兜底刷新间隔调低。
- 保留请求日志 tab 激活刷新策略。

事件流不稳定:

- 保留低频 timer 作为 fallback。
- 事件流只作为加速刷新，不作为唯一状态来源。
