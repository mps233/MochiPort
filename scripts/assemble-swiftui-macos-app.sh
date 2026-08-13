#!/bin/sh
set -eu

if [ "$#" -ne 4 ]; then
  echo "usage: $0 BUILD XCODE_APP DAEMON_BINARY OUTPUT_APP" >&2
  exit 2
fi

BUILD=$1
XCODE_APP=$2
DAEMON_BINARY=$3
OUTPUT_APP=$4

case "$BUILD" in
  ''|*[!0-9]*)
    echo "build must be a positive integer" >&2
    exit 2
    ;;
esac

if [ ! -d "$XCODE_APP" ] || [ ! -x "$XCODE_APP/Contents/MacOS/ThreadRelay" ]; then
  echo "Xcode app bundle is unavailable" >&2
  exit 1
fi
if [ ! -x "$DAEMON_BINARY" ]; then
  echo "daemon binary is unavailable" >&2
  exit 1
fi

rm -rf "$OUTPUT_APP"
cp -R "$XCODE_APP" "$OUTPUT_APP"
mkdir -p "$OUTPUT_APP/Contents/Helpers"
cp "$DAEMON_BINARY" "$OUTPUT_APP/Contents/Helpers/threadrelay-daemon"
chmod 755 "$OUTPUT_APP/Contents/Helpers/threadrelay-daemon"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $BUILD" "$OUTPUT_APP/Contents/Info.plist"

codesign --force --deep --sign - "$OUTPUT_APP"
codesign --verify --deep --strict "$OUTPUT_APP"
