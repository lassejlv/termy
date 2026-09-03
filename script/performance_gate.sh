#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SAMPLES="${TMON_BENCHMARK_SAMPLES:-30}"
OUTPUT="${TMON_BENCHMARK_OUTPUT:-$ROOT_DIR/performance/results/tmon-metal-gate-$(date +%s).json}"

while (( $# > 0 )); do
  case "$1" in
    --samples)
      SAMPLES="${2:?--samples requires a positive integer}"
      shift 2
      ;;
    --output)
      OUTPUT="${2:?--output requires a path}"
      shift 2
      ;;
    *)
      echo "usage: $0 [--samples N] [--output PATH]" >&2
      exit 2
      ;;
  esac
done

cd "$ROOT_DIR"
cargo test --release --workspace
bash script/test_ffi_c.sh
cargo run --release -p engine --example stability
cargo test --release -p render retained_history_scroll_benchmark -- --ignored --nocapture
cargo test --release -p render retained_dense_cjk_benchmark -- --ignored --nocapture
exec bash script/benchmark_metal.sh --samples "$SAMPLES" --output "$OUTPUT"
