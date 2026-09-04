# Repository Guidelines

## Project Structure & Module Organization

Tmon is a Rust 2024 workspace for a native macOS terminal. Workspace crates live under `crates/`: `app` builds the `tmon` binary, `engine` owns terminal state and parsing, `mux` manages persistent sessions, `pty` wraps the native PTY, `render` provides Metal-backed drawing, and `ffi` exposes the C/Swift ABI. Keep platform boundaries intact: terminal behavior belongs in `engine`, while windowing and GPU work belong in `app` or `render`. Integration tests sit in each crate's `tests/` directory; shaders, icons, packaging templates, performance reports, and developer utilities live in `crates/render/src/`, `assets/`, `packaging/`, `performance/`, and `script/` respectively.

## Build, Test, and Development Commands

- `cargo run --release -p tmon` runs the terminal locally; pass a command after `--`, such as `/usr/bin/top`.
- `cargo build --workspace` compiles all crates.
- `cargo test --workspace` runs unit and integration tests.
- `cargo fmt --all -- --check` verifies formatting.
- `cargo clippy --workspace --all-targets -- -D warnings` enforces the workspace lint policy.
- `bash script/test_ffi_c.sh` builds the ABI and exercises a real C consumer.
- `bash script/package_macos.sh --universal` creates the universal `dist/Tmon.app` release bundle.
- `bash script/performance_gate.sh --samples 30` runs the full release performance gate; use an unobscured Tmon window for its Metal workload.

## Coding Style & Naming Conventions

Use four-space indentation and let `rustfmt` decide layout. Follow Rust conventions: `snake_case` for modules, functions, and tests; `CamelCase` for types and traits; `SCREAMING_SNAKE_CASE` for constants. Prefer explicit ownership and bounded buffers in PTY/rendering hot paths. Workspace lints enable Clippy `all` and `pedantic`; unsafe code is limited to the audited FFI and native PTY boundaries.

## Testing Guidelines

Add focused unit tests beside small modules and cross-component tests under `crates/<crate>/tests/`. Name tests after observable behavior, for example `oversized_output_is_lossless_ordered_bounded_and_rearms_wakes`. Run the affected crate first (`cargo test -p engine`), then the workspace suite. Performance-sensitive changes should also run the relevant release example or benchmark documented in `performance/README.md`. No numeric coverage threshold is enforced; regressions need a targeted test.

## Commit & Pull Request Guidelines

Recent commits use short, imperative, sentence-case subjects such as `Set Tmon app icon`; keep each commit focused. Pull requests should explain user-visible behavior, list verification commands, and note macOS/hardware assumptions. Link related issues and include screenshots or recordings for UI/rendering changes. Call out ABI, protocol, config, or performance impacts explicitly, and avoid committing generated `target/` or local `dist/` artifacts.
