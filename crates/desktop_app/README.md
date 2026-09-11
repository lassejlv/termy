# termy

Main desktop application.

## Owner

This crate owns the GPUI app shell, windows, titlebar/chrome, menus, settings, onboarding, command execution, and user-visible desktop workflows.

Important internal areas:

- `src/terminal_view/`: terminal surface, tabs, panes, search, command palette, input, rendering, persistence, and runtime coordination. `session.rs` owns coherent tab/workspace/pane state; `backend.rs` owns the core-native/tmux facade and the source-specific cell presentation policy.
- `src/settings_view/`: settings UI and state application.
- `src/onboarding/`: first-run and import flows.
- `src/config/`: app-owned config I/O and mutation.
- `src/ui/`: desktop-only presentation and state, including update banners, toasts, and scrollbars.

Push reusable headless behavior into `termy_core` or a pure domain crate. Push GPUI-adjacent terminal adapter behavior into `termy_terminal_ui` only when it is reusable outside the desktop app shell. Push reusable chrome presentation — surfaces, controls, status affordances — into `termy_ui`, and keep the state and behavior behind it here.

`src/settings_view/` renders its section headers and grouped cards with `termy_ui`. Its colors are published to the kit by `SettingsWindow::sync_ui_tokens`, which maps this window's own translucent chrome colors onto `termy_ui::Tokens`; do not swap that for `Tokens::from_palette`, which is opaque and would drop the window's transparency.

## Kitty graphics

The terminal surface renders static images sent through the Kitty graphics
protocol. The shared terminal core handles APC parsing, direct and file-backed
transfers, chunking, PNG/RGB/RGBA data, zlib compression, placements, deletion,
quiet-mode replies, source rectangles, cursor movement, and storage limits. The
desktop renderer handles clipping, cell/pixel sizing, z-index ordering, and GPU
image caching. Unicode placeholder placements and relative placement chains are
supported across the native, experimental Tmon, and tmux-pane paths, including
placeholder-driven clearing. Animations and shared-memory transfers are not
currently supported.

Natural-size placements (no `c`/`r`) use 1:1 image pixels and **truncate** on
the right edge of the screen from the cursor, matching Kitty — they do not
scale-to-fit. Explicit `c`/`r` (used by clients such as Grok Build’s image
preview via `fit_image_to_cells`) scale the image into that cell rectangle.
PTY `TIOCGWINSZ` pixel metrics are derived from cell size and never report zero,
so clients can size placements correctly.

## Validation

macOS startup resolves an installed terminal font directly before falling back
to the complete font catalog and fixed-pitch validation. A packaged default icon
is supplied by macOS; startup only replaces it when the selected icon or a custom
Finder icon requires that. Config reloads skip reapplying an unchanged icon.
`TERMY_LAUNCH_PROBE_FILE` records the first usable frame and optional startup-stage
timings. Measurements and limitations are recorded in
[`performance-2026-09-11.md`](../../docs/engineering/performance-2026-09-11.md).

```sh
cargo test -p termy
cargo check -p termy
```

## Forbidden Dependencies

- `termy_ffi`
- native host app packages
- website packages

## Kitty graphics visual check

Run `python3 crates/desktop_app/examples/kitty_graphics_conformance.py` inside
Termy. The alternate-screen demo covers natural and cell-based sizes, crops and
pixel offsets, all three z layers, repeated Unicode placeholders with gaps,
clipped relative placements, animation, margin scrolling, and final-chunk
cursor placement. Space switches pages, R redraws, and Q restores the shell.
Use `--scroll` to start on the scrolling page. Resize the window while viewing
each page. The test is intentionally synthetic; application-specific behavior
still needs testing in the relevant application.
