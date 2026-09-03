# Current Goal

## Objective
Replace `portable-pty` with a fast, ergonomic Tmon-owned native macOS PTY implementation without regressing terminal process behavior.

## Status
completed

## Definition of done
- [x] Tmon opens, configures, spawns, reads, writes, resizes, signals, waits for, and tears down PTY children through repository-owned code, with no `portable-pty` dependency remaining.
- [x] The existing `PtySession` and FFI-facing API stays simple and compatible while shell startup, environment, working directory, interactive I/O, resize, exit reporting, process-group teardown, and bounded lossless output behavior are covered by tests.
- [x] Targeted PTY tests, workspace build/lint checks, and a sustained-output performance check pass on macOS; any unrelated pre-existing failure is identified separately.
- [x] PTY architecture and platform scope are documented so future callers can use and maintain it without depending on hidden `portable-pty` behavior.

## Constraints
- Preserve the current macOS-only product scope and existing multiplexer/session behavior.
- Keep unsafe OS interop isolated, minimal, documented, and outside the terminal emulator logic.
- Optimize the hot path for direct file-descriptor I/O, bounded memory, coalesced wakeups, and suppressed duplicate resize syscalls; avoid speculative abstractions or cross-platform shims.
- Do not undo compatible external workspace edits.

## Progress
- Added the macOS-only `tmon-pty` crate with a safe command/master/child API and an isolated native syscall boundary.
- Rewired `engine::pty::PtySession` to direct `File` descriptors while preserving its caller-facing API, bounded queue, wake coalescing, cancellation, and resize metrics.
- Removed `portable-pty` and `filedescriptor` from manifests and the lockfile; repository search finds no remaining references.
- Documented architecture, safety/platform boundaries, direct usage, and the sustained-output gate in the root and crate READMEs.

## Evidence
- `cargo test -p tmon-pty` passes 3 direct tests for controlling TTY/process group, environment/cwd, PATH lookup, bidirectional I/O, exec errors, signals, and wait.
- `cargo test --workspace` passes the complete workspace, including 9 engine PTY tests, the FFI PTY test, and 2 detach/reconnect multiplexer tests; the previously hanging stubborn-descendant teardown test now completes.
- The new 8 MiB sustained-output gate passes at 13.7 MiB/s in debug and 11.4 MiB/s in release on the current macOS host.
- `cargo check --workspace --all-targets`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo fmt --all -- --check` pass.
- `bash script/test_ffi_c.sh` passes the real C ABI smoke test.
- Final repository search finds no `portable-pty` or `filedescriptor` dependency/code references; unsafe sites are confined to documented syscalls in `crates/pty/src/native.rs`.

## Next action
Clear the saved goal when its record is no longer needed.

## Blocker
None.
