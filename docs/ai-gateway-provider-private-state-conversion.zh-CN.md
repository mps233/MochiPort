# AI Gateway 密文、签名与 Provider 私有状态转换

更新时间：2026-07-13

状态：已落地。本文档描述当前代码的实际行为，是 Provider 私有状态转换、问题排查和后续协议升级的实现依据。

相关文档：

- [`ai-gateway-encrypted-content-scope.zh-CN.md`](ai-gateway-encrypted-content-scope.zh-CN.md)：marker、渠道指纹和迁移策略的简明说明。
- [`ai-gateway-anthropic-messages.zh-CN.md`](ai-gateway-anthropic-messages.zh-CN.md)：Anthropic Messages adapter 的整体设计。
- [`ai-gateway-provider-adapter-design.zh-CN.md`](ai-gateway-provider-adapter-design.zh-CN.md)：Provider adapter 与 Gateway IR 的总体约束。

## 1. 为什么需要单独处理

Codex 只看到一个 OpenAI Responses 入口，但 CodexHub 后面可能连接不同 Provider。部分 Provider 会返回只能由原渠道继续使用的不透明状态：

| Provider 协议 | 原生字段 | 作用 |
| --- | --- | --- |
| OpenAI Responses | `reasoning.encrypted_content` | 后续 reasoning 连续性和 Provider 私有校验 |
| Grok Responses | `reasoning.encrypted_content` | Grok 后续 reasoning 连续性和私有校验 |
| Anthropic Messages | `thinking.signature` | `thinking` block 的完整性与后续工具循环回放 |
| Anthropic Messages | `redacted_thinking.data` | 不可展示但必须保持不透明的 thinking 数据 |

本文统一把这些字段称为 **Provider 私有状态**。

“密文”和“签名”不是同一个密码学概念，但对 Gateway 的处理原则相同：

1. 不解析、不修改原始内容。
2. 只允许回到产生它的协议和 Provider route。
3. 切换渠道时必须过滤。
4. CodexHub 内部 marker 绝不能发送给上游。

以下内容不属于本文处理范围：

- Anthropic web search 结果中的私有 `encrypted_content`。
- API Key、OAuth token 等身份凭证。
- Codex Responses compaction blob 的业务语义。

## 2. Codex 侧统一表示

Codex 使用 Responses `reasoning` item 保存私有状态：

```json
{
  "type": "reasoning",
  "summary": [
    {"type": "summary_text", "text": "visible thinking"}
  ],
  "encrypted_content": "opaque-provider-state"
}
```

CodexHub 的 `ResponseItem` 也使用：

```text
summary: Option<Vec<SummaryPart>>
encrypted_content: Option<String>
```

因此 Anthropic 的两个不同原生 block 最终都会经过 `encrypted_content`。仅靠该字段本身无法判断原始类型，必须在 marker 中显式保存 `thinking` 或 `redacted_thinking`。

## 3. Marker 格式

### 3.1 OpenAI 与 Grok

```text
codexhub:enc:v1:<protocol>:<footprint>:<raw encrypted content>
```

示例：

```text
codexhub:enc:v1:grok:4f3b0cb6a91e:p3HD...G1SY
```

### 3.2 Anthropic

```text
codexhub:enc:v1:anthropic:<footprint>:thinking:<raw signature>
codexhub:enc:v1:anthropic:<footprint>:redacted_thinking:<raw data>
```

Anthropic `kind` 是协议信息，不是原始签名的一部分。解包后发送给上游的仍然只有原始 `signature` 或 `data`。

### 3.3 Provider footprint

`footprint` 是 Provider route 字符串的 SHA-256 前 6 字节，编码成 12 位十六进制：

```text
provider name + provider type + base URL
```

设计理由：

- Provider 名称区分用户配置的逻辑渠道。
- Provider 类型区分 OpenAI、Grok、Anthropic 等协议。
- Base URL 区分同协议下的不同服务端。
- API Key 不参与指纹，避免泄漏凭证，并允许同一渠道轮换 Key。

