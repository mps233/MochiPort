#!/bin/sh

set -eu

# Build the universal daemon and SwiftUI shell, install the formal app, and
# switch the running daemon through the authenticated transactional API.
# The output is intentionally only ThreadRelay.app; no archive is produced.

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd -P)
FORMAL_APP="$REPO_ROOT/outputs/ThreadRelay.app"
CONFIG_DIR="$HOME/Library/Application Support/CodexHub"
CONTROL_FILE="$CONFIG_DIR/threadrelay-control.json"
DAEMON_LABEL="io.github.mps233.threadrelay.daemon"
GUI_LABEL="io.github.mps233.threadrelay.gui"

usage() {
  cat >&2 <<'EOF'
usage: scripts/formal-macos-handoff.sh [--build N] [--force] [--fast]

Build and hand off the formal macOS app. --force explicitly allows replacing a
daemon while protected work is present and may interrupt active Codex/IM work.
--fast skips the Rust and Xcode test suites but still performs build and app
integrity checks.
EOF
}

fail() {
  printf 'formal-handoff: %s\n' "$*" >&2
  exit 1
}

need_command() {
  command -v "$1" >/dev/null 2>&1 || fail "missing command: $1"
}

BUILD=""
FORCE=0
FAST=0
while [ "$#" -gt 0 ]; do
  case "$1" in
    --build)
      [ "$#" -ge 2 ] || { usage; exit 2; }
      BUILD=$2
      shift 2
      ;;
    --force)
      FORCE=1
      shift
      ;;
    --fast)
      FAST=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      usage
      exit 2
      ;;
  esac
done

[ "$(uname -s)" = Darwin ] || fail "this handoff is macOS-only"
need_command cargo
need_command curl
need_command jq
need_command lipo
need_command shasum
need_command xcodebuild
need_command xcrun
need_command codesign

process_pids_for_path() {
  expected_path=$1
  /bin/ps -axo pid=,command= | /usr/bin/awk -v expected="$expected_path" '
    {
      for (field = 2; field <= NF; field++) {
        if ($field == expected) {
          print $1
          break
        }
      }
    }'
}

wait_for_pids_to_exit() {
  pids=$1
  attempt=0
  while [ "$attempt" -lt 40 ]; do
    alive=0
    for pid in $pids; do
      case "$pid" in
        ''|*[!0-9]*) continue ;;
      esac
      if kill -0 "$pid" 2>/dev/null; then
        alive=1
        break
      fi
    done
    [ "$alive" -eq 0 ] && return 0
    sleep 0.25
    attempt=$((attempt + 1))
  done
  return 1
}

wait_for_gui_agent_unloaded() {
  target=$1
  attempt=0
  while [ "$attempt" -lt 40 ]; do
    if ! /bin/launchctl print "$target" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.25
    attempt=$((attempt + 1))
  done
  return 1
}

wait_for_gui_agent_loaded() {
  target=$1
  attempt=0
  while [ "$attempt" -lt 40 ]; do
    if /bin/launchctl print "$target" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.25
    attempt=$((attempt + 1))
  done
  return 1
}

if [ -z "$BUILD" ]; then
  if [ -f "$FORMAL_APP/Contents/Info.plist" ]; then
    CURRENT_BUILD=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$FORMAL_APP/Contents/Info.plist" 2>/dev/null || true)
  else
    CURRENT_BUILD=""
  fi
  case "$CURRENT_BUILD" in
    ''|*[!0-9]*) BUILD=1 ;;
    *) BUILD=$((CURRENT_BUILD + 1)) ;;
  esac
fi
case "$BUILD" in
  ''|0|*[!0-9]*) fail "build must be a positive integer" ;;
esac

[ -f "$CONTROL_FILE" ] || fail "management control file is unavailable: $CONTROL_FILE"

WORK_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/threadrelay-formal-handoff.XXXXXX")
cleanup() {
  code=$?
  trap - EXIT HUP INT TERM
  case "$WORK_ROOT" in
    "${TMPDIR:-/tmp}"/threadrelay-formal-handoff.*) rm -rf "$WORK_ROOT" ;;
  esac
  exit "$code"
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

DERIVED_DATA="$REPO_ROOT/macos/ThreadRelay/.derived-data/formal-$BUILD"
VERSION_XCCONFIG="$REPO_ROOT/macos/ThreadRelay/Config/Version.xcconfig"
DAEMON_ARM="$REPO_ROOT/target/aarch64-apple-darwin/release/threadrelay"
DAEMON_X86="$REPO_ROOT/target/x86_64-apple-darwin/release/threadrelay"
DAEMON_UNIVERSAL="$WORK_ROOT/threadrelay-daemon"
XCODE_APP="$DERIVED_DATA/Build/Products/Release/ThreadRelay.app"

