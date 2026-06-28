# CodexHub 桌面托盘与自动更新计划

更新时间：2026-06-28

本文记录 CodexHub 桌面端的系统托盘、Windows MSI 打包、Windows/macOS 自动更新方案和实施计划。目标是先落地成熟、可维护的安装与更新链路，避免自研下载、替换、提权和重启逻辑。

## 0. 当前落地状态

| 模块 | 状态 | 说明 |
| --- | --- | --- |
| 托盘 / 菜单栏状态项 | 已实现第一版 | GUI 启动创建托盘图标；关闭窗口默认隐藏；菜单“退出 CodexHub”才停止本地 backend。 |
| Windows MSI | 已接入 CI 骨架 | 新增 WiX 配置，Windows release workflow 产出 MSI、便携 zip、`latest-windows.json`、`appcast-windows.xml`。 |
| Windows 快捷方式 | 已实现 | MSI 默认创建开始菜单快捷方式和桌面快捷方式。 |
| GUI 单实例 | 已实现 | GUI 启动时使用单实例检查，已有实例运行时第二个 GUI 进程直接退出，避免多托盘和多 backend。 |
| Windows 自动更新 | 半自动 fallback 已实现 | GUI 优先读取 `latest-windows.json`，发现新版本时下载 MSI、校验 SHA256，并启动安装器；WinSparkle 仍在后续阶段接入。 |
| macOS 更新产物 | 已接入 CI 骨架 | macOS release workflow 产出带版本号的 DMG、`.app.zip`、`latest-macos.json`；GUI fallback 可下载 DMG、校验 SHA256 并打开；Sparkle framework / appcast 签名仍在后续阶段接入。 |
| 平台更新清单 | 已拆分 | Windows / macOS 不再上传同名 `latest.json`，应用内保留旧 `latest.json` 和 GitHub Release API fallback；每次 GUI 启动自动检查一次，失败静默。 |

## 1. 目标

- Windows 提供 MSI 安装包，支持开始菜单快捷方式、卸载、覆盖升级。
- Windows MSI 默认创建桌面快捷方式。
- Windows 提供系统托盘图标，关闭窗口时可隐藏到托盘，用户显式退出时才停止本地 backend。
- GUI 限制单实例运行，重复点击快捷方式不创建第二个托盘实例。
- macOS 提供菜单栏状态项，行为与 Windows 托盘保持一致。
- macOS 支持成熟自动更新。
- Windows 在没有代码签名证书的阶段先支持半自动更新，后续有证书后升级体验。
- GitHub CI 负责产出安装包、更新元数据和 Release 附件。

## 2. 非目标

- 不安装开机启动项，除非后续 GUI 增加用户显式开关。
- 不默认静默更新 Windows。未签名 MSI 会触发未知发布者提示，静默更新体验不可控。
- 不自研完整自动更新器。下载校验、替换、重启、权限处理优先交给成熟更新框架。
- 不改变 CodexHub 的核心边界：Codex App / VS Code 插件 / Codex CLI 仍由用户正常启动，CodexHub 不包装或替换 Codex。

## 3. 当前基础

仓库已有 Windows 图标和 manifest：

- `packaging/windows/codexhub.rc`
- `packaging/windows/codexhub.exe.manifest`
- `packaging/icons/AppIcon.ico`
- `build.rs` 已将 Windows 图标和 manifest 嵌入 exe

仓库已有三平台 Release workflow：

- `.github/workflows/release-windows.yml`
- `.github/workflows/release-macos.yml`
- `.github/workflows/release-linux.yml`

Windows workflow 已改为构建 zip、MSI、`latest-windows.json` 和 `appcast-windows.xml`，支持可选 Windows 代码签名 secret，但目前没有 Windows 签名证书。macOS workflow 已保留签名和公证流程，并额外生成 `.app.zip` 与 `latest-macos.json`。

`wxdragon` 已提供 `TaskBarIcon` 封装：

