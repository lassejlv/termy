# Terminal engine replacement

This directory owns the parser, screen storage, and platform PTY transport used
by native, display-only, tmux and persistent Termy sessions.

## Design

- `parser.rs` is a streaming UTF-8/VT state machine. Printable ASCII is passed
  to the screen in runs. Complete UTF-8 scalars within a chunk use one dispatch;
  malformed and fragmented sequences retain the streaming fallback. CSI
  parameters use fixed arrays; OSC/DCS/APC share a reusable, bounded buffer.
  Completed or aborted strings release buffers larger than 64 KiB. Fragment
  boundaries must never change behavior; the synchronized-output scanner uses
  the same string limits without retaining a second copy of each payload.
- `grid.rs` owns primary/alternate screens, scrollback, wide characters,
  cursor state and dirty ranges. Scrolling moves row ownership and reuses
  evicted row allocations once history is full. Rows track a conservative
  occupied prefix, so recycling resets only changed cells unless the erase
  background changes. Scalar writes replace destination cells directly and
  repair only the outside halves of overwritten wide glyphs. At most 32 ordered
  scroll operations let renderers move cached rows and repaint only exposed spans.
  Resize streams reflow into a bounded window and reuses discarded row buffers,
  avoiding a temporary allocation proportional to old history times new width.
- `types.rs` provides compact engine-owned values. Colors occupy four bytes;
  cells occupy at most 32 bytes. Combining marks and links use shared optional
  metadata, so ordinary cells and ASCII writes allocate nothing.
- `grid/combining.rs` interns repeated compositions in a bounded cache. Its byte
  hash selects one of 64 buckets with four entries each; full byte and hyperlink
  pointer equality decide a hit. Fixed-width comparisons handle keys up to four
  bytes, and longer prefixes use ordinary slice equality. Collisions never
  increase lookup work beyond four entries or change immutable cell metadata.
- `grid/print.rs` joins streaming emoji/grapheme sequences and adjusts their
  column width, including right-margin promotion and one-column reflow.
  Width changes damage erased spacer cells even when the cursor is hidden.
  `grid/row.rs` packs cold history into scalar/flag arrays, style runs, and shared
  metadata while leaving active rows dense.
- `dispatch.rs` applies control sequences and owns modes, palette changes,
  hyperlinks and bounded event/reply queues. Clipboard controls share the parser
  and synchronized commits, avoiding a second scan and filtered input copy.
  DECSTR resets VT state while preserving screen contents. Supported DEC private
  modes can be saved and restored, including synchronized output.
  `queries.rs` reports live VT state.
- `sync.rs` buffers synchronized output with a 2 MiB limit and a 150 ms timeout.
  A syntax-aware marker scanner preserves ordering across fragmented strings.
- `graphics.rs` applies image commands and ordered scroll/clear effects inside
  the same parser commits as text. Upload chunks do not scan the viewport for
  Unicode placeholders; visual mutations invalidate placeholder placement state.
  Virtual placement counts are maintained per screen, so checking for
  placeholders during ordinary text feeds does not scan retained images. History
  evicted during height resize moves image anchors with the retained text.
  Animation revision polling allocates nothing.
- `media/shared_memory.rs` copies bounded image transfers into owned storage.
  Linux descriptor reads handle concurrent truncation as an error. macOS uses
  a kernel-mediated mapping copy because its shared-memory descriptors do not
  support descriptor reads.
- `transport/` provides bounded native PTY input/output on Unix and Windows.
  The runtime maintenance thread sleeps until a synchronized-output deadline or
  a pending history-compaction step. History compaction starts 250 ms after the
  last added history row and processes at most 256 rows per step. Windows control
  workers wait on child-exit and resize/close events instead of polling a timer.
- `Engine` is single-owner state. Transport/runtime synchronization belongs
  outside it. Active viewport reads borrow row slices. Cold history is expanded
  on demand for borrowed reads; invalidation visits only the interval containing
  those reads. Search and bulk visitors reuse scratch storage.

Sustained output retains the dense scrolling fast path. Quiet history compacts
without changing damage or generation. Direct engine hosts can explicitly call
`compact_history()` after a burst. Allocation benchmarks report active and
settled retained heap separately from parsing throughput.
The transition out of quiet compaction counts output bytes across feeds, so
fragmenting a burst into small writes does not keep packing new history rows.
Grid dimensions are clamped to 4,096 per axis and 1,048,576 cells in total.
History retains at most 20,000 rows and 1,048,576 cells, so wide grids can retain
fewer rows than the configured history count. Combining suffixes, CSI parameters,
strings, title stacks, keyboard stacks, events and replies are also bounded. The runtime event queue
has an 8 MiB retained-payload budget as well as a count limit; dropping clipboard
chunks aborts the transaction, and resets still revoke permission grants.
Large direct feeds and synchronized replay apply graphics effects in 64 KiB
segments, so effect queues cannot grow with an arbitrarily large caller buffer.

