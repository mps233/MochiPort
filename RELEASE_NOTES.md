# CodexHub v0.3.0

## 🎉 项目更名：codex-remote → CodexHub

项目已正式更名为 **CodexHub**，反映其作为"本地 Codex 后台总控台"的定位：远程 IM 控制 + 多模型 AI Gateway + 会话管理。

### ⚠️ 升级提醒

由于更名导致应用标识符变化（macOS Bundle ID、Windows 程序名），新版会被系统识别为不同的应用。建议：

- **macOS**：安装前先删除旧版 `Codex Remote.app`（或保留共存，手动切换）
- **Windows**：安装前先删除旧版 `Codex Remote.exe`
- **Linux**：直接替换 AppImage 即可

旧版配置文件和状态会自动识别（state 文件名已改为 `codexhub-state.json`，环境变量改为 `CODEXHUB_HOME`）。

## 更名涉及的变化

- 产品显示名：`Codex Remote` → `CodexHub`
- 二进制文件名：`codex-remote` / `codex-remote.exe` → `codexhub` / `codexhub.exe`
- macOS Bundle Identifier：`com.codexremote.app` → `com.codexhub.app`
- 配置与状态文件：`codex-remote-state.json` → `codexhub-state.json`
- App Support 备份目录：`Codex Remote` → `CodexHub`
- 环境变量：`CODEX_REMOTE_HOME` → `CODEXHUB_HOME`

完整更名文档：[docs/rename-codexhub.zh-CN.md](https://github.com/happy-loki/codexhub/blob/main/docs/rename-codexhub.zh-CN.md)

## 其他改进（自 v0.2.15）

- 将 AI Gateway 渠道列表的管理入口（新增 / 编辑 / 删除）移至列表上方，macOS 端无需滚动到底部
- 改进 Anthropic gateway 诊断信息和工具映射
- 优化 GUI 主题、onboarding 界面、provider 对话框
- 优化 AI Gateway 控件布局
- 自动格式化清理部分代码（移除 BOM、统一缩进）

## 验证

- `cargo build --release --features gui --bin codexhub`（注意二进制名已变更）

---

有问题可以提 GitHub issue，也可以关注 README 里的公众号后直接发消息给我。
