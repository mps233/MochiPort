#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP="${1:-$ROOT/target/dist/Codex Remote.app}"
IDENTITY="${APPLE_CODESIGN_IDENTITY:-}"
NOTARY_KEYCHAIN_PROFILE="${NOTARY_KEYCHAIN_PROFILE:-}"
APPLE_API_KEY_ID="${APPLE_API_KEY_ID:-}"
APPLE_API_ISSUER_ID="${APPLE_API_ISSUER_ID:-}"
APPLE_API_KEY_PATH="${APPLE_API_KEY_PATH:-}"
TEAM_ID="${APPLE_TEAM_ID:-}"
APPLE_ID="${APPLE_ID:-}"
APP_PASSWORD="${APPLE_APP_SPECIFIC_PASSWORD:-}"
ENTITLEMENTS="$ROOT/packaging/macos/entitlements.plist"
DMG="${APP%.app}.dmg"
ZIP="${APP%.app}.zip"

if [[ -z "$IDENTITY" ]]; then
  echo "APPLE_CODESIGN_IDENTITY is required, for example: Developer ID Application: Name (TEAMID)" >&2
  exit 2
fi
if [[ -z "$NOTARY_KEYCHAIN_PROFILE" && ( -z "$APPLE_API_KEY_ID" || -z "$APPLE_API_ISSUER_ID" || -z "$APPLE_API_KEY_PATH" ) && ( -z "$TEAM_ID" || -z "$APPLE_ID" || -z "$APP_PASSWORD" ) ]]; then
  echo "Set NOTARY_KEYCHAIN_PROFILE, or APPLE_API_KEY_ID/APPLE_API_ISSUER_ID/APPLE_API_KEY_PATH, or APPLE_TEAM_ID/APPLE_ID/APPLE_APP_SPECIFIC_PASSWORD for notarization" >&2
  exit 2
fi
if [[ ! -d "$APP" ]]; then
  echo "App bundle not found: $APP" >&2
  exit 2
fi

xcrun_notarytool_submit() {
  local path="$1"
  if [[ -n "$NOTARY_KEYCHAIN_PROFILE" ]]; then
    xcrun notarytool submit "$path" \
      --keychain-profile "$NOTARY_KEYCHAIN_PROFILE" \
      --wait
  elif [[ -n "$APPLE_API_KEY_ID" ]]; then
    xcrun notarytool submit "$path" \
      --key "$APPLE_API_KEY_PATH" \
      --key-id "$APPLE_API_KEY_ID" \
      --issuer "$APPLE_API_ISSUER_ID" \
      --wait
  else
    xcrun notarytool submit "$path" \
      --apple-id "$APPLE_ID" \
      --team-id "$TEAM_ID" \
      --password "$APP_PASSWORD" \
      --wait
  fi
}

codesign --force --timestamp --options runtime \
  --entitlements "$ENTITLEMENTS" \
  --sign "$IDENTITY" \
  "$APP/Contents/Resources/codex-remote"

codesign --force --timestamp --options runtime \
  --entitlements "$ENTITLEMENTS" \
  --sign "$IDENTITY" \
  "$APP"

codesign --verify --deep --strict --verbose=2 "$APP"
spctl --assess --type execute --verbose=4 "$APP" || true

rm -f "$ZIP" "$DMG"
ditto -c -k --keepParent "$APP" "$ZIP"

xcrun_notarytool_submit "$ZIP"

xcrun stapler staple "$APP"

hdiutil create -volname "Codex Remote" \
  -srcfolder "$APP" \
  -ov \
  -format UDZO \
  "$DMG"

codesign --force --timestamp --sign "$IDENTITY" "$DMG"
xcrun_notarytool_submit "$DMG"

xcrun stapler staple "$DMG"
spctl --assess --type open --context context:primary-signature --verbose=4 "$DMG"

echo "$DMG"
