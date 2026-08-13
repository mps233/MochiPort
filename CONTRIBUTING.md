# Contributing to ThreadRelay

Thanks for considering contributing to ThreadRelay.

ThreadRelay sits between Codex remote-control clients, IM channels, and the local AI Gateway, so small protocol changes can affect message routing, approvals, and model requests. Please keep changes conservative and easy to review.

## Development Setup

```powershell
cargo fmt
cargo test
cargo build
```

For approval-related changes:

```powershell
cargo test approval
```

## Design Rules

- Keep Codex as the source of truth.
- Do not change Codex cwd, model, sandbox, approval policy, or environment from the bridge.
- Prefer forwarding official app-server protocol payloads instead of inventing bridge-specific semantics.
- Keep Codex App launch clean: configuration should point it at the local backend, but `threadrelay` should not wrap or launch Codex.
- IM interfaces should be compact and stateful; avoid repeated explanatory messages when an existing message or card can show the result.

## Pull Request Checklist

- Explain the user-facing behavior change.
- Include verification steps.
- Add or update docs for command/config/protocol changes.
- Avoid committing local config, credentials, state files, logs, or build outputs.

## Security

Do not include real IM credentials, API keys, user ids, private chat ids, screenshots with sensitive content, or local project paths in issues or pull requests.
