#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

expect_failure() {
    local name="$1"
    shift
    local output
    if output="$(env -u TERMY_RENDER_REPORT_PATH "$@" "$SCRIPT_DIR/check-render-perf.sh" --skip-build 2>&1)"; then
        echo "render-perf-regressions: $name unexpectedly passed" >&2
        printf '%s\n' "$output" >&2
        exit 1
    fi
    echo "render-perf-regressions: $name failed as expected"
}

if ! "$SCRIPT_DIR/check-render-perf.sh"; then
    echo "render-perf-regressions: primary gate failed; retrying (1/2)" >&2
    if ! "$SCRIPT_DIR/check-render-perf.sh" --skip-build; then
        echo "render-perf-regressions: primary gate failed again; retrying (2/2)" >&2
        "$SCRIPT_DIR/check-render-perf.sh" --skip-build
    fi
fi
expect_failure "forced full redraw" TERMY_BENCHMARK_FORCE_FULL=1
expect_failure "artificial render delay" TERMY_BENCHMARK_BUILD_DELAY_MICROS=5000

echo "Render performance regression fixtures passed"
