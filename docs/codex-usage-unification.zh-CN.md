# Codex 本地用量解析收敛设计

本文记录把 macOS 与 Windows 两份重复的 Codex 本地用量解析收敛到 daemon 的设计。
目标状态是：**daemon 是唯一的解析实现，两个客户端只负责展示**。

## 现状

同一份领域逻辑目前有两份独立实现：

| 端 | 位置 | 规模 |
| --- | --- | --- |
| macOS | `macos/MochiPort/Sources/MochiPortMac/UsageEngine/` | 17 个文件、约 3378 行 Swift |
| Windows | `windows/MochiPort/src-tauri/src/codex_usage.rs` + `codex_usage/history.rs` | 约 3198 行 Rust |

两边都在做同一件事：定位 Codex 会话日志、解析 token 用量、估算成本、预测额度耗尽、
按日/周聚合、维护历史快照。macOS 额外用 `DirectoryWatcher` + `IncrementalLineReader`
做实时 HUD。

daemon 目前**没有**这套能力：它的 token 统计只来自 AI Gateway 的请求日志
（`src/ai_gateway/request_log.rs`），`/api/v1/manage/sessions` 也只返回会话列表。
所以这次收敛需要在 daemon 侧新建解析实现，而不是搬运既有代码。

## 数据源

实测确认（`~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`，每行一个 JSON 事件）：

```json
{"timestamp":"2026-08-25T02:16:45.551Z","ordinal":0,"type":"session_meta","payload":{...}}
```

与用量相关的关键事件是 `event_msg` 中的 `token_count`：

```json
{
  "type": "token_count",
  "info": {
    "total_token_usage": {
      "input_tokens": 24230,
      "cached_input_tokens": 22272,
      "cache_write_input_tokens": 0,
      "output_tokens": 203,
      "reasoning_output_tokens": 48,
      "total_tokens": 24433
    },
    "last_token_usage": { "...": "同上结构" },
    "model_context_window": 258400
  },
  "rate_limits": {
    "limit_id": "codex",
    "primary": null,
    "secondary": null,
    "credits": null,
    "plan_type": null,
    "rate_limit_reached_type": null
  }
}
```

要点：

- 用量取 `info.total_token_usage`（累计）与 `info.last_token_usage`（单次），字段名固定。
- 额度信息在兄弟节点 `rate_limits`，官方账号下 `primary`/`secondary` 才有窗口数据；
  走 AI Gateway 时为 `null`。**解析必须容忍这些字段为 null**，不能据此丢弃整个事件。
- 会话元数据来自 `session_meta`（含 `cwd`、`originator`、`session_id`），
  项目维度聚合要靠 `cwd`。
- 单文件事件类型混杂：`response_item`、`event_msg`、`turn_context`、`compacted` 等，
  解析需按 `type` 分派并跳过不认识的事件。

## 目标架构

新增 daemon 模块 `src/usage/`：

| 模块 | 职责 |
| --- | --- |
| `locator` | 定位 Codex 会话根目录与 rollout 文件，按日期分层遍历 |
| `parser` | 逐行解析 rollout JSONL，提取 `token_count` / `session_meta` 事件 |
| `incremental` | 记录文件读取偏移，仅解析新增行（对应 macOS `IncrementalLineReader`） |
| `aggregate` | 按日、周、项目聚合用量 |
| `cost` | 成本估算（模型单价表） |
| `quota` | 额度耗尽预测（线性外推剩余窗口） |
| `store` | 历史快照持久化（复用现有 `src/store.rs` 的 SQLite 设施） |
| `service` | 对外服务：组合以上模块，供 HTTP 层调用 |

对外接口接在现有管理 API 下：

- `GET /api/v1/manage/usage/summary` — 当前用量与额度摘要
- `GET /api/v1/manage/usage/history?days=N` — 历史序列
- 变更推送复用 daemon 既有的 WebSocket 通道，客户端 HUD 订阅增量而非自行轮询文件

## 客户端改造

**macOS**：`UsageEngine/` 从"解析器"退化为"展示层 + 缓存"。
`LogLocator`、`CodexLogParser`、`IncrementalLineReader`、`DirectoryWatcher`、
`CostEstimator`、`DepletionEstimator`、`DailyStatsStore` 的职责全部移入 daemon；
保留 `UsageStore`、`BriefingEngine`、`MilestoneTracker`、`HUDEventPresentation`
作为 UI 侧聚合与呈现。

