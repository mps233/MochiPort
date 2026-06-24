# CodexHub v0.3.1

## 改进内容

- 新增 AI Gateway 请求日志开关，可以在界面中启用或关闭请求日志记录。
- 改进上游请求失败时的错误摘要，补充连接、超时、请求、解码、状态码和底层 cause 信息，便于排查第三方渠道问题。
- 调整主窗口首次打开尺寸，降低在 MacBook Air 等小屏设备上标题栏被顶出屏幕的概率。
- 优化 Codex 初始化说明，明确可恢复到初始化前状态，包括 ChatGPT 登录状态。
- 将入口按钮文案调整为“会话历史修复管理”，更准确表达用途。

## 验证

- `cargo fmt --check`
- `cargo check --features gui`
- `cargo build --release --features gui --bin codexhub`

---

有问题可以提 GitHub issue，也可以关注 README 里的公众号后直接发消息给我。
