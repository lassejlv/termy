# Tmon Metal performance contract

The native benchmark exercises the real `MetalRenderer`, swapchain, glyph atlas, terminal engine,
and multiplexer fixture from the already packaged `dist/Tmon.app` candidate. Run it with the window
visible and unobscured, keep the machine on AC power, leave the display at its fixed refresh mode,
and avoid interacting with the benchmark windows:

```sh
bash script/benchmark_metal.sh --samples 30
```

For the longer release check, including deterministic workspace, ABI, bounded-memory, retained
scroll, and dense-CJK gates:

```sh
bash script/performance_gate.sh --samples 30
```

Pass `--binary PATH` only when intentionally measuring another packaged candidate. The scripts
refuse to overwrite an existing report path. The command prints a short summary and writes
versioned JSON under `performance/results`. Cold
ASCII records the first atlas population; every warm workload gets an unrecorded warm-up frame.
Two consecutive runs are required for a release comparison. Compare like-for-like hardware,
display mode, scale, window/grid, font, build profile, warm-up policy, and sample count.
Each report records the application and bundle-build, terminal-snapshot, and mux-protocol versions
plus the Git revision and dirty state embedded when the package was compiled. The report therefore
describes the tested executable rather than whatever checkout happens to be the current directory.
The benchmark writes a `release_gate` decision and exits nonzero when workload sample coverage is
incomplete, any warm workload exceeds the detected display's CPU-p95 or renderer-p99 budget, or
idle, occluded, or inactive-tab behavior does work. The JSON is written before failure so a
rejected run remains diagnosable.

## Reference machine and budgets

The current reference is the checked MacBook Pro (`Mac16,1`), Apple M4, 24 GB RAM, macOS 27.0
build `26A5425a`, at 120 Hz and 2x scale with Menlo 15 pt and a 2000x1280 physical-pixel surface
(normally a 108x34 grid). The JSON also carries a 60 Hz comparison budget. Older checked reports
retain the OS/build they actually measured; do not relabel them as current-reference evidence.

Warm-workload release blockers are:

| Display | Warm CPU preparation p95 | Warm renderer p99 |
| --- | ---: | ---: |
| 120 Hz | <= 5.833 ms | <= 16.667 ms |
| 60 Hz | <= 11.667 ms | <= 33.333 ms |

Cold-atlas latency is reported but is not a timing blocker. A release must also keep idle,
occluded, and inactive-tab observations at zero presented frames, text prepares, and uploads;
avoid deterministic allocation growth after warm-up; and retain bounded terminal/PTY queues and
scrollback memory.

The renderer p99 allowance is two refresh intervals because swapchain acquisition is vsync-paced
and an individual sample can straddle the compositor boundary. The stricter
`missed_refresh_deadlines` counter still records every renderer frame exceeding one interval.
Native-tab `pipeline_end_to_end` includes AppKit activation and is reported separately from the
renderer budget.

The native harness also forces same-window surface recreation and alternates the renderer between
1x and 2x scale before restoring the connected display's real scale. Those deterministic paths
exercise surface-loss recovery and scale-dependent font/grid rebuilds, but they do not substitute
for the packaged-app display matrix: a release still needs recorded Retina/non-Retina moves,
refresh-rate changes, sleep/wake, and display disconnect/reconnect on physical displays.

Timing thresholds stay out of generic CI because compositor and system variance are material.
CI-safe invariants cover row-move semantics, copied-cell counts, row-local text/static-geometry
reuse, cursor/selection isolation, ASCII/complex shaping classification, geometry coordinates,
resize pacing, multiplexer backpressure/resynchronization, and C ABI layout/use.

## Reports

- `results/phase1-baseline-run1.json` and `results/phase1-baseline-run2.json` are the pre-retention
  measurements.
- `results/phase5-final-run1.json` and `results/phase5-final-run2.json` are the matched final
  measurements.
- `results/diagnostic.json` is a one-sample live Metal validation of the retained row pipelines;
  it is not used as the release comparison.

The complete before/after table, deterministic work reductions, tradeoffs, and remaining hardware
limitations are in `final-report.md`.

## What is measured

Every sample records multiplexer wake-to-drain and decode, terminal feed, frame extraction/cell
copying, retained apply/shaping, surface acquisition, viewport updates, glyph preparation,
static/dynamic geometry build and upload, encoding, submission, presentation, total CPU
preparation, and pipeline end-to-end time. Work counters include row updates/moves, cells copied,
row shapes/reuse, ASCII versus complex shaping, row-local text prepares, static versus dynamic
geometry work, transform writes, bytes uploaded, capacity growth, coalesced updates, skipped and
presented frames, retries, and missed refresh intervals. The report also samples process CPU/RSS,
terminal capacity, hardware, OS, adapter, display, font, and grid conditions.

`glyphon` 0.12 does not expose atlas texture dimensions, and this adapter contract does not enable
Metal timestamp queries. Atlas pressure is therefore bounded and observed through row-local
preparation, the 120-preparation trim generation, allocation/RSS stability, and cold/warm results;
GPU execution must be inspected with Instruments Metal System Trace when required. The report does
not invent either value.

## Text and memory policy

Ordinary, unstyled, single-cell ASCII rows use basic monospace shaping. Combining marks, emoji,
CJK/wide cells, fallback-sensitive content, bold, and italic retain advanced shaping. Font
ligatures are intentionally outside the ASCII fast-path contract; complex/styled text continues
through the correctness path.

The daily-driver scrollback limit remains 5,000 rows by default (100,000 maximum), inactive tabs
default to 1,000 rows, PTY pending output remains bounded at 512 KiB, and the renderer uses
power-of-two row-buffer capacities plus periodic atlas trimming. Any future fixed renderer-memory
cap should be based on measured atlas growth from an API or trace that can report it accurately.
