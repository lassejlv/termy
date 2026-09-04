#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_NAME="Tmon"
BINARY_NAME="tmon"
APP_ICON_SOURCE="$ROOT_DIR/assets/tmon-logo-smooth.png"
APP_ICON_RESOURCE="$ROOT_DIR/packaging/Tmon.icns"
APP_ICON_NAME="Tmon.icns"
ARCH_MODE="universal"
CREATE_ARCHIVE=1
MINIMUM_MACOS="${TMON_MINIMUM_MACOS:-14.0}"
BUILD_NUMBER="${TMON_BUILD_NUMBER:-1}"
SIGN_IDENTITY="${TMON_SIGN_IDENTITY:--}"
BUNDLE_IDENTIFIER="${TMON_BUNDLE_IDENTIFIER:-com.tmon.app}"

while (( $# > 0 )); do
  case "$1" in
    --native)
      ARCH_MODE="native"
      ;;
    --universal)
      ARCH_MODE="universal"
      ;;
    --no-archive)
      CREATE_ARCHIVE=0
      ;;
    *)
      echo "usage: $0 [--native|--universal] [--no-archive]" >&2
      exit 2
      ;;
  esac
  shift
done

if [[ ! "$MINIMUM_MACOS" =~ ^[0-9]+\.[0-9]+$ ]]; then
  echo "TMON_MINIMUM_MACOS must look like 14.0" >&2
  exit 2
fi
if [[ ! "$BUILD_NUMBER" =~ ^[0-9]+(\.[0-9]+){0,2}$ ]]; then
  echo "TMON_BUILD_NUMBER must contain one to three integer components" >&2
  exit 2
fi
if [[ ! "$BUNDLE_IDENTIFIER" =~ ^[A-Za-z0-9][A-Za-z0-9-]*(\.[A-Za-z0-9][A-Za-z0-9-]*)+$ ]]; then
  echo "TMON_BUNDLE_IDENTIFIER must be a reverse-DNS identifier" >&2
  exit 2
fi

PACKAGE_ID="$(cargo pkgid --locked --manifest-path "$ROOT_DIR/Cargo.toml" -p tmon)"
VERSION="${PACKAGE_ID##*@}"
if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "Tmon package version must be a numeric three-component version" >&2
  exit 2
fi

if [[ -z "${TMON_SOURCE_REVISION:-}" ]]; then
  TMON_SOURCE_REVISION="$(git -C "$ROOT_DIR" rev-parse HEAD 2>/dev/null || printf 'unavailable')"
fi
if [[ -z "${TMON_SOURCE_DIRTY:-}" ]]; then
  if TREE_STATUS="$(git -C "$ROOT_DIR" status --porcelain=v1 --untracked-files=all 2>/dev/null)"; then
    if [[ -n "$TREE_STATUS" ]]; then
      TMON_SOURCE_DIRTY=true
    else
      TMON_SOURCE_DIRTY=false
    fi
  else
    TMON_SOURCE_DIRTY=unknown
  fi
fi
case "$TMON_SOURCE_DIRTY" in
  true|false|unknown) ;;
  *)
    echo "TMON_SOURCE_DIRTY must be true, false, or unknown" >&2
    exit 2
    ;;
esac
export TMON_BUILD_NUMBER TMON_SOURCE_REVISION TMON_SOURCE_DIRTY

export MACOSX_DEPLOYMENT_TARGET="$MINIMUM_MACOS"
STAGING_DIR="$(mktemp -d /tmp/tmon-package.XXXXXX)"
trap 'rm -rf "$STAGING_DIR"' EXIT
STAGED_APP="$STAGING_DIR/$APP_NAME.app"
STAGED_CONTENTS="$STAGED_APP/Contents"
STAGED_MACOS="$STAGED_CONTENTS/MacOS"
STAGED_RESOURCES="$STAGED_CONTENTS/Resources"
STAGED_BINARY="$STAGED_MACOS/$BINARY_NAME"
mkdir -p "$STAGED_MACOS" "$STAGED_RESOURCES"

if [[ ! -f "$APP_ICON_SOURCE" ]]; then
  echo "missing app icon source: $APP_ICON_SOURCE" >&2
  exit 1
fi
if [[ ! -f "$APP_ICON_RESOURCE" ]]; then
  echo "missing packaged app icon generated from $APP_ICON_SOURCE: $APP_ICON_RESOURCE" >&2
  exit 1
fi
cp "$APP_ICON_RESOURCE" "$STAGED_RESOURCES/$APP_ICON_NAME"
for LICENSE_FILE in LICENSE-MIT LICENSE-APACHE; do
  if [[ ! -f "$ROOT_DIR/$LICENSE_FILE" ]]; then
    echo "missing license text: $ROOT_DIR/$LICENSE_FILE" >&2
    exit 1
  fi
  cp "$ROOT_DIR/$LICENSE_FILE" "$STAGED_RESOURCES/$LICENSE_FILE"
