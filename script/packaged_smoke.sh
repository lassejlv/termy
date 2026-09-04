#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_BUNDLE="$ROOT_DIR/dist/Tmon.app"
ARCHIVE=""
OUTPUT="$ROOT_DIR/release/evidence/packaged-smoke-$(date +%s).json"
ALLOW_ADHOC=0

while (( $# > 0 )); do
  case "$1" in
    --app)
      APP_BUNDLE="${2:?--app requires a Tmon.app path}"
      shift 2
      ;;
    --archive)
      ARCHIVE="${2:?--archive requires a zip path}"
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
      echo "usage: $0 [--app Tmon.app | --archive Tmon.zip] [--output PATH] [--allow-adhoc]" >&2
      exit 2
      ;;
  esac
done

if [[ -n "$ARCHIVE" && ! -f "$ARCHIVE" ]]; then
  echo "archive does not exist: $ARCHIVE" >&2
  exit 1
fi
if [[ -z "$ARCHIVE" && ! -d "$APP_BUNDLE" ]]; then
  echo "app bundle does not exist: $APP_BUNDLE" >&2
  exit 1
fi
if [[ -e "$OUTPUT" ]]; then
  echo "refusing to overwrite smoke evidence: $OUTPUT" >&2
  exit 1
fi

WORK_DIR="$(mktemp -d /tmp/tmon-packaged-smoke.XXXXXX)"
TEST_HOME="$WORK_DIR/home"
EXTRACTED="$WORK_DIR/extracted"
DOWNLOADED="$WORK_DIR/downloaded"
FIRST_MARKER="$WORK_DIR/first-session-started"
SECOND_MARKER="$WORK_DIR/replacement-session-started"
CHILD_PID_FILE="$WORK_DIR/child.pid"
SUPPORT_REPORT="$WORK_DIR/support.json"
FIRST_APP_PID=""
SECOND_APP_PID=""
TEST_BINARY=""
TEST_SOCKET="$TEST_HOME/Library/Application Support/Tmon/runtime/multiplexer-v3.sock"

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

cleanup() {
  terminate_app "$FIRST_APP_PID"
  terminate_app "$SECOND_APP_PID"
  if [[ -x "$TEST_BINARY" ]]; then
    env HOME="$TEST_HOME" TMON_CONFIG="$TEST_HOME/config.toml" \
      "$TEST_BINARY" --terminate-sessions >/dev/null 2>&1 || true
  fi
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
  echo "timed out waiting for packaged smoke path: $path" >&2
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

mkdir -p "$TEST_HOME" "$EXTRACTED" "$DOWNLOADED"
chmod 700 "$TEST_HOME"
: > "$TEST_HOME/config.toml"

ARCHIVE_CHECKSUM=""
if [[ -n "$ARCHIVE" ]]; then
  ARCHIVE_NAME="$(basename "$ARCHIVE")"
  CHECKSUM_SOURCE="$ARCHIVE.sha256"
  if [[ ! -f "$CHECKSUM_SOURCE" ]]; then
    echo "missing adjacent checksum file: $CHECKSUM_SOURCE" >&2
    exit 1
  fi
  cp "$ARCHIVE" "$DOWNLOADED/$ARCHIVE_NAME"
  cp "$CHECKSUM_SOURCE" "$DOWNLOADED/$ARCHIVE_NAME.sha256"
  (
    cd "$DOWNLOADED"
    shasum -a 256 -c "$ARCHIVE_NAME.sha256"
  )
  ARCHIVE_CHECKSUM="$(shasum -a 256 "$DOWNLOADED/$ARCHIVE_NAME" | awk '{print $1}')"
  /usr/bin/xattr -w com.apple.quarantine "0081;$(date +%x);TmonPackagedSmoke;" \
    "$DOWNLOADED/$ARCHIVE_NAME"
  /usr/bin/ditto -x -k "$DOWNLOADED/$ARCHIVE_NAME" "$EXTRACTED"
  APP_BUNDLE="$EXTRACTED/Tmon.app"
else
  /usr/bin/ditto "$APP_BUNDLE" "$EXTRACTED/Tmon.app"
  APP_BUNDLE="$EXTRACTED/Tmon.app"
fi

TEST_BINARY="$APP_BUNDLE/Contents/MacOS/tmon"
INFO_PLIST="$APP_BUNDLE/Contents/Info.plist"
VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$INFO_PLIST")"
BUILD_NUMBER="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$INFO_PLIST")"
BUNDLE_IDENTIFIER="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$INFO_PLIST")"
TMON_EXPECTED_VERSION="$VERSION" \
TMON_BUILD_NUMBER="$BUILD_NUMBER" \
TMON_BUNDLE_IDENTIFIER="$BUNDLE_IDENTIFIER" \
  "$ROOT_DIR/script/verify_macos_bundle.sh" "$APP_BUNDLE"

