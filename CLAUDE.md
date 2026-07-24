# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Termy is a terminal emulator built with GPUI (Zed's UI framework) and `alacritty_terminal`. It is a Rust workspace plus a public-beta native macOS SwiftUI host (`macos/`) and a website (`website/`).

## Commands

```sh
just                  # list all recipes
cargo run -p termy    # run the desktop app (release: just run)
just check            # cargo check --workspace
just test             # desktop app tests (release)
just test-workspace   # all workspace crate tests (release) — matches CI
just fmt-check        # cargo fmt --all -- --check
just check-boundaries # crate boundaries, README metadata, generated docs, dependency policy, file sizes
just validate         # full local gate: check + fmt + tests + boundaries + clippy -D warnings
```

Single test: `cargo test -p <crate> <test_name>` (e.g. `cargo test -p termy_config_core`).

Use the smallest validation pass that proves a change: `cargo check -p termy` for UI tweaks, `cargo test -p <crate>` for domain logic, `just check-boundaries` after changing crate dependencies, crate README metadata, generated docs, dependency policy, command/keybind behavior, or config behavior, `just validate` before large PRs.

tmux integration tests (require tmux ≥ 3.3, ignored by default): `just test-tmux-integration`.

macOS Swift host: `just run-macos` (builds `crates/ffi` first, then SwiftPM app at `macos/dist/Termy.app`), `just test-macos-config` (Swift config/schema parity tests), `just test-macos-stress`, `just check-macos-launch`, `just check-macos-release`.

Website (`website/`): bun + Vite + TanStack Start + fumadocs. `bun run dev`, `bun run build`, `bun run lint` (oxlint), `bun run types:check`.

## Generated files — do not edit by hand

- `docs/keybindings.md` → regenerate with `just generate-keybindings-doc`
- `docs/configuration.md` → regenerate with `just generate-config-doc`
- Verify with `just check-keybindings-doc` / `just check-config-doc`

## Architecture

The workspace is split by **ownership boundary**, not implementation convenience. Put behavior in the smallest crate that owns it; `scripts/check-boundaries.sh` enforces dependency rules.

Product surfaces:
- `crates/desktop_app/` (`termy`): the GPUI desktop app — windows, chrome, settings, menus, command execution. The only crate that owns complete user-facing desktop workflows. `src/terminal_view/` owns the GPUI terminal experience (rendering, tabs, panes, command palette, search UI, input handling).
- `crates/cli/` (`termy_cli`): `termy-cli` companion.
- `crates/ffi/` (`termy_ffi`): C-compatible libtermy surface; must wrap `termy_core`, not copy desktop app behavior. Header at `crates/ffi/include/termy.h`.
- `macos/`: public-beta native SwiftUI host consuming libtermy via FFI. Swift tests live in `macos/Tests/TermySwiftTests/`; config schema parity between Rust and Swift is tested (`SettingsSchemaParityTests`).

Runtime and UI:
- `crates/core/` (`termy_core`): headless terminal runtime/API for embedders. **Must stay free of GPUI and app UI code.**
- `crates/terminal_ui/` (`termy_terminal_ui`): GPUI-facing terminal adapter, grid paint cache, native pane runtime, tmux support.
- `crates/native_sdk/`: narrow platform-native helpers.

Pure domain crates (no GPUI, no app presentation): `command_core` (command IDs/keybind-facing definitions — must stay independent of config parsing and UI), `config_core` (config schema/defaults), `theme_core`, `themes`, `search`.

Release/install/support: `release_core`, `auto_update`, `auto_update_ui`, `cli_install_core`, `toast_sdk`, `xtask` (repo automation, generated-doc checks, render benchmarks).

Each crate has a local `README.md` with `Owner`, `Validation`, and `Forbidden Dependencies` sections; update it when ownership, test commands, or dependency boundaries change. `crates/README.md` is the workspace index.

## Cross-cutting change recipes

Config key changes: update schema in `crates/config_core`, keep parsing/defaults/rendering in sync, regenerate config docs. The macOS Swift host mirrors the schema (`macos/Sources/TermySwift/Models/SettingsSchema.swift`) — run `just test-macos-config` for parity.

Command/keybind changes: update the catalog in `crates/command_core`, wire the action through `crates/desktop_app/src/`, regenerate keybinding docs if defaults or public names changed.

Docs placement: contributor docs in `docs/`, public user docs in `website/content/`. Update docs in the same PR as behavior changes.

Packaging stays rooted in `scripts/` (see `docs/architecture/release-packaging.md`); do not add parallel packaging entrypoints.

## Debugging rendering

Debug-only render churn metrics: `RUST_LOG=info TERMY_RENDER_METRICS=1 cargo run -p termy` (not emitted in `--release`). Real-window render benchmarks (macOS only, needs `xctrace`): `cargo run -p xtask -- benchmark-compare ...` — see `docs/development.md`.

## Style

- Prefer small, explicit code paths over clever abstractions.
- Clippy is strict in `just validate` (`-D warnings`); workspace lints in root `Cargo.toml` warn on `dbg!`, `todo!`, `unimplemented!`, redundant clones, uninlined format args, etc.
- Do not add `#[allow(clippy::cognitive_complexity)]` unless the same line links the tracking issue.
- Avoid unrelated drive-by changes in the same PR.
