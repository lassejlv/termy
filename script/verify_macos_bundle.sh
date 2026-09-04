#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_BUNDLE="${1:-$ROOT_DIR/dist/Tmon.app}"
INFO_PLIST="$APP_BUNDLE/Contents/Info.plist"
APP_BINARY="$APP_BUNDLE/Contents/MacOS/tmon"
if [[ ! -f "$INFO_PLIST" ]]; then
  echo "missing Info.plist: $INFO_PLIST" >&2
  exit 1
fi
APP_ICON_NAME="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIconFile' "$INFO_PLIST")"
APP_ICON="$APP_BUNDLE/Contents/Resources/$APP_ICON_NAME"
EXPECTED_BUNDLE_IDENTIFIER="${TMON_BUNDLE_IDENTIFIER:-com.tmon.app}"
EXPECTED_BUILD_NUMBER="${TMON_BUILD_NUMBER:-1}"
EXPECTED_MINIMUM_MACOS="${TMON_MINIMUM_MACOS:-14.0}"
EXPECTED_VERSION="${TMON_EXPECTED_VERSION:-}"
EXPECTED_ARCHS="${EXPECTED_ARCHS:-}"

if [[ ! -x "$APP_BINARY" ]]; then
  echo "missing executable: $APP_BINARY" >&2
  exit 1
fi
if [[ ! -f "$APP_ICON" ]]; then
  echo "missing app icon: $APP_ICON" >&2
  exit 1
fi
for LICENSE_FILE in LICENSE-MIT LICENSE-APACHE; do
  if [[ ! -f "$APP_BUNDLE/Contents/Resources/$LICENSE_FILE" ]]; then
    echo "missing bundled license: $LICENSE_FILE" >&2
    exit 1
  fi
done
for DOCUMENT_FILE in \
  ACCESSIBILITY.md \
  PACKAGED_SMOKE.md \
  SECURITY.md \
  SESSION_LIFECYCLE.md \
  SUPPORT.md \
  THIRD_PARTY_LICENSES.md \
  UPDATE.md; do
  if [[ ! -f "$APP_BUNDLE/Contents/Resources/$DOCUMENT_FILE" ]]; then
    echo "missing bundled release document: $DOCUMENT_FILE" >&2
    exit 1
  fi
done

plutil -lint "$INFO_PLIST"
ACTUAL_BUNDLE_IDENTIFIER="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$INFO_PLIST")"
ACTUAL_BUILD_NUMBER="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$INFO_PLIST")"
ACTUAL_MINIMUM_MACOS="$(/usr/libexec/PlistBuddy -c 'Print :LSMinimumSystemVersion' "$INFO_PLIST")"
ACTUAL_VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$INFO_PLIST")"
if [[ "$ACTUAL_BUNDLE_IDENTIFIER" != "$EXPECTED_BUNDLE_IDENTIFIER" ]]; then
  echo "unexpected bundle identifier: $ACTUAL_BUNDLE_IDENTIFIER" >&2
  exit 1
fi
if [[ "$ACTUAL_BUILD_NUMBER" != "$EXPECTED_BUILD_NUMBER" ]]; then
  echo "unexpected build number: $ACTUAL_BUILD_NUMBER" >&2
  exit 1
fi
if [[ "$ACTUAL_MINIMUM_MACOS" != "$EXPECTED_MINIMUM_MACOS" ]]; then
  echo "unexpected minimum macOS version: $ACTUAL_MINIMUM_MACOS" >&2
  exit 1
fi
if [[ -n "$EXPECTED_VERSION" && "$ACTUAL_VERSION" != "$EXPECTED_VERSION" ]]; then
  echo "unexpected app version: $ACTUAL_VERSION" >&2
  exit 1
fi
sips -g format -g pixelWidth -g pixelHeight "$APP_ICON"
codesign --verify --deep --strict --verbose=2 "$APP_BUNDLE"
lipo -info "$APP_BINARY"
ARCHS="$(lipo -archs "$APP_BINARY")"
for ARCH in $ARCHS; do
  ACTUAL_BINARY_MINIMUM="$(
    otool -l -arch "$ARCH" "$APP_BINARY" \
      | awk '/cmd LC_BUILD_VERSION/{build_version=1; next} build_version && /minos/{print $2; exit}'
  )"
  if [[ "$ACTUAL_BINARY_MINIMUM" != "$EXPECTED_MINIMUM_MACOS" ]]; then
    echo "unexpected $ARCH Mach-O deployment target: ${ACTUAL_BINARY_MINIMUM:-unavailable}" >&2
    exit 1
  fi
done
case "$EXPECTED_ARCHS" in
  universal)
    for REQUIRED_ARCH in arm64 x86_64; do
      if [[ " $ARCHS " != *" $REQUIRED_ARCH "* ]]; then
        echo "universal bundle is missing architecture: $REQUIRED_ARCH" >&2
        exit 1
      fi
    done
    ;;
  native)
    if [[ " $ARCHS " != *" $(uname -m) "* ]]; then
      echo "native bundle does not contain host architecture: $(uname -m)" >&2
      exit 1
    fi
    ;;
  "")
    ;;
  *)
    echo "EXPECTED_ARCHS must be native, universal, or empty" >&2
    exit 2
    ;;
esac

SIGNING_DETAILS="$(codesign -dvvv --entitlements - "$APP_BUNDLE" 2>&1)"
printf '%s\n' "$SIGNING_DETAILS"
if [[ "$SIGNING_DETAILS" != *"runtime"* ]]; then
  echo "bundle signature does not enable the hardened runtime" >&2
  exit 1
fi
if [[ "$SIGNING_DETAILS" == *"Signature=adhoc"* ]]; then
  echo "signing: ad hoc local build; Developer ID signing and notarization are still required for distribution"
else
  if spctl --assess --type execute --verbose=4 "$APP_BUNDLE"; then
    echo "gatekeeper: accepted"
  else
    echo "gatekeeper: Developer ID signature is valid, but notarization may still be pending"
  fi
fi
