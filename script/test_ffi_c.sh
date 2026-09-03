#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
target_dir="$repo_root/target/debug"
smoke_binary="$target_dir/ffi-c-smoke"

cargo build --manifest-path "$repo_root/Cargo.toml" -p ffi

case "$(uname -s)" in
  Darwin)
    library_name="libtmon_ffi.dylib"
    ;;
  Linux)
    library_name="libtmon_ffi.so"
    ;;
  *)
    echo "unsupported platform for the C ABI smoke test" >&2
    exit 1
    ;;
esac

cc -std=c11 -Wall -Wextra -Werror \
  -I "$repo_root/crates/ffi/include" \
  "$repo_root/crates/ffi/tests/c_smoke.c" \
  -L "$target_dir" -ltmon_ffi \
  -Wl,-rpath,"$target_dir" \
  -o "$smoke_binary"

"$smoke_binary"
