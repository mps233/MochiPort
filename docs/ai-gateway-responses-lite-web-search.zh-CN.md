# Codex Responses Lite 协议与 Web Search 对接避坑说明

日期：2026-07-13

状态：GPT-5.6 托管 `web_search` 兼容方案和 `/alpha/search` 透明代理已落地；默认 provider 恢复 `name = "ai-gateway" + requires_openai_auth = true`，优先保证完整模型列表、账户态和 Remote Control。原生 `web.run` 的 provider gate 暂不默认开启。

本文记录 Codex 新版 Responses Lite 请求形态、工具注册与执行边界、当前 CodexHub/Sub2API 的兼容策略，以及原生 `web.run` 的剩余验证工作。本文以本仓库 `references/codex-main`、`references/sub2api-main`、2026-07-12 的真实请求验证和 2026-07-13 的代码审查为准。

相关文档：

- [`ai-gateway-web-search-protocol.zh-CN.md`](ai-gateway-web-search-protocol.zh-CN.md)：标准 Responses 托管 `web_search` 以及 Anthropic/GLM 转换规则。
- [`ai-gateway-architecture.zh-CN.md`](ai-gateway-architecture.zh-CN.md)：AI Gateway 总体架构。
- [`codex-app-fast-startup-statsig.zh-CN.md`](codex-app-fast-startup-statsig.zh-CN.md)：Codex App 本地后端与 feature gate 兼容边界。

## 1. 当前结论

先给出当前版本必须遵守的结论：

1. GPT-5.6 Sol、Terra、Luna 的模型元数据都启用了 `use_responses_lite = true`。
2. Responses Lite 把客户端执行工具放进 `input[].additional_tools`，而不是顶层 `tools`。
3. Codex 原生 Lite 搜索工具是客户端工具 `web.run`，不是托管工具 `web_search`。
4. `web.run` 执行时会独立请求 provider 的 `/alpha/search`，不会在当前 `/responses` 请求中完成搜索。
5. CodexHub 默认 provider 使用 `name="ai-gateway"` 和 `requires_openai_auth=true`，保留账户态与完整模型列表，同时不满足 `provider.is_openai()`，因此不会触发 OpenAI 远程压缩。
6. CodexHub 已提供 `/ai-gateway/v1/alpha/search`，当前 Sub2API `0.1.152` 也已提供 `/v1/alpha/search`；搜索请求可以按 OpenAI Responses Provider 路由并透明转发。
7. 把托管 `{"type":"web_search"}` 塞进 `additional_tools.tools` 会被当前上游静默忽略。这不是 Codex 原生 Lite 结构。
8. 默认配置不会注册原生 `web.run`，GPT-5.6 继续使用顶层 hosted `web_search` 兼容路径；若实验配置已经注册原生 `web.run`，CodexHub 不会重复注入 hosted 工具。
9. 该形态是 CodexHub 的兼容扩展，不是 Codex 当前源码主动生成的原生 Lite 形态。
10. `/alpha/search` transport 已完成，但 Actor Authorization 实机会令 provider account 为 `None`，并触发 Codex App 官方 Statsig `available_models` 白名单，导致自定义模型消失，因此该 gate 方案不作为默认发布配置。

## 2. `use_responses_lite` 不是普通能力开关

最容易误判的一点，是把模型元数据里的：

```json
"use_responses_lite": true
```

理解成一个只影响后端路由的小开关。实际上它会改变整个请求的编码方式。

当前 GPT-5.6 模型：

| 模型 | `tool_mode` | `use_responses_lite` | `web_search_tool_type` |
| --- | --- | --- | --- |
| `gpt-5.6-sol` | `code_mode_only` | `true` | `text_and_image` |
| `gpt-5.6-terra` | `code_mode_only` | `true` | `text_and_image` |
| `gpt-5.6-luna` | `code_mode_only` | `true` | `text_and_image` |

来源：`src/ai_gateway/models.json`。

### 2.1 标准 Responses 请求

标准模式大致是：

```json
{
  "model": "gpt-5.4",
  "instructions": "You are Codex...",
  "input": [
    {
      "type": "message",
      "role": "user",
      "content": [{"type": "input_text", "text": "hello"}]
    }
  ],
  "tools": [
    {"type": "custom", "name": "exec"},
    {"type": "web_search", "external_web_access": true}
  ],
  "parallel_tool_calls": true,
  "stream": true
}
```

### 2.2 Responses Lite 请求

Lite 模式大致是：

```json
{
  "model": "gpt-5.6-sol",
  "input": [
    {
      "type": "additional_tools",
      "role": "developer",
      "tools": [
        {"type": "custom", "name": "exec"},
        {"type": "function", "name": "wait"},
        {"type": "function", "name": "request_user_input"}
      ]
    },
    {
      "type": "message",
      "role": "developer",
      "content": [
        {"type": "input_text", "text": "You are Codex..."}
      ]
    },
    {
      "type": "message",
      "role": "user",
      "content": [{"type": "input_text", "text": "hello"}]
    }
  ],
  "reasoning": {
    "context": "all_turns"
  },
  "parallel_tool_calls": false,
  "stream": true
}
```

