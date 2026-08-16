#!/bin/sh

set -eu

FORMAL_DAEMON_LABEL="io.github.mps233.threadrelay.daemon"

usage() {
  cat >&2 <<'EOF'
usage: scripts/test-macos-daemon-fault-matrix.sh --run [DAEMON_BINARY]

Runs an isolated macOS launchd fault matrix against a real ThreadRelay daemon.
The explicit --run flag is required because the test starts and stops temporary
launchd jobs. It never loads, unloads, or signals the production daemon label.
EOF
}

fail() {
  printf 'fault-matrix: %s\n' "$*" >&2
  exit 1
}

note() {
  printf 'fault-matrix: %s\n' "$*"
}

if [ "${1:-}" != "--run" ]; then
  usage
  exit 2
fi
shift

if [ "$#" -gt 1 ]; then
  usage
  exit 2
fi

if [ "$(uname -s)" != "Darwin" ]; then
  fail "this matrix is macOS-only"
fi

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd -P)
DAEMON_BINARY=${1:-"$REPO_ROOT/target/release/threadrelay"}

if [ ! -x "$DAEMON_BINARY" ]; then
  fail "daemon binary is not executable: $DAEMON_BINARY"
fi
DAEMON_DIR=$(CDPATH= cd -- "$(dirname -- "$DAEMON_BINARY")" && pwd -P)
DAEMON_BINARY="$DAEMON_DIR/$(basename -- "$DAEMON_BINARY")"

case "$("$DAEMON_BINARY" --version 2>/dev/null || true)" in
  threadrelay\ *) ;;
  *) fail "daemon binary does not identify as ThreadRelay" ;;
esac

PYTHON=$(command -v python3 || true)
CURL=$(command -v curl || true)
PLUTIL=$(command -v plutil || true)
LAUNCHCTL=/bin/launchctl
STRINGS=/usr/bin/strings
XCRUN=/usr/bin/xcrun

[ -n "$PYTHON" ] || fail "python3 is required"
[ -n "$CURL" ] || fail "curl is required"
[ -n "$PLUTIL" ] || fail "plutil is required"
[ -x "$LAUNCHCTL" ] || fail "launchctl is unavailable"
[ -x "$STRINGS" ] || fail "strings is unavailable"
[ -x "$XCRUN" ] || fail "xcrun is unavailable"
SWIFTC=$("$XCRUN" --find swiftc 2>/dev/null || true)
[ -x "$SWIFTC" ] || fail "Swift compiler is required"
MACOS_SDK=$($XCRUN --sdk macosx --show-sdk-path 2>/dev/null || true)
[ -d "$MACOS_SDK" ] || fail "macOS SDK is required"
if ! "$STRINGS" "$DAEMON_BINARY" \
  | /usr/bin/grep -q 'THREADRELAY_SKIP_DESKTOP_INTEGRATION'; then
  fail "daemon binary does not contain the required desktop-integration isolation switch"
fi
if ! "$STRINGS" "$DAEMON_BINARY" \
  | /usr/bin/grep -q 'THREADRELAY_RUNTIME_SWITCH_HOLD'; then
  fail "daemon binary does not contain runtime-switch candidate hold support"
fi

TEST_UID=$(/usr/bin/id -u)
[ "$TEST_UID" -ne 0 ] || fail "do not run the launchd matrix as root"
REAL_HOME=${HOME:?HOME is required}
TMP_PARENT=${TMPDIR:-/tmp}
TMP_PARENT=$(CDPATH= cd -- "$TMP_PARENT" && pwd -P)
TEST_ROOT=$(/usr/bin/mktemp -d "$TMP_PARENT/threadrelay-daemon-fault-matrix.XXXXXX") \
  || fail "unable to create temporary test root"

cleanup_temp_only() {
  status=$?
  trap - 0 HUP INT TERM
  case "$TEST_ROOT" in
    "$TMP_PARENT"/threadrelay-daemon-fault-matrix.*)
      /bin/rm -rf "$TEST_ROOT"
      ;;
  esac
  exit "$status"
}

trap cleanup_temp_only 0
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

case "$TEST_ROOT" in
  "$TMP_PARENT"/threadrelay-daemon-fault-matrix.*) ;;
  *) fail "mktemp returned an unexpected path: $TEST_ROOT" ;;
esac

RUN_ID=${TEST_ROOT##*.}
RUN_ID=$(printf '%s' "$RUN_ID" | /usr/bin/tr -cd '[:alnum:]')
[ -n "$RUN_ID" ] || fail "temporary test id is empty"

DAEMON_LABEL="io.github.mps233.threadrelay.tests.fault-matrix.${TEST_UID}.${RUN_ID}.daemon"
BLOCKER_LABEL="io.github.mps233.threadrelay.tests.fault-matrix.${TEST_UID}.${RUN_ID}.blocker"
DOMAIN="gui/$TEST_UID"
FORMAL_DAEMON_SERVICE="$DOMAIN/$FORMAL_DAEMON_LABEL"
DAEMON_SERVICE="$DOMAIN/$DAEMON_LABEL"
BLOCKER_SERVICE="$DOMAIN/$BLOCKER_LABEL"

case "$DAEMON_LABEL:$BLOCKER_LABEL" in
  *"$FORMAL_DAEMON_LABEL"*) fail "test label collides with the production daemon label" ;;
