# Brand Assets

Brand SVGs are copied from `@lobehub/icons` version `5.8.0` when the package contains a matching icon. The package is distributed under the MIT license. Runtime copies live under `macos/MochiPort/Sources/MochiPortMac/Resources/`.

- Codex: `references/lobehub-icons/es/Codex/components/Color.js` -> `Resources/ClientLogos/codex.svg`
- OpenAI: `references/lobehub-icons/es/OpenAI/components/Mono.js` -> `Resources/ProviderLogos/openai.svg`
- Grok: `references/lobehub-icons/es/Grok/components/Mono.js` -> `Resources/ProviderLogos/grok.svg`
- DeepSeek: `references/lobehub-icons/es/DeepSeek/components/Color.js` -> `Resources/ProviderLogos/deepseek.svg`
- Anthropic: `references/lobehub-icons/es/Anthropic/components/Mono.js` -> `Resources/ProviderLogos/anthropic.svg`
- Zhipu: `references/lobehub-icons/es/Zhipu/components/Color.js` -> `Resources/ProviderLogos/zhipu.svg`

The SwiftUI client bundles monochrome copies of these provider marks under
`macos/MochiPort/Sources/MochiPortMac/Resources/ProviderLogos/`.

Lucide UI icons are copied from Lucide Icons and covered by `packaging/brand/LICENSE.lucide-icons`.

- Codex CLI terminal: custom terminal treatment -> `Resources/ClientLogos/codex-cli.svg`
- VS Code: custom SVG using official VS Code brand colors -> `Resources/ClientLogos/vscode.svg`
- App icon sources: `packaging/macos/AppIcon.svg` (light) and
  `packaging/macos/AppIcon-dark.svg` (dark). The assembled macOS application
  uses `AppIcon.icns` as its default light icon and carries the matching
  `AppIcon-dark.icns` variant in its resources.

The `references/` directory is intentionally ignored by git, so runtime SVGs are tracked directly in the SwiftUI resource directories. Windows uses `packaging/icons/AppIcon.ico`; macOS release packaging uses the tracked ICNS files above.

## High-DPI Rendering

All logo SVGs are designed to render crisply at any scale, including Retina and high-DPI displays. The SwiftUI client loads them as bundle resources through `NSImage`.