done
for DOCUMENT_FILE in \
  ACCESSIBILITY.md \
  PACKAGED_SMOKE.md \
  SECURITY.md \
  SESSION_LIFECYCLE.md \
  SUPPORT.md \
  THIRD_PARTY_LICENSES.md \
  UPDATE.md; do
  if [[ ! -f "$ROOT_DIR/$DOCUMENT_FILE" ]]; then
    echo "missing bundled release document: $ROOT_DIR/$DOCUMENT_FILE" >&2
    exit 1
  fi
  cp "$ROOT_DIR/$DOCUMENT_FILE" "$STAGED_RESOURCES/$DOCUMENT_FILE"
done

case "$ARCH_MODE" in
  native)
    NATIVE_ARCH="$(uname -m)"
    case "$NATIVE_ARCH" in
      arm64)
        TARGET="aarch64-apple-darwin"
        ;;
      x86_64)
        TARGET="x86_64-apple-darwin"
        ;;
      *)
        echo "unsupported native architecture: $NATIVE_ARCH" >&2
        exit 2
        ;;
    esac
    cargo build --locked --manifest-path "$ROOT_DIR/Cargo.toml" --release -p tmon --target "$TARGET"
    cp "$ROOT_DIR/target/$TARGET/release/$BINARY_NAME" "$STAGED_BINARY"
    ARCHIVE_ARCH="$NATIVE_ARCH"
    ;;
  universal)
    INSTALLED_TARGETS="$(rustup target list --installed)"
    for TARGET in aarch64-apple-darwin x86_64-apple-darwin; do
      if [[ $'\n'"$INSTALLED_TARGETS"$'\n' != *$'\n'"$TARGET"$'\n'* ]]; then
        echo "Rust target is not installed: $TARGET" >&2
        echo "Install it with: rustup target add $TARGET" >&2
        exit 2
      fi
      cargo build --locked --manifest-path "$ROOT_DIR/Cargo.toml" --release -p tmon --target "$TARGET"
    done
    lipo -create \
      "$ROOT_DIR/target/aarch64-apple-darwin/release/$BINARY_NAME" \
      "$ROOT_DIR/target/x86_64-apple-darwin/release/$BINARY_NAME" \
      -output "$STAGED_BINARY"
    ARCHIVE_ARCH="universal"
    ;;
esac

chmod 755 "$STAGED_BINARY"
/usr/bin/sed \
  -e "s|@VERSION@|$VERSION|g" \
  -e "s|@BUILD_NUMBER@|$BUILD_NUMBER|g" \
  -e "s|@MINIMUM_MACOS@|$MINIMUM_MACOS|g" \
  -e "s|@BUNDLE_IDENTIFIER@|$BUNDLE_IDENTIFIER|g" \
  "$ROOT_DIR/packaging/Info.plist.in" > "$STAGED_CONTENTS/Info.plist"
printf 'APPL????' > "$STAGED_CONTENTS/PkgInfo"
plutil -lint "$STAGED_CONTENTS/Info.plist"

if [[ "$SIGN_IDENTITY" == "-" ]]; then
  codesign --force --options runtime --sign - "$STAGED_APP"
else
  codesign --force --options runtime --timestamp --sign "$SIGN_IDENTITY" "$STAGED_APP"
fi
codesign --verify --deep --strict --verbose=2 "$STAGED_APP"

DIST_DIR="$ROOT_DIR/dist"
FINAL_APP="$DIST_DIR/$APP_NAME.app"
mkdir -p "$DIST_DIR"
if [[ -e "$FINAL_APP" ]]; then
  rm -rf "$FINAL_APP"
fi
mv "$STAGED_APP" "$FINAL_APP"

EXPECTED_ARCHS="$ARCH_MODE" \
TMON_EXPECTED_VERSION="$VERSION" \
"$ROOT_DIR/script/verify_macos_bundle.sh" "$FINAL_APP"

if (( CREATE_ARCHIVE == 1 )); then
  ARCHIVE_NAME="$APP_NAME-$VERSION-$BUILD_NUMBER-macos-$ARCHIVE_ARCH.zip"
  STAGED_ARCHIVE="$STAGING_DIR/$ARCHIVE_NAME"
  FINAL_ARCHIVE="$DIST_DIR/$ARCHIVE_NAME"
  ditto -c -k --sequesterRsrc --keepParent "$FINAL_APP" "$STAGED_ARCHIVE"
  mv "$STAGED_ARCHIVE" "$FINAL_ARCHIVE"
  (
    cd "$DIST_DIR"
    shasum -a 256 "$ARCHIVE_NAME" > "$ARCHIVE_NAME.sha256"
  )
  echo "archive: $FINAL_ARCHIVE"
  echo "checksum: $FINAL_ARCHIVE.sha256"
fi

echo "app: $FINAL_APP"