请求头还会增加：

```http
x-openai-internal-codex-responses-lite: true
```

### 2.3 当前源码中已确认的差异

| 行为 | 标准 Responses | Responses Lite |
| --- | --- | --- |
| 基础指令 | 顶层 `instructions` | `input` 前缀中的 `role=developer` message |
| 工具定义 | 顶层 `tools` | `input[].type=additional_tools` |
| 托管工具 | 可以生成 | Codex 当前主动排除 |
| `parallel_tool_calls` | 跟随模型能力 | 强制为 `false` |
| reasoning context | 省略，使用服务端默认 | 显式 `all_turns` |
| 图片 detail | 保留 | Codex 会剥离 detail |
| Lite 请求头 | 无 | `x-openai-internal-codex-responses-lite: true` |

关键源码：

- `references/codex-main/codex-rs/core/src/client.rs`
  - `build_responses_request`
  - `build_reasoning`
  - `add_responses_lite_header`
- `references/codex-main/codex-rs/core/src/client_common.rs`
  - `get_formatted_input_for_request`
  - `strip_image_details`

因此，修改 `use_responses_lite` 会同时改变 instructions、tools、reasoning、并行工具调用和图片输入，不应当只为了恢复一个工具就随意切换。

## 3. `additional_tools` 的真实语义

`additional_tools` 是一个 Responses Lite input item：

```json
{
  "type": "additional_tools",
  "role": "developer",
  "tools": [...]
}
```

它承载的是客户端执行工具规格，例如：

- `exec`
- `wait`
- `request_user_input`
- `namespace` 工具
- MCP/插件提供的客户端工具
- 原生 Lite 搜索工具 `web.run`

### 3.1 不要把 `additional_tools` 当普通 developer message

它虽然带有 `role=developer`，但不是一条文本指令。转换器必须首先根据 `type=additional_tools` 识别它，不能：

- 转成普通 system/developer 文本。
- 合并进用户消息。
- 删除 `tools` 内部的 `namespace`。
- 改变它在 Lite input 前缀中的顺序。
- 把它持久化成会话中的自然语言消息。

Lite 请求中频繁出现 developer role，并不代表用户或模型反复插入了高优先级系统指令。前两个 item 往往只是工具声明和基础 instructions。

### 3.2 工具规格不等于工具执行器

模型看见工具定义，不代表 Codex App 一定有对应执行器。

Codex 工具计划分为两部分：

```text
model_visible_specs：发给模型看的规格
ToolRegistry：客户端真正可以执行的 runtime
```

客户端 `function_call` / `custom_tool_call` 会进入 `ToolRegistry` 查找执行器。若只在请求中伪造一个函数：

```json
{
  "type": "function",
  "name": "web_search"
}
```

模型可能调用它，但 Codex 本地没有同名 runtime，最终会得到 unsupported/unknown tool 错误。

关键源码：

- `references/codex-main/codex-rs/core/src/tools/spec_plan.rs`
- `references/codex-main/codex-rs/core/src/tools/registry.rs`
- `references/codex-main/codex-rs/core/src/tools/router.rs`

### 3.3 托管 `web_search` 不需要进入 ToolRegistry

标准 Responses 托管工具：

```json
{"type":"web_search"}
```

由上游执行。上游返回的是：

```json
{
  "type": "web_search_call",
  "id": "ws_...",
  "status": "completed",
  "action": {
    "type": "search",
    "query": "..."
  }
}
```

`WebSearchCall` 不进入本地工具分发，而是直接映射成 Codex UI/历史中的 WebSearch item。

关键源码：

- `references/codex-main/codex-rs/core/src/event_mapping.rs`
- `references/codex-main/codex-rs/core/src/stream_events_utils.rs`
- `references/codex-main/codex-rs/protocol/src/models.rs`

## 4. 两种完全不同的 Web Search

不要因为名字相近，就把 `web_search` 和 `web.run` 当成同一个协议。

### 4.1 标准 Responses 托管 `web_search`

请求：

```http
POST /v1/responses
```

```json
{
  "tools": [
    {
      "type": "web_search",
      "external_web_access": true,
      "search_content_types": ["text", "image"]
    }
  ]
}
```

执行者：Responses 上游。

返回：Responses SSE 中的 `web_search_call` 过程事件、最终 message 和 citations。

特点：

- 模型自己决定 query。
- 搜索和模型推理在同一个 Responses 请求中完成。
- Codex 只消费上游事件，不执行本地搜索函数。
- 当前 Sub2API/LLMX 路径已经支持。

### 4.2 Responses Lite 原生 `web.run`

工具声明：

```json
{
  "type": "namespace",
  "name": "web",
  "tools": [
    {
      "type": "function",
      "name": "run",
      "parameters": {...}
    }
  ]
}
```

