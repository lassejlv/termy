# Unsafe-code audit

Audit date: 2026-09-04. Scope: production Rust under `crates/*/src`. The release gate runs
`script/audit_unsafe.sh` and strict Clippy with `undocumented_unsafe_blocks = deny`; adding an unsafe
construct anywhere else fails the candidate until this document and the inventory are reviewed.

## Native PTY boundary

Allowed files: `crates/engine/src/pty/native.rs` and the narrow module allowance in
`crates/engine/src/pty/process.rs`.

Reviewed invariants:

- All command strings, argument/environment pointer arrays, the executable path, working directory,
  error channel, and descriptor list are fully allocated before `forkpty`; the child performs only
  fixed-storage, async-signal-safe syscalls before `execve` or `_exit`.
- The parent takes ownership of the one master descriptor returned by `forkpty`; failed setup kills
  and reaps the exact positive child. Successful descriptors are close-on-exec.
- Resize passes initialized `winsize` storage to `TIOCSWINSZ`. Process-group termination rejects
  leaders at or below one before using a negative group identifier. `waitpid` targets the exact
  child and retries interruption.
- Dropping an unreaped child kills its complete PTY process group and reaps it. The async session
  cancels blocked reads and joins its worker before owned state disappears.

Evidence: native PTY integration tests cover path resolution, controlling terminal/session/process
group, working directory and environment, bidirectional I/O, resize, exec failure, signal exit,
blocked-reader cancellation, descendants holding the slave open, backpressure, drop, and reap.

Residual contract: a process may open a new descriptor concurrently between the pre-fork inventory
and `forkpty`. Tmon marks its own descriptors close-on-exec and closes the prepared inventory in the
child; future process-wide descriptor additions must also be close-on-exec. Replacing this with a
spawn API is not currently possible because Tmon requires a controlling PTY and pre-exec session
setup. This is a reviewed resource-leak constraint, not a memory-safety finding.

## Experimental C ABI boundary

Allowed files: `crates/ffi/src/lib.rs`, `pty.rs`, `terminal.rs`, `types.rs`, and `util.rs`.

Reviewed invariants:

- Every exported record has `repr(C)` and fixed discriminant/value constants. Opaque handles come
  only from `Box::into_raw` and ownership returns exactly once through the matching free function.
- Null pointers, zero-length slices, UTF-8, enum values, dimensions, and writable out pointers are
  checked before safe engine operations. As with every C ABI, Rust cannot prove that a non-null
  foreign pointer is aligned, live, sized, or uniquely borrowed; those remain explicit host
  preconditions in `tmon.h`.
- Borrowed output views point into handle-owned reusable storage and have documented invalidation
  points. Calls against one handle are serialized and no handle may be freed during a call.
- PTY callback payloads remain alive for the callback duration. The spawn gate publishes the PTY
  handle before queued events are delivered. Callbacks must return normally, schedule work only,
  and never call or free the same handle from the reader thread.
- Exported calls contain Rust panics and map them to `TMON_PANICKED`; an abort, OOM, invalid foreign
  pointer, double free, data race, or foreign unwind cannot be recovered by this boundary.

Evidence: Rust FFI tests and the warnings-as-errors C consumer cover defaults, ABI version/layout,
null and invalid inputs, UTF-8 failures, panic/error diagnostics, construction/destruction, borrowed
frames/events/encoded bytes, callbacks, PTY I/O, resize, metrics, and process teardown.

## Result

No release-blocking unsafe-code finding was identified in the reviewed revision. The C surface
remains explicitly experimental before 1.0; any inventory change, ABI layout change, new syscall,
new callback behavior, or relaxation of handle serialization requires a fresh review.
