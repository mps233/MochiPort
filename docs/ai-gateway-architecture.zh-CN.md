# AI Gateway 架构设计

更新时间：2026-06-15

本文记录 `mochiport` 内置 AI Gateway 的目标、协议边界、缓存策略和分期实现计划。它是后续实现的约束文档，不代表当前代码已经完成这些能力。

Provider adapter 的长期边界见 [`ai-gateway-provider-adapter-design.zh-CN.md`](ai-gateway-provider-adapter-design.zh-CN.md)。Anthropic Messages 与智谱 GLM 的当前实现分别见 [`ai-gateway-anthropic-messages.zh-CN.md`](ai-gateway-anthropic-messages.zh-CN.md) 和 [`ai-gateway-glm-anthropic-integration.zh-CN.md`](ai-gateway-glm-anthropic-integration.zh-CN.md)。

当前已落地链路：

```text
Codex (OpenAI Responses)
  <-> MochiPort AI Gateway
  <-> OpenAI / gpt-5.5 (Responses)
   |-> DeepSeek (Chat Completions)
   |-> Anthropic-compatible (Messages)
```

后续战略链路：

```text
Codex (OpenAI Responses)
  <-> MochiPort AI Gateway
  <-> OpenAI Responses
   |-> Anthropic Messages-compatible providers
```

## 1. 目标

AI Gateway 要解决的是：Codex 只按 OpenAI Responses 协议发请求，但我们可以在 gateway 后面选择不同 provider。

核心目标：

- Codex 侧继续配置一个 OpenAI-compatible `base_url`，例如 `http://127.0.0.1:3847/ai-gateway/v1`。
- Codex 发出的 `/responses` HTTP/SSE 请求由 gateway 接收。
- `gpt-5.5` 等 OpenAI Responses 模型走 OpenAI `/v1/responses`。
- Anthropic Messages 兼容模型优先走 `/v1/messages` 形态的出站 adapter。
- 既有 `deepseek` Chat Completions 链路保留为 legacy/fallback，不再作为新增 provider 的主要扩展方向。
- Gateway 负责 Responses 与 Anthropic Messages 之间的协议转换；Chat Completions 转换只继续维护既有能力。
- Gateway 保留并稳定传递 cache 相关信号，尤其是 `prompt_cache_key`、`Session-Id`、`thread-id`。

第一阶段不做完整多租户、计费、渠道池、复杂 fallback。AxonHub 已经覆盖这些大系统能力，本项目更适合先做一个贴合 Codex 的小内核。

## 2. 非目标

第一阶段明确不做：

- 不替代 Codex 的 thread、turn、approval、tool execution 语义。
- 不把现有 `chatgpt_base_url = ".../backend-api"` 改造成模型 API 入口。
- 不跨 provider 共享上游真实 cache。OpenAI cache 和 DeepSeek cache 是不同系统，不能互相复用。
- 不在第一阶段支持 Codex Responses WebSocket transport。
- 不直接把 AxonHub 全量代码迁入本项目。

现有 remote-control backend 继续只服务 Codex App / VS Code / CLI 的远控协议。AI Gateway 是独立模型 API 层。

## 3. 参考结论

### 3.1 Codex 请求事实

参考源码位于 `references/codex-main/codex-rs`。

当前 Codex 普通 HTTP/SSE `/responses` 请求使用 `ResponsesApiRequest`：

- `ResponsesApiRequest` 没有 `previous_response_id` 字段。
- 普通请求会发送完整 `input` history。
- 普通请求会携带 `prompt_cache_key`。
- `prompt_cache_key` 默认来自 Codex `thread_id`。
- `Session-Id` 和 `thread-id` 是 HTTP header。

`previous_response_id` 主要属于 WebSocket `response.create`：

- `ResponseCreateWsRequest` 才有 `previous_response_id`。
- 从普通 request 转 WebSocket request 时，默认 `previous_response_id = None`。
- 只有当前 input 是上一轮 input + assistant output 的严格前缀扩展时，WebSocket 才发送 `previous_response_id` 并只发送 delta input。

这意味着第一阶段 HTTP/SSE gateway 可以不依赖 `previous_response_id` 账本。Codex 已经把完整上下文放在每次请求里。

### 3.2 AxonHub 可借鉴点

参考目录：`references/axonhub-unstable`。

可借鉴的设计：

