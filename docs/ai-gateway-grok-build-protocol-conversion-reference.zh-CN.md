# Grok Build 协议转换调研与 CodexHub 借鉴建议

更新时间：2026-07-16

状态：源码调研快照。本文不代表已确定的重构计划；后续实施前应以当时的 Grok Build、
Codex 和 CodexHub 最新代码重新核对。

## 1. 调研范围

本次调研源码位于：

```text
references/grok-build-main
```

重点关注以下问题：

- Grok Build 如何同时支持 OpenAI Responses、Chat Completions 和 Anthropic Messages。
- Responses 会话历史如何进入 Anthropic Messages 请求。
- Anthropic SSE 如何转换回统一的流式事件和会话历史。
- 文本、图片、工具、thinking/signature、缓存和异常工具历史如何处理。
- 哪些设计适合 CodexHub，哪些实现存在语义损失或跨 Provider 风险。

该目录是代码快照，没有独立的 Git 元数据，因此本文只能记录本地调研日期，不能可靠记录其
上游 commit。升级该参考源码后，应重新检查本文列出的文件和行为。

## 2. 核心结论

Grok Build **不是**把一份 Responses JSON 直接改写成 Anthropic JSON。

它采用统一内部会话模型：

```text
                          +-> OpenAI Responses encoder -> /v1/responses
ConversationItem[] -------+-> Chat Completions encoder -> /v1/chat/completions
                          +-> Anthropic Messages encoder -> /v1/messages

/v1/responses SSE --------+-> normalized SamplingEvent / ConversationResponse
/v1/messages SSE ---------+
                                  |
                                  v
                           ConversationItem[]
```

因此，从 Responses 模型切换到 Anthropic Messages 模型时，真实过程是：

```text
Responses response.output
  -> response_to_conversation_items()
  -> ConversationItem[] 持久化
  -> build_messages_request()
  -> Anthropic /v1/messages
```

这套设计最值得 CodexHub 借鉴的不是某个字段转换函数，而是：

> 先把不同 wire protocol 归一成协议无关 IR，再由各 Provider adapter 独立编码和解码。

CodexHub 已有 `GatewayTurn` IR 和 Responses inbound decoder，但当前生产 Anthropic 路径仍直接
消费偏 Responses 结构的 `GatewayRequest`。后续真正推进统一 adapter 时，可以参考 Grok Build
的分层方式，但不能直接替换现有转换器。

## 3. Grok Build 的代码分层

### 3.1 统一会话模型

核心文件：

- [`conversation.rs`](../references/grok-build-main/crates/codegen/xai-grok-sampling-types/src/conversation.rs)
- [`messages.rs`](../references/grok-build-main/crates/codegen/xai-grok-sampling-types/src/messages.rs)

`ConversationItem` 的主要类型：

```text
System
User
Assistant
ToolResult
BackendToolCall
Reasoning
```

关键设计：

- `Assistant` 保存普通文本和由客户端执行的函数工具调用。
- `ToolResult` 是独立会话项，并通过 `tool_call_id` 关联工具调用。
- Responses 原生 `reasoning` 是 Assistant 前面的独立 sibling，避免多个 reasoning item 被覆盖。
- Responses 服务端执行的 web search、X search、code interpreter 保存为 `BackendToolCall`。
- 用户图片和工具结果图片统一表示为 `ContentPart::Image`。

### 3.2 Provider 选择与发送

核心文件：

- [`client.rs`](../references/grok-build-main/crates/codegen/xai-grok-sampler/src/client.rs)
- [`request_task.rs`](../references/grok-build-main/crates/codegen/xai-grok-sampler/src/actor/request_task.rs)

`ApiBackend` 在发送前决定使用哪个 encoder：

```text
Responses       -> conversation_stream_responses()
Messages        -> conversation_stream_messages()
ChatCompletions -> conversation_stream()
```

Messages 请求最终发送到 `{base_url}/messages`。流式请求设置 `stream=true`，使用 SSE 读取
`message_start`、`content_block_*`、`message_delta` 和 `message_stop`。

### 3.3 响应归一化

核心文件：

- [`stream/responses.rs`](../references/grok-build-main/crates/codegen/xai-grok-sampler/src/stream/responses.rs)
- [`stream/messages.rs`](../references/grok-build-main/crates/codegen/xai-grok-sampler/src/stream/messages.rs)

两种协议最终都产生统一的：

```text
SamplingEvent::ChannelToken
SamplingEvent::ToolCallDelta
SamplingEvent::Completed
SamplingEvent::Failed
```

