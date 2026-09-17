# termy_core::command_core

Shared command catalog.

## Owner

This module owns Termy's public command identifiers, command metadata, and command/keybinding-facing definitions. It should stay pure and must not depend on GPUI or config parsing.

Use this module when adding, renaming, documenting, or grouping user-facing commands. Wire execution in `crates/desktop_app/`.

## Validation

```sh
cargo test -p termy_core
just check-keybindings-doc
```

## Boundaries

GPUI and desktop application state. Shared helpers are sibling core modules.
