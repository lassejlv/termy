#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

usage() {
  cat <<EOF
Usage: $0 [--version VERSION] [--arch ARCH] [--target TARGET] [--format FORMAT]

Options:
  --version VERSION   Set version (default: read from crates/desktop_app/Cargo.toml)
  --arch ARCH         Set architecture (x86_64 or aarch64)
  --target TARGET     Set target triple (x86_64-unknown-linux-gnu or aarch64-unknown-linux-gnu)
  --format FORMAT     Output format: tarball (default), appimage, deb, or rpm
  --help, -h          Show this help message

Output:
  target/dist/Termy-<version>-linux-<arch>.tar.gz
  or
  target/dist/Termy-<version>-linux-<arch>.AppImage
  or
  target/dist/Termy-<version>-linux-<arch>.deb
  or
  target/dist/Termy-<version>-linux-<arch>.rpm
EOF
}

die() {
  echo "Error: $*" >&2
  exit 1
}

log() {
  echo "==> $*"
}

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "'$1' is required"
}

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
    x86_64|amd64) echo "x86_64-unknown-linux-gnu" ;;
    aarch64|arm64) echo "aarch64-unknown-linux-gnu" ;;
    *) return 1 ;;
  esac
}

target_to_arch() {
  case "$1" in
    x86_64-unknown-linux-gnu) echo "x86_64" ;;
    aarch64-unknown-linux-gnu) echo "aarch64" ;;
    *) return 1 ;;
  esac
}

