# Tmon fuzz targets

These targets cover the three untrusted decoders in the production roadmap:

- `terminal-feed`: arbitrary PTY output with chunk boundaries;
- `snapshot-decode`: daemon terminal snapshots; and
- `mux-frame-decode`: length-prefixed client protocol frames.

Install `cargo-fuzz`, then run each target from the repository root:

```sh
cargo +nightly fuzz run terminal-feed
cargo +nightly fuzz run snapshot-decode
cargo +nightly fuzz run mux-frame-decode
```

Keep minimized reproductions under `fuzz/corpus/<target>/`. Crashes must become deterministic unit
or integration tests before they are closed. Fuzzing is a dedicated release/security job rather
than part of the ordinary deterministic CI gate. `bash script/fuzz_smoke.sh` runs the bounded
release smoke for all targets; set `TMON_FUZZ_SECONDS_PER_TARGET` to extend its default 30-second
budget.
