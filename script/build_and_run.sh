#!/usr/bin/env bash
set -euo pipefail

MODE="${1:-run}"
APP_NAME="tmon"
DISPLAY_NAME="Tmon"
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_BUNDLE="$ROOT_DIR/dist/$DISPLAY_NAME.app"

if (( $# > 0 )); then
  shift
fi

pkill -x "$APP_NAME" >/dev/null 2>&1 || true

case "$MODE" in
  --debug|debug)
    cargo build --manifest-path "$ROOT_DIR/Cargo.toml" -p tmon
    exec lldb -- "$ROOT_DIR/target/debug/$APP_NAME" "$@"
    ;;
  run|--logs|logs|--telemetry|telemetry|--verify|verify)
    "$ROOT_DIR/script/package_macos.sh" --native --no-archive
    ;;
  *)
    echo "usage: $0 [run|--debug|--logs|--telemetry|--verify]" >&2
    exit 2
    ;;
esac

open_app() {
  if (( $# > 0 )); then
    /usr/bin/open -n "$APP_BUNDLE" --args "$@"
  else
    /usr/bin/open -n "$APP_BUNDLE"
  fi
}

case "$MODE" in
  run)
    open_app "$@"
    ;;
  --logs|logs|--telemetry|telemetry)
    open_app "$@"
    exec /usr/bin/log stream --info --style compact --predicate "process == \"$APP_NAME\""
    ;;
  --verify|verify)
    open_app "$@"
    sleep 1
    APP_PID="$(pgrep -x "$APP_NAME" | head -n 1)"
    kill -0 "$APP_PID"
    echo "$APP_NAME is running (pid $APP_PID)"
    ;;
esac
