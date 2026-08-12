#!/bin/sh
set -eu

if [ "$#" -ne 2 ]; then
  echo "usage: $0 BUILD OUTPUT_APP" >&2
  exit 2
fi

BUILD="$1"
APP="$2"
CONTENTS="$APP/Contents"
MACOS="$CONTENTS/MacOS"
RESOURCES="$CONTENTS/Resources"
VERSION=$(sed -n 's/^version *= *"\([^"]*\)".*/\1/p' Cargo.toml | head -n 1)

case "$BUILD" in
  ''|*[!0-9]*)
    echo "build must be a positive integer" >&2
    exit 2
    ;;
esac

if [ -z "$VERSION" ] || [ ! -x target/release/codexhub ]; then
  echo "release binary or Cargo version is unavailable" >&2
  exit 1
fi

rm -rf "$APP"
mkdir -p "$MACOS" "$RESOURCES/brand"
cp packaging/macos/Info.plist "$CONTENTS/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString ${VERSION%%[-+]*}" "$CONTENTS/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $BUILD" "$CONTENTS/Info.plist"
cp packaging/macos/AppIcon.icns "$RESOURCES/AppIcon.icns"
cp packaging/brand/*.png "$RESOURCES/brand/"
cp target/release/codexhub "$MACOS/CodexHub"
chmod +x "$MACOS/CodexHub"
codesign --force --deep --sign - "$APP"
codesign --verify --deep --strict "$APP"
