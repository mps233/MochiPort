#!/bin/sh
set -eu
set +x

# Build the two components together and let the installed GUI perform the
# protected daemon handoff during startup. This is intentionally a local
# handoff helper; signed release publication remains in CI.

LC_ALL=C
export LC_ALL
MAX_BUILD=9223372036854775807

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
cd "$ROOT_DIR"

if [ "$#" -gt 3 ]; then
  echo "usage: $0 [UI_VERSION] [UI_BUILD_NUMBER] [DAEMON_BUILD_NUMBER]" >&2
  exit 2
fi
if [ "$#" -eq 1 ] && { [ "$1" = "-h" ] || [ "$1" = "--help" ]; }; then
  echo "usage: $0 [UI_VERSION] [UI_BUILD_NUMBER] [DAEMON_BUILD_NUMBER]"
  echo "defaults: keep the current UI version and increment the UI build; daemon build follows UI build"
  exit 0
fi

VERSION_FILE="$ROOT_DIR/macos/MochiPort/Config/Version.xcconfig"
UI_VERSION=${1:-$(sed -n 's/^MARKETING_VERSION *= *\([^ ]*\).*/\1/p' "$VERSION_FILE" | head -n 1)}
CURRENT_BUILD=$(sed -n 's/^CURRENT_PROJECT_VERSION *= *\([^ ]*\).*/\1/p' "$VERSION_FILE" | head -n 1)

validate_build() {
  value=$1
  case "$value" in
    ''|0|0[0-9]*|*[!0-9]*)
      echo "build must be a canonical positive 64-bit integer: $value" >&2
      exit 2
      ;;
  esac
  if [ "${#value}" -gt 19 ] \
    || { [ "${#value}" -eq 19 ] && [ "$value" \> "$MAX_BUILD" ]; }; then
    echo "build must be a canonical positive 64-bit integer: $value" >&2
    exit 2
  fi
}

validate_build "$CURRENT_BUILD"
if [ "$CURRENT_BUILD" = "$MAX_BUILD" ]; then
  echo "current UI build cannot be incremented past $MAX_BUILD" >&2
  exit 2
fi
NEXT_BUILD=$((CURRENT_BUILD + 1))
UI_BUILD=${2:-$NEXT_BUILD}
DAEMON_BUILD=${3:-$UI_BUILD}

validate_build "$UI_BUILD"
validate_build "$DAEMON_BUILD"
if [ -z "$UI_VERSION" ]; then
  echo "UI version is unavailable" >&2
  exit 2
fi

ARCH=$(uname -m)
case "$ARCH" in
  arm64|x86_64) ;;
  *)
    echo "unsupported local macOS architecture: $ARCH" >&2
    exit 1
    ;;
esac

DERIVED_DATA="$ROOT_DIR/macos/MochiPort/.build/xcode"
XCODE_APP="$DERIVED_DATA/Build/Products/Release/MochiPort.app"
DAEMON_BINARY="$ROOT_DIR/target/release/mochiport"
if [ "${MOCHIPORT_OUTPUT_APP+x}" = x ]; then
  echo "MOCHIPORT_OUTPUT_APP is not supported; formal output is fixed" >&2
  exit 2
fi
OUTPUT_APP="/Users/miaopasi/codexhub/outputs/MochiPort.app"
APP_EXECUTABLE="$OUTPUT_APP/Contents/MacOS/MochiPort"

echo "Building daemon $DAEMON_BUILD for $ARCH"
MOCHIPORT_DAEMON_BUILD_NUMBER="$DAEMON_BUILD" \
  cargo build --release --bin mochiport

echo "Building MochiPort $UI_VERSION (UI build $UI_BUILD)"
MOCHIPORT_UI_VERSION="$UI_VERSION" MOCHIPORT_UI_BUILD_NUMBER="$UI_BUILD" \
  scripts/generate-swift-version.sh "$VERSION_FILE"
xcodebuild \
  -project macos/MochiPort/MochiPort.xcodeproj \
  -scheme MochiPort \
  -configuration Release \
  -derivedDataPath "$DERIVED_DATA" \
  -arch "$ARCH" \
  CODE_SIGNING_ALLOWED=NO \
  CODE_SIGNING_REQUIRED=NO \
  build

