# CodexHub v0.3.20

## 改进内容

- Anthropic / GLM 主请求的缓存断点策略重写，对齐业界干净 API 客户端（OpenCode / LangChain 缓存中间件）的默认 AUTO 方案，替换此前反复调整的「跟尾巴」单/双滚动：
  - **tools**：在最后一个 tool 定义上打 1 个断点（此前不打），把整段工具定义缓存成独立可复用前缀。
  - **system**：维持在最后一条 text block 打 1 个。
  - **messages**：只在最后一条 `role==user` 消息打 1 个断点，落点优先该消息最后一个 text block（无 text 则最后一个 content block，覆盖 tool_result-only）。**不再标记 assistant/tool_use 尾块**。
  - **4-断点预算守卫**：按 tools → system → messages 顺序从共享的 4 个名额扣减，超额从 messages 尾先丢并告警，防多 system block 撞 4 上限被整请求拒绝。
- 根因：assistant/tool_use 尾块的相对位置每轮移动，把断点落在它上面会导致下一轮摘除该历史块的 `cache_control`，实测（生产日志 3741→3742）表明这会破坏前缀哈希、令上一轮写入的整段缓存读不回来（cache_read 从 ~90k 崩到 ~13k）。只标稳定推进的最后一条 user，把「被摘除 cache_control 的历史块」降到最少。
- 撤销主请求的 `metadata.user_id` 注入（v0.3.19），主对话请求回到干净 API 客户端形态。

## 保持不变

- 请求 headers / anthropic-beta / auth 仍保留 Claude Code 指纹（含 `context-1m-2025-08-07` 等），是 1M 上下文等能力的前提，本次不动。
- 内部 web-search 独立请求（`internal_web_search_body`）整体仍模拟 Claude Code，未改动。

## 验证

- `cargo fmt`
- `cargo test`（356 项通过，含 AUTO 落点与 4-断点预算新用例）

---

有问题可以提 GitHub issue，也可以关注 README 里的公众号后直接发消息给我。
