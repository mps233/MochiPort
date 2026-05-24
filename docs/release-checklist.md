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
- [ ] Install shim from the web console.
- [ ] Open a new terminal in a test project.
- [ ] Run `codex`.
- [ ] Confirm relay status shows TUI and upstream connected.
- [ ] Send a local Codex message and confirm Feishu receives it.
- [ ] Send a Feishu message and confirm Codex receives it.
- [ ] Trigger a command approval and confirm one Feishu approval card appears.
- [ ] Select the approval in Feishu and confirm the original card changes to `已审批`.
- [ ] Disable bridge and confirm `codex` passes through to the real Codex CLI.

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
