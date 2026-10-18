# termy_core::theme_core

Shared theme data model.

## Owner

This module owns theme structs, serialization-compatible color types, theme id normalization, and the theme JSON format used by config, bundled themes, local theme files, docs, and embedders.

Keep bundled theme values in `termy_core::themes`. Filesystem paths stay with their callers.

## Validation

```sh
cargo test -p termy_core
```

## Boundaries

GPUI and desktop application state. Shared helpers are sibling core modules.
