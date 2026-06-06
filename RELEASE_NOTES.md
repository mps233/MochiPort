# Codex Remote v0.2.12

本次版本调整了 thread list 交互和支持多语言。

## 更新内容

- 调整微信、飞书、Telegram 的 thread list 和目录选择展示，减少冗余内容并改善分页提示位置。
- Telegram thread list 支持 inline keyboard 选择。
- 桌面 GUI 增加 `Language / 语言` 菜单，支持 `中文（简体）` 与 `English`。
- IM 端提示词、菜单、目录选择和会话控制文案支持跟随桌面端语言配置。

## 兼容性说明

- 语言配置统一写入当前 `config.toml`，不再读取旧的 GUI 独立配置文件。
- 语言切换需要重启 `codex-remote` 后生效。

## 验证

- `cargo fmt`
- `cargo check --features gui --bin codex-remote`
- `cargo build --release --features gui --bin codex-remote`