SIGNING_DETAILS="$(codesign -dvvv "$APP_BUNDLE" 2>&1)"
if [[ "$SIGNING_DETAILS" == *"Signature=adhoc"* ]]; then
  SIGNATURE_KIND="ad_hoc"
  DISTRIBUTION_READY=false
  if (( ALLOW_ADHOC == 0 )); then
    echo "the package is ad hoc signed; pass --allow-adhoc only for internal runtime smoke" >&2
    exit 1
  fi
  /usr/bin/xattr -dr com.apple.quarantine "$APP_BUNDLE" 2>/dev/null || true
else
  SIGNATURE_KIND="developer_id_application"
  DISTRIBUTION_READY=true
  /usr/bin/xattr -w com.apple.quarantine "0081;$(date +%x);TmonPackagedSmoke;" "$APP_BUNDLE"
  /usr/sbin/spctl --assess --type execute --verbose=4 "$APP_BUNDLE"
fi

LAUNCH_ENV=(
  HOME="$TEST_HOME"
  PATH="/usr/bin:/bin:/usr/sbin:/sbin"
  SHELL="/bin/zsh"
  TMON_CONFIG="$TEST_HOME/config.toml"
  TMON_SMOKE_MARKER="$FIRST_MARKER"
  TMON_SMOKE_PID="$CHILD_PID_FILE"
)
env -i "${LAUNCH_ENV[@]}" "$TEST_BINARY" /bin/sh -c \
  'trap "exit 0" HUP TERM INT; printf "started\n" > "$TMON_SMOKE_MARKER"; printf "%s\n" "$$" > "$TMON_SMOKE_PID"; while :; do sleep 1; done' &
FIRST_APP_PID=$!
wait_for_path "$FIRST_MARKER"
wait_for_path "$TEST_SOCKET"

STATUS="$(env HOME="$TEST_HOME" TMON_CONFIG="$TEST_HOME/config.toml" \
  "$TEST_BINARY" --session-status)"
if [[ "$STATUS" != *"protocol v3 (current): running"* ]]; then
  echo "packaged binary did not report its isolated daemon as running" >&2
  exit 1
fi

env HOME="$TEST_HOME" TMON_CONFIG="$TEST_HOME/config.toml" \
  "$TEST_BINARY" --support-bundle "$SUPPORT_REPORT"
if rg -Fq "$TEST_HOME" "$SUPPORT_REPORT" \
  || rg -Fq "$WORK_DIR" "$SUPPORT_REPORT" \
  || rg -Fq "terminal text" "$SUPPORT_REPORT" \
  || rg -Fq "started" "$SUPPORT_REPORT"; then
  echo "support bundle included test-private content or paths" >&2
  exit 1
fi

CHILD_PID="$(<"$CHILD_PID_FILE")"
if ! process_is_live "$CHILD_PID"; then
  echo "packaged PTY child did not remain alive" >&2
  exit 1
fi

