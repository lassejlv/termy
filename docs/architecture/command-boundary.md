# Command Boundary

This document defines ownership boundaries for command and keybind behavior.

## Ownership

- `termy_core::command_core` owns:
  - Command IDs and config-facing command names.
  - Command config-name parsing and normalization.
  - Keybind defaults.
  - Keybind directive parsing (`clear`, bind, unbind).
  - Deterministic keybind resolution order.
- App/CLI adapters own:
  - UI labels, keywords, and command-palette presentation.
  - Platform-specific visibility policy for palette entries.
  - UI trigger canonicalization and validation (for example GPUI keystroke parsing).

## Dependency Rule

- `termy_core::command_core` must remain a pure domain crate.
- `termy_core::command_core` must not depend on:
  - `termy_core::config_core`
  - `gpui`
  - other UI or presentation crates

## Integration Pattern

- Adapters convert parsed config keybind lines into `termy_core::command_core::KeybindLineRef`.
- Adapters call `parse_keybind_directives_from_iter`; trigger canonicalization happens in `termy_core::command_core`.
- Adapters call `resolve_keybinds` over `default_resolved_keybinds`.

This keeps one canonical command/keybind engine while preserving thin and readable adapter code.