完成后再构造 `ConversationResponse`，并把响应写回 `ConversationItem[]` 历史。

## 4. Responses 历史到 Anthropic Messages 的映射

主要入口是 `build_messages_request()`。

| 内部会话语义 | Anthropic Messages 形态 | 备注 |
| --- | --- | --- |
| `System` | 顶层 `system[]` text block | 所有 System 都被提取到顶层 |
| `User` 文本 | `role=user` + text block | 保留内容顺序 |
| `User` data URL 图片 | base64 image source | 解析 media type 和 base64 数据 |
| `User` HTTP 图片 | URL image source | CodexHub 当前尚未支持该分支 |
| `Assistant` 文本 | `role=assistant` + text block | 可与 thinking/tool_use 合并 |
| `Assistant.tool_calls` | `tool_use` | 参数字符串解析为 JSON |
| `ToolResult` | `role=user` + `tool_result` | 多个结果可合并到同一 user message |
| 工具结果图片 | `tool_result.content[]` 内 image | 与 CodexHub 当前“提升为 sibling image”不同 |
| `Reasoning` 文本 | `thinking.thinking` | summary/content 被拼成可见 thinking |
| `Reasoning.encrypted_content` | `thinking.signature` | 没有 Provider 来源校验，存在风险 |
| `BackendToolCall` | assistant 文本摘要 | 丢失原始结构，只保留可读说明 |

### 4.1 System 和 Developer

Grok Build 的内部模型没有独立 `developer` 角色。项目指令、系统提醒等运行时注入内容，部分会被
建模为带 `synthetic_reason` 的 User item，而不是全部提升成 System。

因此它不能直接回答 Codex 高频 developer 消息该如何分级。CodexHub 现有的 Codex developer
内容分类和低优先级 User 降级逻辑仍应保留。

### 4.2 Tool call ID

Grok Build 会把工具调用 ID 中不属于以下字符集的字符替换成 `_`：

```text
[a-zA-Z0-9_-]
```

这样能减少 Anthropic 拒绝请求的概率，但不同原始 ID 可能被替换成相同 ID。CodexHub 当前选择
提前校验并报错，不应直接改成无状态字符替换。若未来需要兼容非法 ID，应使用请求级双向映射，
保证 call 和 result 使用同一映射且不发生碰撞。

### 4.3 Tool arguments

Grok Build 对工具参数执行 JSON 解析；解析失败时静默替换成 `{}`。这会让请求通过，但可能让模型
重新执行错误工具或丢失重要参数。

CodexHub 不应照搬该策略。更合适的行为是：

- 对支持字符串参数的目标协议保留原始字符串。
- 对要求 JSON object 的协议给出明确转换错误。
- 只有在有可靠修复规则时才自动修复，并记录日志。

### 4.4 Tool choice

Grok Build 把 `ConversationToolChoice::None` 映射成 Anthropic `Auto`。这不是等价转换，不能作为
CodexHub 的通用规则。

### 4.5 Prompt cache

Grok Build 只在最后一个 system block 上添加：

```json
{"cache_control":{"type":"ephemeral"}}
```

没有在会话尾部增加滚动断点。CodexHub 当前已经同时处理 system 和最后一条可缓存会话消息，
缓存策略比该实现更完整，不需要回退。

## 5. Anthropic SSE 到统一响应的映射

Grok Build 为每个 Anthropic content block index 维护独立 accumulator：

| Anthropic SSE | 统一结果 |
| --- | --- |
| `text_delta` | Text channel token，并累积 Assistant 文本 |
| `thinking_delta` | Reasoning channel token |
| `signature_delta` | 保存到 reasoning `encrypted_content` |
| `input_json_delta` | ToolCallDelta，并累积完整 JSON 参数 |
| `content_block_stop(tool_use)` | 生成内部 ToolCall |
| `message_delta.usage` | 更新 token 和 cache usage |
| `message_stop` | 结束当前流 |

Token 统计采用：

```text
prompt_tokens = input_tokens
              + cache_read_input_tokens
              + cache_creation_input_tokens

cached_prompt_tokens = cache_read_input_tokens
```

这个口径与 CodexHub 当前日志统计方向一致。

### 5.1 Stop reason

Grok Build 显式处理：

| Anthropic stop reason | 内部处理 |
| --- | --- |
| `end_turn` | Stop |
| `tool_use` | ToolCalls |
| `max_tokens` | Length / 终止错误 |
| `refusal` | ContentFilter，保留已流式输出 |
| `pause_turn` | 当作 Stop，不自动续发 |
| `model_context_window_exceeded` | Length |
| 未知值 | 记录 warning 后按 Stop 完成 |

