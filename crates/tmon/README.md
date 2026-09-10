# Tmon

Tmon is Termy's experimental lightweight terminal engine, located in the
`crates/tmon` directory so its implementation stays independent of GPUI and
the desktop application.

The current prototype is an independent engine with no production Cargo
dependencies. It owns its incremental VT parser, bounded primary/alternate
grids and scrollback, damage tracking, renderer-neutral cell types, and native
process lifecycle. It does not depend on `termy_core` or GPUI. Native process
I/O uses direct platform APIs: a PTY backend on Linux, Android, and macOS, plus
a dependency-free Windows ConPTY backend whose entry points are loaded
dynamically. PNG and Kitty graphics use Tmon's safe, portable, std-only
zlib/DEFLATE decoder.

## Owner

This crate owns the experimental Tmon parser, grids and scrollback, terminal
state, damage model, graphics protocol, and native PTY/ConPTY lifecycle. It
must remain renderer-neutral and independently usable without Termy's desktop
application or the existing Alacritty-backed core.

## Validation

```sh
cargo test -p tmon
cargo clippy -p tmon --all-targets -- -D warnings
```

The cross-engine benchmark harness lives in `crates/xtask` so this crate does
not depend on `termy_core`. Its timed mode can run locally with
`just benchmark-tmon`; GitHub Actions runs the same comparison for relevant
pushes to `main` and manual dispatches.

## Forbidden Dependencies

- `gpui`
- `termy_core`
- `termy_terminal_ui`
- `termy_ui`
- `termy_ffi`
- `termy` / `crates/desktop_app`

`alacritty_terminal` and `unicode-width` are development-only dependencies for
semantic parity checks; Tmon's production dependency section remains empty.

Run the desktop app with Tmon selected:

```sh
TERMY_EXPERIMENTAL_TMON_ENGINE=1 just run
```

Open Termy's inspector and select the **Terminal** tab; the **Engine** row reads
`tmon` when the experimental backend is active and `alacritty` otherwise. The
startup log also prints `using experimental Tmon terminal engine` when selected.

The environment variable affects native terminals only. Tmux display panes
continue to use their existing parser. Windows Tmon CI launches live ConPTY
children directly through Tmon and verifies final-output draining before the
exit callback, immediate resize plus stdin round-trip, and batch-wrapper launch
in a normalized working directory. It does not exercise desktop engine
selection or GPUI rendering through a live Windows terminal. An exact `1`
selects Tmon when the required ConPTY APIs are available; otherwise Termy logs a
warning and falls back to the unchanged Alacritty-backed native engine. Without
an exact `1`, Alacritty remains the default on every platform. Other Unix
targets also keep that fallback because Tmon does not advertise an unsupported
native PTY ABI there.

## Direction

- Keep the engine independent of GPUI.
- Keep one bounded grid allocation and bounded scrollback.
- Emit damage-scoped frame updates rather than full-grid redraws.
- Keep PTY reads and parsing off the UI thread.
- Keep expanding VT/OSC/DCS and graphics compatibility behind the opt-in gate.
- Differential-test common shells and TUIs against the existing native engine.

Tmon is still experimental. The existing Alacritty-backed native engine remains
the default and tmux panes keep their existing behavior unless the environment
variable is exactly `1`.

Renderers that can move cached rows use `take_render_damage_snapshot()`. Its
scroll operations are chronological and its partial spans describe final
viewport coordinates, so consumers replay every scroll before patching cells.
The accompanying generation must still match when cells are visited; otherwise
the consumer rebuilds a fresh frame. The older `take_damage_snapshot()` remains
source-compatible and deliberately reports scrolls as full damage for callers
that do not understand row movement.

## Compatibility boundary

Tmon is differential-tested against Termy's Alacritty-backed native wrapper for
the VT sequences that wrapper implements. It intentionally implements several
standards and xterm extensions that the pinned parser ignores: legacy
alternate-screen/cursor modes 47, 1047, and 1048; DEC selective erase and
DECST8C tab-stop reset; ISO 2022 G1/G2/G3 designation and single shifts; and
DECRQSS/XTGETTCAP replies. OSC 52 additionally accepts valid unpadded or
whitespace-separated base64. These are compatibility extensions, not benchmark
shortcuts, and are tested separately from strict differential fixtures.

