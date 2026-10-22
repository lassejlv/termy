# xtask

Repository automation binary.

## Owner

This crate owns maintainer commands that generate or verify repository
artifacts, including generated documentation and desktop benchmark reports.

Keep product runtime code out of this crate. If an automation command needs
shared domain data, depend on the smallest domain crate that owns that data.

## Validation

```sh
cargo test -p termy_cli --bin xtask
cargo run -p termy_cli --bin xtask -- generate-keybindings-doc --check
cargo run -p termy_cli --bin xtask -- generate-config-doc --check
cargo run -p termy_cli --bin xtask -- check-dependency-policy
```

## Forbidden Dependencies

- `termy_core::ffi`
- `termy` / `crates/desktop_app`
- product runtime workflows

The headless terminal engine benchmark lives in `crates/core/examples/terminal_engine_bench.rs`
and runs with `just benchmark-terminal-engine`. It exercises the same engine as
the native and display runtimes without depending on another emulator.

## Echo latency sampling

The native `echo-train` workload keeps 40 timed writes at roughly 250 ms
intervals. A fixed 250 ms interval is exactly 15 refresh periods at 60 Hz and
30 at 120 Hz, so two launches can sample different display phases throughout
the run. The driver varies the scheduled emission offsets by 0–16 ms in a
deterministic cycle to cover these phases. Both compared apps use this same
driver; latency timestamps still precede the actual write and flush.

The phase simulation tests cover launch offsets across a refresh period at
60 and 120 Hz, and verify that a real processing delay remains visible. They
do not replace the native macOS gate or prove the hosted runner is noise-free.
The 10 ms p95 regression threshold is unchanged.
