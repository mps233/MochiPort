# ThreadRelay for macOS

Phase 3 SwiftUI client for macOS 13 and newer. It includes the overview, daemon
lifecycle diagnostics, and messaging-account management over the versioned
loopback management API.

```sh
swift build --package-path macos/ThreadRelay
swift test --package-path macos/ThreadRelay
swift run --package-path macos/ThreadRelay ThreadRelayMac
```

The app probes `GET /healthz` and reads the authenticated
`GET /api/v1/manage/dashboard`, `GET /api/v1/manage/lifecycle`, and
`GET /api/v1/manage/im/accounts` endpoints with the shared management
credential. Account enable/disable, delete, and Telegram token onboarding use
authenticated POST endpoints under `/api/v1/manage/im/*`; Feishu credential
and scan onboarding, WeChat scan/verification-code onboarding, and WeCom scan
onboarding use the same protected namespace. When no service is reachable, the
stable bundle installs a per-user LaunchAgent for its embedded Rust helper and
starts it. An already-running service is never restarted by normal app launch,
and closing the GUI leaves the LaunchAgent-managed daemon running.

The account screen currently supports Telegram, Feishu, WeChat, and WeCom
status, filtering, search, enable/disable, expansion, and deletion. A shared
four-step onboarding sheet supports Telegram bot tokens, Feishu device-code
scan or App ID/App Secret, WeChat scan with an optional verification-code
step, and WeCom scan. Credentials are write-only and never returned to the
GUI; QR expiry and validation failures stay in the current step for retry,
and a rejected enable/disable rolls the switch back. Older daemons that do
not expose the versioned account routes are shown as requiring a backend
update; the running daemon is not restarted automatically.

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
