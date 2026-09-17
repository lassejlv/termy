# termy_core::config_core

Shared configuration schema and defaults.

## Owner

This module owns Termy's config data model, defaults, validation-friendly types, and theme references used by app, CLI, docs, and embedding surfaces.

Keep terminal runtime behavior in `termy_core`, command metadata in `termy_core::command_core`, and bundled theme definitions in `termy_core::themes`.

## Validation

```sh
cargo test -p termy_core
just check-config-doc
```

## Boundaries

GPUI and desktop application state. Shared helpers are sibling core modules.
