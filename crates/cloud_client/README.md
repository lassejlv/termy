# termy_cloud_client

Blocking client library for the Termy cloud API (`crates/api`). Owns the
device-authorization login flow, the shared on-disk session file
(`cloud_auth.json` next to the Termy config file, mode 0600 — the same file
the desktop app writes), and typed calls for the `/api` surface: account,
Railway provider connection status, cloud projects, and sandbox sessions.

Consumed by `termy_cli` (`termy cloud ...`). The desktop app still carries its
own copy of the device flow in `crates/desktop_app/src/cloud_auth.rs`;
migrating it onto this crate is a known follow-up.

## Owner

Owns the client-side wire contract with `termy_api`: request/response types,
session-file format, and device-flow behavior. Server-side changes to `/api`
or `/auth/device/*` must update this crate in the same PR.

## Validation

```sh
cargo test -p termy_cloud_client
```

```sh
cargo check -p termy_cloud_client
```

## Forbidden Dependencies

- `gpui` — headless library; no UI framework.
- `termy` (desktop app), `termy_terminal_ui` — no app/presentation code.
- `termy_api` — clients never link the server; the contract is HTTP.