修改 Provider 名称、类型或 Base URL 后，旧私有状态会被视为其他渠道的数据。

## 4. 总体转换流程

```text
上游响应
  -> 提取原生私有状态
  -> 转成 Responses reasoning item
  -> 添加协议、渠道指纹和 Anthropic block 类型
  -> 返回 Codex
  -> Codex 保存并在后续 input 中回放
  -> CodexHub 校验 marker
     -> 匹配：解包并恢复原生字段
     -> 不匹配：过滤私有状态
  -> 上游请求
```

任何方向都不依赖进程内映射或临时数据库，因此 CodexHub 重启后仍能判断状态归属。

## 5. OpenAI/Grok Responses 转换

### 5.1 上游响应到 Codex

CodexHub 递归检查 Responses JSON 或 SSE `data:` 中的 Provider 私有 item：

- `reasoning`
- `compaction`
- `compaction_summary`
- `context_compaction`

发现非空 `encrypted_content` 时添加 scope marker。已经带 `codexhub:enc:v1:` 的内容不会重复包装。

### 5.2 Codex 请求到上游

发送给当前 Responses Provider 前：

1. 协议和 footprint 完全匹配：移除 marker，恢复原始 `encrypted_content`。
2. marker 属于其他协议或渠道：删除 `encrypted_content`、Provider 私有 `id` 和 `status`。
3. 删除私有字段后仍有 `summary` 或 `content`：保留可读 reasoning 内容。
4. 删除后 item 没有可回放内容：删除整个 item。

示例：Grok 切换 OpenAI。

Codex 回放：

```json
{
  "type": "reasoning",
  "id": "rs_grok",
  "summary": [{"type": "summary_text", "text": "visible summary"}],
  "encrypted_content": "codexhub:enc:v1:grok:4f3b0cb6a91e:opaque"
}
```

发送给 OpenAI：

```json
{
  "type": "reasoning",
  "summary": [{"type": "summary_text", "text": "visible summary"}]
}
```

Grok 的密文、item id 和 status 都不会进入 OpenAI 请求。

## 6. Anthropic 响应转换

### 6.1 普通 thinking

Anthropic 响应：

```json
{
  "type": "thinking",
  "thinking": "inspect the repository",
  "signature": "sig_123"
}
```

返回 Codex：

```json
{
  "type": "reasoning",
  "summary": [
    {"type": "summary_text", "text": "inspect the repository"}
  ],
  "encrypted_content": "codexhub:enc:v1:anthropic:<footprint>:thinking:sig_123"
}
```

### 6.2 不展示内容的 thinking

Anthropic 允许 thinking 文本为空但 signature 存在：

```json
{
  "type": "thinking",
  "thinking": "",
  "signature": "sig_omitted"
}
```

CodexHub 必须保留它，并用空 `summary` 数组表达“这是 thinking，但没有可见文本”：

```json
{
  "type": "reasoning",
  "summary": [],
  "encrypted_content": "codexhub:enc:v1:anthropic:<footprint>:thinking:sig_omitted"
}
```

不能因为 summary 为空就把它转换成 `redacted_thinking`。类型由 marker 中的 `thinking` 决定。

### 6.3 Redacted thinking

Anthropic 响应：

```json
{
  "type": "redacted_thinking",
  "data": "encrypted_456"
}
```

返回 Codex：

```json
{
  "type": "reasoning",
  "encrypted_content": "codexhub:enc:v1:anthropic:<footprint>:redacted_thinking:encrypted_456"
}
```

Redacted item 不使用 `summary` 字段。这样 JSON/SSE 中间表示也能保持 block 类型，marker 则提供最终的稳定类型依据。

## 7. Anthropic 请求回放

### 7.1 Thinking 回放

只有以下条件全部满足时才恢复 `thinking`：

1. marker 协议是 `anthropic`。
2. footprint 与当前 Provider route 一致。
3. marker kind 是 `thinking`。
4. reasoning item 的 `summary` 字段存在，可以是空数组。

