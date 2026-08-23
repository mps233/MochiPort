# MochiPort for macOS

Native SwiftUI client for the formal macOS 26 app target. The Swift package
manifest remains buildable on macOS 14 and newer for local tests. It includes the overview,
Codex integration, session movement, messaging accounts, AI Gateway providers,
request logs, and native Settings over the authenticated versioned loopback
management API.

```sh
swift build --package-path macos/ThreadRelay
swift test --package-path macos/ThreadRelay
swift run --package-path macos/ThreadRelay MochiPort
```

The app probes `GET /healthz` and reads the authenticated
`/api/v1/manage/*` endpoints with the shared management credential. In addition
to dashboard, lifecycle, and IM routes, the protected surface exposes sanitized
Codex status/actions, sessions, AI Gateway configuration, request-log summaries
and on-demand details, and non-secret Settings state. Provider keys and existing
proxy credentials are write-only; request-log details are redacted again before
they leave the management API. When no service is reachable, the stable bundle
installs a per-user LaunchAgent for its embedded Rust helper and starts it. An
already-running service is never restarted by normal app launch, and closing
the GUI leaves the LaunchAgent-managed daemon running.

The account screen currently supports Telegram, Feishu, WeChat, and WeCom
status, filtering, search, enable/disable, expansion, and deletion. A shared
four-step onboarding sheet supports Telegram bot tokens, Feishu device-code
scan or App ID/App Secret, WeChat scan with an optional verification-code
step, and WeCom scan. Credentials are write-only and never returned to the
GUI; QR expiry and validation failures stay in the current step for retry,
and a rejected enable/disable rolls the switch back. Older daemons that do
not expose the versioned account routes are shown as requiring a backend
update; the running daemon is not restarted automatically.

The Codex screen can configure, repair, uninstall, refresh models, and perform
an enhanced launch after preflight. Sessions can be searched and moved between
their original provider and AI Gateway. The Gateway screen reads and writes
provider protocol, URL, model list, priority, timeout, enablement, logging, and
visible-model settings. Request logs support server-side cursor pagination,
combined filters, search, sorting, destructive-clear confirmation, and lazy
request/upstream/SSE/response detail loading. Settings
controls the service-message language, app appearance, local connection mode,
outbound proxy, daemon diagnostics, log directory, and a manual GitHub release
check.

For a deterministic visual review that never contacts the real daemon, open
the shared `ThreadRelayPreview` scheme in Xcode and run it. The scheme sets
`MOCHIPORT_PREVIEW_FIXTURE=available`; use `bridge` or `unavailable` to
review the other service states. The fixture path is read-only and only
changes in-memory view state.

Generate the UI version settings independently from the Rust daemon with:

```sh
MOCHIPORT_UI_VERSION=0.5.3 MOCHIPORT_UI_BUILD_NUMBER=446 \
  scripts/generate-swift-version.sh \
  macos/ThreadRelay/Config/Version.xcconfig
```

For daemon-affecting changes, the default handoff builds and assembles the
formal App immediately. Use one build number for both daemon architectures and
an independently selected UI version/build; the assembly script rejects a
daemon mismatch. This updates
`outputs/MochiPort.app`, but never replaces or restarts the daemon that is
already running. The complete handoff rule is in
[`docs/threadrelay-change-handoff.zh-CN.md`](../../docs/threadrelay-change-handoff.zh-CN.md).

```sh
DAEMON_BUILD_NUMBER=439
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
MOCHIPORT_UI_VERSION=0.5.3 MOCHIPORT_UI_BUILD_NUMBER=446 \
  scripts/generate-swift-version.sh \
  macos/ThreadRelay/Config/Version.xcconfig
xcodebuild \
  -project macos/ThreadRelay/ThreadRelay.xcodeproj \
  -scheme ThreadRelay \
  -configuration Release \
  -derivedDataPath macos/ThreadRelay/.build/xcode \
  build
scripts/assemble-swiftui-macos-app.sh \
  "$DAEMON_BUILD_NUMBER" \
  macos/ThreadRelay/.build/xcode/Build/Products/Release/MochiPort.app \
  target/release/mochiport \
  outputs/MochiPort.app
```
