# Security fuzzing

Tmon treats PTY output, daemon snapshots, and local multiplexer frames as untrusted input. The
release fuzz smoke exercises those three decoder boundaries with libFuzzer:

- `terminal-feed` creates a bounded terminal and feeds arbitrary bytes across deliberately uneven
  chunk boundaries;
- `snapshot-decode` attempts to restore arbitrary bytes through the production snapshot decoder;
  and
- `mux-frame-decode` exercises the production request-frame length and postcard decoder.

Run the bounded release smoke from the repository root:

```sh
bash script/fuzz_smoke.sh
```

The default is 30 seconds per target with a 2 GiB RSS kill limit and a five-second per-input
timeout. Longer security runs can set `TMON_FUZZ_SECONDS_PER_TARGET`; the deterministic unit suite
separately covers exact multi-megabyte size boundaries that mutation-based smoke runs may not reach.

A fuzz crash blocks release. Preserve the minimized input in `fuzz/corpus/<target>/`, add a focused
deterministic regression test, fix the underlying boundary, and rerun both the regression and all
three fuzz targets. Corpus growth and build artifacts remain ignored; only deliberately reviewed,
minimal regression seeds should be committed.

Local evidence from the first complete three-target run is recorded in
`security/fuzz-smoke-2026-09-04.md`. The scheduled workflow repeats a 60-second smoke per target on
the pinned `nightly-2026-09-03` instrumentation toolchain.
