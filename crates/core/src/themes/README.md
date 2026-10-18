# termy_core::themes

Bundled Termy themes.

## Owner

This module owns the built-in themes every host ships offline, listed by `builtin_themes()` and resolved by `builtin_theme()` / `find_builtin_theme()`: Termy's own `termy` and `termy-light`, then the collection in `collection/themes.tsv`. The collection comes from [iTerm2-Color-Schemes](https://github.com/mbadolato/iTerm2-Color-Schemes) (MIT, see `collection/LICENSE`) and is regenerated with `scripts/import-themes.py`; do not edit the TSV by hand.

The other palette modules here are bundled only for embedders that list them themselves (the web package). This module depends on `termy_core::theme_core` for the data model and stays independent of GPUI and app config I/O.

## Validation

```sh
cargo test -p termy_core
```

## Boundaries

GPUI and desktop application state. Shared helpers are sibling core modules.