Tmon also keeps combining marks available to selection and search. Termy's
existing Alacritty cell adapter omits that out-of-line text; its default behavior
is deliberately unchanged by the experiment. OSC/DCS payloads are bounded to
64 KiB so an unterminated control string cannot grow without limit.
Width reflow is differential-tested while scrolled and while changing rows and
columns together. Tmon additionally keeps its public one-column grid finite for
wide characters, even though the pinned Alacritty engine supports a minimum of
two columns.
Standalone Tmon keeps the conservative `Osc52::OnlyCopy` default; the Termy
desktop adapter explicitly enables `CopyPaste` so OSC 52 clipboard queries match
the existing native engine and still pass through the application's reply host.
Kitty OSC 5522 packets are emitted as bounded protocol events for the host to
authorize and service, while private mode 5522 is tracked for MIME-aware paste
notifications.
Lifecycle events for shell prompt, command start, execution, and completion have
queue priority over discardable noise, so a bounded event queue preserves a
coherent command cycle under load.

Kitty graphics supports RGB/RGBA, PNG, zlib, chunked uploads, local files,
temporary files, and shared memory. Uploads and inflated images are capped at
64 MiB each; decoded image and animation storage has a separate 128 MiB quota
and a 4,096 image limit. Controls stop at 4 KiB and 64 fields. File transfers
reject non-regular files and use nonblocking opens on supported Unix targets.

Images expose shared RGBA pixels through `GraphicsImage::rgba()`. The PNG
representation is encoded lazily by `GraphicsImage::png()` for export. PNG
scanlines, palette transparency, packed samples, 16-bit samples, and Adam7
passes are decoded inside Tmon with no production Cargo dependencies.

Placement geometry preserves natural pixels and fractional aspect ratios.
Cell offsets are included inside explicitly requested dimensions. Unicode
placeholders render individual cells and support gaps and repeated instances;
relative placements retain signed positions. Images move and clip with page
margins, and font-size changes update their occupied cells.

Animation frame upload (`a=f`), playback (`a=a`), composition (`a=c`), and frame
deletion (`d=f/F`) share the renderer-neutral animation implementation with
core. A visible snapshot provides `animation_deadline`; hosts schedule a wakeup
at that instant and request a fresh snapshot. Stopped animations and loading
animations awaiting another frame have no wakeup. Frame generations stay stable
across loops so textures can be reused.

Image numbers (`I`) allocate distinct IDs and always select the newest upload.
Deletion frees only matching images after their final placement disappears.
ID and ID-range deletion is global; coordinate deletion targets the active
screen. Scrollback placements survive a visible-screen delete.

The Unix PTY captures bytes readable when the direct child exits, then uses a
short bounded nonblocking drain to retain final output without allowing a live
descendant to delay exit forever. Dropping a live Unix PTY sends hangup, allows
a 250 ms grace period, then escalates to `SIGKILL` so a child that ignores
hangup cannot leak indefinitely. Windows uses separate synchronous ConPTY
reader and writer paths and keeps draining output through shutdown. On both
platforms, queued plus active normal input is capped at 8 MiB and 4,096 writes,
including retained `Vec` capacity. A separate priority lane reserves 2 MiB and
256 entries for terminal protocol replies, including OSC 52 query responses.
Replies can preempt normal input at bounded 16 KiB write boundaries; interrupted
normal input resumes at the same offset and keeps its FIFO order. If even the
priority lane cannot accept a required reply, Tmon closes the session instead
of silently leaving a child waiting forever. Resize and close notifications
coalesce to a single wake while the newest resize and sticky close state remain
out of band, so control traffic cannot grow an unbounded queue or wait behind
the input backlog. Synchronized-update expiry uses one shared scheduler with at
most one queued watchdog task per engine instead of spawning or retaining an
unbounded task stream.

