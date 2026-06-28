# AI Gateway Web Search 协议对接说明

状态：已落地。本文记录 Codex 使用的 OpenAI Responses `web_search` 工具，与 Anthropic Messages client tool `WebSearch` 及内部 `web_search_20250305` server tool 之间的转换规则。

相关文档：

- [`ai-gateway-anthropic-messages.zh-CN.md`](ai-gateway-anthropic-messages.zh-CN.md)
- [`ai-gateway-glm-anthropic-integration.zh-CN.md`](ai-gateway-glm-anthropic-integration.zh-CN.md)
- OpenAI Responses API Reference: <https://developers.openai.com/api/reference/resources/responses>
- OpenAI Web Search Guide: <https://platform.openai.com/docs/guides/tools-web-search?api-mode=responses>
- Anthropic Web Search Tool: <https://docs.anthropic.com/en/docs/agents-and-tools/tool-use/web-search-tool>

## 1. 目标

Codex 侧只理解 Responses 协议。AI Gateway 对 Anthropic/Claude、GLM Anthropic-compatible 等上游时，需要做到：

- Codex 发来的 `tools[].type = "web_search"` 在主对话中转换为 Claude Code 风格 `WebSearch` client tool。
- 上游返回 `tool_use(name=WebSearch)` 时，Gateway 截留该调用，使用短上下文内部请求调用 Anthropic `web_search_20250305` server tool。
- 搜索过程必须转回 Responses `web_search_call` SSE 事件，让 Codex App / CLI / TUI 识别为搜索过程，而不是只在最终答案中体现。
- 上游搜索引用必须尽量转成 Responses `output_text.annotations`，避免来源标注丢失。
- 不把 Anthropic 私有字段泄漏到 Codex 应用层。
- 对 Anthropic-compatible 上游保持保守兼容，默认使用更广泛支持的 tool tag。

## 2. Codex Responses 侧形态

Codex 发给 Gateway 的工具定义通常是：

```json
{
  "type": "web_search",
  "external_web_access": true,
  "search_content_types": ["text", "image"],
  "filters": {
    "allowed_domains": ["example.com"]
  }
}
```

Codex 源码里 `web_search` 工具由 `ToolSpec::WebSearch` 序列化，字段包括：

| Responses 字段 | 含义 |
| --- | --- |
| `external_web_access` | 是否允许 live web access；`true` 通常对应 live/indexed 搜索，`false` 通常对应 cached。 |
| `index_gated_web_access` | 是否只允许 index-gated web access。 |
| `filters.allowed_domains` | 限定允许搜索的域名。 |
| `user_location` | 搜索用户位置。 |
| `search_context_size` | 搜索上下文大小。 |
| `search_content_types` | 搜索内容类型，例如 `["text", "image"]`。 |

Codex 消费上游结果时，关键 item 是：

```json
{
  "type": "web_search_call",
  "id": "ws_...",
  "status": "completed",
  "action": {
    "type": "search",
    "query": "weather seattle",
    "queries": ["weather seattle"]
  }
}
```

Codex 源码 `core/src/event_mapping.rs` 会把 `web_search_call.action` 转成 UI 和会话历史里的 WebSearch item。也就是说，`id` 与 `action` 是 Codex web search UI 的关键字段。

## 3. Anthropic Messages 出站工具

Gateway 对 Codex Responses `web_search` 使用两段式映射，这一点和直接把工具改成 Anthropic server tool 不同。

第一段是主对话。Codex 的 `web_search` / `web_search_preview` 会被构造成 Claude Code 风格的 Anthropic client tool：

```json
{
  "name": "WebSearch",
  "description": "Search the web. Returns result blocks with titles and URLs. US-only...",
  "input_schema": {
    "type": "object",
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "required": ["query"],
    "properties": {
      "query": {"type": "string", "minLength": 2},
      "allowed_domains": {"type": "array", "items": {"type": "string"}},
      "blocked_domains": {"type": "array", "items": {"type": "string"}}
    },
    "additionalProperties": false
  }
}
```

模型如果判断需要搜索，会返回标准 Anthropic `tool_use`：