VERSION=""
ARCH=""
TARGET=""
FORMAT="tarball"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --version)
      [[ $# -ge 2 ]] || die "--version requires a value"
      VERSION="$2"
      shift 2
      ;;
    --arch)
      [[ $# -ge 2 ]] || die "--arch requires a value"
      ARCH="$2"
      shift 2
      ;;
    --target)
      [[ $# -ge 2 ]] || die "--target requires a value"
      TARGET="$2"
      shift 2
      ;;
    --format)
      [[ $# -ge 2 ]] || die "--format requires a value"
      FORMAT="$2"
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      die "Unknown option: $1 (use --help)"
      ;;
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

APP_NAME="Termy"
APP_NAME_LOWER="termy"
OS_NAME="linux"
DIST_DIR="$REPO_ROOT/target/dist"
TARGET_RELEASE_DIR="$REPO_ROOT/target/$TARGET/release"
BINARY_PATH="$TARGET_RELEASE_DIR/$APP_NAME_LOWER"
APPIMAGETOOL_BIN="${APPIMAGETOOL:-appimagetool}"

require_cmd cargo
if [[ "$FORMAT" == "deb" ]]; then
  require_cmd dpkg-deb
fi
if [[ "$FORMAT" == "rpm" ]]; then
  require_cmd rpmbuild
fi
if [[ "$FORMAT" == "appimage" ]]; then
  if [[ "$APPIMAGETOOL_BIN" == */* ]]; then
    [[ -x "$APPIMAGETOOL_BIN" ]] || die "AppImage tool not executable: $APPIMAGETOOL_BIN"
  else
    require_cmd "$APPIMAGETOOL_BIN"
  fi
fi

log "Building $APP_NAME v$VERSION for $ARCH ($TARGET)"
(cd "$REPO_ROOT" && cargo build --release --target "$TARGET" -p termy -p termy_cli)

CLI_BINARY_PATH="$TARGET_RELEASE_DIR/termy-cli"
[[ -f "$BINARY_PATH" ]] || die "Binary not found at $BINARY_PATH"
[[ -f "$CLI_BINARY_PATH" ]] || die "CLI binary not found at $CLI_BINARY_PATH"

mkdir -p "$DIST_DIR"

# Shared FHS staging for deb/rpm packages: payload in /usr/lib/termy with a
# /usr/bin/termy launcher, so assets stay a sibling of the real binary,
# matching the tarball and AppImage layouts.
FILE_MANAGER_DIR="$REPO_ROOT/scripts/file-manager"
OPEN_TAB_HERE_LABEL="Open new Termy tab here"

install_linux_file_manager_share() {
  local share_root="$1"
  local nautilus_script_source="$FILE_MANAGER_DIR/nautilus-open-tab.sh"
  local kde_source="$FILE_MANAGER_DIR/termy-open-tab.desktop"
  local nemo_source="$FILE_MANAGER_DIR/termy-open-tab.nemo_action"

  [[ -f "$nautilus_script_source" ]] || die "Nautilus script not found at $nautilus_script_source"
  [[ -f "$kde_source" ]] || die "KDE service menu not found at $kde_source"
  [[ -f "$nemo_source" ]] || die "Nemo action not found at $nemo_source"

  mkdir -p \
    "$share_root/kio/servicemenus" \
    "$share_root/kservices5/ServiceMenus" \
    "$share_root/nemo/actions" \
    "$share_root/nautilus/scripts" \
    "$share_root/caja/scripts"

  cp "$kde_source" "$share_root/kio/servicemenus/termy-open-tab.desktop"
  cp "$kde_source" "$share_root/kservices5/ServiceMenus/termy-open-tab.desktop"
  cp "$nemo_source" "$share_root/nemo/actions/termy-open-tab.nemo_action"
  install -m 755 "$nautilus_script_source" "$share_root/nautilus/scripts/$OPEN_TAB_HERE_LABEL"
  install -m 755 "$nautilus_script_source" "$share_root/caja/scripts/$OPEN_TAB_HERE_LABEL"
}

stage_linux_package_root() {
  local root="$1"
  local desktop_file_source="$REPO_ROOT/scripts/aur/${APP_NAME_LOWER}.desktop"
  local icon_source="$REPO_ROOT/assets/${APP_NAME_LOWER}_icon.png"

  [[ -f "$desktop_file_source" ]] || die "Desktop file not found at $desktop_file_source"
  [[ -f "$icon_source" ]] || die "Linux app icon not found at $icon_source"

  mkdir -p \
    "$root/usr/bin" \
    "$root/usr/lib/$APP_NAME_LOWER" \
    "$root/usr/share/applications" \
    "$root/usr/share/pixmaps"

  cp "$BINARY_PATH" "$root/usr/lib/$APP_NAME_LOWER/termy-bin"
  cp "$CLI_BINARY_PATH" "$root/usr/lib/$APP_NAME_LOWER/termy-cli"
  chmod 755 "$root/usr/lib/$APP_NAME_LOWER/termy-bin" "$root/usr/lib/$APP_NAME_LOWER/termy-cli"

  if [[ -d "$REPO_ROOT/assets" ]]; then
    mkdir -p "$root/usr/lib/$APP_NAME_LOWER/assets"
    cp -r "$REPO_ROOT/assets/"* "$root/usr/lib/$APP_NAME_LOWER/assets/" 2>/dev/null || true
  fi

  cat > "$root/usr/bin/$APP_NAME_LOWER" <<LAUNCHER
#!/usr/bin/env bash
set -euo pipefail

if [[ "\${TERMY_LINUX_BACKEND:-x11}" == "x11" && -n "\${DISPLAY:-}" ]]; then
  unset WAYLAND_DISPLAY
fi

exec /usr/lib/$APP_NAME_LOWER/termy-bin "\$@"
LAUNCHER
  chmod 755 "$root/usr/bin/$APP_NAME_LOWER"
  ln -s "../lib/$APP_NAME_LOWER/termy-cli" "$root/usr/bin/termy-cli"

  cp "$desktop_file_source" "$root/usr/share/applications/${APP_NAME_LOWER}.desktop"
  cp "$icon_source" "$root/usr/share/pixmaps/${APP_NAME_LOWER}.png"
  install_linux_file_manager_share "$root/usr/share"
}

case "$FORMAT" in
  tarball)
    STAGING_DIR="$REPO_ROOT/target/linux-staging"
    TARBALL_NAME="${APP_NAME}-${VERSION}-${OS_NAME}-${ARCH}.tar.gz"
    OUTPUT_PATH="$DIST_DIR/$TARBALL_NAME"

    log "Creating tarball staging directory"
    rm -rf "$STAGING_DIR"
    mkdir -p "$STAGING_DIR/$APP_NAME_LOWER"

    cp "$BINARY_PATH" "$STAGING_DIR/$APP_NAME_LOWER/termy-bin"
    cp "$CLI_BINARY_PATH" "$STAGING_DIR/$APP_NAME_LOWER/"
    cat > "$STAGING_DIR/$APP_NAME_LOWER/$APP_NAME_LOWER" <<'LAUNCHER'
#!/usr/bin/env bash
set -euo pipefail

if [[ "${TERMY_LINUX_BACKEND:-x11}" == "x11" && -n "${DISPLAY:-}" ]]; then
  unset WAYLAND_DISPLAY
fi

exec "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/termy-bin" "$@"
LAUNCHER
    chmod +x "$STAGING_DIR/$APP_NAME_LOWER/$APP_NAME_LOWER" "$STAGING_DIR/$APP_NAME_LOWER/termy-bin"

    if [[ -d "$REPO_ROOT/assets" ]]; then
      mkdir -p "$STAGING_DIR/$APP_NAME_LOWER/assets"
      cp -r "$REPO_ROOT/assets/"* "$STAGING_DIR/$APP_NAME_LOWER/assets/" 2>/dev/null || true
    fi

    mkdir -p "$STAGING_DIR/$APP_NAME_LOWER/file-manager"
    cp "$REPO_ROOT/scripts/aur/${APP_NAME_LOWER}.desktop" "$STAGING_DIR/$APP_NAME_LOWER/file-manager/termy.desktop"
    cp "$FILE_MANAGER_DIR/termy-open-tab.desktop" "$STAGING_DIR/$APP_NAME_LOWER/file-manager/"
    cp "$FILE_MANAGER_DIR/termy-open-tab.nemo_action" "$STAGING_DIR/$APP_NAME_LOWER/file-manager/"
    install -m 755 "$FILE_MANAGER_DIR/nautilus-open-tab.sh" "$STAGING_DIR/$APP_NAME_LOWER/file-manager/nautilus-open-tab.sh"

    # Embed a tarball-local installer.
    cat > "$STAGING_DIR/$APP_NAME_LOWER/install.sh" <<'INSTALL_SCRIPT'
#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
INSTALL_DIR="${1:-$HOME/.local/bin}"

mkdir -p "$INSTALL_DIR"
rm -f "$INSTALL_DIR/termy" "$INSTALL_DIR/termy-bin" "$INSTALL_DIR/termy-cli"
if [[ -f "$SCRIPT_DIR/termy-bin" ]]; then
  cp "$SCRIPT_DIR/termy-bin" "$INSTALL_DIR/termy-bin"
else
  cp "$SCRIPT_DIR/termy" "$INSTALL_DIR/termy-bin"
fi
cp "$SCRIPT_DIR/termy-cli" "$INSTALL_DIR/termy-cli"
cat > "$INSTALL_DIR/termy" <<'LAUNCHER'
#!/usr/bin/env bash
set -euo pipefail

if [[ "${TERMY_LINUX_BACKEND:-x11}" == "x11" && -n "${DISPLAY:-}" ]]; then
  unset WAYLAND_DISPLAY
fi

exec "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/termy-bin" "$@"
LAUNCHER
chmod +x "$INSTALL_DIR/termy" "$INSTALL_DIR/termy-bin" "$INSTALL_DIR/termy-cli"

if [[ -d "$SCRIPT_DIR/file-manager" ]]; then
  SHARE_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
  mkdir -p \
    "$SHARE_HOME/applications" \
    "$SHARE_HOME/kio/servicemenus" \
    "$SHARE_HOME/kservices5/ServiceMenus" \
    "$SHARE_HOME/nemo/actions" \
    "$SHARE_HOME/nautilus/scripts" \
    "$SHARE_HOME/caja/scripts"
  cp "$SCRIPT_DIR/file-manager/termy.desktop" "$SHARE_HOME/applications/termy.desktop"
  cp "$SCRIPT_DIR/file-manager/termy-open-tab.desktop" "$SHARE_HOME/kio/servicemenus/termy-open-tab.desktop"
  cp "$SCRIPT_DIR/file-manager/termy-open-tab.desktop" "$SHARE_HOME/kservices5/ServiceMenus/termy-open-tab.desktop"
  cp "$SCRIPT_DIR/file-manager/termy-open-tab.nemo_action" "$SHARE_HOME/nemo/actions/termy-open-tab.nemo_action"
  install -m 755 "$SCRIPT_DIR/file-manager/nautilus-open-tab.sh" "$SHARE_HOME/nautilus/scripts/Open new Termy tab here"
  install -m 755 "$SCRIPT_DIR/file-manager/nautilus-open-tab.sh" "$SHARE_HOME/caja/scripts/Open new Termy tab here"
fi

echo "Installed termy and termy-cli to $INSTALL_DIR/"
echo "Make sure $INSTALL_DIR is in your PATH"
INSTALL_SCRIPT
    chmod +x "$STAGING_DIR/$APP_NAME_LOWER/install.sh"

    log "Creating tarball"
    (cd "$STAGING_DIR" && tar -czvf "$OUTPUT_PATH" "$APP_NAME_LOWER")
    rm -rf "$STAGING_DIR"

    echo "Done: $OUTPUT_PATH"
    ;;

  appimage)
    APPIMAGE_STAGING_ROOT="$REPO_ROOT/target/linux-appimage-staging"
    APPDIR="$APPIMAGE_STAGING_ROOT/${APP_NAME}.AppDir"
    APPIMAGE_NAME="${APP_NAME}-${VERSION}-${OS_NAME}-${ARCH}.AppImage"
    OUTPUT_PATH="$DIST_DIR/$APPIMAGE_NAME"
    DESKTOP_FILE_SOURCE="$REPO_ROOT/scripts/aur/${APP_NAME_LOWER}.desktop"
    ICON_SOURCE="$REPO_ROOT/assets/${APP_NAME_LOWER}_icon.png"

    log "Creating AppImage staging directory"
    rm -rf "$APPIMAGE_STAGING_ROOT"
    mkdir -p \
      "$APPDIR/usr/bin" \
      "$APPDIR/usr/share/applications" \
      "$APPDIR/usr/share/icons/hicolor/512x512/apps"

    cp "$BINARY_PATH" "$APPDIR/usr/bin/$APP_NAME_LOWER"
    cp "$CLI_BINARY_PATH" "$APPDIR/usr/bin/termy-cli"
    chmod +x "$APPDIR/usr/bin/$APP_NAME_LOWER" "$APPDIR/usr/bin/termy-cli"

    # Keep assets as a sibling to the binary, matching the tarball layout.
    if [[ -d "$REPO_ROOT/assets" ]]; then
      mkdir -p "$APPDIR/usr/bin/assets"
      cp -r "$REPO_ROOT/assets/"* "$APPDIR/usr/bin/assets/" 2>/dev/null || true
    fi

    if [[ -f "$DESKTOP_FILE_SOURCE" ]]; then
      cp "$DESKTOP_FILE_SOURCE" "$APPDIR/${APP_NAME_LOWER}.desktop"
      cp "$DESKTOP_FILE_SOURCE" "$APPDIR/usr/share/applications/${APP_NAME_LOWER}.desktop"
    else
      cat > "$APPDIR/${APP_NAME_LOWER}.desktop" <<EOF
[Desktop Entry]
Name=$APP_NAME
Exec=$APP_NAME_LOWER %F
Icon=$APP_NAME_LOWER
Type=Application
Categories=System;TerminalEmulator;
MimeType=inode/directory;x-scheme-handler/termy;
Actions=open-tab-here;

[Desktop Action open-tab-here]
Name=Open new Termy tab here
Exec=$APP_NAME_LOWER --working-directory %f
EOF
      cp "$APPDIR/${APP_NAME_LOWER}.desktop" "$APPDIR/usr/share/applications/${APP_NAME_LOWER}.desktop"
    fi

    install_linux_file_manager_share "$APPDIR/usr/share"

    [[ -f "$ICON_SOURCE" ]] || die "Linux app icon not found at $ICON_SOURCE"
    cp "$ICON_SOURCE" "$APPDIR/${APP_NAME_LOWER}.png"
    cp "$ICON_SOURCE" "$APPDIR/usr/share/icons/hicolor/512x512/apps/${APP_NAME_LOWER}.png"

    cat > "$APPDIR/AppRun" <<'APP_RUN'
#!/usr/bin/env bash
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [[ "${TERMY_LINUX_BACKEND:-x11}" == "x11" && -n "${DISPLAY:-}" ]]; then
  unset WAYLAND_DISPLAY
fi

exec "$HERE/usr/bin/termy" "$@"
APP_RUN
    chmod +x "$APPDIR/AppRun"

    log "Creating AppImage"
    rm -f "$OUTPUT_PATH"
    if [[ "$APPIMAGETOOL_BIN" == *.AppImage ]]; then
      ARCH="$ARCH" "$APPIMAGETOOL_BIN" --appimage-extract-and-run "$APPDIR" "$OUTPUT_PATH"
    else
      ARCH="$ARCH" "$APPIMAGETOOL_BIN" "$APPDIR" "$OUTPUT_PATH"
    fi

    [[ -f "$OUTPUT_PATH" ]] || die "AppImage was not created at $OUTPUT_PATH"

    rm -rf "$APPIMAGE_STAGING_ROOT"
    echo "Done: $OUTPUT_PATH"
    ;;

  deb)
    DEB_STAGING_ROOT="$REPO_ROOT/target/linux-deb-staging"
    DEB_ROOT="$DEB_STAGING_ROOT/root"
    DEB_NAME="${APP_NAME}-${VERSION}-${OS_NAME}-${ARCH}.deb"
    OUTPUT_PATH="$DIST_DIR/$DEB_NAME"

    # Debian version strings must start with a digit.
    DEB_VERSION="${VERSION#v}"
    case "$ARCH" in
      x86_64) DEB_ARCH="amd64" ;;
      aarch64) DEB_ARCH="arm64" ;;
      *) die "Unsupported deb architecture: $ARCH" ;;
    esac

    log "Creating deb staging directory"
    rm -rf "$DEB_STAGING_ROOT"
    mkdir -p "$DEB_ROOT/DEBIAN"
    stage_linux_package_root "$DEB_ROOT"

    INSTALLED_SIZE="$(du -sk "$DEB_ROOT/usr" | cut -f1)"
    cat > "$DEB_ROOT/DEBIAN/control" <<EOF
Package: $APP_NAME_LOWER
Version: $DEB_VERSION
Architecture: $DEB_ARCH
Maintainer: Lasse Vestergaard <hello@lassejlv.dk>
Installed-Size: $INSTALLED_SIZE
Depends: bash, libc6, libgcc-s1, libglib2.0-0, libfreetype6, libfontconfig1, libxcb1, libwayland-client0, libxkbcommon0, libxkbcommon-x11-0, libvulkan1
Section: x11
Priority: optional
Homepage: https://github.com/lassejlv/termy
Description: Minimal GPUI-powered terminal
 Termy is a terminal emulator built with GPUI and alacritty_terminal.
EOF

    log "Creating deb package"
    rm -f "$OUTPUT_PATH"
    dpkg-deb --build --root-owner-group "$DEB_ROOT" "$OUTPUT_PATH"

    [[ -f "$OUTPUT_PATH" ]] || die "Deb package was not created at $OUTPUT_PATH"

    rm -rf "$DEB_STAGING_ROOT"
    echo "Done: $OUTPUT_PATH"
    ;;

  rpm)
    RPM_STAGING_ROOT="$REPO_ROOT/target/linux-rpm-staging"
    RPM_ROOT="$RPM_STAGING_ROOT/root"
    RPM_TOPDIR="$RPM_STAGING_ROOT/rpmbuild"
    RPM_NAME="${APP_NAME}-${VERSION}-${OS_NAME}-${ARCH}.rpm"
    OUTPUT_PATH="$DIST_DIR/$RPM_NAME"

    # RPM version strings must not contain dashes or a leading 'v'.
    RPM_VERSION="${VERSION#v}"
    [[ "$RPM_VERSION" != *-* ]] || die "RPM version must not contain dashes: $RPM_VERSION"

    log "Creating rpm staging directory"
    rm -rf "$RPM_STAGING_ROOT"
    mkdir -p "$RPM_TOPDIR"/{BUILD,BUILDROOT,RPMS,SPECS,SOURCES,SRPMS}
    stage_linux_package_root "$RPM_ROOT"

    # AutoReqProv is disabled because this packages a pre-built binary; letting
    # rpmbuild derive shared-library requirements would tie the package to the
    # builder distro's exact soname provides.
    cat > "$RPM_TOPDIR/SPECS/$APP_NAME_LOWER.spec" <<EOF
%global __os_install_post %{nil}
%global debug_package %{nil}

Name: $APP_NAME_LOWER
Version: $RPM_VERSION
Release: 1
Summary: Minimal GPUI-powered terminal
License: MIT
URL: https://github.com/lassejlv/termy
AutoReqProv: no

%description
Termy is a terminal emulator built with GPUI and alacritty_terminal.

%install
cp -a "$RPM_ROOT/." "%{buildroot}/"

%files
/usr/bin/$APP_NAME_LOWER
/usr/bin/termy-cli
/usr/lib/$APP_NAME_LOWER
/usr/share/applications/$APP_NAME_LOWER.desktop
/usr/share/pixmaps/$APP_NAME_LOWER.png
/usr/share/kio/servicemenus/termy-open-tab.desktop
/usr/share/kservices5/ServiceMenus/termy-open-tab.desktop
/usr/share/nemo/actions/termy-open-tab.nemo_action
"/usr/share/nautilus/scripts/Open new Termy tab here"
"/usr/share/caja/scripts/Open new Termy tab here"
EOF

    log "Creating rpm package"
    rpmbuild -bb \
      --define "_topdir $RPM_TOPDIR" \
      --target "$ARCH" \
      "$RPM_TOPDIR/SPECS/$APP_NAME_LOWER.spec"

    BUILT_RPM="$RPM_TOPDIR/RPMS/$ARCH/$APP_NAME_LOWER-$RPM_VERSION-1.$ARCH.rpm"
    [[ -f "$BUILT_RPM" ]] || die "RPM was not created at $BUILT_RPM"

    rm -f "$OUTPUT_PATH"
    cp "$BUILT_RPM" "$OUTPUT_PATH"

    rm -rf "$RPM_STAGING_ROOT"
    echo "Done: $OUTPUT_PATH"
    ;;

  *)
    die "Unknown format: $FORMAT (use tarball, appimage, deb, or rpm)"
    ;;
esac