接线点已定位在 `MenubarCoordinator`：

- `private lazy var codexCollector = CodexCollector(roots: [...])` 是本地解析入口；
- `private var directoryWatcher: DirectoryWatcher?` 在文件变化时触发 `refresh()`；
- `refreshTimer`（30 秒）是兜底刷新；
- `UsageStore` 与 `DailyStatsStore` 是 HUD 的实际数据源。

因此切换集中在 `refresh()`：改为调用 `APIClient.fetchUsageSummary` 并写入 store，
同时删除 `codexCollector` 与 `directoryWatcher`。`refreshTimer` 应保留，
作为 API 不可用时的退避轮询；失败时沿用 store 中上一次的快照，而不是清空。

注意 `UsageStore` 现有接口按"事件流"设计（`addEvents` 带去重键），
而 API 返回的是按日聚合，因此需要一个适配步骤，不能直接对接。

**Windows**：删除 `codex_usage.rs` 与 `codex_usage/history.rs`，
`src/state/CodexUsage.tsx` 等改为消费管理 API。

接线点：`src-tauri/src/lib.rs` 的 `codex_usage_snapshot` command 转发给
`codex_usage::snapshot`，前端 `src/state/CodexUsage.tsx` 通过
`invoke("codex_usage_snapshot")` 调用它。切换时前端改成
`api.usageSummary()`（该客户端方法已就绪），随后移除该 command 与整个
`codex_usage` 模块。

两端都不需要先在客户端做聚合：daemon 返回的就是按日聚合结果，
客户端只负责展示与缓存。

### 阶段 3 的前置：daemon 的数据模型需要先扩展

核对 macOS 的 `TokenEvent` 后发现，客户端 HUD 依赖的维度远多于当前 API 返回的
按日聚合，直接切换会丢功能：

| 维度 | 客户端用途 | 当前 daemon API |
| --- | --- | --- |
| `timestamp`（分钟级） | 实时曲线、comeback gap 检测 | 只有按日 |
| `project`（`cwd` 的 lastPathComponent） | 按项目统计 | 无 |
| `model` | 按模型统计 | 无 |
| `source`（`ai-gateway` / `custom` / `sub2api`） | 区分来源 | 无 |
| `sessionID` + `cumulativeUsage` | replay 去重 | 无 |
| `limits` / `percentHistory` | 额度耗尽预测 | `rate_limits` 仅原样保留 |

也就是说，阶段 3 不能直接开始替换客户端。正确顺序是先扩展 daemon 的解析
与数据模型（至少覆盖 `model`、`project`、`source` 与时间桶），再决定 API 的
暴露形式——事件级列表，还是按多个维度聚合——然后才谈客户端切换。

在此之前，客户端的本地解析仍是 HUD 的唯一数据来源，不应删除。

#### 维度来源（实测）

扩展解析前先确认各维度在日志里的出处。实测
`~/.codex/sessions/<年>/<月>/<日>/rollout-*.jsonl`：

| 维度 | 出处 | 实测示例 |
| --- | --- | --- |
| `model` | `event_msg` / `thread_settings_applied` 的 `thread_settings.model` | `gpt-5.6-sol` |
| `source` | 同一事件的 `thread_settings.model_provider_id` | `ai-gateway` |
| `project` | `session_meta.payload.cwd` 的最后一段 | `codexhub` |
| 时间桶 | 每条记录自带的 `timestamp`（RFC3339） | — |

一个关键细节：`thread_settings_applied` 在一个会话里**可能出现多次**（用户切换模型或
provider 时）。所以不能只读第一条——要么按时间跟踪"当前设置"，要么把每个用量样本
按其发生时生效的设置归属，否则切过模型的会话会被整段算到错误的模型上。

## 降级策略（必须保留）

macOS HUD 当前靠本地文件监听做到实时；改走 daemon 后：

- daemon 未运行时 HUD 不得报错或留空，应显示上一次缓存快照并标注数据时间；
- 推送通道断开时回退为带退避的低频轮询，而不是停止刷新；
- 首次连接且无缓存时显示"等待 daemon"，不阻塞窗口启动。

## 口径一致性验证

收敛的前提是 daemon 的输出与现有实现**逐字段一致**。做法：

1. 固定若干份 rollout 样例作为 fixture，覆盖：单次会话、多会话同日、
   跨项目、`rate_limits` 为 null 与有值两种情况、`compacted` 会话、超长会话；
