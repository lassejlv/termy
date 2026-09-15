#!/usr/bin/env bash
set -euo pipefail

# Termy Linux Installer
# Usage: curl -fsSL https://raw.githubusercontent.com/lassejlv/termy/main/scripts/install-linux.sh | bash

REPO="lassejlv/termy"
INSTALL_DIR="${TERMY_INSTALL_DIR:-$HOME/.local/bin}"

die() {
  echo "Error: $*" >&2
  exit 1
}

log() {
  echo "==> $*"
}

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "'$1' is required but not found"
}

json_string_values() {
  local key="$1"
  { grep -Eo "\"${key}\"[[:space:]]*:[[:space:]]*\"[^\"]*\"" || true; } \
    | sed -n 's/.*"[[:space:]]*:[[:space:]]*"\([^"]*\)"$/\1/p'
}

detect_arch() {
  local arch
  arch="$(uname -m)"
  case "$arch" in
    x86_64|amd64) echo "x86_64" ;;
    aarch64|arm64) echo "aarch64" ;;
    *) die "Unsupported architecture: $arch" ;;
  esac
}

require_cmd curl
require_cmd tar
require_cmd grep
require_cmd sed

log "Detecting system architecture..."
ARCH="$(detect_arch)"
log "Architecture: $ARCH"

log "Fetching latest release from GitHub..."
RELEASE_JSON="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest")"

TAG="$(printf '%s\n' "$RELEASE_JSON" | json_string_values "tag_name" | head -n1)"
if [[ -z "$TAG" ]]; then
  die "Could not determine latest release tag"
fi
log "Latest version: $TAG"

DOWNLOAD_URL="$(
  printf '%s\n' "$RELEASE_JSON" \
    | json_string_values "browser_download_url" \
    | grep -E "linux.*${ARCH}.*\.tar\.gz$" \
    | head -n1 \
    || true
)"

if [[ -z "$DOWNLOAD_URL" ]]; then
  DOWNLOAD_URL="$(
    printf '%s\n' "$RELEASE_JSON" \
      | json_string_values "browser_download_url" \
      | grep -E "linux.*\.tar\.gz$" \
      | grep -Ev 'linux.*(x86_64|amd64|aarch64|arm64).*\.tar\.gz$' \
      | head -n1 \
      || true
  )"
fi

if [[ -z "$DOWNLOAD_URL" ]]; then
  die "Could not find Linux tarball for architecture '$ARCH' in release $TAG"
fi

log "Download URL: $DOWNLOAD_URL"

TEMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TEMP_DIR"' EXIT

TARBALL_PATH="$TEMP_DIR/termy.tar.gz"

log "Downloading Termy $TAG..."
curl -fsSL "$DOWNLOAD_URL" -o "$TARBALL_PATH"

log "Extracting..."
tar -xzf "$TARBALL_PATH" -C "$TEMP_DIR"

BINARY_PATH=""
if [[ -f "$TEMP_DIR/termy/termy-bin" ]]; then
  BINARY_PATH="$TEMP_DIR/termy/termy-bin"
elif [[ -f "$TEMP_DIR/termy/termy" ]]; then
  BINARY_PATH="$TEMP_DIR/termy/termy"
elif [[ -f "$TEMP_DIR/termy-bin" ]]; then
  BINARY_PATH="$TEMP_DIR/termy-bin"
elif [[ -f "$TEMP_DIR/termy" ]]; then
  BINARY_PATH="$TEMP_DIR/termy"
else
  BINARY_PATH="$(find "$TEMP_DIR" \( -name "termy-bin" -o -name "termy" \) -type f -executable 2>/dev/null | head -n1)"
fi

if [[ -z "$BINARY_PATH" || ! -f "$BINARY_PATH" ]]; then
  die "Could not find termy binary in downloaded tarball"
fi

CLI_BINARY_PATH=""
if [[ -f "$TEMP_DIR/termy/termy-cli" ]]; then
  CLI_BINARY_PATH="$TEMP_DIR/termy/termy-cli"
