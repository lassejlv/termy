#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SECONDS_PER_TARGET="${TMON_FUZZ_SECONDS_PER_TARGET:-30}"
RSS_LIMIT_MB="${TMON_FUZZ_RSS_LIMIT_MB:-2048}"
FUZZ_TOOLCHAIN="${TMON_FUZZ_TOOLCHAIN:-nightly}"

if ! [[ "$SECONDS_PER_TARGET" =~ ^[1-9][0-9]*$ ]]; then
    echo "TMON_FUZZ_SECONDS_PER_TARGET must be a positive integer" >&2
    exit 2
fi

if ! [[ "$RSS_LIMIT_MB" =~ ^[1-9][0-9]*$ ]]; then
    echo "TMON_FUZZ_RSS_LIMIT_MB must be a positive integer" >&2
    exit 2
fi

if ! command -v cargo-fuzz >/dev/null 2>&1; then
    echo "cargo-fuzz is required; install the pinned release with:" >&2
    echo "  cargo install cargo-fuzz --version 0.13.2 --locked" >&2
    exit 1
fi

if ! rustup run "$FUZZ_TOOLCHAIN" rustc --version >/dev/null 2>&1; then
    echo "Rust toolchain '$FUZZ_TOOLCHAIN' is required for libFuzzer instrumentation" >&2
    exit 1
fi

cd "$ROOT_DIR"

for target in terminal-feed snapshot-decode mux-frame-decode; do
    echo "fuzzing $target for ${SECONDS_PER_TARGET}s"
    cargo "+$FUZZ_TOOLCHAIN" fuzz run "$target" -- \
        -max_total_time="$SECONDS_PER_TARGET" \
        -timeout=5 \
        -rss_limit_mb="$RSS_LIMIT_MB" \
        -verbosity=0 \
        -print_final_stats=1
done
