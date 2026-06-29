# CodexHub v0.3.11

## 改进内容

- 修复安装在受保护目录（如 `C:\Program Files\CodexHub`）时保存配置报 `failed to write config` / HTTP 500 的问题：当 exe 同目录不可写时，配置自动回退到用户目录 `%LOCALAPPDATA%\CodexHub\config.toml`，不再要求管理员权限即可保存大模型厂商等配置。
- 检查更新安装时自动退出 CodexHub（含本地 backend）：启动安装器后主动让出可执行文件占用，避免 Windows 反复弹出「关闭正在运行的程序」提示。
- 统一三个 IM 平台（微信、Telegram、飞书）未接入会话时的引导文案，明确选项 2 为「恢复历史会话或接入当前 Codex 活跃会话」，中英文同步。

## 已知问题

- 在 CodexHub 模式下，Codex App 插件页点击 `computer-use` 进入详情页可能显示「未找到插件」。这是 Codex App 前端对 bundled 本地插件详情的展示行为，`computer-use` 功能本身可正常使用，不影响实际调用。

## 验证

- `cargo fmt`
- `cargo check --features gui --bin codexhub`

---

有问题可以提 GitHub issue，也可以关注 README 里的公众号后直接发消息给我。
