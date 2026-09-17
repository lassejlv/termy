# termy_cli

Command-line companion for Termy.

## Owner

This crate owns the `termy-cli` binary, including user-facing terminal commands, config inspection helpers, theme/config utilities, and install/update commands that belong outside the desktop app.

Keep reusable install logic in `termy_core::cli_install_core`, release metadata logic in `termy_core::release_core`, and desktop UI actions in `crates/desktop_app/`.

The `mux` commands control persistent sessions and layouts with JSON responses.
Repository tooling is the `xtask` binary in `src/xtask/`.

## Validation

```sh
cargo test -p termy_cli
```

## Forbidden Dependencies

- `gpui`
- `termy` / `crates/desktop_app`
