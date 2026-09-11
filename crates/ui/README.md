# termy_ui

Termy's settings design system as reusable GPUI components.

## Owner

This crate owns Termy's chrome vocabulary: color tokens derived from the active
terminal theme, layout metrics, and the stateless components built on them —
sidebar, section headers, grouped cards, setting rows, controls (select,
stepper, switch, slider, text field, segmented control, shortcut box), and
status surfaces (badge, banner, toast, empty state).

Keep product behavior out of here. Config keys, plugin inventory, SSH hosts, and
command execution stay in `crates/desktop_app/`; this crate only knows how a
value should look once someone hands it over. A component that cannot be
rendered from plain arguments belongs to the surface that owns the state.

Numbers mirror the shipped settings window
(`crates/desktop_app/src/settings_view/`), so a kit-built panel lines up with the
app's own panels. See `src/metrics.rs` for the full table and `src/theme.rs` for
the alpha ladder.

## Validation

```sh
cargo test -p termy_ui
cargo run -p termy_ui --example settings_gallery
```

The desktop app consumes this crate from `crates/desktop_app/src/settings_view/`.
After changing tokens or a component used there, also run:

```sh
cargo test -p termy settings
```

## Forbidden Dependencies

- `termy` / `crates/desktop_app`
- `termy_terminal_ui`
- `termy_config_core`, `termy_command_core`, `termy_plugin_runtime`, `termy_ssh_core`