printf 'formal-handoff: building build %s\n' "$BUILD"
THREADRELAY_BUILD_NUMBER="$BUILD" cargo build --locked --release --features gui --bin threadrelay --target aarch64-apple-darwin
THREADRELAY_BUILD_NUMBER="$BUILD" cargo build --locked --release --features gui --bin threadrelay --target x86_64-apple-darwin
[ -x "$DAEMON_ARM" ] || fail "arm64 daemon build is missing"
[ -x "$DAEMON_X86" ] || fail "x86_64 daemon build is missing"
lipo -create "$DAEMON_ARM" "$DAEMON_X86" -output "$DAEMON_UNIVERSAL"
chmod 755 "$DAEMON_UNIVERSAL"
THREADRELAY_BUILD_NUMBER="$BUILD" "$REPO_ROOT/scripts/generate-swift-version.sh" "$VERSION_XCCONFIG"

if [ "$FAST" -eq 0 ]; then
  THREADRELAY_BUILD_NUMBER="$BUILD" cargo test --locked --features gui --bin threadrelay
fi

xcodebuild build \
  -project "$REPO_ROOT/macos/ThreadRelay/ThreadRelay.xcodeproj" \
  -scheme ThreadRelay \
  -configuration Release \
  -destination 'platform=macOS' \
  -derivedDataPath "$DERIVED_DATA" \
  ARCHS='arm64 x86_64' \
  ONLY_ACTIVE_ARCH=NO \
  CODE_SIGNING_ALLOWED=NO

[ -x "$XCODE_APP/Contents/MacOS/ThreadRelay" ] || fail "SwiftUI app build is missing"
if [ "$FAST" -eq 0 ]; then
  xcodebuild test \
    -project "$REPO_ROOT/macos/ThreadRelay/ThreadRelay.xcodeproj" \
    -scheme ThreadRelay \
    -configuration Release \
    -destination 'platform=macOS' \
    -derivedDataPath "$DERIVED_DATA" \
    ARCHS='arm64 x86_64' \
    ONLY_ACTIVE_ARCH=NO \
    CODE_SIGNING_ALLOWED=NO
fi

"$REPO_ROOT/scripts/assemble-swiftui-macos-app.sh" \
  "$BUILD" "$XCODE_APP" "$DAEMON_UNIVERSAL" "$FORMAL_APP"

SWIFTC=$(xcrun --find swiftc)
MACOS_SDK=$(xcrun --sdk macosx --show-sdk-path)
SWITCH_BINARY="$WORK_ROOT/threadrelay-runtime-switch"
case "$(uname -m)" in
  arm64) SWIFT_TARGET=arm64-apple-macosx13.0 ;;
  x86_64) SWIFT_TARGET=x86_64-apple-macosx13.0 ;;
  *) fail "unsupported host architecture: $(uname -m)" ;;
esac
"$SWIFTC" \
  -sdk "$MACOS_SDK" \
  -target "$SWIFT_TARGET" \
  -O \
  -parse-as-library \
  "$REPO_ROOT/macos/ThreadRelay/Sources/ThreadRelayMac/APIClient.swift" \
  "$REPO_ROOT/macos/ThreadRelay/Sources/ThreadRelayMac/DaemonLauncher.swift" \
  "$REPO_ROOT/scripts/macos-runtime-switch.swift" \
  -o "$SWITCH_BINARY"

TOKEN=$(jq -r '.managementToken // empty' "$CONTROL_FILE")
[ -n "$TOKEN" ] || fail "management token is unavailable"
LIFECYCLE_JSON="$WORK_ROOT/lifecycle.json"
curl -fsS -H "Authorization: Bearer $TOKEN" \
  http://127.0.0.1:3847/api/v1/manage/lifecycle > "$LIFECYCLE_JSON"

PROTECTED=$(jq -r '.protectedWorkItems.total // 0' "$LIFECYCLE_JSON")
if [ "$PROTECTED" -gt 0 ] && [ "$FORCE" -eq 0 ]; then
  fail "后台仍有 $PROTECTED 项受保护任务；需要明确传入 --force 才会切换"
fi

INSTALLATION_ID=$(defaults read io.github.mps233.threadrelay \
  threadrelay.management.installation-id 2>/dev/null || true)
if [ -z "$INSTALLATION_ID" ]; then
  INSTALLATION_ID=$(jq -r '.lease.installationId // empty' "$CONTROL_FILE")
fi
[ -n "$INSTALLATION_ID" ] || fail "无法确定 GUI 管理安装 ID"

DAEMON_PID=$(jq -r '.service.pid' "$LIFECYCLE_JSON")
DAEMON_INSTANCE=$(jq -r '.service.instanceId' "$LIFECYCLE_JSON")
DAEMON_EXECUTABLE=$(jq -r '.executable' "$LIFECYCLE_JSON")
LEASE_GENERATION=$(jq -r '.management.leaseGeneration // empty' "$LIFECYCLE_JSON")
[ -n "$LEASE_GENERATION" ] || fail "当前 GUI 尚未取得后台服务管理租约，请先启动正式 GUI"