它位于 `input[].additional_tools.tools` 中。

典型调用链：

```text
Responses Lite 模型
    -> 返回 web.run 客户端工具调用
Codex App WebSearchTool runtime
    -> POST provider_base_url/alpha/search
搜索后端
    -> { encrypted_output, output }
Codex App
    -> 把工具结果加入下一轮 /responses
模型
    -> 继续生成最终回答
```

执行者：Codex App 中的 Web Search extension，加上 provider 提供的 `/alpha/search` 服务。

关键源码：

- `references/codex-main/codex-rs/ext/web-search/src/extension.rs`
- `references/codex-main/codex-rs/ext/web-search/src/tool.rs`
- `references/codex-main/codex-rs/codex-api/src/endpoint/search.rs`
- `references/codex-main/codex-rs/codex-api/src/search.rs`

## 5. `/alpha/search` 不是 `/responses + web_search` 的别名

`/alpha/search` 是独立协议。

请求：

```json
{
  "id": "session-id",
  "model": "gpt-5.6-sol",
  "input": [...],
  "commands": {
    "search_query": [
      {
        "q": "2026 世界杯四强",
        "recency": 7,
        "domains": ["fifa.com"]
      }
    ],
    "response_length": "short"
  },
  "settings": {
    "allowed_callers": ["direct"],
    "external_web_access": true
  },
  "max_output_tokens": 10000
}
```

响应：

```json
{
  "encrypted_output": "optional opaque value",
  "output": "Search result text"
}
```

`commands` 还支持：

- `search_query`
- `image_query`
- `open`
- `click`
- `find`
- `screenshot`
- `finance`
- `weather`
- `sports`
- `time`

因此不能把 `/alpha/search` body 原样转发到 `/v1/responses`。未来如果 CodexHub 用托管 `web_search` 模拟 `/alpha/search`，必须显式完成命令转换和返回转换，而且不一定能完整覆盖 open/click/find/screenshot/finance/weather/sports/time 的语义。

## 6. Provider gate 与账户语义

Codex Web Search extension 的可用条件是：

```text
(provider.is_openai() OR provider.uses_openai_actor_authorization())
AND web_search_mode != disabled
```

CodexHub 默认写入的本地 provider：

```toml
chatgpt_base_url = "http://127.0.0.1:3847/backend-api"
web_search = "live"

[model_providers.ai-gateway]
name = "ai-gateway"
wire_api = "responses"
requires_openai_auth = true
base_url = "http://127.0.0.1:3847/ai-gateway/v1"
experimental_bearer_token = "dummy-token"
```

表键和身份字段都保持 `ai-gateway`，因此 `provider.is_openai()` 为假，不会仅因本地 Gateway 身份而启用 OpenAI 私有协议行为。`requires_openai_auth=true` 保留模拟 ChatGPT 账户态，让 Codex App 使用 CodexHub 的模型目录和 Remote Control 状态。

### 6.1 Actor Authorization 的判定

`uses_openai_actor_authorization()` 要求：

1. `requires_openai_auth = false`
2. provider 静态 `http_headers` 中存在非空 `x-openai-actor-authorization`

只满足其中一个没有用。

`requires_openai_auth=false` 不只是一个工具 gate。Codex model-provider 会同时把 provider account 设为 `None`。在 Codex App 26.707.8479 的实机验证中，前端随后采用官方 Statsig dynamic config `107580212` 的 `available_models` 白名单；即使 CodexHub `/models` 返回 12 个模型，下拉框也只显示两边交集中的 6 个 GPT 模型。

Remote Control transport 虽然可以独立读取 `auth.json`、`chatgpt_base_url` 和持久化 enablement，但这不能修复前端模型白名单。产品默认配置不能为了原生搜索牺牲自定义模型，因此 Actor Authorization 仅保留为协议研究结论，不再默认写入；已有旧 header 会在重新配置时移除。

### 6.2 为什么不再使用 `name = "OpenAI"`

`is_openai()` 不只控制 Web Search。直接改名会同时影响或开启其他 OpenAI 私有行为，例如：

- OpenAI 请求压缩策略。
- 私有 metadata passthrough。
- remote compaction。
- model-switch compaction。
- reasoning summary 并发/流式行为。

这些行为与开启搜索无关，却扩大了所有渠道的协议面，尤其会把 Grok、Anthropic 和 DeepSeek 会话也带入 OpenAI remote compaction 判断。因此默认配置既不伪装 OpenAI，也不使用 Actor Authorization，而是保留 hosted `web_search` 兼容链路。

其中一个立即可见的差异是请求压缩。Codex 的 `enable_request_compression` 默认开启；当认证使用 Codex backend 且 `provider.is_openai()` 为真时，流式 `/responses` body 会使用 zstd，并携带 `Content-Encoding: zstd`。默认 `ai-gateway` 身份不再触发该分支，但 CodexHub 仍保留 zstd 解压支持，以兼容旧配置和用户显式配置的 OpenAI provider。