2. daemon 侧对 fixture 的聚合结果写成断言；
3. 在替换客户端实现**之前**，用同一批 fixture 比对 daemon 输出与现有
   macOS / Windows 实现的输出，确认一致后再删除旧实现。

### 已实测：累加口径是硬约束，不是实现细节

两个客户端都累加每条记录的 `last_token_usage`（macOS 见
`CodexLogParser.swift`，Windows 的注释写作 "Sum of Codex's reported per-turn
`total_tokens` values"）。

`total_token_usage` 是会话累计值：它把同一份上下文在每个 turn 重复计入，
所以**既不能求和、也不能只取最后一条**作为用量。

对本机 1114 个真实会话实测，两种口径在 **968 个（87%）**会话上结果不同，
差距可达数十倍：

```text
rollout-2026-07-20T19-37-08-...jsonl: cumulative=298255064 summed=8087961
rollout-2026-07-20T23-42-57-...jsonl: cumulative=291644508 summed=508187
```

daemon 的早期实现取的正是"最后一条累计值"，若照此切换，HUD 会显示放大约
37 倍的用量。现已改为 `last_token_usage` 优先、缺失时回退
`total_token_usage`，并逐条累加（`src/usage/scan.rs` 的 `accumulate`）。

这两条命令可在本机复现：

```text
cargo test --lib compare_accumulation -- --ignored --nocapture
cargo test --lib scan_local_sessions -- --ignored --nocapture
```

> 注意：macOS 侧还有去重键、8 天保留与 replay 替换等处理，用于抵消文件
> rotation 导致的重解析。daemon 走的是"每次从头扫描"，天然不会重复计数，
> 因此不需要对应机制；这一点在阶段 3 切换到增量扫描时必须重新评估。

### 实施记录：daemon 侧已完成的部分

`src/usage/` 与 `/api/v1/manage/usage/summary` 目前覆盖：

- **token 用量**：与两端一致的 per-turn 累加口径（见上文"累加口径是硬约束"）；
- **按日**：来自 `<年>/<月>/<日>` 目录布局；
- **按小时**：来自每条记录自身时间戳的小时位，不引入日期库；
- **按项目**：`session_meta.cwd` 的最后一段；没有 cwd 的会话记入空 project（与两端
  的 `event.project ?? ""` 一致），不再被跳过——`breakdown` 是客户端历史库的**替换**来源，
  跳过即等于删掉那部分用量；
- **按模型 / 按 provider**：按**样本记录时生效**的设置归属——Codex 每次变更设置都会
  重发 `thread_settings_applied`，所以切换过模型的会话会分别计入各自模型，而不是
  整段算到首尾某一个。归属带**回退**，因此没有样本会掉出 `breakdown`：
  model 回退到 `codex`（客户端的 `currentModel.isEmpty ? "codex"`），provider 先取
  `thread_settings_applied.model_provider_id`，否则回退 `session_meta.model_provider`
  （客户端 `source` 的来源），再回退 `legacy`；
- **额度窗口**：`rate_limits` 原样保留，尚未结构化。

> 归属回退是修掉一处数据缺口后加的：早期实现只在存在 `thread_settings_applied` 时写入
> `by_model_provider`，而本机 1114 个会话中只有 802 个会发出该事件，导致 `breakdown`
> 仅覆盖 **83.2%** 的 token，最近一天甚至只有 **3.3%**。补齐回退后实测 **100%**。
> `src/usage/scan.rs` 的 `attribution_coverage` 已断言这一点：
>
> ```text
> cargo test --lib attribution_coverage -- --ignored --nocapture
> ```

### 实施记录：扫描范围的两处修正（根目录与分钟序列）

**两个 root 都要扫。** 早先只扫 `~/.codex/sessions`，漏掉 `~/.codex/archived_sessions`
（本机 54 个会话、约 3.48 亿 token）。两端客户端本来就扫两个 root，daemon 漏扫的后果
比"少显示"更重：历史重建是**替换**当日行，漏掉的会话等于把那些天删掉。两棵树布局不同
（`sessions` 是 `<年>/<月>/<日>` 嵌套，`archived_sessions` 是平铺），因此日期按文件解析：
优先目录，平铺时取 `rollout-<YYYY-MM-DD>T…` 文件名，再退到记录自身时间戳。本机实测目录
日期与文件名日期在 400 个文件上**零分歧**。修正后 `sessions` 由 1114 升到 **1168**。

