#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

EXPECTED=$'crates/engine/src/pty/native.rs\ncrates/engine/src/pty/process.rs\ncrates/ffi/src/lib.rs\ncrates/ffi/src/pty.rs\ncrates/ffi/src/terminal.rs\ncrates/ffi/src/types.rs\ncrates/ffi/src/util.rs'
ACTUAL="$(
  rg -l 'unsafe[[:space:]]+(extern|fn)|unsafe[[:space:]]*\{|#\[allow\(unsafe_code\)|#\[unsafe\(' \
    crates/*/src --glob '*.rs' | LC_ALL=C sort
)"

if [[ "$ACTUAL" != "$EXPECTED" ]]; then
  echo "production unsafe-code inventory changed; review UNSAFE_AUDIT.md" >&2
  echo "expected:" >&2
  echo "$EXPECTED" >&2
  echo "actual:" >&2
  echo "$ACTUAL" >&2
  exit 1
fi

echo "unsafe boundary inventory matches the reviewed PTY and FFI surfaces"
