# Terminal engine audit probes

These standalone probes reproduce the scrollback and graphics performance
findings from the October 2026 engine audit. They use public engine APIs and can
be compiled from identical source against the baseline and candidate libraries.

Build each revision with `cargo build --release -p termy_core --lib`, then compile
each probe, replacing the library directory and output path for that revision:

```sh
rustc --edition=2024 -O scripts/terminal-engine-audit/compaction.rs \
  --extern termy_core=/path/to/revision/target/release/deps/libtermy_core.rlib \
  -L dependency=/path/to/revision/target/release/deps \
  -o /tmp/compaction-baseline
```

- `compaction.rs`: warm a 120×30 grid with 1,000 history rows, optionally compact
  it, then feed 300,000 short lines. Six alternating dense/compacted pairs cover
  plain text and alternating styles. Allocation counts are cumulative allocator
  requests during measurement, not retained heap or RSS.
- `history-cache.rs`: read 24 compacted viewport rows, then feed one NUL byte,
  repeating 2,000 times with 1,000 or 20,000 history rows. Four alternating pairs
  expose invalidation work that scales with unread history.
- `graphics-upload.rs`: upload one image in 512 chunks, with and without a
  virtual placement prototype. Five alternating pairs at three grid sizes
  expose viewport scans during chunk assembly. The prototype has no printed
  Unicode placeholders; image decode is included, rendering is not.
- `graphics-text.rs`: perform 100,000 two-byte text feeds on the alternate
  screen with 0, 64, 512 or 4,096 direct placements retained on the primary
  screen. Five rounds alternate case order; setup and warmup are excluded.
  This exposes image-placement scans in ordinary text parsing even when no
  images are visible.

Finish all builds before timing. Run one probe at a time, alternate baseline
and candidate order, and retain the raw output. Use
`scripts/benchmark-terminal-facades.py` separately for the seven standard
throughput workloads, both with and without damage consumption. None of these
probes measures GPU presentation or input-to-display latency.