**分钟序列必须裁剪。** `minutes` 原先覆盖整个请求窗口，而 105 天刷新意味着每次（客户端
30 秒一次）传回磁盘上所有分钟——本机实测 **27547** 个桶，而客户端只用到最后一天
（macOS 裁到 48h、baseline 24h；Windows baseline 24h、速率窗口 3/10 分钟）。现按最新样本
向前裁剪 **48 小时**，实测降到 **123** 个桶（跨度 35.4 小时），且不依赖墙钟——只读旧日志的
机器仍能拿到自己最近的活跃分钟。

### 实施记录：客户端的两处回退缺口

**`daemonBreakdown` 一度是单向的。** 一旦 staging 了 daemon 的 breakdown，`persistStatsIfNeeded`
就永远走 `replaceCodexDays`，`upsert(events:)` 再也不会执行。daemon 停掉（或仍是未含 usage
接口的旧版，本机即如此）后，新事件不会落库，而且每次 persist 都用冻结的快照覆盖同一天。
现改为：刷新失败、或 daemon 明确返回空 breakdown 时释放 staging，把持久化交回本地事件流。

**简报的昨日 token 少了本地回退。** `yesterdayCost` 与 `yesterdayTopProject` 都有
`?? 本地事件` 回退，只有 `yesterdayTokens` 直接 `?? 0`。而 `upsert` 在 `needsCodexRebuild`
期间是不写入的，所以老库迁移的那段时间简报会把昨日显示成 **0**，尽管事件就在内存里。
现已按同一口径回退（用 `reportedTotalTokens`，与 `upsert` 聚合的字段一致）。

### 额度窗口：已取得真实样本，修正了两个假设

先前记录"本机 242 条记录中有值的为 0 条"——那是**只扫了单个文件**得出的错误结论。
对全部 1183 个 rollout 文件（210820 条含 `rate_limits` 的记录）完整搜索后，找到了
**5 条有值样本**，据此修正了两个与实际不符的假设：

| 假设 | 实测 |
| --- | --- |
| `primary` 是 5 小时窗口 | `primary` 的 `window_minutes` 实测为 **43200 / 43800**（30 天级） |
| 只存在 `resets_at`（绝对秒）与 `resets_in_seconds`（相对） | 还存在 **`resets_at: null`**，表示窗口没有计划中的重置 |
| `secondary` 会一并出现 | 5 条样本中 `secondary` **全为 null** |

因此实现改为：

- 解析并保留 **`window_minutes`**，调用方应据此区分窗口，而不是依赖
  `primary`/`secondary` 的位置含义（macOS 现有注释写的 "primary→session5h，
  secondary→weekly" 与实际数据不符）；
- `resets_at: null` 视为"无计划重置"而非畸形记录；
- 三种 `resets` 形态都归一化为绝对毫秒。

这两条真实样本已作为测试常量固定在 `src/usage/log.rs` 中，后续改动会立刻对照它们。

### 尚不能收敛的两项（含实测依据）

> 本节记录的是**当时的评估**，其中两项此后已被解决，保留原文以便对照判断依据：
>
> - **额度窗口**：已找到 5 条真实有值样本并完成解析（见上一节），不再是阻塞项；
> - **实时曲线**：daemon 现已提供分钟级 `minutes` 序列（按最新样本向前裁剪 48 小时），
>   两端客户端都已接入并用它计算实时速率。
>
> 唯一的遗留差异是**裁剪窗口**而非能力本身：客户端只用到 24–48 小时，因此 daemon
> 不再回传更早的分钟。

1. **额度窗口**。本机 242 条 `token_count` 记录里，`rate_limits.primary` / `secondary`
   有值的为 **0 条**——经 AI Gateway 时这些字段恒为 `null`（官方账号下才有值）。
   缺少有值样本就无法为其编写可信解析，硬写等于在生产环境盲赌；而 macOS 的额度
   耗尽预测（`DepletionEstimator` + `percentHistory`）恰好依赖这一维度。
2. **实时曲线**。HUD 的分钟级序列要求保留每个样本，而 daemon 目前只暴露小时级聚合。
   补齐会改变内存与传输特征，应按实际需要再定。

### 为什么客户端切换不是"换个数据源"（接口确证）

