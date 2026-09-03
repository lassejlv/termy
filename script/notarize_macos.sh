#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_BUNDLE="$ROOT_DIR/dist/Tmon.app"
NOTARY_PROFILE="${APPLE_NOTARY_PROFILE:-}"

if [[ -z "$NOTARY_PROFILE" ]]; then
  echo "APPLE_NOTARY_PROFILE must name a notarytool keychain profile" >&2
  exit 2
fi
if [[ ! -d "$APP_BUNDLE" ]]; then
  echo "package Tmon before notarizing it" >&2
  exit 1
fi

SIGNING_DETAILS="$(codesign -dvvv "$APP_BUNDLE" 2>&1)"
if [[ "$SIGNING_DETAILS" == *"Signature=adhoc"* ]] || [[ "$SIGNING_DETAILS" == *"TeamIdentifier=not set"* ]]; then
  echo "Tmon must be signed with a Developer ID Application identity before notarization" >&2
  exit 1
fi

VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$APP_BUNDLE/Contents/Info.plist")"
ARCHIVE="$ROOT_DIR/dist/Tmon-$VERSION-macos-universal.zip"
STAGED_ARCHIVE="$(mktemp /tmp/tmon-notary.XXXXXX.zip)"
trap 'rm -f "$STAGED_ARCHIVE"' EXIT

ditto -c -k --sequesterRsrc --keepParent "$APP_BUNDLE" "$STAGED_ARCHIVE"
xcrun notarytool submit "$STAGED_ARCHIVE" --keychain-profile "$NOTARY_PROFILE" --wait
xcrun stapler staple "$APP_BUNDLE"
xcrun stapler validate "$APP_BUNDLE"
codesign --verify --deep --strict --verbose=2 "$APP_BUNDLE"
spctl --assess --type execute --verbose=4 "$APP_BUNDLE"

rm -f "$STAGED_ARCHIVE"
ditto -c -k --sequesterRsrc --keepParent "$APP_BUNDLE" "$STAGED_ARCHIVE"
mv "$STAGED_ARCHIVE" "$ARCHIVE"
shasum -a 256 "$ARCHIVE" > "$ARCHIVE.sha256"
echo "notarized archive: $ARCHIVE"