esac
case "$DAEMON_LABEL" in
  io.github.mps233.threadrelay.tests.fault-matrix.*.daemon) ;;
  *) fail "unsafe daemon test label: $DAEMON_LABEL" ;;
esac
case "$BLOCKER_LABEL" in
  io.github.mps233.threadrelay.tests.fault-matrix.*.blocker) ;;
  *) fail "unsafe blocker test label: $BLOCKER_LABEL" ;;
esac

TEST_HOME="$TEST_ROOT/home"
THREADRELAY_TEST_HOME="$TEST_ROOT/threadrelay-home"
OCCUPIED_TEST_HOME="$TEST_ROOT/occupied-threadrelay-home"
LAUNCH_AGENT_DIR="$TEST_HOME/Library/LaunchAgents"
LOG_DIR="$TEST_ROOT/logs"
DAEMON_LOG="$LOG_DIR/daemon.log"
BLOCKER_LOG="$LOG_DIR/blocker.log"
DIRECT_FAILURE_LOG="$LOG_DIR/direct-bind-failure.log"
DAEMON_PLIST="$LAUNCH_AGENT_DIR/$DAEMON_LABEL.plist"
BLOCKER_PLIST="$LAUNCH_AGENT_DIR/$BLOCKER_LABEL.plist"
HELPER_DIR="$TEST_ROOT/helper"
HELPER_COPY="$HELPER_DIR/threadrelay-daemon"
CONFIG_PATH="$THREADRELAY_TEST_HOME/config.toml"
OCCUPIED_CONFIG_PATH="$OCCUPIED_TEST_HOME/config.toml"
BLOCKER_SCRIPT="$TEST_ROOT/hold-port.py"
LIFECYCLE_JSON="$TEST_ROOT/lifecycle.json"
HARNESS_SOURCE="$TEST_ROOT/DaemonFaultMatrixHarness.swift"
HARNESS_BINARY="$TEST_ROOT/daemon-fault-matrix-harness"
DAEMON_LAUNCHER_SOURCE="$REPO_ROOT/macos/ThreadRelay/Sources/ThreadRelayMac/DaemonLauncher.swift"

BUILD_ID=$(
  "$DAEMON_BINARY" --version \
    | /usr/bin/sed -n 's/.*(build \([^)]*\)).*/\1/p'
)
case "$BUILD_ID" in
  ''|.|..|*[!abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._-]*)
    fail "daemon build identifier is invalid: ${BUILD_ID:-empty}"
    ;;
esac
PREVIOUS_PROGRAM="$THREADRELAY_TEST_HOME/runtimes/$BUILD_ID/threadrelay-daemon"
OCCUPIED_PROGRAM="$OCCUPIED_TEST_HOME/runtimes/$BUILD_ID/threadrelay-daemon"
ACTIVE_LOCATOR="$TEST_HOME/Library/Application Support/ThreadRelay/threadrelay-active-daemon.json"
FORMAL_STATE_BEFORE="$TEST_ROOT/formal-state.before"
FORMAL_STATE_AFTER="$TEST_ROOT/formal-state.after"
FORMAL_ENV_BEFORE="$TEST_ROOT/formal-environment.before"
FORMAL_ENV_AFTER="$TEST_ROOT/formal-environment.after"
FORMAL_LOCATORS_BEFORE="$TEST_ROOT/formal-locators.before"
FORMAL_LOCATORS_AFTER="$TEST_ROOT/formal-locators.after"
FORMAL_SNAPSHOT_READY=0

snapshot_formal_service() {
  output=$1
  if "$LAUNCHCTL" print "$FORMAL_DAEMON_SERVICE" >"$TEST_ROOT/formal-launchctl.txt" 2>/dev/null; then
    formal_pid=$(
      /usr/bin/awk '$1 == "pid" && $2 == "=" && $3 ~ /^[0-9]+$/ { print $3; exit }' \
        "$TEST_ROOT/formal-launchctl.txt"
    )
    formal_program=$(
      /usr/bin/sed -n 's/^[[:space:]]*program = //p' "$TEST_ROOT/formal-launchctl.txt" \
        | /usr/bin/head -n 1
    )
    printf 'loaded\npid=%s\nprogram=%s\n' "$formal_pid" "$formal_program" >"$output"
  else
    printf 'unloaded\n' >"$output"
  fi
}