echo "Assembling $OUTPUT_APP"
scripts/assemble-swiftui-macos-app.sh \
  "$DAEMON_BUILD" \
  "$XCODE_APP" \
  "$DAEMON_BINARY" \
  "$OUTPUT_APP"

assembled_ui_build=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' \
  "$OUTPUT_APP/Contents/Info.plist")
assembled_daemon_build=$(/usr/libexec/PlistBuddy -c 'Print :MochiPortDaemonBuild' \
  "$OUTPUT_APP/Contents/Info.plist")
if [ "$assembled_ui_build" != "$UI_BUILD" ] || [ "$assembled_daemon_build" != "$DAEMON_BUILD" ]; then
  echo "assembled build mismatch: UI=$assembled_ui_build daemon=$assembled_daemon_build" >&2
  exit 1
fi

running_gui_pids() {
  ps -Ao pid=,command= | awk -v expected="$APP_EXECUTABLE" '$2 == expected { print $1 }'
}

is_formal_gui_pid() {
  pid=$1
  case "$pid" in
    ''|*[!0-9]*) return 1 ;;
  esac
  kill -0 "$pid" 2>/dev/null || return 1
  ps -p "$pid" -o command= 2>/dev/null \
    | awk -v expected="$APP_EXECUTABLE" '$1 == expected { found = 1 } END { exit !found }'
}

new_formal_gui_is_alive() {
  for pid in $NEW_GUI_PIDS; do
    if is_formal_gui_pid "$pid"; then
      return 0
    fi
  done
  return 1
}

canonical_path() {
  path=$1
  link_depth=0
  case "$path" in
    /*) ;;
    *) path="$(pwd -P)/$path" ;;
  esac

  while :; do
    parent=$(dirname "$path")
    leaf=$(basename "$path")
    parent=$(CDPATH= cd -P -- "$parent" 2>/dev/null && pwd -P) || return 1
    path="$parent/$leaf"
    if [ ! -L "$path" ]; then
      printf '%s\n' "$path"
      return 0
    fi
    link_depth=$((link_depth + 1))
    [ "$link_depth" -le 32 ] || return 1
    link=$(readlink "$path") || return 1
    case "$link" in
      /*) path=$link ;;
      *) path="$(dirname "$path")/$link" ;;
    esac
  done
}

json_value() {
  /usr/bin/plutil -extract "$1" raw -o - "$2" 2>/dev/null
}

sha256_file() {
  /usr/bin/shasum -a 256 "$1" 2>/dev/null | awk 'NR == 1 { print $1 }'
}

is_decimal() {
  case "$1" in
    ''|*[!0-9]*) return 1 ;;
  esac
  return 0
}

is_positive_decimal() {
  is_decimal "$1" && [ "$1" != 0 ]
}

is_loopback_base_url() {
  base_url=$1
  case "$base_url" in
    http://127.0.0.1:*) port_and_path=${base_url#http://127.0.0.1:} ;;
    http://\[::1\]:*) port_and_path=${base_url#http://\[::1\]:} ;;
    *) return 1 ;;
  esac
  case "$port_and_path" in
    */) port=${port_and_path%/} ;;
    *) port=$port_and_path ;;
  esac
  case "$port" in
    ''|*[!0-9]*) return 1 ;;
  esac
  [ "${#port}" -le 5 ] && [ "$port" -ge 1 ] && [ "$port" -le 65535 ]
}

is_sha256() {
  value=$1
  [ "${#value}" -eq 64 ] || return 1
  case "$value" in
    *[!0123456789abcdef]*) return 1 ;;
  esac
  return 0
}

launchctl_pid() {
  launchctl print "$SERVICE_TARGET" 2>/dev/null | awk '
    /^[[:space:]]*pid = [0-9]+$/ {
      value = $0
      sub(/^[[:space:]]*pid = /, "", value)
      print value
      exit
    }
  '
}

