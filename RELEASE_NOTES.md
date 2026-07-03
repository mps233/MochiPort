# CodexHub v0.3.18

## 改进内容

- Anthropic / GLM 渠道的 messages 缓存断点从「双滚动」回退为「单滚动」，与 Claude Code 原生行为对齐：只在会话尾部最后一条 `user`/`assistant` 消息的尾块上打一个断点（跳过 mid-conversation `system`）。
  - 依据：Anthropic 读缓存时从断点向前回溯约 20 个 block，而 Codex agent 循环每轮尾部只增长几个 block，远小于回溯窗口，单断点即可稳定命中上一轮写入的前缀。第二个较早的读取锚在本链路负载下不提供额外命中，却每轮多写一份缓存（cache write 计费 1.25x），并因多标一个随轮移动的 marker 而扩大历史消息 `cache_control` 的增删面。
  - 断点预算由 ≤3 降至 ≤2（system 末尾 1 + messages 尾部 1），更稳地控制在 4 上限内。
  - 更新了缓存文档第 5/6/8 节与相关测试。

## 验证

- `cargo fmt`
- `cargo test`（354 项通过）

---

有问题可以提 GitHub issue，也可以关注 README 里的公众号后直接发消息给我。
