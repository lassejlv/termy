#!/usr/bin/env bash
# Build, sign, notarize and package Termy Next for release, separate from
# Termy's own release. Produces in next/dist:
#   TermyNext-<version>-macos-universal.dmg   signed, notarized and stapled
#   TermyNext-<version>-macos-universal.zip   the stapled app, zipped
#
# Usage: scripts/release.sh --version X.Y.Z [--build N] [--native] [--no-notarize]
#   --native       build for this Mac's architecture only (faster, for testing)
#   --no-notarize  sign but skip notarization and stapling
#
# Signing uses TERMY_SIGN_IDENTITY (a Developer ID Application identity, or
# "-" to sign ad hoc for a dry run). Notarization uses TERMY_NOTARY_KEY (the
# App Store Connect .p8 path), TERMY_NOTARY_KEY_ID and TERMY_NOTARY_ISSUER, as
# scripts/setup-macos-signing.sh exports them in CI.
set -euo pipefail

die() { echo "Error: $*" >&2; exit 1; }
log() { echo "==> $*"; }

version=""
build_number="1"
universal=1
notarize=1
while [[ $# -gt 0 ]]; do
  case "$1" in
    --version) version="$2"; shift ;;
    --build) build_number="$2"; shift ;;
    --native) universal=0 ;;
    --no-notarize) notarize=0 ;;
    *) die "unknown argument: $1" ;;
  esac
  shift
done
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$ ]] || die "pass --version X.Y.Z (got '${version}')"

identity="${TERMY_SIGN_IDENTITY:-}"
[[ -n "$identity" ]] || die "set TERMY_SIGN_IDENTITY to a Developer ID Application identity (or - for ad hoc)"
if [[ "$identity" == "-" ]]; then
  notarize=0
fi
if [[ "$notarize" -eq 1 ]]; then
  for name in TERMY_NOTARY_KEY TERMY_NOTARY_KEY_ID TERMY_NOTARY_ISSUER; do
    [[ -n "${!name:-}" ]] || die "notarization needs $name (or pass --no-notarize)"
  done
fi

next_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dist="$next_dir/dist"
app="$next_dir/build/Termy Next.app"
arch_name=$([[ "$universal" -eq 1 ]] && echo universal || uname -m)
base="TermyNext-$version-macos-$arch_name"
dmg="$dist/$base.dmg"
zip="$dist/$base.zip"

log "Building Termy Next $version ($arch_name)"
bundle_args=(release --version "$version" --build "$build_number")
[[ "$universal" -eq 1 ]] && bundle_args+=(--universal)
"$next_dir/scripts/bundle.sh" "${bundle_args[@]}"

log "Signing with $( [[ "$identity" == "-" ]] && echo "an ad hoc signature" || echo "$identity" )"
xattr -cr "$app"
sign=(codesign --force --sign "$identity")
if [[ "$identity" != "-" ]]; then
  # Hardened runtime and a secure timestamp, which notarization requires.
  sign+=(--options runtime --timestamp)
fi
# Inside out: the library and helper first, then the app that seals them.
"${sign[@]}" "$app/Contents/Frameworks/libtermy_core.dylib"
"${sign[@]}" "$app/Contents/Helpers/termy-session-host"
"${sign[@]}" "$app/Contents/MacOS/TermyNext"
"${sign[@]}" "$app"
codesign --verify --deep --strict --verbose=2 "$app"

notarize_file() {
  xcrun notarytool submit "$1" --key "$TERMY_NOTARY_KEY" --key-id "$TERMY_NOTARY_KEY_ID" \
    --issuer "$TERMY_NOTARY_ISSUER" --wait --timeout 30m
}

rm -rf "$dist"
mkdir -p "$dist"

if [[ "$notarize" -eq 1 ]]; then
  log "Notarizing the app"
  ditto -c -k --keepParent "$app" "$dist/notarize.zip"
  notarize_file "$dist/notarize.zip"
  rm -f "$dist/notarize.zip"
  xcrun stapler staple "$app"
  xcrun stapler validate "$app"
fi

log "Packaging $base.zip"
ditto -c -k --keepParent "$app" "$zip"

log "Packaging $base.dmg"
staging="$(mktemp -d)"
trap 'rm -rf "$staging"' EXIT
ditto "$app" "$staging/Termy Next.app"
ln -s /Applications "$staging/Applications"
hdiutil create -volname "Termy Next $version" -srcfolder "$staging" -fs HFS+ -format UDZO \
  -imagekey zlib-level=9 -ov "$dmg" >/dev/null
if [[ "$identity" == "-" ]]; then
  codesign --force --sign - "$dmg"
else
  codesign --force --timestamp --sign "$identity" "$dmg"
fi
codesign --verify --verbose=2 "$dmg"

if [[ "$notarize" -eq 1 ]]; then
  log "Notarizing the DMG"
  notarize_file "$dmg"
  xcrun stapler staple "$dmg"
  xcrun stapler validate "$dmg"
  log "Checking with Gatekeeper"
  spctl --assess --type execute --verbose=2 "$app"
  spctl --assess --type open --context context:primary-signature --verbose=2 "$dmg"
fi

(cd "$dist" && shasum -a 256 "$base.dmg" "$base.zip" > "$base.sha256")
log "Done:"
ls -lh "$dist"
