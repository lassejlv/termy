# Project Layout

Termy is a single repository with several product surfaces. Keep changes in the smallest owner that can express the behavior.

## Main Areas

- `crates/desktop_app/` owns the desktop app shell: GPUI windows, app chrome, settings, onboarding, menus, and app-only interaction behavior.
- `crates/desktop_app/src/terminal_view/` owns the GPUI terminal experience: rendering, tabs, panes, command palette, search UI, mouse/input handling, and app runtime coordination.
- `crates/core/` owns the reusable headless libtermy runtime/API and renderer-neutral terminal glyph semantics used by embedders. It must stay independent of GPUI and app UI code.
- `crates/tmon/` owns the experimental renderer-neutral terminal engine and native PTY/ConPTY runtime.
- `crates/plugin_runtime/` owns plugin discovery, descriptor/action validation, and the on-demand Bun host with one Worker per plugin. It must stay independent of GPUI and desktop command execution.
- `crates/terminal_ui/` owns GPUI terminal painting, final pixel snapping, and keystroke adapters plus tmux pane display/client support used by the desktop app. Shared headless terminal types and special-glyph plans are imported directly from `termy_core`.
- `crates/multiplexer/` owns the built-in background session host and remote clients. Terminal buffers and parser state remain in `termy_core`; desktop window and workspace layout remain app-owned.
- `crates/tmux_control_core/` owns the UI-agnostic tmux control-mode protocol and transport shared by terminal UI and FFI.
- `crates/ui/` owns Termy's design system in GPUI: theme-derived color tokens, layout metrics, and stateless chrome components. It must stay free of config, command, plugin, and SSH domain crates.
- `crates/config_core/`, `crates/command_core/`, `crates/theme_core/`, and `crates/search/` own pure domain logic shared by the app, CLI, docs generation, and embedding surfaces.
- `crates/ssh_core/` owns saved SSH host validation, non-secret persistence, exact OpenSSH arguments, and system-keychain credential lifecycle.
- `crates/ffi/` exposes libtermy to C-compatible hosts.
- `website/` owns the public website and user-facing docs.
- `docs/` owns contributor and architecture docs inside the repo.
- `scripts/` owns packaging, platform, and release support.
- `assets/` owns app icons, shell completions, UI icons, and media.
- `tools/` owns standalone benchmark and development packages outside the main Cargo workspace.
- `skills/` owns agent-facing workflows that must stay aligned with Termy's public contracts.

## Workspace Crates

- `termy` (`crates/desktop_app/`) is the product app and the only crate that should own complete user-facing desktop workflows.
- `termy_core` (`crates/core/`) is the headless runtime/API for embedders.
- `tmon` (`crates/tmon/`) is the experimental standalone terminal engine.
- `termy_plugin_runtime` (`crates/plugin_runtime/`) is the GPUI-free TypeScript plugin runtime consumed by the desktop app.
- `termy_terminal_ui` (`crates/terminal_ui/`) is the GPUI-facing terminal adapter used by the desktop app.
- `termy_multiplexer` (`crates/multiplexer/`) keeps native terminal sessions alive independently of the desktop process.
- `termy_tmux_control_core` (`crates/tmux_control_core/`) is the shared headless tmux control-mode layer.
- `termy_ui` (`crates/ui/`) is the GPUI design system: tokens, metrics, and chrome components shared by settings-style surfaces.
- `termy_command_core`, `termy_config_core`, `termy_theme_core`, `termy_search`, `termy_ssh_core`, and `termy_themes` are pure domain crates.
- `termy_ffi` and `termy_native_sdk` are embedding/native-integration surfaces.
- `termy_cli`, `termy_cli_install_core`, `termy_release_core`, and `termy_auto_update` own command-line, install, release, and update support.
- Desktop update presentation and toast state live under `crates/desktop_app/src/ui/` because they have no non-desktop consumers.
- `xtask` owns repository automation such as generated docs.

Each crate should have a local `README.md` with `Owner`, `Validation`, and `Forbidden Dependencies` sections.
The `crates/README.md` file is the workspace crate index.

## Boundary Rules

- Keep `termy_core` headless. Do not add GPUI, app chrome, command palette, or desktop-window concerns there.
- Keep `termy_plugin_runtime` headless. It may return typed plugin commands and actions, but the desktop app owns their presentation and execution.
- Keep pure domain crates free of GPUI and app-specific presentation code.
- Keep `termy_ffi` aligned with the reusable core contract rather than copying desktop app behavior.
- Put user-visible app behavior in `crates/desktop_app/`; extract to sibling crates only when the behavior is shared by multiple surfaces or needs isolated tests.
- Update repo docs and public website docs together when a public behavior or embedding contract changes.
- Keep release packaging rooted in `scripts/` and documented in [Release Packaging](release-packaging.md). Do not add parallel packaging entrypoints unless they become the documented source of truth.
- Keep root-level directory indexes (`crates/README.md`, `docs/README.md`, `scripts/README.md`, `skills/README.md`, and `tools/README.md`) aligned with ownership changes.

## Validation

Use `just check-boundaries` after changing crate dependencies, crate README metadata, generated docs, dependency policy, command/keybind behavior, or config behavior. Use `cargo check --workspace` after moving modules or changing cross-crate contracts.

For release planning and codebase quality gates, see [ROADMAP.md](../../ROADMAP.md) and [docs/engineering/](../engineering/).
