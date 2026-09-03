#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_BUNDLE="${1:-$ROOT_DIR/dist/Tmon.app}"
INFO_PLIST="$APP_BUNDLE/Contents/Info.plist"
APP_BINARY="$APP_BUNDLE/Contents/MacOS/tmon"

if [[ ! -x "$APP_BINARY" ]]; then
  echo "missing executable: $APP_BINARY" >&2
  exit 1
fi

plutil -lint "$INFO_PLIST"
codesign --verify --deep --strict --verbose=2 "$APP_BUNDLE"
lipo -info "$APP_BINARY"

SIGNING_DETAILS="$(codesign -dvvv --entitlements - "$APP_BUNDLE" 2>&1)"
printf '%s\n' "$SIGNING_DETAILS"
if [[ "$SIGNING_DETAILS" == *"Signature=adhoc"* ]]; then
  echo "signing: ad hoc local build; Developer ID signing and notarization are still required for distribution"
else
  if spctl --assess --type execute --verbose=4 "$APP_BUNDLE"; then
    echo "gatekeeper: accepted"
  else
    echo "gatekeeper: Developer ID signature is valid, but notarization may still be pending"
  fi
fi
