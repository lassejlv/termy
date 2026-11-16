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

Device names are sealed on the owning device with ChaCha20-Poly1305 and a
separate HKDF-SHA256 key derived from its Keychain-held identity. Names are
padded to 256 bytes and use random nonces. The database stores only versioned
ciphertext; there is no server decryption key. During version-3 QR pairing,
the phone name is separately encrypted using a domain-separated key from the
QR secret, bound to the invitation and handshake. Its ciphertext is also
included in the pairing HMAC. The Mac name travels in the QR directly, never
through the relay. Readable peer names are cached in each app's Keychain and
are never replaced with a relay-supplied name.

The relay still sees random device IDs, public keys, device kinds, pair
relationships, connection times, and traffic sizes. Bearer tokens are stored
as one-way SHA-256 hashes. These routing/authentication fields are deliberately
not described as end-to-end encrypted. It cannot read names, terminal content,
or input sent by updated apps. A compromised relay can disrupt access but cannot substitute an identity in a valid QR
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
and rate limiters are in memory. Set `TRUSTED_PROXIES` to the address range
Unkey's proxy connects from (see Limits); until then the relay logs a warning
and all users share one registration limit. Deploying this branch replaces the
v1 account endpoints with v2 pairing endpoints. Existing apps need updating
and re-pairing for the original v1 transition. Migration `0003` removes the
plaintext device-name column and the retired v1 account tables (including
emails/password hashes) while preserving all v2 device credentials and links.
Updated apps republish their own encrypted name without re-registering.
Existing iPhone-cached Mac names survive refresh; the Mac shows a generic
phone label until paired again with both apps updated. New QR codes use
version 3 and require updated apps on both sides. Old apps continue existing
connections with generic relay labels; submitted legacy plaintext names are
ignored and never stored. Rolling back to a pre-0003 server requires a schema
restore and would reintroduce the old privacy exposure.

Dropping columns/tables removes them from the live schema, not from historical
backups, WAL, provider retention, or old database branches. Manage those copies
through the database provider's retention policy; do not treat this migration
as proof that historical plaintext has been physically erased.

Configuration: `DATABASE_URL` (required), `HOST` (default `0.0.0.0`), `PORT`
(default `8080`; `just relay` uses `8787`), `TRUSTED_PROXIES` (default none),
`RUST_LOG`, and the optional `APNS_*` variables (see Push notifications).

## Push notifications

A Mac can wake a paired iPhone through Apple Push Notification service. The
relay never sees what a notification says: the Mac sends a blob sealed for the
phone, and the relay wraps it in a generic alert (`Termy` / `A command finished
on your computer.`, `mutable-content: 1`) with the blob in `termy.s` and the
Mac's device ID in `termy.h`. The phone's Notification Service Extension opens
the blob and replaces the title and body. The relay stores only each phone's
APNs device token and environment, and logs neither the token nor the blob,
only a short hash of the token. Unpairing a phone from its last Mac, or
deleting its registration, deletes its token; so does APNs answering 410, or
400 with `BadDeviceToken` or `DeviceTokenNotForTopic`.

Push is off unless all of these are set; setting only some of them stops the
relay at startup. The startup log says whether push is on. The key is never
logged.

| Variable | Meaning |
| --- | --- |
| `APNS_KEY` | Contents of the APNs auth key (`AuthKey_XXXXXXXXXX.p8`, PEM). `\n` written out is accepted. |
| `APNS_KEY_FILE` | Path to the `.p8` instead of `APNS_KEY` |
| `APNS_KEY_ID` | The key's 10-character Key ID |
| `APNS_TEAM_ID` | The Apple Developer Team ID |
| `APNS_TOPIC` | Bundle ID notifications go to (default `sh.termy.app`) |
| `APNS_URL` | Overrides the APNs base URL, for tests. Otherwise each token's environment picks `https://api.push.apple.com` or `https://api.sandbox.push.apple.com`. |

The relay signs an ES256 provider token with the key and reuses it for 50
minutes. Requests to APNs use HTTP/2 over rustls and time out after 10 seconds.

## Limits

Limits count per client address; an IPv6 client is its /64. The address is
the socket's source unless that source is listed in `TRUSTED_PROXIES`
(comma-separated addresses or CIDR blocks). For a listed proxy the relay
takes the rightmost `X-Forwarded-For` entry that isn't itself a listed proxy,
so a client can't choose its own address. List only proxies that append to
that header, and only when clients can't reach the relay around them.

- Registration, the one unauthenticated write: 30 per address per five minutes.
- Every other request: 600 per address per minute, refused with 429 before
  it is authenticated. A source on a private or loopback address that isn't
  a listed proxy is not counted, because it would stand for all its users.
- Requests: 16 KiB body, 15 seconds to send the headers and 15 to finish.
- Push notifications: a Mac may send 20 at once, then one every 3 seconds;
  past that it gets 429. A sealed notification is 1 to 1536 bytes.
