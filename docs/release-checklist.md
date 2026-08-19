# Release Checklist

Use this before publishing the repository or creating a release.

For post-change GUI/daemon handoff rules, see
[`docs/threadrelay-change-handoff.zh-CN.md`](threadrelay-change-handoff.zh-CN.md).

## Repository Hygiene

- [ ] Confirm `LICENSE`, `NOTICE`, and third-party attribution files are current.
- [ ] Confirm `config.toml` is not tracked.
- [ ] Confirm `threadrelay-state.json` is not tracked.
- [ ] Confirm logs are not tracked.
- [ ] Confirm build outputs are not tracked.
- [ ] Remove private screenshots, local paths, tokens, open ids, and chat ids from docs.

## Build

```sh
cargo fmt
cargo test
cargo build --release --features gui --bin threadrelay
```

For the formal macOS app, build the Rust daemon and Xcode app with the same
numeric build number, then assemble the bundle without changing the running
GUI or daemon:

```sh
THREADRELAY_BUILD_NUMBER="$BUILD_NUMBER" cargo build --release --bin threadrelay
THREADRELAY_BUILD_NUMBER="$BUILD_NUMBER" scripts/generate-swift-version.sh macos/ThreadRelay/Config/Version.xcconfig
scripts/assemble-swiftui-macos-app.sh "$BUILD_NUMBER" "$XCODE_APP" target/release/threadrelay outputs/ThreadRelay.app
```

- [ ] Confirm the assembled app and embedded daemon report the expected build.
- [ ] Restart the GUI or daemon manually only when the release procedure calls for it.

## Clean Local Artifacts

```powershell
cargo clean
Remove-Item -Recurse -Force target-verify -ErrorAction SilentlyContinue
Remove-Item *.log -ErrorAction SilentlyContinue
```

## Functional Smoke Test

- [ ] Start the ThreadRelay daemon with a clean config.
- [ ] Confirm `GET http://127.0.0.1:3847/api/status` returns service status.
- [ ] Complete Feishu onboarding or enter app credentials.
- [ ] Configure Codex App from the desktop GUI, or run `threadrelay --config config.toml configure-codex-app`.
- [ ] Open Codex App by double-clicking it.
- [ ] Enable remote control in Codex App.
- [ ] Confirm remote-control status shows connected and initialized.
- [ ] Send a Feishu message and confirm Codex receives it.
- [ ] Confirm assistant/tool output for the Feishu turn appears in Feishu.
- [ ] Trigger a command approval and confirm one Feishu approval card appears.
- [ ] Select the approval in Feishu and confirm the original card changes to `已审批`.
- [ ] Disable bridge and confirm Feishu messages are no longer forwarded.

## Suggested GitHub Topics

```text
codex
codex-cli
feishu
lark
rust
websocket
json-rpc
developer-tools
threadrelay
```