```json
{
  "type": "tool_use",
  "id": "toolu_...",
  "name": "WebSearch",
  "input": {"query": "weather seattle"}
}
```

Gateway 不把这个 `WebSearch` 交给 Codex 客户端执行，而是在 provider 内部截留它。

第二段是内部短上下文搜索。Gateway 用 Claude Code 参考请求形态向同一个 Anthropic-compatible 上游发起 server tool 请求：

```json
{
  "model": "claude-opus-4-8",
  "tools": [{"name": "web_search", "type": "web_search_20250305", "max_uses": 8}],
  "stream": true,
  "system": [
    {"type": "text", "text": "You are Claude Code, Anthropic's official CLI for Claude."},
    {"type": "text", "text": "You are an assistant for performing a web search tool use"}
  ],
  "messages": [{
    "role": "user",
    "content": [{"type": "text", "text": "Perform a web search for the query: weather seattle"}]
  }],
  "thinking": {"type": "disabled"},
  "max_tokens": 64000,
  "tool_choice": {"type": "tool", "name": "web_search"},
  "output_config": {"effort": "high"}
}
```

内部搜索完成后，Gateway 将搜索结果整理成 Anthropic `tool_result`，追加回原主对话 messages，再继续请求模型生成最终答案。主对话可能一次返回多个 `WebSearch` tool_use，Gateway 会为同一 assistant turn 的每个 tool_use 都执行内部搜索，并一次性追加全部 tool_result。

### 3.1 为什么内部搜索使用 `web_search_20250305`

Anthropic 官方和多家 Anthropic-compatible 上游都支持 `web_search_20250305`。`web_search_20260209` / `web_search_20260318` 属于更高版本能力，一些上游会直接报错：

```text
Input tag 'web_search_20260209' found using 'type' does not match any of the expected tags ...
```

因此内部短上下文搜索默认使用 `web_search_20250305`。后续如果需要启用更高版本 web search，应通过 provider capability/profile 显式开启，而不是全局切换。

### 3.2 字段映射

| Codex Responses | 主对话 Anthropic client tool | 内部搜索 Anthropic server tool | 当前策略 |
| --- | --- | --- | --- |
| `type = "web_search"` | `name = "WebSearch"` | `type = "web_search_20250305"` | 主对话先暴露 client tool；内部短上下文再执行 server tool。 |
| `filters.allowed_domains` | `allowed_domains` | 暂不下发 | 主对话工具 schema 支持字段；内部请求当前按 query 执行。 |
| `allowed_domains` | `allowed_domains` | 暂不下发 | 已支持扁平化字段进入 `WebSearch` input schema。 |
| `blocked_domains` | `blocked_domains` | 暂不下发 | 已支持进入 `WebSearch` input schema。 |
| `user_location` | 无直接等价字段 | 无直接等价字段 | 不透传。 |
| `max_uses` | 无直接等价字段 | `max_uses = 8` | `8` 来自 Claude Code 内部 websearch 请求形态，不作为 Gateway 主循环轮次限制。 |
| `external_web_access` | 无直接等价字段 | 无直接等价字段 | 不透传。 |
| `index_gated_web_access` | 无直接等价字段 | 无直接等价字段 | 不透传。 |
| `search_context_size` | 无稳定等价字段 | 无稳定等价字段 | 不透传。 |
| `search_content_types` | 无稳定等价字段 | 无稳定等价字段 | 不透传。 |

不透传字段不是删除 Codex 能力，而是 Anthropic client tool / `web_search_20250305` 没有稳定等价字段。为了兼容第三方上游，Gateway 不向 Anthropic-compatible API 发送未知字段。

## 4. Anthropic 非流式回包

Anthropic web search 典型回包内容块：

```json
[
  {
    "type": "server_tool_use",
    "id": "srvtoolu_...",
    "name": "web_search",
    "input": {
      "query": "Portugal World Cup result"
    }
  },
  {
    "type": "web_search_tool_result",
    "tool_use_id": "srvtoolu_...",
    "content": [
      {
        "type": "web_search_result",
        "title": "Result",
        "url": "https://example.com",
        "encrypted_content": "..."
      }
    ]
  },
  {
    "type": "text",
    "text": "Final answer",
    "citations": [
      {
        "type": "web_search_result_location",
        "url": "https://example.com",
        "title": "Result",
        "cited_text": "..."
      }
    ]
  }
]
```