- Windows：系统托盘支持完整鼠标事件和菜单。
- macOS：支持 dock 或菜单栏状态项，但不支持托盘鼠标事件，应该使用菜单式交互。
- Linux：托盘能力取决于桌面环境，本文不作为第一阶段重点。

## 4. 技术决策

### 4.1 Windows 安装包

采用 WiX Toolset 生成 MSI。

理由：

- MSI 是 Windows 标准安装包形态。
- 支持卸载、升级、开始菜单快捷方式。
- 可直接接入现有 GitHub Actions Windows job。
- 后续拿到 Windows 代码签名证书后，可以签名 exe 和 msi，不需要换安装体系。

产物命名建议：

```text
CodexHub-<version>-windows-x64.msi
CodexHub-<version>-windows-x64.zip
```

### 4.2 Windows 自动更新

采用 WinSparkle + MSI。

在没有 Windows 代码签名证书前，定位为半自动更新：

1. App 检查更新。
2. 第一阶段 fallback 弹窗展示更新信息。
3. 用户确认后，CodexHub 下载 MSI 并校验 SHA256。
4. 校验通过后拉起 MSI 安装器。
5. Windows 显示 UAC 或未知发布者提示，由用户确认。

接入 WinSparkle 后：

1. WinSparkle 读取 `appcast-windows.xml`。
2. 用户确认后下载 MSI。
3. 拉起 MSI 安装器。
4. Windows 显示 UAC 或未知发布者提示，由用户确认。

不默认使用 `/qn` 静默安装。

后续有 Windows 代码签名证书后：

- 签名 `CodexHub.exe`。
- 签名 `CodexHub-*.msi`。
- 继续沿用 WinSparkle + MSI。
- 可评估是否启用更少打扰的安装参数，但仍应保留用户确认。

### 4.3 macOS 自动更新

采用 Sparkle 2。

理由：

- macOS 桌面 App 的成熟自动更新方案。
- 支持 appcast、EdDSA 签名、已签名和已公证 App。
- 适合当前已有 macOS 签名和公证的 CI 基础。

推荐产物：

```text
CodexHub-<version>-macos-universal.dmg
CodexHub-<version>-macos-universal.app.zip
appcast-macos.xml
```

DMG 继续用于手动下载和首次安装。Sparkle 自动更新使用 `.app.zip`。

### 4.4 更新元数据

不要再让 Windows 和 macOS workflow 同时写同名 `latest.json` 并上传到同一个 Release，否则后完成的 workflow 可能覆盖先完成的版本。

建议拆分为：

```text
appcast-windows.xml
appcast-macos.xml
latest-windows.json
latest-macos.json
latest-linux.json
latest.json
```

其中：

- `appcast-windows.xml` 给 WinSparkle 使用。
- `appcast-macos.xml` 给 Sparkle 使用。
- `latest-<platform>.json` 给 CodexHub 自有 fallback 更新检查使用。
- `latest.json` 只作为旧版本 fallback 或诊断入口。

如果后续要恢复统一 `latest.json`，应由单独的发布汇总 job 生成，内容包含各平台资产，不能由 Windows/macOS workflow 各自写同名文件。

示例：

```json
{
  "version": "v0.3.5",
  "releaseUrl": "https://github.com/happy-loki/codexhub/releases/tag/v0.3.5",
  "notes": "...",
  "assets": {
    "windows-x86_64": {
      "type": "msi",
      "url": "https://github.com/happy-loki/codexhub/releases/download/v0.3.5/CodexHub-v0.3.5-windows-x64.msi",
      "sha256": "..."
    },
    "macos-universal": {
      "type": "sparkle-zip",
      "url": "https://github.com/happy-loki/codexhub/releases/download/v0.3.5/CodexHub-v0.3.5-macos-universal.app.zip",
      "sha256": "...",
      "sparkleSignature": "..."
    }
  }
}
```

## 5. 托盘与菜单栏行为

### 5.1 统一产品语义

Windows 称为系统托盘，macOS 称为菜单栏状态项。用户感知上保持一致：

