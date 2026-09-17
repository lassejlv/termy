# Tmon performance report

Date: 2026-08-05

This report covers the first focused optimization pass over Tmon. The original
Linux numbers came from commit `03d7ca5c5420ca141afe56f725341faadb71af18`.
All before/after percentages below use an additional same-machine macOS
baseline so host differences are not presented as engine improvements.

## Bytecode scrollback and direct Ghostty comparison

The 2026-08-05 follow-up keeps the active screen dense, but allocates the
alternate screen only when an application first enters it and releases it on a
terminal reset. It also replaces the first compact-history format with one
lossless bytecode `HistoryRow`. Logical width and stored cell count are
separate, exact-default suffixes are omitted, and ordinary characters remain
direct UTF-8. Sparse commands encode style changes, cell state, wide spacers,
inline combining marks, and rare metadata. Colored and attributed rows no
longer need a trimmed 24-byte-cell fallback. A logical row view decodes without
persistent dense storage; the few mutable history paths materialize temporarily
and re-encode immediately.

A conservative line path handles complete default-style `CRLF` rows only while
the cursor is at the bottom of a full-screen scroll region. On the primary
screen it moves cold rows directly into compact history and materializes only
the final viewport. On the alternate screen, the ASCII form rotates and
rewrites dense rows in place without allocating or retaining history. The
primary batched form also supports validated UTF-8 rows with wide cells and one
inline combining mark per cell. More complex input falls back to the normal
parser/grid path. Complete ordinary CSI sequences now bypass the byte-at-a-time
state machine, while private mode changes retain the scalar synchronized-update
handling. The byte encoder also has dedicated default-style and single-byte
ASCII paths.

The physical-memory result below is a local macOS ARM64 run with rustc 1.96.0,
the pinned Ghostty revision `9e30f70f23418fecbdca1088673000417527c4e4`,
120x40 cells, a 10,000-row scrollback limit, and ten fresh processes per case.
It uses `ri_phys_footprint` before semantic traversal. The prior Tmon column is
the hosted first-format result from run `31012483654`.

| Workload | Prior Tmon | Current Tmon | Ghostty | Ghostty compressed | Tmon below compressed |
| --- | ---: | ---: | ---: | ---: | ---: |
| Plain | 4.77 MiB | 2.75 MiB | 11.64 MiB | 3.44 MiB | 20.1% |
| Styled | 7.70 MiB | 2.72 MiB | 13.23 MiB | 3.87 MiB | 29.7% |
| Unicode | 6.77 MiB | 2.73 MiB | 12.63 MiB | 3.25 MiB | 16.0% |
| Mixed | 10.20 MiB | 3.34 MiB | 13.23 MiB | 4.17 MiB | 19.9% |

Tmon's empty and screen-only medians were 1.67 MiB, below Ghostty's 1.69 MiB
empty median and 1.73-1.75 MiB screen medians. Alternate plain, alternate
mixed, TUI redraw, and cursor-edit states measured 1.83, 1.80, 1.81, and
1.67 MiB for Tmon versus 1.88, 1.88, 1.86, and 1.73 MiB for Ghostty. Tmon's
scrollback payload deltas were 1.08, 1.05, 1.06, and 1.67 MiB respectively,
versus compressed Ghostty's 1.75, 2.18, 1.56, and 2.48 MiB. The complete report
records every raw byte sample and P05/P95 range; cursor, history size, and
sampled semantic cells matched before each result was accepted. The benchmark
now fails unless Tmon's median is strictly lower than every corresponding
Ghostty result across all 13 states.

The expanded direct Tmon/Ghostty feed comparison used eight balanced samples
and a 2 MiB warmup. Scrollback uses identical 64 KiB calls and a 32 MiB timed
target, alternate rows use identical row-aligned calls and the same target, and
the more expensive TUI/edit traces use one identical operation batch per call
with 4 MiB and 2 MiB timed targets. Fresh complete-workload preflights matched
bytes, cursor, history size, and semantic digest:

| Workload | Timed target | Tmon | Ghostty | Median paired ratio |
| --- | ---: | ---: | ---: | ---: |
| Plain scrollback | 32 MiB | 1590.5 MiB/s | 903.0 MiB/s | 1.759x |
| Styled scrollback | 32 MiB | 249.6 MiB/s | 97.4 MiB/s | 2.564x |
| Unicode scrollback | 32 MiB | 296.4 MiB/s | 135.3 MiB/s | 2.182x |
| Mixed scrollback | 32 MiB | 219.6 MiB/s | 101.6 MiB/s | 2.163x |
| Plain alternate screen | 32 MiB | 1754.5 MiB/s | 1373.4 MiB/s | 1.269x |
| Mixed alternate screen | 32 MiB | 284.3 MiB/s | 115.4 MiB/s | 2.458x |
| TUI redraw | 4 MiB | 355.0 MiB/s | 315.7 MiB/s | 1.125x |
| Cursor and line edits | 2 MiB | 188.1 MiB/s | 30.7 MiB/s | 6.121x |

These are integrated feed results, not a raw-parser or feature-parity score.
Ghostty's compression is an explicit post-feed operation, so it appears in the
memory comparison rather than its normal feed-throughput path. Every individual
paired ratio in this local acceptance run was above `1.000x`; the GitHub
workflow runs both measurements and rejects any workload whose median paired
Tmon/Ghostty throughput ratio is not above `1.000x`.

The repository's existing eight-sample Tmon/Alacritty parser gate also passed
on the same machine: plain `1.321x`, styled `2.539x`, Unicode `1.231x`, and
edits `1.413x`. Its exact full-grid preflight passed before timing.

## Result

The saved release measurements below used a 120x40 grid, 10,000 scrollback
lines, 32 MiB per parser sample, 7 samples, and 250 full-frame API calls per
snapshot sample. Seven is the historical sample count, not the current harness
default; CI now uses eight so every workload has balanced AB/BA sample order.

| Workload | Before Tmon | After Tmon | Final Alacritty | Tmon improvement | Final ratio |
| --- | ---: | ---: | ---: | ---: | ---: |
| Plain scrollback | 94.3 MiB/s | 236.8 MiB/s | 109.9 MiB/s | +151.1% | 2.15x |
| Styled TUI redraw | 158.1 MiB/s | 215.9 MiB/s | 135.1 MiB/s | +36.6% | 1.60x |
| Unicode + wide cells | 102.6 MiB/s | 148.2 MiB/s | 98.1 MiB/s | +44.4% | 1.51x |
| Cursor + line edits | 4.3 MiB/s | 6.6 MiB/s | 5.1 MiB/s | +53.5% | 1.28x |

The same final code was benchmarked twice. Ratios were 2.11x/2.15x for plain,
1.61x/1.60x for styled, 1.35x/1.51x for Unicode, and 1.24x/1.28x for edits.
Raw throughput moved with temperature and competing system load, but every
target remained above its required margin in both runs.

| Metric | Before | After | Change |
| --- | ---: | ---: | ---: |
| Tmon current full-frame API throughput | 125,310.6 calls/s | 268,084.4 calls/s | +113.9% |
| Static `Cell` size | 32 bytes | 24 bytes | -25.0% |
| Maximum cell storage at benchmark dimensions | about 36.9 MiB | about 27.7 MiB | -9.23 MiB |

The full-frame API result does not compare equivalent raw work. Tmon copies its
internal cells, while `termy_core` converts Alacritty cells into a `TermyFrame`.
The benchmark output now labels this explicitly. The saved same-machine
before/after result shows that Tmon's own throughput improved; it does not make
the cross-engine ratio a valid hosted-CI regression gate.

## Supplied Linux baseline

The target baseline used Linux x86_64 and rustc 1.97.1:

| Workload | Tmon | Alacritty | Ratio |
| --- | ---: | ---: | ---: |
| Plain scrollback | 36.8 MiB/s | 49.6 MiB/s | 0.74x |
| Styled TUI redraw | 72.3 MiB/s | 68.0 MiB/s | 1.06x |
| Unicode + wide cells | 46.9 MiB/s | 60.9 MiB/s | 0.77x |
| Cursor + line edits | 3.2 MiB/s | 3.3 MiB/s | 0.98x |

The repository's `Tmon vs Alacritty Benchmark` workflow remains the authority
for a Linux after-table. It runs on pushes to `main` that change its declared
Cargo, core, Tmon, xtask, revision-gate, or workflow paths, is manually
dispatchable, and uploads `tmon-alacritty-benchmark.txt`.

## Last hosted baseline before the current working tree

GitHub Actions run
[`30932203428`](https://github.com/lassejlv/termy/actions/runs/30932203428)
measured commit `8c6acfb5c465e9950e6e27ac7f2f544c7ea61698` on an AMD EPYC 7763 runner with
Rust 1.97.1. This is the last immutable hosted result before the currently
uncommitted follow-up work and is therefore the comparison point for the next
push, not evidence for the current working tree.

| Workload | Tmon | Alacritty | Median paired ratio | Target |
| --- | ---: | ---: | ---: | ---: |
| Plain scrollback | 121.2 MiB/s | 47.3 MiB/s | 2.515x | 1.050x |
| Styled TUI redraw | 105.1 MiB/s | 66.1 MiB/s | 1.589x | 1.050x |
| Unicode + wide cells | 66.2 MiB/s | 59.5 MiB/s | 1.114x | 1.000x |
| Cursor + line edits | 4.1 MiB/s | 3.0 MiB/s | 1.385x | 1.000x |

The parser/grid gate passed. Tmon's full-frame API measured 159,400.2 calls/s,
the report-only paired ratio was 19.076x, and retention against the supplied
19.880x ratio was 96.0%. Both engines used 24-byte cells. The artifact also
recorded zero allocation calls for Tmon's styled redraw feed loop and roughly
10,000 retained row allocations for each scrolling workload; these are
requested-byte allocator counters, not RSS measurements.

## Benchmark and profile audit

Both engines receive identical bytes, a 120x40 grid, a 10,000-line history
limit, alternating sample order, and release builds. The current default is
eight samples; odd values are rejected so every workload contains the same
number of Tmon-first and Alacritty-first pairs. Before any timed samples, the
benchmark feeds up to 32 MiB of every workload into fresh terminals and compares
every normalized grid and scrollback cell, cursor state, scroll state,
alternate-screen mode, and bracketed-paste mode. The saved metric uses the same
small payload-sized public `feed_output` calls as the baseline.

Parser decisions use the median of the eight per-sample Tmon/Alacritty ratios,
not the ratio of two independently aggregated medians. The report preserves the
individual pair ratios and their order, prints actual ratios and committed
targets to three decimals, and rejects non-finite target values. GitHub Actions
enforces the four parser/grid targets after the benchmark has written its
output.

Snapshot throughput remains report-only. The report shows retention against the
supplied 19.880x baseline, but does not enforce the tempting
`19.880x * 0.95 = 18.886x` threshold. That gate is not sound on hosted CI: Tmon
copies raw cells while `termy_core` constructs a `TermyFrame`, so the numerator
and denominator are different operations whose relative cost can move with the
runner. Retention is a trend to investigate, not an equivalent-work pass/fail
claim.

The harness now adds a separate renderer-neutral normalized frame for an honest
cross-engine comparison. Both paths allocate the same owned frame shape and
preserve visible cells, coherent metadata, raw colors, style flags, combining
text, exact underline styles and colors, hyperlink presence, distinct
wide-spacer roles, and wrapping. A mixed
common-subset fixture must compare exactly before timing, and every sample runs
for at least 250 ms. The first Linux Actions run establishes this new metric's
baseline; the invalid historical 19.880x ratio is not reused as its target.

The standalone gate described below was removed with the benchmark tools.
This section records its historical methodology; current CI runs only the
cross-engine comparison and allocation accounting.

The standalone gate measured current Tmon against immutable commit
`03d7ca5c5420ca141afe56f725341faadb71af18`. Both revisions execute their native
`Terminal::snapshot()` on identical state in the same process. Before timing,
an exact preflight checks each snapshot against independently visited terminal
state and the other revision. It uses 20 balanced interleaved blocks, adaptive
measurements of at least 250 ms per side, and paired throughput ratios. The
sixth-smallest ratio is its conservative lower bound and the fifteenth-smallest
is its upper bound: lower at or above 0.95 passes, upper below 0.95 is a clear
regression, and overlap is inconclusive. Both regression and inconclusive
results fail CI. This makes the no-more-than-5% requirement independent of
hosted-runner absolute speed without pretending the older cross-engine ratio
was comparable work.

The Actions report is initialized under `runner.temp` before checkout. The
timed comparison, immutable snapshot gate, and subsequent allocation-only pass
append through Bash `pipefail` pipelines. An `always()` finalizer records the
outcome of checkout, toolchain and cache setup, baseline checkout, the engine
comparison, the snapshot gate, and allocation accounting. If the runner remains
available, the report is still added to the job summary and uploaded even when
an earlier step fails. Allocation accounting also caps total parser work at
1,024 MiB, so an invalid oversized manual input fails quickly enough for the
finalizer to preserve the report.

It is an integrated runtime comparison, not a raw-parser comparison. Tmon feeds
its engine directly. Alacritty is exercised through `termy_core`, including its
locking and graphics-interception boundary. The report identifies both selected
runtimes explicitly, and neither path is selected through the desktop engine
environment. The payload calls are also smaller than normal PTY reads. These
constraints are printed in the report rather than hidden by a broader claim.

Pre-change Time Profiler samples identified these hotspots:

- Plain: `put_char` 28.1% leaf time, row shifting 17.5%, `write_cell_at` 15.4%,
  and parser advance 15.1%. Parser advance was 94.5% inclusive.
- Unicode: `put_char` 25.2%, parser advance 21.4%, row shifting 17.4%, cell
  writes 10.9%, and UTF-8 conversion 4.9%.
- Edits: erase-display 68.5%, upward row shifts 23.1%, and downward shifts
  1.6%.

## Accepted changes

1. Full damage no longer clears every partial-damage slot repeatedly. Cleanup
   happens once when the full damage snapshot is consumed.
2. Upward and downward row shifts rotate and reuse displaced row vectors
   directly. The two temporary vectors on every upward shift and the temporary
   vector on every downward shift are gone.
3. Ground-state text is decoded as valid UTF-8 spans. Malformed data is handled
   in the same linear pass through the existing decoder.
4. `Attributes` and structural cell booleans are packed into two one-byte
   bitfields. This made Tmon cells the same 24-byte size as Alacritty cells.
5. Printable ASCII is written in safe row-local runs. The scalar path still
   handles insert mode, deferred wrapping, charset translation, wide-cell
   cleanup, and every complex destination.

Two adversarial quadratic cases found during review were fixed: long malformed
UTF-8 runs and long ASCII spans in insert/no-wrap modes. No unsafe Rust or new
dependency was introduced.

An erase-display experiment that tried to skip fills of already blank rows was
discarded. Its first version fell to 3.3 MiB/s on edits; an exact-first revision
reached 4.4 MiB/s versus the retained 4.5 MiB/s at that stage.

## Follow-up awaiting CI measurement

The saved after-table predates a small static hot-path follow-up. Printable
ASCII at the start of a ground-state batch now bypasses the control-span scan
and UTF-8 validation pass, wide-cell replacement reuses one scalar-width
calculation, and OSC 8 explicit identities use a reverse hash index. Hyperlink
garbage collection is also amortized after 4,096 live links instead of scanning
the full primary, alternate, and scrollback grids for every later link.
Long combining sequences now append in place to their uniquely owned pooled
metadata entry instead of cloning the complete prefix for every new mark. A
1,024-mark regression verifies linear growth and a single pooled record.

Width reflow now matches the pinned Alacritty engine across exhaustive scrolled
soft-wrap matrices that vary both rows and columns, including its retained
clear cursor-continuation rule. The display-offset shadow is skipped entirely
for the common unscrolled path. Tmon also guarantees progress for a wide
character resized to one column, which is below Alacritty's supported minimum
width. These are correctness and complexity results only; no resize timing is
inferred from them. Raw C1 string terminators were aligned with the pinned VTE
parser at the same time and are covered by exact differential fixtures.

These changes are intentionally not folded into the measured table above. The
next GitHub Actions benchmark run is the acceptance measurement; no newer
completed local result is accepted as evidence here. If the parser ratios or
Tmon full-frame regression trend move backward there, the focused changes
should be reconsidered independently.
That run will also produce the first equivalent-work normalized-frame ratio.

PTY control traffic was hardened separately from the parser benchmark. Resize
requests now coalesce out of band and close is a sticky cancellation signal, so
neither waits behind a saturated input queue. A deterministic PTY test fills
the old queue-sized backlog, verifies that the newest resize reaches the child,
and confirms that every accepted input block remains in FIFO order. This is a
correctness and tail-latency safeguard; the saved parser/grid table does not
measure it.

Normal viewport scrolling now has a renderer-aware incremental path, also not
included in the saved table. Tmon retains chronological row permutations,
transforms pending dirty spans into final viewport coordinates, and exposes the
batch with a generation token. The desktop replays those permutations by moving
cached row handles and patches only final dirty cells; a generation mismatch
falls back to a fresh full cache. Resize/reflow, alternate-screen transitions,
palette or graphics changes, reset, selection/search overlays, scrollback view,
invalid geometry, and bounded-log overflow remain conservative full rebuilds.
The paint cache rebuilds every row in a moved region for now, while retaining
its existing cross-row shape reuse.

The cached render cell no longer stores a redundant destination row; cursor,
hover, underline, text-batch, and geometry decisions derive it from the outer
row. Tests replay full, partial, opposing, and multi-row scrolls into an
incremental cache and compare the result exactly with a fresh Tmon snapshot
after every update. No local timing was run for this change. Its report-only
cache-update measurement in the next Actions artifact determines whether the
optimization is retained and whether paint-cache row movement is worth a
second step.

Extended underline compatibility was added after the saved measurements and
has not been benchmarked locally. `Attributes` widened to retain all five SGR 4
underline styles, and each cell now carries an optional indexed/RGB SGR 58
color. Combining text and OSC 8 identity were consolidated into one tagged
metadata word, so `Color`, `Option<Color>`, and the full `Cell` remain 4, 4,
and 24 bytes respectively. Exact Alacritty differential fixtures cover color
and style resets, long combining text plus hyperlinks, edits, erasure, wide
cell cleanup, reflow, scrollback, saved cursors, and alternate screens. The
next Actions artifact is the first performance acceptance measurement for this
layout; no speed claim is inferred from the unchanged static size.
Termy's Tmon-only render path now retains the five styles and explicit color.
Straight and curly styles use GPUI decorations; double, dotted, and dashed
styles paint one bounded path per text batch. Style and color participate in
batch/cache identity, while the native Alacritty and tmux mapping deliberately
keeps its prior straight-foreground behavior. This painting work is likewise
outside the saved parser/grid measurements.

Graphics decoding is now portable and dependency-free. A safe, std-only
zlib/DEFLATE implementation validates PNG payloads and Kitty `o=z` data on every
supported platform, replacing the Unix libz boundary and former non-Unix
unsupported path. This work has no new performance number: the parser/grid
benchmark does not exercise graphics decoding. Decoded uploads and inflated
scanline data are capped at 64 MiB per image, with a separate 128 MiB stored-PNG
quota and a 4,096 image-record limit. Raw RGB/RGBA encoding now streams scanline
filter bytes and stored
DEFLATE blocks directly into the final PNG allocation; the full filtered and
zlib staging vectors are gone. CRC32 uses a compile-time lookup table and
Adler32 is incremental. Byte-for-byte regression fixtures cover a DEFLATE block
boundary. Kitty control data is independently capped at 4 KiB and 64 fields
before conversion, including unterminated commands split across reads. Supplied
PNGs are compacted in place after validation to remove compressed iCCP/zTXt/iTXt
metadata and APNG chunks, keeping downstream decoding within the validated
static-image boundary. The common single-IDAT path validates directly from the
upload buffer without allocating a duplicate compressed stream; multi-IDAT
files are joined only when their chunk boundaries require it. The decoder rejects
streams above 65,536 DEFLATE blocks, chunk continuations are capacity-checked
before base64 allocation, and Unix file transfers reject FIFOs without a
blocking open. This is a bounded-memory improvement rather than a speed or
peak-RSS claim because the parser/grid benchmark does not exercise graphics.

Image-number lookup uses the existing bounded image table and generation field,
so supporting Kitty's `I` key adds no unbounded index. Uppercase deletion now
tracks only image IDs affected by the selector instead of scanning and freeing
every unrelated soft-deleted image.

Native process handling also expanded outside the measured parser path. Tmon
now provides both its Unix PTY and a dependency-free Windows ConPTY backend that
loads the required entry points dynamically. The exact
`TERMY_EXPERIMENTAL_TMON_ENGINE=1` gate selects it only when available; the
Alacritty default and fallback remain unchanged. Shell command lifecycle events
have priority in the bounded event queue, and Unix child exit now captures its
readable output tail before a short bounded drain. Both PTY writers admit at
most 8 MiB across 4,096 queued-or-active nonempty writes, reserve borrowed input
before copying it, charge owned allocation capacity, release accounting through
RAII, and coalesce resize/close wakes while preserving FIFO for every accepted
write. A separate 2 MiB/256-entry FIFO gives required protocol replies priority
at bounded 16 KiB write boundaries. An interrupted normal write resumes at the
same offset and normal input order remains FIFO; saturation closes the session
rather than silently losing a response. Unsupported Unix ABIs now compile
through Tmon's non-native fallback instead of entering a partially defined PTY
module. The parser/grid benchmark does not measure this PTY or event-queue work.
Windows CI launches live ConPTY children directly through Tmon and covers final
output draining before the exit callback, immediate resize plus stdin
round-trip, and batch-wrapper launch in a normalized working directory. It does
not exercise ConPTY through the desktop application or measure Windows PTY
performance. The next GitHub Actions benchmark artifact is still pending.

Dropping a live Unix PTY now sends hangup, waits for a bounded 250 ms grace
period, and escalates to `SIGKILL`, preventing a hangup-ignoring child from
leaking indefinitely. Synchronized-update expiry also moved from a global
unbounded queue to one shared coalescing scheduler with at most one queued task
per engine. OSC 52 query replies use the reserved protocol-reply lane rather
than competing with normal input. These are boundedness and lifecycle fixes;
they have no parser/grid benchmark number.

## Allocation accounting

Whole-process allocator requests are now measured by a separate, report-only
pass after the timed comparison. The xtask benchmark example's
`benchmark-allocations` feature installs a `System` wrapper only in the example
binary and requires
`TMON_BENCH_ALLOCATIONS_ONLY=1`; a feature build refuses timed mode, while a
normal build rejects allocation-only mode. The normal throughput executable
therefore contains no allocator-counter overhead.

For each engine/workload pair, the pass warms 2 MiB on an uncounted terminal,
constructs a fresh terminal while counting is disabled, resets and enables the
counter with an unwind-safe non-nestable guard, and counts only the same
complete-payload `feed_output` loop used by timed samples. It reports the exact
processed byte count, which can be slightly above the configured target. It
atomically closes registration and waits for in-flight allocator hooks before
observing a snapshot and before dropping the terminal. The output reports raw
allocation totals (including zeroed allocations),
deallocation totals, and successful realloc request totals plus a signed net
requested-byte change; no allocation threshold is enforced.

This is scoped allocator-request accounting, not a memory-usage measurement.
The process-global wrapper also sees concurrent allocator traffic while the
guard is active, and the signed net is requested bytes in that interval, not
RSS, retained or usable heap, allocator metadata, or physical memory. Terminal
construction, the warmup, snapshot creation, and terminal destruction are
deliberately outside the count.

Before this path existed, macOS Allocations could not attach to the profiling
binary under the current System Integrity Protection configuration. The static
hot-path estimates from that earlier audit remain useful context:

| Workload | Removed temporary row-vector allocations per 32 MiB sample |
| --- | ---: |
| Plain scrollback | about 1.20 million |
| Styled TUI redraw | none from row shifting |
| Unicode + wide cells | about 1.22 million |
| Cursor + line edits | about 3.17 million |

The counts follow directly from the old paths: two temporary vectors per
upward row shift and one per downward shift. Parser span processing and packed
flags allocate nothing. Normal rows are still allocated while history first
grows, then evicted row storage is reused.

## Correctness and compatibility

Coverage added or exercised includes scrollback rollover, multi-row shifts,
damage reset, packed flag independence, malformed and split UTF-8, long ASCII
slow modes, ASCII/scalar equivalence through wrap and scrollback, wide-cell
fallback, and REP after a batched span. Existing randomized parser invariants
and deterministic Alacritty differential traces remain enabled.

Packing is an intentional source-level API change in this experimental crate:
direct flag fields become getters/setters, and full external `Cell` struct
literals must become `Cell::default()` followed by field/setter updates. Public
setters retain the ability to construct every former flag state.

No terminal behavior was deliberately removed or weakened for speed. The only
accepted compatibility tradeoff is the source-level `Cell` construction change
above; the engine remains experimental and its broader VT/graphics parity is
tracked separately from these benchmark results.

A follow-up parity audit removed Tmon's artificial 4,096-character REP cap, so
the full parsed `u16` repeat count now matches the pinned VTE behavior. The
desktop adapter also enables OSC 52 clipboard queries through Termy's existing
reply host, matching the native engine while leaving standalone Tmon's
copy-only default unchanged.

Search, selection, and persisted-buffer extraction now visit complete line
ranges under one backing-terminal lock. Bounds, dimensions, and cells therefore
come from one state while avoiding a lock/unlock cycle for every scrollback row.
The developer inspector likewise captures Tmon's dimensions, scroll state,
modes, and cursor under one engine lock instead of taking seven independent
snapshots per pane and potentially mixing adjacent frames.

## Remaining hotspots

- `put_char`, cell writes, and row movement remain the largest plain/Unicode
  costs. Future changes should be driven by fresh per-workload profiles because
  the current targets already clear their required margins.
- Erase-display and row shifts still dominate the edit trace. The rejected
  blank-row shortcut shows that extra state checks can cost more than a bulk
  fill, so another attempt needs an adversarial differential test and repeatable
  measurements.
- The legacy public full-frame APIs perform different work, so their ratio must
  not be presented as a raw engine advantage. The new shared normalized-frame
  metric is equivalent work but still needs its first CI baseline. The separate
  immutable-reference gate enforces Tmon's historical no-more-than-5% snapshot
  regression requirement; its latest code still needs the next Actions run.
- The first ordered scroll-cache path still rebuilds paint operations across
  each moved region. Moving paint-row operations in lockstep could reduce that
  work, but needs a renderer-inclusive measurement and cursor/hover/selection
  correctness proof before changing the conservative invalidation.
- Whole-process allocator requests are counted only during the allocation
  pass's `feed_output` scopes. RSS, retained heap, allocator fragmentation and
  metadata, terminal construction/destruction, snapshots, PTY I/O, and renderer
  allocations remain outside what this report can claim.
- Shared GitHub runners report a median and range but cannot guarantee CPU
  frequency or isolation. Cross-push results are useful trend signals, not
  laboratory-grade absolute measurements.
