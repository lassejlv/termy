# termy_core::plugin_runtime

On-demand Bun runtime for trusted local plugins.

## Owner

This module owns plugin discovery, manifest capability validation and gating, command, lifecycle-event, and native-view dispatch, descriptor/action/document validation, the read-only invocation context contract, managed TypeScript and JSX declarations, the on-demand Bun host, per-plugin Worker lifecycle, protocol limits, timeouts, and bundle caching.

It returns typed commands, actions, and allowlisted UI document nodes to callers. Command-palette presentation, GPUI rendering/state, built-in command execution, terminal tabs, toasts, and settings remain owned by `crates/desktop_app/`.

## Validation

```sh
cargo test -p termy_core
```

## Boundaries

GPUI and desktop application state. Shared helpers are sibling core modules.
