#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PREVIOUS_APP=""
CURRENT_APP="$ROOT_DIR/dist/Tmon.app"
PREVIOUS_PROTOCOL=2
CURRENT_PROTOCOL=3
OUTPUT="$ROOT_DIR/release/evidence/upgrade-rollback-$(date +%s).json"
ALLOW_ADHOC=0

while (( $# > 0 )); do
  case "$1" in
    --previous-app)
      PREVIOUS_APP="${2:?--previous-app requires an app path}"
      shift 2
      ;;
    --current-app)
      CURRENT_APP="${2:?--current-app requires an app path}"
      shift 2
      ;;
    --previous-protocol)
      PREVIOUS_PROTOCOL="${2:?--previous-protocol requires an integer}"
      shift 2
      ;;
    --current-protocol)
      CURRENT_PROTOCOL="${2:?--current-protocol requires an integer}"
      shift 2
      ;;
    --output)
      OUTPUT="${2:?--output requires a new JSON path}"
      shift 2
      ;;
    --allow-adhoc)
      ALLOW_ADHOC=1
      shift
      ;;
    *)
      echo "usage: $0 --previous-app Tmon-N-1.app [--current-app Tmon.app] [--previous-protocol N] [--current-protocol N] [--output PATH] [--allow-adhoc]" >&2
      exit 2
      ;;
  esac
done

if [[ -z "$PREVIOUS_APP" || ! -d "$PREVIOUS_APP" ]]; then
  echo "--previous-app must name an existing Tmon app" >&2
  exit 1
fi
if [[ ! -d "$CURRENT_APP" ]]; then
  echo "current app does not exist: $CURRENT_APP" >&2
  exit 1
fi
if [[ ! "$PREVIOUS_PROTOCOL" =~ ^[1-9][0-9]*$ || ! "$CURRENT_PROTOCOL" =~ ^[1-9][0-9]*$ ]]; then
  echo "protocol versions must be positive integers" >&2
  exit 2
fi
if (( PREVIOUS_PROTOCOL >= CURRENT_PROTOCOL )); then
  echo "previous protocol must be lower than current protocol" >&2
  exit 2
fi
if [[ -e "$OUTPUT" ]]; then
  echo "refusing to overwrite upgrade evidence: $OUTPUT" >&2
  exit 1
fi

PREVIOUS_BINARY="$PREVIOUS_APP/Contents/MacOS/tmon"
CURRENT_BINARY="$CURRENT_APP/Contents/MacOS/tmon"
for binary in "$PREVIOUS_BINARY" "$CURRENT_BINARY"; do
  if [[ ! -x "$binary" ]]; then
    echo "missing packaged executable: $binary" >&2
    exit 1
  fi
done

PREVIOUS_PLIST="$PREVIOUS_APP/Contents/Info.plist"
CURRENT_PLIST="$CURRENT_APP/Contents/Info.plist"
PREVIOUS_ID="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$PREVIOUS_PLIST")"
CURRENT_ID="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$CURRENT_PLIST")"
if [[ "$PREVIOUS_ID" != "$CURRENT_ID" ]]; then
  echo "N-1 and N bundle identifiers differ" >&2
  exit 1
fi
PREVIOUS_VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$PREVIOUS_PLIST")"
CURRENT_VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$CURRENT_PLIST")"
PREVIOUS_BUILD="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$PREVIOUS_PLIST")"
CURRENT_BUILD="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$CURRENT_PLIST")"

signature_kind() {
  local details
  details="$(codesign -dvvv "$1" 2>&1)"
  if [[ "$details" == *"Signature=adhoc"* ]]; then
    printf 'ad_hoc'
  elif [[ "$details" == *"Authority=Developer ID Application:"* ]]; then
    printf 'developer_id_application'
  else
    printf 'other'
  fi
}

