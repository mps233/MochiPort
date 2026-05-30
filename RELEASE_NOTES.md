# Codex Remote v0.2.4

本次版本继续修复 Windows GUI 中飞书机器人接入二维码难以扫码的问题。

## 更新内容

- 飞书扫码弹窗现在支持手动拖拽放大。
- 二维码区域使用独立可扩展容器，窗口变大时二维码区域会同步展开。
- 二维码使用更高分辨率底图并按比例完整适配容器。
- 增加“扫码失败？打开飞书确认链接”兜底入口，扫码不稳定时可以直接打开确认链接。

## 使用方式

1. 下载对应平台安装包。
2. 打开 Codex Remote，接入飞书机器人。
3. 在“Codex 接入”里填写第三方 Base URL 和 API Key，并点击“写入配置”。
4. 启动 Codex App 或 Codex VS Code 插件，打开 remote-control / 控制这台电脑。
5. 回到飞书开始会话。

## 平台产物

- macOS: `Codex Remote.dmg`
- Windows: `Codex Remote Windows.zip`