恢复结果：

```json
{
  "type": "thinking",
  "thinking": "summary 中按顺序拼接的文本",
  "signature": "解包后的原始 signature"
}
```

如果 marker 是 thinking，但 `summary` 字段已经丢失，CodexHub 不构造一个可能无法通过签名校验的 block，而是忽略该私有 reasoning item。

### 7.2 Redacted thinking 回放

marker kind 为 `redacted_thinking` 时恢复：

```json
{
  "type": "redacted_thinking",
  "data": "解包后的原始 data"
}
```

### 7.3 Assistant message 合并

Anthropic thinking 不是独立 role。以下 Responses 历史：

```text
reasoning
assistant message
function_call / tool_use
```

必须合并成同一条 Anthropic assistant message：

```json
{
  "role": "assistant",
  "content": [
    {"type": "thinking", "thinking": "...", "signature": "..."},
    {"type": "text", "text": "..."},
    {"type": "tool_use", "id": "toolu_123", "name": "...", "input": {}}
  ]
}
```

`tool_result` 仍属于下一条 user message。不得把 thinking、text 和同一轮 tool_use 拆成连续的多条 assistant message。

## 8. JSON 与 SSE 一致性

### 8.1 非流式 JSON

Anthropic JSON 先转换成 `ResponseObject`，再根据：

- `summary: Some(...)` -> `thinking`
- `summary: None` -> `redacted_thinking`

添加 typed marker。

这里的 summary 是否为空不影响类型；`Some([])` 仍然是 thinking。

### 8.2 流式 SSE

流式转换必须为每个 reasoning item发出完整事件顺序：

```text
response.output_item.added
可选 reasoning summary delta/done
response.output_item.done
最终 response.completed
```

规则：

- 普通 thinking 的 added/done item 带 `summary`，允许为空数组。
- redacted thinking 的 added/done item 不带 `summary`。
- 连续出现 thinking 和 redacted block 时，先关闭前一个 item，再创建下一个 item。
- 只有 signature、没有 thinking delta 时，也必须发出 `output_item.added`，不能直接发送 done。
- `response.output_item.done` 和 `response.completed.output[]` 中的 marker 必须完全一致。

最终 SSE 在返回 Codex 前通过统一 Responses compatibility stream 添加 scope marker。

## 9. 跨 Provider 行为矩阵

| 来源 | 目标 | 行为 |
| --- | --- | --- |
| OpenAI | 同一 OpenAI route | 解包并回放 |
| OpenAI | 其他 OpenAI route | 过滤 |
| OpenAI | Grok/Anthropic | 过滤 |
| Grok | 同一 Grok route | 解包并回放 |
| Grok | 其他 Grok route | 过滤 |
| Grok | OpenAI/Anthropic | 过滤 |
| Anthropic | 同一 Anthropic route | 按 kind 恢复 signature/data |
| Anthropic | 其他 Anthropic route | 过滤 |
| Anthropic | OpenAI/Grok | 过滤 |

判断依据永远是协议和 Provider route，不使用模型名称。模型 alias、同模型多渠道和负载路由都不能作为私有状态归属依据。

### 9.1 GPT-5.6 会话切换 Grok 的工具历史规范化

过滤 Provider 私有状态只解决 reasoning 密文归属，不代表其余历史 item 一定能被目标 Provider 解析。

GPT-5.6/Codex 会话可能包含 OpenAI/Codex 特有历史：

```text
custom_tool_call
custom_tool_call_output
assistant message.phase
function_call_output.output = content item array
```

Grok `ModelInput` 出站前执行以下规范化：

1. `custom_tool_call` 转成 `function_call`。
2. freeform `input` 包装成 JSON arguments 字符串：`{"input":"..."}`。
3. 删除 custom call 的 `status`。
4. `custom_tool_call_output` 转成 `function_call_output`。
5. text-only output 数组按原顺序用换行拼成字符串。
6. 含非文本内容的 output 使用 JSON 字符串保存原始结构。
7. 删除 assistant message 的 Codex 私有 `phase` 字段。