PREVIOUS_SIGNATURE="$(signature_kind "$PREVIOUS_APP")"
CURRENT_SIGNATURE="$(signature_kind "$CURRENT_APP")"
SIGNED_PAIR=true
if [[ "$PREVIOUS_SIGNATURE" != developer_id_application || "$CURRENT_SIGNATURE" != developer_id_application ]]; then
  SIGNED_PAIR=false
  if (( ALLOW_ADHOC == 0 )); then
    echo "both upgrade apps must be Developer ID signed; use --allow-adhoc for internal protocol evidence only" >&2
    exit 1
  fi
else
  /usr/sbin/spctl --assess --type execute --verbose=4 "$PREVIOUS_APP"
  /usr/sbin/spctl --assess --type execute --verbose=4 "$CURRENT_APP"
fi

WORK_DIR="$(mktemp -d /tmp/tmon-upgrade-rollback.XXXXXX)"
TEST_HOME="$WORK_DIR/home"
CONFIG="$TEST_HOME/config.toml"
RUNTIME="$TEST_HOME/Library/Application Support/Tmon/runtime"
PREVIOUS_SOCKET="$RUNTIME/multiplexer-v$PREVIOUS_PROTOCOL.sock"
CURRENT_SOCKET="$RUNTIME/multiplexer-v$CURRENT_PROTOCOL.sock"
PREVIOUS_MARKER="$WORK_DIR/previous-started"
CURRENT_MARKER="$WORK_DIR/current-started"
ROLLBACK_MARKER="$WORK_DIR/rollback-spawned-duplicate"
PREVIOUS_CHILD_PID_FILE="$WORK_DIR/previous-child.pid"
CURRENT_CHILD_PID_FILE="$WORK_DIR/current-child.pid"
PREVIOUS_APP_PID=""
CURRENT_APP_PID=""
ROLLBACK_APP_PID=""

terminate_app() {
  local pid="$1"
  if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
    kill -TERM "$pid" 2>/dev/null || true
    for _ in {1..50}; do
      if ! kill -0 "$pid" 2>/dev/null; then
        wait "$pid" 2>/dev/null || true
        return
      fi
      sleep 0.1
    done
    kill -KILL "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  fi
}

stop_isolated_daemon() {
  local socket="$1"
  if [[ ! -S "$socket" ]]; then
    return
  fi
  local daemon_pids
  daemon_pids="$(/usr/sbin/lsof -t "$socket" 2>/dev/null | sort -u || true)"
  while IFS= read -r pid; do
    if [[ "$pid" =~ ^[1-9][0-9]*$ ]]; then
      kill -TERM "$pid" 2>/dev/null || true
    fi
  done <<< "$daemon_pids"
}

cleanup() {
  terminate_app "$PREVIOUS_APP_PID"
  terminate_app "$CURRENT_APP_PID"
  terminate_app "$ROLLBACK_APP_PID"
  env HOME="$TEST_HOME" TMON_CONFIG="$CONFIG" "$CURRENT_BINARY" --terminate-sessions >/dev/null 2>&1 || true
  stop_isolated_daemon "$PREVIOUS_SOCKET"
  for pid_file in "$PREVIOUS_CHILD_PID_FILE" "$CURRENT_CHILD_PID_FILE"; do
    if [[ -f "$pid_file" ]]; then
      local child_pid
      child_pid="$(<"$pid_file")"
      if [[ "$child_pid" =~ ^[1-9][0-9]*$ ]]; then
        kill -TERM "$child_pid" 2>/dev/null || true
      fi
    fi
  done
  rm -rf "$WORK_DIR"
}
trap cleanup EXIT

wait_for_path() {
  local path="$1"
  for _ in {1..100}; do
    if [[ -e "$path" ]]; then
      return 0
    fi
    sleep 0.1
  done
  echo "timed out waiting for isolated upgrade path: $path" >&2
  return 1
}