- **Inbound/Outbound Transformer 架构**（`llm/transformer/interfaces.go`）：Inbound 把客户端格式转成统一 `llm.Request`，Outbound 把统一格式转成 provider 原生格式，Pipeline 在中间连接两者。
- **Responses API 完整数据模型**（`llm/transformer/openai/responses/model.go`）：覆盖 function_call、custom_tool_call、reasoning、web_search_call、image_generation_call、compaction 等全部 item 类型。
- **Responses→Chat 转换**（`llm/transformer/openai/responses/inbound.go` 的 `convertInputToMessages`）：reasoning + 后续 function_call 合并成单个 assistant message，多个连续 function_call 合并到同一 tool_calls。
- **SSE 流状态机**（`llm/transformer/openai/responses/inbound_stream.go`）：`responsesInboundStream` 维护 outputIndex、contentIndex、sequenceNumber，按正确顺序生成完整 Responses SSE 事件序列。
- **Codex header 处理**（`llm/transformer/openai/codex/headers.go`）：从 `Session_id` 或 `X-Codex-Turn-Metadata` JSON 的 `session_id` 提取 session。
- **上游 header 合并规则**（`llm/httpclient/utils.go`）：参考 `MergeHTTPHeaders`，向上游合并安全的入站 header，例如 `User-Agent`、`Accept`、`Session_id`、`thread-id`、`X-Codex-*`；过滤 `Authorization`、API key、cookie、`Content-Type`、`Host`、`Content-Length`、`Accept-Encoding`、hop-by-hop、浏览器安全头、代理注入头和 `Cf-*` / `Cdn-*` 等边缘代理头。
- **OpenAI Responses 出站**（`llm/transformer/openai/responses/outbound.go`）：如果请求没有 `prompt_cache_key`，从 context session 补一个稳定值。
- **DeepSeek 出站**（`llm/transformer/deepseek/outbound.go`）：继承 OpenAI Chat Completions outbound，`json_schema` 降级为 `json_object`，`reasoning.effort="none"` 转为 `thinking={type:"disabled"}` 并清除 effort 字段，thinking 启用时所有 assistant message 缺少 `reasoning_content` 的补空字符串。

其它参考项目：