- 应用运行时显示状态图标。
- 点击菜单可打开主窗口。
- 关闭窗口默认隐藏，不退出 backend。
- 菜单提供明确的退出入口。
- 用户点击退出时，停止本次由 GUI 启动的 backend，并移除托盘/菜单栏图标。

### 5.2 菜单项

第一阶段已实现菜单项：

```text
打开 CodexHub
检查更新
退出 CodexHub
```

后续可增加：

```text
启动本地服务 / 停止本地服务
打开日志目录
打开 GitHub Release
开机启动
复制本地服务地址
```

### 5.3 Windows 行为

- 使用 `TaskBarIconType::Default`。
- 可以绑定左键双击打开主窗口。
- 右键显示菜单。
- 窗口 close 事件默认隐藏到托盘。
- 菜单“退出”才执行真正退出。

### 5.4 macOS 行为

- 使用 `TaskBarIconType::CustomStatusItem`。
- 不依赖左键、右键、双击事件。
- 使用 `set_popup_menu` 提供菜单式交互。
- 关闭窗口默认隐藏到菜单栏状态项。
- 菜单“退出”才执行真正退出。

## 6. CI 计划

### 6.1 Windows release workflow

`.github/workflows/release-windows.yml` 已增加：

1. 安装 WiX Toolset。
2. 构建 `codexhub.exe`。
3. 如果存在 Windows 签名 secret，则签名 exe。
4. 生成 MSI。
5. 如果存在 Windows 签名 secret，则签名 MSI。
6. 计算 MSI SHA256。
7. 生成 `latest-windows.json` 和 `appcast-windows.xml`。
8. 上传 zip、msi、manifest、appcast 到 GitHub Release。

新增文件：

```text
packaging/windows/CodexHub.wxs
```

当前先把 appcast 写在 workflow 中，后续如果模板复杂化再拆到脚本：

```text
scripts/windows/build-msi.ps1
scripts/windows/write-appcast.ps1
```

### 6.2 macOS release workflow

`.github/workflows/release-macos.yml` 已增加：

1. 继续构建 `.app`。
2. 继续签名和公证 `.app` / `.dmg`。
3. 额外生成后续 Sparkle 更新用 `.app.zip`。
4. 生成 `latest-macos.json`。
5. 上传 dmg、app.zip、manifest 到 GitHub Release。

仍待接入：

1. 引入 Sparkle 2 framework。
2. 写入 Sparkle appcast URL 和 EdDSA 公钥。
3. 使用 Sparkle 工具生成并上传 `appcast-macos.xml`。

新增或调整文件：

```text
packaging/macos/Info.plist
packaging/macos/sparkle/
```

Sparkle 私钥不能提交仓库，应放在 GitHub Secrets。公钥写入 `Info.plist` 或运行时配置。

## 7. 应用内实现计划

### Phase 1：托盘和菜单栏

- [x] 在 GUI 启动时创建跨平台 tray/status item。
- [x] 增加菜单项和事件处理。
- [x] 调整窗口 close 行为：默认隐藏到托盘/菜单栏。
- [x] 增加“真正退出”路径，复用现有 daemon 停止逻辑。
- 验证 Windows 和 macOS 的主窗口显示、隐藏、退出和 backend 生命周期。

### Phase 2：Windows MSI

- [x] 新增 WiX 配置。
- [x] Windows CI 产出 MSI。
- [x] 保留 zip 作为便携包或 fallback。
- [x] 生成 `latest-windows.json` 和 `appcast-windows.xml`。
- 验证安装、覆盖升级、卸载、开始菜单快捷方式。

### Phase 3：平台 fallback 更新清单

- [x] GUI 优先读取平台清单：`latest-windows.json` / `latest-macos.json` / `latest-linux.json`。
- [x] 旧 `latest.json` 和 GitHub Release API 作为 fallback。
- [x] GUI 每次启动自动检查一次，访问 GitHub 失败时静默忽略。
- [x] Windows manifest 中写入 MSI 下载 URL、SHA256、文件大小和签名状态。
- [x] macOS manifest 中写入 DMG / `.app.zip` 下载 URL、SHA256、文件大小、签名和公证状态。