process_is_live() {
  local pid="$1"
  if [[ ! "$pid" =~ ^[1-9][0-9]*$ ]] || ! kill -0 "$pid" 2>/dev/null; then
    return 1
  fi
  local state
  state="$(/bin/ps -o stat= -p "$pid" 2>/dev/null | tr -d '[:space:]')"
  [[ -n "$state" && "$state" != Z* ]]
}

launch_app() {
  local binary="$1"
  local marker="$2"
  local child_pid_file="$3"
  env -i HOME="$TEST_HOME" PATH="/usr/bin:/bin:/usr/sbin:/sbin" SHELL="/bin/zsh" TMON_CONFIG="$CONFIG" TMON_UPGRADE_MARKER="$marker" TMON_UPGRADE_CHILD_PID="$child_pid_file" "$binary" /bin/sh -c 'trap "exit 0" HUP TERM INT; printf "started\n" > "$TMON_UPGRADE_MARKER"; printf "%s\n" "$$" > "$TMON_UPGRADE_CHILD_PID"; while :; do sleep 1; done' &
  LAUNCHED_PID=$!
}

mkdir -p "$TEST_HOME"
chmod 700 "$TEST_HOME"
: > "$CONFIG"

launch_app "$PREVIOUS_BINARY" "$PREVIOUS_MARKER" "$PREVIOUS_CHILD_PID_FILE"
PREVIOUS_APP_PID="$LAUNCHED_PID"
wait_for_path "$PREVIOUS_MARKER"
wait_for_path "$PREVIOUS_SOCKET"
PREVIOUS_CHILD_PID="$(<"$PREVIOUS_CHILD_PID_FILE")"
terminate_app "$PREVIOUS_APP_PID"
PREVIOUS_APP_PID=""
if ! process_is_live "$PREVIOUS_CHILD_PID"; then
  echo "N-1 session did not survive app detachment" >&2
  exit 1
fi

launch_app "$CURRENT_BINARY" "$CURRENT_MARKER" "$CURRENT_CHILD_PID_FILE"
CURRENT_APP_PID="$LAUNCHED_PID"
wait_for_path "$CURRENT_MARKER"
wait_for_path "$CURRENT_SOCKET"
CURRENT_CHILD_PID="$(<"$CURRENT_CHILD_PID_FILE")"
terminate_app "$CURRENT_APP_PID"
CURRENT_APP_PID=""

STATUS="$(env HOME="$TEST_HOME" TMON_CONFIG="$CONFIG" "$CURRENT_BINARY" --session-status)"
if [[ "$STATUS" != *"protocol v$PREVIOUS_PROTOCOL (older): running"* ]]; then
  echo "current app did not discover the older isolated protocol generation" >&2
  exit 1
fi
if [[ "$STATUS" != *"protocol v$CURRENT_PROTOCOL (current): running"* ]]; then
  echo "current app did not discover both isolated protocol generations" >&2
  exit 1
fi
if ! process_is_live "$PREVIOUS_CHILD_PID" || ! process_is_live "$CURRENT_CHILD_PID"; then
  echo "a live generation was lost during upgrade coexistence" >&2
  exit 1
fi

launch_app "$PREVIOUS_BINARY" "$ROLLBACK_MARKER" "$WORK_DIR/rollback-child.pid"
ROLLBACK_APP_PID="$LAUNCHED_PID"
sleep 2
if ! kill -0 "$ROLLBACK_APP_PID" 2>/dev/null; then
  echo "N-1 app did not reattach during rollback" >&2
  exit 1
fi
if [[ -e "$ROLLBACK_MARKER" ]]; then
  echo "rollback spawned a duplicate N-1 PTY instead of reattaching" >&2
  exit 1
fi
if ! process_is_live "$PREVIOUS_CHILD_PID" || ! process_is_live "$CURRENT_CHILD_PID"; then
  echo "rollback disturbed a live protocol generation" >&2
  exit 1
fi
terminate_app "$ROLLBACK_APP_PID"
ROLLBACK_APP_PID=""

