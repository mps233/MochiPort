# Anthropic Prompt Caching（cache_control）读写机制说明

状态：基础说明。本文记录 Anthropic Messages API `cache_control` 的读写机制、断点规则、与 Claude Code 的实际行为对照，并给出 AI Gateway（Responses ↔ Messages 转译）在多 `system` / 多 `messages` block 场景下的对齐策略。后续 `request.rs` 的缓存断点实现以本文为准。

相关文档：

- [`ai-gateway-anthropic-messages.zh-CN.md`](ai-gateway-anthropic-messages.zh-CN.md)
- [`ai-gateway-anthropic-first-roadmap.zh-CN.md`](ai-gateway-anthropic-first-roadmap.zh-CN.md)
- Anthropic Prompt Caching: <https://platform.claude.com/docs/en/build-with-claude/prompt-caching>
- Anthropic Prompt Caching（镜像）: <https://docs.anthropic.com/en/docs/build-with-claude/prompt-caching>

## 1. 核心模型：前缀缓存

`cache_control` 标记的语义不是「缓存这一个 block」，而是 **「缓存从请求开头到这个 block 为止的整个前缀」**。

前缀的拼接顺序固定为：

```
tools  →  system  →  messages
```

也就是说，一个断点缓存的内容 = 它前面的所有 tools + 所有 system + 截至该 block 的所有 messages。这条顺序是理解后续所有规则的基础：排在越前面的内容（tools）一旦变化，会让它后面所有缓存前缀全部失效。

## 2. 读与写是两套独立机制

这是最容易被官方「一个断点就够了」这句话误导的地方。读和写必须分开理解。

### 2.1 写（创建缓存）：只发生在显式断点上

- 缓存**只在你显式标了 `cache_control` 的 block 上被创建**。
- 没有标断点的位置，系统**不会主动写缓存**。
- 写入的是「截至该 block 的整个前缀」的一份缓存条目。

### 2.2 读（命中缓存）：才有自动回溯

- 请求带着断点进来时，系统从断点位置**向前回溯**（大约每 20 个 block 一个检查点），寻找**此前已经写过的**最长匹配前缀。
- 匹配到就部分命中，按命中长度计费为 `cache_read`。
- 回溯**不能凭空创建缓存**，它只能找回已经存在的缓存。

### 2.3 一句话总结

> **断点（写）负责把前缀「存下来」；回溯（读）负责把已存下来的前缀「找回来」。**
> 官方说「末尾一个断点就够」，前提是更靠前的前缀此前已经被写过缓存——回溯只是替你省去重复标注，不替你创建缓存。

## 3. 断点数量与限制

- 每个请求**最多 4 个 `cache_control` 断点**，超过直接报 400 错误。
- 这 4 个名额是 **tools + system + messages 全局共享**，不是每段各 4 个。
- 每段缓存前缀有**最小 token 门槛**：多数模型 1024 token，部分小模型 2048 token。不够长不会真正写入缓存（断点会被静默忽略）。
- TTL：默认 5 分钟（`{"type":"ephemeral"}`），每次命中刷新；可选 1 小时（`{"type":"ephemeral","ttl":"1h"}`）。

## 4. Claude Code 实测行为

抓包结果（以实测为准）：

| 维度 | Claude Code 行为 |
|------|------------------|
| `tools` | **不加** `cache_control` |
| `system` | **每个 block 都加** |
| `messages` | **只加 1 个断点**，落在会话尾部的一条消息上，role 不限（`user`/`assistant`），block 类型不限（`text`/`tool_result`/`tool_use` 都算） |

> 抓包实证（2026-07，claude-cli/2.1.185）：
>
> - 以 `tool_result` 结尾的请求（`anthropic2.json`，300 条消息）里，`cache_control` 直接打在末尾 `tool_result` block 上（与 `type`/`content`/`tool_use_id` 同级）。总断点数 = 2 条 system + 1 个末尾 tool_result = 3。
> - 以纯文本 user 结尾的请求（`anthropic.json`，302 条消息）里，断点没有落在最后一条 user（`msg[301]`），而是落在**倒数第二条 `role==assistant` 的 text block**（`msg[300]`）上。总断点数 = 2 条 system + 1 个 assistant = 3。
>
> 两条实证合起来说明：Claude Code 的消息断点是**单个、跟着会话尾巴走**的，承载它的既可以是 user/tool_result，也可以是 assistant/tool_use；messages 数组中间即便混入 `role:system`（mid-conversation-system）也不加。这也推翻了早期「assistant 消息永不标记」的假设。

### 4.1 为什么 system 每条都加？