Extended underline state is preserved rather than collapsed to a boolean. Tmon
tracks single, double, curly, dotted, and dashed SGR 4 styles plus indexed and
RGB SGR 58 colors; SGR 24, 59, and 0 reset the same independent pieces as the
pinned VTE parser. Underline color remains directly readable from a copied cell.
Combining text and OSC 8 identity share one tagged rare-metadata word, keeping
`Cell` at 24 bytes even when both features are supported together.
The desktop Tmon adapter carries that state into Termy's renderer: single and
curly underlines use GPUI's native straight/wavy decoration, while double,
dotted, and dashed styles use bounded batched paths. Native Alacritty and tmux
cells retain their previous single-underline rendering behavior.

## Compact scrollback

Primary and alternate screens remain dense so parsing and rendering keep direct
cell access. A row is compacted only when it enters primary-screen scrollback:
its logical width and stored cell count are kept separately, an exact-default
suffix is omitted, and ordinary characters are stored directly as UTF-8.
Sparse bytecode commands carry style transitions, structural cell state, wide
spacers, inline combining marks, and rare metadata. Colored and attributed
rows use the same lossless encoding instead of falling back to 24-byte cells.

History reads use a logical row view that supplies default cells for the omitted
suffix and can visit cells without allocating. Resize and reflow materialize
dense storage only while rebuilding rows; cross-row wide-spacer repair updates
the compact encoding immediately. Evicted byte storage is reused, pooled
combining metadata is released with its owning row, and
clearing or shrinking history releases unused compact storage. Live row extents
avoid rescanning known-default tails when scrolling without changing the dense
live-grid representation.

For complete default-style `CRLF` batches, a guarded parser/grid path moves cold
primary rows directly into compact history and rebuilds only the final
viewport. On the alternate screen, the ASCII form rotates and rewrites the
dense rows in place without allocating or recording history. The primary path
also handles validated UTF-8 rows with wide cells and one inline combining mark
per cell. It activates only at the bottom of a full-screen scroll region with
default modes and style; partial or more complex input, custom charsets, and
every ambiguous state keep using the normal scalar path. Complete ordinary CSI
sequences also bypass the byte-at-a-time state machine; private mode changes
stay on the scalar path so synchronized-update boundaries remain unchanged.

The ten-sample macOS ARM64 results and tradeoffs are recorded in
[PERFORMANCE.md](PERFORMANCE.md). The compact uncompressed representation meets
the current memory target, so no cold-page compression or new dependency is
included.

## Benchmarking

Run the headless Tmon versus Alacritty parser/grid comparison and write
`tmon-alacritty-benchmark.txt`:

```sh
just benchmark-tmon
```

The separate macOS harness compares Tmon with pinned, statically linked
`libghostty-vt`. Its memory mode uses fresh processes for 13 empty, primary,
alternate, interactive, and scrollback states. Its balanced feed mode compares
eight semantically checked workloads using identical 64 KiB scrollback chunks,
row-aligned alternate-screen chunks, or one complete interactive operation
batch per call. Run both locally with a Ghostty source checkout, or manually
dispatch the **Tmon vs Ghostty Benchmark** GitHub Actions workflow:

```sh
GHOSTTY_DIR=../ghostty just benchmark-tmon-ghostty-memory
```

The `Tmon vs Alacritty Benchmark` GitHub Actions workflow runs on pushes to
`main` that change its declared Cargo, core, Tmon, xtask, revision-gate, or
workflow paths, and it can also be started manually with configurable workload
sizes. When the runner
remains available, the report is uploaded as a 30-day artifact and included in
the job summary; its finalizer records setup outcomes even if the comparison
cannot start. CI defaults to eight samples and rejects odd counts so each
workload has four Tmon-first and four Alacritty-first pairs. It fails when the
median of a workload's paired ratios misses its committed target; local runs
print the same verdict without failing unless `TMON_BENCH_ENFORCE_TARGETS=1` is
set. Both `just benchmark-tmon` and CI then run an isolated snapshot regression
gate against immutable Tmon commit
`03d7ca5c5420ca141afe56f725341faadb71af18` and append a report-only allocation
pass to the same text report. Manual inputs are bounded so an oversized request
fails before it can consume the job timeout and prevent report upload. The first
measured optimization pass is documented in [PERFORMANCE.md](PERFORMANCE.md).

