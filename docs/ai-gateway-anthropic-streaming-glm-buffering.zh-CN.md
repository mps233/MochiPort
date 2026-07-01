# AI Gateway Anthropic 流式与 GLM 增量过滤设计

更新时间：2026-07-01

状态：已落地。GLM Anthropic profile 由「整段缓冲」改为「默认流式、按需缓冲」。

本文记录 codexhub AI Gateway 在 Anthropic Messages 出站链路上如何把上游 SSE
转成 Codex Responses SSE，重点说明 GLM profile 私有网搜文本过滤为什么会破坏
打字机效果，以及本次如何用增量切割 `split_streamable` 恢复逐 token 流式。

相关文档：

- [`ai-gateway-anthropic-messages.zh-CN.md`](ai-gateway-anthropic-messages.zh-CN.md)：Anthropic Messages adapter 总体设计（第 7 章为流式映射）。
- [`ai-gateway-glm-anthropic-integration.zh-CN.md`](ai-gateway-glm-anthropic-integration.zh-CN.md)：GLM Anthropic 接入说明。
- [`ai-gateway-web-search-protocol.zh-CN.md`](ai-gateway-web-search-protocol.zh-CN.md)：web search 协议对接与 GLM 私有网搜差异。

## 1. 背景与问题

Anthropic Messages 流式回包是 SSE，事件形态与 Codex Responses 不同，必须由
`AnthropicSseToResponsesSse`（`stream.rs`）逐事件转换成 Responses SSE。文本部分的
标准链路是：

```text
content_block_start(text)
content_block_delta(text_delta)   # 上游逐 token 推送
content_block_stop
```

转成 Responses：

```text
response.output_item.added(message)
response.content_part.added(output_text)
response.output_text.delta        # 每个 token 一条，前端据此实现打字机
response.output_text.done
response.content_part.done
response.output_item.done
```

关键点：Codex 前端的打字机效果依赖持续到达的 `response.output_text.delta`。
只要 delta 一次性到齐，前端就会「瞬间出一整段」，失去逐字输出的观感。

### 1.1 GLM 为什么会整段输出

GLM 走 Anthropic 兼容协议，上游 **本身是逐 token 推 `text_delta` 的**（实测一条
回答会被拆成十多个小片段）。但历史实现里，`handle_text_delta`
（`stream_message.rs`）在 GLM profile 下会把每个 delta 都塞进 `glm_pending_text`
缓冲，直到 `content_block_stop` / `message_stop` 才通过 `flush_glm_pending_text`
一次性 emit：

```rust
// 旧逻辑（已废弃）
if matches!(self.profile, AnthropicProviderProfile::GlmAnthropic) {
    self.glm_pending_text
        .get_or_insert_with(String::new)
        .push_str(text);
    return; // 不立即 emit delta
}
```

结果就是：即便上游是流式的，经过 Gateway 后前端也只收到一整段，没有打字机效果。

### 1.2 缓冲的初衷

这段缓冲不是无意义的。GLM 会在普通文本里混入私有网搜片段，这些片段不应透传给
Codex：

- 工具块：`**🌐 Z.ai Built-in Tool: web_search_prime**` … `*Executing on server...*`
- 结果摘要：`**Output:**` + `**web_search_prime_result_summary:** [...]`

清洗逻辑 `glm_compat::clean_private_web_search_text` 依赖 **整段文本** 才能准确
定位块的起止边界（尤其是要回溯到块前的加粗 `**` 和 `🌐` 图标、吞掉块后的空行）。
按 token 到达时如果贸然逐段吐出，很容易把私有片段的一部分先漏出去。这就是当初
选择「整段缓冲再清洗」的原因，代价是牺牲了流式。

## 2. 设计目标

1. GLM 普通回答恢复逐 token 流式（多条 `response.output_text.delta`）。
2. GLM 私有网搜文本仍然被完整过滤，不透传给 Codex。
3. 不影响其他 profile（Anthropic / Claude）既有的流式行为。
4. 兜底安全：即便切割逻辑漏判，收尾 flush 仍做一次完整清洗。

## 3. 方案：默认流式，按需缓冲

核心是新增 `glm_compat::split_streamable`，把当前缓冲切成两段：

```rust
/// 返回 (emit, keep)：
/// - emit 保证不含任何私有片段，可以立即作为 delta 下发；
/// - keep 需要写回缓冲，等待后续 token 或最终 flush 时用
///   clean_private_web_search_text 收尾。
pub(super) fn split_streamable(buf: &str) -> (String, String);
```

切割规则：

- **无私有标记**：除结尾一小段「可能是标记前缀或引导片段」的字节外，其余全部立即
  下发。保留的尾部由 `holdback_len` 计算，覆盖两类情况：
  - `marker_prefix_suffix`：缓冲结尾正好是某个标记的前缀（例如标记被拆在两个
    token 边界上）。
  - `trailing_leadin_len`：结尾是标记的引导片段，即连续的 `**`、`🌐`、内联空白
    （不含换行），这些可能是私有工具块 `** 🌐 Z.ai ...` 的开头。
