CodexHub v0.3.31

紧急修复连接状态显示和新版 VS Code 插件适配问题：

- 修复 Codex App remote-control 已可用但状态仍显示未连接/初始化中的问题。
- 统一 Codex App、VS Code 插件、Codex CLI 三个入口的状态判断，避免互相串状态。
- 适配新版 VS Code Codex 插件启动参数，确保自动注入 remote-control 仍然生效。
