CodexHub v0.3.28

本次版本重点修复 AI Gateway 与远程控制诊断链路：

- 修复 DeepSeek Chat Completions 出站时的工具调用回合处理，避免 Codex 工具调用上下文被错误分类。
- 修复 AI Gateway 模型映射编辑器会把所有上游模型自动生成小写 Codex 映射的问题；现在只保留明确支持的 Claude 友好别名，第三方模型不会再被自动映射。
- 增强 remote-control 超时诊断：超时日志会记录绑定 client 的初始化、恢复、pong 和 pending 状态，方便判断端点沉默还是恢复中。
- 连接诊断导出现在会保留 remote-control 协议诊断行，同时脱敏 IM client_key，便于用户上传日志排查飞书/微信/Telegram 远控问题且不暴露会话身份。
