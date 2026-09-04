# MochiPort 改动交接规则

本文档适用于 `/Users/miaopasi/codexhub`，是构建和交接 macOS SwiftUI App、Rust daemon 时的项目级约定。

本规则的目标是：GUI 与 daemon 仍由 launchd 分别托管，但正式 App 更新后由 GUI 自动完成一次受保护的 daemon 版本切换。任何可能影响用户会话、IM 流式消息、Codex turn 或 AI Gateway 请求的操作，都必须经过 daemon 排空、管理租约和 readiness 校验。

## 先判断改动范围

### 仅 GUI 的改动

只涉及以下内容时，按 GUI-only 处理：

- SwiftUI 视图、布局、样式、动画和本地 UI 状态
- macOS 视觉资源和本地化文案
- 不改变管理 API、daemon 配置、启动参数或生命周期的 App 逻辑

### 影响 daemon 的改动

出现以下任一项时，按 daemon-affecting 处理：

- Rust 源码、Cargo 配置或 daemon 配置
- 管理 API、鉴权、协议或生命周期契约
- launchd、helper、daemon 打包和嵌入逻辑
- 同时修改 SwiftUI 与 daemon，且无法明确证明两者相互独立

无法判断时，按影响 daemon 处理。

## GUI-only 交接

1. 运行 Swift 测试和 macOS Release 构建。
2. 使用当前正在运行的 daemon 组装正式 App：

   ```sh
   scripts/assemble-swiftui-macos-app.sh <build-number> \
     macos/MochiPort/.build/xcode/Build/Products/Release/MochiPort.app \
     "$HOME/Library/Application Support/MochiPort/runtimes/<build-number>/mochiport-daemon" \
     outputs/MochiPort.app
   ```

3. 只退出并重新打开 `outputs/MochiPort.app`。
4. 重新确认：

   - GUI 运行路径是 `outputs/MochiPort.app`
   - App 版本和 build number 正确
   - daemon PID、可执行文件路径和 SHA-256 与重启前一致

GUI-only 交接不得重新构建、替换、切换或重启 daemon。组装 App 时应复用原 daemon 二进制，不能把新构建的 daemon 偷渡进 GUI 更新。

## daemon 改动交接

1. 运行与改动相关的 Rust、Swift 测试和构建。
2. daemon 改动完成后默认直接生成正式 macOS App 产物，不需要用户再次单独要求“打包”。UI 与 daemon 的版本、构建号独立指定；组装时传入 daemon 构建号：

   ```sh
   UI_VERSION=0.5.4
   UI_BUILD_NUMBER=457
   DAEMON_BUILD_NUMBER=457
   MOCHIPORT_DAEMON_BUILD_NUMBER="$DAEMON_BUILD_NUMBER" cargo build --release \
     --target aarch64-apple-darwin --bin mochiport
   MOCHIPORT_DAEMON_BUILD_NUMBER="$DAEMON_BUILD_NUMBER" cargo build --release \
     --target x86_64-apple-darwin --bin mochiport
   mkdir -p target/release
   lipo -create \
     target/aarch64-apple-darwin/release/mochiport \
     target/x86_64-apple-darwin/release/mochiport \
     -output target/release/mochiport
   chmod 755 target/release/mochiport
   MOCHIPORT_UI_VERSION="$UI_VERSION" MOCHIPORT_UI_BUILD_NUMBER="$UI_BUILD_NUMBER" \
     scripts/generate-swift-version.sh \
     macos/MochiPort/Config/Version.xcconfig
   xcodebuild \
     -project macos/MochiPort/MochiPort.xcodeproj \
     -scheme MochiPort \
     -configuration Release \
     -derivedDataPath macos/MochiPort/.build/xcode \
     build
   scripts/assemble-swiftui-macos-app.sh \
     "$DAEMON_BUILD_NUMBER" \
     macos/MochiPort/.build/xcode/Build/Products/Release/MochiPort.app \
     target/release/mochiport \
     outputs/MochiPort.app
   ```

   实际使用时分别将 `UI_BUILD_NUMBER` 和 `DAEMON_BUILD_NUMBER` 改为本次发布使用的下一个正整数；不要复用已经发布的构建号。

