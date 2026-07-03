# CodexHub v0.3.21

## 改进内容

- Anthropic / GLM 主请求的缓存断点策略回归**模仿 Claude Code** 的形态：
  - **system**：只在最后一条 text block 打 1 个断点（不变）。
  - **messages**：单个断点跟着会话尾巴走——最后一条 `role==user`/`assistant` 消息（跳过 mid-conversation system；tool_result 是 user、tool_use 是 assistant，agent 循环尾部天然覆盖）。
  - **tools**：不打断点（维持现状）。
  - 主请求恢复注入 `metadata.user_id`（跟会话稳定，与 Claude Code 一致）。
- 唯一增强（相对旧的「跟尾巴」实现）：落点优先该消息的**最后一个 text block**，无 text 才落最后一个 content block（覆盖 tool_result-only / tool_use-only）。这更贴近 Claude Code 的实际行为——它把断点打在尾块的 text 上。
- 撤销 v0.3.20 的 OpenCode AUTO 方案（tools 断点 + 4-断点预算 + 仅 user）。生产验证显示偶发 miss 的主因指向 Anthropic 服务端分片最终一致性，非本地断点策略可根治；与 Claude Code 保持一致的单断点是最简、风险最低的默认。

## 保持不变

- headers / anthropic-beta / auth 的 Claude Code 指纹（含 `context-1m-2025-08-07`），是 1M 上下文等能力的前提。
- 内部 web-search 独立请求（`internal_web_search_body`）整体仍模拟 Claude Code。

## 验证

- `cargo fmt`
- `cargo test`（358 项通过）

---

有问题可以提 GitHub issue，也可以关注 README 里的公众号后直接发消息给我。