### 6.3 Image generation 与 Web Search 使用不同 gate

Codex 当前对 image generation 和 web search 的 runtime gate 并不完全相同。Image generation 接受 OpenAI provider、`requires_openai_auth=true` 或 Actor Authorization；Web Search extension 接受 OpenAI provider或 Actor Authorization。

当前默认配置通过 `requires_openai_auth=true` 保留 image generation gate，但不满足原生 Web Search gate。两者的执行协议本来就不同，不能因为模型同时支持图片与搜索就共用同一条后端路由。

### 6.4 不要依赖 `[features].image_generation` 控制 Codex App

历史版本中，`[features].image_generation = false` 对 Codex CLI 有效，但不能作为 Codex App 的可靠控制面。Codex App 使用 app-server、runtime feature enablement 和自身工具注册流程；不同版本还可能存在配置覆盖或不立即 reload 的差异。

CodexHub 因此不再通过修改该字段实现“过滤生图工具”，而是在 AI Gateway 转发前按实际请求结构处理：

- 删除旧版顶层 hosted tool：`tools[].type = image_generation`。
- 删除新版 standalone namespace：`name = image_gen`。
- 删除 Responses Lite `input[].type = additional_tools` 中的 `image_gen` 工具。
- Code mode 只向上游暴露一个 `exec` custom tool 时，从 `exec.description` 中移除完整的 `## image_gen` 工具注册段。

最后一项移除的是模型可见的常规工具声明，并不会卸载 Codex App 进程内已经注册的 extension，也不会改写 code mode 运行时的 `ALL_TOOLS`。因此它能让模型在正常工具选择中不再看到 `image_gen`，但不是安全隔离边界；若未来需要绝对禁用执行能力，仍需 Codex App 提供可靠的 feature/capability 控制。当前方案的目标是稳定过滤转发请求，同时避免修改 Codex App 本体或依赖不稳定的 App 配置开关。

对齐 Codex 源码时不要扩大过滤范围：`Feature::ImageGeneration=false` 只会让 Tool Plan 跳过 `image_gen.imagegen` executor。系统 `imagegen` skill 由独立的 bundled-skills 机制加载，code mode 模板中的 `generatedImage` 与 `ALL_TOOLS` 也是通用固定文案，因此 Codex 原生关闭 image generation 后仍可能保留这些内容。Gateway 应保留它们，只删除 Codex 在 feature 关闭时本来就不会序列化的工具声明。

## 7. 当前 Sub2API 能力边界

对 `references/sub2api-main` 的代码审查结论：

### 7.1 已支持

- `/v1/responses` 请求。
- GPT-5.6 模型映射。
- 普通 Responses Lite `additional_tools` 透传与保留。
- `namespace`/custom/function 等客户端工具声明。
- 顶层标准 Responses `web_search`。
- `web_search_call` 响应和协议转换。
- 部分协议下的 Web Search emulation。

这解释了为什么 Lite 下的 `exec`、浏览器插件等客户端工具可以正常工作。

### 7.2 新增支持

当前 `references/sub2api-main` 版本为 `0.1.152`，已经新增：

- `/v1/alpha/search`。
- `/alpha/search`。
- `/backend-api/codex/alpha/search`。
- API Key 上游转发到 provider `/v1/alpha/search`。
- OAuth 上游转发到 ChatGPT Codex `/backend-api/codex/alpha/search`。
- model mapping、query string、未知请求/响应字段和上游错误原样保留。

CodexHub 对应链路为：

```text
Codex App
  -> http://127.0.0.1:3847/ai-gateway/v1/alpha/search
CodexHub
  -> Sub2API /v1/alpha/search
Sub2API
  -> OpenAI-compatible provider /v1/alpha/search
```

### 7.3 正确归因

不能简单说“Sub2API 不支持 Responses Lite”。更准确的说法是：

- Sub2API 支持 Lite 请求和客户端工具声明的转发。
- Sub2API 支持标准托管 `web_search`。
- Sub2API 已实现原生 Lite `web.run` 所依赖的 `/alpha/search` 搜索服务。
- CodexHub 已实现 OpenAI Responses-only 的透明代理、模型映射、路由粘性、熔断反馈和请求日志。
- `web.run` 是否出现仍由 Codex App provider gate 决定，不能把“后端路由可用”和“前端工具已注册”混为一谈。

## 8. 2026-07-12 真实验证记录

### 8.1 失败验证：托管工具放进 `additional_tools`

CodexHub 曾尝试把：

```json
{
  "type": "web_search",
  "external_web_access": true,
  "search_content_types": ["text", "image"]
}
```

追加到 `input[].additional_tools.tools`。

请求日志 `8623` 到 `8627` 显示：

