# termy_core::session_model

## Owner

Portable saved window, workspace, tab, and pane types extracted from the desktop
workspace store and multiplexer manager. SQLite stays in the desktop store;
these types preserve its existing JSON representation for CLI operations.
The desktop's split geometry, tree mutations, inference, and tree serialization
also live here, reused by host operations exposed to the CLI. Removing panes
(`prune.rs`) is shared by the host, desktop and CLI so every client collapses
tabs the same way, and `rebase_session` (`rebase.rs`) merges a window's
local edits onto the host's layout by pane ID when `merge_session` reports a
conflict. No GPUI, filesystem, or terminal runtime dependency.

## Validation

```sh
cargo test -p termy_core
cargo test -p termy --bin termy workspace_store
```

## Boundaries

GPUI and desktop application state. Shared helpers are sibling core modules.
