# Development

## Crash Logs

Termy appends panic reports to a crash log before Rust's normal panic output
runs. The default location is:

- Linux: `${XDG_DATA_HOME:-~/.local/share}/termy/crash.log`
- Windows: `%LOCALAPPDATA%\termy\crash.log`
- macOS: `~/Library/Application Support/termy/crash.log`
- Fallback when no platform data directory is available: `./termy-crash.log`

Set `TERMY_CRASH_LOG_PATH=/path/to/crash.log` to override the location when
debugging or collecting a support artifact. Crash reports include the Termy
version, panic location, panic message, thread name, and a captured backtrace.

## Render metrics (debug-only)

Enable render churn metrics in debug builds:

```sh
RUST_LOG=info TERMY_RENDER_METRICS=1 cargo run -p termy
```

Note:
- Metrics logs are `debug_assertions`-only, so `--release` will not emit `render_metrics` lines.
- Counter meaning:
  - `full`: full per-pane cell cache rebuild decisions
  - `partial`: dirty-span patch decisions
  - `reuse`: no cell cache update decisions
  - `dirty_span`: number of dirty spans consumed during partial updates
  - `patched_cell`: number of cells patched from dirty spans
  - `grid_paint` / `shape_line`: paint + text shaping work done that interval
- Cursor-blink sanity check: `full` should stay near `0`; `reuse` or small `partial` values are expected depending on reported terminal damage.

## Render benchmarks

Prerequisites: macOS with `xctrace` / Activity Monitor installed. This harness
is not supported on Linux or Windows.

Run the automated real-window render benchmark compare on macOS:

```sh
cargo run -p termy_cli --bin xtask -- benchmark-compare \
  --baseline termy:/path/to/termy/worktree \
  --candidate ghostty:/Applications/Ghostty.app \
  --output /tmp/termy-benchmark-compare
```

This builds the release `termy` binary for any Termy target, launches real app
windows with deterministic benchmark scenarios, records an Activity Monitor
trace with `xctrace`, and writes a comparison report.

Target specs use `kind:/path`:

- `termy:/path/to/worktree`
- `ghostty:/path/to/ghostty`
- `ghostty:/Applications/Ghostty.app`

Legacy Termy-only compare syntax still works:

```sh
cargo run -p termy_cli --bin xtask -- benchmark-compare \
  --baseline-root /path/to/baseline/worktree \
  --candidate-root /path/to/candidate/worktree \
  --output /tmp/termy-benchmark-compare
```

Notes:

- Shared report tables use external Activity Monitor data so mixed-app runs stay
  comparable.
- Shared frame cadence now comes from a second `Animation Hitches` trace pass,
  so `Displayed frame p50/p95/p99`, `Displayed FPS avg`, and hitch counts work
  for both Termy and Ghostty.
- Termy emits additional in-app frame/redraw diagnostics; Ghostty runs do not,
  so those appear as app-specific diagnostics rather than headline metrics.
- Ghostty benchmark targets require Ghostty `>= 1.2.0`.
- Ghostty runs use a generated config with `initial-command = direct:...` and
  launch via `--config-default-files=false --config-file=...` instead of `-e`.

Artifacts are written under the chosen output directory:

- `report.md`: human-readable comparison summary
- `summary.json`: machine-readable top-level summary
- `raw/<build>/<scenario>/app/summary.json`: in-app aggregate metrics for a run
- `raw/<build>/<scenario>/app/timeline.ndjson`: sampled frame/CPU timeline
- `raw/<build>/<scenario>/app/frames.ndjson`: per-frame presentation events
- `raw/<build>/<scenario>/driver/markers.ndjson`: benchmark-driver event markers
- `energy/<build>/<scenario>/activity-monitor.trace`: raw `xctrace` recording
- `energy/<build>/<scenario>/energy.json`: parsed Activity Monitor summary
- `animation/<build>/<scenario>/animation-hitches.trace`: raw `Animation Hitches` recording
- `animation/<build>/<scenario>/animation-summary.json`: parsed external frame summary

For non-Termy targets, the `raw/<build>/<scenario>/app/` directory may be
empty because no in-app diagnostics are available.

Current scenarios:

- `idle-blink`
- `idle-burst`
- `echo-train`
- `steady-scroll`
- `alt-screen-anim`

## Tmux integration tests

Run the local end-to-end tmux split integration harness:

```sh
just test-tmux-integration
```

Requirements:
- tmux `>= 3.3`

Optional:
- Override tmux binary path with `TERMY_TEST_TMUX_BIN=/path/to/tmux`
