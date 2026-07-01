# CodexHub v0.3.14

## 改进内容

- 恢复 GLM 的逐字（打字机）输出。GLM 走 Anthropic 兼容协议，上游本来就是逐 token 推送文本的，但此前为了过滤 GLM 私有网搜片段（`web_search_prime` 工具块与结果摘要），适配层会把整段文本缓冲到结束才一次性下发，导致前端「瞬间出一大片」。本版改为默认流式、按需缓冲：普通回答逐 token 实时下发，只有真正出现私有网搜标记时才保留那一小段做清洗，私有片段仍然不会透传给 Codex。
- Anthropic 内部网搜链路改为逐 token 流式。启用 web search 的对话此前要等整轮上游流跑完才转发，现在每一轮上游流经转换器实时转发，配合统一的事件序号管理，网搜过程中的正文也能边生成边显示。
- 修复 Anthropic 首 token 延迟（TTFT）在请求日志里一直为空的问题。几乎所有 Codex 请求都带 web search 工具、走内部网搜缓冲路径而没有 delta 事件，导致日志流看不到首 token。本版改为从首个内容 token 记录 TTFT，Anthropic/GLM 请求现在能正确显示 TTFT。
- 请求日志默认关闭。日志功能默认不再开启，需要排查时在设置里手动打开即可，避免默认写入大量请求/响应 JSON。
- 写缓存（write cache）显示细分与命中占比。请求日志详情新增 5 分钟 / 1 小时写缓存 token 的细分展示，并以百分比呈现，便于观察缓存效果。
- 无 VPN 环境下 Codex 启动更快。默认关闭 Codex 的 OTLP 遥测导出器（日志与指标两路都关），此前在无法访问遥测端点时启动会长时间卡顿，现在不再因等待遥测连接而拖慢重启。
- 优化多轮对话的 Anthropic prompt cache 命中率。对齐 Claude Code 的做法，采用双滚动 `cache_control` 断点标注消息历史，抹平多轮命中率的锯齿波动。
- 仪表盘状态布局更紧凑，信息密度更高、更利于快速扫读。

## 修复

- 修复历史几个版本 GitHub Release 页没有发布说明的问题。三个平台的发布流程此前未把 `RELEASE_NOTES.md` 作为 release 正文，导致 Release 页只显示一个空标题；本版起统一读取 `RELEASE_NOTES.md` 生成正文。

## 已知问题

- 在 CodexHub 模式下，Codex App 插件页点击 `computer-use` 进入详情页可能显示「未找到插件」。这是 Codex App 前端对 bundled 本地插件详情的展示行为，`computer-use` 功能本身可正常使用，不影响实际调用。

## 验证

- `cargo test --bin codexhub ai_gateway::`（243 项通过）

---

有问题可以提 GitHub issue，也可以关注 README 里的公众号后直接发消息给我。
