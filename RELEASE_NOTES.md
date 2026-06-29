# CodexHub v0.3.10

## 改进内容

- 新增 Intel 架构的 macOS 原生构建：release CI 通过 job 矩阵在 `macos-15` (Apple Silicon) 与 `macos-13` (Intel) 上分别原生编译，两者各自签名、公证并发布独立的 dmg、app.zip 和自动更新清单（`latest-macos.json` 与 `latest-macos-intel.json`）。
- macOS 安装包按架构显式命名，便于区分：Apple Silicon 包为 `…-macos-apple-silicon.dmg`，Intel 包为 `…-macos-intel.dmg`。
- 自动更新按 CPU 架构区分清单与下载资产，避免 Intel 设备误更新成 Apple Silicon 包。

## 下载说明

- Apple Silicon (M 系列) 选 `CodexHub-vX.Y.Z-macos-apple-silicon.dmg`。
- Intel Mac 选 `CodexHub-vX.Y.Z-macos-intel.dmg`。

## 已知问题

- 在 CodexHub 模式下，Codex App 插件页点击 `computer-use` 进入详情页可能显示「未找到插件」。这是 Codex App 前端对 bundled 本地插件详情的展示行为，`computer-use` 功能本身可正常使用，不影响实际调用。
- macOS Intel 构建依赖 GitHub `macos-13` runner，高峰期该 runner 排队较久，Intel 产物可能晚于 universal 包出现在 release 页。

## 验证

- `cargo fmt`
- `cargo build --release --features gui --bin codexhub`

---

有问题可以提 GitHub issue，也可以关注 README 里的公众号后直接发消息给我。