snapshot_formal_environment() {
  output=$1
  : >"$output"
  for key in \
    CODEX_API_BASE_URL \
    CODEX_API_ENDPOINT \
    CODEX_APP_SERVER_LOGIN_ISSUER \
    CODEX_CLI_PATH \
    CODEXHUB_REAL_CODEX_CLI_PATH \
    NO_PROXY \
    no_proxy; do
    value=$("$LAUNCHCTL" getenv "$key" 2>/dev/null || true)
    printf '%s=%s\n' "$key" "$value" >>"$output"
  done
}

snapshot_formal_locators() {
  output=$1
  : >"$output"
  for locator in \
    "$REAL_HOME/Library/Application Support/ThreadRelay/threadrelay-active-daemon.json" \
    "$REAL_HOME/Library/Application Support/CodexHub/threadrelay-active-daemon.json"; do
    if [ -f "$locator" ]; then
      digest=$(/usr/bin/shasum -a 256 "$locator" | /usr/bin/awk '{ print $1 }')
      printf 'present %s %s\n' "$digest" "$locator" >>"$output"
    else
      printf 'absent %s\n' "$locator" >>"$output"
    fi
  done
}

verify_formal_unchanged() {
  [ "$FORMAL_SNAPSHOT_READY" -eq 1 ] || return 0
  snapshot_formal_service "$FORMAL_STATE_AFTER"
  snapshot_formal_environment "$FORMAL_ENV_AFTER"
  snapshot_formal_locators "$FORMAL_LOCATORS_AFTER"
  /usr/bin/cmp -s "$FORMAL_STATE_BEFORE" "$FORMAL_STATE_AFTER" \
    && /usr/bin/cmp -s "$FORMAL_ENV_BEFORE" "$FORMAL_ENV_AFTER" \
    && /usr/bin/cmp -s "$FORMAL_LOCATORS_BEFORE" "$FORMAL_LOCATORS_AFTER"
}

safe_service() {
  case "$1" in
    "$DAEMON_SERVICE"|"$BLOCKER_SERVICE") return 0 ;;
    *) return 1 ;;
  esac
}

cleanup() {
  status=$?
  trap - 0 HUP INT TERM
  set +e
  cleanup_failed=0
  for cleanup_service in "$DAEMON_SERVICE" "$BLOCKER_SERVICE"; do
    if ! safe_service "$cleanup_service"; then
      cleanup_failed=1
      continue
    fi
    if "$LAUNCHCTL" print "$cleanup_service" >/dev/null 2>&1; then
      "$LAUNCHCTL" bootout "$cleanup_service" >/dev/null 2>&1 \
        || cleanup_failed=1
    fi
    cleanup_attempts=0
    while "$LAUNCHCTL" print "$cleanup_service" >/dev/null 2>&1 \
      && [ "$cleanup_attempts" -lt 100 ]; do
      cleanup_attempts=$((cleanup_attempts + 1))
      /bin/sleep 0.1
    done
    if "$LAUNCHCTL" print "$cleanup_service" >/dev/null 2>&1; then
      cleanup_failed=1
    fi
  done
  if ! verify_formal_unchanged; then
    printf 'fault-matrix: production daemon state changed during the isolated matrix\n' >&2
    /usr/bin/diff -u "$FORMAL_STATE_BEFORE" "$FORMAL_STATE_AFTER" >&2 || true
    /usr/bin/diff -u "$FORMAL_ENV_BEFORE" "$FORMAL_ENV_AFTER" >&2 || true
    /usr/bin/diff -u "$FORMAL_LOCATORS_BEFORE" "$FORMAL_LOCATORS_AFTER" >&2 || true
    cleanup_failed=1
  fi
  if [ "$cleanup_failed" -eq 0 ]; then
    case "$TEST_ROOT" in
      "$TMP_PARENT"/threadrelay-daemon-fault-matrix.*)
        /bin/rm -rf "$TEST_ROOT"
        ;;
    esac
  else
    printf 'fault-matrix: cleanup could not unload an isolated test job; preserving %s\n' \
      "$TEST_ROOT" >&2
    [ "$status" -ne 0 ] || status=1
  fi
  exit "$status"
}

trap cleanup 0
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

/bin/mkdir -p \
  "$TEST_HOME" \
  "$TEST_ROOT/codex-home" \
  "$TEST_ROOT/tmp" \
  "$TEST_ROOT/vscode-extensions" \
  "$THREADRELAY_TEST_HOME" \
  "$OCCUPIED_TEST_HOME" \
  "$LAUNCH_AGENT_DIR" \
  "$HELPER_DIR" \
  "$LOG_DIR"
/bin/cp "$DAEMON_BINARY" "$HELPER_COPY"
/bin/chmod 755 "$HELPER_COPY"

snapshot_formal_service "$FORMAL_STATE_BEFORE"
snapshot_formal_environment "$FORMAL_ENV_BEFORE"
snapshot_formal_locators "$FORMAL_LOCATORS_BEFORE"
FORMAL_SNAPSHOT_READY=1

