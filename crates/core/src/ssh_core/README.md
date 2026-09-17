# termy_core::ssh_core

Saved SSH host and credential domain logic.

## Owner

This module owns validated saved-host data, atomic non-secret persistence, exact OpenSSH program arguments, keychain account construction, and credential lifecycle orchestration. It is headless and does not own GPUI presentation, terminal tabs, or process spawning.

Passwords and private-key passphrases are never serialized by this module. Private keys remain user-owned files; only their paths are stored.

## Validation

```sh
cargo test -p termy_core
```

## Boundaries

GPUI and desktop application state. Shared helpers are sibling core modules.
