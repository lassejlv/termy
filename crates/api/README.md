# termy_api

Cloud API backend for Termy cloud agents (`termy-api` binary). Serves account
auth via [better-auth](https://github.com/better-auth-rs/better-auth-rs)
(email/password, sessions, and device authorization) mounted under `/auth`,
with application routes under `/api`. The TanStack web app in `web/` is built
with Bun/Vite and served as a static SPA by Axum. Backed by Postgres via sqlx;
migrations live in `migrations/` and run automatically on startup.

Application queries use a separate query pool (`src/db.rs`) built on
[neon-serverless-sqlx](https://github.com/lassejlv/neon-serverless-sqlx)
(sqlx 0.9): URLs on `*.neon.tech` are tunneled over Neon's WebSocket proxy,
anything else gets a plain direct pool (no TLS — local/dev only). Auth traffic
and migrations connect directly via sqlx 0.8, independent of the query pool.

## Owner

Owns the hosted Termy cloud API surface: HTTP routing, auth configuration,
and the auth database schema. Must not contain terminal runtime or desktop
UI behavior — embed `termy_core` only if a future endpoint genuinely needs
terminal semantics.

## Environment

- `DATABASE_URL` (required): Postgres connection string.
- `TERMY_API_SECRET` (required): auth signing secret, 32+ characters.
- `TERMY_API_ENCRYPTION_KEY` (required): base64 of 32 random bytes; encrypts
  provider tokens at rest (generate with `openssl rand -base64 32`). Separate
  from `TERMY_API_SECRET` so rotating one does not invalidate the other.
- `TERMY_API_BASE_URL` (optional, default `https://app.termy.sh`).
- `PORT` (optional, default `8080`).
- `TERMY_WEB_DIR` (optional, default `crates/api/web/dist`).
- `TERMY_GITHUB_CLIENT_ID` and `TERMY_GITHUB_CLIENT_SECRET` (optional): set
  both to enable **Continue with GitHub**. Leave both unset/empty to hide it.
- `TERMY_RAILWAY_CLIENT_ID` and `TERMY_RAILWAY_CLIENT_SECRET` (optional): set
  both to enable connecting a Railway account for cloud projects (create the
  OAuth app under Railway workspace **Developer settings**; callback URL is
  `{TERMY_API_BASE_URL}/api/providers/railway/callback`).

Create separate GitHub OAuth Apps for development and production. Their
authorization callback URLs must match the API base URL:

- Local: `http://127.0.0.1:8080/oauth-complete`
- Production: `https://app.termy.sh/oauth-complete`

Run locally:

```sh
(cd crates/api/web && bun install && bun run build)
DATABASE_URL=postgres://localhost/termy \
TERMY_API_SECRET=<32+ chars> \
TERMY_API_BASE_URL=http://127.0.0.1:8080 \
cargo run -p termy_api
```

From the workspace root, `just api` loads `.env.local`, installs/builds the web
app, and runs the same local configuration. Open `http://127.0.0.1:8080` to use
the web app.

## Web app

The SPA uses TanStack Router file-based routes, TanStack Query, Tailwind CSS v4
through its Vite plugin, and the official Better Auth React client. The client
is pinned to `better-auth@1.4.19`, the wire contract targeted by
`better-auth-rs` 0.10. shadcn is configured in `web/components.json`; the full
coss ui primitive set is source-owned under `web/src/components/ui`. The
generated route tree lives at `web/src/routeTree.gen.ts`.

```sh
cd crates/api/web
bun install
bun run check
```

The desktop login uses Better Auth's device flow:

1. Termy requests a device code from `/auth/device/code`.
2. The desktop opens `/device?user_code=...` in the default browser.
3. The user signs in and approves the request in the web app.
4. Termy polls `/auth/device/token` and stores the returned session.

## Cloud projects (TRM-34)

Git-backed projects that run in Railway sandboxes (Priority Boarding beta;
the connected Railway account must be enrolled). Provider tokens are stored
encrypted (`TERMY_API_ENCRYPTION_KEY`); Git is the source of truth — no
working trees live in Termy. Provider behavior sits behind the
`SandboxProvider` trait in `src/providers/`; Railway is the only adapter and
talks plain GraphQL to `backboard.railway.com/graphql/v2` (operation shapes
pinned against the open-source `railway-ts-sdk` / `railwayapp/cli` sources —
beta API, see fixture tests in `src/providers/railway.rs`).

Routes (all session-authenticated, under `/api`):

- `GET|DELETE /api/providers/railway`, `GET .../connect`, `GET .../callback` —
  OAuth (PKCE) connect flow, status, disconnect.
- `GET|POST /api/projects`, `GET|PATCH|DELETE /api/projects/{id}` — CRUD;
  create provisions a dedicated Railway project/environment.
- `POST /api/projects/{id}/sessions` — starts a sandbox session (202 +
  background pipeline `provisioning → cloning → setting_up → ready`).
- `GET /api/sessions/{id}`, `GET .../connection`, `DELETE` — poll status,
  fetch connection info (`ready` only), stop/destroy.
- `GET /api/sessions/{id}/terminal` — websocket **terminal relay**. The server
  authenticates the session, mints the sandbox shell token, dials the
  provider's exec bridge, and pipes frames both ways. The browser terminal
  (xterm.js, cookie auth) and the CLI (bearer auth) connect here; the provider
  endpoint and token never reach the client. No SSH keys — users can work from
  the dashboard on any device.

**Fast restarts:** stopping a `ready` session captures a Railway checkpoint of
the disk (`projects.checkpoint_key`, migration `0003`) before destroying the
sandbox. The next start boots from that checkpoint and only refreshes Git
(`git fetch`/`reset --hard`), skipping clone and setup. A missing or stale
checkpoint falls back to a fresh clone.

Clients use `crates/cloud_client` (`termy-cli cloud ...`) or the dashboard.
Manual end-to-end check (needs `TERMY_RAILWAY_CLIENT_ID/SECRET` and a
sandbox-enrolled account):

```sh
termy-cli cloud login
termy-cli cloud connect railway     # browser consent
termy-cli cloud projects create --name demo --repo https://github.com/<owner>/<repo>
termy-cli cloud run --project demo  # waits for ready, then opens a terminal
termy-cli cloud stop --project demo # checkpoints, then destroys
```

## Validation

```sh
cargo check -p termy_api
```

```sh
cargo test -p termy_api
```

```sh
cd crates/api/web && bun run check
```

## Forbidden Dependencies

- `gpui` — this is a headless server crate; no UI framework.
- `termy_terminal_ui`, `termy` (desktop app) — no desktop presentation code.
