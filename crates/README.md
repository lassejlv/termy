# Crates

Termy is a Rust workspace split by ownership boundary, not by implementation convenience. Prefer the smallest crate that owns the behavior.

## Product Surface

- `desktop_app/` (`termy`): GPUI desktop app, windows, app chrome, settings, onboarding, command execution, and user-visible desktop workflows.
- `cli/` (`termy_cli`): `termy-cli` command-line companion.
- `ffi/` (`termy_ffi`): C-compatible libtermy surface.

## Runtime And UI

- `core/` (`termy_core`): headless terminal runtime/API and renderer-neutral terminal glyph semantics for embedders.
- `tmon/` (`tmon`): experimental renderer-neutral terminal engine and native PTY/ConPTY runtime.
- `plugin_runtime/` (`termy_plugin_runtime`): plugin discovery, typed protocol validation, and the on-demand Bun/Worker runtime.
- `terminal_ui/` (`termy_terminal_ui`): GPUI grid painting/pixel snapping and keystroke adapters plus tmux pane display/client support; shared terminal types and special-glyph plans come directly from `termy_core`.
- `tmux_control_core/` (`termy_tmux_control_core`): UI-agnostic tmux control-mode protocol, session, and transport logic shared by terminal UI and FFI.
- `ui/` (`termy_ui`): Termy's design system in GPUI — theme-derived tokens and stateless chrome.
- `native_sdk/` (`termy_native_sdk`): narrow platform-native helpers, including file-manager "Open new Termy tab here" registration, MIME-aware system clipboard access, and permission prompts.

## Pure Domain Crates

- `command_core/` (`termy_command_core`): command IDs, keybinding-facing definitions, and command metadata.
- `config_core/` (`termy_config_core`): config schema, defaults, and config-facing types.
- `theme_core/` (`termy_theme_core`): theme data model.
- `themes/` (`termy_themes`): bundled theme definitions.
- `search/` (`termy_search`): reusable terminal search primitives.
- `ssh_core/` (`termy_ssh_core`): saved SSH host validation, non-secret persistence, OpenSSH launch arguments, and keychain-backed credentials.

## Release, Install, And Support

- `release_core/` (`termy_release_core`): release metadata and version helpers.
- `auto_update/` (`termy_auto_update`): update discovery, verification, and platform update decisions.
- `cli_install_core/` (`termy_cli_install_core`): shared CLI install/path helpers.
- `xtask/` (`xtask`): repository automation and generated-doc checks.

Each crate has its own `README.md` with `Owner`, `Validation`, and `Forbidden Dependencies` sections. Update the local README when a crate gains or loses ownership of a responsibility, changes its test command, or changes its dependency boundary.

## Dependency Rules

- `termy_core`, `termy_ffi`, `termy_cli`, `termy_cli_install_core`, and pure domain crates must not depend on GPUI.
- `termy_ffi` should wrap `termy_core`, not copy desktop app behavior.
- `termy_command_core` must stay independent of config parsing and UI presentation.
- `termy_plugin_runtime` must stay independent of GPUI, desktop command execution, and terminal presentation.
- `termy_ui` owns presentation only: it must not depend on config, command, plugin, or SSH crates, and must not reach back into `desktop_app/`.
- App-only behavior belongs in `desktop_app/` until another product surface needs it.

Run `just check-boundaries` after changing crate dependencies, crate README metadata, generated docs, command/keybind behavior, or config behavior.
