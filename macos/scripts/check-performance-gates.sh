#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MACOS_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
REPO_ROOT="$(cd "$MACOS_DIR/.." && pwd)"

SUMMARY=""
RUN_COMPARE=0
RUN_NATIVE=0
BASELINE_ROOT=""
CANDIDATE_ROOT="$REPO_ROOT"
OUTPUT_ROOT="$REPO_ROOT/target/macos-performance-gate"
DURATION_SECS=5
GATE_ARGS=()

usage() {
  cat <<EOF
Usage: $0 (--summary PATH | --run-compare [options] | --native-render-metrics) [gate options]

Validate Termy benchmark output against soft regression gates.

Input options:
  --summary PATH          Existing benchmark-compare summary.json
  --run-compare           Run benchmark-compare before gating
  --native-render-metrics Run the native Swift render metrics gate and live terminal workload
  --baseline-root PATH    Baseline Termy repo root for --run-compare
  --candidate-root PATH   Candidate Termy repo root for --run-compare (default: repo root)
  --output PATH           Output directory for --run-compare (default: target/macos-performance-gate)
  --duration-secs SECS    Scenario duration for --run-compare (default: 5)

Gate options are forwarded to:
  cargo run -p xtask -- benchmark-gate

Useful gate options:
  --max-cpu-delta-percent N
  --max-memory-delta-mib N
  --max-frame-p95-delta-ms N
  --max-hitch-count-delta N
  --max-idle-wakeup-delta N
  --max-echo-p95-delta-ms N
  --max-echo-missed-delta N
  --min-displayed-frames N
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --summary)
      [[ $# -ge 2 ]] || { echo "Error: --summary requires a value" >&2; exit 2; }
      SUMMARY="$2"
      shift 2
      ;;
    --run-compare)
      RUN_COMPARE=1
      shift
      ;;
    --native-render-metrics)
      RUN_NATIVE=1
      shift
      ;;
    --baseline-root)
      [[ $# -ge 2 ]] || { echo "Error: --baseline-root requires a value" >&2; exit 2; }
      BASELINE_ROOT="$2"
      shift 2
      ;;
    --candidate-root)
      [[ $# -ge 2 ]] || { echo "Error: --candidate-root requires a value" >&2; exit 2; }
      CANDIDATE_ROOT="$2"
      shift 2
      ;;
    --output)
      [[ $# -ge 2 ]] || { echo "Error: --output requires a value" >&2; exit 2; }
      OUTPUT_ROOT="$2"
      shift 2
      ;;
    --duration-secs)
      [[ $# -ge 2 ]] || { echo "Error: --duration-secs requires a value" >&2; exit 2; }
      DURATION_SECS="$2"
      shift 2
      ;;
    --max-*|--min-*)
      [[ $# -ge 2 ]] || { echo "Error: $1 requires a value" >&2; exit 2; }
      GATE_ARGS+=("$1" "$2")
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      echo "Error: unknown option: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ "$RUN_NATIVE" -eq 1 ]]; then
  echo "==> Building libtermy FFI for native render metrics gate"
  (cd "$REPO_ROOT" && cargo build -p termy_ffi)
  echo "==> Checking native render metrics gate"
  (
    cd "$REPO_ROOT"
    TERMY_FFI_LIBRARY_PATH="$REPO_ROOT/target/debug" \
      swift test --package-path macos --filter NativeRenderMetricsGateTests
  )
fi

if [[ "$RUN_COMPARE" -eq 1 ]]; then
  [[ -n "$BASELINE_ROOT" ]] || {
    echo "Error: --run-compare requires --baseline-root" >&2
    exit 2
  }
  echo "==> Running benchmark compare"
  (
    cd "$CANDIDATE_ROOT"
    cargo run -p xtask -- benchmark-compare \
      --baseline-root "$BASELINE_ROOT" \
      --candidate-root "$CANDIDATE_ROOT" \
      --output "$OUTPUT_ROOT" \
      --duration-secs "$DURATION_SECS"
  )
  SUMMARY="$OUTPUT_ROOT/summary.json"
fi

if [[ "$RUN_NATIVE" -eq 1 && "$RUN_COMPARE" -eq 0 && -z "$SUMMARY" ]]; then
  exit 0
fi

[[ -n "$SUMMARY" ]] || {
  echo "Error: pass --summary PATH or --run-compare" >&2
  usage >&2
  exit 2
}
[[ -f "$SUMMARY" ]] || {
  echo "Error: summary not found: $SUMMARY" >&2
  exit 2
}

echo "==> Checking performance gates"
# ${arr[@]+...} guard: empty-array expansion under `set -u` errors on bash 3.2 (macOS /bin/bash).
(cd "$REPO_ROOT" && cargo run -p xtask -- benchmark-gate --summary "$SUMMARY" ${GATE_ARGS[@]+"${GATE_ARGS[@]}"})
