# termy_core::search_engine

Reusable terminal search primitives.

## Owner

This module owns text matching and search state that can be shared by the desktop app, headless runtime, and tests. Keep GPUI rendering, selection visuals, and command-palette behavior outside this module.

Use this module when changing search matching semantics or reusable search state.

## Validation

```sh
cargo test -p termy_core
```

## Boundaries

GPUI and desktop application state. Shared helpers are sibling core modules.
