# ThreadRelay for macOS

Native SwiftUI client for macOS 13 and newer. It includes the overview,
Codex integration, session movement, messaging accounts, AI Gateway providers,
request logs, and native Settings over the authenticated versioned loopback
management API.

```sh
swift build --package-path macos/ThreadRelay
swift test --package-path macos/ThreadRelay
swift run --package-path macos/ThreadRelay ThreadRelayMac
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
`THREADRELAY_PREVIEW_FIXTURE=available`; use `bridge` or `unavailable` to
review the other service states. The fixture path is read-only and only
changes in-memory view state.

Generate the shared Xcode version settings from the Rust package version with:

```sh
THREADRELAY_BUILD_NUMBER=1 scripts/generate-swift-version.sh \
  macos/ThreadRelay/Config/Version.xcconfig
```

Use the same `THREADRELAY_BUILD_NUMBER` when compiling the Rust daemon. The
assembly script rejects a bundle when its embedded daemon was built with a
different number, so the GUI and daemon cannot silently drift apart:

```sh
THREADRELAY_BUILD_NUMBER=389 cargo build --release --bin threadrelay
THREADRELAY_BUILD_NUMBER=389 scripts/generate-swift-version.sh \
  macos/ThreadRelay/Config/Version.xcconfig
```
