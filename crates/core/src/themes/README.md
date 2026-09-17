# termy_core::themes

Bundled Termy themes.

## Owner

This module owns built-in theme definitions and their registration. It should depend on `termy_core::theme_core` for the data model and stay independent of GPUI and app config I/O.

Use this module when adding or changing bundled color themes.

## Validation

```sh
cargo test -p termy_core
```

## Boundaries

GPUI and desktop application state. Shared helpers are sibling core modules.
