# Release Checklist

Use this before publishing the repository or creating a release.

## Repository Hygiene

- [ ] Choose and add a license file.
- [ ] Confirm `config.toml` is not tracked.
- [ ] Confirm `codex-remote-state.json` is not tracked.
- [ ] Confirm logs are not tracked.
- [ ] Confirm build outputs are not tracked.
- [ ] Remove private screenshots, local paths, tokens, open ids, and chat ids from docs.

## Build

```powershell
cargo fmt
cargo test
cargo build --release
```

## Clean Local Artifacts

```powershell
cargo clean
Remove-Item -Recurse -Force target-verify -ErrorAction SilentlyContinue
Remove-Item *.log -ErrorAction SilentlyContinue
```

## Functional Smoke Test

- [ ] Start daemon with a clean config.
- [ ] Open `http://127.0.0.1:3847`.
- [ ] Complete Feishu onboarding or enter app credentials.
- [ ] Click `Configure Codex App` in the web console, or run `codex-remote --config config.toml configure-codex-app`.
- [ ] Open Codex App by double-clicking it.
- [ ] Enable remote control in Codex App.
- [ ] Confirm remote-control status shows connected and initialized.
- [ ] Send a Feishu message and confirm Codex receives it.
- [ ] Confirm assistant/tool output for the Feishu turn appears in Feishu.
- [ ] Trigger a command approval and confirm one Feishu approval card appears.
- [ ] Select the approval in Feishu and confirm the original card changes to `已审批`.
- [ ] Disable bridge and confirm Feishu messages are no longer forwarded.
- [ ] Optional: install shim from the web console and confirm `codex` CLI helper mode still starts a remote-control session.

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
```