verify_lifecycle_probe() {
  [ -f "$ACTIVE_LOCATOR" ] && [ ! -L "$ACTIVE_LOCATOR" ] || return 1
  [ -f "$CONTROL_FILE" ] && [ ! -L "$CONTROL_FILE" ] || return 1

  data_directory_canonical=$(canonical_path "$DATA_DIRECTORY") || return 1
  control_file_canonical=$(canonical_path "$CONTROL_FILE") || return 1
  expected_runtime="$data_directory_canonical/runtimes/$DAEMON_BUILD/mochiport-daemon"
  [ -f "$expected_runtime" ] && [ -x "$expected_runtime" ] || return 1
  expected_runtime_canonical=$(canonical_path "$expected_runtime") || return 1
  [ "$expected_runtime_canonical" = "$expected_runtime" ] || return 1

  locator_service=$(json_value service "$ACTIVE_LOCATOR") || return 1
  locator_api_major=$(json_value apiMajor "$ACTIVE_LOCATOR") || return 1
  locator_instance_id=$(json_value instanceId "$ACTIVE_LOCATOR") || return 1
  locator_pid=$(json_value pid "$ACTIVE_LOCATOR") || return 1
  locator_started_at=$(json_value startedAtMs "$ACTIVE_LOCATOR") || return 1
  locator_base_url=$(json_value baseUrl "$ACTIVE_LOCATOR") || return 1
  locator_control_file=$(json_value controlFile "$ACTIVE_LOCATOR") || return 1
  [ "$locator_service" = mochiport ] || return 1
  [ "$locator_api_major" = 1 ] || return 1
  [ -n "$locator_instance_id" ] || return 1
  is_positive_decimal "$locator_pid" || return 1
  is_decimal "$locator_started_at" || return 1
  is_loopback_base_url "$locator_base_url" || return 1
  locator_control_file_canonical=$(canonical_path "$locator_control_file") || return 1
  [ "$locator_control_file_canonical" = "$control_file_canonical" ] || return 1

  token=$(json_value managementToken "$CONTROL_FILE") || return 1
  case "$token" in
    ''|*[![:graph:]]*) return 1 ;;
  esac
  [ "${#token}" -le 256 ] || return 1
  locator_fingerprint=$(sha256_file "$ACTIVE_LOCATOR") || return 1
  control_fingerprint=$(sha256_file "$CONTROL_FILE") || return 1
  [ -n "$locator_fingerprint" ] && [ -n "$control_fingerprint" ] || return 1

  case "$locator_base_url" in
    */) lifecycle_url="${locator_base_url}api/v1/manage/lifecycle" ;;
    *) lifecycle_url="$locator_base_url/api/v1/manage/lifecycle" ;;
  esac
  if ! printf 'Authorization: Bearer %s\n' "$token" \
    | /usr/bin/curl --fail --silent --show-error --globoff --noproxy '*' --proto '=http' \
      --connect-timeout 1 --max-time 2 --header @- \
      --output "$LIFECYCLE_RESPONSE" "$lifecycle_url"; then
    unset token
    return 1
  fi
  unset token

  [ "$(sha256_file "$ACTIVE_LOCATOR")" = "$locator_fingerprint" ] || return 1
  [ "$(sha256_file "$CONTROL_FILE")" = "$control_fingerprint" ] || return 1

  lifecycle_service=$(json_value service.service "$LIFECYCLE_RESPONSE") || return 1
  lifecycle_api_major=$(json_value service.apiMajor "$LIFECYCLE_RESPONSE") || return 1
  lifecycle_ready=$(json_value service.ready "$LIFECYCLE_RESPONSE") || return 1
  lifecycle_instance_id=$(json_value service.instanceId "$LIFECYCLE_RESPONSE") || return 1
  lifecycle_pid=$(json_value service.pid "$LIFECYCLE_RESPONSE") || return 1
  lifecycle_started_at=$(json_value service.startedAtMs "$LIFECYCLE_RESPONSE") || return 1
  lifecycle_runtime_state=$(json_value runtime.state "$LIFECYCLE_RESPONSE") || return 1
  lifecycle_runtime_api_major=$(json_value runtime.apiMajor "$LIFECYCLE_RESPONSE") || return 1
  lifecycle_build=$(json_value runtime.buildNumber "$LIFECYCLE_RESPONSE") || return 1
  lifecycle_executable=$(json_value executable "$LIFECYCLE_RESPONSE") || return 1
  lifecycle_sha256=$(json_value executableSha256 "$LIFECYCLE_RESPONSE") || return 1
  [ "$lifecycle_service" = mochiport ] || return 1
  [ "$lifecycle_api_major" = 1 ] || return 1
  [ "$lifecycle_runtime_api_major" = 1 ] || return 1
  [ "$lifecycle_ready" = true ] || return 1
  [ "$lifecycle_runtime_state" = active ] || return 1
  [ "$lifecycle_build" = "$DAEMON_BUILD" ] || return 1
  [ "$lifecycle_instance_id" = "$locator_instance_id" ] || return 1
  [ "$lifecycle_started_at" = "$locator_started_at" ] || return 1
  is_positive_decimal "$lifecycle_pid" || return 1
  [ "$lifecycle_pid" = "$locator_pid" ] || return 1

  loaded_pid=$(launchctl_pid)
  is_positive_decimal "$loaded_pid" || return 1
  [ "$loaded_pid" = "$locator_pid" ] || return 1
  [ "$loaded_pid" = "$lifecycle_pid" ] || return 1

  case "$lifecycle_executable" in
    /*) ;;
    *) return 1 ;;
  esac
  lifecycle_executable_canonical=$(canonical_path "$lifecycle_executable") || return 1
  [ "$lifecycle_executable_canonical" = "$expected_runtime_canonical" ] || return 1
  lifecycle_sha256=$(printf '%s' "$lifecycle_sha256" | tr '[:upper:]' '[:lower:]')
  local_sha256=$(sha256_file "$expected_runtime_canonical") || return 1
  is_sha256 "$lifecycle_sha256" || return 1
  is_sha256 "$local_sha256" || return 1
  [ "$lifecycle_sha256" = "$local_sha256" ] || return 1
}

echo "Closing the previous formal GUI"
if [ -n "$(running_gui_pids)" ]; then
  osascript -e 'tell application "MochiPort" to quit' >/dev/null 2>&1 || true
  i=0
  while [ -n "$(running_gui_pids)" ] && [ "$i" -lt 40 ]; do
    sleep 0.25
    i=$((i + 1))
  done
  if [ -n "$(running_gui_pids)" ]; then
    echo "formal GUI did not exit cleanly; refusing to force-kill it" >&2
    exit 1
  fi
fi

echo "Opening $OUTPUT_APP"
open "$OUTPUT_APP"
i=0
while [ -z "$(running_gui_pids)" ] && [ "$i" -lt 80 ]; do
  sleep 0.25
  i=$((i + 1))
done
if [ -z "$(running_gui_pids)" ]; then
  echo "formal GUI did not start: $APP_EXECUTABLE" >&2
  exit 1
fi

NEW_GUI_PIDS=$(running_gui_pids)
if ! new_formal_gui_is_alive; then
  echo "new formal GUI is not running from $APP_EXECUTABLE" >&2
  exit 1
fi

DATA_DIRECTORY=${MOCHIPORT_HOME:-"$HOME/Library/Application Support/MochiPort"}
SERVICE_TARGET="gui/$(id -u)/io.github.mps233.mochiport.daemon"
ACTIVE_LOCATOR="$DATA_DIRECTORY/mochiport-active-daemon.json"
CONTROL_FILE="$DATA_DIRECTORY/mochiport-control.json"
LIFECYCLE_RESPONSE=$(mktemp "${TMPDIR:-/tmp}/mochiport-lifecycle.XXXXXX")
trap 'rm -f "$LIFECYCLE_RESPONSE"' EXIT
trap 'exit 1' HUP INT TERM

stable_probes=0
i=0
while [ "$i" -lt 240 ]; do
  if new_formal_gui_is_alive && verify_lifecycle_probe; then
    stable_probes=$((stable_probes + 1))
    if [ "$stable_probes" -ge 3 ] && new_formal_gui_is_alive; then
      echo "MochiPort UI build $UI_BUILD and daemon build $DAEMON_BUILD are running"
      exit 0
    fi
  else
    stable_probes=0
  fi
  sleep 0.25
  i=$((i + 1))
done

if ! new_formal_gui_is_alive; then
  echo "new formal GUI exited before daemon verification completed" >&2
else
  echo "daemon did not pass authenticated lifecycle verification at build $DAEMON_BUILD" >&2
fi
exit 1
