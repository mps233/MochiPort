# AI Gateway 智谱 GLM Anthropic Messages 对接说明

更新时间：2026-06-20

状态：已落地首版，用于指导后续 Anthropic-compatible 厂商接入。

本文记录 `codex-remote` AI Gateway 如何通过 Anthropic Messages 协议接入智谱 GLM。它也是后续新增 Kimi、DeepSeek Anthropic Messages 等显式厂商 profile 时的参考模板。

相关文档：

- [`ai-gateway-anthropic-first-roadmap.zh-CN.md`](ai-gateway-anthropic-first-roadmap.zh-CN.md)：Anthropic Messages 优先路线。
- [`ai-gateway-anthropic-messages.zh-CN.md`](ai-gateway-anthropic-messages.zh-CN.md)：Anthropic Messages adapter 设计。
- [`provider-logo-assets.zh-CN.md`](provider-logo-assets.zh-CN.md)：provider logo 资源维护方式。

官方参考：

- 智谱 Claude API 兼容说明：<https://docs.bigmodel.cn/cn/guide/develop/claude/introduction>
- 智谱 Coding Plan 模型配置说明：<https://docs.bigmodel.cn/cn/guide/develop/codingplan/model>

## 1. 接入结论

智谱 GLM 当前按 Anthropic Messages 兼容协议接入，不新增独立 `ProviderType`，也不走 Chat Completions。

配置形态：

```toml
[[aiGateway.providers]]
name = "glm"
enabled = true
providerType = "anthropic_messages"
compatibility = "glm_anthropic"
baseUrl = "https://open.bigmodel.cn/api/anthropic"
modelsUrl = "https://open.bigmodel.cn/api/paas/v4/models"
apiKey = "..."
models = ["glm-4.6"]
modelAliases = { "glm-5.2" = "GLM-5.2" }
```

关键约束：

- `providerType` 表示协议族，智谱使用 `anthropic_messages`。
- `compatibility` 表示显式厂商 profile，智谱使用 `glm_anthropic`。
- `baseUrl` 表示推理请求入口；`modelsUrl` 表示模型列表入口。智谱两者不是同一个路径。
- `modelAliases` 表示 Codex 侧模型名到上游模型名的映射。路由按 key 匹配，出站请求用 value。
- GUI 新建智谱渠道时只生成 `glm_anthropic`，不生成通用未知兼容 profile。
- `zhipu_anthropic` 只作为别名解析保留，方便将来配置迁移；项目主推名称是 `glm_anthropic`。

## 2. 智谱协议入口

智谱官方 Claude API 兼容说明中，Anthropic 兼容 base URL 为：

```text
https://open.bigmodel.cn/api/anthropic
```

Gateway 运行时会用统一的 `provider_api_root()` 处理 base URL，然后拼接 Anthropic Messages 路径：

```text
{baseUrl}/v1/messages
```

因此智谱最终请求地址为：

```text
https://open.bigmodel.cn/api/anthropic/v1/messages
```

模型列表不从 Anthropic 兼容 base URL 推导。智谱模型列表使用 OpenAI-compatible 模型列表入口：

```text
GET https://open.bigmodel.cn/api/paas/v4/models
Authorization: Bearer <apiKey>
```

GUI 的智谱 GLM 模板会自动填充该 `modelsUrl`，用户通常不需要手工填写或推断模型列表地址。

当前 GLM profile 与 Anthropic 官方 profile 共用同一套传输形态：

| 项 | 当前取值 |
| --- | --- |
| 协议族 | Anthropic Messages |
| endpoint style | `/v1/messages` |
| auth header | `x-api-key: <apiKey>` |
| version header | `anthropic-version: <ANTHROPIC_VERSION>` |
| stream shape | Anthropic SSE |
| usage shape | Anthropic usage |

如果后续实测发现智谱对某些字段宽容或忽略，不需要在 Gateway 主动补兼容分支。只有出现“必须不一样才能跑通”的差异，才进入 `GlmAnthropic` profile。

## 3. 代码落点

### 3.1 配置字段

`ProviderConfig` 增加 `compatibility` 与 `models_url`：

```rust
pub struct ProviderConfig {
    pub provider_type: ProviderType,
    pub compatibility: Option<String>,
    pub base_url: String,
    pub models_url: Option<String>,
    pub api_key: String,
    pub models: Vec<String>,
    pub model_aliases: BTreeMap<String, String>,
}
```

约定：

- 未配置 `compatibility` 时，`anthropic_messages` 默认按官方 Anthropic 处理。
- 已配置时必须命中白名单 profile。
- 未知 profile 返回明确 bad request，不降级成“通用兼容”。
- `models_url` 是模型列表接口，独立于推理 `base_url`。为空时 GUI 会按服务商 profile 或 `base_url` 推导默认 `/models` 地址。
- `model_aliases` 解决三方渠道模型名大小写或命名细节差异。例如 Codex App 暴露 `glm-5.2`，上游只接受 `GLM-5.2` 或 `xx-GLM-5-2`。
- 未配置 alias 时，路由仍会先做精确匹配，再做大小写不敏感匹配；大小写不同但实际同名的模型不需要用户额外配置。

### 3.2 Anthropic profile

智谱 profile 位于：

```text
src/ai_gateway/providers/anthropic_messages/options.rs
```

当前白名单：

