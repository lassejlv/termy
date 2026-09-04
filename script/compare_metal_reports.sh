#!/usr/bin/env bash
set -euo pipefail

ALLOW_DIRTY=0
if [[ "${1:-}" == "--allow-dirty" ]]; then
  ALLOW_DIRTY=1
  shift
fi
if (( $# != 2 )); then
  echo "usage: $0 [--allow-dirty] RUN1.json RUN2.json" >&2
  exit 2
fi

RUN1="$1"
RUN2="$2"
if [[ "$RUN1" == "$RUN2" ]]; then
  echo "the two Metal reports must be distinct files" >&2
  exit 1
fi
for REPORT in "$RUN1" "$RUN2"; do
  if [[ ! -f "$REPORT" ]]; then
    echo "missing Metal report: $REPORT" >&2
    exit 1
  fi
done

extract() {
  plutil -extract "$2" raw -o - "$1"
}

for REPORT in "$RUN1" "$RUN2"; do
  if [[ "$(extract "$REPORT" report_version)" != "3" ]]; then
    echo "Metal report is not the enforced schema version 3: $REPORT" >&2
    exit 1
  fi
  if [[ "$(extract "$REPORT" release_gate.passed)" != "true" ]]; then
    echo "Metal release gate did not pass: $REPORT" >&2
    exit 1
  fi
  if (( ALLOW_DIRTY == 0 )) && [[ "$(extract "$REPORT" source.source_dirty)" != "false" ]]; then
    echo "production Metal evidence must come from a clean source tree: $REPORT" >&2
    exit 1
  fi
done

COMPARABLE_FIELDS=(
  source.application_version
  source.bundle_build_number
  source.source_revision
  source.source_dirty
  source.terminal_snapshot_version
  source.mux_protocol_version
  conditions.metal_adapter
  conditions.display_scale
  conditions.display_refresh_hz
  conditions.window_pixels.0
  conditions.window_pixels.1
  conditions.grid.0
  conditions.grid.1
  conditions.font_family
  conditions.font_size_points
  conditions.warmup_policy
  conditions.samples_per_warm_workload
  conditions.build_profile
  release_gate.evaluated_refresh_hz
  release_gate.cpu_preparation_p95_budget_ns
  release_gate.renderer_end_to_end_p99_budget_ns
)
for FIELD in "${COMPARABLE_FIELDS[@]}"; do
  FIRST_VALUE="$(extract "$RUN1" "$FIELD")"
  SECOND_VALUE="$(extract "$RUN2" "$FIELD")"
  if [[ "$FIRST_VALUE" != "$SECOND_VALUE" ]]; then
    echo "Metal reports are not comparable at $FIELD: $FIRST_VALUE != $SECOND_VALUE" >&2
    exit 1
  fi
done

FIRST_TIMESTAMP="$(extract "$RUN1" generated_unix_seconds)"
SECOND_TIMESTAMP="$(extract "$RUN2" generated_unix_seconds)"
if [[ ! "$FIRST_TIMESTAMP" =~ ^[0-9]+$ || ! "$SECOND_TIMESTAMP" =~ ^[0-9]+$ ]]; then
  echo "Metal report timestamps are not integers" >&2
  exit 1
fi
if (( SECOND_TIMESTAMP <= FIRST_TIMESTAMP )); then
  echo "the second Metal report must be generated after the first" >&2
  exit 1
fi

echo "two-run Metal release evidence is passing, ordered, and comparable"
if (( ALLOW_DIRTY == 1 )); then
  echo "result: internal-only comparison; dirty evidence was permitted"
fi
