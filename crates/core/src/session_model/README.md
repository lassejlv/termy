# termy_core::session_model

## Owner

Portable saved window, workspace, tab, and pane types extracted from the desktop
workspace store and multiplexer manager. SQLite stays in the desktop store;
these types preserve its existing JSON representation for CLI operations.
The desktop's split geometry, tree mutations, inference, and tree serialization
also live here, reused by host operations exposed to the CLI. No GPUI,
filesystem, or terminal runtime dependency.

## Validation

```sh
cargo test -p termy_core
cargo test -p termy --bin termy workspace_store
```

## Boundaries

GPUI and desktop application state. Shared helpers are sibling core modules.