```rust
pub(super) enum AnthropicProviderProfile {
    Anthropic,
    GlmAnthropic,
}
```

解析规则：

```rust
match compatibility {
    None | Some("anthropic") | Some("claude") => Anthropic,
    Some("glm_anthropic") | Some("zhipu_anthropic") => GlmAnthropic,
    Some(other) => unsupported profile error,
}
```

`GlmAnthropic` 当前复用 Anthropic 基础 options。这个设计故意保持保守：先跑通官方兼容路径，等实测差异明确后再在 profile 内收敛差异。

### 3.3 请求发送

Anthropic Messages provider 负责：

- 根据 `compatibility` 构造 `AnthropicProviderOptions`。
- 用 `options.messages_url(provider)` 生成上游 URL。
- 根据 `options.auth` 注入鉴权 header。
- 根据 `options.version_header` 注入版本 header。
- 复用 `AppState` 中共享的 `reqwest::Client`。
- 用统一 `ensure_success_response()` 归一化上游错误。

这保证新增兼容厂商时，大多数差异只改 `options.rs`，而不是修改 request / response / stream 三套转换逻辑。

## 4. GUI 落点

智谱作为显式渠道出现在“新增 / 编辑渠道”弹窗的服务商列表中。

GUI 默认模板：

```rust
ProviderConfig {
    name: "glm".to_string(),
    provider_type: ProviderType::AnthropicMessages,
    compatibility: Some("glm_anthropic".to_string()),
    base_url: "https://open.bigmodel.cn/api/anthropic".to_string(),
    models_url: Some("https://open.bigmodel.cn/api/paas/v4/models".to_string()),
    ..Default::default()
}
```

保存逻辑：

- 选中“智谱 GLM”时，保存 `providerType = "anthropic_messages"`。
- 同时保存 `compatibility = "glm_anthropic"`。
- 同时保存 `modelsUrl = "https://open.bigmodel.cn/api/paas/v4/models"`。
- 编辑已有 `glm_anthropic` / `zhipu_anthropic` 渠道时，服务商选项显示为“智谱 GLM”。
- 选中普通 Anthropic 时，保存 `compatibility = "anthropic"`。
- 模型列表获取优先使用显式 `modelsUrl`；为空才按 `baseUrl` 推导候选地址。

渠道列表 logo：

- `openai_responses` 显示 OpenAI。
- `chat_completions` 显示 DeepSeek。
- `anthropic_messages + glm_anthropic/zhipu_anthropic` 显示智谱。
- 其它 `anthropic_messages` 显示 Anthropic。

## 5. Logo 资源

智谱 logo 文件：

```text
packaging/brand/providers/zhipu.svg
```

来源记录：

```text
packaging/brand/providers/SOURCES.md
```

GUI 通过 `ProviderLogoKind::Zhipu` 编译期嵌入该 SVG。后续新增厂商 logo 时，按同样方式：

1. 把 SVG 放入 `packaging/brand/providers/`。
2. 在 `SOURCES.md` 记录来源。
3. 在 `ProviderLogoKind` 增加枚举值。
4. 在 `provider_logo_bitmap()` 增加 `include_bytes!()`。
5. 在 provider row 的 logo 选择逻辑中按 profile 映射。

## 6. 验证清单

新增或修改 GLM profile 后至少跑：

```powershell
cargo fmt
cargo test ai_gateway
cargo check
cargo check --features gui
git diff --check
```

目前已有测试覆盖：

- `ProviderConfig` 可反序列化 `compatibility = "glm_anthropic"`。
- `glm_anthropic` 映射到 `GlmAnthropic`。
- `zhipu_anthropic` 别名映射到 `GlmAnthropic`。
- 未知 Anthropic compatibility profile 返回明确错误。
- GLM profile URL 拼接为 `https://open.bigmodel.cn/api/anthropic/v1/messages`。

建议后续补充的实测用例：

- 非流式文本请求。
- SSE 文本流。
- tool_use / tool_result 多轮。
- prompt cache control 是否被智谱接受或忽略。
- thinking / reasoning 字段是否接受 Anthropic 原生形态。
- web_search_server 等 Anthropic server tool 是否支持。

## 7. 后续新增厂商模板

后续新增 Anthropic-compatible 厂商时，不要复制一套 provider。按以下步骤做：

1. 确认官方文档：base URL、路径、鉴权 header、版本 header、stream 格式、tool 格式。
2. 在 `AnthropicProviderProfile` 增加显式 profile，例如 `KimiAnthropic`。
3. 在 `from_compatibility()` 增加白名单字符串，例如 `kimi_anthropic`。
4. 在 `AnthropicProviderOptions` 增加厂商 options；如果与 Anthropic 完全一致，可以先复用 `base(profile)`。
5. 如果 GUI 要支持一键新增，增加默认 provider 模板和服务商单选项。
6. 如果有独立品牌展示，加入 logo 并按 profile 映射。
7. 增加 options 单元测试和配置反序列化测试。
8. 用真实 API 做最小 smoke test，再决定是否打开更多能力。

差异处理原则：

- 厂商会自行忽略的字段，不必在 Gateway 主动删除。
- 只有会导致请求失败、响应解析失败或 Codex 行为错误的差异，才进入 profile。
- profile 是白名单，不接受用户随便填写未知兼容厂商。
- 新厂商稳定后，优先只保留 Anthropic Messages 接法，不再新增 Chat Completions 接法。