VT behavior is checked against the
[XTerm control sequence reference](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html)
and the [Kitty keyboard protocol](https://sw.kovidgoyal.net/kitty/keyboard-protocol/).
Tests use explicit expected states, malformed-input fixtures and all-boundary
fragmentation checks, without an Alacritty test oracle.

## Verification

The Unicode, CJK rendering, memory, and native presented-frame measurements are
documented in the [follow-up report](../../../../docs/engineering/unicode-history-performance-2026-10-05.md).
The [October 7 audit report](../../../../docs/engineering/terminal-engine-audit-fixes-2026-10-07.md)
records the subsequent correctness fixes, targeted regression measurements, and
remaining throughput and presentation limitations. The
[October 8 review fixes](../../../../docs/engineering/terminal-engine-review-fixes-2026-10-08.md)
cover shared-memory truncation, resize image anchors, grapheme damage and retained
placement lookup costs.

```sh
cargo test -p termy_core terminal_engine
cargo run --release -p termy_core --example terminal_engine_bench -- 32
```

The allocation benchmark warms each workload first, feeds both 64 KiB and
one-byte chunks, and counts actual allocator calls. Steady plain-text scrolling,
styled redraws and wide Unicode must reuse their warmed storage. Repeated
combining suffixes also allocate nothing after warmup: a per-grid, 256-entry cache
shares immutable metadata, including hyperlink identity, without changing older
cells when a new mark is appended. Suffixes are capped at 256 bytes, and links with more than
1024 bytes of allocated string storage bypass the cache to bound retained memory.
New combinations still allocate; the benchmark asserts zero allocations for its
repeated combining workload as well as ordinary text. Timings include
allocator instrumentation and are not a substitute for PTY/UI latency tests.
The heap figures are allocator-requested bytes for the process, not OS RSS.
The varied-Unicode case cycles through 16,384 distinct CJK scalars to exercise
width lookups that a small cache cannot retain. Keep it alongside the repeated
Unicode case when evaluating width optimizations. The cloud evaluation rejected
a 512-entry width cache because its mixed results did not justify adopting it.

`crates/core/examples/terminal_facade_bench.rs` measures uninstrumented throughput
through the public terminal facade. Compile the identical helper source against
each revision's release library before comparing saved executables; its source
header gives the `rustc` command. The seven cases cover plain scrolling, styled
redraws, repeated Unicode, 16,384 distinct Unicode scalars, repeated combining
marks, 112 distinct combining marks, and one-byte fragmented plain text.

Run the saved binaries with `scripts/benchmark-terminal-facades.py` from the
repository root:

```sh
python3 scripts/benchmark-terminal-facades.py \
  --baseline /path/to/baseline --candidate /path/to/candidate \
  --mib 32 --pairs 6 --output target/facade-feed.json
```

The runner alternates baseline/candidate order, records raw samples and binary
hashes, and reports median paired throughput ratios. Finish builds before timing;
run no concurrent benchmark or build. The runner requests the historical Alacritty
backend and verifies the helper's reported engine. Baselines may report Alacritty
or the custom engine; unsupported, missing or changing labels are rejected. Candidates
must report the custom engine. Check the recorded baseline engine before making
an Alacritty comparison.

Repeat with `--consume-damage` and a separate output file to drain render damage
once per full payload block: approximately 64 KiB, or 100 KiB for varied Unicode.
Scrolling can saturate full damage within a block, so this mode measures damage
reset/consumption overhead without modeling an incremental renderer's cadence.
It does not render cells or measure PTY, focused-window, or presented-frame latency.
The PR workflow builds identical helpers for the base and head commits on Linux
and macOS, then runs both modes. Each workload's median paired candidate/baseline
ratio must reach `0.95`; this permits 5% timing variation and does not itself
establish parity. Assess the recorded medians and native latency checks separately.

## Integration validation

Keep these checks green when changing the engine:

- Native PTYs on macOS/Linux and Windows ConPTY use this engine, with bounded
  input queues, ordered replies, resize, shutdown and child-exit handling.
- Native, display, tmux and remote sessions share the public core render and
  terminal contracts. Search, links, selection, palette, clipboard, shell
  integration, Kitty graphics and synchronized output retain their behavior.
- Resize/reflow, history anchoring, Unicode, screen editing and damage replay
  are covered by expected-state regression tests. DCS/APC handlers and
  synchronized-output commit/timeout behavior must pass fragmentation and native
  transport tests.
- All active Alacritty imports, adapters and Cargo dependencies are removed.
  The former experimental display engine and its tests and tooling are removed.
- Core, desktop, FFI, IPC and tmux integration suites pass, along with workspace
  checks, formatting, Clippy and repository architecture gates.
- A real shell and tmux pane are exercised in the app. Parser throughput,
  steady-state allocations, retained memory, idle wakeups and input/frame
  latency are measured. Record regressions and memory tradeoffs alongside gains.
- The branch and PR contain the verified changes and required CI checks have
  reached successful terminal states.

The public runtime facade remains stable. Desktop panes use the shared facade,
so tmux and native sessions exercise the same engine and graphics pipeline.

Measured performance and native QA are recorded in
the [original report](../../../../docs/engineering/custom-engine-2026-10-05.md)
and [throughput follow-up](../../../../docs/engineering/custom-engine-throughput-2026-10-05.md).