env HOME="$TEST_HOME" TMON_CONFIG="$CONFIG" "$CURRENT_BINARY" --terminate-sessions
for _ in {1..100}; do
  if [[ ! -e "$CURRENT_SOCKET" ]] && ! process_is_live "$CURRENT_CHILD_PID"; then
    break
  fi
  sleep 0.1
done
if [[ -e "$CURRENT_SOCKET" ]] || process_is_live "$CURRENT_CHILD_PID"; then
  echo "current generation did not terminate explicitly and independently" >&2
  exit 1
fi
if [[ ! -S "$PREVIOUS_SOCKET" ]] || ! process_is_live "$PREVIOUS_CHILD_PID"; then
  echo "terminating current sessions disturbed the N-1 generation" >&2
  exit 1
fi

stop_isolated_daemon "$PREVIOUS_SOCKET"
for _ in {1..100}; do
  if [[ ! -e "$PREVIOUS_SOCKET" ]]; then
    break
  fi
  sleep 0.1
done

VERSIONS_DISTINCT=true
if [[ "$PREVIOUS_VERSION" == "$CURRENT_VERSION" && "$PREVIOUS_BUILD" == "$CURRENT_BUILD" ]]; then
  VERSIONS_DISTINCT=false
fi
PRODUCTION_PAIR=false
if [[ "$SIGNED_PAIR" == true && "$VERSIONS_DISTINCT" == true ]]; then
  PRODUCTION_PAIR=true
fi

mkdir -p "$(dirname "$OUTPUT")"
umask 077
printf '{\n' > "$OUTPUT"
printf '  "schema_version": 1,\n' >> "$OUTPUT"
printf '  "generated_unix_seconds": %s,\n' "$(date +%s)" >> "$OUTPUT"
printf '  "bundle_identifier": "%s",\n' "$CURRENT_ID" >> "$OUTPUT"
printf '  "previous_version": "%s",\n' "$PREVIOUS_VERSION" >> "$OUTPUT"
printf '  "previous_build": "%s",\n' "$PREVIOUS_BUILD" >> "$OUTPUT"
printf '  "current_version": "%s",\n' "$CURRENT_VERSION" >> "$OUTPUT"
printf '  "current_build": "%s",\n' "$CURRENT_BUILD" >> "$OUTPUT"
printf '  "previous_protocol": %s,\n' "$PREVIOUS_PROTOCOL" >> "$OUTPUT"
printf '  "current_protocol": %s,\n' "$CURRENT_PROTOCOL" >> "$OUTPUT"
printf '  "previous_signature": "%s",\n' "$PREVIOUS_SIGNATURE" >> "$OUTPUT"
printf '  "current_signature": "%s",\n' "$CURRENT_SIGNATURE" >> "$OUTPUT"
printf '  "release_versions_distinct": %s,\n' "$VERSIONS_DISTINCT" >> "$OUTPUT"
printf '  "signed_release_pair": %s,\n' "$SIGNED_PAIR" >> "$OUTPUT"
printf '  "previous_session_survived_upgrade": true,\n' >> "$OUTPUT"
printf '  "generations_coexisted": true,\n' >> "$OUTPUT"
printf '  "rollback_reattached_without_duplicate": true,\n' >> "$OUTPUT"
printf '  "current_termination_left_previous_alive": true,\n' >> "$OUTPUT"
printf '  "protocol_rollback_passed": true,\n' >> "$OUTPUT"
printf '  "production_upgrade_rollback_passed": %s\n' "$PRODUCTION_PAIR" >> "$OUTPUT"
printf '}\n' >> "$OUTPUT"

echo "upgrade/rollback evidence: $OUTPUT"
echo "protocol coexistence and rollback: pass"
if [[ "$PRODUCTION_PAIR" == false ]]; then
  echo "production release pair: unproven (requires distinct Developer ID signed N-1 and N apps)"
fi
