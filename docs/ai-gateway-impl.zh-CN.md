# AI Gateway 实现细节

更新时间：2026-06-15

本文档基于 [架构设计](ai-gateway-architecture.zh-CN.md) 的约束，参考 AxonHub（`references/axonhub-unstable`）的
Inbound/Outbound Transformer 架构，记录 `codex-remote` AI Gateway 的 Rust 实现细节和逐步计划。

---

## 1. 参考项目

| 项目 | 参考内容 |
|---|---|
| **AxonHub** `llm/transformer/` | Inbound/Outbound 双向 Transformer 架构、Responses API 完整数据模型和 SSE 流状态机、DeepSeek 兼容处理、Codex header 提取 |
| **codex-relay** (Rust) | Rust 实现蓝本：SSE 事件序列化、tool call delta 积累、`previous_response_id` session store |
| **codex-bridge** (Node.js) | 多 provider 路由策略、reasoning effort 六级映射表、LRU session store |
| **axonhub fork (dev)** [`doubaoyui/axonhub`](https://github.com/doubaoyui/axonhub/tree/dev) | 成熟的 DeepSeek 严格约束处理：reasoning_content 回传、developer→system 转换、thinking 参数清理、无效 assistant 消息过滤 |

AxonHub 的 `llm/transformer/openai/responses/` 是当前找到的最完整的 Responses API 转换实现，覆盖
function_call、custom_tool_call、reasoning、web_search_call、image_generation_call、compaction 等全部 item 类型。
本项目的 Rust 实现以该目录为权威参考，第一阶段只实现其中必要的子集。

---

## 2. 核心架构：双向 Transformer Pipeline

参考 AxonHub `llm/transformer/interfaces.go` 的 `Inbound` / `Outbound` 接口拆分。

```text
Codex (POST /ai-gateway/v1/responses)
  │
  ▼
InboundTransformer          ← 解析 Responses API 请求 → 统一 GatewayRequest
  │
  ▼
ProviderRouter              ← 按 model 名筛选 provider，按 session_id 做 HRW 粘性选择
  │
  ├─ OpenAI Responses ──▶ OutboundOpenAI   ← 补齐 cache 字段后透传
  │
  └─ DeepSeek Chat ─────▶ OutboundDeepSeek ← Responses→Chat 转换
  │
  ▼
上游 Provider HTTP/SSE
  │
  ▼
OutboundTransformer         ← 解析上游响应/SSE → 统一 GatewayResponse
  │
  ▼
InboundTransformer          ← 统一 GatewayResponse → Responses API HTTP/SSE
  │
  ▼
Codex (SSE 事件流)
```

### 2.1 与 AxonHub 的区别

| AxonHub | codex-remote AI Gateway |
|---|---|
| Go，独立服务，多租户、计费、RBAC | Rust，内嵌到 codex-remote daemon |
| 通用 LLM 网关，支持 embedding/image/audio/video | 只服务 Codex `/responses` |
| Pipeline 有 retry/failover/middleware | Phase 1 不做 retry |
| 完整 Responses WebSocket outbound | Phase 1 只做 HTTP/SSE |
| `llm.Request` 是全量中间格式 | `GatewayRequest` 只保留 Responses 相关字段 |

---

## 3. 统一中间格式

参考 AxonHub `llm/model.go` 的 `Request` / `Response` 和 `responses/model.go` 的 `Item` 类型。

### 3.1 GatewayRequest

```rust
pub struct GatewayRequest {
    pub model: String,
    pub instructions: Option<String>,
    pub input: Vec<ResponseItem>,
    pub tools: Vec<serde_json::Value>,
    pub tool_choice: Option<serde_json::Value>,
    pub reasoning: Option<Reasoning>,
    pub text: Option<TextOptions>,
    pub stream: bool,
    pub max_output_tokens: Option<i64>,

    // cache
    pub prompt_cache_key: Option<String>,
    pub prompt_cache_retention: Option<String>,
    pub previous_response_id: Option<String>,

    // 保留原始 body 用于 OpenAI 透传
    pub raw_body: serde_json::Value,
}
```

### 3.2 GatewayContext

从 HTTP header 提取，参考 AxonHub `codex/headers.go`。

```rust
pub struct GatewayContext {
    pub request_id: String,
    pub session_id: Option<String>,    // Session_id header
    pub thread_id: Option<String>,     // thread-id header
    pub window_id: Option<String>,     // X-Codex-Window-Id
    pub prompt_cache_key: String,      // 最终确定的 cache key
}
```

Cache key 确定优先级（参考架构文档第 7 节）：

1. body `prompt_cache_key`
2. header `Session_id`
3. header `session-id`
4. header `thread-id`
5. `X-Codex-Turn-Metadata` JSON 的 `session_id`
6. fallback `codex-remote:<uuid>`

### 3.3 ResponseItem（Responses API Item 子集）

参考 AxonHub `responses/model.go` 的 `Item` 结构。Phase 1 只需要以下 type：

| type | 方向 | 说明 |
|---|---|---|
| `message` | in/out | 带 role + content 的消息 |
| `input_text` | in | 用户文本 |
| `input_image` | in | 用户图片 |
| `function_call` | in/out | 工具调用 |
| `function_call_output` | in | 工具结果 |
| `reasoning` | in/out | 推理内容 |
| `output_text` | out | 嵌套在 message.content 里 |

Phase 2+ 再加 `custom_tool_call`、`web_search_call`、`image_generation_call`、`compaction` 等。

### 3.4 Responses → Chat Messages 转换规则

参考 AxonHub `responses/inbound.go` 的 `convertInputToMessages`。

```text
instructions                    → system message
input_text / message(role=user) → user message
message(role=assistant)         → assistant message
function_call                   → assistant message 的 tool_calls[]
function_call_output            → tool message (role=tool, tool_call_id=call_id)
reasoning + 后续 function_call  → 合并成单个 assistant message（含 reasoning_content + tool_calls）
input_image                     → user message 的 content[] 里 image_url part
```

关键边界条件（AxonHub 已处理）：

- reasoning item 后面紧跟 function_call 时，合并为同一个 assistant message
- 多个连续 function_call 合并到同一个 assistant message 的 tool_calls
- function_call_output 的 Output 字段为 nil 时返回错误
- input 是纯字符串时转为单条 user message

### 3.5 Chat Messages → Responses 转换规则

参考 AxonHub `responses/outbound_convert.go` 的 `convertToResponsesAPIResponse`。

```text
assistant message content       → message item (role=assistant, content=[output_text])
assistant message tool_calls[]  → function_call items
assistant reasoning_content     → reasoning item (summary=[summary_text])
assistant reasoning_signature   → reasoning item 的 encrypted_content
tool message                    → function_call_output item
finish_reason=stop              → status=completed
finish_reason=length            → status=incomplete
finish_reason=tool_calls        → status=completed
```

---

## 4. SSE 流状态机

参考 AxonHub `responses/inbound_stream.go` 的 `responsesInboundStream`。
这是最复杂的部分，必须按正确顺序生成 Responses SSE 事件。

### 4.1 事件类型全集

参考 AxonHub `responses/stream_event.go`。Phase 1 需要实现的事件标 ✓：

| 事件 | Phase 1 | 说明 |
|---|---|---|
| `response.created` | ✓ | 初始 response 对象 |
| `response.in_progress` | ✓ | 正在处理 |
| `response.completed` | ✓ | 完成（含完整 output + usage） |
| `response.failed` | ✓ | 失败 |
| `response.incomplete` | ✓ | 不完整（max tokens） |
| `response.output_item.added` | ✓ | 新增 output item |
| `response.output_item.done` | ✓ | output item 完成 |
| `response.content_part.added` | ✓ | 新增 content part |
| `response.content_part.done` | ✓ | content part 完成 |
| `response.output_text.delta` | ✓ | 文本增量 |
| `response.output_text.done` | ✓ | 文本完成 |
| `response.function_call_arguments.delta` | ✓ | 工具参数增量 |
| `response.function_call_arguments.done` | ✓ | 工具参数完成 |
| `response.reasoning_summary_part.added` | ✓ | 推理摘要开始 |
| `response.reasoning_summary_part.done` | ✓ | 推理摘要完成 |
| `response.reasoning_summary_text.delta` | ✓ | 推理文本增量 |
| `response.reasoning_summary_text.done` | ✓ | 推理文本完成 |
| `error` | ✓ | 错误 |

### 4.2 状态机设计

参考 AxonHub `responsesInboundStream` 的状态字段：

```rust
struct ResponsesStreamState {
    // 阶段标记
    response_created: bool,
    message_item_started: bool,
    reasoning_item_started: bool,
    reasoning_summary_part: bool,
    content_part_started: bool,
    finished: bool,
    response_completed: bool,

    // 元数据
    response_id: String,
    model: String,
    created_at: i64,

    // 索引
    output_index: usize,
    content_index: usize,
    sequence_number: usize,
    current_item_id: String,

    // 积累器
    accumulated_text: String,
    accumulated_reasoning: String,

    // 工具调用追踪
    tool_calls: HashMap<usize, ToolCallState>,

    // usage
    usage: Option<Usage>,

    // 事件队列
    event_queue: VecDeque<StreamEvent>,
}
```

### 4.3 Chat SSE chunk → Responses SSE 事件的映射

每收到一个 Chat Completions SSE chunk，状态机执行以下逻辑：

```text
收到第一个 chunk:
  → 生成 response_id, 记录 model/created_at
  → 发射 response.created
  → 发射 response.in_progress

chunk.choices[0].delta.reasoning_content 非空:
  IF !reasoning_item_started:
    → 发射 response.output_item.added (type=reasoning, output_index++)
    reasoning_item_started = true
  IF !reasoning_summary_part:
    → 发射 response.reasoning_summary_part.added
    reasoning_summary_part = true
  → 发射 response.reasoning_summary_text.delta

chunk.choices[0].delta.content 非空:
  IF reasoning_item_started:
    → 关闭 reasoning item（发射 summary_text.done, summary_part.done, output_item.done）
    reasoning_item_started = false
  IF !message_item_started:
    → 发射 response.output_item.added (type=message, role=assistant, output_index++)
    message_item_started = true
  IF !content_part_started:
    → 发射 response.content_part.added (type=output_text)
    content_part_started = true
  → 发射 response.output_text.delta

chunk.choices[0].delta.tool_calls 非空:
  IF reasoning_item_started:
    → 关闭 reasoning item
  FOR each tool_call in delta.tool_calls:
    IF tool_call.index 第一次出现:
      → 发射 response.output_item.added (type=function_call, output_index++)
      → 记录 ToolCallState { item_id, call_id, name, arguments }
    IF tool_call.function.arguments 非空:
      → 追加到对应 ToolCallState.arguments
      → 发射 response.function_call_arguments.delta
  finish 时:
    FOR each 已完成的 tool_call:
      → 发射 response.function_call_arguments.done
      → 发射 response.output_item.done

chunk.choices[0].finish_reason 非空:
  → 关闭所有打开的 item（content_part.done, output_text.done, output_item.done）
  finished = true

chunk.usage 非空 且 finished:
  → 构建完整 Response 对象
  → 发射 response.completed
  response_completed = true
```

### 4.4 每个事件的 JSON 结构

参考 AxonHub `stream_event.go` 的 `StreamEvent`：

```rust
struct StreamEvent {
    #[serde(rename = "type")]
    event_type: String,
    sequence_number: usize,

    // 以下字段按事件类型选择性填充
    #[serde(skip_serializing_if = "Option::is_none")]
    response: Option<ResponseObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    item: Option<ResponseItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    part: Option<ContentPart>,
    #[serde(skip_serializing_if = "Option::is_none")]
    delta: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    // function_call 字段
    #[serde(skip_serializing_if = "Option::is_none")]
    item_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    arguments: Option<String>,
}
```

---

## 5. Provider 实现

### 5.1 OpenAI Responses 出站

参考 AxonHub `responses/outbound.go`。

主路径是**透传**，只做以下调整：

1. 补齐 `prompt_cache_key`（从 GatewayContext 取稳定值）
2. 补齐 `prompt_cache_retention`（如配置了 `promptCacheRetention`）
3. 注入 API key（从 gateway 配置取，不暴露给 Codex 侧）
4. 透传 Codex header（`Session_id`、`X-Codex-Turn-Metadata`、`X-Codex-Window-Id`、`X-Client-Request-Id`）
5. 上游 SSE 直接透传给 Codex，不做事件转换

```rust
async fn openai_responses_passthrough(
    ctx: &GatewayContext,
    raw_body: serde_json::Value,
    config: &ProviderConfig,
) -> Result<Response<Body>> {
    let mut body = raw_body;
    // 补齐 cache 字段
    if body.get("prompt_cache_key").and_then(|v| v.as_str()).unwrap_or("").is_empty() {
        body["prompt_cache_key"] = json!(ctx.prompt_cache_key);
    }
    if let Some(retention) = &config.prompt_cache_retention {
        body["prompt_cache_retention"] = json!(retention);
    }
    // 透传到上游
    proxy_to_upstream(config.base_url, "/v1/responses", body, config.api_key).await
}
```

### 5.2 DeepSeek Chat 出站

参考 AxonHub `deepseek/outbound.go` 和 `openai/outbound_convert.go`。

#### 5.2.1 请求转换

```text
GatewayRequest → Chat Completions Request:

1. instructions → messages[0] = {role: "system", content: instructions}
2. input items → messages[] (按 3.4 节规则)
3. tools[] → tools[] (只保留 type=function 的)
4. tool_choice → tool_choice
5. reasoning effort 映射（`normalize_deepseek_effort()`）:
   - effort="none" → thinking={type: "disabled"}, 不发 reasoning_effort
   - effort="low"/"medium"/"minimal" → "high"（DeepSeek 最低只支持 high）
   - effort="xhigh" → "max"
   - 其它（"high"/"max"） → 原值透传
   - thinking 启用时 → thinking={type: "enabled"}, reasoning_effort 透传
6. text.format（`apply_response_format()`）:
   - json_schema → 降级为 json_object (DeepSeek 不支持 json_schema)
7. thinking 启用时的 assistant 消息处理（`pad_reasoning_content()`）:
   - 所有 assistant message 缺少 reasoning_content 的补空字符串
8. max_output_tokens → max_tokens (Chat Completions 字段名)
9. developer 角色转换（`normalize_developer_messages()`）:
   - role="developer" → role="system"（DeepSeek 不支持 developer 角色）
10. 无效 assistant 消息过滤（`drop_invalid_assistant_messages()`）:
    - 删除 content 和 tool_calls 都为空的 assistant 消息（reasoning-only 消息会被 DeepSeek 拒绝）
11. tool_calls 的 reasoning_content 回传（`ensure_thinking_tool_call_reasoning_content()`）:
    - 当 assistant 消息有 tool_calls 但缺少 reasoning_content 时，从最近的含 reasoning_content 的 assistant 消息回填
    - 这是 DeepSeek 的严格要求：含 tool_calls 的 assistant 消息必须有 reasoning_content，否则返回 400
12. thinking 启用时参数清理:
    - 移除 temperature、top_p、presence_penalty、frequency_penalty（DeepSeek thinking 模式下不允许这些参数）
13. Codex 专属工具类型过滤:
    - tools[] 只保留 type="function" 的工具
    - web_search、image_generation 等 Codex 专属工具类型静默过滤（通过 ItemType `#[serde(other)]` Unknown 变体）
```

#### 5.2.2 响应转换（非流式）

```text
Chat Completions Response → Responses API Response:

1. 生成 gateway response id (gwresp_xxx)
2. choices[0].message.content → output[]: message item
3. choices[0].message.tool_calls → output[]: function_call items
4. choices[0].message.reasoning_content → output[]: reasoning item
5. usage → usage (字段映射)
6. finish_reason → status
```

#### 5.2.3 响应转换（流式）

Chat SSE chunk → 状态机 → Responses SSE 事件（按第 4 节状态机处理）。

---

## 6. Provider 路由

按显式模型列表筛选候选 provider，不做默认渠道、兜底渠道或 provider name 前缀猜测。

当多个已启用 provider 都显式支持同一个 model 时，使用 `session_id` 做 Rendezvous/HRW Hash 粘性选择：

```text
candidates = enabled providers where provider.models contains request.model
score = sha256("codex-remote-ai-gateway-hrw-v1", session_id, provider_route_id)
selected_provider = candidate with max(score)
```

这样同一个 `session_id + model` 在配置不变时会稳定落到同一个上游；provider 增删时，未落在受影响 provider 上的 session 通常保持原路由。没有 `session_id` 时保持旧行为，返回配置顺序里的第一个匹配 provider。

---

## 7. 错误处理

参考 AxonHub `responses/inbound.go` 的 `TransformError`。

Gateway 返回 Responses API 格式的错误：

```json
{
  "error": {
    "message": "upstream provider returned 429: rate limit exceeded",
    "type": "upstream_error",
    "code": "rate_limit_exceeded"
  }
}
```

错误类型映射：

| 场景 | HTTP status | error.type |
|---|---|---|
| 请求解析失败 | 400 | `invalid_request_error` |
| model 不在任何 provider | 422 | `invalid_model_error` |
| 上游超时 | 504 | `upstream_timeout` |
| 上游 4xx/5xx | 上游 status | `upstream_error` |
| 流式中断 | — | SSE `error` 事件 |

---

## 8. 模块结构

```text
src/ai_gateway.rs                         ← mod 声明 + axum 路由挂载
src/ai_gateway/
    config.rs                             ← [aiGateway] 配置解析
    context.rs                            ← GatewayContext 提取
    error.rs                              ← 错误类型和 Responses 格式错误响应
    model.rs                              ← GatewayRequest, ResponseItem, StreamEvent 等类型
    router.rs                             ← provider 路由选择
    handler.rs                            ← POST /ai-gateway/v1/responses 处理入口
    providers/
        mod.rs
        openai_responses.rs               ← OpenAI Responses 透传
        deepseek_chat.rs                  ← DeepSeek Chat 出站
    transform/
        mod.rs
        responses_to_chat.rs              ← Responses input → Chat messages（含 DeepSeek 严格约束处理）
        chat_to_responses.rs              ← Chat response → Responses response
        responses_stream.rs               ← Chat SSE → Responses SSE 状态机（含 tool_calls 流转换）
```

### 8.1 与现有代码的集成点

- `src/web.rs`：新增 `/ai-gateway/v1/responses` 路由
- `src/config.rs`：新增 `AiGatewayConfig` 段落解析
- 不修改 `remote_control_backend/`、`im/`、`bridge.rs`

---

## 9. 逐步实现计划

### Step 1：骨架和配置 ✅

创建文件：
- `src/ai_gateway.rs` — mod 声明
- `src/ai_gateway/config.rs` — `AiGatewayConfig` / `ProviderConfig` 解析
- `src/ai_gateway/handler.rs` — axum handler 骨架，返回 501
- `src/ai_gateway/error.rs` — `GatewayError` 和 Responses 格式错误响应

集成：
- `src/config.rs` 新增 `ai_gateway` 可选段
- `src/web.rs` 挂载 `/ai-gateway/v1/responses` 路由

验收：
- `cargo build` 通过
- `POST /ai-gateway/v1/responses` 返回 501 + Responses 格式错误

### Step 2：数据模型和 Context 提取 ✅

创建文件：
- `src/ai_gateway/model.rs` — `GatewayRequest`、`ResponseItem`、`Reasoning`、`TextOptions`、`ResponseObject`、`Usage`
- `src/ai_gateway/context.rs` — 从 header 提取 `GatewayContext`（含 `prompt_cache_key` 多级提取）
- `src/ai_gateway/router.rs` — `select_provider()`

> 注：`cache.rs` 未单独创建，`prompt_cache_key` 提取逻辑合并在 `context.rs` 中。

验收：
- handler 能解析请求 body 和 header
- 日志输出 model、session_id、prompt_cache_key、选中的 provider

### Step 3：OpenAI Responses 透传 ✅

创建文件：
- `src/ai_gateway/providers/openai_responses.rs`

实现：
- 补齐 `prompt_cache_key` 和 `prompt_cache_retention`
- 注入上游 API key
- 透传 Codex header
- 流式：直接代理上游 SSE 到 Codex
- 非流式：直接代理上游 JSON 到 Codex

验收：
- Codex 配置 `base_url = http://127.0.0.1:3847/ai-gateway/v1`，model=gpt-5.5
- 普通对话可用，SSE 流式正常
- 日志可见 prompt_cache_key

### Step 4：Responses → Chat 请求转换 ✅

创建文件：
- `src/ai_gateway/transform/responses_to_chat.rs`

实现（参考 AxonHub `responses/inbound.go` 的 `convertInputToMessages`）：
- instructions → system message
- input items 遍历，按 type 分发
- message(role=user) → user message
- message(role=assistant) → assistant message
- function_call → assistant tool_calls
- function_call_output → tool message
- reasoning + 后续 function_call 合并
- input_image → image_url content part
- text.format json_schema → 降级为 json_object

验收：
- 单元测试覆盖所有 item type 转换
- 多轮对话（含 function_call + function_call_output）序列化正确

### Step 5：DeepSeek Chat 出站 — 非流式 ✅

创建文件：
- `src/ai_gateway/providers/deepseek_chat.rs`
- `src/ai_gateway/transform/chat_to_responses.rs`

实现：
- 请求转换：GatewayRequest → Chat Completions JSON
- DeepSeek 特殊处理：thinking 参数、json_schema 降级、reasoning_content 补空
- 响应转换：Chat Completions Response → Responses API Response
- 生成 gateway response id

验收：
- Codex model=deepseek-v4-flash，非流式对话可用
- 响应格式符合 Responses API

### Step 6：Chat SSE → Responses SSE 流状态机 ✅

创建文件：
- `src/ai_gateway/transform/responses_stream.rs`

实现（参考 AxonHub `responses/inbound_stream.go` 的状态机）：
- `ResponsesStreamState` 结构体
- Chat SSE chunk 解析
- 第 4 节描述的完整状态机逻辑
- response.created → output_text.delta → response.completed 完整链路
- reasoning_content → reasoning 事件生成
- 错误事件处理

验收：
- Codex model=deepseek-v4-flash，流式对话可用
- SSE 事件顺序正确，Codex 能正常渲染流式输出
- reasoning 模型的推理过程正确显示

### Step 7：工具调用转换 ✅

> 注：工具调用已在 Step 4-6 中一并实现，未单独拆分阶段。

已完成：
- `responses_to_chat.rs`：function_call → Chat tool_calls，reasoning + function_call 合并
- `deepseek_chat.rs`：Chat tool_calls delta 积累
- `responses_stream.rs`：function_call_arguments.delta/done 事件生成，ToolCallState 追踪
- `chat_to_responses.rs`：Chat tool_calls → function_call items

验收：
- DeepSeek 能触发 Codex 工具调用
- 工具结果回填后 DeepSeek 继续回答
- 流式工具调用事件正确

### Step 8：GET /models 和健壮性（约 1 天）

新增路由：
- `GET /ai-gateway/v1/models` — 返回 `aiGateway.codexVisibleModels` 白名单中、且在内置 catalog 里可见的 model 列表，并附带 `ETag`

完善：
- 上游超时处理
- 上游连接错误处理
- SSE 流中断恢复
- 请求/响应日志（model、provider、latency、tokens）

验收：
- `/v1/models` 只返回允许给 Codex 看的 model 列表
- 上游故障时返回明确错误
- 日志可观测

### Codex 模型列表更新机制

Codex App 侧的模型列表不是“保存后立刻推送刷新”，而是按 Codex 自己的 `model/list` 链路更新：

```text
Codex App
  -> app-server model/list
  -> models-manager OnlineIfUncached
  -> models_cache.json 命中则直接返回
  -> 缓存失效后重新请求 /ai-gateway/v1/models
```

因此，保存可见模型后：

- 如果 Codex 还在用新鲜的 `models_cache.json`，最多会延迟到缓存过期，默认 300 秒。
- 如果 Codex 收到一次 `/responses` 且 `x-models-etag` 变化，会触发一次 `/models` 刷新。
- 如果要稳定看到最新列表，退出 Codex App、删除 `models_cache.json`、再启动最直接。

---

## 10. 后续阶段提要

### Phase 3：Ledger 和跨 provider 状态

- Gateway-owned response id（`gwresp_xxx`）
- `GatewayResponseRecord` 存储（SQLite）
- provider-local branch state
- cache prefix hash 调试日志

### Phase 4：WebSocket

参考 AxonHub `responses/outbound.go` 的 `websocket_executor.go`：
- Codex Responses WebSocket inbound
- OpenAI Responses WebSocket outbound
- WebSocket delta input 还原（依赖 ledger）
- WebSocket fallback 到 HTTP/SSE

---

## 11. 关键文件对照表

| 本项目文件 | AxonHub 参考文件 |
|---|---|
| `ai_gateway/model.rs` | `llm/transformer/openai/responses/model.go` |
| `ai_gateway/context.rs` | `llm/transformer/openai/codex/headers.go` |
| `ai_gateway/transform/responses_to_chat.rs` | `llm/transformer/openai/responses/inbound.go` (`convertInputToMessages`) |
| `ai_gateway/transform/chat_to_responses.rs` | `llm/transformer/openai/responses/inbound.go` (`convertToResponsesAPIResponse`) |
| `ai_gateway/transform/responses_stream.rs` | `llm/transformer/openai/responses/inbound_stream.go` |
| `ai_gateway/providers/openai_responses.rs` | `llm/transformer/openai/responses/outbound.go` |
| `ai_gateway/providers/deepseek_chat.rs` | `llm/transformer/deepseek/outbound.go` |
| `ai_gateway/config.rs` | 架构文档第 4 节 |
| `ai_gateway/context.rs` (cache 部分) | 架构文档第 7 节 + `codex/headers.go` |
| `ai_gateway/router.rs` | codex-bridge 路由逻辑 |
| `ai_gateway/error.rs` | `llm/transformer/openai/responses/inbound.go` (`TransformError`) |

---

## 12. 测试覆盖

当前共 **67 个单元测试**，全部通过。

| 模块 | 测试数 | 覆盖范围 |
|---|---|---|
| `config.rs` | 7 | provider 路由（精确/前缀/禁用/无匹配）、TOML 反序列化 |
| `context.rs` | 8 | prompt_cache_key 6 级优先级、passthrough header 收集、无效 JSON 处理 |
| `transform/responses_to_chat.rs` | 28 | 全部 item type 转换、DeepSeek 严格约束（developer→system、reasoning effort 映射、thinking 参数清理、无效消息过滤、tool_calls reasoning_content 回填）、多轮对话、Codex 专属工具过滤 |
| `transform/chat_to_responses.rs` | 11 | 简单/reasoning/tool_calls/多 tool_calls 响应、空内容跳过、usage 详情、无 usage 字段 |
| `transform/responses_stream.rs` | 13 | 纯文本流、reasoning→text 切换、并行 tool_calls、reasoning→tool_calls、序列号单调递增、finish/completion |

运行命令：`cargo test --bin codex-remote ai_gateway`

---

## 13. 设计决策记录

| 决策 | 理由 |
|---|---|
| OpenAI 走透传而非转换 | 避免序列化损失，保留 OpenAI 原生 cache/WebSocket 能力 |
| 中间格式只保留 Responses 子集 | 不需要 embedding/image/audio 等不相关类型 |
| SSE 状态机在 gateway 内实现 | 不依赖外部库，与 codex-remote 的 tokio 运行时一致 |
| Phase 1 不做 previous_response_id ledger | Codex HTTP/SSE 模式下发完整 input，不需要 |
| reasoning effort 映射放在 DeepSeek outbound | 不同 provider 的 reasoning 参数格式不同 |
| 错误用 Responses API 格式返回 | Codex 期望的错误格式 |
