# Codex Remote v0.2.1

本次版本整理了 Codex Remote 的正式使用路径：本地 remote-control backend + 飞书 bridge。

## 更新内容

- 支持 macOS 和 Windows。
- 支持 Codex App 接入本地 remote-control backend。
- 支持 Codex VS Code 插件接入本地 remote-control backend。
- GUI 自动启动本地 backend，退出 GUI 时关闭本次启动的 backend。
- 写入配置时支持第三方 Base URL、API Key 和 provider 配置。
- 首页状态更直观：只要“本地服务”“飞书”“Codex 控制通道”都是绿色，就可以直接使用。
- Codex App 的“连接”设置页不需要显示远程连接设备列表，本项目不依赖那个列表展示。
- 优化配置读取速度，GUI 会先读取本地 Codex 配置，不再等待 bridge 网络初始化。
- 修复 macOS 关闭窗口时 timer 残留导致的意外退出。
- 修复 Windows GUI 关闭时等待环境变量清理导致的卡顿。

## 使用方式

1. 下载对应平台安装包。
2. 打开 Codex Remote，接入飞书机器人。
3. 在“Codex 接入”里填写第三方 Base URL 和 API Key，并点击“写入配置”。
4. 启动 Codex App 或 Codex VS Code 插件，打开 remote-control / 控制这台电脑。
5. 回到飞书开始会话。

## 平台产物

- macOS: `Codex Remote.dmg`
- Windows: `Codex Remote Windows.zip`