Gateway 转成 Responses：

- `server_tool_use(name=web_search)` -> `web_search_call`
- `web_search_tool_result` -> 完成对应 `web_search_call.status`
- `text.citations[]` -> `output_text.annotations[]`

输出示例：

```json
{
  "type": "web_search_call",
  "id": "srvtoolu_...",
  "call_id": "srvtoolu_...",
  "status": "completed",
  "action": {
    "type": "search",
    "query": "Portugal World Cup result",
    "queries": ["Portugal World Cup result"]
  }
}
```

引用转成：

```json
{
  "type": "url_citation",
  "start_index": 0,
  "end_index": 12,
  "url": "https://example.com",
  "title": "Result"
}
```

## 5. Anthropic 流式 SSE 回包

Anthropic 流式事件序列通常是：

```text
message_start
content_block_start(server_tool_use)
content_block_delta(input_json_delta)
content_block_stop
content_block_start(web_search_tool_result)
content_block_stop
content_block_start(text)
content_block_delta(citations_delta)
content_block_delta(text_delta)
content_block_stop
message_delta
message_stop
```

Gateway 转成 Responses SSE。真实 Responses websearch 不只有一个 `web_search_call` item，而是一组过程事件：

| Anthropic SSE / 内部动作 | Responses SSE |
| --- | --- |
| query 可用，准备执行搜索 | `response.output_item.added`，item 为 `web_search_call`，`status = "in_progress"`，不带 `action`。 |
| 搜索 item 已进入执行状态 | `response.web_search_call.in_progress`。 |
| 正在搜索 | `response.web_search_call.searching`。 |
| 搜索完成 | `response.web_search_call.completed`。 |
| 搜索 item 完成 | `response.output_item.done`，item 为 `web_search_call`，`status = "completed"`，`action.type = "search"`，带 `action.query` / `action.queries`。 |
| `content_block_delta(citations_delta)` | `response.output_text.annotation.added`。 |
| `content_block_delta(text_delta)` | `response.output_text.delta`。 |
| text block done | `response.content_part.done` 与 message `response.output_item.done`，annotations 放入最终 content part。 |

`response.completed.response.output` 只放最终 message/reasoning 等回答内容，不把已经通过过程事件发出的 `web_search_call` 再塞进最终 output。

### 5.1 延迟发出 web_search_call

有些上游会先发：

```json
{
  "type": "server_tool_use",
  "id": "srvtoolu_...",
  "name": "web_search",
  "input": {}
}
```

随后才通过 `input_json_delta` 给 query。Gateway 必须等 query 非空后再发 Responses `web_search_call` 过程事件，否则 Codex 会看到空搜索。`output_item.added` 阶段只表示搜索开始，不带 `action`；`output_item.done` 阶段才补 `action.query` 和 `action.queries`。

### 5.2 忽略 Anthropic 内部空 server_tool_use

标准 Anthropic API 可能同时出现两类搜索块：

- 模型显式 `tool_use(name=WebSearch)` 或兼容上游 `tool_use(name=web_search)`，带 query。
- 上游内部 `server_tool_use(name=web_search)`，不带 query，只配合 `web_search_tool_result`。

Gateway 只把带 query 的搜索转成 Codex 可见 `web_search_call`。没有 query 的内部 `server_tool_use` 不生成空搜索 item。

## 6. Citation / Annotation 对齐

Anthropic 的搜索引用可能出现在：

- 非流式 text block 的 `citations[]`
- 流式 `content_block_delta.delta.type = "citations_delta"`

Gateway 统一转成 Responses `url_citation` annotation：

```json
{
  "type": "url_citation",
  "start_index": 0,
  "end_index": 20,
  "url": "https://www.rust-lang.org/",
  "title": "Rust"
}
```

流式场景下，Anthropic 可能先发 citation，再发 text。Gateway 当前策略：

- citation 到达时，以当前输出文本长度作为 `start_index`。
- 后续同一 message text 增长时，更新待闭合 annotation 的 `end_index`。
- 最终 `response.content_part.done` 和 message `response.output_item.done` 都带完整 annotations。

