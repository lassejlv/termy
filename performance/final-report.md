# Tmon retained-rendering final report

Date: 2026-09-02. Reference: Apple M4 MacBook Pro (`Mac16,1`), 24 GB RAM, macOS
26.6.2, 120 Hz, 2x scale, Menlo 15 pt, release LTO, 2000x1280 pixels, 108x34 cells.

The matched inputs are `results/phase1-baseline-run1.json` and
`results/phase1-baseline-run2.json` versus `results/phase5-final-run1.json` and
`results/phase5-final-run2.json`. Each warm workload has 12 recorded frames after one unrecorded
warm-up. Values below are the two-run CPU-preparation p95 range in milliseconds.

| Workload | Baseline p95 | Final p95 |
| --- | ---: | ---: |
| `cold_ascii` | 1.737-2.356 | 1.328-13.367 |
| `sparse_shell_typing` | 0.381-0.411 | 0.379-0.390 |
| `one_line_full_screen_scroll` | 1.360-1.384 | 0.359-0.384 |
| `bursty_compiler_output` | 1.428-1.446 | 0.686-0.736 |
| `rapid_full_screen_tui` | 0.567-0.572 | 0.671-0.741 |
| `history_scroll` | 0.298-0.363 | 0.313-0.323 |
| `dense_cjk_emoji` | 1.470-2.674 | 1.196-1.201 |
| `box_braille_heavy` | 0.455-0.501 | 0.420-0.446 |
| `cursor_blink` | 0.205-0.312 | 0.230-0.248 |
| `selection_drag` | 0.205 | 0.254-0.292 |
| `resize` | 0.399-0.434 | 0.678-0.785 |
| `surface_only_resize` | not recorded | 0.210-0.225 |
| `native_tab_switch` | 0.271-0.312 | 0.276-0.486 |
| `multiplexer_output` | 0.464-0.575 | 0.384-0.499 |

All warm CPU p95 values are below the 5.833 ms 120 Hz budget. All warm renderer p99 values are
below the two-refresh 16.667 ms budget; the worst was 14.464 ms for one dense-CJK sample. The
stricter one-refresh counter recorded one final warm miss in run 1 and zero in run 2. The two runs
have the same deterministic work counts and the same surface-acquisition-dominant warm stage
ranking. Cold first-frame cost remains intentionally variable because it includes pipeline and
atlas population; run 1 recorded 13.367 ms and run 2 recorded 1.328 ms.

## Work reduction

- Twelve output-scroll frames changed from 408 row payloads, 44,064 copied cells, and 396 rebuilt
  rows to 12 row moves, 24 row payloads, 1,500 copied cells, and 24 rebuilt rows. Live CPU p95 fell
  by roughly 72-74%. The focused engine contract proves the simpler one-line case as one move plus
  one exposed-row update.
- The 50,000-line sustained engine gate moved 2.45 million retained rows while copying 32,538,000
  cells, down from the 416,638,000-cell baseline (92.2% less). Its 74.0 MiB raw cell capacity
  remained stable and throughput reached 311,075.7 lines/s on this run.
- Cursor blink and selection frames performed zero row shaping, zero text preparation, and zero
  static-geometry writes. Blink used six tiny overlay writes across 12 toggles; selection used one
  dynamic write per frame.
- The TUI workload uploaded 6,528 bytes instead of 26,496 bytes (75.4% less). The box/Braille
  workload uploaded 113,600 bytes instead of 1,986,912 bytes (94.3% less).
- Surface-only resize and native-tab retargeting performed zero text preparation and zero static or
  dynamic geometry builds/writes. They changed only the row-transform buffer. Font/scale and real
  grid-size changes retain the explicit full rebuild path.
- The dense-CJK manual gate holds one grouped wide-glyph buffer per compatible row rather than 60
  buffers per 120-column row, and completed at 0.237 ms per changed row in the final release run.
- Final Metal runs peaked at 115.6 MiB and 111.7 MiB RSS. Both ended near their sampled peak with
  identical terminal cell capacity, and every idle, occluded, and inactive-tab observation passed
  with zero presentation, text preparation, and upload work.

## Tradeoffs and limitations

The per-row renderer adds draw/viewport bookkeeping. Rapid TUI, selection, full grid resize, and
native-tab CPU p95 are modestly higher than the old whole-frame implementation, but remain below
0.8 ms except native activation variance and are far inside the agreed CPU budget. The gains are
algorithmic: scroll, static geometry, and text work now scale with changed rows, while cursor,
selection, surface resize, and tab retarget no longer invalidate unrelated content.

Exact atlas texture dimensions are unavailable from `glyphon` 0.12, so atlas pressure is bounded
by row-local resources, a 120-preparation trim generation, cold/warm results, and RSS/allocation
stability. Metal GPU timestamps are not enabled by the current adapter contract; use Instruments
Metal System Trace for GPU execution. Intel and older macOS runtime behavior remains a separate
hardware-validation item and is not inferred from this Apple Silicon run.
