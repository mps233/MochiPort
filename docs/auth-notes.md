# Auth Notes

This document explains why the repository contains auth-shape reference files and how that relates to the normal `codex-remote` runtime path.

It does not document a supported way to bypass official Codex or ChatGPT login.

## Scope

`codex-remote` is a remote-control backend plus a Feishu bridge. It is not an authentication layer for Codex.

Normal runtime responsibility is split like this:

- official Codex owns account state, login state, provider selection, and app-server startup
- `codex-remote` owns remote-control transport and Feishu adaptation after app-server is already running

## Supported Path

The supported workflow is:

1. Configure official Codex so it can start normally.
2. Make sure official Codex app-server can run.
3. Attach `codex-remote` through remote-control.

If official Codex cannot start because of login or account state, fix that in Codex first. The bridge only becomes relevant after the app-server can already run.

## What `codex-remote` Reads

`codex-remote` reads:

- `config.toml`
- local bridge state
- Feishu credentials and bridge-local settings

`codex-remote` does not read or manage:

- `~/.codex/auth.json`
- Codex session refresh logic
- ChatGPT access tokens
- Codex account storage internals

The daemon and shim do not load the reference auth artifacts under `references/`.

## Why `references/` Exists

The repository may contain local reference artifacts because auth file shape was inspected during local research.

Typical reasons to keep them as references:

- compare observed `auth.json` layouts
- reason about which fields were present in a local experiment
- verify that the remote-control bridge is orthogonal to Codex auth storage

They are not part of daemon startup, shim startup, Feishu onboarding, or remote-control handshake.

## Relationship To Remote-Control

The remote-control path begins after official Codex app-server is already alive.

At that point, `codex-remote` mostly cares about:

- app-server connecting to `/backend-api/wham/remote/control/server`
- remote-control `initialize` / `initialized`
- thread and turn notifications
- approval requests and responses

It does not care how Codex originally reached a runnable state, as long as that state came from an official or otherwise user-managed Codex configuration that can actually launch the app-server.

In other words:

- auth state affects whether Codex can start
- remote-control affects how a running Codex session is bridged to Feishu

Those are adjacent concerns, not the same concern.

## Unsupported Directions

This repository does not document or support:

- handcrafted or forged token material as a production path
- using `references/` as a replacement for real Codex login state
- treating local auth experiments as part of the public bridge contract

If you are publishing or operating `codex-remote`, describe the supported path as:

- official Codex first
- remote-control second
- Feishu bridge third

## Practical Guidance

If you are debugging auth-related behavior, keep the questions separate:

- "Can official Codex start?"  
  This is a Codex auth/provider/config question.

- "Can `codex-remote` receive remote-control events after Codex starts?"  
  This is a bridge/protocol/runtime question.

- "Can Feishu receive and send messages once a thread is subscribed?"  
  This is a Feishu bridge question.

Keeping those boundaries clean makes both debugging and documentation much easier.
