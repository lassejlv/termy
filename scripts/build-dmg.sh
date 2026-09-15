#!/usr/bin/env bash
set -euo pipefail

# Build the GPUI `termy` app into a drag-to-Applications macOS DMG.
#
# Signing and notarization are optional: without a signing identity this
# produces an unsigned DMG; with one it signs (and, if credentials are
# available, notarizes + staples) the bundle and disk image.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

usage() {
  cat <<EOF
Usage: $0 [options]

Build the GPUI Termy app as a drag-to-Applications DMG.

Build options:
  --version VERSION       Version (default: read from crates/desktop_app/Cargo.toml)
  --arch ARCH             Architecture: arm64 or x86_64 (default: host)
  --target TARGET         Target triple (aarch64-apple-darwin or x86_64-apple-darwin)
  --no-layout             Skip Finder icon-layout customization

Signing options (optional — omit for an unsigned DMG):
  --sign-identity NAME    Developer ID Application identity (enables signing)
  --entitlements PATH     Entitlements plist for app signing
  --no-sign-dmg           Sign the app but not the final DMG

Notarization options (require a signing identity):
  --notary-profile NAME   notarytool keychain profile name
  --notary-key PATH       App Store Connect API key file (.p8)
  --notary-key-id ID      App Store Connect API key ID
  --notary-issuer UUID    App Store Connect issuer ID
  --no-notarize           Skip notarization + stapling even when credentials exist

  --help, -h              Show this help message

Environment variable defaults:
  TERMY_SIGN_IDENTITY  TERMY_ENTITLEMENTS
  TERMY_NOTARY_PROFILE TERMY_NOTARY_KEY TERMY_NOTARY_KEY_ID TERMY_NOTARY_ISSUER

Output:
  dist/Termy-<version>-macos-<arch>[-signed].dmg
EOF
}

die() { echo "Error: $*" >&2; exit 1; }
log() { echo "==> $*"; }
require_cmd() { command -v "$1" >/dev/null 2>&1 || die "'$1' is required"; }

read_version_from_cargo_toml() {
  awk '
    /^\[package\]$/ { in_package = 1; next }
    /^\[/ && in_package { exit }
    in_package && $1 == "version" {
      gsub(/"/, "", $3)
      print $3
      exit
    }
  ' "$REPO_ROOT/crates/desktop_app/Cargo.toml"
}

arch_to_target() {
  case "$1" in
    arm64|aarch64) echo "aarch64-apple-darwin" ;;
    x86_64|amd64) echo "x86_64-apple-darwin" ;;
    *) return 1 ;;
  esac
}

target_to_arch() {
  case "$1" in
    aarch64-apple-darwin) echo "arm64" ;;
    x86_64-apple-darwin) echo "x86_64" ;;
    *) return 1 ;;
  esac
}

ensure_termy_url_scheme() {
  local plist_path="$1/Contents/Info.plist"
  local plist_buddy="/usr/libexec/PlistBuddy"

  [[ -f "$plist_path" ]] || die "App bundle Info.plist not found at $plist_path"
  [[ -x "$plist_buddy" ]] || die "PlistBuddy is required to patch $plist_path"

  "$plist_buddy" -c "Delete :CFBundleURLTypes" "$plist_path" >/dev/null 2>&1 || true
  "$plist_buddy" -c "Add :CFBundleURLTypes array" "$plist_path"
  "$plist_buddy" -c "Add :CFBundleURLTypes:0 dict" "$plist_path"
  "$plist_buddy" -c "Add :CFBundleURLTypes:0:CFBundleURLName string com.lassevestergaard.termy.deeplink" "$plist_path"
  "$plist_buddy" -c "Add :CFBundleURLTypes:0:CFBundleURLSchemes array" "$plist_path"
  "$plist_buddy" -c "Add :CFBundleURLTypes:0:CFBundleURLSchemes:0 string termy" "$plist_path"
  /usr/bin/plutil -lint "$plist_path" >/dev/null
}

