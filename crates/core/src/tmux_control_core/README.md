# termy_core::tmux_control_core

Shared, UI-agnostic core for tmux **control mode** (`tmux -CC`): command-line construction, payload escaping, the control-stream parser/state machine, notification coalescing, session launch, and worker channel plumbing.

## Owner

This module owns tmux control-mode contracts that must be shared by `termy::terminal_ui` and `termy_core::ffi`. It has no GPUI, app UI, or `termy::terminal_ui` dependency.

Pane/layout integration and GPUI state live in consumers such as `termy::terminal_ui::tmux`.

## Validation

```sh
cargo test -p termy_core
```

## Boundaries

GPUI and desktop application state. Shared helpers are sibling core modules.
