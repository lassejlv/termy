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
BUILD_NUMBER="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$APP_BUNDLE/Contents/Info.plist")"
ARCHIVE="$ROOT_DIR/dist/Tmon-$VERSION-$BUILD_NUMBER-macos-universal.zip"
STAGED_ARCHIVE="$(mktemp /tmp/tmon-notary.XXXXXX.zip)"
EVIDENCE_DIRECTORY="${TMON_RELEASE_EVIDENCE_DIR:-$ROOT_DIR/release/evidence}"
NOTARY_RESULT="$EVIDENCE_DIRECTORY/notarytool-$VERSION-$BUILD_NUMBER.json"
DISTRIBUTION_EVIDENCE="$EVIDENCE_DIRECTORY/distribution-$VERSION-$BUILD_NUMBER.json"
EVIDENCE_PLIST="$(mktemp /tmp/tmon-notary-evidence.XXXXXX.plist)"
trap 'rm -f "$STAGED_ARCHIVE" "$EVIDENCE_PLIST"' EXIT

if [[ -e "$NOTARY_RESULT" || -e "$DISTRIBUTION_EVIDENCE" ]]; then
  echo "refusing to overwrite notarization evidence for Tmon $VERSION ($BUILD_NUMBER)" >&2
  exit 1
fi
SOURCE_REVISION="$(git -C "$ROOT_DIR" rev-parse HEAD 2>/dev/null || printf unavailable)"
SOURCE_DIRTY=false
if [[ -n "$(git -C "$ROOT_DIR" status --porcelain=v1 --untracked-files=all 2>/dev/null)" ]]; then
  SOURCE_DIRTY=true
fi
mkdir -p "$EVIDENCE_DIRECTORY"

ditto -c -k --sequesterRsrc --keepParent "$APP_BUNDLE" "$STAGED_ARCHIVE"
xcrun notarytool submit "$STAGED_ARCHIVE" \
  --keychain-profile "$NOTARY_PROFILE" \
  --wait \
  --output-format json > "$NOTARY_RESULT"
NOTARY_STATUS="$(plutil -extract status raw -o - "$NOTARY_RESULT")"
if [[ "$NOTARY_STATUS" != "Accepted" ]]; then
  echo "Apple notarization did not accept Tmon: $NOTARY_STATUS" >&2
  exit 1
fi
NOTARY_ID="$(plutil -extract id raw -o - "$NOTARY_RESULT")"
xcrun stapler staple "$APP_BUNDLE"
xcrun stapler validate "$APP_BUNDLE"
codesign --verify --deep --strict --verbose=2 "$APP_BUNDLE"
spctl --assess --type execute --verbose=4 "$APP_BUNDLE"

rm -f "$STAGED_ARCHIVE"
ditto -c -k --sequesterRsrc --keepParent "$APP_BUNDLE" "$STAGED_ARCHIVE"
mv "$STAGED_ARCHIVE" "$ARCHIVE"
shasum -a 256 "$ARCHIVE" > "$ARCHIVE.sha256"
ARCHIVE_SHA256="$(awk 'NR == 1 { print $1 }' "$ARCHIVE.sha256")"
SIGNING_DETAILS="$(codesign -dvvv "$APP_BUNDLE" 2>&1)"
TEAM_ID="$(printf '%s\n' "$SIGNING_DETAILS" | sed -n 's/^TeamIdentifier=//p' | head -1)"
if [[ -z "$TEAM_ID" ]]; then
  echo "Developer ID signature did not expose a Team ID" >&2
  exit 1
fi

plutil -create xml1 "$EVIDENCE_PLIST"
plutil -insert schema_version -integer 1 "$EVIDENCE_PLIST"
plutil -insert generated_at -string "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" "$EVIDENCE_PLIST"
plutil -insert application_version -string "$VERSION" "$EVIDENCE_PLIST"
plutil -insert bundle_build_number -string "$BUILD_NUMBER" "$EVIDENCE_PLIST"
plutil -insert source_revision -string "$SOURCE_REVISION" "$EVIDENCE_PLIST"
plutil -insert source_dirty -bool "$SOURCE_DIRTY" "$EVIDENCE_PLIST"
plutil -insert team_id -string "$TEAM_ID" "$EVIDENCE_PLIST"
plutil -insert notary_submission_id -string "$NOTARY_ID" "$EVIDENCE_PLIST"
plutil -insert notary_status -string "$NOTARY_STATUS" "$EVIDENCE_PLIST"
plutil -insert stapled_ticket -bool true "$EVIDENCE_PLIST"
plutil -insert gatekeeper_accepted -bool true "$EVIDENCE_PLIST"
plutil -insert archive -string "dist/$(basename "$ARCHIVE")" "$EVIDENCE_PLIST"
plutil -insert archive_sha256 -string "$ARCHIVE_SHA256" "$EVIDENCE_PLIST"
plutil -convert json -o "$DISTRIBUTION_EVIDENCE" "$EVIDENCE_PLIST"
plutil -p "$NOTARY_RESULT" >/dev/null
plutil -p "$DISTRIBUTION_EVIDENCE" >/dev/null

echo "notarized archive: $ARCHIVE"
echo "notary result: $NOTARY_RESULT"
echo "distribution evidence: $DISTRIBUTION_EVIDENCE"