- Sockets: 4 MiB per frame. A device may send a 32 MiB burst, then 2 MiB/s;
  beyond that the relay reads its socket more slowly and drops nothing.
- A device may have 32 frames or 16 MiB waiting, and all devices together
  256 MiB. A device that falls further behind, takes over a minute to accept
  one frame, or sends nothing (not even a pong) for 75 seconds is
  disconnected and resynchronises when it reconnects.
- Hourly, the relay deletes registrations that have not connected for 90
  days, are offline, and have no link or open invitation. Such a device gets
  401, which the apps report as an ended registration. Paired devices are kept.

Bearer tokens do not expire. A device replaces its token by deleting its
registration and pairing again.

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
intend to erase all pairings. Device registration is public; see Limits.
`compose.yaml` sets `TRUSTED_PROXIES` to the private ranges, which is safe
there because only Caddy can reach the relay's port. Add stricter edge quotas
if exposing a relay to a large audience.

## API

Every route except registration and health requires the device's bearer token.
Encrypted names are base64 envelopes: version `0x01`, 12-byte nonce, 256 bytes
of padded ciphertext, and 16-byte authentication tag (285 bytes total).
Device DTO `name` fields contain only generic `Mac`/`iPhone` compatibility labels.
Legacy registrations/claims may omit ciphertext but never persist plaintext.

Tokens are random 256-bit values, stored only as SHA-256 hashes on the server.
Device private keys and credentials live in the clients' Keychain.

| Method | Path | Purpose |
| --- | --- | --- |
| GET | `/health` | Liveness |
| POST | `/v2/devices` | Register `{encryptedName,kind,publicKey}` → `{token,device}` |
| GET | `/v2/devices` | Only this device's paired peers, with presence |
| PUT | `/v2/device` | Replace this device’s encrypted name `{encryptedName}` |
| DELETE | `/v2/device` | Delete this registration and all its links |
| DELETE | `/v2/links/{peer}` | Revoke one phone–Mac link |
| POST | `/v2/pairings` | Host creates an expiring invitation |
| GET | `/v2/pairings/{id}` | Host or claiming phone polls status |
| DELETE | `/v2/pairings/{id}` | Host cancels an unapproved invitation |
| POST | `/v2/pairings/{id}/claim` | Phone submits `{initPacket,proof,encryptedClientName}` (base64) |
| POST | `/v2/pairings/{id}/approve` | Host submits `{clientId,initPacket,responsePacket}` |
| PUT | `/v2/device/push` | Phone stores its APNs token `{token,environment}` (hex; `sandbox` or `production`) |
| DELETE | `/v2/device/push` | Phone removes its APNs token |
| POST | `/v2/push/{phone}` | Paired Mac sends `{sealed,collapseId?}` → `{sent}`; 503 if push is off |
| GET | `/v2/relay` | Authenticated WebSocket |

Socket framing is unchanged: binary `0x01`, recipient UUID (16 bytes), then
sealed packet. The relay replaces the UUID with the sender's. Text events are
`ready`, `presence`, and `undeliverable`. Frames are capped at 4 MiB. Slow peers
are disconnected rather than accumulating an unbounded output queue (see Limits).

## Validation

The Rust tests cover pairing-only routing, non-transitive permissions, socket
replacement, and a database-backed API lifecycle (expiry, cancellation,
approval, retries, revocation, isolation and device deletion). `push_api`
covers token registration, who may push, the request a mock APNs receives,
forgetting refused tokens, and the push rate limit.

```sh
# Use an isolated branch/database and a direct (non-pooler) connection.
RELAY_SWIFT_E2E=1 TEST_DATABASE_URL=postgresql://… cargo test -p termy_relay --features integration-tests pairing_api_lifecycle
TEST_DATABASE_URL=postgresql://… cargo test -p termy_relay --features integration-tests push_api
just test-remote
# With a local relay backed by an isolated database and the staged Swift SDK:
RELAY_TEST_URL=http://127.0.0.1:8787 swift test --package-path next --filter RemoteHostTests
```

`RELAY_SWIFT_E2E=1` additionally starts a local relay on a random loopback port
inside the isolated test schema and invokes the Swift host integration test.
The migration test seeds the former schema with private names and account
records, applies `0003`, checks removal, and verifies current links still work.

The opt-in host test uses disposable devices and a private session host. It
pairs through the real API, establishes an encrypted socket channel, creates a
shell, verifies streamed program output, resizes/restores it, and revokes access.
The relay has no dependency on libtermy or any session decryption keys.

## Forbidden Dependencies

- No `termy_core`, `termy_pty` or GPUI: this service never runs or parses a terminal.
- No dependency that could decrypt terminal sessions; session keys remain on devices.
