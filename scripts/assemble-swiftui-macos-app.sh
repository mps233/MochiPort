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
GUI_SUPERVISOR=packaging/macos/threadrelay-gui-supervisor
APP_ICON=packaging/macos/AppIcon.icns
THIRD_PARTY_LICENSE_GENERATOR=packaging/generate-third-party-licenses.py
LUCIDE_LICENSE=packaging/brand/LICENSE.lucide-icons
PROVIDER_LICENSE=packaging/brand/providers/LICENSE.lobehub-icons
PROVIDER_SOURCES=packaging/brand/providers/SOURCES.md

case "$BUILD" in
  ''|0|*[!0-9]*)
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
if [ ! -f "$GUI_SUPERVISOR" ]; then
  echo "GUI supervisor script is unavailable" >&2
  exit 1
fi
for RESOURCE in "$APP_ICON" "$THIRD_PARTY_LICENSE_GENERATOR" LICENSE NOTICE \
  "$LUCIDE_LICENSE" "$PROVIDER_LICENSE" "$PROVIDER_SOURCES"; do
  if [ ! -f "$RESOURCE" ]; then
    echo "required bundle resource is unavailable: $RESOURCE" >&2
    exit 1
  fi
done

case "$OUTPUT_APP" in
  *.app) ;;
  *)
    echo "output must be an .app bundle" >&2
    exit 2
    ;;
esac

OUTPUT_PARENT=$(dirname "$OUTPUT_APP")
OUTPUT_NAME=$(basename "$OUTPUT_APP")
if [ ! -d "$OUTPUT_PARENT" ]; then
  echo "output parent directory is unavailable" >&2
  exit 1
fi
OUTPUT_PARENT=$(cd "$OUTPUT_PARENT" && pwd -P)
OUTPUT_APP="$OUTPUT_PARENT/$OUTPUT_NAME"
XCODE_APP=$(cd "$XCODE_APP" && pwd -P)

if [ "$OUTPUT_APP" = "$XCODE_APP" ] || { [ -e "$OUTPUT_APP" ] && [ "$OUTPUT_APP" -ef "$XCODE_APP" ]; }; then
  echo "output app must differ from the Xcode build product" >&2
  exit 2
fi
if [ -e "$OUTPUT_APP" ] && [ ! -d "$OUTPUT_APP" ]; then
  echo "existing output is not an app bundle directory" >&2
  exit 1
fi
if [ -L "$OUTPUT_APP" ]; then
  echo "existing output app must not be a symbolic link" >&2
  exit 1
fi

case "$OUTPUT_APP/" in
  "$XCODE_APP/"*)
    echo "output app must not be inside the Xcode build product" >&2
    exit 2
    ;;
esac
case "$XCODE_APP/" in
  "$OUTPUT_APP/"*)
    echo "Xcode build product must not be inside the output app" >&2
    exit 2
    ;;
esac

DAEMON_VERSION=$("$DAEMON_BINARY" --version 2>/dev/null || true)
case "$DAEMON_VERSION" in
  threadrelay\ *) ;;
  *)
    echo "daemon binary does not identify as ThreadRelay" >&2
    exit 1
    ;;
esac
DAEMON_BUILD=$(printf '%s\n' "$DAEMON_VERSION" | sed -n 's/.*(build \([^)]*\)).*/\1/p')
if [ -z "$DAEMON_BUILD" ] || [ "$DAEMON_BUILD" != "$BUILD" ]; then
  echo "daemon build mismatch: expected $BUILD, got ${DAEMON_BUILD:-unknown}" >&2
  exit 1
fi

GUI_ARCHS=$(lipo -archs "$XCODE_APP/Contents/MacOS/ThreadRelay")
DAEMON_ARCHS=$(lipo -archs "$DAEMON_BINARY")
for ARCH in $GUI_ARCHS; do
  case " $DAEMON_ARCHS " in
    *" $ARCH "*) ;;
    *)
      echo "daemon binary is missing GUI architecture $ARCH" >&2
      exit 1
      ;;
  esac
done
for ARCH in $DAEMON_ARCHS; do
  case " $GUI_ARCHS " in
    *" $ARCH "*) ;;
    *)
      echo "GUI is missing daemon architecture $ARCH" >&2
      exit 1
      ;;
  esac
done

STAGING_ROOT=$(mktemp -d "$OUTPUT_PARENT/.threadrelay-assemble.XXXXXX")
STAGED_APP="$STAGING_ROOT/$OUTPUT_NAME"
BACKUP_APP="$STAGING_ROOT/previous.app"
cleanup() {
  status=$?
  trap - EXIT HUP INT TERM
  if [ -e "$BACKUP_APP" ] && [ ! -e "$OUTPUT_APP" ]; then
    if ! mv "$BACKUP_APP" "$OUTPUT_APP"; then
      echo "failed to restore previous app; backup kept at $BACKUP_APP" >&2
      exit 1
    fi
  fi
  rm -rf "$STAGING_ROOT"
  exit "$status"
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM

/usr/bin/ditto "$XCODE_APP" "$STAGED_APP"
mkdir -p "$STAGED_APP/Contents/Helpers"
mkdir -p "$STAGED_APP/Contents/Resources/brand/providers"
cp "$DAEMON_BINARY" "$STAGED_APP/Contents/Helpers/threadrelay-daemon"
chmod 755 "$STAGED_APP/Contents/Helpers/threadrelay-daemon"
cp "$GUI_SUPERVISOR" "$STAGED_APP/Contents/Helpers/threadrelay-gui-supervisor"
chmod 755 "$STAGED_APP/Contents/Helpers/threadrelay-gui-supervisor"
cp "$APP_ICON" "$STAGED_APP/Contents/Resources/AppIcon.icns"
python3 "$THIRD_PARTY_LICENSE_GENERATOR" \
  "$STAGED_APP/Contents/Resources/THIRD_PARTY_LICENSES.txt"
cp LICENSE NOTICE "$STAGED_APP/Contents/Resources/"
cp "$LUCIDE_LICENSE" "$STAGED_APP/Contents/Resources/brand/"
cp "$PROVIDER_LICENSE" "$PROVIDER_SOURCES" \
  "$STAGED_APP/Contents/Resources/brand/providers/"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $BUILD" "$STAGED_APP/Contents/Info.plist"
if /usr/libexec/PlistBuddy -c "Print :CFBundleIconFile" \
  "$STAGED_APP/Contents/Info.plist" >/dev/null 2>&1; then
  /usr/libexec/PlistBuddy -c "Set :CFBundleIconFile AppIcon" "$STAGED_APP/Contents/Info.plist"
else
  /usr/libexec/PlistBuddy -c "Add :CFBundleIconFile string AppIcon" "$STAGED_APP/Contents/Info.plist"
fi

codesign --force --deep --sign - "$STAGED_APP"
codesign --verify --deep --strict "$STAGED_APP"
codesign --verify --strict "$STAGED_APP/Contents/Helpers/threadrelay-daemon"

if [ -e "$OUTPUT_APP" ]; then
  mv "$OUTPUT_APP" "$BACKUP_APP"
fi
if ! mv "$STAGED_APP" "$OUTPUT_APP"; then
  if [ -e "$BACKUP_APP" ]; then
    mv "$BACKUP_APP" "$OUTPUT_APP"
  fi
  echo "failed to install assembled app" >&2
  exit 1
fi
