# AI Gateway Web Search 协议对接说明

状态：已落地。本文记录 Codex 使用的 OpenAI Responses `web_search` 工具与 Anthropic Messages `web_search` server tool 之间的转换规则。

相关文档：

- [`ai-gateway-anthropic-messages.zh-CN.md`](ai-gateway-anthropic-messages.zh-CN.md)
- [`ai-gateway-glm-anthropic-integration.zh-CN.md`](ai-gateway-glm-anthropic-integration.zh-CN.md)
- OpenAI Responses API Reference: <https://developers.openai.com/api/reference/resources/responses>
- OpenAI Web Search Guide: <https://platform.openai.com/docs/guides/tools-web-search?api-mode=responses>
- Anthropic Web Search Tool: <https://docs.anthropic.com/en/docs/agents-and-tools/tool-use/web-search-tool>

## 1. 目标

Codex 侧只理解 Responses 协议。AI Gateway 对 Anthropic/Claude、GLM Anthropic-compatible 等上游时，需要做到：

- Codex 发来的 `tools[].type = "web_search"` 可以触发上游 server-side web search。
- 上游搜索调用回包必须转回 Responses `web_search_call`，让 Codex App / CLI / TUI 识别为搜索事件。
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
    "query": "weather seattle"
  }
}
```

Codex 源码 `core/src/event_mapping.rs` 会把 `web_search_call.action` 转成 UI 和会话历史里的 WebSearch item。也就是说，`id` 与 `action` 是 Codex web search UI 的关键字段。

## 3. Anthropic Messages 出站工具

Gateway 将 Responses `web_search` / `web_search_preview` 转成 Anthropic server tool：

```json
{
  "type": "web_search_20250305",
  "name": "web_search"
}
```

### 3.1 为什么默认使用 `web_search_20250305`

Anthropic 官方和多家 Anthropic-compatible 上游都支持 `web_search_20250305`。`web_search_20260209` / `web_search_20260318` 属于更高版本能力，一些上游会直接报错：

```text
Input tag 'web_search_20260209' found using 'type' does not match any of the expected tags ...
```

因此 Gateway 默认使用 `web_search_20250305`。后续如果需要启用更高版本 web search，应通过 provider capability/profile 显式开启，而不是全局切换。

### 3.2 字段映射

| Codex Responses | Anthropic Messages | 当前策略 |
| --- | --- | --- |
| `type = "web_search"` | `type = "web_search_20250305"` | 固定映射。 |
| `name` | `name = "web_search"` | 固定映射。 |
| `filters.allowed_domains` | `allowed_domains` | 已映射。 |
| `allowed_domains` | `allowed_domains` | 已支持，用于兼容已经扁平化的配置。 |
| `blocked_domains` | `blocked_domains` | 已透传。 |
| `user_location` | `user_location` | 已透传。 |
| `max_uses` | `max_uses` | 已透传。 |
| `external_web_access` | 无直接等价字段 | 不透传；Anthropic server tool 本身表示上游搜索。 |
| `index_gated_web_access` | 无直接等价字段 | 不透传。 |
| `search_context_size` | 无稳定等价字段 | 不透传。 |
| `search_content_types` | 无稳定等价字段 | 不透传。 |

不透传字段不是删除 Codex 能力，而是 Anthropic `web_search_20250305` 没有稳定等价字段。为了兼容第三方上游，Gateway 不向 Anthropic-compatible API 发送未知字段。

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
    "query": "Portugal World Cup result"
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

Gateway 转成 Responses SSE：

| Anthropic SSE | Responses SSE |
| --- | --- |
| `content_block_start(server_tool_use)` | 延迟处理，等 query 可用。 |
| `content_block_delta(input_json_delta)` | 累积 query；query 可用后发 `response.output_item.added`。 |
| `content_block_start(web_search_tool_result)` | 绑定 `tool_use_id`，完成对应搜索。 |
| search completed | `response.output_item.done`，item 为 `web_search_call`。 |
| `content_block_delta(citations_delta)` | `response.output_text.annotation.added`。 |
| `content_block_delta(text_delta)` | `response.output_text.delta`。 |
| text block done | `response.content_part.done` 与 message `response.output_item.done`，annotations 放入最终 content part。 |

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

随后才通过 `input_json_delta` 给 query。Gateway 必须等 query 非空后再发 Responses `web_search_call`，否则 Codex 会看到空搜索。

### 5.2 忽略 Anthropic 内部空 server_tool_use

标准 Anthropic API 可能同时出现两类搜索块：

- 模型显式 `tool_use(name=web_search)`，带 query。
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

`glm_anthropic` profile 仍然从 Codex Responses `web_search` 出站构造：

```json
{
  "type": "web_search_20250305",
  "name": "web_search"
}
```

但 GLM 回包可能使用：

- `server_tool_use.name = "web_search_prime"`
- `tool_result`，而不是 Anthropic 原生 `web_search_tool_result`
- 私有文本块：`Z.ai Built-in Tool: web_search_prime`
- 私有摘要块：`web_search_prime_result_summary`

Gateway 在 `GlmAnthropic` profile 中：

- 接受 `web_search_prime` 作为内部搜索 server tool。
- 把它转成标准 Responses `web_search_call`。
- 不把 GLM 私有文本块泄漏给 Codex。
- 不把搜索结果塞进 `web_search_call.action.result`，保持 Codex 期望的标准 `action.type/query` 形态。

## 8. 已知限制

- `search_content_types=["image"]` 没有映射到 Anthropic `web_search_20250305`。如果后续 Anthropic 或某厂商提供稳定字段，需要通过 provider capability 显式开启。
- `external_web_access=false` 暂不降级为 cached search。Anthropic server tool 默认表示上游搜索，Gateway 不做本地搜索。
- `web_search_tool_result.content[].encrypted_content` 不透传给 Responses。该字段主要服务 Anthropic 自身后续引用，不属于 Codex 当前消费的 `web_search_call` 形态。
- 如果上游只支持更旧或私有 search tag，需要新增 provider profile，不应在通用 Anthropic profile 中硬编码厂商差异。

## 9. 代码落点

| 模块 | 职责 |
| --- | --- |
| `providers/anthropic_messages/types.rs` | 定义默认 `ANTHROPIC_WEB_SEARCH_TYPE = "web_search_20250305"`。 |
| `providers/anthropic_messages/request_tools.rs` | Responses `web_search` -> Anthropic `web_search_20250305`。 |
| `providers/anthropic_messages/response.rs` | 非流式 Anthropic content -> Responses output。 |
| `providers/anthropic_messages/stream_state.rs` | Anthropic SSE 事件分发。 |
| `providers/anthropic_messages/stream_tools.rs` | `server_tool_use` / `web_search_tool_result` -> `web_search_call`。 |
| `providers/anthropic_messages/stream_message.rs` | text delta 和 citation delta -> Responses output_text。 |
| `providers/anthropic_messages/citations.rs` | Anthropic citation -> Responses annotation。 |
| `providers/anthropic_messages/tests.rs` | web search、internal empty search、citation 映射测试。 |

## 10. 测试覆盖

当前应保持通过：

```powershell
cargo test --features gui --bin codexhub anthropic_web_search
cargo test --features gui --bin codexhub anthropic_internal_web_search
cargo test --features gui --bin codexhub anthropic_citations
cargo check --features gui --bin codexhub
```

重点测试点：

- Responses `web_search` 构造成 Anthropic `web_search_20250305`。
- Responses `filters.allowed_domains` 映射到 Anthropic `allowed_domains`。
- Anthropic `server_tool_use` / `web_search_tool_result` 转成 `web_search_call`。
- 内部空 `server_tool_use` 不生成空 query 的 `web_search_call`。
- Anthropic `citations_delta` 转成 Responses `output_text.annotations`。