转换保持以下信息不变：

- item 顺序。
- `call_id` 配对。
- tool name。
- 工具输入和输出文本。

该规则只应用于 `ProviderType::GrokResponses`。OpenAI Responses 路径继续保留原始 `custom_tool_call` 语义。

请求日志 9668 的实际转换结果：

```text
移除 phase                    13
custom_tool_call -> function 11
custom output -> function    11
结构化 output -> string      11

最终 input：
message                      29
function_call                20
function_call_output         20
```

规范化后不再包含 `custom_tool_call`、`custom_tool_call_output`、数组型 function output 或 message `phase`。

### 9.2 GPT-5.6 Responses Lite 跨 Provider 边界

GPT-5.6 Responses Lite 需要区分两类数据，不能因为历史工具调用转换成功，就认为整个 Lite 请求已经兼容目标 Provider。

第一类是已经发生过的工具历史：

```text
custom_tool_call
custom_tool_call_output
```

当前处理如下：

| 目标 Provider | 历史调用转换 |
| --- | --- |
| OpenAI Responses | 保留原始 Responses Lite 语义 |
| Grok Responses | 转成 `function_call` / `function_call_output` |
| DeepSeek Chat Completions | 转成 assistant `tool_calls` / `role=tool` |
| Anthropic Messages（Opus、GLM 等） | 转成 `tool_use` / `tool_result` |

第二类是当前轮可用的工具注册表：

```json
{
  "type": "additional_tools",
  "role": "developer",
  "tools": []
}
```

`additional_tools` 不是普通 developer 指令，也不是历史消息，而是 Responses Lite 放在 `input` 中的工具描述载体。对不支持 Responses Lite 的目标协议，正确降级应当是：

1. 从所有 `additional_tools` item 提取 `tools`。
2. 与顶层 `tools` 合并并去重。
3. 按目标协议转换 `custom`、`namespace` 和 `function` 工具定义。
4. 从发送给上游的历史 input/messages 中删除 `additional_tools` item。
5. 保持工具名称映射与历史 `call_id` 配对一致。

当前代码的完整度如下：

| 目标 Provider | 历史 `custom_tool_call` | `additional_tools` 注册表 |
| --- | --- | --- |
| OpenAI Responses | 原生保留 | 原生保留 |
| Grok Responses | 已规范化 | 已提取并合并；`custom`/`namespace` 降级为 function，并使用可逆名称映射 |
| DeepSeek Chat Completions | 已转换 | 已在反序列化前提取并合并，由现有 Chat 工具转换器降级 |
| Anthropic Messages | 已转换 | 已在反序列化前提取并合并，由现有 Anthropic 工具转换器降级 |

当前处理位于 Provider 分发和 `GatewayRequest` 反序列化之前，因此 `additional_tools` 不会先退化成 `ItemType::Unknown`。顶层 `tools` 保持优先，后续 carrier 按出现顺序追加，重复工具去重；重复 namespace 会继续合并未出现的子工具。

Grok Responses 不接受 Codex 私有 `additional_tools` carrier，也不原生接受 `custom`/`namespace` 定义。CodexHub 会：

1. 将 custom tool 包装成参数为 `{input: string}` 的 function。
2. 将 namespace 子 function 编码成 Provider-safe 名称。
3. 用同一份名称映射重写旧历史中的 custom/namespace 调用。
4. 在 JSON、SSE `output_item` 和 `response.completed.output` 中把 Grok function call 还原为 Codex 的 custom/namespace item。

因此 `exec`、`apply_patch` 和 namespace 插件不会因为降级而被 Codex 当成错误类型的普通函数。仍需持续保留以下回归范围：