printf 'formal-handoff: switching daemon (protected=%s, force=%s)\n' "$PROTECTED" "$FORCE"
THREADRELAY_HOME="$CONFIG_DIR" "$SWITCH_BINARY" switch "$FORMAL_APP" \
  "$INSTALLATION_ID" "$DAEMON_INSTANCE" "$DAEMON_PID" "$DAEMON_EXECUTABLE" \
  "$LEASE_GENERATION" "$CONTROL_FILE" "$FORCE"

# The SwiftUI process that was already running is tied to the previous app
# bundle. Reload the formal LaunchAgent so the visible UI and supervisor use
# the same build as the daemon.
UID_NOW=$(id -u)
GUI_TARGET="gui/$UID_NOW/$GUI_LABEL"
GUI_PLIST="$HOME/Library/LaunchAgents/$GUI_LABEL.plist"
[ -f "$GUI_PLIST" ] || fail "GUI LaunchAgent plist is missing: $GUI_PLIST"

GUI_EXECUTABLE="$FORMAL_APP/Contents/MacOS/ThreadRelay"
GUI_SUPERVISOR="$FORMAL_APP/Contents/Helpers/threadrelay-gui-supervisor"
OLD_GUI_PIDS=$(process_pids_for_path "$GUI_EXECUTABLE" || true)
OLD_SUPERVISOR_PIDS=$(process_pids_for_path "$GUI_SUPERVISOR" || true)

# bootout is asynchronous with respect to the child GUI. Wait for launchd to
# unload the job, then terminate any process left from the old bundle before
# bootstrapping the new supervisor. This keeps a stale GUI from winning the
# single-instance lock after the new app is launched.
/bin/launchctl bootout "$GUI_TARGET" >/dev/null 2>&1 || true
wait_for_gui_agent_unloaded "$GUI_TARGET" \
  || fail "旧版 GUI LaunchAgent 未能退出，已取消 GUI 重启"
for pid in $OLD_GUI_PIDS $OLD_SUPERVISOR_PIDS; do
  case "$pid" in
    ''|*[!0-9]*) continue ;;
  esac
  kill -TERM "$pid" 2>/dev/null || true
done
wait_for_pids_to_exit "$OLD_GUI_PIDS $OLD_SUPERVISOR_PIDS" \
  || fail "旧版 GUI 进程未能退出，已取消 GUI 重启"

# An explicit handoff is a requested relaunch, so a marker written by the
# previous GUI must not make the freshly bootstrapped supervisor exit.
rm -f "$CONFIG_DIR/gui-normal-exit.marker"
/bin/launchctl bootstrap "gui/$UID_NOW" "$GUI_PLIST" \
  || fail "无法重新加载 ThreadRelay GUI LaunchAgent"
wait_for_gui_agent_loaded "$GUI_TARGET" \
  || fail "ThreadRelay GUI LaunchAgent 未能重新加载"

GUI_PID=""
for _ in $(seq 1 40); do
  GUI_PID=$(process_pids_for_path "$GUI_EXECUTABLE" | head -1 || true)
  [ -n "$GUI_PID" ] && break
  sleep 0.25
done
GUI_SUPERVISOR_PID=$(process_pids_for_path "$GUI_SUPERVISOR" | head -1 || true)

FINAL_JSON="$WORK_ROOT/final-lifecycle.json"
curl -fsS -H "Authorization: Bearer $TOKEN" \
  http://127.0.0.1:3847/api/v1/manage/lifecycle > "$FINAL_JSON"
FINAL_BUILD=$(jq -r '.runtime.buildNumber // empty' "$FINAL_JSON")
FINAL_EXECUTABLE=$(jq -r '.executable' "$FINAL_JSON")
[ "$FINAL_BUILD" = "$BUILD" ] || fail "daemon build mismatch after handoff: expected $BUILD, got ${FINAL_BUILD:-unknown}"
case "$FINAL_EXECUTABLE" in
  "$CONFIG_DIR/runtimes/$BUILD/threadrelay-daemon") ;;
  *) fail "daemon executable does not point to runtime $BUILD: $FINAL_EXECUTABLE" ;;
esac
curl -fsS http://127.0.0.1:3847/healthz >/dev/null || fail "daemon health check failed after handoff"
[ -n "$GUI_PID" ] || fail "formal GUI did not relaunch"
[ -n "$GUI_SUPERVISOR_PID" ] || fail "formal GUI supervisor did not relaunch"
case " $OLD_GUI_PIDS " in
  *" $GUI_PID "*) fail "formal GUI PID was not replaced during handoff" ;;
esac

printf 'formal-handoff: ready app=%s build=%s daemon_pid=%s gui_pid=%s\n' \
  "$FORMAL_APP" "$BUILD" "$(jq -r '.service.pid' "$FINAL_JSON")" "$GUI_PID"
