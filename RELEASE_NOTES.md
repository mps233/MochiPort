# CodexHub v0.3.12

## 改进内容

- 彻底修复安装在受保护目录（如 `C:\Program Files\CodexHub`）时保存配置报 `failed to write config` / HTTP 500 的问题。v0.3.11 只修复了 CLI 启动路径，GUI 仍走独立的配置定位逻辑而未生效；本版补齐 GUI（`daemon` 子进程）路径：exe 同目录不可写时，配置自动回退到用户目录 `%LOCALAPPDATA%\CodexHub\config.toml`，无需管理员权限即可保存。

## 已知问题

- 在 CodexHub 模式下，Codex App 插件页点击 `computer-use` 进入详情页可能显示「未找到插件」。这是 Codex App 前端对 bundled 本地插件详情的展示行为，`computer-use` 功能本身可正常使用，不影响实际调用。

## 验证

- `cargo fmt`
- `cargo build --release --features gui --bin codexhub`
- 端到端：在只读目录放置 exe + `config.toml`，启动后确认配置实际回退到 `%LOCALAPPDATA%\CodexHub\config.toml`。

---

有问题可以提 GitHub issue，也可以关注 README 里的公众号后直接发消息给我。