- GPT-5.6 -> Grok Responses。
- GPT-5.6 -> DeepSeek Chat Completions。
- GPT-5.6 -> Anthropic Messages/Opus。
- 同时包含顶层 `tools` 和 `additional_tools` 时不重复注册。
- custom tool 历史、当前工具定义和后续工具调用使用同一名称映射。
- `additional_tools` 不得被转换成 system、developer 或 user 文本消息。

## 10. 旧会话迁移

### 10.1 OpenAI/Grok Responses

旧会话中的无 marker `encrypted_content` 无法判断来源，当前迁移策略为：

1. 整个请求完全没有 CodexHub marker 时，第一次仍保留旧密文。
2. 上游明确返回密文校验错误时，删除所有 Provider 私有密文并只重试一次。
3. 新成功响应会写入 marker。
4. 请求一旦包含新 marker，其他无 marker 私有内容都按迁移残留过滤。

### 10.2 Anthropic

Anthropic 旧无 marker 私有状态一律忽略，不尝试回放。

原因：旧 Responses `encrypted_content` 无法可靠判断原始内容是：

- `thinking.signature`
- `redacted_thinking.data`
- OpenAI/Grok 私有密文

错误猜测 block 类型可能破坏签名校验。新的 Anthropic 响应会自动写入 typed marker，之后正常回放。

## 11. 错误恢复边界

OpenAI/Grok Responses 路径允许在明确的密文校验错误后执行一次清理重试。重试必须满足：

- 只删除 Provider 私有状态。
- 最多执行一次。
- 不改变用户消息、工具结果和普通 assistant 内容。

Anthropic 路径不猜测、不降级私有状态：

- marker 不匹配：请求构造阶段直接忽略。
- marker kind 无效：直接忽略。
- thinking marker 缺少 summary：直接忽略。
- 不把 thinking 自动改成 redacted，也不把 redacted 自动改成 thinking。

## 12. 安全与稳定性约束

必须保持以下不变量：

1. `codexhub:enc:v1:` marker 只存在于 Codex 和 CodexHub 之间。
2. 上游请求中不得出现 CodexHub marker。
3. 解包后的原始密文或签名字节必须保持不变。
4. API Key 不得进入 marker 或 footprint。
5. 不得跨 Provider route 复用 item `id`、`status` 或私有状态。
6. 不得解析或修改 opaque state 的内部内容。
7. Anthropic block kind 必须显式保存，不能根据文本是否为空猜测。
8. JSON 与 SSE 必须使用相同 scope 和 kind 规则。

## 13. 代码位置

| 文件 | 职责 |
| --- | --- |
| `src/ai_gateway/encrypted_content.rs` | marker、协议类型、footprint、Responses 请求清理和 Anthropic typed marker |
| `src/ai_gateway/responses_lite_tools.rs` | `additional_tools` 提取、顶层工具合并去重、Grok 工具定义降级和名称映射 |
| `src/ai_gateway/responses_compat.rs` | Responses JSON/SSE 密文包装，以及 Grok function call 向 Codex custom/namespace item 的回程还原 |
| `src/ai_gateway/providers/openai_responses.rs` | OpenAI/Grok 请求解包、Grok ModelInput 工具历史规范化、错误识别和单次清理重试 |
| `src/ai_gateway/handler.rs` | Provider 分发前调用通用 Lite 工具准备逻辑 |
| `src/ai_gateway/transform/responses_to_chat.rs` | Responses custom tool 历史 -> DeepSeek/OpenAI Chat `tool_calls` / tool message |
| `src/ai_gateway/providers/anthropic_messages/mod.rs` | Anthropic Provider scope 创建、JSON/SSE 返回路径接入 |
| `src/ai_gateway/providers/anthropic_messages/response.rs` | Anthropic JSON thinking/redacted -> Responses reasoning |
| `src/ai_gateway/providers/anthropic_messages/stream_reasoning.rs` | Anthropic SSE reasoning item 生命周期和类型保持 |
| `src/ai_gateway/providers/anthropic_messages/stream_state.rs` | thinking/signature/redacted SSE 事件分发 |
| `src/ai_gateway/providers/anthropic_messages/request_content.rs` | Responses reasoning -> Anthropic thinking/redacted 回放、custom tool 历史 -> `tool_use` / `tool_result` 与 assistant 合并 |
| `src/ai_gateway/providers/anthropic_messages/tests.rs` | Anthropic JSON/SSE、空 thinking、跨渠道和回放测试 |