CodexHub 当前主要只特殊处理 `max_tokens`。后续可以借鉴其 refusal、pause_turn、未知 stop reason
测试，但需要先定义它们在 Responses 协议中的准确终态，不能直接复制内部枚举映射。

### 5.2 Grok Build 流式实现的缺口

该 Messages 类型和流式转换没有完整覆盖：

- `redacted_thinking`
- `server_tool_use`
- `web_search_tool_result`
- `citations_delta`
- 多种 Provider 私有扩展事件

另外，`content_block_start(tool_use)` 中的非空初始 `input` 没有完整进入 accumulator，主要依赖后续
`input_json_delta`。CodexHub 已经处理初始 input，不应退回该实现。

## 6. 工具历史修复

Grok Build 的会话层提供了独立 `repair_history()`，分三步处理严格 Provider 的工具配对要求：

1. 删除重复 `ToolResult`。
2. 删除没有紧邻所属 Assistant 的孤儿或错位 `ToolResult`。
3. 为没有结果的 `tool_call` 插入合成失败结果。

这部分比 `build_messages_request()` 本身更有参考价值。转换器不会自动修复任意错误历史，修复发生在
统一会话层。

CodexHub 的 Chat Completions 路径已有孤儿工具降级和配对修复，但 Anthropic 请求路径目前主要负责
分组和格式转换，没有同等完整的请求级历史修复。未来应优先在统一 IR 层实现一次，而不是分别在
Chat 和 Anthropic adapter 内重复实现。

## 7. 跨 Provider reasoning/signature 风险

这是 Grok Build 实现中最不适合照搬的部分。

Responses 响应会把 OpenAI/xAI `encrypted_content` 保存到统一 `Reasoning`；构造 Anthropic 请求时，
该字段会直接变成 `thinking.signature`。反方向也一样：Anthropic signature 被保存回通用
`encrypted_content`，随后可能被发送给 Responses Provider。

这意味着内部类型没有记录密文属于哪个协议和 Provider。

Grok Build 在切换到更小上下文模型并达到阈值时会主动压缩，但它不会因为“Responses 切换到
Messages”就保证压缩。因此，不能把压缩视为可靠的跨 Provider 密文清理机制。

CodexHub 当前 `EncryptedContentScope` 的方向更安全：

- Anthropic signature/redacted data 带协议和 Provider 作用域。
- 只向匹配的 Anthropic Provider 解包回放。
- 外来或无标记私有内容不会伪装成 Anthropic signature。
- OpenAI 原生 Responses 密文可以继续保持原生处理策略。

该机制应继续保留，不能为对齐 Grok Build 而移除。

## 8. 与 CodexHub 当前能力对比

| 维度 | Grok Build | CodexHub 当前实现 | 判断 |
| --- | --- | --- | --- |
| 统一 IR | 生产主链路使用 `ConversationItem` | `GatewayTurn` 已存在，但 Anthropic 生产路径尚未接入 | 借鉴其落地方式 |
| Responses Lite 工具 | 内部模型表达有限 | namespace、custom、tool_search 已支持 | CodexHub 更完整 |
| 工具名回解 | 基本直接使用名称 | `ToolNameMap` 支持 namespace 和回解 | CodexHub 更完整 |
| 私有密文 | 无 Provider 作用域 | `EncryptedContentScope` | CodexHub 更安全 |
| Prompt cache | system 尾部单断点 | system + 会话尾部滚动断点 | CodexHub 更完整 |
| 用户 URL 图片 | 支持 | 目前只支持 data URL | 可借鉴 |
| 工具结果图片 | 嵌入 `tool_result.content` | 提升为同一 user message 的 sibling image | 必须实测，不能直接改 |
| SSE 文本/工具 | 支持 | 支持 | 基本一致 |
| redacted thinking | 未完整支持 | 支持并区分类型 | CodexHub 更完整 |
| citations | 未完整支持 | 支持 Responses annotation | CodexHub 更完整 |
| Anthropic server search | 降级为文本摘要 | 支持结构化 web search call | CodexHub 更完整 |
| 工具历史修复 | 会话层三阶段修复 | Chat 有，Anthropic 尚不完整 | 借鉴统一修复层 |
| Stop reason | refusal/未知值测试较完整 | 主要处理 max_tokens | 可补测试和语义 |

## 9. 建议借鉴优先级

### P0：只补测试和明确缺口

