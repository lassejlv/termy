# Testing strategy

Where tests live and which command to run for a given change.

## Pyramid

| Layer | What | Where | Command |
|-------|------|-------|---------|
| **Unit** | Pure logic, parsers, catalogs | `config_core`, `command_core`, `search`, `core`, inline `#[test]` | `cargo test -p <crate>` |
| **Integration** | Tmux client, grid, FFI | `terminal_ui/tests/`, `ffi` | `cargo test -p termy_terminal_ui` |
| **App** | GPUI terminal view, settings, commands | `desktop_app` | `just test` |
| **Manual** | Visual chrome, GPU paint | — | Run app; see [development.md](../development.md) render metrics |

## macOS titlebar dragging (#391)

Run the built app and drag both the Termy branding and empty titlebar space,
with a single tab (auto-hidden tab strip) and with multiple visible tabs. The
window should follow the pointer. Repeat a drag after releasing the mouse;
AppKit can consume mouse-up during native window movement. Also verify that
double-click still performs the system titlebar action, tabs can be selected
and reordered, the new-tab button works, and terminal text selection does not
move the window.

The pinned GPUI 0.2.2 `start_window_move` is a no-op on macOS. Termy calls
AppKit's `performWindowDragWithEvent:` from the hit-tested mouse-down handler;
state-machine tests alone cannot verify this native handoff.

## Linux close confirmation (#390)

`cargo test -p termy --bin termy linux_prompt::tests` dispatches keyboard and
mouse input through GPUI's rendered prompt, checks that the terminal underneath
receives neither, and verifies focus restoration after cancellation. The quit
interaction tests also exercise cancellation, repeated close requests, and
confirmed window closure with a real `TerminalView`.

For a desktop smoke check on Linux, set `warn_on_quit = true`, open a terminal,
then request window close. Confirm and cancel with both the buttons and
Enter/Escape; Tab and arrow keys should select buttons. Clicking outside the
prompt must leave it open without focusing or sending input to the terminal.
Repeating the close request must not bypass confirmation.

## Ignored tests

- `crates/terminal_ui/tests/tmux_split_integration.rs` — requires **tmux ≥ 3.3** locally.
- Run: `just test-tmux-integration`
- CI: macOS `architecture-checks` job (when tmux available).

Every `#[ignore]` must reference a tracking issue in a comment.
`just check-boundaries` enforces that the repo stays at or below 10 ignored tests.

## Before opening a PR

Use the **smallest** pass that proves your change:

```sh
cargo check -p termy              # UI-only tweak
cargo test -p termy_config_core   # config schema/parser
just check-boundaries             # deps, generated docs, commands
just test-workspace               # broad Rust change (target: matches CI E0.1)
just validate                     # full local gate (once E0 lands)
```

## Roadmap

CI parity and tmux reliability: [roadmap.md](roadmap.md) phase E0–E2.