> 同上的历史评估。"daemon 现状"一列描述的是当时状态；分钟级序列与额度维度此后均已补齐，
> 下表保留下来是为了说明当时为什么不能直接切换。

`UsageStore` 被 HUD 消费的读取接口几乎全部建立在**样本级 / 分钟级**数据之上：

| 接口 | 依赖 | daemon 现状 |
| --- | --- | --- |
| `tokensPerMinute(windowMinutes:)` | 分钟级 bucket | 只有小时级 |
| `consumeComebackGap()` | 相邻活动的分钟间隔 | 无 |
| `activeBaselineRate(now:)` | 分钟级速率序列 | 无 |
| `depletion(for:)` / `approxFullReset(...)` | 额度窗口 + `percentHistory` 采样 | 无（见上文额度限制） |
| `projectServiceBreakdown(days:)` | `service` 维度 | 只有 project，无 service |
| `dailyTotals(days:)` / `todayTokens()` | 日聚合 | **已有** |

也就是说，HUD 的实时指标（速率、回归间隙、基线、耗尽预测）都要求 daemon 提供
**分钟级序列**与 **service 维度**，而这两项目前都没有；日聚合是唯一已经可以直接
对接的部分。

因此在"删除客户端解析"这一步上，有两种收口方式：

- **A（完整收敛）**：取得一份官方账号下产生的 rollout 样本 → 补全额度维度 →
  切换两端并删除旧实现；
- **B（部分收敛）**：daemon 作为用量、项目、模型与历史聚合的权威来源，客户端保留
  额度窗口与实时曲线的本地逻辑。选择 B 时必须把这条边界写进代码注释与本文，
  否则后来者会误以为已经单一实现。

### 实施记录：macOS 历史库已切换（含回退）

`MenubarCoordinator` 的历史库重建已改为**优先 daemon**：

- 可用时用 `/api/v1/manage/usage/history` 的 `breakdown` 投影成 `DailyStatsRow`；
- 不可用时回退 `CodexCollector.historicalRows`（保持原有本地行为）。

回退**不是可选项**：`beginCodexRebuildIfAllowed` 失败会进入 6 小时重试窗口，而
`needsCodexRebuild` 为真期间 `upsert` 直接返回、新事件不写入。若 daemon 未启动
就判定失败，用户的历史库会被卡住半天。

接入成本也比预想低：`APIClient` 的默认构造会自行读取 daemon 地址与凭证，
`dashboard()` 一类便捷方法已内置候选凭证轮换，因此 HUD 无需处理地址、凭证或轮换。

HUD 的**实时指标**（`tokensPerMinute`、`consumeComebackGap`、`activeBaselineRate`）
与**额度预测**（`depletion`、`approxFullReset`）仍走本地。

### 实施记录：Windows 历史/趋势已切换

`codex_usage::snapshot()` 现在先向 daemon 请求 `/api/v1/manage/usage/history`，
成功时覆盖 `sevenDay` / `dailyUsage` / `sevenDayProjects` / `topProject` /
`estimatedCostUsd`；失败（daemon 未启动、非 200、解析失败）时**整个快照仍由本地
collector 产生**，所有字段都有值。

几个必须处理的语义差异，均有测试锁定：

- **补零日期轴**：daemon 只返回有数据的日期，而图表需要连续 x 轴，安静的日子记 0；
- **同日多行求和**：`breakdown` 按 `(day, model, provider, project)` 存储，同一天
  会有多行，必须求和而不是覆盖；
- **窗口外不泄漏**：7 天序列不得混入窗口外的行；
- **空项目名跳过**：没有 `cwd` 的会话不显示成一个叫空字符串的项目。注意这只适用于
  **排名**：这类行仍按空 project 计入当日总量（`windowed_projects` 里
  `row.project.is_empty()` 只跳过聚合，`daily_totals` 不跳过）。

之所以用 `breakdown` 而不是 `days` + `projects`：Windows 需要 7 天与 105 天两个
序列，而 `breakdown` 的每一行都带 `day`，两个窗口可以从同一份响应里切出来，
只需请求一次。（`/usage/summary` 的 90 天上限不够 105 天，所以历史接口不共用该上限。）