- 增加 Anthropic 请求历史中的孤儿、错位、重复和未完成工具配对测试。
- 增加 HTTP/HTTPS 用户图片的映射测试，明确支持或显式拒绝，避免静默丢图片。
- 增加 `refusal`、`pause_turn`、`model_context_window_exceeded` 和未知 stop reason 测试。
- 保持并扩展跨 Provider encrypted reasoning 不串用的测试。
- 增加 `content_block_start(tool_use)` 携带非空 input、但没有 delta 的流式测试。

### P1：完成统一 IR 的生产接入

后续单独立项，把当前：

```text
GatewayRequest -> Anthropic request builder
```

逐步替换为：

```text
Responses wire
  -> GatewayTurn
  -> Anthropic encoder
```

建议采用双编码比对，而不是一次性替换：

1. 同一测试 fixture 同时运行旧 builder 和新 IR encoder。
2. 对可等价字段做结构化 diff。
3. 对有意变化的字段建立显式 allowlist 和设计说明。
4. 覆盖历史会话、图片、并行工具、Responses Lite、thinking 和搜索后再切生产路径。

### P2：统一历史修复

在 `GatewayTurn` 层实现 Provider 无关的工具历史验证报告，再由 adapter 选择策略：

```text
valid       -> 原样发送
orphan      -> 降级为 user 上下文或拒绝
dangling    -> 过滤未完成调用、补合成结果或拒绝
displaced   -> 重排仅限语义可证明安全的情况
duplicate   -> 去重
```

网关是无状态转发层，是否补合成结果比 Grok Build 会话运行时更敏感。默认策略需要显式、可记录，
不能静默改变历史。

## 10. 明确不采用的实现

- 不把任何来源的 `encrypted_content` 直接当 Anthropic signature。
- 不在工具参数 JSON 解析失败时静默替换成 `{}`。
- 不用简单字符替换生成可能碰撞的 tool call ID。
- 不把 `tool_choice=none` 无提示改成 `auto`。
- 不把结构化 web search、X search、code interpreter 一律降级为普通 assistant 文本。
- 不用 Grok Build 的 system-only cache 策略覆盖 CodexHub 当前缓存实现。
- 不因 413 自动无提示删除图片后重试；若未来需要该恢复能力，必须在日志中明确记录语义降级。
- 不直接改回 Grok Build 的嵌套工具结果图片形态；先用真实 Anthropic 和兼容上游做 A/B 验证。

## 11. 继续调研时的入口

Grok Build：

- 统一会话类型和协议 encoder：
  `crates/codegen/xai-grok-sampling-types/src/conversation.rs`
- Anthropic wire types：
  `crates/codegen/xai-grok-sampling-types/src/messages.rs`
- Anthropic HTTP client：
  `crates/codegen/xai-grok-sampler/src/client.rs`
- Anthropic SSE 归一化：
  `crates/codegen/xai-grok-sampler/src/stream/messages.rs`
- 会话修复：
  `crates/codegen/xai-chat-state/src/compaction_utils.rs`
- 自定义 Provider 配置说明：
  `crates/codegen/xai-grok-pager/docs/user-guide/11-custom-models.md`

CodexHub：

- 目标 IR：[`ir.rs`](../src/ai_gateway/ir.rs)
- Responses decoder：[`responses_inbound.rs`](../src/ai_gateway/codec/responses_inbound.rs)
- Anthropic 请求转换：
  [`request_content.rs`](../src/ai_gateway/providers/anthropic_messages/request_content.rs)
- Anthropic 工具转换：
  [`request_tools.rs`](../src/ai_gateway/providers/anthropic_messages/request_tools.rs)
- Anthropic SSE 状态机：
  [`stream_state.rs`](../src/ai_gateway/providers/anthropic_messages/stream_state.rs)
- Provider 私有状态：
  [`encrypted_content.rs`](../src/ai_gateway/encrypted_content.rs)

相关设计文档：

- [`ai-gateway-provider-adapter-design.zh-CN.md`](ai-gateway-provider-adapter-design.zh-CN.md)
- [`ai-gateway-anthropic-messages.zh-CN.md`](ai-gateway-anthropic-messages.zh-CN.md)
- [`ai-gateway-provider-private-state-conversion.zh-CN.md`](ai-gateway-provider-private-state-conversion.zh-CN.md)
- [`ai-gateway-encrypted-content-scope.zh-CN.md`](ai-gateway-encrypted-content-scope.zh-CN.md)

## 12. 许可证说明

Grok Build 根目录声明第一方代码采用 Apache-2.0。架构思路和测试场景可以参考；若未来直接复制或
改写具体实现，应按其 `LICENSE` 和相关 notice 要求保留版权、许可证及修改说明。第三方或 vendored
代码仍需单独核对各自许可证。
