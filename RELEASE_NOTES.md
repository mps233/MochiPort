# Codex Remote v0.2.5

本次版本聚焦 Codex App 第三方 provider 管理和本地插件可见性。

## 更新内容

- Codex 接入页改为更直观的 Provider 管理，支持新增、保存、启动和删除 provider。
- 启动 provider 只切换 `model_provider`，保存 provider 不会自动切换当前使用项。
- 卸载只移除根级 `chatgpt_base_url` 和 `model_provider`，保留已有 `[model_providers.*]`、认证和环境配置。
- GUI 关闭时只关闭本地 backend，不再触碰 Codex App 配置。
- 自动写入本地可用的 OpenAI bundled marketplace，让新用户能看到本地插件入口。
- Windows 默认使用 `127.0.0.1`，避免 `localhost` 解析到 IPv6 导致本地服务不可达。

## 验证

- `cargo test`
- `cargo build --release --features gui --bin codex-remote`