job_is_loaded() {
  safe_service "$1" || return 1
  "$LAUNCHCTL" print "$1" >/dev/null 2>&1
}

bootout_exact_test_job() {
  service=$1
  safe_service "$service" || fail "refusing to bootout non-test service: $service"
  if job_is_loaded "$service"; then
    "$LAUNCHCTL" bootout "$service" >/dev/null 2>&1 \
      || fail "unable to bootout test service: $service"
  fi
}

wait_for_job_unloaded() {
  service=$1
  attempts=0
  while [ "$attempts" -lt 100 ]; do
    if ! job_is_loaded "$service"; then
      return 0
    fi
    attempts=$((attempts + 1))
    /bin/sleep 0.1
  done
  fail "test service did not unload: $service"
}

bootstrap_test_job() {
  service=$1
  plist=$2
  safe_service "$service" || fail "refusing to bootstrap non-test service: $service"
  "$PLUTIL" -lint "$plist" >/dev/null
  expected_label=${service##*/}
  actual_label=$(
    "$PYTHON" - "$plist" <<'PY'
import plistlib
import sys

with open(sys.argv[1], "rb") as stream:
    print(plistlib.load(stream)["Label"])
PY
  )
  [ "$actual_label" = "$expected_label" ] \
    || fail "plist label mismatch: expected $expected_label, got $actual_label"
  [ "$actual_label" != "$FORMAL_DAEMON_LABEL" ] \
    || fail "refusing to bootstrap the production daemon label"
  "$LAUNCHCTL" bootstrap "$DOMAIN" "$plist" >/dev/null \
    || fail "unable to bootstrap test service: $service"
}

job_pid() {
  service=$1
  safe_service "$service" || return 1
  "$LAUNCHCTL" print "$service" 2>/dev/null \
    | /usr/bin/awk '$1 == "pid" && $2 == "=" && $3 ~ /^[0-9]+$/ { print $3; exit }'
}

job_run_count() {
  service=$1
  safe_service "$service" || return 1
  "$LAUNCHCTL" print "$service" 2>/dev/null \
    | /usr/bin/awk '$1 == "runs" && $2 == "=" && $3 ~ /^[0-9]+$/ { print $3; exit }'
}

wait_for_job_pid() {
  service=$1
  previous_pid=${2:-}
  attempts=0
  while [ "$attempts" -lt 200 ]; do
    current_pid=$(job_pid "$service" || true)
    if [ -n "$current_pid" ] \
      && [ "$current_pid" != "$previous_pid" ] \
      && /bin/kill -0 "$current_pid" 2>/dev/null; then
      printf '%s\n' "$current_pid"
      return 0
    fi
    attempts=$((attempts + 1))
    /bin/sleep 0.1
  done
  fail "test service did not expose a replacement pid: $service"
}

wait_for_run_count() {
  service=$1
  minimum=$2
  attempts=0
  while [ "$attempts" -lt 200 ]; do
    count=$(job_run_count "$service" || true)
    if [ -n "$count" ] && [ "$count" -ge "$minimum" ]; then
      return 0
    fi
    attempts=$((attempts + 1))
    /bin/sleep 0.1
  done
  fail "test service did not reach run count $minimum: $service"
}

allocate_port() {
  "$PYTHON" - <<'PY'
import socket

with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
    listener.bind(("127.0.0.1", 0))
    print(listener.getsockname()[1])
PY
}

wait_for_port_available() {
  port=$1
  "$PYTHON" - "$port" <<'PY'
import socket
import sys
import time

port = int(sys.argv[1])
for _ in range(200):
    try:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
            listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            listener.bind(("127.0.0.1", port))
        raise SystemExit(0)
    except OSError:
        time.sleep(0.1)
raise SystemExit("port did not become available")
PY
}

wait_for_port_listener() {
  port=$1
  "$PYTHON" - "$port" <<'PY'
import socket
import sys
import time

port = int(sys.argv[1])
for _ in range(200):
    try:
        with socket.create_connection(("127.0.0.1", port), timeout=0.2):
            raise SystemExit(0)
    except OSError:
        time.sleep(0.1)
raise SystemExit("port listener did not become ready")
PY
}

wait_for_health() {
  port=$1
  attempts=0
  while [ "$attempts" -lt 200 ]; do
    if "$CURL" --noproxy '*' --fail --silent --max-time 1 \
      "http://127.0.0.1:$port/healthz" \
      | /usr/bin/grep -q '"ready":true'; then
      return 0
    fi
    attempts=$((attempts + 1))
    /bin/sleep 0.1
  done
  fail "daemon health endpoint did not become ready on port $port"
}

write_config() {
  path=$1
  data_home=$2
  port=$3
  case "$data_home" in
    *'"'*|*'\\'*) fail "temporary data path cannot be represented safely in TOML" ;;
  esac
  cat >"$path" <<EOF
bind = "127.0.0.1:$port"
statePath = "$data_home/threadrelay-state.json"

[outboundProxy]
mode = "direct"
url = ""

[bridge]
enabled = false
accountId = "default"
sendStreaming = false

[logging]
diagnostic = true
maxMb = 5
retentionDays = 1
EOF
}