- **axonhub fork (dev)**（Go，[github.com/doubaoyui/axonhub](https://github.com/doubaoyui/axonhub/tree/dev)）：AxonHub 的 fork，dev 分支有成熟的 DeepSeek 严格约束处理实现，是本项目 DeepSeek 兼容处理的主要参考。
- **codex-relay**（Rust，[github.com/MetaFARS/codex-relay](https://github.com/MetaFARS/codex-relay)）：Rust 实现蓝本，SSE 事件序列化、tool call delta 积累、`previous_response_id` session store。
- **codex-bridge**（Node.js，[github.com/wujfeng712-ui/codex-bridge](https://github.com/wujfeng712-ui/codex-bridge)）：多 provider 路由策略（本项目按显式模型列表筛选 provider，同模型多 provider 使用 `session_id` 的 Rendezvous/HRW Hash 粘性选择，不采用 fallback）、reasoning effort 六级映射表、LRU session store。

不直接照搬的点：

- AxonHub 是完整渠道编排系统，包含 endpoint pool、鉴权、计费、fallback、指标等。本项目第一阶段不需要这些复杂度。
- AxonHub 的 `previous_response_id` 更偏同一上游 channel 的透传；我们的跨 provider 切换最终需要 gateway-owned id 和 ledger，而不是只透传上游 id。

### 3.3 本轮架构收敛

2026-06 本轮优化先处理低风险、高收益的运行时路径，不改变对外协议：

- **复用上游 HTTP client**：`reqwest::Client` 必须由 `AppState` 持有并在 provider 间共享，provider 只接收引用或 clone。这样保留连接池、TLS session 和 keep-alive 复用，避免每个请求重新创建 client。
- **移除热路径中的未消费 IR 解码**：`GatewayTurn` / `GatewayItem` 是后续统一 IR 与 ledger 的目标模型，但当前 production 转换仍以 `GatewayRequest` 为输入。默认请求路径不应为了日志提前 decode `GatewayTurn`；需要诊断时再按需启用。
- **暂不加入入站 bearer token 校验**：AI Gateway 当前定位为本机工具，默认绑定 `127.0.0.1`。本轮不改变本地调用鉴权行为，后续如果要支持非本机监听或多用户场景，再引入强制 token 校验。
- **归一化上游错误**：provider 非 2xx 响应不再把整段上游 body 当成纯字符串返回。Gateway 先解析 OpenAI/DeepSeek Chat 类 `{ error: ... }` 和 Anthropic `{ type: "error", error: ... }` 结构，再输出稳定的 Responses 风格错误，同时保留原 HTTP status。

后续如果进入 Phase 3 / Phase 4，应把 `GatewayTurn` 扶正为转换层和 ledger 的唯一内部语义模型，而不是继续维护 `GatewayRequest` 与 `GatewayTurn` 两套并行模型。

## 4. 配置设计

`mochiport` 自身配置新增一个独立段落：

```toml
[aiGateway]
enabled = true
routePrefix = "/ai-gateway/v1"
promptCacheRetention = "24h"

[[aiGateway.providers]]
name = "openai"
enabled = true
kind = "openai_responses"
baseUrl = "https://api.openai.com/v1"
apiKeyEnv = "OPENAI_API_KEY"
models = ["gpt-5.5", "gpt-5.4", "gpt-5.2"]

[[aiGateway.providers]]
name = "deepseek"
enabled = true
kind = "deepseek_chat"
baseUrl = "https://api.deepseek.com/v1"
apiKeyEnv = "DEEPSEEK_API_KEY"
models = ["deepseek-v4-flash", "deepseek-v4-pro"]
```

`baseUrl` 可以填厂商根地址（如 `https://api.deepseek.com`）或带版本路径的地址（如 `https://api.deepseek.com/v1`）。运行时会统一规范化，避免拼出重复的 `/v1/v1/...`。GUI 获取远端模型列表时会先尝试 `{baseUrl}/models`，如果缺少 API 版本会再尝试 `{baseUrl}/v1/models`。

Codex 侧 provider 配置示例：

```toml
model = "gpt-5.5"
model_provider = "MochiPort"
web_search = "live"

[model_providers.MochiPort]
name = "MochiPort"
base_url = "http://127.0.0.1:3847/ai-gateway/v1"
wire_api = "responses"
requires_openai_auth = true
supports_websockets = false
supports_standalone_web_search = true
```

新增渠道时 `models` 初始为空。模型列表只来自手工添加或远端模型列表同步；空列表不会按渠道名自动匹配模型。

Codex 可见模型由 `aiGateway.codexVisibleModels` 控制。它是一个显式白名单：只有配置在该列表里的模型名才会出现在 `GET /ai-gateway/v1/models` 返回值里；白名单之外的名字一律不出现。

每个模型名的目录条目按三层解析（`src/ai_gateway/catalog.rs`）：

1. **内置目录** `src/ai_gateway/models.json`（编译进二进制，`visibility = "list"`）；
2. **用户自定义条目** `aiGateway.customModels[]`（`slug` / `providerName` / `displayName` / `description` / `family` / `contextWindow` / `supportsImageInput`），字段可选，未填的部分继承协议家族模板；
3. **自动合成**：既不在内置目录、也没有自定义条目，但被某个已启用 provider 的 `models`（或 `modelAliases`）声明过的模型名，会按该 provider 的协议家族（`open_ai_responses` / `deepseek_responses` / `grok_responses` / `chat_completions` / `anthropic_messages`）合成一个条目：继承家族的 `comp_hash`、`use_responses_lite`、`apply_patch_tool_type`、`shell_type`、推理等级等 Codex 依赖字段。能力取值优先级是「自定义条目 → 上游 `/models` 的明确声明 → 家族保守兜底」：默认纯文本、不接受图片，上下文取家族兜底值；只有上游在 `input_modalities` 等字段里明确声明图片能力时才会打开图片，不做基于模型名的推测。

上游 `/models` 的模型元数据在同步时落库到 `provider.discoveredModels[]`（`id` / `displayName` / `contextWindow` / `supportsImageInput`，字段全部可选）。「获取模型」按钮和新建服务商时的自动同步都会调用它：`POST /api/v1/manage/gateway/provider/models/fetch` 返回 `models`（名字，写进 `provider.models` 供路由匹配）和 `modelDetails`（元数据）。同步只负责“识别”，是否暴露给 Codex 仍由用户在「Codex 可用模型」里勾选决定。

都没有命中的模型名会被静默跳过。GUI「AI 网关 → 概览 → Codex 可用模型」把三层来源分别标成「内置 / 自定义 / 自动」：勾选即可加入白名单，「新建自定义模型…」用来声明目录之外的名字。

因此**新增一个模型不再需要改 Rust 源码或重建 daemon**：在 GUI 里声明（或让 provider 的模型列表覆盖它）→ 加入可见白名单 → 用「启动 Codex」重启一次 Codex 让它重新拉取目录。只有想修改内置条目本身的能力口径时才需要改 `models.json` 并重建。

### 同名模型归属（`aiGateway.modelOwners`）

同一个模型名可能被多个 provider 声明（例如两家网关都提供 `glm-5.3`）。默认按 `weight` + 会话粘性选路；需要钉死时配置归属：

```toml
[aiGateway.modelOwners]
"glm-5.3" = "autoclaw"
```

- 归属命中且该 provider 仍然可用（存在、启用、声明该模型、未被健康检查拉黑、协议类型匹配）时，路由器直接锁定它，并写回会话绑定；优先级高于权重与已有粘性。
- 归属失效时**静默回落**到默认选路，不报错；`GET /api/v1/manage/codex/models/catalog` 会在该条目上返回 `owner` 与 `ownerEffective=false`，GUI 用橙色提示「归属的服务商已停用或不再声明该模型」。
- 归属只影响路由，不改变 Codex 侧看到的模型清单——Codex 依然是「一个名字一条」。若要让同一上游模型在两个服务商下各出现一次，需要改名字（用 `modelAliases` 把 `provider/模型名` 映射回上游名）。
- 目录接口同时返回 `providers`（声明该模型的已启用服务商），GUI 的「Codex 可用模型」据此按服务商分组显示、并在同名时给出归属下拉。

### 内置模型库

`src/ai_gateway/model_library.json` 是编译进二进制的模型能力表（3700+ 个模型，约 930KB），由 `scripts/generate-model-library.py` 从社区目录 [models.dev](https://models.dev/api.json) 生成：

- **生成期归并**：同一模型名会在几十家转售商里重复出现（`deepseek-v4-pro` 有 32 家），脚本按模型 id 归并——数值字段（上下文、最大输出）取**多数表决**，布尔能力取**并集**；上下文并列时偏向 2 的幂（转售商常把 1,048,576 写成 1,000,000 或 1,050,000）；
- **运行期填充**：目录解析时，若某模型既无内置条目也无自定义条目，就查这张表补上 `display_name` / `context_window` / `input_modalities`；
- **优先级**：自定义条目 → 内置模型库 → 上游 `/models` 声明 → 协议家族保守兜底。自定义条目放最前，用户显式声明的值永远生效；

### 请求日志保留

请求日志会随使用持续增长（实测数周达 37 MB / 7 万条），因此 daemon 内置自动清理：

- `aiGateway.requestLogRetentionDays`（默认 30）：删除早于该天数的记录；设 0 关闭；
- `aiGateway.requestLogMaxMb`（默认 256）：库体积超过上限时从最旧记录开始裁剪；设 0 不限制；
- daemon 启动时先跑一次，之后每小时执行；清理后会 `VACUUM` 回收空间（否则文件只增不减）；
- GUI 在「网关 → 概览 → 使用记录」里显示当前占用，并提供保留天数、体积上限与「清空请求日志」。

两个维度都为 0 时完全手动管理。

### 协议家族模板

`src/ai_gateway/family_templates.json` 存每个协议家族的 **Codex 私有协议字段**（`comp_hash`、`use_responses_lite`、`tool_mode`、`shell_type`、`apply_patch_tool_type`、`truncation_policy` 以及 `base_instructions` 提示词等），由 `scripts/generate-family-templates.py` 生成。

改版前，合成一个模型是"从内置目录里挑一条真实模型整条复制过来"——Chat Completions 家族借用 `deepseek-v4.1-flash`、Anthropic 家族借用 `GLM-5.2` 等。副作用是这些**模板条目必须留在模型目录里**，于是在用户的模型列表里显示成"没人提供的模型"（`Grok-4.6`、`DeepSeek-V4-Pro`、`DeepSeek-V4-Flash`、`GLM-5.2` 等）。

现在协议字段已独立成表：

| 层 | 提供什么 | 来源 |
| --- | --- | --- |
| 协议模板表 | `comp_hash` / `tool_mode` / 提示词等 Codex 私有字段 | `family_templates.json` |
| 内置模型库 | 显示名 / 上下文 / 图片能力 | `model_library.json` |
| 内置目录 | 官方与项目自建的**真实模型条目** | `models.json` |

因此 `models.json` 里那些只当模板用的条目已经删除，目录只保留真实在用的模型；合成逻辑也不再依赖"目录里必须存在某个模板模型"，模板缺失时会直接失败的老路径一并消失。

GUI 的「Codex 可用模型」列表还会滤掉**没有任何启用服务商提供**的内置条目（例如官方目录里有定义、但本地无上游可提供的 `gpt-5.6-luna`）——它们勾选后请求必然失败，属于纯噪音。用户显式声明的自定义条目不受此限制。

- **边界**：库里**只有展示与能力字段**，不含 Codex 私有协议字段（`use_responses_lite` / `tool_mode` / `comp_hash` / `base_instructions`），后者永远来自本地协议家族模板或官方目录。

更新方式：`python3 scripts/generate-model-library.py`（联网拉取后覆盖文件），然后重建 daemon。

> ⚠️ models.dev 是社区数据，可能与官方 API 不一致（例如把 1,048,576 取整为 1,000,000）。它的上下文值只作为**兜底**，服务商自己声明过的值仍然优先。

### 模型显示名前缀

Codex App 的模型选择器是**扁平列表**：它只消费静态目录里的 `display_name` 与 Statsig 白名单（`available_models: Set<string>`），目录 schema 里没有任何"分组/来源"字段，前端也没有分组渲染逻辑。因此"按服务商分组显示"无法真正实现，只能把归属写进显示名：

- `aiGateway.providerDisplayPrefix = true`：给每个模型名加上**服务商名**前缀（`Mac_Local · GPT-5.6-Terra`）。
- 前缀只跟随 `provider.name`，没有逐服务商覆盖字段：改服务商名，前缀自动跟着变。
- 前缀只改 `display_name`，不改 `slug`：Codex 发回的名字、路由匹配、`modelAliases` 全部不受影响，切换前缀不需要重建任何东西（但 Codex 侧要重新拉一次目录才能看到，见下节）。

Codex 侧那个「默认 / 推荐模型集」（`composer.modelPicker.default.*`）是 Codex 自己渲染的 "Default" 选项，由客户端传入的 `defaultOption` 决定，MochiPort 无法通过目录字段去掉它。

Codex 侧的目录缓存不在 MochiPort 控制内：Codex 会把 `/models` 的响应缓存在 Codex home 的 `models_cache.json`（默认 TTL 300 秒），并且前端模型选择器还会用 Statsig 的 `available_models` 白名单二次过滤；`model_catalog_json` 之类的 Codex 配置会把目录钉死在某个文件上，导致新增模型永远不出现。稳定刷新方式见「Codex 模型列表刷新机制」一节。

默认本地 provider 不写占位 bearer token 或 `env_key`。Codex 官方账号状态由
Codex 自己的 `auth.json` 维护；真实上游 provider key 只保存在 MochiPort 配置或
运行环境中，不写入 Codex 配置。旧 `ai-gateway`/`ai-codex` provider 只作为迁移
和清理输入识别。

## 5. 路由设计

第一阶段暴露：

```text
POST /ai-gateway/v1/responses
```

可选兼容路由：

```text
GET  /ai-gateway/v1/models
POST /ai-gateway/v1/chat/completions
```

第一阶段主路径只要求 Codex 能打 `/responses`。

### 5.1 Codex 模型列表刷新机制

Codex App 前端不直接读取 `mochiport` 配置，也不监听 `aiGateway.codexVisibleModels` 文件变化。Codex 侧模型列表链路是：

```text
Codex App
  -> app-server JSON-RPC model/list
  -> models_manager.list_models(OnlineIfUncached)
  -> models_cache.json 命中则直接使用缓存
  -> 缓存缺失/过期/版本不匹配时请求 GET /ai-gateway/v1/models
```

Codex `models_cache.json` 的默认 TTL 是 300 秒。保存可见模型后，Codex 端何时更新取决于它何时重新请求 `model/list`，以及本地 `models_cache.json` 是否仍然新鲜。没有额外操作时，旧模型列表可能继续保留到缓存过期。

Gateway 需要在两个位置提供模型指纹：

- `GET /ai-gateway/v1/models` 返回 `ETag`。
- `POST /ai-gateway/v1/responses` 响应返回 `x-models-etag`。

Codex core 收到 `/responses` 的 `x-models-etag` 后，会与当前内存中的模型 ETag 比较；如果不同，会强制刷新 `/models`。这只能刷新 Codex core/models-manager 的缓存，前端模型选择器是否马上重拉 `model/list` 仍由 Codex App 自身决定。稳定复现新列表的方式是退出 Codex App，删除 Codex home 下的 `models_cache.json`，再启动 Codex App。

处理流程：

```text
POST /ai-gateway/v1/responses
  -> 读取 headers 和 JSON body
  -> 提取 session/thread/cache key
  -> 解析 OpenAI Responses request
  -> 按 model 筛选 provider；同 model 多 provider 时按 session_id 做 HRW 粘性选择
  -> provider = openai_responses:
       补齐 cache 字段后透传到 /v1/responses
  -> provider = deepseek_chat:
       Responses input 转 Chat messages
       tools / tool_choice / reasoning 做兼容转换
       调用 /v1/chat/completions
       Chat SSE 转 Responses SSE
  -> provider = anthropic_messages:
       Responses input 转 Anthropic Messages
       tools / tool_choice / tool_result 按 Anthropic content blocks 映射
       调用 /v1/messages
       Anthropic Message/SSE 转 Responses JSON/SSE
```

## 6. 内部模型

Gateway 内部使用一个轻量统一结构，不直接把所有 provider 字段揉在一个 `serde_json::Value` 里。

概念结构：

```rust
GatewayRequest {
    model: String,
    instructions: Option<String>,
    input: Vec<ResponseItem>,
    tools: Vec<Value>,
    tool_choice: Option<Value>,
    reasoning: Option<Value>,
    text: Option<Value>,
    stream: bool,
    prompt_cache_key: Option<String>,
    prompt_cache_retention: Option<String>,
    previous_response_id: Option<String>,
    raw_body: Value,
}

GatewayContext {
    session_id: Option<String>,
    thread_id: Option<String>,
    request_id: String,
    prompt_cache_key: String,
}
```

第一阶段 `previous_response_id` 只保留字段，不作为 HTTP/SSE 主路径的核心状态。

## 7. Cache 策略

关键判断：

- OpenAI 的 prompt cache 是 OpenAI provider 内部能力。
- DeepSeek 的 cache 如果存在，也是 DeepSeek provider 内部能力。
- Gateway 不能让 OpenAI 直接复用 DeepSeek 上游 cache。
- Gateway 能做的是稳定重放相同前缀，让 OpenAI 在切回 OpenAI 时命中它自己见过的前缀 cache。

第一阶段策略：

1. 如果 Codex 请求 body 里有 `prompt_cache_key`，优先使用它。
2. 如果 body 没有，按顺序从 header 提取：
   - `Session_id`
   - `session-id`
   - `thread-id`
   - `X-Codex-Turn-Metadata.session_id`
3. 如果仍然没有，生成 `mochiport:<request/session fallback>`。
4. 出站 OpenAI Responses 时写入：

```json
{
  "prompt_cache_key": "<stable-key>",
  "prompt_cache_retention": "24h"
}
```

如果 Codex 第一次用 `gpt-5.5`，第二次切到 DeepSeek，第三次切回 `gpt-5.5`：

```text
1. gpt:       [1(gpt), 2(gpt), 3(gpt)]
2. deepseek:  [1(gpt), 2(gpt), 3(gpt), 4(deepseek)]
3. gpt:       [1(gpt), 2(gpt), 3(gpt), 4(deepseek), 5(gpt)]
```

第三次 OpenAI 是否命中 cache，取决于：

- `prompt_cache_key` 是否稳定；
- 前缀序列化是否足够一致；
- OpenAI 是否已经缓存过相同前缀；
- 上游模型和账号侧 cache 策略。

Gateway 应保证前两点。后两点属于 provider 行为。

## 8. Responses -> Chat 转换

DeepSeek 出站需要把 Responses `input` 转成 Chat Completions `messages`。

基本规则（参考 AxonHub `responses/inbound.go` 的 `convertInputToMessages`）：

- `instructions` 转成第一条 `system` message。
- `message` item：
  - `role=user` 转 user message。
  - `role=assistant` 转 assistant message。
  - content 里的 `input_text`、`output_text` 提取为文本。
- `function_call` 转 assistant message 的 `tool_calls`。多个连续 `function_call` 合并到同一 assistant message。
- `function_call_output` 转 tool message（`role=tool`，`tool_call_id=call_id`）。
- `reasoning` item 后面紧跟 `function_call` 时，合并为同一个 assistant message（含 `reasoning_content` + `tool_calls`）。单独的 `reasoning` item 转为 assistant message 并携带 `reasoning_content` 和 `encrypted_content`（如有）。
- `input_image` 转为 user message 的 `content[]` 里 `image_url` part。
- `custom_tool_call`、`web_search_call` 等复杂 item 第一阶段保守处理：能转文本就转文本，不能转就保留摘要文本或返回明确错误。

DeepSeek 请求形态：

```json
{
  "model": "deepseek-v4-flash",
  "messages": [
    { "role": "system", "content": "..." },
    { "role": "user", "content": "..." }
  ],
  "stream": true,
  "tools": [],
  "tool_choice": "auto"
}
```

DeepSeek 严格约束处理（参考 [axonhub fork dev 分支](https://github.com/doubaoyui/axonhub/tree/dev)）：

- **json_schema 降级**：`json_schema` response format 降级为 `json_object`（DeepSeek 不支持 json_schema）。
- **reasoning effort 映射**：`none` → thinking disabled；`low`/`medium`/`minimal` → `high`（DeepSeek 最低只支持 high）；`xhigh` → `max`。
- **thinking 参数清理**：thinking 启用时移除 `temperature`、`top_p`、`presence_penalty`、`frequency_penalty`。
- **developer 角色转换**：`role="developer"` → `role="system"`（DeepSeek 不支持 developer 角色）。
- **无效 assistant 消息过滤**：删除只有 reasoning 没有 content/tool_calls 的 assistant 消息。
- **tool_calls reasoning_content 回传**：含 tool_calls 的 assistant 消息必须有 `reasoning_content`，否则 DeepSeek 返回 400。gateway 自动从最近的含 reasoning_content 的 assistant 消息回填。
- **Codex 专属工具类型过滤**：`web_search`、`image_generation` 等 Codex 专属工具静默过滤，tools[] 只保留 `type=function`。
- Chat Completions 的 `tool_calls` delta 在返回 Responses SSE 时组装成 Responses `function_call` item。

## 9. Chat SSE -> Responses SSE

Codex 侧期望 OpenAI Responses SSE 事件。OpenAI Responses provider 可以直接透传 upstream SSE，但 DeepSeek Chat provider 需要转换。

第一阶段至少生成这些事件：

```text
response.created
response.output_item.added
response.content_part.added
response.output_text.delta
response.output_text.done
response.content_part.done
response.output_item.done
response.completed
```

文本流转换：

```text
chat.completion.chunk choices[0].delta.content
  -> response.output_text.delta delta
```

完成转换：

```text
chat finish_reason = "stop"
  -> response.output_text.done
  -> response.completed
```

工具调用流转换（已实现）：

```text
chat delta.tool_calls[*]
  -> response.output_item.added (type=function_call)
  -> response.function_call_arguments.delta
  -> response.function_call_arguments.done
  -> response.output_item.done
```

工具调用转换已在 Phase 1 中完成实现，支持并行多 tool_calls 和 reasoning→tool_calls 切换。

## 10. OpenAI Responses 出站

OpenAI provider 出站尽量少改 body：

- 保留 `model`、`instructions`、`input`、`tools`、`tool_choice`、`reasoning`、`text`、`stream`。
- 补齐或覆盖 `prompt_cache_key`，策略见第 7 节。
- 如配置了 `promptCacheRetention`，补 `prompt_cache_retention`。
- 不把 DeepSeek 的虚拟 response id 传给 OpenAI `previous_response_id`。

第一阶段 HTTP/SSE 下，Codex 通常不会发送 `previous_response_id`。如果请求体里真的出现该字段：

- provider 是 OpenAI Responses：可以透传，但需要记录日志。
- provider 是 DeepSeek Chat：忽略该字段，使用完整 input 转换。

## 11. Gateway-Owned Ledger

第一阶段不依赖 ledger，但架构上要预留。

后续 WebSocket 或跨 provider 增量请求需要 gateway 自己维护 response id：

```text
Codex sees:    gwresp_001 -> gwresp_002 -> gwresp_003
OpenAI sees:   resp_xxx   -> resp_yyy
DeepSeek sees: chatcmpl_a -> chatcmpl_b
```

内部记录：

```rust
GatewayResponseRecord {
    id: String,
    parent_id: Option<String>,
    session_id: Option<String>,
    requested_model: String,
    provider: ProviderKind,
    upstream_response_id: Option<String>,
    input_items: Vec<Value>,
    output_items: Vec<Value>,
    normalized_messages: Vec<GatewayMessage>,
}
```

这个 ledger 的作用：

- WebSocket `previous_response_id + delta input` 还原成完整 input。
- provider 切换时不把 A provider 的 response id 暴露给 B provider。
- 调试 cache 前缀是否稳定。
- 后续实现 provider-local branch state。

## 12. WebSocket 分期

AxonHub 支持 inbound/outbound 都是 WebSocket 的情况，这一点后续可以借鉴，但不要放进第一阶段主线。

WebSocket 阶段目标：

```text
Codex Responses WebSocket
  <-> Gateway WebSocket inbound
  <-> OpenAI Responses WebSocket outbound
```

以及：

```text
Codex Responses WebSocket
  <-> Gateway WebSocket inbound
  <-> DeepSeek Chat HTTP/SSE outbound
```

关键难点：

- Codex WebSocket 可能只发送 delta input。
- Gateway 必须根据 `previous_response_id` 找到完整历史。
- 如果目标 provider 是 DeepSeek Chat，必须把完整历史转成 messages。
- 如果目标 provider 是 OpenAI Responses，只有同一 OpenAI branch 才能安全透传上游 `previous_response_id`。
- 跨 provider 切换时，必须使用 gateway-owned id，不应让 Codex 直接依赖上游 id。

WebSocket 实现前置条件：

- 第一阶段 HTTP/SSE transformer 已稳定。
- GatewayResponseRecord 已落库或至少进程内稳定存储。
- Responses SSE/WS event 生成器可复用。
- 工具调用事件转换已覆盖。

## 13. 分期计划

### Phase 1：HTTP/SSE 最小可用 + 工具调用 ✅

已完成（含原 Phase 2 工具调用）：

- `[aiGateway]` 配置（`AiGatewayConfig`、`ProviderConfig`、`ProviderType`）。
- `POST /ai-gateway/v1/responses` 路由。
- OpenAI Responses 透传（补齐 cache 字段、注入 API key、透传 Codex header）。
- `prompt_cache_key` 提取和注入（6 级优先级）。
- DeepSeek Chat 非流式和流式转换。
- DeepSeek Chat SSE 转 Responses SSE（文本 + reasoning + tool_calls 完整事件链）。
- Anthropic Messages 独立 provider（非流式 text/tools、基础 streaming text/tool_use）。
- DeepSeek 严格约束处理（7 项，见 §8）。
- Responses tools 转 Chat tools（含 Codex 专属工具类型过滤）。
- Chat tool_calls SSE 转 Responses function_call events（含并行 tool_calls）。
- `function_call_output` 转 Chat tool message。
- 基础错误响应（Responses API 格式）。
- AI Gateway 相关单元测试通过。

验收：

- Codex 配置 `base_url = http://127.0.0.1:3847/ai-gateway/v1` 可发起普通对话。
- 模型为 `gpt-5.5` 时打 OpenAI Responses。
- 模型为 `deepseek-v4-flash` 或 `deepseek-v4-pro` 时打 DeepSeek Chat。
- 模型配置为 `anthropic_messages` provider 时打 Anthropic Messages。
- DeepSeek 能触发 Codex 工具调用，工具结果回填后继续回答。
- 日志能看到 session、thread、prompt_cache_key、provider route。

### Phase 2：（已合并入 Phase 1）

> 工具调用兼容已在 Phase 1 中一并完成，不再单独设阶段。

### Phase 3：Ledger 和跨 provider 状态

完成：

- Gateway-owned response id。
- GatewayResponseRecord 存储。
- provider-local branch state。
- cache prefix hash 调试日志。

验收：

- gpt -> deepseek -> gpt 的切换路径下，OpenAI 出站仍使用稳定 `prompt_cache_key`。
- 可以解释每次请求的 canonical prefix。

### Phase 4：WebSocket

完成：

- Codex Responses WebSocket inbound。
- OpenAI Responses WebSocket outbound。
- WebSocket delta input 还原。
- WebSocket fallback 到 HTTP/SSE。

验收：

- Codex 启用 `supports_websockets = true` 后可正常对话。
- WebSocket prefix request 能通过 ledger 还原完整历史。
- 跨 provider 切换不泄漏上游 response id。

## 14. 实现模块建议

建议新增：

```text
src/ai_gateway.rs                              ← mod 声明 + axum 路由挂载
src/ai_gateway/config.rs                       ← [aiGateway] 配置解析
src/ai_gateway/context.rs                      ← GatewayContext 提取（含 prompt_cache_key）
src/ai_gateway/error.rs                        ← 错误类型和 Responses 格式错误响应
src/ai_gateway/model.rs                        ← GatewayRequest, ResponseItem, StreamEvent 等类型
src/ai_gateway/router.rs                       ← provider 路由选择
src/ai_gateway/handler.rs                      ← POST /ai-gateway/v1/responses 处理入口
src/ai_gateway/providers/
    mod.rs
    openai_responses.rs                        ← OpenAI Responses 透传
    deepseek_chat.rs                           ← DeepSeek Chat 出站
src/ai_gateway/transform/
    mod.rs
    responses_to_chat.rs                       ← Responses input → Chat messages（含 DeepSeek 约束）
    chat_to_responses.rs                       ← Chat response → Responses response
    responses_stream.rs                        ← Chat SSE → Responses SSE 状态机
src/ai_gateway/ledger.rs                       ← (Phase 3, 未实现)
```

第一阶段可以少建文件，但边界应保持：

- `web.rs` 只挂路由，不写转换逻辑。
- `config.rs` 只定义配置，不写 provider 逻辑。
- provider 出站与协议转换分开。
- SSE event 生成单独封装，方便 Phase 4 复用到 WebSocket。

## 15. 风险

- DeepSeek Chat 和 OpenAI Responses 的工具调用事件不完全同构，工具流转换必须补测试。
- Responses item 类型很多，第一阶段不要承诺全部支持。
- OpenAI cache 命中不可由 gateway 强制保证，只能保证 key 和前缀稳定。
- 如果 Codex 后续把普通 HTTP/SSE 也改成 `previous_response_id` 增量，Phase 1 需要提前降级为 ledger 路径。
- WebSocket 同时涉及连接状态、增量上下文、event id 和 provider branch，必须单独设计测试。

## 16. 推荐实现顺序

1. 配置和路由骨架。
2. OpenAI Responses 透传，加 cache key 注入。
3. DeepSeek request 转换，先非流式测试转换结果。
4. DeepSeek streaming 文本转换。
5. 工具调用转换。
6. Ledger。
7. WebSocket。

这个顺序能尽快验证 Codex 到 gateway 的主链路，同时把 cache 关键路径尽早落地。
