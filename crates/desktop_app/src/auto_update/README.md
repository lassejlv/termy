# termy::auto_update

Update-checking and installer handoff logic for Termy.

## Owner

This module owns release discovery, artifact verification, platform update decisions, and OS handoff points. It may depend on `termy_core::release_core` for release metadata, but it should not own desktop rendering or update UI.

Use this module when changing how Termy finds, validates, or launches updates.

## Validation

```sh
cargo test -p termy
```

## Boundaries

Application state must stay in the desktop app shell; shared headless logic belongs in core.
