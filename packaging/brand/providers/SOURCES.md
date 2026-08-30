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
- App icon authoring sources: `packaging/brand/mochiport-liquid-glass-icon.json`,
  `packaging/brand/mochiport-liquid-glass-background.svg`, and
  `packaging/brand/mochiport-liquid-glass-face.svg`. The production Icon
  Composer document is `packaging/macos/AppIcon.icon`; Xcode compiles it with
  `actool` into `Assets.car` and a standalone `AppIcon.icns`, and the assembler
  preserves those compiled resources. The SVGs contain no authored border
  stroke; the background layer keeps glass disabled, while Icon Composer adds
  restrained glass, refraction, specular, and translucency to the face and
  macOS renders the outer chiclet edge.

`packaging/macos/AppIcon.svg` and `packaging/macos/AppIcon-dark.svg` remain the
static README artwork. Their matching tracked ICNS exports are not copied into
the formal macOS application.

The `references/` directory is intentionally ignored by git, so runtime SVGs are tracked directly in the SwiftUI resource directories. Windows uses `packaging/icons/AppIcon.ico`; the macOS release build compiles the tracked Icon Composer package above.

## High-DPI Rendering

All logo SVGs are designed to render crisply at any scale, including Retina and high-DPI displays. The SwiftUI client loads them as bundle resources through `NSImage`.
