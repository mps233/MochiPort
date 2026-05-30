# Codex Remote v0.2.3

本次版本修复 Windows GUI 中飞书机器人接入二维码过大时难以扫码的问题。

## 更新内容

- 修复飞书扫码弹窗里二维码在长授权链接下被放得过大、底部显示空间不足的问题。
- 二维码渲染现在限制在稳定尺寸内，同时保留静区，提升手机扫码识别率。
- 调整扫码弹窗布局，避免提示文字和按钮挤压二维码区域。

## 使用方式

1. 下载对应平台安装包。
2. 打开 Codex Remote，接入飞书机器人。
3. 在“Codex 接入”里填写第三方 Base URL 和 API Key，并点击“写入配置”。
4. 启动 Codex App 或 Codex VS Code 插件，打开 remote-control / 控制这台电脑。
5. 回到飞书开始会话。

## 平台产物

- macOS: `Codex Remote.dmg`
- Windows: `Codex Remote Windows.zip`
