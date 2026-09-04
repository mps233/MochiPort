# Release Checklist

Use this before publishing the repository or creating a release.

For post-change GUI/daemon handoff rules, see
[`docs/mochiport-change-handoff.zh-CN.md`](mochiport-change-handoff.zh-CN.md).

## Release Notes

- `RELEASE_NOTES.md` must contain only the version being published.
- Replace the file contents for every new release. Do not append, copy, or embed
  historical release notes.
- Keep older release notes on their existing GitHub Release pages and Git tags.
- Before pushing a release tag, confirm the file has exactly one version heading
  and does not contain sections labeled as historical releases.

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
independent version/build values, then assemble the formal bundle. The formal
handoff launches the assembled app; its startup performs the protected daemon
handoff automatically when the embedded build is newer:

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
  scripts/generate-swift-version.sh macos/MochiPort/Config/Version.xcconfig
scripts/assemble-swiftui-macos-app.sh "$DAEMON_BUILD_NUMBER" "$XCODE_APP" target/release/mochiport outputs/MochiPort.app
```

- [ ] Confirm the assembled app and embedded daemon report the expected build.
- [ ] Launch `/Users/miaopasi/codexhub/outputs/MochiPort.app` and verify that the GUI
      observes the newer daemon build, drains protected work, reloads the LaunchAgent,
      waits for a new ready instance, and reacquires the management lease.
- [ ] If handoff readiness fails, verify that the old runtime, plist, and daemon are
      restored and that the GUI reports the rollback instead of claiming success.
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
- [ ] Confirm `GET http://127.0.0.1:3847/healthz` returns `service=mochiport`, `apiMajor=1`, and `ready=true`.
- [ ] If legacy CLI compatibility is in scope, separately confirm `GET http://127.0.0.1:3847/api/status`; this is not the primary health check.
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