这能保证来源标注不会丢失，但由于 Anthropic 没有直接提供 OpenAI `url_citation` 的精确偏移，`start_index/end_index` 是基于流式到达顺序推导的近似区间。

## 7. GLM Anthropic-compatible 差异

`glm_anthropic` profile 仍然从 Codex Responses `web_search` 出站构造 Claude Code 风格 `WebSearch` client tool。内部短上下文搜索仍使用 `web_search_20250305` server tool。

GLM 回包可能使用：

- `server_tool_use.name = "web_search_prime"`
- `tool_result`，而不是 Anthropic 原生 `web_search_tool_result`
- 私有文本块：`Z.ai Built-in Tool: web_search_prime`
- 私有摘要块：`web_search_prime_result_summary`

Gateway 在 `GlmAnthropic` profile 中：

- 接受 `web_search_prime` 作为内部搜索 server tool。
- 把它转成标准 Responses `web_search_call`。
- 不把 GLM 私有文本块泄漏给 Codex。
- 不把搜索结果塞进 `web_search_call.action.result`，保持 Codex 期望的标准 `action.type/query/queries` 形态。

## 8. 已知限制

- `search_content_types=["image"]` 没有映射到 Anthropic `web_search_20250305`。如果后续 Anthropic 或某厂商提供稳定字段，需要通过 provider capability 显式开启。
- `external_web_access=false` 暂不降级为 cached search。内部 Anthropic server tool 默认表示上游搜索，Gateway 不做本地搜索。
- `web_search_tool_result.content[].encrypted_content` 不透传给 Responses。该字段主要服务 Anthropic 自身后续引用，不属于 Codex 当前消费的 `web_search_call` 形态。
- 如果上游只支持更旧或私有 search tag，需要新增 provider profile，不应在通用 Anthropic profile 中硬编码厂商差异。

## 9. 代码落点

| 模块 | 职责 |
| --- | --- |
| `providers/anthropic_messages/types.rs` | 定义默认 `ANTHROPIC_WEB_SEARCH_TYPE = "web_search_20250305"`。 |
| `providers/anthropic_messages/request_tools.rs` | Responses `web_search` -> Anthropic `WebSearch` client tool；原生 `web_search_20250305` 仍作为 server tool 保留。 |
| `providers/anthropic_messages/response.rs` | 非流式 Anthropic content -> Responses output。 |
| `providers/anthropic_messages/mod.rs` | 截留 `tool_use(name=WebSearch)`，执行内部短上下文搜索，合成 Responses websearch SSE 过程事件。 |
| `providers/anthropic_messages/stream_state.rs` | Anthropic SSE 事件分发。 |
| `providers/anthropic_messages/stream_tools.rs` | `server_tool_use` / `web_search_tool_result` -> `web_search_call` 及 `response.web_search_call.*` 过程事件。 |
| `providers/anthropic_messages/stream_message.rs` | text delta 和 citation delta -> Responses output_text。 |
| `providers/anthropic_messages/citations.rs` | Anthropic citation -> Responses annotation。 |
| `providers/anthropic_messages/tests.rs` | WebSearch client tool、内部两段式搜索、Responses SSE 过程事件、citation 映射测试。 |

## 10. 测试覆盖

当前应保持通过：

```powershell
cargo test --features gui --bin codexhub anthropic
cargo build --release --features gui --bin codexhub
```

重点测试点：

- Responses `web_search` 构造成 Anthropic `WebSearch` client tool。
- Responses `filters.allowed_domains` 映射到 Anthropic `allowed_domains`。
- Anthropic `tool_use(name=WebSearch)` 被 Gateway 截留并触发内部短上下文 `web_search_20250305` 请求。
- Anthropic `server_tool_use` / `web_search_tool_result` 转成 `web_search_call` 和 `response.web_search_call.in_progress/searching/completed`。
- 内部空 `server_tool_use` 不生成空 query 的 `web_search_call`。
- `output_item.added` 阶段不带 `action`，`output_item.done` 阶段带 `action.query` / `action.queries`。
- Anthropic `citations_delta` 转成 Responses `output_text.annotations`。