ensure_folder_document_type() {
  local plist_path="$1/Contents/Info.plist"
  local plist_buddy="/usr/libexec/PlistBuddy"

  [[ -f "$plist_path" ]] || die "App bundle Info.plist not found at $plist_path"
  [[ -x "$plist_buddy" ]] || die "PlistBuddy is required to patch $plist_path"

  "$plist_buddy" -c "Delete :CFBundleDocumentTypes" "$plist_path" >/dev/null 2>&1 || true
  "$plist_buddy" -c "Add :CFBundleDocumentTypes array" "$plist_path"
  "$plist_buddy" -c "Add :CFBundleDocumentTypes:0 dict" "$plist_path"
  "$plist_buddy" -c "Add :CFBundleDocumentTypes:0:CFBundleTypeName string Folder" "$plist_path"
  "$plist_buddy" -c "Add :CFBundleDocumentTypes:0:CFBundleTypeRole string Viewer" "$plist_path"
  "$plist_buddy" -c "Add :CFBundleDocumentTypes:0:LSHandlerRank string Alternate" "$plist_path"
  "$plist_buddy" -c "Add :CFBundleDocumentTypes:0:LSItemContentTypes array" "$plist_path"
  "$plist_buddy" -c "Add :CFBundleDocumentTypes:0:LSItemContentTypes:0 string public.folder" "$plist_path"
  "$plist_buddy" -c "Add :CFBundleDocumentTypes:0:LSItemContentTypes:1 string public.directory" "$plist_path"
  /usr/bin/plutil -lint "$plist_path" >/dev/null
}

install_finder_open_tab_service() {
  local app_path="$1"
  local exe_path="$app_path/Contents/MacOS/$APP_NAME"
  local service_root="$app_path/Contents/Library/Services/Open new Termy tab here.workflow"
  local service_contents="$service_root/Contents"
  local info_source="$REPO_ROOT/scripts/file-manager/macos/Info.plist"
  local workflow_template="$REPO_ROOT/scripts/file-manager/macos/document.wflow.in"

  [[ -x "$exe_path" ]] || die "App binary not found at $exe_path"
  [[ -f "$info_source" ]] || die "Finder service Info.plist not found at $info_source"
  [[ -f "$workflow_template" ]] || die "Finder service workflow template not found at $workflow_template"

  mkdir -p "$service_contents"
  cp "$info_source" "$service_contents/Info.plist"
  local quoted_exe
  quoted_exe="$(printf "%q" "$exe_path")"
  sed "s|@TERMY_EXECUTABLE@|$quoted_exe|g" "$workflow_template" > "$service_contents/document.wflow"
}

ensure_app_icon() {
  local app_path="$1"
  local plist_path="$app_path/Contents/Info.plist"
  local resources_dir="$app_path/Contents/Resources"
  local plist_buddy="/usr/libexec/PlistBuddy"
  local icns_source="$REPO_ROOT/assets/termy.icns"

  [[ -f "$icns_source" ]] || die "App icon not found at $icns_source"
  [[ -f "$plist_path" ]] || die "App bundle Info.plist not found at $plist_path"
  [[ -x "$plist_buddy" ]] || die "PlistBuddy is required to patch $plist_path"

  mkdir -p "$resources_dir"
  cp "$icns_source" "$resources_dir/termy.icns"

  "$plist_buddy" -c "Delete :CFBundleIconFile" "$plist_path" >/dev/null 2>&1 || true
  "$plist_buddy" -c "Add :CFBundleIconFile string termy" "$plist_path"
  /usr/bin/plutil -lint "$plist_path" >/dev/null
}

VERSION=""
ARCH=""
TARGET=""
SKIP_LAYOUT=0

SIGN_IDENTITY="${TERMY_SIGN_IDENTITY:-}"
ENTITLEMENTS="${TERMY_ENTITLEMENTS:-}"
NOTARY_PROFILE="${TERMY_NOTARY_PROFILE:-}"
NOTARY_KEY="${TERMY_NOTARY_KEY:-}"
NOTARY_KEY_ID="${TERMY_NOTARY_KEY_ID:-}"
NOTARY_ISSUER="${TERMY_NOTARY_ISSUER:-}"
NOTARIZE=1
SIGN_DMG=1

