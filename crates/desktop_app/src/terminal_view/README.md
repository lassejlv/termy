# Terminal view

This directory owns the complete GPUI terminal experience inside the desktop
app. Reusable terminal semantics belong in `termy_core`; reusable GPUI grid and
tmux adapters belong in `termy::terminal_ui`.

## Find behavior by task

- `backend.rs`: desktop facade over native and tmux terminal sources.
- `session.rs`, `workspaces.rs`, and `persistence.rs`: coherent tab, pane,
  workspace, and saved-session state.
- `tabs/`: tab identity, lifecycle, drag, reset, and sizing behavior.
- `tab_strip/`: tab chrome, layout, hit testing, gestures, and painting.
- `interaction/`: keyboard, mouse, selection, scroll, context-menu, and
  terminal action routing.
- `runtime/`: app runtime state and tmux orchestration; low-level tmux protocol
  handling remains in `termy::terminal_ui` and `termy_core::tmux_control_core`.
- `render.rs`, `render_cache.rs`, `appearance.rs`, and `metrics.rs`: terminal
  paint orchestration, cached cells, visual policy, and render diagnostics.
- `command_palette/`: palette state, presentation, built-in actions, and plugin
  command integration. Public command definitions remain in
  `termy_core::command_core`.
- `plugin_ui.rs` and `plugin_ui/`: desktop presentation and interaction for
  validated plugin documents returned by `termy_core::plugin_runtime`.
- `search.rs`, `inline_input.rs`, and `overlay_view.rs`: terminal overlays and
  focused input surfaces.
- `titles/`: title source tracking, formatting, and asynchronous updates.

`mod.rs` should remain orchestration and shared `TerminalView` state rather
than becoming the default home for new behavior. Follow the
[decomposition plan](../../../../docs/engineering/terminal-view-decomposition.md)
when moving existing code; use behavior-preserving vertical slices instead of
a broad rewrite.

## Validation

```sh
cargo check -p termy
cargo test -p termy
```

Run the desktop app manually when rendering or interaction behavior changes.