> 窗口上限曾有一处实现错误：`history_summary` 先按 `HISTORY_WINDOW_DAYS` 收敛、再调用
> `summary()`，而后者又做了一次 `normalize_window_days`，于是 `days=105` 与"全量"
> 都被静默压回 90 天。macOS 的历史重建会 `DELETE ... WHERE service='codex'` 后重插，
> 因此 90 天以前的历史会被**永久删除**。现已拆成 `history_window_days`（历史上限）
> 与 `normalize_window_days`（交互上限）两条独立路径，并有测试锁定。

### 实施记录：macOS 侧没有可以删除的组件

切换历史库后，我曾预期可以退役 `CodexLogParser` / `LogLocator` /
`IncrementalLineReader` / `DirectoryWatcher`（合计 488 行）。逐一核对引用后，
**结论是不能删**：

| 组件 | 同时服务于 | 结论 |
| --- | --- | --- |
| `CodexLogParser` | `historicalRows`（158–176 行）与 `hydrateRecentHistory` / `collect`（369–384 行） | 保留 |
| `LogLocator` | `historicalRows` 的 `allFiles` 与实时路径的 `recentFiles` | 保留 |
| `IncrementalLineReader` | collector 的实例字段，服务实时增量读取 | 保留 |
| `DirectoryWatcher` | 由 `MenubarCoordinator` 用于文件变更触发刷新 | 保留 |

原因是 collector 有**两条**用途：除了历史重建，它还通过 `addEvents` /
`setLimits` 把事件流喂给 `UsageStore`，而 HUD 的实时速率与额度预测都建立在这条
事件流上——这恰是 daemon 目前无法提供的部分。

因此 macOS 侧达成的收敛是：**历史重建优先走 daemon，本地实现降级为回退**；
而不是"删掉一份解析实现"。这两件事需要区分清楚，否则会误判剩余工作量。

### 实施记录：Windows 侧不能整文件删除

`codex_usage.rs`（2511 行）+ `codex_usage/history.rs`（687 行）与 daemon **只有部分重复**：

| 职责 | 与 daemon 关系 | 位置 |
| --- | --- | --- |
| 解析、增量读取、日/周/项目聚合 | **重复**，可切到 daemon | `collect_recent_jsonl`、`read_increment`、`parse_line`、`extract_day_from_path`、`project_name` 等 |
| 成本估算 | daemon **没有**（内置价格表 + 用户自定义价格） | `codex_pricing`、`resolve_pricing`、`embedded_codex_pricing` |
| 额度耗尽预测 | daemon **没有**（需 `rate_limits` 有值样本） | `estimate_session_depletion`、`estimate_weekly_depletion`、`parse_quota_windows` |
| 实时速率 / 连续天数 | daemon **没有**（需分钟级序列） | `tokens_per_minute`、`burn_rate_*`、`history.rs` |

而且前端消费的是**整体快照**（`CodexUsageSnapshot`，21 个字段），不是分块数据。
因此 Windows 侧的正确做法不是删文件，而是：

1. 把"解析 + 日/周/项目聚合"切到 daemon 的 `breakdown` / `days`；
2. **成本与额度上移到 daemon**——它们同样属于"单一实现"的目标，但需要先迁移
   价格表并确认计费口径；
3. 实时速率与连续天数在 daemon 提供分钟级序列之前保留本地。

第 2 步是让 Windows 侧真正收敛的前提，且它比"接个接口"更接近一次功能搬迁。

### 实施记录：两端都不能删除的本地状态（核查结论）

切换完成后，我逐项核对了"哪些本地代码仍不可替代"。结论是**两端都保留了自己的实时/长期状态**，
它们与 daemon 的职责不同，不构成重复实现：

**macOS**

| 本地组件 | 独有职责 | 为什么 daemon 不能替代 |
| --- | --- | --- |
| `UsageStore.events` | `DirectoryWatcher` 推送式触发（秒级） | daemon 是 30 秒拉取；HUD 需要即时响应文件变化 |
| `DailyStatsStore` | 本地 SQLite + 重建状态机 | 承载"重建中/已重建"的迁移状态 |
| `EventEngine` / `MilestoneTracker` / `BriefingEngine` | 事件、里程碑、简报 | 纯 UI 派生，不在 daemon 范围内 |

**Windows**

| 本地组件 | 独有职责 | 为什么 daemon 不能替代 |
| --- | --- | --- |
| `UsageHistoryStore` | `streak_days`、`previous_best_daily_tokens`、`weekly_daily_rate` | daemon 只给原始数据，不做长期派生状态 |
| 同上 | primary + `.bak` 双代备份与恢复 | 数据安全设计，daemon 无对应机制 |
| `collector.files` | 增量读取游标（含 rotation 处理） | daemon 每次全量扫描，无跨调用游标 |