while [[ $# -gt 0 ]]; do
  case "$1" in
    --version) [[ $# -ge 2 ]] || die "--version requires a value"; VERSION="$2"; shift 2 ;;
    --arch) [[ $# -ge 2 ]] || die "--arch requires a value"; ARCH="$2"; shift 2 ;;
    --target) [[ $# -ge 2 ]] || die "--target requires a value"; TARGET="$2"; shift 2 ;;
    --sign-identity) [[ $# -ge 2 ]] || die "--sign-identity requires a value"; SIGN_IDENTITY="$2"; shift 2 ;;
    --entitlements) [[ $# -ge 2 ]] || die "--entitlements requires a value"; ENTITLEMENTS="$2"; shift 2 ;;
    --notary-profile) [[ $# -ge 2 ]] || die "--notary-profile requires a value"; NOTARY_PROFILE="$2"; shift 2 ;;
    --notary-key) [[ $# -ge 2 ]] || die "--notary-key requires a value"; NOTARY_KEY="$2"; shift 2 ;;
    --notary-key-id) [[ $# -ge 2 ]] || die "--notary-key-id requires a value"; NOTARY_KEY_ID="$2"; shift 2 ;;
    --notary-issuer) [[ $# -ge 2 ]] || die "--notary-issuer requires a value"; NOTARY_ISSUER="$2"; shift 2 ;;
    --no-notarize) NOTARIZE=0; shift ;;
    --no-sign-dmg) SIGN_DMG=0; shift ;;
    --no-layout) SKIP_LAYOUT=1; shift ;;
    --help|-h) usage; exit 0 ;;
    *) die "Unknown option: $1 (use --help)" ;;
  esac
done

if [[ -z "$VERSION" ]]; then
  VERSION="$(read_version_from_cargo_toml)"
  [[ -n "$VERSION" ]] || die "Could not read version from crates/desktop_app/Cargo.toml"
fi

if [[ -z "$ARCH" && -z "$TARGET" ]]; then
  ARCH="$(uname -m)"
fi
if [[ -n "$ARCH" && -z "$TARGET" ]]; then
  TARGET="$(arch_to_target "$ARCH")" || die "Unsupported architecture: $ARCH"
fi
if [[ -n "$TARGET" && -z "$ARCH" ]]; then
  ARCH="$(target_to_arch "$TARGET")" || die "Unsupported target: $TARGET"
fi
if [[ -n "$ARCH" && -n "$TARGET" ]]; then
  EXPECTED_TARGET="$(arch_to_target "$ARCH")" || die "Unsupported architecture: $ARCH"
  [[ "$EXPECTED_TARGET" == "$TARGET" ]] || die "Mismatched --arch ($ARCH) and --target ($TARGET)"
fi

SIGN=0
if [[ -n "$SIGN_IDENTITY" ]]; then
  SIGN=1
else
  # Nothing to sign with — disable downstream signing/notarization cleanly.
  NOTARIZE=0
  SIGN_DMG=0
  if [[ -n "$NOTARY_PROFILE$NOTARY_KEY$NOTARY_KEY_ID$NOTARY_ISSUER$ENTITLEMENTS" ]]; then
    log "No signing identity set; ignoring signing/notarization options (unsigned build)"
  fi
fi

# Resolve notarization credential mode up front so we fail fast on bad input.
NOTARY_MODE=""
if [[ "$NOTARIZE" -eq 1 ]]; then
  if [[ -n "$NOTARY_PROFILE" && -n "$NOTARY_KEY$NOTARY_KEY_ID$NOTARY_ISSUER" ]]; then
    die "Use either --notary-profile OR the API-key flags, not both"
  elif [[ -n "$NOTARY_PROFILE" ]]; then
    NOTARY_MODE="profile"
  elif [[ -n "$NOTARY_KEY$NOTARY_KEY_ID$NOTARY_ISSUER" ]]; then
    [[ -n "$NOTARY_KEY" ]] || die "Missing --notary-key (or TERMY_NOTARY_KEY)"
    [[ -n "$NOTARY_KEY_ID" ]] || die "Missing --notary-key-id (or TERMY_NOTARY_KEY_ID)"
    [[ -n "$NOTARY_ISSUER" ]] || die "Missing --notary-issuer (or TERMY_NOTARY_ISSUER)"
    [[ -f "$NOTARY_KEY" ]] || die "Notary API key file not found: $NOTARY_KEY"
    NOTARY_MODE="apikey"
  else
    # Signing without notarization credentials is a valid (ad-hoc) flow.
    NOTARIZE=0
    log "No notarization credentials provided; skipping notarization"
  fi
fi

if [[ -n "$ENTITLEMENTS" ]]; then
  [[ -f "$ENTITLEMENTS" ]] || die "Entitlements file not found: $ENTITLEMENTS"
fi

APP_NAME="Termy"
SUFFIX=""
[[ "$SIGN" -eq 1 ]] && SUFFIX="-signed"
DMG_NAME="${APP_NAME}-${VERSION}-macos-${ARCH}${SUFFIX}"
VOLUME_NAME="${APP_NAME}-${VERSION}"

TARGET_RELEASE_DIR="$REPO_ROOT/target/$TARGET/release"
CLI_BINARY_PATH="$TARGET_RELEASE_DIR/termy-cli"
DIST_DIR="$REPO_ROOT/dist"
DMG_ROOT="$REPO_ROOT/target/dmg-root-$ARCH"
RW_DMG="$DIST_DIR/${DMG_NAME}-rw.dmg"
OUTPUT_DMG="$DIST_DIR/${DMG_NAME}.dmg"

require_cmd cargo
require_cmd hdiutil
cargo bundle --version >/dev/null 2>&1 || die "cargo-bundle not found. Install with: cargo install cargo-bundle"

if [[ "$SIGN" -eq 1 ]]; then
  require_cmd codesign
  require_cmd security
  require_cmd xattr
  if ! security find-identity -v -p codesigning | grep -F "$SIGN_IDENTITY" >/dev/null 2>&1; then
    security find-identity -v -p codesigning >&2 || true
    die "Signing identity not found in keychain: $SIGN_IDENTITY"
  fi
fi
if [[ "$NOTARIZE" -eq 1 ]]; then
  require_cmd xcrun
  require_cmd spctl
  xcrun notarytool --help >/dev/null 2>&1 || die "xcrun notarytool is required for notarization"
fi

if [[ ! -f "$REPO_ROOT/assets/termy.icns" || "$REPO_ROOT/assets/termy_icon@1024px.png" -nt "$REPO_ROOT/assets/termy.icns" ]]; then
  log "Generating app icon"
  "$SCRIPT_DIR/generate-icon.sh"
fi

log "Building $APP_NAME v$VERSION for $ARCH ($TARGET)"
(cd "$REPO_ROOT" && cargo build --release --target "$TARGET" -p termy -p termy_cli)
(cd "$REPO_ROOT" && cargo bundle --release --format osx --target "$TARGET" --package termy)

APP_PATH="$TARGET_RELEASE_DIR/bundle/osx/$APP_NAME.app"
if [[ ! -d "$APP_PATH" ]]; then
  APP_PATH="$(find "$REPO_ROOT/target" -maxdepth 5 -type d -name "$APP_NAME.app" -path "*/bundle/osx/*" | head -n1 || true)"
fi
[[ -n "$APP_PATH" && -d "$APP_PATH" ]] || die "Could not find built app bundle"
[[ -f "$CLI_BINARY_PATH" ]] || die "CLI binary not found at $CLI_BINARY_PATH"

log "Installing termy-cli into app bundle"
cp "$CLI_BINARY_PATH" "$APP_PATH/Contents/MacOS/termy-cli"
chmod +x "$APP_PATH/Contents/MacOS/termy-cli"

log "Registering termy:// URL scheme in app bundle"
ensure_termy_url_scheme "$APP_PATH"

log "Registering folder Open With support in app bundle"
ensure_folder_document_type "$APP_PATH"

log "Installing Finder Open new Termy tab here service"
install_finder_open_tab_service "$APP_PATH"

log "Embedding app icon in bundle"
ensure_app_icon "$APP_PATH"

if [[ "$SIGN" -eq 1 ]]; then
  log "Signing app bundle with: $SIGN_IDENTITY"
  xattr -rc "$APP_PATH"
  CODESIGN_ARGS=(--force --deep --options runtime --timestamp --sign "$SIGN_IDENTITY")
  [[ -n "$ENTITLEMENTS" ]] && CODESIGN_ARGS+=(--entitlements "$ENTITLEMENTS")
  codesign "${CODESIGN_ARGS[@]}" "$APP_PATH"
  codesign --verify --deep --strict --verbose=2 "$APP_PATH"
fi

log "Preparing DMG staging folder"
rm -rf "$DMG_ROOT"
mkdir -p "$DMG_ROOT" "$DIST_DIR"
cp -R "$APP_PATH" "$DMG_ROOT/"
ln -s /Applications "$DMG_ROOT/Applications"

log "Creating temporary DMG"
rm -f "$RW_DMG" "$OUTPUT_DMG"
hdiutil create \
  -volname "$VOLUME_NAME" \
  -srcfolder "$DMG_ROOT" \
  -ov \
  -fs HFS+ \
  -format UDRW \
  "$RW_DMG" >/dev/null

DEVICE=""
cleanup() {
  if [[ -n "${DEVICE:-}" ]]; then
    hdiutil detach "$DEVICE" -quiet >/dev/null 2>&1 || true
  fi
  rm -rf "$DMG_ROOT"
}
trap cleanup EXIT

ATTACH_INFO="$(hdiutil attach -readwrite -noverify -noautoopen "$RW_DMG")"
ATTACH_LINE="$(printf '%s\n' "$ATTACH_INFO" | awk '/\/Volumes\// {print; exit}')"
DEVICE="${ATTACH_LINE%%[[:space:]]*}"
MOUNT_POINT="$(printf '%s\n' "$ATTACH_LINE" | sed -E 's@.*(/Volumes/.*)$@\1@')"
[[ -n "$DEVICE" && -n "$MOUNT_POINT" && -d "$MOUNT_POINT" ]] || die "Failed to mount temporary DMG. hdiutil output: $ATTACH_INFO"

if [[ "$SKIP_LAYOUT" -eq 0 && -x "/usr/bin/osascript" ]]; then
  log "Applying Finder layout"
  if ! /usr/bin/osascript <<EOF
tell application "Finder"
  tell disk "$VOLUME_NAME"
    open
    set current view of container window to icon view
    set toolbar visible of container window to false
    set statusbar visible of container window to false
    set bounds of container window to {120, 120, 660, 440}
    set opts to the icon view options of container window
    set arrangement of opts to not arranged
    set icon size of opts to 128
    set text size of opts to 12
    set position of item "$APP_NAME.app" to {150, 180}
    set position of item "Applications" to {390, 180}
    update without registering applications
    delay 1
    close
  end tell
end tell
EOF
  then
    echo "Warning: Finder layout customization failed; continuing without layout tweaks" >&2
  fi
else
  log "Skipping Finder layout customization"
fi

hdiutil detach "$DEVICE" -quiet
DEVICE=""

log "Converting to compressed DMG"
hdiutil convert "$RW_DMG" -format UDZO -imagekey zlib-level=9 -o "$OUTPUT_DMG" >/dev/null
rm -f "$RW_DMG"

if [[ "$SIGN" -eq 1 && "$SIGN_DMG" -eq 1 ]]; then
  log "Signing DMG"
  codesign --force --timestamp --sign "$SIGN_IDENTITY" "$OUTPUT_DMG"
  codesign --verify --verbose=2 "$OUTPUT_DMG"
fi

if [[ "$NOTARIZE" -eq 1 ]]; then
  log "Submitting DMG for notarization"
  NOTARY_ARGS=()
  if [[ "$NOTARY_MODE" == "profile" ]]; then
    NOTARY_ARGS+=(--keychain-profile "$NOTARY_PROFILE")
  else
    NOTARY_ARGS+=(--key "$NOTARY_KEY" --key-id "$NOTARY_KEY_ID" --issuer "$NOTARY_ISSUER")
  fi
  xcrun notarytool submit "$OUTPUT_DMG" "${NOTARY_ARGS[@]}" --wait

  log "Stapling notarization ticket"
  xcrun stapler staple "$OUTPUT_DMG"
  xcrun stapler validate "$OUTPUT_DMG"

  log "Assessing final DMG with Gatekeeper"
  spctl --assess --type open --verbose=2 "$OUTPUT_DMG"
fi

echo "Done: $OUTPUT_DMG"