else
  CLI_BINARY_PATH="$(find "$TEMP_DIR" -name "termy-cli" -type f -executable 2>/dev/null | head -n1)"
fi

if [[ -z "$CLI_BINARY_PATH" || ! -f "$CLI_BINARY_PATH" ]]; then
  die "Could not find termy-cli binary in downloaded tarball"
fi

mkdir -p "$INSTALL_DIR"

log "Installing to $INSTALL_DIR/termy..."
rm -f "$INSTALL_DIR/termy" "$INSTALL_DIR/termy-bin" "$INSTALL_DIR/termy-cli"
cp "$BINARY_PATH" "$INSTALL_DIR/termy-bin"
cp "$CLI_BINARY_PATH" "$INSTALL_DIR/termy-cli"
cat > "$INSTALL_DIR/termy" <<'LAUNCHER'
#!/usr/bin/env bash
set -euo pipefail

if [[ "${TERMY_LINUX_BACKEND:-x11}" == "x11" && -n "${DISPLAY:-}" ]]; then
  unset WAYLAND_DISPLAY
fi

exec "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/termy-bin" "$@"
LAUNCHER
chmod +x "$INSTALL_DIR/termy" "$INSTALL_DIR/termy-bin" "$INSTALL_DIR/termy-cli"

if [[ -d "$TEMP_DIR/termy/file-manager" ]]; then
  SHARE_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
  mkdir -p \
    "$SHARE_HOME/applications" \
    "$SHARE_HOME/kio/servicemenus" \
    "$SHARE_HOME/kservices5/ServiceMenus" \
    "$SHARE_HOME/nemo/actions" \
    "$SHARE_HOME/nautilus/scripts" \
    "$SHARE_HOME/caja/scripts"
  if [[ -f "$TEMP_DIR/termy/file-manager/termy.desktop" ]]; then
    cp "$TEMP_DIR/termy/file-manager/termy.desktop" "$SHARE_HOME/applications/termy.desktop"
  fi
  if [[ -f "$TEMP_DIR/termy/file-manager/termy-open-tab.desktop" ]]; then
    cp "$TEMP_DIR/termy/file-manager/termy-open-tab.desktop" "$SHARE_HOME/kio/servicemenus/termy-open-tab.desktop"
    cp "$TEMP_DIR/termy/file-manager/termy-open-tab.desktop" "$SHARE_HOME/kservices5/ServiceMenus/termy-open-tab.desktop"
  fi
  if [[ -f "$TEMP_DIR/termy/file-manager/termy-open-tab.nemo_action" ]]; then
    cp "$TEMP_DIR/termy/file-manager/termy-open-tab.nemo_action" "$SHARE_HOME/nemo/actions/termy-open-tab.nemo_action"
  fi
  if [[ -f "$TEMP_DIR/termy/file-manager/nautilus-open-tab.sh" ]]; then
    install -m 755 "$TEMP_DIR/termy/file-manager/nautilus-open-tab.sh" "$SHARE_HOME/nautilus/scripts/Open new Termy tab here"
    install -m 755 "$TEMP_DIR/termy/file-manager/nautilus-open-tab.sh" "$SHARE_HOME/caja/scripts/Open new Termy tab here"
  fi
fi

log "Termy $TAG installed successfully!"

if [[ ":$PATH:" != *":$INSTALL_DIR:"* ]]; then
  echo ""
  echo "NOTE: $INSTALL_DIR is not in your PATH."
  echo "Add it to your shell config:"
  echo ""
  echo "  # For bash (~/.bashrc):"
  echo "  export PATH=\"\$HOME/.local/bin:\$PATH\""
  echo ""
  echo "  # For zsh (~/.zshrc):"
  echo "  export PATH=\"\$HOME/.local/bin:\$PATH\""
  echo ""
  echo "  # For fish (~/.config/fish/config.fish):"
  echo "  set -gx PATH \$HOME/.local/bin \$PATH"
  echo ""
fi

echo ""
echo "Run 'termy' to start the terminal."
