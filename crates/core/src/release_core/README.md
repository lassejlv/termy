# termy_core::release_core

Release metadata and version helpers.

## Owner

This module owns shared release/update metadata parsing and version comparison logic used by the CLI and updater. It should not own installer execution, UI, or platform packaging scripts.

Use this module when changing how Termy understands releases, versions, downloadable artifacts, or GitHub release notes.

## Validation

```sh
cargo test -p termy_core
```

## Boundaries

GPUI and desktop application state. Shared helpers are sibling core modules.
