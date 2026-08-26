# Release Checklist

Use this before publishing the repository or creating a release.

For post-change GUI/daemon handoff rules, see
[`docs/threadrelay-change-handoff.zh-CN.md`](threadrelay-change-handoff.zh-CN.md).

## Repository Hygiene

- [ ] Confirm `LICENSE`, `NOTICE`, and third-party attribution files are current.
- [ ] Confirm `config.toml` is not tracked.
- [ ] Confirm `mochiport-state.json` is not tracked.
- [ ] Confirm logs are not tracked.
- [ ] Confirm build outputs are not tracked.
- [ ] Remove private screenshots, local paths, tokens, open ids, and chat ids from docs.
- [ ] Confirm release, issue, and update URLs target `https://github.com/mps233/mochiport`.

## Build

```sh
cargo fmt
cargo test
cargo build --release --bin mochiport
```

For every daemon-affecting change, build the Rust daemon and Xcode app with
independent version/build values, then assemble the formal bundle. This packaging step
is automatic in the handoff workflow, but it never changes the running GUI or
daemon:

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
  scripts/generate-swift-version.sh macos/ThreadRelay/Config/Version.xcconfig
scripts/assemble-swiftui-macos-app.sh "$DAEMON_BUILD_NUMBER" "$XCODE_APP" target/release/mochiport outputs/MochiPort.app
```

- [ ] Confirm the assembled app and embedded daemon report the expected build.
- [ ] Restart the GUI or daemon manually only when the release procedure calls for it.
- [ ] Confirm the release contains `latest-macos.json` and `latest-windows.json` with MochiPort asset URLs.
- [ ] Publish daemon metadata only from a signed macOS build; unsigned releases intentionally publish UI metadata only.

## Clean Local Artifacts

```powershell
cargo clean
Remove-Item -Recurse -Force target-verify -ErrorAction SilentlyContinue
Remove-Item *.log -ErrorAction SilentlyContinue
```

## Functional Smoke Test

- [ ] Start the MochiPort daemon with a clean config.
- [ ] Confirm `GET http://127.0.0.1:3847/api/status` returns service status.
- [ ] Complete Feishu onboarding or enter app credentials.
- [ ] Configure Codex App from the desktop GUI, or run `mochiport --config config.toml configure-codex-app`.
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
mochiport
```