- Codex 原始请求没有 `web_search`。
- CodexHub 上游请求已成功追加 `web_search`。
- 上游均返回 HTTP 成功，没有 400。
- 所有 SSE 都没有 `web_search_call`。
- 所有 SSE 都没有 `function/custom name=web_search`。
- 模型明确回答“当前环境没有提供 web_search 工具”。

结论：当前上游把 `additional_tools` 中的 hosted `web_search` 静默忽略。没有报错不等于工具生效。

### 8.2 成功验证：Lite 请求中使用顶层托管工具

真实探针 request id：

```text
codexhub-web-search-probe-1783795233976
```

探针保持：

- `x-openai-internal-codex-responses-lite: true`
- `input[].additional_tools`
- GPT-5.6 Sol

只把托管工具改到顶层：

```json
{
  "tools": [
    {
      "type": "web_search",
      "external_web_access": true,
      "search_content_types": ["text"]
    }
  ]
}
```

结果：

- HTTP 200。
- SSE 正常 completed。
- SSE 出现真实 `web_search_call`。

结论：当前 LLMX/Sub2API/上游组合接受“Lite transport + 顶层 hosted web_search”的混合形态。

## 9. CodexHub 当前兼容实现

当前实现在：

```text
src/ai_gateway/handler.rs
inject_hosted_web_search_into_lite_request_tools
```

匹配条件：

1. provider type 为 `OpenAiResponses`。
2. model 为 `gpt-5.6` 或 `gpt-5.6-*`。
3. `input` 中存在合法 `additional_tools.tools` 数组，用于确认这是 Lite 请求。
4. `additional_tools` 中既不存在原生 `namespace=web, function=run`，也不存在 Code Mode `exec.description` 注册的 ``### `web__run` `` 工具标题。

注入位置：顶层 `tools`。

```json
{
  "type": "web_search",
  "external_web_access": true,
  "search_content_types": ["text", "image"]
}
```

其他规则：

- 顶层已有 `web_search` / `web_search_preview` 时不重复添加。
- `additional_tools` 已直接包含原生 `web.run`，或 Code Mode `exec` 已注册 `web__run` 时不注入，避免两套搜索执行链同时暴露给模型。
- 顶层已有其他工具时保留原顺序并追加。
- Grok Responses 不注入。
- GPT-5.5、GPT-5.4 等旧模型不注入。
- 没有 `additional_tools` 的标准 Responses 请求不注入。
- 注入后记录日志：`injected hosted web_search into top-level tools for Responses Lite request`。

### 9.1 这是兼容扩展，不是原生 Lite

当前形态是：

```text
Lite 客户端工具：input[].additional_tools
托管搜索工具：顶层 tools
```

Codex 当前源码不会主动生成这个混合结构，但当前真实上游已经验证支持。

Hosted `web_search` 与原生 Lite `web.run` 是替代关系，不是叠加关系。Codex 原生测试也要求 standalone `web.run` 出现时不再携带 hosted `web_search`。因此当前兼容注入只在 `web.run` 缺失时生效；未来原生搜索链路接通后，会自然停止注入 hosted 工具。

2026-07-13 的真实日志暴露了第二种注册形态：Codex App 在 Code Mode 中只发送 `exec` custom tool，并在其 description 中用精确 Markdown 标题 ``### `web__run` `` 注册搜索工具。模型随后返回 `exec` 调用并执行 `tools.web__run(...)`，最终触发 `/alpha/search`。如果互斥逻辑只检查 namespace 结构，就会错误地再注入 hosted `web_search`。因此当前实现同时识别 namespace 与 Code Mode 标题；普通 prose 中偶然出现 `web__run` 不视为工具注册。

### 9.2 当前兼容方案的已知短板

1. Gateway 无法从 Lite body 判断用户是否主动关闭了 Web Search，因此当前对目标模型始终注入。
2. 当前固定使用 `external_web_access=true`，没有完整继承用户 Cached/Indexed/Live 模式。
3. 当前没有携带 `filters.allowed_domains`、`user_location`、`search_context_size` 等用户配置。
4. 顶层工具可用不代表模型每轮一定调用；`tool_choice=auto` 时模型仍可选择不搜索。
5. 如果模型同时看见浏览器插件，可能选择浏览器而不是 hosted `web_search`。
6. 混合形态是当前上游兼容行为，未来 GPT-5.6 或 Sub2API 升级后必须重新回归。

这些短板是选择“小改动恢复托管搜索”的代价，不应在文档中伪装成完全原生行为。

## 10. 已验证失败或不推荐的方案

