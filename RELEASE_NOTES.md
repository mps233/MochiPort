# Codex Remote v0.2.6

本次版本聚焦 GUI 简化、第三方 provider 管理和 GitHub Releases 手动更新检查。

## 更新内容

- Codex 接入页精简为 provider 列表 + 详情表单，支持新增、保存、删除和启用。
- `启用` 会保存当前 provider，并把 Codex App 的 `model_provider` 切换到该 provider。
- `清除 Codex 接入` 只移除根级 `chatgpt_base_url` 和 `model_provider`，保留已保存 provider、认证和其他配置。
- 移除单独的本地服务页面，GUI 打开时自动启动 backend，退出时只关闭本次启动的 backend。
- 飞书接入页支持断开后 `重新接入`，并将首次/更换扫码入口统一为 `扫码使用新机器人`。
- About 页面保留一个可点击的 GitHub 项目主页链接。
- 新增 `Help -> Check for Updates`，通过 GitHub Releases 检查新版本并引导打开下载页。
- Release workflow 生成并上传 `latest.json`，供 GUI 手动更新检查使用。

## 验证

- `cargo test --features gui`
- `cargo build --release --features gui --bin codex-remote`
