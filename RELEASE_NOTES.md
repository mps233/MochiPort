# CodexHub v0.3.9

## 改进内容

- 适配新版 Codex App 的 bundled 插件策略：不再写入会与 Codex App 自带 `openai-bundled` marketplace 冲突的 `[marketplaces.openai-bundled]`，初始化时确保 `browser`、`chrome`、`computer-use` 等 bundled 插件为 enabled，并清理旧版 CodexHub 写入的 bundled remote 残留状态。
- 收敛 Codex App 插件接入链路：保留基于本地 `openai-curated` 缓存的最小 remote catalog fallback，对旧 bundled remote ID 提供只读详情兼容，避免插件列表出现重复项。
- 优化 Codex App 本地 auth 写入，保持 `chatgptAuthTokens` 形态以满足 remote-control 鉴权，同时把本地 dummy token 仅用于本机入口。
- 自动更新增加下载进度对话框：用可取消的进度条替代原来的静态提示，下载更新包时实时显示字节进度。
- 对齐 Anthropic web search 历史映射，修正多轮会话里 web search 工具调用与结果的回放一致性。
- 修正更新说明展示并压缩请求日志存储，减小 AI Gateway 请求日志体积。

## 已知问题

- 在 CodexHub 模式下，Codex App 插件页点击 `computer-use` 进入详情页可能显示「未找到插件」。这是 Codex App 前端对 bundled 本地插件详情的展示行为，`computer-use` 功能本身可正常使用，不影响实际调用。

## 验证

- `cargo fmt`
- `cargo build --release --features gui --bin codexhub`

---

有问题可以提 GitHub issue，也可以关注 README 里的公众号后直接发消息给我。
