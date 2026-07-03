# CodexHub v0.3.16

## 改进内容

- Anthropic Messages 渠道的消息缓存断点放宽到 `user` + `assistant` 双滚动。此前只在最后两条 `role==user` 消息上打断点，现在改为在最后两条 `user`/`assistant` 消息的尾块上各打一个（跳过 mid-conversation `system`）。抓包实证表明 Claude Code 本身就会把消息断点打在 assistant 尾块（含 `tool_use`）上，放宽后两个断点稳定贴着 append-only 的会话尾巴走，读侧回溯更容易命中上一轮写入的前缀。
- 补齐 Anthropic prompt caching 生产验证文档。基于真实请求日志（SQLite `ai-gateway-request-logs`）逐轮对比 system/tools 哈希、断点落点与 `cache_read`/`cache_write` 曲线，确认双滚动在前缀稳定时命中率可达 92-94%；并定位到偶发 `cache_read=0` 属于 Anthropic API 服务端分片最终一致性延迟，miss 后下一轮自动恢复，非 Gateway 逻辑问题。

## 验证

- `cargo fmt`
- `cargo test`（anthropic_messages 模块 80 项通过）

---

有问题可以提 GitHub issue，也可以关注 README 里的公众号后直接发消息给我。
