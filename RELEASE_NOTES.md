# MochiPort v0.5.12

本版本把 Codex 本地用量解析收敛到 daemon，给管理 API 补上自动生成的类型契约，
并修掉两个会造成「回答说到一半」与「后台服务切不动」的缺陷。

## 用量解析收敛到 daemon

同一份 Codex 本地用量口径此前在 macOS（Swift 约 3378 行）与 Windows（Rust）
各实现一遍，改一处就得同步两处，必然漂移。现在 **daemon 是唯一实现**，两个
客户端只负责展示；daemon 不可达时客户端仍回落到本地采集，界面不会空掉。

- 新增 `src/usage/`：`log` 解析 rollout 记录、`scan` 遍历 sessions 目录按会话
  累计、`cost` 用内置价目表加用户覆盖计价、`service` 汇总成 API 结构。
- 新增 `GET /api/v1/manage/usage/summary` 与 `/usage/history`，走统一的
  management bearer 鉴权。
- macOS：`UsageStore` 应用 daemon 给出的额度窗口。窗口按记录里的
  `windowMinutes` 分类，而不是按位置——实测记录把 30 天窗口放在 `primary`，
  按位置当成 5 小时窗口会标错；长度对不上任何已知窗口就跳过，不硬塞进最近的
  一种。`DailyStatsStore` 由 daemon 的交叉维度重建历史行，
  `MenubarCoordinator` 低频拉取分钟桶，都不阻塞 HUD。
- Windows：`codex_usage` 改为消费 daemon 的 history 端点。
- 设计说明见 `docs/codex-usage-unification.zh-CN.md`。

## 管理 API 契约自动生成

两端原先各自手写管理 API 的类型，daemon 改了字段，客户端要到运行或编译失败
才发现。现在 daemon 用 `schemars` 反射出一份 JSON Schema 作为唯一事实来源，
再由脚本生成 Swift 与 TypeScript 客户端。

- `src/web/contracts.rs` 把两端消费的全部请求/响应形状列在一个目录类型里；
  它不在运行时构造，只为导出稳定的 schema。
- `cargo test --lib write_management_contract_schema` 写出
  `contracts/manage-contracts.schema.json`，`scripts/generate-client-contracts.py`
  由它生成 `macos/.../Generated/ManageContracts.swift` 与
  `windows/MochiPort/src/api/generated/manageContracts.ts`。
- **加一个契约类型只需在目录里列一行**，不必再手工改两端。
- CI 增加两步：导出 schema 后以 `--check` 校验生成物是否为最新，防止改了
  daemon 却忘了重新生成。

## 修复：上游流断在半路不再谎报完成

chat-completions 的契约是先来一个带 `finish_reason` 的分片、再来 `[DONE]`。
上游崩溃、代理空闲超时或网关重启时两者都没有，body 直接断在半个回答上；此前
这种情况发出 `response.completed`，客户端把截断的文本当成完整答复，turn 就此
结束且不会重试。

现在把「缺少 `finish_reason`」当作截断的确凿证据，发出 **`response.failed`**，
Codex 会识别并重试采样请求。（不用 `response.incomplete`：Codex 只识别
`response.completed` 与 `response.failed`，发前者会把流吊死得和静默完成一样。）

## 修复：回复被拆成每行两个字

部分网关（实测 CodeBuddy）在**每个**分片上都带 `"finish_reason": ""`。空串和
`null` 一样表示「还在生成」，只有最后一片才有真实取值；原代码把它当结束，于是
每个 token 分片都关掉再重开消息 item，客户端渲染成一行一条消息。现在与
`content`/`reasoning_content` 一致地过滤空串。

## 修复：daemon 卡在 draining 三小时

只有「等 permit」有超时，但排空还要停 IM 桥、取两次快照、抢租约锁、发关机
信号；这些 await 中任意一个被丢弃（调用方断开、代理放弃请求），admission 就
停在 `Draining` 且没有留下能离开它的持有者——此后所有网关请求回
`daemon_draining` (503)，而 launchd 因为 `KeepAlive` 只对进程退出反应、进程却
一直活着而毫无动作。

- 改用 RAII 守卫 `DrainingGuard`：`Drop` 时若 admission 仍为 `Draining` 就恢复
  服务并把桥交还运行时；排空已刻意完成时它是 no-op。
- 给整个重启流程加 330 秒硬上限，排空里任何一个无界 await 都不再能把请求永远
  挂住，超时即恢复服务并报 drain 失败。

## 修复：launchd 的 `spawn scheduled` 被当成未知

`launchdServiceState` 不认识 `spawn scheduled`、`spawning`、`terminated`
（三者都是 `launchctl print` 的真实状态），落进 `default` → `.unknown`，启动器
fail closed 抛「无法确认已登记后台服务的运行状态。」，把健康的切换中断了。
现在 `spawn scheduled`/`spawning` 视为「已登记且启动已排队」——任务已被拥有，
只是还没有 pid；`terminated` 归入已停止。

## 内部重构

- `main.rs` 里的实现搬进新的 `src/lib.rs`，二进制与测试共用同一个 crate，
  `main.rs` 只剩 `mochiport::run()`。
- 模块收进所属领域：`codex_app_*.rs`、`vscode_extension_patch.rs` → `codex/`；
  `daemon_process.rs`、`storage_migration.rs` → `daemon/`；`im_runtime.rs` →
  `im/runtime.rs`；`manage_api.rs` → `web/manage.rs`。
- Telegram 的 `adapter.rs`、`flow.rs`、`polling.rs`、`progress.rs` 各
  3000~7000 行，按职责拆成同名子模块目录，测试搬进各自的 `tests.rs`。
- Cargo 改为单一工作区：`windows/MochiPort/src-tauri` 成为根工作区成员，
  两处产物共用一个 `Cargo.lock` 与一个 `target/`；CI 新增 `windows-client`
  任务在 windows-latest 上单独检查该 crate。

验证：Rust 1255 项测试通过、Swift 206 项通过，`cargo fmt --all --check` 通过，
生成的客户端契约与 schema 一致。
