# ThreadRelay 改动交接规则

本文档适用于 `/Users/miaopasi/codexhub`，是构建和交接 macOS SwiftUI App、Rust daemon 时的项目级约定。

本规则的目标是：更新界面时不打断正在运行的后台服务；更新 daemon 时不在后台偷偷替换运行中的版本。任何可能影响用户会话、IM 流式消息、Codex turn 或 AI Gateway 请求的操作，都必须让用户知道实际发生了什么。

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
     macos/ThreadRelay/.build/xcode/Build/Products/Release/ThreadRelay.app \
     "$HOME/Library/Application Support/CodexHub/runtimes/<build-number>/threadrelay-daemon" \
     outputs/ThreadRelay.app
   ```

3. 只退出并重新打开 `outputs/ThreadRelay.app`。
4. 重新确认：

   - GUI 运行路径是 `outputs/ThreadRelay.app`
   - App 版本和 build number 正确
   - daemon PID、可执行文件路径和 SHA-256 与重启前一致

GUI-only 交接不得重新构建、替换、切换或重启 daemon。组装 App 时应复用原 daemon 二进制，不能把新构建的 daemon 偷渡进 GUI 更新。

## daemon 改动交接

1. 运行与改动相关的 Rust、Swift 测试和构建。
2. 可以生成构建产物，但不得对当前运行中的 daemon 执行以下操作：

   - kill、bootstrap、kickstart 或 unload
   - 自动替换或切换 runtime
   - 自动重启、回滚或故障恢复
   - 通过 GUI 关闭旧 daemon 并启动新 daemon

3. 不要因为 GUI 启动、版本不一致或构建完成而自动安装新 helper。
4. 向用户明确说明：改动已构建完成，但需要用户在确认合适的时机后手动重启后台服务。

设置页保留的“安全重启后台服务”是用户主动操作，不属于自动交接流程。该操作只能针对当前已确认身份的 daemon，并由 daemon 先检查受保护工作项。

## 版本和身份核对

交接时至少记录以下信息：

- GUI bundle 路径、版本和 build number
- daemon PID
- daemon 可执行文件绝对路径
- daemon build number
- daemon 可执行文件 SHA-256

GUI 更新前后 daemon 的 PID、路径和 SHA-256 应保持一致。若 daemon 改动需要生效，必须明确标注“等待手动重启”，不能把 GUI 已更新描述成 daemon 已更新。

## 禁止恢复的旧流程

项目不再使用以下机制：

- 自动 runtime staging 或候选 runtime
- runtime 切换 journal
- 自动回滚和重试驱动的 daemon 重启
- GUI 驱动的强制接管、强制停止或版本恢复
- 为了交接创建 ZIP 压缩包

普通 GUI 启动只复用已运行的兼容 daemon；只有服务不存在时，才允许完成首次安装和启动。GUI 关闭或崩溃不应停止 launchd 托管的 daemon。

## 交接后的报告

每次交接都应简短说明：

1. 改动属于 GUI-only 还是 daemon-affecting。
2. 执行了哪些测试和构建。
3. 是否更新并重启了正式 GUI。
4. daemon 是否被重启；若没有，说明用户需要何时手动重启。
5. 当前 App 版本、build number 和 daemon 身份是否核对通过。

本规则只约束构建和运行交接，不自动提交、推送或创建 Pull Request；这些操作需要单独得到用户请求。