- **出现完整私有块**（标记与其结束符都已到达）：剔除该块，对剩余部分递归调用
  `split_streamable`，继续把安全内容流式吐出。
- **出现不完整私有块**（标记已现、结束符未到）：从块起点开始整体保留到缓冲，等
  后续 token 补齐。

工具块与结果摘要都复用清洗模块里已有的边界函数（`tool_block_start`、
`output_marker_start`、`result_summary_end`、`consume_trailing_blank_lines`），
保证「流式切割」与「最终清洗」对边界的判定完全一致。

### 3.1 delta 处理链路

`handle_text_delta` 现在每收到一个 GLM delta 就切一次，emit 安全前缀、keep 尾部：

```rust
if matches!(self.profile, AnthropicProviderProfile::GlmAnthropic) {
    let buf = self.glm_pending_text.get_or_insert_with(String::new);
    buf.push_str(text);
    let (emit, keep) = glm_compat::split_streamable(buf);
    *buf = keep;
    if !emit.is_empty() {
        self.emit_message_text(&emit, queue); // 逐段下发，形成打字机
    }
    return;
}
```

### 3.2 收尾兜底

`content_block_stop` 与 `message_stop`（经 `handle_done`）仍会调用
`flush_glm_pending_text`，但语义从「唯一出口」降级为「残余兜底」：

```rust
pub(super) fn flush_glm_pending_text(&mut self, queue: &mut VecDeque<Bytes>) {
    let Some(text) = self.glm_pending_text.take() else { return; };
    if let Some(cleaned) = glm_compat::clean_private_web_search_text(&text) {
        self.emit_message_text(&cleaned, queue); // 对残余缓冲做一次完整清洗
    }
    self.close_message_item(queue);
}
```

正常情况下 `keep` 只剩很短的尾部（或为空），flush 时清洗一次即可；私有块因为在
切割阶段就被识别并保留，最终一定经过完整清洗，不会漏出。

## 4. 代码落点

- `src/ai_gateway/providers/anthropic_messages/glm_compat.rs`
  - 新增 `split_streamable` / `holdback_len` / `marker_prefix_suffix` / `trailing_leadin_len`。
  - 复用既有 `clean_private_web_search_text` 及其边界辅助函数。
- `src/ai_gateway/providers/anthropic_messages/stream_message.rs`
  - `handle_text_delta`：GLM 分支改为增量切割 + 立即 emit。
  - `flush_glm_pending_text`：降级为残余兜底清洗。
- `src/ai_gateway/providers/anthropic_messages/stream_state.rs`
  - `content_block_stop` / `handle_done` 的 flush 调用点保持不变。

## 5. 边界与已知取舍

- **尾部延迟一拍**：为防止标记被 token 边界拆开，结尾若是标记前缀或 `**`/`🌐`/
  内联空白，会被保留到下一个 delta 或收尾时才吐出。对普通文本，这最多让极少量
  结尾字符晚一个 delta，观感无影响。
- **仅 GLM profile 生效**：其他 Anthropic profile 不进入该分支，行为不变。
- **依赖标记文本稳定**：切割与清洗都以 `web_search_prime` / `result_summary` /
  `Executing on server...` 等固定标记为准；若智谱调整私有片段格式，需同步更新
  `glm_compat.rs` 里的常量与两侧逻辑。

## 6. 测试覆盖

`cargo test --bin codexhub ai_gateway::` 全量通过。相关用例：

- `streams_glm_plain_text_token_by_token`（tests.rs）：纯文本产出多条独立
  `response.output_text.delta`，且拼接结果与最终 message 一致，验证打字机恢复。
- `streams_glm_private_web_search_text_is_filtered`（tests.rs，既有）：私有网搜
  文本被过滤，输出流中不含 `web_search_prime` / `result_summary`。
- `streams_glm_web_search_prime_as_responses_sse`（tests.rs，既有）：GLM
  web_search_prime 正确转成 Responses 的 `web_search_call` 事件。
- `split_streams_plain_text_immediately` / `split_holds_back_partial_marker_lead_in`
  / `split_filters_private_tool_block_but_streams_rest`（glm_compat.rs）：直接覆盖
  `split_streamable` 的三类切割路径。

## 7. 后续

- 若接入其他会在正文混入私有片段的 Anthropic-compatible 厂商，可复用
  `split_streamable` 的「默认流式 + 标记感知保留」思路，把厂商专属标记抽象成参数。
- 可考虑把 holdback 的最大保留长度纳入监控，避免异常上游把整段文本长期滞留在
  缓冲里。
