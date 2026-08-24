# MochiPort for Windows

Windows 客户端使用 React、TypeScript、Tauri 2 和系统 WebView2。界面进程只通过 Tauri 原生桥访问本机 MochiPort 管理 API；随安装包分发的 `mochiport-daemon.exe` 来自仓库根目录的 Rust backend，并且不启用旧 wxDragon GUI feature。

## 环境要求

- Windows 10/11 x64
- Node.js 24（最低需满足 Vite 的 Node.js 版本要求）
- Rust stable MSVC toolchain，并安装 `x86_64-pc-windows-msvc` target
- Visual Studio 2022 Build Tools：Desktop development with C++ 与 Windows SDK
- PowerShell 5.1 或更高版本

MSI 使用 WebView2 Evergreen Bootstrapper。安装时若系统尚无 WebView2 Runtime，需要联网下载当前 Evergreen Runtime；不会固定或内嵌 Chromium 版本。

## 开发运行

在仓库根目录执行：

```powershell
cd windows\MochiPort
npm ci
npm run tauri:dev
```

`tauri:dev` 会先运行 `scripts\build-sidecar.ps1`。脚本使用 `Cargo.lock` 构建根 crate 的 `mochiport` binary，并输出 Tauri 约定的：

```text
src-tauri\binaries\mochiport-daemon-x86_64-pc-windows-msvc.exe
```

`src-tauri\tauri.windows.conf.json` 只在 Windows target 下声明此 `externalBin`，因此从 macOS 做 Rust 静态检查时不会要求生成无意义的 macOS sidecar。

只查看 React fixture，不连接 daemon 时可以运行：

```powershell
npm run dev -- --host 127.0.0.1
```

然后打开 `http://127.0.0.1:1420/?fixture=1`。

## 本地安装包

```powershell
cd windows\MochiPort
npm ci
npm run tauri:build
```

Tauri 产物位于：

```text
src-tauri\target\x86_64-pc-windows-msvc\release\bundle\msi\
```

正式发布由 `.github\workflows\release-windows.yml` 完成。该 workflow 会：

1. 以无 GUI feature 的方式构建 daemon sidecar。
2. 构建 Tauri GUI；正式 tag 会先签 GUI 与 sidecar。
3. 生成并签署唯一公开安装器 MSI；不再构建或发布 NSIS，以避免两套安装技术产生重复卸载登记。
4. 生成含 GUI 与 sidecar 的 portable ZIP、Windows 更新 JSON 和 appcast，并沿用 GitHub Release 发布约定。

正式 `v<version>` tag 只有在同时配置以下两个签名 secret 时才会签名；如果 secret 缺失，workflow 会继续生成名称明确标注为 `unsigned` 的 MSI 和 ZIP，不会在构建前失败。手动分支运行同样不会使用正式签名凭据，只上传 `unsigned` 的内部 artifact。

签名 secret 名称保持为：

- `WINDOWS_CODESIGN_PFX_BASE64`
- `WINDOWS_CODESIGN_PFX_PASSWORD`

本地构建不会安装、替换或重启当前正在运行的 daemon。应用首次连接失败时会尝试启动一次随包分发的 sidecar；失败后用户也可以通过“启动本地服务”再次显式恢复。已有 daemon 正在运行时不会因版本差异自动替换、切换或重启。