## 14. 日志排查方法

排查密文或签名问题时对比三个位置：

1. **上游响应**：应该是 Provider 原始字段，不含 CodexHub marker。
2. **返回 Codex 的响应**：应该包含匹配 Provider route 的 marker。
3. **下一轮上游请求**：同渠道时恢复原始字段，切换渠道时私有状态消失。

启用请求日志详情时，响应和上游请求 JSON 可能包含 opaque signature/data。日志只用于协议排查，不对内容进行解码；导出、分享日志前应按敏感数据处理。

Anthropic 工具循环重点检查：

- `thinking.signature` 是否原样恢复。
- `redacted_thinking.data` 是否原样恢复。
- 空 thinking 是否仍为 `type=thinking`。
- thinking/text/tool_use 是否在同一 assistant message。
- `tool_result` 是否位于后续 user message。

发现以下情况即为实现错误：

- 上游请求中出现 `codexhub:enc:v1:`。
- Grok 密文进入 OpenAI 请求。
- Anthropic signature 被恢复为 `redacted_thinking.data`。
- 空 thinking signature 被丢弃。
- SSE 只出现 reasoning done，没有对应 added。
- `response.output_item.done` 和 `response.completed` 使用不同 marker。

## 15. 升级检查清单

Codex、OpenAI、Grok、Anthropic 或兼容厂商升级后，至少执行以下检查：

1. Codex 是否仍使用 `reasoning.encrypted_content` 回放私有状态。
2. Responses 是否新增 Provider 私有 item type。
3. Responses SSE 完整 item 是否仍出现在 `output_item.done` 和 `response.completed`。
4. Anthropic thinking block 是否仍包含 `thinking/signature`。
5. Anthropic 是否新增其它 thinking/redacted block 类型。
6. 空 thinking + signature 是否仍是合法响应形态。
7. Anthropic 工具循环是否仍要求回放原 assistant thinking block。
8. Provider route 的组成是否仍足以区分渠道。
9. 同渠道 OpenAI、Grok、Anthropic 回放测试是否通过。
10. OpenAI <-> Grok <-> Anthropic 的全部切换组合是否过滤私有状态。
11. JSON、普通 SSE 和内部 WebSearch SSE 是否使用同一 marker 规则。
12. GUI feature 编译和全量测试是否通过。
13. GPT/OpenAI 会话切换 Grok 后，custom tool 历史和结构化 output 是否已规范化。
14. GPT-5.6 `additional_tools` 是否仍是当前版本的工具注册表载体。
15. GPT-5.6 切换到 Grok、DeepSeek、Anthropic 后，当前工具定义是否完成目标协议降级。
16. `additional_tools` 是否只用于提取工具定义，没有被当成对话文本发送给模型。

## 16. 禁止的实现方式

- 只按模型名称判断密文来源。
- 把所有 Provider 私有状态统一当成 OpenAI `encrypted_content` 直接透传。
- 用 summary 是否为空区分 Anthropic thinking 和 redacted。
- 为了“尽量成功”而跨渠道保留未知密文。
- 在内存中保存密文归属并依赖进程生命周期。
- 把 Provider API Key 加入 fingerprint。
- 密文错误无限重试。
- 修改 signature/data 后再发送给上游。
- 只转换历史 `custom_tool_call`，却忽略当前请求中的 `additional_tools` 工具注册表。
- 把 `additional_tools` 序列化成 system、developer 或 user 文本消息。

本文档与代码行为不一致时，应先确认真实协议和测试结果，再同时修正实现与本文档，不能只改其中一侧。