cat >"$HARNESS_SOURCE" <<'SWIFT'
import Darwin
import Foundation

private enum HarnessFailure: LocalizedError {
    case message(String)

    var errorDescription: String? {
        switch self {
        case let .message(value): value
        }
    }
}

private func runCommand(_ executable: URL, _ arguments: [String]) throws -> CommandResult {
    let process = Process()
    let output = Pipe()
    process.executableURL = executable
    process.arguments = arguments
    process.standardOutput = output
    process.standardError = output
    try process.run()
    process.waitUntilExit()
    let data = output.fileHandleForReading.readDataToEndOfFile()
    return CommandResult(
        exitCode: process.terminationStatus,
        output: String(decoding: data, as: UTF8.self)
    )
}

private func lastLine(_ output: String) -> String {
    output.split(whereSeparator: \.isNewline).last.map(String.init) ?? ""
}

private final class BindFailureInjector: @unchecked Sendable {
    private let launchctl = URL(fileURLWithPath: "/bin/launchctl")
    private let daemonService: String
    private let blockerService: String
    private let domain: String
    private let blockerPlist: String
    private let blockerLog: String
    private let stateLock = NSLock()
    private var daemonBootoutCount = 0

    init(
        daemonService: String,
        blockerService: String,
        domain: String,
        blockerPlist: String,
        blockerLog: String
    ) {
        self.daemonService = daemonService
        self.blockerService = blockerService
        self.domain = domain
        self.blockerPlist = blockerPlist
        self.blockerLog = blockerLog
    }

    func call(_ executable: URL, _ arguments: [String]) throws -> CommandResult {
        let result = try runCommand(executable, arguments)
        guard executable.path == launchctl.path,
              arguments == ["bootout", daemonService]
        else {
            return result
        }

        let printAfterBootout = try runCommand(launchctl, ["print", daemonService])
        guard result.exitCode == 0 || printAfterBootout.exitCode != 0 else {
            return result
        }

        stateLock.lock()
        daemonBootoutCount += 1
        let completedBootouts = daemonBootoutCount
        stateLock.unlock()

        if completedBootouts == 1 {
            try startBlocker()
        } else if completedBootouts == 2 {
            try stopBlocker()
        }
        return result
    }

    func waitForCandidateBindFailures(logURL: URL, minimum: Int) throws {
        for _ in 0..<200 {
            let contents = (try? String(contentsOf: logURL, encoding: .utf8)) ?? ""
            let failures = contents
                .split(whereSeparator: \.isNewline)
                .map { $0.lowercased() }
                .filter {
                    $0.contains("address already in use") || $0.contains("os error 48")
                }
                .count
            if failures >= minimum {
                return
            }
            Thread.sleep(forTimeInterval: 0.1)
        }
        throw HarnessFailure.message(
            "candidate helper did not record \(minimum) bind failures"
        )
    }

    func assertRecoveryInjectionCompleted() throws {
        stateLock.lock()
        let completedBootouts = daemonBootoutCount
        stateLock.unlock()
        guard completedBootouts >= 2 else {
            throw HarnessFailure.message(
                "rollback did not unload the failed candidate service "
                    + "(observed \(completedBootouts) daemon bootouts)"
            )
        }
        let blocker = try runCommand(launchctl, ["print", blockerService])
        guard blocker.exitCode != 0 else {
            throw HarnessFailure.message("port blocker remained loaded after rollback")
        }
    }

    private func startBlocker() throws {
        let bootstrap = try runCommand(
            launchctl,
            ["bootstrap", domain, blockerPlist]
        )
        guard bootstrap.exitCode == 0 else {
            throw HarnessFailure.message(
                "failed to inject port blocker: \(lastLine(bootstrap.output))"
            )
        }
        for _ in 0..<200 {
            let contents = (try? String(contentsOfFile: blockerLog, encoding: .utf8)) ?? ""
            if contents.contains("fault-matrix blocker ready") {
                return
            }
            Thread.sleep(forTimeInterval: 0.05)
        }
        throw HarnessFailure.message("port blocker did not become ready")
    }

    private func stopBlocker() throws {
        let printResult = try runCommand(launchctl, ["print", blockerService])
        if printResult.exitCode == 0 {
            let bootout = try runCommand(launchctl, ["bootout", blockerService])
            let stillLoaded = try runCommand(launchctl, ["print", blockerService])
            guard bootout.exitCode == 0 || stillLoaded.exitCode != 0 else {
                throw HarnessFailure.message(
                    "failed to remove port blocker: \(lastLine(bootout.output))"
                )
            }
        }
        for _ in 0..<100 {
            if try runCommand(launchctl, ["print", blockerService]).exitCode != 0 {
                return
            }
            Thread.sleep(forTimeInterval: 0.05)
        }
        throw HarnessFailure.message("port blocker did not unload")
    }
}