### Phase 4：macOS Sparkle

- 引入 Sparkle 2。
- 配置 appcast URL、公钥、更新菜单。
- CI 生成 Sparkle appcast。
- 验证从旧版本更新到新版本。

### Phase 5：Windows WinSparkle

- 引入 WinSparkle DLL 或静态分发方式。
- 配置 appcast URL、应用信息。
- 菜单“检查更新”接到 WinSparkle。
- 验证未签名 MSI 的用户提示链路。
- 后续有 Windows 签名证书后补签名和体验优化。

### Phase 6：统一更新入口与 fallback

- 保留现有 `latest.json` 检查逻辑作为旧版本 fallback。
- 如果平台更新框架不可用，则打开 GitHub Release 页面。
- GUI 文案区分“检查更新”和“下载安装包”。

## 8. 验收标准

### Windows

- GitHub Release 包含 MSI。
- MSI 可全新安装。
- MSI 可覆盖升级旧版本。
- MSI 默认创建开始菜单快捷方式和桌面快捷方式。
- MSI 可卸载并清理开始菜单快捷方式和桌面快捷方式。
- 重复点击快捷方式不会创建第二个 GUI / 托盘实例。
- 未签名环境下用户能看懂安装提示。
- 托盘图标可见。
- 关闭窗口后 backend 继续运行。
- 托盘菜单“退出”后 backend 停止。
- 更新检查能发现新版本、下载 MSI、校验 SHA256 并启动安装器。
- 接入 WinSparkle 后，更新检查能读取 appcast 并拉起 MSI 安装。

### macOS

- DMG 仍可用于首次安装。
- `.app.zip` 作为 Sparkle 后续更新载体上传到 Release。
- `latest-macos.json` 可被 fallback 更新检查读取，DMG 可下载、校验 SHA256 并打开。
- 接入 Sparkle 后，Sparkle appcast 可被 App 读取，自动更新能从旧版本升级到新版本。
- 更新后的 App 仍保持签名和公证有效。
- 菜单栏状态项可见。
- 关闭窗口后 backend 继续运行。
- 菜单“退出”后 backend 停止。

## 9. 风险与处理

### Windows 未签名

风险：

- SmartScreen 可能拦截。
- MSI 显示未知发布者。
- 用户对自动更新信任感较弱。

处理：

- 第一阶段不做静默更新。
- 保留 SHA256 和 appcast 校验。
- 文案明确提示将打开安装器。
- 后续采购 Windows 代码签名证书后补齐签名。

### 多 workflow 覆盖更新元数据

风险：

- Windows 和 macOS 同时上传 `latest.json`，后完成的 workflow 覆盖先完成的 workflow。

处理：

- 平台 appcast 和 manifest 使用不同文件名。
- `latest.json` 由汇总 job 统一生成，或仅作为旧版本 fallback。

### macOS 菜单栏事件限制

风险：

- macOS 不支持类似 Windows 托盘的点击/双击事件。

处理：

- macOS 只使用菜单式交互。
- 不实现双击打开等平台不一致行为。

### backend 生命周期

风险：

- 关闭窗口、托盘退出、更新安装、系统退出之间的 daemon 停止语义混乱。

处理：

- 明确三种状态：
  - 隐藏窗口：不停止 backend。
  - 用户退出：停止 GUI 启动的 backend。
  - 更新安装：先提示用户，退出 GUI 后由安装器升级。

## 10. 推荐落地顺序

1. 托盘/菜单栏状态项。
2. Windows MSI 打包和 Windows manifest。
3. macOS `.app.zip` 和 macOS manifest。
4. macOS Sparkle 自动更新。
5. Windows WinSparkle 半自动更新。
6. 更新 manifest/appcast 汇总和文档完善。
7. 有 Windows 代码签名证书后补签 exe 和 msi。
