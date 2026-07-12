CodexHub v0.3.36

本版本重点适配 GPT-5.6 Sol、Terra、Luna 的 Responses Lite 工具协议，并完善 Web Search、生图和会话历史拉取。

## GPT-5.6 Responses Lite

- 识别 `input[].type = additional_tools`、namespace tool 和 `tool_mode = code_mode_only` 下的嵌套工具结构，兼容 GPT-5.6 系列通过 `exec` 调用工具的新方式。
- 日志分别保留 Codex 原始请求和 Gateway 实际上游请求，便于判断工具是由 Codex、Gateway 还是上游处理。
- 图片工具过滤同时覆盖旧版 hosted `image_generation`、新版 `image_gen` namespace、`additional_tools` 以及 `exec` 描述中的 `image_gen__imagegen`。

## Web Search

- GPT-5.6 Responses Lite 请求使用 OpenAI Responses 渠道时，在顶层注入标准 hosted `web_search`，恢复当前上游已支持的搜索能力。
- 注入逻辑保持幂等，并保留原有工具；当请求已经包含原生 `web.run` 时不会重复注入 hosted Web Search。
- 本版本暂不伪装 `/alpha/search` 或原生 `web.run` runtime，避免出现工具可见但执行 404 的半成品状态。

## Image Generation

- 新增 `/ai-gateway/v1/images/generations` 和 `/ai-gateway/v1/images/edits`，支持 Codex 新版 standalone `image_gen` 工具调用独立 Images API。
- 按 `gpt-image-2` 选择已启用 Provider，并支持通过 `modelAliases` 映射上游图片模型。
- 复用现有 Provider 的权重、API Key、超时、传输重试和错误映射；未配置图片模型时返回明确的 `invalid_model`，不再本地 404。
- 图片生成和编辑会进入请求日志，记录渠道、状态、耗时和 usage；图片 base64、URL 与鉴权信息均脱敏，只保留 MIME 和大小摘要。
- 无模型 alias 时直接转发原始请求字节，避免复制大型图片 data URL。

## Session History

- 会话历史改为直接查询已初始化且健康的 Codex App、CLI 或 VS Code remote-control 连接，优先使用当前活跃和最近有响应的连接。
- `thread/list` 使用 `useStateDbOnly = true`，只扫描 CLI 和 VS Code 交互会话，避免慢速文件系统全量发现。
- 支持分页拉取，并在当前连接失败时切换到其他健康连接，提高历史会话列表的打开速度和稳定性。

## Documentation

- 新增 GPT-5.6 Responses Lite Web Search 协议、限制和未来原生 `web.run` 接入说明。
- 新增 Codex 5.5 与 5.6 生图链路变化、Images API 路由和升级审计文档。