@main
private struct DaemonFaultMatrixHarness {
    static func main() async throws {
        let arguments = Array(CommandLine.arguments.dropFirst())
        guard arguments.count >= 8 else {
            throw HarnessFailure.message("insufficient harness arguments")
        }

        let action = arguments[0]
        let label = arguments[1]
        let helperURL = URL(fileURLWithPath: arguments[2])
        let configURL = URL(fileURLWithPath: arguments[3])
        let launchAgentURL = URL(fileURLWithPath: arguments[4])
        let logURL = URL(fileURLWithPath: arguments[5])
        let homeURL = URL(fileURLWithPath: arguments[6], isDirectory: true)
        let buildIdentifier = arguments[7]
        let configuration = try DaemonLaunchConfiguration(
            testLaunchdLabel: label,
            helperURL: helperURL,
            configURL: configURL,
            launchAgentURL: launchAgentURL,
            logURL: logURL,
            homeURL: homeURL,
            buildIdentifier: buildIdentifier
        )

        if action == "start" {
            let launcher = DaemonLauncher(
                configurationLoader: { configuration },
                commandRunner: runCommand
            )
            try await launcher.startIfNeeded()
            return
        }

        guard action == "switch-and-rollback", arguments.count == 16,
              let expectedPID = Int32(arguments[8])
        else {
            throw HarnessFailure.message("invalid switch-and-rollback arguments")
        }
        let expectedInstanceId = arguments[9]
        let expectedExecutable = arguments[10]
        let injector = BindFailureInjector(
            daemonService: arguments[11],
            blockerService: arguments[12],
            domain: arguments[13],
            blockerPlist: arguments[14],
            blockerLog: arguments[15]
        )
        let launcher = DaemonLauncher(
            configurationLoader: { configuration },
            commandRunner: injector.call
        )
        let transaction = try await launcher.prepareRuntimeSwitch(
            expectedPID: expectedPID,
            expectedInstanceId: expectedInstanceId,
            expectedExecutable: expectedExecutable
        )
        try await launcher.activatePreparedRuntime(
            transaction,
            expectedPID: expectedPID,
            expectedExecutable: expectedExecutable
        )
        try injector.waitForCandidateBindFailures(logURL: logURL, minimum: 2)
        try await launcher.rollbackRuntime(
            transaction,
            expectedPID: nil,
            expectedExecutable: nil
        )
        try injector.assertRecoveryInjectionCompleted()
        try await launcher.commitRuntimeSwitch(transaction)
    }
}
SWIFT

[ -f "$DAEMON_LAUNCHER_SOURCE" ] || fail "DaemonLauncher.swift is unavailable"
"$SWIFTC" \
  -sdk "$MACOS_SDK" \
  -target arm64-apple-macosx13.0 \
  -D DEBUG \
  -parse-as-library \
  "$DAEMON_LAUNCHER_SOURCE" \
  "$HARNESS_SOURCE" \
  -o "$HARNESS_BINARY"

run_harness() {
  /usr/bin/env -i \
    HOME="$TEST_HOME" \
    PATH="/usr/bin:/bin:/usr/sbin:/sbin" \
    TMPDIR="$TEST_ROOT/tmp" \
    "$HARNESS_BINARY" "$@"
}

cat >"$BLOCKER_SCRIPT" <<'PY'
import signal
import socket
import sys

host = sys.argv[1]
port = int(sys.argv[2])
listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
listener.bind((host, port))
listener.listen(8)
print(f"fault-matrix blocker ready {host}:{port}", flush=True)
signal.pause()
PY

write_blocker_plist() {
  port=$1
  "$PYTHON" - \
    "$BLOCKER_PLIST" \
    "$BLOCKER_LABEL" \
    "$PYTHON" \
    "$BLOCKER_SCRIPT" \
    "$port" \
    "$TEST_HOME" \
    "$BLOCKER_LOG" \
    "$TEST_ROOT" <<'PY'
import plistlib
import sys

output, label, python, script, port, home, log, working_directory = sys.argv[1:]
job = {
    "Label": label,
    "ProgramArguments": [python, script, "127.0.0.1", port],
    "EnvironmentVariables": {
        "HOME": home,
        "PATH": "/usr/bin:/bin:/usr/sbin:/sbin",
    },
    "WorkingDirectory": working_directory,
    "RunAtLoad": True,
    "KeepAlive": True,
    "ProcessType": "Background",
    "ThrottleInterval": 1,
    "StandardOutPath": log,
    "StandardErrorPath": log,
}
with open(output, "wb") as stream:
    plistlib.dump(job, stream, fmt=plistlib.FMT_XML, sort_keys=True)
PY
  "$PLUTIL" -lint "$BLOCKER_PLIST" >/dev/null
}

