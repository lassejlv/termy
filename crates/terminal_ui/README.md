# termy_terminal_ui

GPUI-facing terminal runtime support for the desktop app.

## Owner

This crate owns the terminal grid paint cache, GPUI painting and final pixel snapping, GPUI keystroke adapter, tmux pane display runtime, and tmux client support used by `crates/desktop_app/src/terminal_view/`. Tmux panes route Kitty OSC 5522 clipboard requests and paste-event mode controls through the same desktop clipboard policy as native terminals. Renderer-neutral special-glyph semantics come from `termy_core`; this crate converts those shared plans into GPUI quads and paths.

Box lines and rounded corners resolve against the same cell bounds snapped in physical pixels. Stroke widths are calculated after applying the window's display scale, then converted back to GPUI's logical coordinates for painting. Resolve these plans at paint time so moving a window between display scales does not reuse stale stroke geometry.

## Validation

```sh
cargo test -p termy_terminal_ui
```

## Forbidden Dependencies

- `termy_ffi`
- `termy` / `crates/desktop_app`
- app settings, workspace stores, or command execution workflows

`TerminalGrid::split_background()` shares cached row operations between the
background and foreground passes when Kitty images must be painted between
cell backgrounds and text. Panes without such images keep one grid pass.

The paint cache retains immutable source rows and their cursor/hover decorations.
When an engine reports a full redraw, unchanged or shifted rows can reuse their
draw operations and shaped text without rebuilding them. Tmon's explicit scroll
operations rotate the retained rows directly; exposed rows and cursor transitions
are rebuilt. Every frame still paints all visible rows because GPUI does not
preserve the previous frame's pixels. Font, color, selection, and geometry changes
remain part of cache validation. Clearing a hidden pane also drops retained cells.

Run the bounded scroll preparation comparison with:

```sh
cargo test --locked --release -p termy_terminal_ui repeated_full_damage_scrolling -- --nocapture
```

This compares row preparation with and without source-row reuse and checks that
both paths produce the same final operations. It does not measure GPU frame time.