The suite compares identical byte streams, grid dimensions, and scrollback
limits in release mode. The report names the direct Tmon runtime and the
Alacritty-backed `termy_core` runtime explicitly; desktop engine environment is
not used to choose either path. Before timing, an untimed preflight compares up
to 32
MiB of every workload across every normalized grid and scrollback cell plus
cursor, scroll, and screen state. It reports integrated Termy backend
throughput, current full-frame API throughput, and static cell sizes. Feed calls
intentionally keep the original small workload payloads so results remain
comparable with the saved baseline. The snapshot APIs perform different
conversion work, so that ratio is not a raw engine-to-engine snapshot
comparison. Its ratio and retention against the supplied 19.880x baseline are
report-only: enforcing `19.880x * 0.95` on hosted CI would not be sound because
the APIs do different work and can react differently to runner noise. The suite
also measures both engines through one benchmark-local normalized frame contract
covering the same visible cells, metadata, raw colors, styles, combining text,
exact underline styles and colors, hyperlinks, wide-cell roles, and wrapping.
Every normalized sample runs for at
least 250 ms, and an untimed mixed-frame preflight requires exact equality. This
new equivalent-work ratio is report-only until its first Linux CI baseline is
available. The suite does not measure graphics decoding, PTY I/O, GPUI painting,
input latency, or complete terminal compatibility, and speed ratios are not a
feature-parity score. Windows CI separately launches live ConPTY children to
cover final-output draining, immediate resize and stdin round-trip, and a
batch-wrapper working-directory case. It does not exercise ConPTY through the
desktop application or measure Windows PTY performance. The first GitHub
Actions measurement of the current follow-up work is also pending.

It also prints a report-only Tmon scroll-cache measurement. Parsing and damage
capture happen once outside both timed paths; an untimed preflight requires an
incrementally replayed cache to equal a fresh normalized visible cache exactly.
The metric compares cache-update work, not GPUI painting, PTY I/O, or an
Alacritty renderer path. It has no pass/fail threshold until Linux Actions has
established a stable baseline.

The revision gate is a different comparison: current Tmon and the pinned
reference call the same native `Terminal::snapshot()` API on identical terminal
state in one process. Before timing, an exact preflight verifies each actual
snapshot against independently visited terminal state and the other revision.
Twenty balanced interleaved blocks run each side for at least 250 ms per
measurement; the gate uses conservative order statistics over paired throughput
ratios. Current Tmon passes only when the lower bound is at least 95% of the
immutable reference. A clear regression or an inconclusive noisy result fails
CI instead of silently accepting uncertainty. The reference SHA is ratcheted
only by an explicit source change after a reviewed measurement, never to the
previous push automatically.

The allocation pass rebuilds only the xtask benchmark example with the
`benchmark-allocations` feature and requires
`TMON_BENCH_ALLOCATIONS_ONLY=1`; that feature build refuses timed mode. Its
`System` allocator wrapper is absent from the normal throughput binary, so the
timed measurements have no counter overhead. For every engine and workload it
warms 2 MiB on an uncounted terminal, constructs a fresh terminal uncounted, and
then counts process-wide allocator requests only during the same complete-payload
`feed_output` loop as the timed samples. It reports the exact processed byte
count, which can slightly exceed the configured target. Snapshots and terminal
destruction happen after the guard has disabled new hooks and waited for any
in-flight hooks. The report includes
successful allocation calls and requested bytes (including `alloc_zeroed`),
deallocation calls and requested bytes, successful reallocation calls with
old/new requested bytes, and their signed net requested-byte change. These are
allocator-request figures, not RSS, retained or usable heap, allocator metadata,
or physical memory; allocations from concurrent process activity can also be
included while the guard is active. All allocation results are report-only.
