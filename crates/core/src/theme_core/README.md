# termy_core::theme_core

Shared theme data model.

## Owner

This module owns theme structs, serialization-compatible color types, registry data contracts, and the versioned `theme_registry.cache` representation used by config, bundled themes, docs, and embedders.

Keep bundled theme values in `termy_core::themes`. Filesystem paths, network fetching, and app-specific cache policy stay with their callers.

## Validation

```sh
cargo test -p termy_core
```

## Boundaries

GPUI and desktop application state. Shared helpers are sibling core modules.
