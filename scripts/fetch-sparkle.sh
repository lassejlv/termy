#!/usr/bin/env bash
set -euo pipefail

# Download the pinned Sparkle release (framework + signing tools) into
# target/sparkle/<version> and print that directory. Reuses an earlier
# download when it is already there.

SPARKLE_VERSION="2.10.0"
SPARKLE_SHA256="c2bf58aa8387266ac179357b1415d6f2635f044da8be41042af32425dae6da0c"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
DEST="$REPO_ROOT/target/sparkle/$SPARKLE_VERSION"

die() { echo "Error: $*" >&2; exit 1; }

if [[ -d "$DEST/Sparkle.framework" && -x "$DEST/bin/sign_update" ]]; then
  echo "$DEST"
  exit 0
fi

archive="$(mktemp -t sparkle.XXXXXX)"
trap 'rm -f "$archive"' EXIT

echo "==> Downloading Sparkle $SPARKLE_VERSION" >&2
curl -fsSL -o "$archive" \
  "https://github.com/sparkle-project/Sparkle/releases/download/$SPARKLE_VERSION/Sparkle-$SPARKLE_VERSION.tar.xz"
echo "$SPARKLE_SHA256  $archive" | shasum -a 256 -c - >/dev/null \
  || die "Sparkle $SPARKLE_VERSION archive checksum mismatch"

rm -rf "$DEST"
mkdir -p "$DEST"
tar -xf "$archive" -C "$DEST"
[[ -d "$DEST/Sparkle.framework" ]] || die "Sparkle.framework missing from the archive"

echo "$DEST"