start_blocker() {
  port=$1
  bootout_exact_test_job "$BLOCKER_SERVICE"
  wait_for_job_unloaded "$BLOCKER_SERVICE"
  : >"$BLOCKER_LOG"
  write_blocker_plist "$port"
  bootstrap_test_job "$BLOCKER_SERVICE" "$BLOCKER_PLIST"
  wait_for_port_listener "$port"
}

assert_loaded_program() {
  service=$1
  expected_program=$2
  loaded_program=$(
    "$LAUNCHCTL" print "$service" 2>/dev/null \
      | /usr/bin/sed -n 's/^[[:space:]]*program = //p' \
      | /usr/bin/head -n 1
  )
  [ "$loaded_program" = "$expected_program" ] \
    || fail "loaded program mismatch: expected $expected_program, got ${loaded_program:-none}"
}

crash_test_daemon_job() {
  expected_pid=$1
  assert_loaded_program "$DAEMON_SERVICE" "$PREVIOUS_PROGRAM"
  actual_pid=$(job_pid "$DAEMON_SERVICE" || true)
  [ "$actual_pid" = "$expected_pid" ] \
    || fail "test daemon pid changed before KeepAlive signal: $expected_pid -> ${actual_pid:-none}"
  "$LAUNCHCTL" kill SIGKILL "$DAEMON_SERVICE" >/dev/null \
    || fail "unable to signal the isolated daemon service"
}

fetch_and_assert_lifecycle() {
  port=$1
  expected_pid=$2
  expected_program=$3
  expected_state=$4
  control_file="$THREADRELAY_TEST_HOME/threadrelay-control.json"
  attempts=0
  while [ ! -f "$control_file" ] && [ "$attempts" -lt 100 ]; do
    attempts=$((attempts + 1))
    /bin/sleep 0.1
  done
  [ -f "$control_file" ] || fail "management control file was not created"
  token=$(
    "$PYTHON" - "$control_file" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as stream:
    print(json.load(stream)["managementToken"])
PY
  )
  "$CURL" --noproxy '*' --fail --silent --max-time 2 \
    -H "Authorization: Bearer $token" \
    "http://127.0.0.1:$port/api/v1/manage/lifecycle" \
    >"$LIFECYCLE_JSON"
  "$PYTHON" - \
    "$LIFECYCLE_JSON" \
    "$expected_pid" \
    "$expected_program" \
    "$expected_state" <<'PY'
import json
import os
import sys

path, expected_pid, expected_program, expected_state = sys.argv[1:]
with open(path, encoding="utf-8") as stream:
    lifecycle = json.load(stream)
actual_pid = lifecycle["service"]["pid"]
if actual_pid != int(expected_pid):
    raise SystemExit(f"lifecycle pid mismatch: {actual_pid} != {expected_pid}")
actual_program = os.path.realpath(lifecycle["executable"])
if actual_program != os.path.realpath(expected_program):
    raise SystemExit(f"lifecycle executable mismatch: {actual_program} != {expected_program}")
actual_state = lifecycle["runtime"]["state"]
if actual_state != expected_state:
    raise SystemExit(f"lifecycle runtime state mismatch: {actual_state} != {expected_state}")
instance_id = lifecycle["service"]["instanceId"]
if not instance_id:
    raise SystemExit("lifecycle instance id is empty")
print(instance_id)
PY
}

wait_for_bind_failures() {
  log_path=$1
  minimum=$2
  attempts=0
  while [ "$attempts" -lt 200 ]; do
    count=$(
      /usr/bin/grep -Eic \
        'address already in use|os error 48' \
        "$log_path" 2>/dev/null \
        || true
    )
    if [ "$count" -ge "$minimum" ]; then
      return 0
    fi
    attempts=$((attempts + 1))
    /bin/sleep 0.1
  done
  fail "daemon did not record $minimum bind failures in $log_path"
}

note "isolated root: $TEST_ROOT"
note "daemon label: $DAEMON_LABEL"
note "blocker label: $BLOCKER_LABEL"

note "case 1/3: occupied port fences a KeepAlive daemon start"
OCCUPIED_PORT=$(allocate_port)
write_config "$OCCUPIED_CONFIG_PATH" "$OCCUPIED_TEST_HOME" "$OCCUPIED_PORT"
start_blocker "$OCCUPIED_PORT"
: >"$DIRECT_FAILURE_LOG"
run_harness \
  start \
  "$DAEMON_LABEL" \
  "$HELPER_COPY" \
  "$OCCUPIED_CONFIG_PATH" \
  "$DAEMON_PLIST" \
  "$DIRECT_FAILURE_LOG" \
  "$TEST_HOME" \
  "$BUILD_ID"