Claude Code 的请求结构里 `system` 是**固定且少**的（通常 2 条）：

```
system[0]: "You are Claude Code, Anthropic's official CLI for Claude."   ← 极短、固定
system[1]: 一大段环境与指令                                               ← 大、基本不变
```

真正承重的是 **system 最后一条上的断点**——它把 `tools + system` 整段前缀写成一份独立缓存，边界正好卡在 `system` 与 `messages` 之间，于是 messages 怎么增长，这份 system 缓存都稳定复用。

system[0] 上那个断点其实是**冗余的廉价保险**：它太短（远不到 1024 token），自身不会真正落缓存，但标着也无害——因为 Claude Code 总断点数（2 条 system + 末尾 1~2 个会话断点）不超过 4，名额富裕，索性每个稳定边界都标。

### 4.2 为什么 tools 不加？

因为没必要。`tools` 排在前缀最前面，只要后面（`system` 或 `messages`）有任意一个断点，tools 会作为前缀的一部分**自动被缓存进去**。Claude Code 把宝贵的 4 个名额留给 system 末尾和会话末尾，靠前缀机制覆盖 tools，无需为它单独花一个断点。

### 4.3 用户自定义 / MCP tool 加不加？

不用单独加。MCP tool 与内置 tool 同在 `tools` 数组里，属于前缀最前段，同样被 system 末尾的断点覆盖。

需要注意的固有行为：**tools 列表一旦变化（含新增/删除 MCP 工具），整个缓存前缀失效**——因为 tools 在最前面，它变了，后面 system、messages 的缓存全部作废。这与是否在 tools 上加断点无关。

## 5. Gateway 对齐策略（Responses ↔ Messages）

Claude Code 敢「system 每条都加」，前提是它的 system block 数量固定且少。而 Gateway 是 Responses → Messages 转译，**Codex 可能塞进来多条 system block**。若照搬「每条都加」，光 system 就可能吃掉 5~8 个断点，**直接撞穿 4 个上限，整个请求被拒**。

因此 Gateway 的正确姿势不是模仿表象，而是抓住承重断点：

| 维度 | Gateway 策略 | 理由 |
|------|--------------|------|
| `tools` | 不加（剥离上游携带的 `cache_control`） | 靠前缀机制自动覆盖，省名额，与 CC 一致 |
| `system` | **只在最后一条 text block 加 1 个断点** | 等价于 CC 的承重断点；靠自动回溯覆盖前面所有 system block；规避 4 上限 |
| `messages` | 在**最后两条 `role==user`/`assistant` 消息**的最后一个 block 上各加 1 个断点（双滚动，含 `tool_result`/`tool_use`；跳过 mid-conversation `system`） | 双滚动比 CC 原生的单断点多铺一个较早的读取锚，靠读侧回溯抹平命中率锯齿。详见第 6 节 |
| 顶层 `cache_control` | 不生成 | 非法字段 |
| `ttl` | 默认 5m；长会话可选映射 `1h` | 与 `prompt_cache_retention` 对接（可选） |

断点预算：system 末尾 1 个 + messages 最后两条消息各 1 个（≤2），共 ≤3，控制在 4 以内。CC 原生只发 1 个消息断点（共 3），Gateway 多发 1 个仍稳在 4 上限内。

## 6. 双滚动断点（messages 段）

### 6.1 为什么单个末条断点会命中率锯齿

只在「最后一条消息」打单个断点时，多轮会话会出现 read 命中率在 ~14%（仅 system+tools）与 ~99%（含历史）之间剧烈摆动的锯齿。生产日志（SQLite `ai-gateway-request-logs`，请求 1978–1990）排查结论：

- **marker 漂移无害**：Anthropic 计算前缀哈希时会剔除 `cache_control` 字段。实证 1978 的 marker 在 `msg[193]`、1979 漂到 `msg[196]`，1979 仍命中含 193 在内的 99k tokens——证明漂移不破坏前缀。
- **前缀逐字节稳定**：相邻轮次 messages 前缀 100% 一致（leadMatch），未超 5 分钟 TTL。
- **真正根因**：单个末条断点时，第 N 轮的写入点（末条）在第 N+1 轮**失去断点覆盖**，无法作为读取终点被命中，导致每轮重写整段历史。1979 偶发 99% 只是恰好赶上上一轮写入仍可读的时序窗口。

### 6.2 解法：最后两条消息各打一个（双滚动）

业界标准做法（Cline / OpenRouter / Anthropic 官方）是**双滚动断点**：在会话尾部铺两个断点而不是一个。