3. 正式 App 组装完成后，使用统一脚本或等价流程退出并重新打开
   `/Users/miaopasi/codexhub/outputs/MochiPort.app`。GUI 启动时发现内置 daemon
   构建号高于当前 runtime，且当前安装持有管理租约、受保护任务为零时，会执行以下事务：

   - 先校验当前 runtime、LaunchAgent 身份和旧 plist，并把新 helper 原子 staging；
   - 请求 daemon 执行带租约的安全排空，拒绝强杀和强制接管；
   - 原子切换 `runtimes/current`，通过 `launchctl bootout/bootstrap` 重新加载同一 LaunchAgent；
   - 等待新 instance、build 和健康状态稳定，并重新取得管理租约后才报告成功；
   - 新 daemon 未 ready、身份不符或 launchd 操作失败时，自动恢复旧 runtime、plist 和服务。

4. 受保护任务非零、管理租约失效、当前 runtime 身份无法核验或回滚条件不完整时，升级必须 fail closed：不切换、不强杀，并在 GUI 中显示可操作错误，等待下一次刷新或重新打开正式 App。

5. GUI-only 改动仍不得重启 daemon；只有 daemon-affecting 改动且新正式 App 已安装时，才允许按上一事务自动切换。用户无需再手动重启后台服务。

设置页保留的“安全重启后台服务”是用户主动操作，不属于版本升级流程。该操作只能针对当前已确认身份的 daemon，并由 daemon 先检查受保护工作项。

## GUI 启动时的受限恢复

本节只定义正常 GUI 启动的可用性恢复；不适用于 daemon 改动交接、版本升级或用户主动“安全重启”。

1. GUI 先通过 `launchctl print` 读取已登记服务的实际 job 状态，并核验已加载 LaunchAgent 的配置和其指向 runtime 的身份。
2. 服务已登记且状态为运行中时，健康检查通过则复用；健康检查未通过则显示“进程仍在运行但健康检查未通过”的诊断，绝不 `kickstart`、重启、替换或干预该 daemon。
3. 服务已登记且 `launchctl` 明确表明未运行时，只有配置和 runtime 身份均已验证，才可执行不带 `-k` 的 `launchctl kickstart` 恢复现有服务。恢复不得写入 plist、复制、staging、替换或升级 runtime。
4. 只有 `launchctl` 明确返回服务不存在，才可走随包 helper 的校验、首次 runtime 安装和 `bootstrap`。查询报错、输出未知、配置不可信或 runtime 身份不符时一律 fail closed：不写 plist、不 stage runtime、不 `bootstrap`，也不 `kickstart`。
5. 自动启动最多尝试两次；每次等待有限次数的就绪探测。自动尝试用尽后，界面应区分“服务仍在运行但健康检查未通过”和“已多次启动仍未就绪”，提供“启动本地服务”重试和“查看诊断”。手动重试仍需执行同一套身份和状态验证，不能放宽上述限制。

## 版本和身份核对

交接时至少记录以下信息：

- GUI bundle 路径、版本和 build number
- daemon PID
- daemon 可执行文件绝对路径
- daemon build number
- daemon 可执行文件 SHA-256

GUI-only 更新前后 daemon 的 PID、路径和 SHA-256 应保持一致。daemon-affecting 更新则必须记录切换前后的 instance、PID、路径、build 和 SHA-256；只有 readiness 与租约回收都通过，才能把 daemon 描述成已更新。

## 禁止恢复的旧流程

项目不再使用以下机制：

- 自动候选 runtime、无条件升级 staging 或无保护的 runtime 切换
- runtime 切换 journal 和不透明的后台恢复任务
- 没有排空、租约或 readiness 校验的 daemon 重启
- GUI 驱动的强制接管、强制停止或版本恢复
- 为了交接创建 ZIP 压缩包

普通 GUI 启动会复用已运行的兼容 daemon，或按上一节以无 `-k` 的 `kickstart` 恢复已验证停止的服务。只有 daemon-affecting 更新同时满足租约、受保护任务和身份校验时，才允许执行一次明确的 runtime 切换；GUI 关闭或崩溃不应停止 launchd 托管的 daemon。

## 交接后的报告

每次交接都应简短说明：

1. 改动属于 GUI-only 还是 daemon-affecting。
2. 执行了哪些测试和构建。
3. 是否更新并重启了正式 GUI。
4. daemon 是否按自动升级事务完成切换；若未切换，说明阻塞原因和下一步操作。
5. 当前 App 版本、build number 和 daemon 身份是否核对通过。

本规则只约束构建和运行交接，不自动提交、推送或创建 Pull Request；这些操作需要单独得到用户请求。