| 方案 | 结果/风险 | 结论 |
| --- | --- | --- |
| 把 hosted `web_search` 放进 `additional_tools` | 上游静默忽略，无 `web_search_call` | 不使用 |
| 在 `additional_tools` 伪造 function `web_search` | 模型可能调用，但 Codex 无本地 runtime | 不使用 |
| 只在请求中伪造 `web.run` | 没有 Codex 本地 runtime，模型调用后无法执行 | 不使用 |
| 默认注入 Actor Authorization | `account=None`，前端套用官方 Statsig 模型白名单，自定义模型消失 | 实机失败，不使用 |
| 表键保持 `ai-gateway`，身份写为 `OpenAI` | 启用原生 `web.run`，同时开启 remote compaction、请求压缩等 OpenAI 私有行为 | 不使用 |
| CodexHub 将 `/alpha/search` 透明转发到当前 Sub2API OpenAI 渠道 | 保留 command 协议和 opaque response，不做错误的 `/responses` 转换 | 已采用 |
| 直接把 `use_responses_lite=false` | 会改变完整请求编码，GPT-5.6 兼容性未知 | 需单独验证 |
| 把 `/alpha/search` body 原样转成 `/responses` body | 两个协议字段和返回完全不同 | 无效 |

## 11. 原生 `web.run` 当前实现与剩余工作

原生 Lite Web Search 必须同时具备工具注册和搜索 backend。当前 backend transport 已完成，但默认 provider gate 因模型列表副作用主动关闭；hosted `web_search` 继续作为默认兼容路径。

### 11.1 Provider 能力与 gate

当前实现：

1. provider 表键和身份字段都保持 `ai-gateway`。
2. 默认本地 provider 写入 `requires_openai_auth=true` 和顶层 `web_search="live"`，不写 Actor Authorization header。
3. 该形态保留账户态、完整自定义模型、Remote Control 和 Codex 本地摘要压缩，但不会注册原生 `web.run`。
4. Gateway 的 `/alpha/search` transport 与 Actor header 过滤/脱敏逻辑仍保留，供未来稳定 gate 或显式实验配置使用。
5. `name="OpenAI" + requires_openai_auth=true`、Actor Authorization 形态和旧 `ai-codex` 均作为升级/卸载兼容形态识别；重新配置会迁移到当前认证型 `ai-gateway` 身份。

未来只有在 Codex 提供不会清空 provider account、不会触发官方模型白名单的稳定 capability gate 后，才考虑默认开启原生 `web.run`。届时仍需回归 `/alpha/search` 调用、工具结果回填、前端模型展示和 Remote Control。

### 11.2 CodexHub 路由

按当前 base URL，CodexHub 已实现：

```http
POST /ai-gateway/v1/alpha/search
```

该路由只选择 `ProviderType::OpenAiResponses`，并复用现有 model alias、权重优先级、会话粘性和熔断状态。请求会转发到：

```http
POST {provider_api_root}/v1/alpha/search
```

原始 query string、除 `model` 映射外的未知 JSON 字段、上游状态码、`content-type` 和响应 body 均保留。

### 11.3 SearchRequest 支持范围

CodexHub 不绑定不断演进的 SearchRequest schema，只要求顶层 `model` 是非空字符串。`id`、`input`、全部 `commands`、`settings`、`max_output_tokens` 以及未来新增字段均透明转发。因此命令支持范围由当前 Sub2API 和最终 OpenAI-compatible 上游决定，而不是由 CodexHub 白名单决定。

### 11.4 搜索 backend 选择

当前采用原生透明转发，不再把 `/alpha/search` 转换成 hosted `/responses + web_search`。后者仍有以下语义差距：

- `/alpha/search` 是明确 command 驱动。
- hosted `web_search` 由模型决定搜索过程。
- open/click/find/screenshot/finance/weather/sports/time 不一定能一一对应。
- hosted Responses 返回 SSE/response items，而 `/alpha/search` 只返回 `{encrypted_output, output}`。

因此 hosted `web_search` 只作为另一条兼容路径，不能冒充 `/alpha/search`。

### 11.5 SearchResponse

Codex 当前最低可解析响应：

```json
{
  "encrypted_output": null,
  "output": "search result"
}
```

注意：

- `output` 是 Codex 当前需要的结果字符串。
- `encrypted_output` 是可选 opaque 数据，CodexHub 不解析、不修改。
- CodexHub 不重组 SearchResponse；成功、错误和未来新增字段都按上游 wire body 返回。

### 11.6 UI 事件

原生 `web.run` runtime 会由 Codex extension 产生 WebSearch begin/end item。若使用原生链路，Gateway 不需要把 `/alpha/search` 本身伪装成 Responses `web_search_call`。

这和当前 hosted `web_search` 不同：当前 UI item 来自上游 SSE 中的 `web_search_call`。

## 12. Responses Lite 其他协议坑

### 12.1 严格反序列化容易被新字段击穿

Responses Lite 会引入：

- `additional_tools`
- `namespace`
- 新的 tool spec 形态
- 新 input item 类型

Responses 到 Responses 透传路径不应先强制反序列化成只覆盖旧协议的完整业务结构。CodexHub 当前使用宽松 `GatewayRequestEnvelope` 只读取路由所需字段，再保留原始 JSON 透传，这是正确方向。

典型错误：

```text
Unknown parameter: input[N].namespace
```

该错误可能来自上游不支持 Lite，也可能来自中间转换器把 namespace 放错层级。必须比较 Codex 原始请求和上游请求，不能只看最终 400。

