#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SAMPLES="${TMON_BENCHMARK_SAMPLES:-30}"
OUTPUT="${TMON_BENCHMARK_OUTPUT:-$ROOT_DIR/performance/results/tmon-metal-$(date +%s).json}"

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

if [[ ! "$SAMPLES" =~ ^[1-9][0-9]*$ ]]; then
  echo "sample count must be a positive integer" >&2
  exit 2
fi

mkdir -p "$(dirname "$OUTPUT")"
cd "$ROOT_DIR"
exec cargo run --release -p tmon -- \
  --benchmark-metal \
  --samples "$SAMPLES" \
  --output "$OUTPUT"
