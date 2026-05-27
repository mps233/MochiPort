#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_NAME="Codex Remote"
DIST_DIR="$ROOT/target/dist"
APP="$DIST_DIR/$APP_NAME.app"
CONTENTS="$APP/Contents"
MACOS="$CONTENTS/MacOS"
RESOURCES="$CONTENTS/Resources"

cd "$ROOT"
cargo build --release
PATH="/opt/homebrew/bin:$PATH" cargo build --release --features gui --bin codex-remote-gui

rm -rf "$APP"
mkdir -p "$MACOS" "$RESOURCES"

cp "$ROOT/packaging/macos/Info.plist" "$CONTENTS/Info.plist"
cp "$ROOT/target/release/codex-remote-gui" "$MACOS/$APP_NAME"
cp "$ROOT/target/release/codex-remote" "$RESOURCES/codex-remote"
chmod +x "$MACOS/$APP_NAME" "$RESOURCES/codex-remote"

echo "$APP"