### 12.2 不要随意改 developer role

Lite 基础 instructions 通过 developer message 注入。把所有 developer message 统一降级为 user 或提升为 system，会改变模型指令层级。

协议转换必须先区分：

- Lite 基础 instructions。
- `additional_tools` carrier。
- 用户配置插入的 developer message。
- 低信任记忆/上下文 developer message。

不要只按 role 一刀切。

### 12.3 不要修改加密上下文

Lite 请求和 compact/续链可能携带：

- `reasoning.encrypted_content`
- compaction blob
- `compaction_summary`
- item id/call id

这些字段可能是上游不透明数据。除已明确验证的兼容修复外，应原样保留。任何重新序列化、裁剪或协议转换都可能触发：

```text
Could not decode the compaction blob. Ensure it is unmodified from the compact response.
```

### 12.4 Lite header 不能被代理层误删

请求体是 Lite 形态时，应保留：

```http
x-openai-internal-codex-responses-lite: true
```

若代理保留 Lite body 却删除 header，上游可能按标准 Responses 解析；若只保留 header 却改写 body，也可能进入不一致状态。

当前 CodexHub header 转发规则会保留该头。升级 header allowlist/blocklist 时必须回归。

### 12.5 `supports_search_tool` 不等于 `web_search` 一定存在

模型元数据中的：

```json
"supports_search_tool": true
```

不代表当前 provider、认证和 runtime 一定提供 Web Search。最终可见工具还受：

- `use_responses_lite`
- provider capabilities
- provider identity
- Actor Authorization
- feature flags
- `web_search_mode`
- extension 是否安装
- `tool_mode=code_mode_only`

共同影响。

同理，`web_search_tool_type=text_and_image` 只决定 hosted tool 被创建时的序列化字段，不会强制创建该工具。

### 12.6 Code Mode Only 会改变工具可见方式

GPT-5.6 使用 `tool_mode=code_mode_only`。很多 namespace 工具不会作为普通顶层 function 直接暴露，而是通过 `exec` 内的工具桥调用，例如：

```javascript
const result = await tools.web__run({
  search_query: [{ q: "query" }]
});
```

因此只看顶层直出工具名称，可能误判工具是否存在。排查时需要同时检查：

- `additional_tools` 完整工具规格。
- `exec` 描述中的嵌套工具。
- Codex ToolRegistry runtime。
- 模型返回的是 custom/function/namespace 还是 hosted call。

Gateway 的 hosted 搜索互斥判断也必须遵守同一规则：直接 namespace `web.run` 和 `exec.description` 中精确的 ``### `web__run` `` 标题都代表原生搜索已注册，此时不得再注入顶层 `web_search`。

### 12.7 原始请求与上游请求必须分开记录

Gateway 可能执行：

- model alias 替换。
- prompt cache 字段补齐。
- Grok reasoning 修复。
- image generation 过滤，包括 hosted `image_generation`、standalone `image_gen` 和 code mode `exec` 描述。
- GPT-5.6 hosted web search 注入。

因此日志必须同时保留：

```text
request_json：Codex 原始请求
upstream_request_json：Gateway 实际发给上游的请求
upstream_response_sse：上游原始流
```

没有这三份数据，就无法判断工具是 Codex 没发、Gateway 删除、上游忽略，还是 Codex UI 没展示。

## 13. 故障定位速查表

| 现象 | 优先检查 | 常见原因 |
| --- | --- | --- |
| 模型说没有 `web_search` | 上游请求中是否有顶层 `tools[].type=web_search` | 工具没注入或放进了 additional_tools |
| `additional_tools` 有 `web_search`，但无搜索事件 | `upstream_response_sse` 是否有 `web_search_call` | hosted 工具放错层，被静默忽略 |
| 模型调用 `web_search` 后报 unknown tool | 返回 item 是 function/custom 还是 hosted | 伪造了客户端函数但没有 runtime |
| `web.run` 不出现在工具列表 | provider gate、Actor Authorization、web_search_mode | 默认配置的预期行为；当前使用 hosted `web_search` |
| `web.run` 可见但执行 404 | 实际请求 URL、安装版本 | CodexHub/Sub2API 仍是旧版本，或 provider base URL 指向不支持该接口的上游 |
| Sub2API 返回 404 | 渠道平台和版本 | 非 OpenAI group，或运行版本早于 `0.1.152` |
| 上游 400 `input[N].namespace` | 对比原始与上游 JSON | 上游不支持 Lite，或 namespace 被错误改写 |
| 有搜索答案但 UI 没有搜索 item | SSE 是否有 `web_search_call` | 上游只生成文本，没有标准事件 |
| `web_search_call` 有但 query 为空 | added/done 事件顺序和 action | 过早发出 completed item |
| GPT-5.6 搜索生效但旧模型行为变化 | 注入 scope | 条件过宽，误伤非 Lite/非 GPT-5.6 |