因此第 2 项的收敛边界是：**数据来源单一化**（两端都优先消费 daemon，并保留回退），
而不是**删除本地实现**。删除本地状态会让 HUD 失去秒级响应、失去连续天数与历史最佳、
失去额度历史的持久化与备份。

### 为什么 streak 类派生必须留在客户端（修正了一个错误判断）

我曾判断"连续天数与历史最佳只是十几行算法，daemon 的 `days` 数据已够，可以迁走"。
**这个判断是错的** —— 我只看了算法复杂度，没看数据生命周期。

关键事实：

- **daemon 不持久化任何用量历史**。`service::compute` 每次都实时扫盘（`scan_window` → 读
  rollout 文件），没有任何数据库。
- **Codex 的日志会被轮转或清理**。本机 `~/.codex/sessions` 已有 5.8G，并存在
  `archived_sessions`；实测 60 天覆盖里**有 5 处断档、共缺 11 天**。
- Windows 的 `UsageHistoryStore` 明确承担长期记忆：`replace_daily_tokens` 的注释写着
  "preserving older rows after their source files age out or rotate away"。

因此：**日志一旦被清理，daemon 就再也看不到那些天**，而本地库仍保留当时的观测值。
连续天数与历史最佳恰恰要跨越这个生命周期 —— 迁到 daemon 会让日志被清理的用户
"连续记录突然断掉"。这类派生属于**持久化端的职责**，不是重复实现。

判断一项逻辑该不该迁，要先问"它的数据源会不会消失"，而不只是"算法难不难"。

### 价格表：三份副本 + 一致性校验

Codex 的单价表在**三处**各有一份：daemon `src/usage/cost.rs`、macOS
`CostEstimator.swift`、Windows `codex_usage.rs`。实测三方**当前完全一致**（27 条、零差异），
且两端各自消费它计算实时成本，所以不能简单删掉任何一份。

真正的风险是**未来漂移且无人发现**：往其中一处加一个模型，另外两处不会跟上，
daemon 与客户端就会对同一个会话报出不同的成本。

因此 `src/usage/cost.rs` 增加了一个跨文件校验测试 `client_price_tables_match_the_daemon`：
它解析两份客户端源码里的价格条目，逐条与 daemon 的内置表比对（数量、名称、三项单价）。
已用"故意改一个价格"验证过它会失败并指出是哪一端、哪个模型。

修价格时要同时改三处 —— 这个测试会拦住只改一处的情况。

### 顺带修掉的三个问题

1. **daemon 缺少用户自定义价格**：两端都读 `~/.claude/pricing.json`，而 daemon 只有内置表。
   用户配置自定义价格时，两端与 daemon 会算出不同数字。已补齐（用户表优先，缺失即回退）。
2. **macOS 拉取无节流**：`refresh()` 由文件监听驱动、可能每秒触发，而历史接口默认返回全量。
   已加 30 秒节流；Windows 是定时刷新，本无此问题。
3. **历史接口无窗口参数**：周期性刷新被迫拉全量。已加 `days`，Windows 客户端刷新取
   105 天（与其 `HISTORY_DAYS` 保留期一致）；只有重建路径请求全量。

> 另有一处实现与注释不符已修正：`history_summary` 的窗口被二次钳制到交互上限
> （90 天），使 105 天刷新与"全量重建"都只拿到 90 天。见上文"窗口上限"注记。
> macOS 侧并不存在"105 天保留期"——`UsageStore.retentionDays` 是事件尾部的 8 天，
> `DailyStatsStore` 不做过期清理；105 只用于 Windows 的图表窗口。

## 分阶段实施

1. **解析引擎**：`src/usage/` 的 `locator`/`parser`/`incremental`/`aggregate` + fixture 测试。
2. **API 暴露**：接入管理 API 与 WebSocket 推送，补接口测试。
3. **客户端接入**：macOS 与 Windows 分别切换到新接口，删除旧的解析实现。
4. **清理**：移除各端已无用的依赖与文件，更新本文与相关文档。

阶段 1 与 2 不改变任何客户端行为，风险最低；阶段 3 是行为变更，
必须按上面的降级策略逐端验证。