assert_loaded_program "$DAEMON_SERVICE" "$OCCUPIED_PROGRAM"
wait_for_run_count "$DAEMON_SERVICE" 2
wait_for_bind_failures "$DIRECT_FAILURE_LOG" 2
[ ! -e "$ACTIVE_LOCATOR" ] \
  || fail "occupied-port failure unexpectedly published an active daemon locator"
bootout_exact_test_job "$DAEMON_SERVICE"
wait_for_job_unloaded "$DAEMON_SERVICE"
bootout_exact_test_job "$BLOCKER_SERVICE"
wait_for_job_unloaded "$BLOCKER_SERVICE"
wait_for_port_available "$OCCUPIED_PORT"

note "case 2/3: KeepAlive replaces the daemon pid three times after SIGKILL"
DAEMON_PORT=$(allocate_port)
write_config "$CONFIG_PATH" "$THREADRELAY_TEST_HOME" "$DAEMON_PORT"
: >"$DAEMON_LOG"
run_harness \
  start \
  "$DAEMON_LABEL" \
  "$HELPER_COPY" \
  "$CONFIG_PATH" \
  "$DAEMON_PLIST" \
  "$DAEMON_LOG" \
  "$TEST_HOME" \
  "$BUILD_ID"
wait_for_health "$DAEMON_PORT"
CURRENT_PID=$(wait_for_job_pid "$DAEMON_SERVICE")
assert_loaded_program "$DAEMON_SERVICE" "$PREVIOUS_PROGRAM"
CURRENT_INSTANCE=$(fetch_and_assert_lifecycle \
  "$DAEMON_PORT" "$CURRENT_PID" "$PREVIOUS_PROGRAM" active)

cycle=1
while [ "$cycle" -le 3 ]; do
  crash_test_daemon_job "$CURRENT_PID"
  REPLACEMENT_PID=$(wait_for_job_pid "$DAEMON_SERVICE" "$CURRENT_PID")
  wait_for_health "$DAEMON_PORT"
  REPLACEMENT_INSTANCE=$(fetch_and_assert_lifecycle \
    "$DAEMON_PORT" \
    "$REPLACEMENT_PID" \
    "$PREVIOUS_PROGRAM" \
    active)
  [ "$REPLACEMENT_INSTANCE" != "$CURRENT_INSTANCE" ] \
    || fail "KeepAlive cycle $cycle reused daemon instance $CURRENT_INSTANCE"
  note "KeepAlive cycle $cycle replaced pid $CURRENT_PID with $REPLACEMENT_PID"
  CURRENT_PID=$REPLACEMENT_PID
  CURRENT_INSTANCE=$REPLACEMENT_INSTANCE
  cycle=$((cycle + 1))
done

note "case 3/3: DaemonLauncher rolls a bind-failing candidate back"
bootout_exact_test_job "$BLOCKER_SERVICE"
wait_for_job_unloaded "$BLOCKER_SERVICE"
: >"$DAEMON_LOG"
: >"$BLOCKER_LOG"
write_blocker_plist "$DAEMON_PORT"
run_harness \
  switch-and-rollback \
  "$DAEMON_LABEL" \
  "$HELPER_COPY" \
  "$CONFIG_PATH" \
  "$DAEMON_PLIST" \
  "$DAEMON_LOG" \
  "$TEST_HOME" \
  "$BUILD_ID" \
  "$CURRENT_PID" \
  "$CURRENT_INSTANCE" \
  "$PREVIOUS_PROGRAM" \
  "$DAEMON_SERVICE" \
  "$BLOCKER_SERVICE" \
  "$DOMAIN" \
  "$BLOCKER_PLIST" \
  "$BLOCKER_LOG"
wait_for_health "$DAEMON_PORT"
RECOVERED_PID=$(wait_for_job_pid "$DAEMON_SERVICE")
assert_loaded_program "$DAEMON_SERVICE" "$PREVIOUS_PROGRAM"
RECOVERED_INSTANCE=$(fetch_and_assert_lifecycle \
  "$DAEMON_PORT" \
  "$RECOVERED_PID" \
  "$PREVIOUS_PROGRAM" \
  active)
[ "$RECOVERED_INSTANCE" != "$CURRENT_INSTANCE" ] \
  || fail "rollback reused the previous daemon instance $CURRENT_INSTANCE"
[ ! -e "$THREADRELAY_TEST_HOME/threadrelay-runtime-switch.json" ] \
  || fail "successful rollback left the runtime-switch journal behind"
[ ! -e "$ACTIVE_LOCATOR" ] \
  || /usr/bin/grep -q "$RECOVERED_INSTANCE" "$ACTIVE_LOCATOR" \
  || fail "active daemon locator does not describe the recovered instance"

verify_formal_unchanged \
  || fail "production daemon state changed during the isolated matrix"

note "all isolated macOS daemon fault cases passed"
