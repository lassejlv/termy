# Testing strategy

Where tests live and which command to run for a given change.

## Pyramid

| Layer | What | Where | Command |
|-------|------|-------|---------|
| **Unit** | Pure logic, parsers, catalogs | `config_core`, `command_core`, `search`, `core`, inline `#[test]` | `cargo test -p <crate>` |
| **Integration** | Tmux client, grid, FFI | `terminal_ui/tests/`, `ffi` | `cargo test -p termy` |
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

## Linux X11 startup and titlebars

On an X11 or XWayland desktop, launch `termy` from a terminal, open a second
window, and open Settings. All windows should open with system-managed titlebars
without a panic. Repeat with `TERMY_LINUX_BACKEND=wayland` on a Wayland desktop.

GPUI 0.2.2's X11 `HasWindowHandle::window_handle` is unimplemented and panics
instead of returning an error. Do not query it to set `_GTK_THEME_VARIANT`;
leave titlebar theming to the window manager until the backend supports it.
GPUI test windows do not exercise this native backend, so unit tests alone
cannot validate Linux startup.

## New-tab working directory (#388)

The desktop regression tests launch a real native PTY and an isolated tmux
server. Each changes the shell directory, starts a foreground process, creates a
new tab, and checks its actual directory. The tmux case also leaves stale prompt
metadata in place and verifies explicit directory overrides.

```sh
cargo test -p termy --bin termy working_dir_tests -- --include-ignored --nocapture
```

The tmux case requires tmux >= 3.3 and runs in `just test-tmux-integration`. For a manual check, change into a project,
start lazygit, and create a tab with `secondary-t`; `pwd` in the new tab should
show that project. Repeat with tmux enabled and disabled.

## Kitty images in persistent sessions

The remote graphics regressions compare local and multiplexed placements while
output scrolls, the viewport moves through history, and Unicode placeholders
are moved or erased. Run them against both terminal engines:

```sh
TERMY_CORE_TEST_BACKEND=alacritty cargo test -p termy_core --test remote_graphics
TERMY_CORE_TEST_BACKEND=tmon cargo test -p termy_core --test remote_graphics
cargo test -p termy_core --test ipc kitty_images_follow_pty_scrolling
```

The IPC test uses an isolated session host and a real PTY. It checks that images
move with text, disappear above the viewport, and return when scrolling
back. Remote placements must refresh on viewport changes even when the image
revision stays unchanged.

## Selection in persistent sessions

Visible selection reads must use the displayed viewport without an IPC round trip.
Scroll commands must make their updated viewport available before the desktop
records its selection baseline, including when connected through the legacy
graphics protocol. Otherwise a delayed user scroll looks like incoming output
and shifts the selection anchor.

```sh
cargo test -p termy_core --test remote_selection
cargo test -p termy_core --test ipc scrolling_reply_updates_viewport
cargo test -p termy --bin termy multiplexer_text_selection_survives_scrolling_and_output
```

Repeat the core commands with `TERMY_CORE_TEST_BACKEND=alacritty` and `tmon`.
The desktop regression drives mouse down, dragging, wheel scrolling, and release
against an isolated session host, then verifies the selected text survives new
output while viewing history.

## Ignored tests

- `crates/desktop_app/tests/tmux_split_integration.rs` — requires **tmux ≥ 3.3** locally.
- Run: `just test-tmux-integration`
- CI: macOS `architecture-checks` job (when tmux available).

Every `#[ignore]` must reference a tracking issue in a comment.
`just check-boundaries` enforces that the repo stays at or below 11 ignored tests.

## Before opening a PR

Use the **smallest** pass that proves your change:

```sh
cargo check -p termy              # UI-only tweak
cargo test -p termy_core   # config schema/parser
just check-boundaries             # deps, generated docs, commands
just test-workspace               # broad Rust change (target: matches CI E0.1)
just validate                     # full local gate (once E0 lands)
```

## Roadmap

CI parity and tmux reliability: [roadmap.md](roadmap.md) phase E0–E2.

## Settings consistency and tab shortcuts (#386, #387)

The UI audit found that keyboard shortcuts used a separate card border and
12px control corners, variable control heights, wide Clear buttons, and raw
config names as descriptions. Plugin controls were 228×28px with 40px text
reset buttons, while built-in controls were 300×30px with 22px icon actions.
Shortcut cards now use the same grouped-card renderer; shortcut and plugin
controls share built-in dimensions, and reset/clear actions use one renderer
with consistent sizing, hover feedback, tooltips, and empty action slots.

Keyboard shortcuts now begin with one **Switch tabs** entry (`cycle_tabs`,
`Ctrl+Tab` by default). **More tab shortcuts** reveals the optional directional
and numbered bindings without changing their configuration. Search for
“tab switch” to find the section.

Run `cargo test -p termy --bin termy settings_view` for rendered expansion,
capture/cancel, control dimensions, and binding round-trip checks. For a visual
check, compare General, Keyboard shortcuts, and plugin settings at the minimum
window size; controls and row actions should align. Verify recording, clearing,
and Escape cancellation, then reload Settings to check persistence.
