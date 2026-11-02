#!/usr/bin/env bash
# Build libtermy (the termy_core cdylib) and the termy-session-host binary that
# runs multiplexer panes, and stage both in sdk/swift/lib so the Swift package
# can link and load them.
#
# Usage: scripts/build-libtermy.sh [debug|release] [--universal]   (default: release)
#   --universal  build for Apple Silicon and Intel and stage fat binaries
#                (needs the aarch64- and x86_64-apple-darwin Rust targets)
set -euo pipefail

profile="${1:-release}"
universal=0
[[ "${2:-}" == "--universal" ]] && universal=1
case "$profile" in
  debug) cargo_flags=() ;;
  release) cargo_flags=(--release) ;;
  *) echo "usage: $0 [debug|release]" >&2; exit 64 ;;
esac

package_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
repo_root="$(cd "$package_dir/../.." && pwd)"
target_dir="${CARGO_TARGET_DIR:-$repo_root/target}"
lib_dir="$package_dir/lib"

build() {
  cargo build ${cargo_flags[@]+"${cargo_flags[@]}"} -p termy_core --lib --bin termy-session-host \
    --manifest-path "$repo_root/Cargo.toml" "$@"
}

case "$(uname -s)" in
  Darwin) library="libtermy_core.dylib" ;;
  Linux) library="libtermy_core.so" ;;
  *) echo "unsupported platform: $(uname -s)" >&2; exit 1 ;;
esac

mkdir -p "$lib_dir"
if [[ "$universal" -eq 1 ]]; then
  [[ "$library" == *.dylib ]] || { echo "--universal is for macOS" >&2; exit 64; }
  targets=(aarch64-apple-darwin x86_64-apple-darwin)
  for target in "${targets[@]}"; do build --target "$target"; done
  lipo -create -output "$lib_dir/$library" \
    "$target_dir/aarch64-apple-darwin/$profile/$library" "$target_dir/x86_64-apple-darwin/$profile/$library"
  lipo -create -output "$lib_dir/termy-session-host" \
    "$target_dir/aarch64-apple-darwin/$profile/termy-session-host" "$target_dir/x86_64-apple-darwin/$profile/termy-session-host"
else
  build
  cp "$target_dir/$profile/$library" "$lib_dir/$library"
fi

if [[ "$library" == *.dylib ]]; then
  # Load through the host's runpath so apps can bundle the dylib in
  # Contents/Frameworks instead of depending on the build directory.
  install_name_tool -id "@rpath/$library" "$lib_dir/$library"
  codesign --force --sign - "$lib_dir/$library" >/dev/null 2>&1 || true
fi

if [[ "$universal" -eq 0 ]]; then
  cp "$target_dir/$profile/termy-session-host" "$lib_dir/termy-session-host"
fi

echo "staged $lib_dir/$library and termy-session-host ($profile$([[ $universal -eq 1 ]] && echo ", universal"))"
