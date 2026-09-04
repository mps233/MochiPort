# MochiPort 项目工作规则

本文件适用于整个仓库。进行代码修改、构建交接和界面验证时，应遵守以下规则。

## MochiPort 界面截图

- 默认只截 MochiPort 当前需要检查的应用窗口，不截整个屏幕。
- 正式 App 必须使用完整路径定位：
  `/Users/miaopasi/codexhub/outputs/MochiPort.app`。
- 不要只用应用名称或 Bundle ID 定位，因为 Xcode 构建产物和正式 App 可能使用相同 Bundle ID。
- 截图中不得出现桌面、菜单栏、Dock、终端、Codex 或其他应用窗口。
- 如果 MochiPort 同时存在多个窗口，只截与当前任务对应的窗口；界面改动默认截主窗口中的目标页面。
- 优先通过 Computer Use 针对上述完整 App 路径读取窗口状态和切换页面；保存正式截图时使用 macOS 原生窗口截图模式，例如 `screencapture -l <窗口ID>`，并保留系统窗口投影。
- 不得使用关闭窗口阴影的 `screencapture -o`，也不得把窗口外接矩形直接裁成不透明图片；正式 PNG 必须保留透明圆角，四角不得出现白色或其它底色填充。
- 发送截图前检查图片四周：边界应包含完整的 MochiPort 窗口圆角和系统投影，但不得包含屏幕背景或其他窗口。若仍包含桌面背景、白色四角或其他窗口，必须重新使用原生窗口模式截图，不能把全屏截图直接交付。
- 只有用户明确要求查看桌面整体布局、多个窗口关系或全屏效果时，才允许截全屏。

## 构建和运行交接

GUI 与 daemon 的分类、构建、重启和身份核对规则见
[`docs/mochiport-change-handoff.zh-CN.md`](docs/mochiport-change-handoff.zh-CN.md)。

- 仅 SwiftUI/GUI 改动可以更新并重启正式 GUI；如果内置 daemon 没有变化，必须复用当前 daemon。
- daemon 相关改动在正式 App 组装并启动后，由 GUI 通过租约、受保护任务、身份和 readiness 校验自动完成安全切换；构建交接不得要求用户再手动重启后台服务。
- 自动切换必须先 staging、排空，再原子替换 runtime 并 reload LaunchAgent；失败时恢复旧 runtime 和 plist。不得强杀、强制接管或在状态未知时干预 daemon。

## 正式版发布说明

- `RELEASE_NOTES.md` 只写当前准备发布的版本，不得附加、复制或保留旧版本的历史发行记录。
- 发布新版本时应整体替换 `RELEASE_NOTES.md` 的内容；旧版本说明保留在各自已有的 GitHub Release 页面和 Git 标签中。
- 推送正式版标签前，确认 `RELEASE_NOTES.md` 只有一个版本标题，且不包含“历史发行记录”章节。

## 工作区与 Git

- 工作区可能包含用户尚未提交的其他改动；不得回退、覆盖或顺手提交无关内容。
- 除非用户明确要求，否则不要提交、推送或创建 Pull Request。
