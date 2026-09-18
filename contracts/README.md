# 客户端契约

本目录存放 daemon 导出的管理 API JSON Schema。macOS 与 Windows 客户端的契约类型由它生成，避免两端各写一份。

## 生成流程

1. **daemon 侧声明**：`src/web/contracts.rs` 的 `ManageContractCatalogue` 逐个列出对外契约类型。
   新增一个对外类型只需在 catalogue 里加一个字段，不需要改生成器。
2. **导出 schema**：

   ```text
   cargo test --lib write_management_contract_schema
   ```

   写入 `manage-contracts.schema.json`。
3. **生成客户端代码**：

   ```text
   python3 scripts/generate-client-contracts.py contracts/manage-contracts.schema.json \
       --swift macos/MochiPort/Sources/MochiPortMac/Generated/ManageContracts.swift \
       --typescript windows/MochiPort/src/api/generated/manageContracts.ts
   ```

4. **CI 检查漂移**：`ci.yml` 会重新导出并对生成物运行 `--check`，生成物过期即失败。

生成物包含三部分：类型声明、Swift 侧的 `Codable` + 容错解码、TypeScript 侧的运行时校验器。
两端因此都不再需要为该接口手写解码模型或类型守卫。

## 生成器处理的口径

这些形态都来自 daemon 实际导出的 schema，不是假设：

- `Option<T>` → `["T", "null"]`，且不出现在 `required` 里，生成为可选值；
- `#[serde(default)]` → `default`，简单字面量（布尔、数字、字符串、空数组）生成为默认值；
  嵌套结构体的默认值会展开成整个对象、没有字面量形式，这类字段退化为可选值；
- 带文档注释的枚举，schemars 输出 `oneOf` + `const` 而不是 `enum`，生成器会先规范化；
- `serde_json::Value` 的 schema 是布尔 `true`，需要 `normalize` 后才能取字段，
  否则该字段会被静默丢弃；
- enum 字段的字符串默认值不能直接赋值，Swift 需要 `Enum(rawValue:)!`；
- Swift 字符串字面量要求 `\u{XXXX}`，而 Python 的 `json.dumps` 默认输出 `\uXXXX`，
  因此生成时必须 `ensure_ascii=False`，否则中文默认值编译不过。

## 接入边界：什么可以替换，什么不行

生成物提供的是**契约 DTO**与**运行时校验器**，这两类可以替代手写实现。

但 macOS 客户端 `APIClient.swift` 里的大多数手写类型**不是纯 DTO**：

- 带 `Identifiable` 的 `id` 计算属性（如 `ManageIMAccount.id`）；
- 带自定义 `init(...)`（测试与调用点会直接构造）；
- 字段类型是客户端专有类型（如 `ManageIMAccountsResponse.service` 用的是
  `ManageDashboard.Service` 枚举，而非契约里的 service 结构）。

这些属于**视图模型**，用生成类型替换会连带重写消费它们的 UI 代码。
经逐字段核对：51 个手写 Codable 类型中，与生成契约字段集合全等的只有 3 个，
其余 30 多个是部分匹配（字段被拍平、增删、或换成客户端类型）。

**因此当前策略是：新增接口一律使用生成契约**（例如用量摘要 `fetchUsageSummary` /
`usageSummary`），存量手写视图模型保持不动。把存量视图模型迁到生成契约属于
独立的重构工作，不应与"建立生成链路"混在一起做。

## 已知缺口

- 生成器目前不产出 Swift 侧的运行时校验，Swift 依赖 `Codable` 解码失败即抛错；
- TypeScript 侧的 `validators.ts` 仍保有大量手写守卫，只有已迁到生成契约的接口
  （目前是用量摘要）改用生成版本。
