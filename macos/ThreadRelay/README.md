# ThreadRelay for macOS

Phase 0 SwiftUI shell for the native ThreadRelay client. It targets macOS 13
and talks to the existing Rust daemon over the versioned loopback API.

```sh
swift build --package-path macos/ThreadRelay
swift test --package-path macos/ThreadRelay
swift run --package-path macos/ThreadRelay ThreadRelayMac
```

The shell probes only `GET /healthz`. It does not start, stop, or modify the
daemon. The installed app bundle and signing workflow remain separate until
the Phase 0 Xcode packaging milestone.

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