terminate_app "$FIRST_APP_PID"
FIRST_APP_PID=""
if [[ ! -S "$TEST_SOCKET" ]] || ! process_is_live "$CHILD_PID"; then
  echo "session did not survive packaged app detachment" >&2
  exit 1
fi

LAUNCH_ENV[4]="TMON_SMOKE_MARKER=$SECOND_MARKER"
env -i "${LAUNCH_ENV[@]}" "$TEST_BINARY" /bin/sh -c \
  'printf "replacement-started\n" > "$TMON_SMOKE_MARKER"; sleep 60' &
SECOND_APP_PID=$!
sleep 2
if ! kill -0 "$SECOND_APP_PID" 2>/dev/null; then
  echo "packaged app did not reattach to the isolated daemon" >&2
  exit 1
fi
if [[ -e "$SECOND_MARKER" ]]; then
  echo "reattach unexpectedly spawned a duplicate PTY session" >&2
  exit 1
fi
terminate_app "$SECOND_APP_PID"
SECOND_APP_PID=""

env HOME="$TEST_HOME" TMON_CONFIG="$TEST_HOME/config.toml" \
  "$TEST_BINARY" --terminate-sessions
for _ in {1..100}; do
  if [[ ! -e "$TEST_SOCKET" ]] && ! process_is_live "$CHILD_PID"; then
    break
  fi
  sleep 0.1
done
if [[ -e "$TEST_SOCKET" ]] || process_is_live "$CHILD_PID"; then
  echo "explicit isolated session termination left the PTY child alive" >&2
  exit 1
fi

MACOS_VERSION="$(sw_vers -productVersion)"
HOST_ARCH="$(uname -m)"
mkdir -p "$(dirname "$OUTPUT")"
umask 077
printf '{\n' > "$OUTPUT"
printf '  "schema_version": 1,\n' >> "$OUTPUT"
printf '  "generated_unix_seconds": %s,\n' "$(date +%s)" >> "$OUTPUT"
printf '  "application_version": "%s",\n' "$VERSION" >> "$OUTPUT"
printf '  "bundle_build_number": "%s",\n' "$BUILD_NUMBER" >> "$OUTPUT"
printf '  "bundle_identifier": "%s",\n' "$BUNDLE_IDENTIFIER" >> "$OUTPUT"
printf '  "macos_version": "%s",\n' "$MACOS_VERSION" >> "$OUTPUT"
printf '  "host_architecture": "%s",\n' "$HOST_ARCH" >> "$OUTPUT"
printf '  "signature_kind": "%s",\n' "$SIGNATURE_KIND" >> "$OUTPUT"
printf '  "distribution_ready": %s,\n' "$DISTRIBUTION_READY" >> "$OUTPUT"
if [[ -n "$ARCHIVE_CHECKSUM" ]]; then
  printf '  "archive_sha256": "%s",\n' "$ARCHIVE_CHECKSUM" >> "$OUTPUT"
else
  printf '  "archive_sha256": null,\n' >> "$OUTPUT"
fi
printf '  "bundle_verified": true,\n' >> "$OUTPUT"
printf '  "isolated_pty_started": true,\n' >> "$OUTPUT"
printf '  "session_survived_detach": true,\n' >> "$OUTPUT"
printf '  "reattached_without_duplicate_session": true,\n' >> "$OUTPUT"
printf '  "support_bundle_privacy_smoke": true,\n' >> "$OUTPUT"
printf '  "explicit_session_termination": true,\n' >> "$OUTPUT"
printf '  "automated_runtime_passed": true,\n' >> "$OUTPUT"
printf '  "visual_matrix_status": "manual_checks_required"\n' >> "$OUTPUT"
printf '}\n' >> "$OUTPUT"

echo "packaged smoke evidence: $OUTPUT"
echo "automated packaged runtime: pass"
if [[ "$DISTRIBUTION_READY" == false ]]; then
  echo "distribution: internal only (ad hoc signature and quarantine launch not accepted)"
fi
