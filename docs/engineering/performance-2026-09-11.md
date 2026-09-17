# Desktop performance validation — 2026-09-11

## Changes

- macOS resolves the configured installed font directly through CoreText, then
  checks its actual GPUI glyph advances. Missing or proportional fonts still use
  the existing catalog and monospace fallback path. Linux fontconfig handling is
  unchanged.
- Packaged launches use the default icon already provided by macOS. An existing
  Finder custom icon or a selected alternate icon retains the explicit update
  path. Reloading unrelated settings no longer reapplies the same icon.
- The shared grid renderer retains source rows, draw operations, and shaped
  text. It can recognize unchanged and shifted rows even when the engine reports
  full damage. Row equality includes colors, styles, selection, combining text,
  and wide-cell state; cursor and hovered-link decorations are checked separately.
- Tmon scroll damage moves cached rows directly, including partial regions and
  successive scrolls. Exposed rows and displaced cursor decorations are rebuilt.
- Background fills use direct comparisons instead of retaining a hash map of
  resolved colors. Hidden-pane eviction releases the retained source rows too.
- The opt-in launch probe records individual startup stages alongside the first
  usable frame, making future startup regressions easier to locate.

The shared renderer benefits native Alacritty, experimental Tmon, and tmux.
Explicit scroll metadata is currently supplied by Tmon. GPU painting still
draws all visible rows each frame, as required by GPUI.

## Measurement setup

Measurements use macOS on the same machine, release binaries, an isolated
configuration, and identical workloads. The baseline was saved before these
performance changes and includes the preceding Kitty, selection, and TUI
background fixes. It is not a clean checkout of the older repository HEAD.

Local artifacts are under `target/performance-2026-09-11/`, including the saved
baseline and candidate binaries, startup results, workload summaries, and frame
timelines. The startup comparison uses matching signed `.app` bundles with the
same bundled icon, alternates their run order, and measures the entire process
tree after idle settling. Timings start in the process and end at the first
usable terminal frame; they do not include macOS Gatekeeper or download time.

## Results

| Measurement | Baseline | Optimized | Change |
| --- | ---: | ---: | ---: |
| Packaged startup, median of 5 alternating runs | 228 ms | 148 ms | 35% faster |
| Settled process-tree RSS, median of 5 runs | 146.2 MiB | 94.8 MiB | 35% lower |
| Scroll row preparation, 240 frames at 48 × 120 cells | 37.45 ms | 9.43 ms | 75% less CPU time |

Startup samples were 227, 227, 237, 228, and 232 ms for the baseline, and 141,
139, 150, 148, and 149 ms for the candidate. Idle CPU rounded to 0.00% for both
medians, so there is no claimed idle CPU reduction. Font setup alone fell from
approximately 43 ms to under 2 ms in the instrumented runs.

The row-preparation comparison allocates fresh input rows for both paths and
times only preparation of cached paint operations. It checks identical final
output; no faster-time assertion makes tests depend on machine load. These gains
do not establish a 4× increase in application FPS or a 75% reduction in total
application CPU. The measured startup improvement is 35%, not a claimed halving.

Validation passed: 1,260 tests across the desktop app, terminal UI, and core;
release build; strict Clippy; formatting; diff whitespace checks; and repository
boundaries. Eight pre-existing tmux integration tests require an explicit local
tmux run and remain ignored. Native resize and selection checks passed with the
visibility limitation described below.

## Correctness and limits

The row-cache regression compares full rebuilds with retained rows using fresh
cell allocations, colors, and repeated scrolling. Additional tests cover both
scroll directions, complete and partial regions, multiple scrolls in one update,
invalid batches, cursor transitions, and hover changes. Existing glyph, selection,
Kitty image, and background tests also run.

Native QA uses an isolated packaged app and a synthetic alternate-screen fixture:
240 scroll updates, fixed header and footer, bold and underlined text, wide and
combining characters, selection, and window resizing. Screenshots are retained
in `/tmp/termy-performance-qa/`. This is a synthetic check, not an authenticated
OpenCode, Cursor, or Grok session.

Automated desktop frame-rate runs can become occluded and stop painting while
terminal output continues. Native captures also needed a resize to restore
painting. Whole-run FPS or CPU from those runs is therefore not evidence of a
performance improvement, and this work does not claim to resolve that visibility
limitation. The row-preparation benchmark isolates CPU work and does not claim
the same multiplier for overall application CPU or GPU rendering.

## Reproduction

```sh
cargo test --locked -p termy -p termy -p termy_core
cargo clippy --locked -p termy -p termy -p termy_core --all-targets -- -D warnings
cargo build --locked --release -p termy --bin termy
cargo test --locked --release -p termy repeated_full_damage_scrolling -- --nocapture
bash scripts/check-boundaries.sh
```

Use `scripts/check-gpui-launch-idle.sh --binary /path/to/Termy.app/Contents/MacOS/termy`
for packaged startup and idle measurements. Preserve its default thresholds for
normal acceptance runs; the comparative experiment permits larger limits so
both baseline and candidate measurements are recorded rather than truncated by
an early failure.
