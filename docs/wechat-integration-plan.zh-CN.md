# 微信集成计划

本文档记录 `codexhub` 接入微信机器人的当前边界。目标是先做可用的微信 IM 通道，并保持代码模块化，不把微信协议细节塞进 `bridge.rs`。

## 目标

- GUI 支持“扫码连接微信”，接入体验和飞书一样直观。
- 微信通道能接收文本消息、选择新建或恢复 Codex thread、发送 Codex 回复。
- 微信协议细节集中在 `src/im/wechat`，公共 thread / approval / outbound 逻辑放在 `src/im/core` 和 `src/im/events`。
- Telegram 和微信共享平台中性的 IM runtime，不要求复用飞书 CardKit 渲染。

## 当前实现

- 配置新增 `[wechat]`，包含 `accountId`、`botToken`、`baseUrl`、`userId`、`botType`、`allowedUserIds`。
- GUI 的“聊天工具接入”页新增微信扫码入口和状态展示。
- 后端新增：
  - `POST /api/wechat/onboard/start`
  - `POST /api/wechat/onboard/poll`
  - `GET /api/wechat/bot`
- 运行时新增 `ImPlatformKind::Wechat` 和 `TurnOrigin::Wechat`。
- 微信模块拆分为 `api`、`polling`、`adapter`、`flow`、`store`、`types`。
- 长轮询使用 OpenClaw 微信机器人链路：
  - QR base URL: `https://ilinkai.weixin.qq.com`
  - bot type: `3`
  - QR start: `ilink/bot/get_bot_qrcode`
  - QR poll: `ilink/bot/get_qrcode_status`
  - receive: `ilink/bot/getupdates`
  - send text: `ilink/bot/sendmessage`

## 已知边界

- 当前只把微信文本和语音转文字内容接入 Codex。
- Codex 图片输出在微信侧暂时降级为本地文件路径文本；完整媒体上传和加密下载后续单独做。
- 微信 UI 没有飞书卡片能力，因此 thread list、审批和工具信息使用文本或轻量交互表达。
- `allowedUserIds` 为空时不做用户 allowlist；真实使用建议扫码后按需限制。

## 下一步

- 为微信补齐图片/文件发送能力。
- 把 Telegram/微信共用的文本 renderer 从 `telegram_renderer` 命名迁到 `im/core`。
- 给 Telegram 增加运行时轮询状态，状态概览能区分“已配置”和“轮询正常”。
- 增加一组不依赖真实 IM 网络的 flow 单元测试。