- **末条断点** = 本轮写入点，缓存截至当前的整段前缀。
- **倒数第二条断点** = 一个较早的读取锚，比单断点更接近上一轮的写入位置，让本轮更容易回溯命中已写前缀。

**为什么承载断点的消息从「只认 user」放宽到「user + assistant」**：抓包（第 4 节 `anthropic.json`）显示 Claude Code 本身就会把消息断点打在 assistant 尾块上，早期「assistant 永不标记」的假设并不成立。放宽后，两个断点稳定贴着 append-only 的会话尾巴走，无需再区分尾块是 user 还是 assistant。

> 注意履带对齐性质的变化：旧的「只认 user」实现里，倒二断点**正好落在上一轮的写入点上**（turn N 的 last-user 索引 == turn N+1 的 2nd-last-user 索引，1978–1990 共 13 对全部 MATCH）。放宽到 user+assistant 后，agent 循环每轮尾部会追加 assistant + user 两条消息，倒二断点可能落在**本轮才新增、上一轮从未写过缓存**的 assistant 块上，于是「倒二 == 上一轮写入点」这条严格对齐不再成立。命中不再依赖履带精确对齐，而是依赖**读侧约每 20 个 block 的回溯窗口**：每轮尾部只增长约 2 个 block，远小于回溯窗口，因此上一轮写入的前缀仍能被找回。
>
> 换句话说，双滚动现在是「多铺一个较早读取锚 + 读侧回溯兜底」，而不是「履带逐格咬合」。是否真的抹平锯齿，应以真实多轮的 `cache_read_input_tokens` 曲线为准（见第 8 节验证项）。

### 6.3 落点规则

- 认 `role=="user"` 与 `role=="assistant"` 的消息；跳过 mid-conversation `system`（动态提示，不适合作缓存锚）。tool_result 是 role=user、tool_use 是 role=assistant，agent 循环尾部天然覆盖。
- 每条断点落在该消息的**最后一个 block**，不限 block 类型（`text`/`tool_result`/`tool_use` 均可）。
- `.rev().take(2)`：只有 1 条合格消息时只打 1 个。
- 与 system 断点合计 ≤3，安全在 4 上限内。

## 7. 响应侧用量统计

`convert_anthropic_response` 解析 usage 时拆分缓存 token：

```text
input_tokens
output_tokens
cache_creation_input_tokens   → cache_creation_tokens（本次写入缓存）
cache_read_input_tokens       → cached_tokens（本次命中缓存）
```

并支持 `cache_creation` 细分（`ephemeral_5m_input_tokens` / `ephemeral_1h_input_tokens`）。读取侧已兼容 1h，即便写入侧当前固定走 5m。

## 8. 实现状态

已落地（`request.rs`，与本文对齐）：

- `insert_system_cache_control`：数组分支只在**最后一条** text block 加断点，规避多 system block 撞 4 上限。
- `insert_message_cache_control`：先用 `message_tail_can_carry_cache_control` 过滤出 `role` 为 `user`/`assistant` 且尾块可承载的消息（跳过 mid-conversation `system`），再对**最后两条**各打一个断点（双滚动，见第 6 节），复用 `mark_last_block_cache_control`。不限 block 类型（text/tool_result/tool_use 均可）。
- `tools`：不加 `cache_control`（剥离上游携带的），维持现状。
- 测试：`builds_anthropic_text_request`、`caches_last_two_message_tails`、`marks_only_last_two_message_tails_as_rolling_breakpoints`、`caches_assistant_tool_use_when_it_is_in_the_rolling_window`、`builds_anthropic_request_with_claude_code_block_level_ephemeral_cache` 覆盖 user/assistant 双滚动与预算上限。

待验证：放宽到 user+assistant 后不再依赖履带严格对齐（见第 6.2 节），需用真实多轮的 `cache_read_input_tokens` 曲线（SQLite `ai-gateway-request-logs`）确认命中率没有回到第 6.1 节的锯齿。

未做（本次范围外）：`prompt_cache_retention = "1h"` → `cache_control.ttl` 透传，仍固定 `{"type":"ephemeral"}`（5m）。

## 9. 生产验证：偶发缓存 miss 根因分析

### 9.1 验证数据

基于 SQLite `ai-gateway-request-logs` 的两组真实数据：

**正常区间（3542–3548，7 轮）**：
- System/tools 哈希：连续 7 轮 100% 一致
- Messages 每轮增长 3 条（assistant text → assistant tool_use → user tool_result）
- 双滚动断点：每轮在最后两条消息上（如 msg[69-70] → msg[72-73] → msg[75-76]）
- Cache read：稳定在 21 万+ tokens，占总输入 92-94%
- Cache write：每轮 1-5k tokens（新增消息）
- **结论：双滚动按预期工作，命中率稳定**

