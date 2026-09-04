#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SECONDS="${TMON_SOAK_SECONDS:-1800}"
OUTPUT="${TMON_SOAK_OUTPUT:-$ROOT_DIR/performance/results/soak-$(date +%s).json}"

while (( $# > 0 )); do
  case "$1" in
    --seconds)
      SECONDS="${2:?--seconds requires a positive integer}"
      shift 2
      ;;
    --output)
      OUTPUT="${2:?--output requires a path}"
      shift 2
      ;;
    *)
      echo "usage: $0 [--seconds N] [--output PATH]" >&2
      exit 2
      ;;
  esac
done

if ! [[ "$SECONDS" =~ ^[1-9][0-9]*$ ]]; then
  echo "soak seconds must be a positive integer" >&2
  exit 2
fi

cd "$ROOT_DIR"
exec cargo run --locked --release -p mux --example soak -- \
  --seconds "$SECONDS" \
  --output "$OUTPUT"
