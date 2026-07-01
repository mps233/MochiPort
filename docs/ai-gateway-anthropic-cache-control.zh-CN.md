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
| `messages` | **只在最后一条消息的最后一个 block 加**，role 不限（`user`/`assistant`），block 类型不限（`text`/`tool_result` 都算） |

> 抓包实证（2026-07，claude-cli/2.1.185）：一次以 `tool_result` 结尾的请求里，`cache_control` 直接打在末尾 `tool_result` block 上（与 `type`/`content`/`tool_use_id` 同级）。messages 数组中间即便混入 `role:system`（mid-conversation-system）也不加；只有数组最后一条拿断点。该请求总断点数 = 2 条 system + 1 个末尾 tool_result = 3，未超 4。

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
| `messages` | 在**最后两条 `role==user` 消息**的最后一个 block 上各加 1 个断点（双滚动，含 tool_result） | 履带式滚动：倒二 user 断点保住上一轮写入前缀作读取锚，末条 user 断点作本轮写入点，抹平命中率锯齿。详见第 8 节 |
| 顶层 `cache_control` | 不生成 | 非法字段 |
| `ttl` | 默认 5m；长会话可选映射 `1h` | 与 `prompt_cache_retention` 对接（可选） |

断点预算：system 末尾 1 个 + messages 最后两条 user 各 1 个（≤2），共 ≤3，控制在 4 以内。

## 6. 双滚动断点（messages 段）

### 6.1 为什么单个末条断点会命中率锯齿

只在「最后一条消息」打单个断点时，多轮会话会出现 read 命中率在 ~14%（仅 system+tools）与 ~99%（含历史）之间剧烈摆动的锯齿。生产日志（SQLite `ai-gateway-request-logs`，请求 1978–1990）排查结论：

- **marker 漂移无害**：Anthropic 计算前缀哈希时会剔除 `cache_control` 字段。实证 1978 的 marker 在 `msg[193]`、1979 漂到 `msg[196]`，1979 仍命中含 193 在内的 99k tokens——证明漂移不破坏前缀。
- **前缀逐字节稳定**：相邻轮次 messages 前缀 100% 一致（leadMatch），未超 5 分钟 TTL。
- **真正根因**：单个末条断点时，第 N 轮的写入点（末条）在第 N+1 轮**失去断点覆盖**，无法作为读取终点被命中，导致每轮重写整段历史。1979 偶发 99% 只是恰好赶上上一轮写入仍可读的时序窗口。

### 6.2 解法：最后两条 user 消息各打一个（Cline 模式）

业界标准做法（Cline / OpenRouter / Anthropic 官方）是**双滚动断点**：

- **末条 user 断点** = 本轮写入点，缓存截至当前的整段前缀。
- **倒数第二条 user 断点** = 本轮读取锚，正好落在上一轮的写入点上，命中上一轮写入的前缀。

两个断点像履带一样交替前进。SQLite 模拟已验证对本链路完全有效：turn N 的 last-user 索引 == turn N+1 的 2nd-last-user 索引，1978–1990 共 13 对全部 MATCH（如 1978.last=193 == 1979.2ndLast=193）。

### 6.3 落点规则

- 只认 `role=="user"` 的消息（tool_result 也是 role=user，agent 循环尾部天然覆盖）；assistant 消息（含 tool_use）永不标记。
- 每条断点落在该 user 消息的**最后一个 block**。
- `.rev().take(2)`：只有 1 条 user 消息时只打 1 个。
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
- `insert_message_cache_control`：在**最后两条 `role==user` 消息**的最后一个 block 上各打一个断点（双滚动，见第 6 节），复用 `mark_last_block_cache_control`。不限 block 类型（text/tool_result 均可）。
- `tools`：不加 `cache_control`（剥离上游携带的），维持现状。
- 测试：`builds_anthropic_text_request`、`caches_last_two_user_messages`（原 `caches_only_last_message_block`）、`does_not_cache_assistant_tool_use_blocks`、`builds_anthropic_request_with_claude_code_block_level_ephemeral_cache` 已更新；新增 `marks_only_last_two_user_messages_as_rolling_breakpoints`。

未做（本次范围外）：`prompt_cache_retention = "1h"` → `cache_control.ttl` 透传，仍固定 `{"type":"ephemeral"}`（5m）。
