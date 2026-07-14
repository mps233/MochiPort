# AI Gateway Provider 私有密文策略

更新时间：2026-07-14

## 当前观察方案

状态：待验证，不是最终架构结论。

当前版本优先保证 OpenAI 原生密文兼容，同时观察 Grok 在不加前缀时的连续推理、模型切换和异常恢复效果。后续可以根据真实日志重新启用 Grok scope marker；本文保留两种方案及切换条件。

CodexHub 对两类私有状态采用不同策略：

| 协议 | 返回 Codex | 后续请求 |
| --- | --- | --- |
| OpenAI Responses | 原样透传 `encrypted_content` | 原样回放 |
| Grok Responses | 原样透传 `encrypted_content` | 原样回放 |
| Anthropic Messages | 使用 typed marker 映射到 `encrypted_content` | 解包为 `thinking.signature` 或 `redacted_thinking.data` |

当前实现中，OpenAI 和 Grok 的新响应暂不增加 `codexhub:enc:v1:` 前缀。这样可以先保证 OpenAI 原生密文仍是 Codex 可直接保存、回放和迁移的原生值，用户停用或卸载 CodexHub 后不会因为 CodexHub 私有前缀而破坏会话。

跨模型或跨协议族切换依赖模型目录中的不同 `comp_hash`。Codex 检测到 `comp_hash` 变化后，会在使用新模型前通过旧模型生成本地文本摘要；新的 replacement history 不再携带旧 reasoning 密文。因此正常切换流程不需要 Gateway 给 Responses 密文增加作用域前缀。

## OpenAI 与 Grok

### 响应方向

JSON、SSE 和 Compact 响应中的下列字段保持原值：

```json
{
  "type": "reasoning",
  "encrypted_content": "opaque-provider-state"
}
```

CodexHub 可以执行与密文无关的兼容转换，例如 Grok 工具名恢复，但不得修改 `encrypted_content` 的内容。

### 请求方向

无前缀的原生密文原样发送给当前 Responses Provider。CodexHub 不猜测该密文来自 OpenAI 还是 Grok，也不维护会话级旁路索引。

若上游明确返回以下密文校验错误，Gateway 可以删除请求中的 Provider 私有密文并最多重试一次：

- `invalid_encrypted_content`
- encrypted content could not be verified
- encrypted content could not be decrypted or parsed

该重试只是异常恢复，不代替 `comp_hash` 驱动的正常切换压缩。

## Anthropic typed marker

Anthropic 继续使用 marker：

```text
codexhub:enc:v1:anthropic:<footprint>:thinking:<raw signature>
codexhub:enc:v1:anthropic:<footprint>:redacted_thinking:<raw data>
```

原因不是为了给 OpenAI/Grok 做跨 Provider 隔离，而是 Responses 的单个 `encrypted_content` 字段无法表示 Anthropic 原始 block 类型。marker 必须显式记录：

- `thinking.signature`
- `redacted_thinking.data`

`footprint` 是 Provider route 的 SHA-256 前 6 字节，route 由 Provider 名称、类型和 Base URL 组成；API Key 不参与计算。

同一 Anthropic route 回放时，CodexHub 解包 marker 并恢复原始 block。marker 不匹配时忽略该私有 block。OpenAI、Grok 与 Anthropic 之间正常切换时，`comp_hash` 应先触发文本压缩，因此 Anthropic marker 不应进入新的 Responses Provider 请求。

## 旧前缀兼容

旧版 CodexHub 曾给 OpenAI 和 Grok 的 Responses 密文写入：

```text
codexhub:enc:v1:<protocol>:<footprint>:<raw encrypted content>
```

该设计现为兼容保留方案，不再写入新响应。请求读取规则为：

1. marker 与当前 Responses route 匹配：解包并恢复原始密文。
2. marker 属于其他 route：删除密文、Provider 私有 `id` 和 `status`；item 没有可回放内容时删除整个 item。
3. 无 marker 的原生密文：始终保留，不会因为同一请求中存在旧 marker 而被误删。

这使旧会话可以逐步迁移，同时保证新会话只保存 Provider 原生密文。

## 待定方案：只隔离 Grok 密文

如果观察表明 Grok 原生密文会在以下场景造成稳定问题，可以只恢复 Grok marker，不改变 OpenAI：

```text
codexhub:enc:v1:grok:<footprint>:<raw encrypted content>
```

候选行为：

1. OpenAI 密文始终原样透传，保证 Codex 原生会话兼容。
2. Grok 响应返回 Codex 前添加 Grok marker。
3. 后续请求仍是同一 Grok route 时解包并回放原始密文。
4. 请求进入 OpenAI 或其他 route 时过滤 Grok marker、私有 item `id` 和 `status`。
5. 模型切换正常触发 `comp_hash` 压缩时，marker 通常不会进入新模型请求；过滤只作为异常边界保护。

重新启用的判断依据：

- Grok 同模型连续多轮因缺少或误处理密文而出现明显能力下降。
- Grok 与 OpenAI 切换时，`comp_hash` 压缩没有覆盖某类真实会话，导致密文校验错误。
- 上游频繁返回 `invalid_encrypted_content`，一次性清理重试成为常态而非偶发兜底。
- 日志能够确认问题来自 Grok 私有状态，而不是工具历史、Compact V2 或模型输入结构。

暂不建议恢复“OpenAI 和 Grok 全部加前缀”。若确需隔离，优先只包装 Grok，继续保持 OpenAI 原生密文不变。

## 不变量

1. OpenAI Responses 的原生 `encrypted_content` 不得添加前缀、编码或改写。
2. 当前观察期内，Grok Responses 的原生 `encrypted_content` 不添加前缀；未来只能通过明确的配置或版本决策启用 Grok marker。
3. Anthropic marker 绝不能原样发送给 Anthropic 上游。
4. 不使用模型名称猜测密文来源；跨协议切换由 `comp_hash` 负责。
5. 密文错误恢复最多重试一次，避免请求循环。
6. 所有可见模型必须声明非空 `comp_hash`；不同协议族使用不同值。

## 升级检查

Codex 或模型目录更新后至少验证：

1. 同一 OpenAI 模型连续两轮时，密文原样往返。
2. OpenAI Compact V2 返回的 blob 原样到达 Codex。
3. OpenAI/Grok 的 JSON 与 SSE 都不出现新的 `codexhub:enc:v1:`。
4. 不同 `comp_hash` 的模型切换仍会执行 pre-turn 本地压缩。
5. Anthropic `thinking` 和 `redacted_thinking` 仍可在同 route 回放。
6. 历史 OpenAI/Grok marker 仍能解包，外来历史 marker 仍会被清理。

详细字段映射和 Anthropic 转换见 [`ai-gateway-provider-private-state-conversion.zh-CN.md`](ai-gateway-provider-private-state-conversion.zh-CN.md)。
