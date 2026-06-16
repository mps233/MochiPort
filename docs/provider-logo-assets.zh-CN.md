# Provider Logo 资源接入说明

AI Gateway 渠道列表和新增渠道弹窗需要展示上游厂商 logo。当前项目不从运行时加载第三方包，所有实际使用的 logo 都要落地到仓库内，并通过 `include_bytes!` 编译进 GUI。

## 资源来源

优先使用 `@lobehub/icons`：

- 本地参考目录：`references/lobehub-icons`
- NPM 包名：`@lobehub/icons`
- 当前使用版本：`5.8.0`
- 授权：MIT License
- 已落地授权文件：`packaging/brand/providers/LICENSE.lobehub-icons`

`references/` 目录已被 `.gitignore` 忽略，只作为取材参考。真正参与编译和提交的资源必须复制到：

```text
packaging/brand/providers/
```

当前已接入：

- OpenAI：`packaging/brand/providers/openai.svg`
- DeepSeek：`packaging/brand/providers/deepseek.svg`
- 来源记录：`packaging/brand/providers/SOURCES.md`

## 从 @lobehub/icons 提取

一般路径如下：

```text
references/lobehub-icons/es/<Provider>/components/
```

常见组件：

- `Color.js`：彩色图标，适合品牌识别。
- `Mono.js`：单色图标，适合跟随产品 UI 风格。
- `Text.js`：带文字的横向 logo，通常不适合小尺寸列表图标。

提取时只复制 SVG 需要的信息：

- `<svg ... viewBox="...">`
- `<title>ProviderName</title>`
- `<path ...>`、`<g ...>` 等图形节点
- 原始 fill / stroke 信息

不要把 React/JSX 组件代码放入项目资源目录。

## SVG 处理规则

小图标在 wxWidgets / wxDragon 下容易遇到裁切，尤其是原始 path 贴满 `viewBox` 边缘的 logo。处理原则：

1. 保留 SVG 作为源资产，不手绘 logo。
2. 如果图形贴边，在 SVG 内部加真实留白，例如：

```xml
<g transform="translate(2.4 2.4) scale(0.8)">
  ...
</g>
```

3. GUI 里不要直接把 SVG `BitmapBundle` 塞给 `StaticBitmap` 显示。当前项目使用固定尺寸 bitmap：

```rust
BitmapBundle::from_svg_data(bytes, Size::new(size, size))
    .and_then(|bundle| bundle.get_bitmap(Size::new(size, size)))
```

这样可以避免 `wxStaticBitmap` 按 SVG bundle 的 intrinsic size 在较小行高里裁切。

## 新增厂商步骤

1. 在 `references/lobehub-icons/es/<Provider>/components/` 找合适组件，优先 `Color.js` 或 `Mono.js`。
2. 提取 SVG，保存到：

```text
packaging/brand/providers/<provider>.svg
```

3. 更新来源记录：

```text
packaging/brand/providers/SOURCES.md
```

4. 在 `src/gui/widgets.rs` 增加枚举和 `include_bytes!`：

```rust
pub(super) enum ProviderLogoKind {
    OpenAi,
    DeepSeek,
    NewProvider,
}
```

并在 `provider_logo_bitmap` 中加入对应资源。

5. 在 AI Gateway 渠道选择 UI 中引用这个 logo：

```rust
Some(ProviderLogoKind::NewProvider)
```

6. 运行验证：

```powershell
cargo build --release --features gui --bin codex-remote
cargo test --bin codex-remote ai_gateway
```

## 注意事项

- 不要提交 `references/`，它只是本地参考源。
- 不要从网络运行时加载 logo，GUI 应保持离线可用。
- 新增第三方资源时必须记录来源和授权。
- 如果 logo 在小尺寸下仍被裁切，先调整 SVG 内部留白，再确认 UI 里使用的是 `provider_logo_bitmap(..., 24)` 这种固定尺寸 bitmap。