## 14. 升级审计清单

每次同步 Codex 或 Sub2API 新版本后，至少检查以下内容。

### 14.1 Codex 模型元数据

- GPT-5.6 是否仍为 `use_responses_lite=true`。
- 是否新增 GPT-5.6 变体。
- `tool_mode` 是否变化。
- `web_search_tool_type` 是否变化。
- `supports_search_tool` 是否变化。

### 14.2 Codex 请求构造

- `core/src/client.rs::build_responses_request`。
- `core/src/tools/spec_plan.rs::hosted_model_tool_specs`。
- Lite 是否仍排除 hosted tools。
- `additional_tools` 是否新增字段。
- Lite header 名称是否变化。
- `reasoning.context` 和 `parallel_tool_calls` 是否变化。

### 14.3 Web Search extension

- `ext/web-search/src/extension.rs` 的 provider gate。
- `ext/web-search/src/tool.rs` 的 namespace/tool 名称。
- `codex-api/src/endpoint/search.rs` 的 endpoint path。
- `codex-api/src/search.rs` 的 SearchRequest/SearchResponse 字段。
- 是否新增 command、settings 或 allowed caller。

### 14.4 Sub2API

- 是否新增 `/alpha/search`。
- 是否新增 Responses Lite 专门解析器。
- 是否继续保留 `additional_tools`。
- 是否继续支持 Lite header + 顶层 `web_search` 混合形态。
- hosted `web_search` 是否仍返回标准 `web_search_call`。

### 14.5 CodexHub 回归

- GPT-5.6 Lite 原始请求不含 hosted `web_search`。
- 上游请求只在顶层新增一次 `web_search`。
- additional client tools 未被移动或删除。
- 原始请求已有直接 `web.run`，或 Code Mode `exec` 已注册 `web__run` 时不新增 hosted `web_search`。
- Grok/Anthropic/Chat Completions 不误注入。
- GPT-5.4/GPT-5.5 标准请求不误注入。
- 上游 SSE 出现 `web_search_call`。
- Codex App 显示搜索 item。
- 搜索结束后会话能继续，不产生孤儿 tool call。

## 15. 测试要求

当前兼容方案至少保留以下单元测试：

- GPT-5.6 Lite 请求创建顶层 `web_search`。
- additional client tools 保持不变。
- 注入幂等。
- 保留已有顶层工具。
- 非 GPT-5.6 不注入。
- 非 OpenAI Responses provider 不注入。
- 无 `additional_tools` 的标准请求不注入。
- `additional_tools` 已包含原生 `web.run` 时不注入。

原生 `web.run` transport 已增加：

- `/ai-gateway/v1/alpha/search` 路由。
- OpenAI Responses Provider 类型限定。
- model alias、会话粘性、权重和熔断复用。
- query、未知字段、状态码和响应 body 透明保留。
- `alpha_search` 请求日志。

仍需增加或做真实环境验证：

- `/alpha/search` 上游 2xx、4xx、5xx 和 query passthrough 集成测试。
- web.run 调用、搜索、工具结果回填、下一轮 Responses 的端到端测试。
- 404、超时、上游 4xx/5xx、空 output、取消请求测试。
- Codex App WebSearch begin/end UI item 测试。
- 后续 Codex 若提供稳定 capability gate，再回归原生 `web.run`、自定义模型、账号区、手机入口和 Remote Control。

## 16. 当前决策记录

当前版本选择：

```text
保留 Responses Lite
+ 顶层注入标准 hosted web_search
+ 复用 Sub2API /v1/responses
+ provider 表键和身份均保持 ai-gateway
+ 默认 requires_openai_auth=true 且 web_search=live
+ Codex App 保留完整模型、账户态和 Remote Control
+ 提供 /ai-gateway/v1/alpha/search 透明代理
+ 只允许 OpenAI Responses Provider 承接 alpha/search
- 不将本地 provider 伪装成 OpenAI
- 不启用 OpenAI remote compaction
- 不伪造或本地模拟 SearchResponse
```

理由：

1. hosted `web_search` 已有真实请求验证，继续作为兼容能力保留。
2. 当前 Sub2API 已实现 alpha search，CodexHub 只做透明 transport，不猜测 command 语义。
3. Actor Authorization 会令 provider account 为 `None`，并让当前 Codex App 使用官方 Statsig 模型白名单；这个副作用会直接隐藏 Grok、DeepSeek、GLM 和 Anthropic 模型，不能接受。
4. `name="OpenAI"` 会额外开启 remote compaction、zstd 和私有 metadata，同样不适合作为单纯开启搜索的手段。
5. 原生 `web.run` 与 hosted `web_search` 已互斥：有原生声明时不重复注入 hosted 工具，没有原生声明时仍可兼容旧路径。

发布前应以本文第 11、14、15 节为验收基线，确认客户端 provider gate、CodexHub endpoint、搜索 backend、账户模型和 Remote Control 在同一版本中同时可用。
