# termy_core::cli_install_core

Shared CLI installation helpers.

## Owner

This module owns path resolution and filesystem helpers used to install or locate Termy's command-line tools. It must stay independent of GPUI and desktop app state.

Use this module when install behavior needs to be reused by the desktop app and `termy-cli`.

## Validation

```sh
cargo test -p termy_core
```

## Boundaries

GPUI and desktop application state. Shared helpers are sibling core modules.
