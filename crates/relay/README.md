# termy_relay

## Owner

Owns device registration, pairing authorization and the sealed-packet relay.

Routes end-to-end encrypted terminal traffic between explicitly paired devices.
The Mac host is **Termy Next → Settings → Mobile**; the client is `ios/`.
No account, email address, password, or terminal command is needed.

## Pairing

1. Enable mobile access in Termy Next, then choose **Pair iPhone…**.
2. Scan the QR code in the iPhone app and confirm the displayed Mac and relay.
3. Approve the phone in Termy Next.

An invitation expires after two minutes, accepts one phone, and grants no
terminal access until the Mac approves it. The QR contains a random 256-bit
secret, invitation ID, relay URL, and Mac public key. The secret never goes
to the relay. The phone authenticates its complete handshake with an HMAC of
that secret; the Mac verifies it before showing approval. The phone verifies
the Mac's encrypted handshake response against the key from the QR. Both
sides pin the resulting peer key. Subsequent connections use `SecureChannel`
in `remote/`, with fresh ephemeral keys and replay-resistant encrypted frames.

The relay sees device names, public keys, pair relationships, connection times,
and traffic sizes. It cannot read terminal content or input. A compromised
relay can disrupt access but cannot substitute an identity in a valid QR
pairing. Someone who sees an unexpired QR can request access; the Mac still
requires explicit approval. Pair only on a trusted Mac screen.

Removing access deletes the link and blocks routing immediately. Pairings
are not transitive: two phones paired with one Mac cannot contact one another.
Switching relay servers requires pairing on the new server. One phone can
remember multiple relays. Termy Next serves while the app is running; closing
its last window keeps access available, quitting or disabling Mobile stops it.
The session host must be enabled.

## Run locally or on Unkey

Postgres is required. Migrations run automatically on startup.

```sh
# Put DATABASE_URL=postgresql://… in crates/relay/.env (gitignored).
just relay                       # binds port 8787 by default
cargo test -p termy_relay
cargo clippy -p termy_relay --all-targets -- -D warnings
```

The default public relay is **https://relay.termy.sh**, deployed on Unkey from
`crates/relay/Dockerfile`. Keep production at **one replica**: the socket hub
and registration limiter are in memory. Deploying this branch replaces the
v1 account endpoints with v2 pairing endpoints. Existing apps need updating
and re-pairing; migration `0002` preserves the old account tables for rollback.
Do not edit migration `0001` or delete production account data during rollout.

Configuration: `DATABASE_URL` (required), `HOST` (default `0.0.0.0`), `PORT`
(default `8080`; `just relay` uses `8787`), and `RUST_LOG`.

## Self-host on a VPS

Use a server with Docker Engine and the Compose plugin, a domain pointing to
its public IP, and inbound ports **80 and 443** open. The relay only needs
outbound connections from each Mac/phone; no incoming Mac port is needed.

From a checkout on the VPS:

```sh
cd crates/relay/deploy
cp .env.example .env
# Edit .env: set RELAY_DOMAIN and a random, URL-safe POSTGRES_PASSWORD.
# Generate a hexadecimal password with: openssl rand -hex 32
chmod 600 .env
docker compose up -d --build
curl --fail https://relay.example.com/health
```

The supplied stack runs Postgres, one relay, and Caddy. Only Caddy exposes
public ports; Postgres and the relay remain on the container network. Caddy
terminates HTTPS and forwards WebSockets. Persisted volumes hold the database
and certificate state. See [Caddy reverse proxy](https://caddyserver.com/docs/caddyfile/directives/reverse_proxy)
and [Compose startup ordering](https://docs.docker.com/compose/how-tos/startup-order/).

In **Termy Next → Settings → Mobile → Relay Server**, enter
`https://relay.example.com`, click **Apply**, and pair the phone. The QR
configures the same server on iPhone automatically. Public relays must use
HTTPS with a trusted certificate; HTTP is accepted only on loopback for
local development. There is no dependency on Unkey or Neon when self-hosting.

Back up the Postgres volume/database. Restart with `docker compose restart`;
update with `docker compose up -d --build`. Do not use `down -v` unless you
intend to erase all pairings. Device registration is public and limited to
30 attempts per socket-source IP per five minutes. Behind a proxy this limit
is shared by that proxy's users; forwarded IP headers are deliberately not
trusted. Add stricter edge quotas if exposing a relay to a large audience.

## API

Every route except registration and health requires the device's bearer token.
Tokens are random 256-bit values, stored only as SHA-256 hashes on the server.
Device private keys and credentials live in the clients' Keychain.

| Method | Path | Purpose |
| --- | --- | --- |
| GET | `/health` | Liveness |
| POST | `/v2/devices` | Register `{name,kind,publicKey}` → `{token,device}` |
| GET | `/v2/devices` | Only this device's paired peers, with presence |
| DELETE | `/v2/device` | Delete this registration and all its links |
| DELETE | `/v2/links/{peer}` | Revoke one phone–Mac link |
| POST | `/v2/pairings` | Host creates an expiring invitation |
| GET | `/v2/pairings/{id}` | Host or claiming phone polls status |
| DELETE | `/v2/pairings/{id}` | Host cancels an unapproved invitation |
| POST | `/v2/pairings/{id}/claim` | Phone submits `{initPacket,proof}` (base64) |
| POST | `/v2/pairings/{id}/approve` | Host submits `{clientId,initPacket,responsePacket}` |
| GET | `/v2/relay` | Authenticated WebSocket |

Socket framing is unchanged: binary `0x01`, recipient UUID (16 bytes), then
sealed packet. The relay replaces the UUID with the sender's. Text events are
`ready`, `presence`, and `undeliverable`. Frames are capped at 4 MiB. Slow peers
are disconnected rather than accumulating an unbounded output queue.

## Validation

The Rust tests cover pairing-only routing, non-transitive permissions, socket
replacement, and a database-backed API lifecycle (expiry, cancellation,
approval, retries, revocation, isolation and device deletion).

```sh
# Use an isolated branch/database and a direct (non-pooler) connection.
TEST_DATABASE_URL=postgresql://… cargo test -p termy_relay --features integration-tests pairing_api_lifecycle
just test-remote
# With a local relay backed by an isolated database and the staged Swift SDK:
RELAY_TEST_URL=http://127.0.0.1:8787 swift test --package-path next --filter RemoteHostTests
```

The opt-in host test uses disposable devices and a private session host. It
pairs through the real API, establishes an encrypted socket channel, creates a
shell, verifies streamed program output, resizes/restores it, and revokes access.
The relay has no dependency on libtermy or any session decryption keys.

## Forbidden Dependencies

- No `termy_core`, `termy_pty` or GPUI: this service never runs or parses a terminal.
- No dependency that could decrypt terminal sessions; session keys remain on devices.