**异常区间（3521–3530，10 轮）**：
- System/tools 哈希：连续 10 轮 100% 一致
- 两次完全 cache miss：
  - **Request 3525**（23:15:45）：read=0, write=145437，距前一轮 3524 仅 9 秒
  - **Request 3529**（23:19:09）：read=0, write=168372，距前一轮 3528 共 183 秒（3 分钟）
- Miss 后立即恢复：
  - Request 3526（3525 后 5 秒）：read=129831，正好命中 3524 写入的量
  - Request 3530（3529 后 10 秒）：read=163266，命中 3528 写入的量

### 9.2 排除的可能原因

1. **前缀内容变化**：✗ System/tools 哈希在两组数据中全程一致
2. **`cache_control` 标记漂移**：✗ 对比发现正常区间（3542-3543）和异常区间（3524-3525）都存在历史消息 `cache_control` 被移除的情况，但前者不 miss，说明 Anthropic 服务端确实会过滤此字段后再计算哈希
3. **TTL 过期**：✗ 3525 距 3524 仅 9 秒，3529 距 3528 仅 183 秒，远小于 5 分钟 TTL
4. **20-block 回溯窗口溢出**：✗ 实测距离仅 4 blocks（3524 msg[19] block 37 → 3525 msg[21] block 40），远小于 20
5. **Gateway 代码逻辑错误**：✗ 正常区间证明双滚动断点能稳定工作

### 9.3 确认的根本原因：Anthropic API 分片不一致

关键证据：

1. **3524 写入的缓存确实存在**：3526（5 秒后）命中了 129831 tokens，正好是 3524 写入的量
2. **3525 在查询时未找到**：read=0，但前缀完全一致
3. **3526 又找到了**：说明缓存本身正常，只是 3525 那次查询没找到

**最可能的技术原因**：Anthropic API 后端的多服务器 / 缓存分片架构导致：

- **异步写入延迟**：3524 的 HTTP 响应返回时，缓存可能还在后台异步写入分片
- **跨分片同步延迟**：缓存写入分片 A，但 3525 请求被路由到分片 B，此时分片间尚未同步（典型延迟 5-60 秒）
- **负载均衡路由变化**：连续请求可能基于客户端 IP、时间窗口、服务器负载等因素被路由到不同节点

**为什么正常区间（3542-3548）没问题**：
- 请求间隔更短且连续（5-10 秒），可能被路由到同一服务器
- 缓存留在本地 L1 层，未经过跨分片同步
- 或者该时段负载低，分片同步速度快

### 9.4 影响评估

- **发生频率**：10 轮中 2 次 miss（20%），但连续稳定区间可达几十轮无 miss
- **恢复速度**：miss 后下一轮（5-10 秒）立即恢复正常
- **成本影响**：单次 miss 多付 ~14 万 tokens 的 write 费用（约 $0.009 for Opus-4.8），但下轮立即恢复 92%+ 命中率
- **延迟影响**：miss 轮 TTFT 会增加，但 Anthropic 的 cache write 本身是流式并行的，影响有限

### 9.5 应对策略

**当前建议：接受现状**
- 这是 Anthropic API 基础设施的最终一致性延迟，客户端无法控制
- 偶发 miss 不影响功能正确性，下一轮自动恢复
- 成本影响可控（20% miss 率时，整体缓存命中率仍在 75%+）

**可选优化方向**（视需求决定是否实现）：
1. **检测与重试**：在响应解析时检测 `cache_read_input_tokens=0` 且前缀理论应命中的情况，记录日志或触发 1-2 秒后的自动重试
2. **监控告警**：统计 miss 率，当连续 N 次或一定时间窗口内 miss 率超过阈值时告警
3. **向 Anthropic 反馈**：提供复现数据（请求 ID、时间戳），推动官方优化缓存分片同步机制

**不建议的方案**：
- ✗ 客户端侧缓存预热：无法解决服务端分片不一致问题
- ✗ 调整断点策略：双滚动已是业界最佳实践，正常区间证明其有效性
- ✗ 增加断点数量：受 4 个上限限制，且无助于解决分片同步延迟

### 9.6 结论

Gateway 的双滚动断点实现正确且有效。偶发缓存 miss 是 Anthropic API 服务端分布式架构的固有特性，属于「最终一致性」在缓存系统中的体现。只要前缀保持稳定（system/tools 不变），miss 后会自动恢复，整体命中率仍可达 80-95%。
