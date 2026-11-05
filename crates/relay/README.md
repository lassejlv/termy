# termy_relay

## Owner

Owns Termy accounts, each account's device list, and the WebSocket hub that
carries sealed packets between an account's devices: the Termy iPhone app
(`ios/`) and computers running `termy remote serve` (`next/Sources/TermyRemoteAgent`).

The relay cannot read terminal sessions. Phones and computers each hold an
X25519 key that never leaves them; the relay only stores the public halves so
devices can find each other, and both sides pin a peer's key the first time
they see it (`remote/Sources/TermyRemoteKit/TrustStore.swift`). Screens, keys
and session lists travel inside `SecureChannel` (Noise-KK-style handshake,
ChaCha20-Poly1305 with counter nonces). The relay sees who talks to whom, when
and how much.

## Data

Postgres on Neon, through sqlx. `migrations/` runs at start-up.

- `users`: email and an Argon2id password hash.
- `devices`: name, kind (`host` or `client`), public key, last seen.
- `sessions`: SHA-256 of each device's bearer token; one per device.

Signing out deletes the device. Deleting the account deletes everything.

## API

| Method | Path | |
| --- | --- | --- |
| POST | `/v1/auth/signup` | `{email, password, device: {name, kind, publicKey}}` → `{token, user, device}` |
| POST | `/v1/auth/login` | same; 10 attempts per address and email per 5 minutes |
| POST | `/v1/auth/logout` | removes this device |
| GET / DELETE | `/v1/me` | the account; DELETE removes it |
| GET | `/v1/devices` | the account's devices with `online` |
| DELETE | `/v1/devices/{id}` | removes a device and closes its socket |
| GET | `/v1/relay` | WebSocket, `Authorization: Bearer` |

On the socket, a binary frame is `0x01`, the 16-byte peer device ID, then the
sealed packet. The relay replaces the recipient's ID with the sender's and
forwards it to that device if it belongs to the same account. Text frames are
events: `ready`, `presence` (`device`, `online`) and `undeliverable`.

## Run

```sh
echo 'DATABASE_URL=postgresql://…' > crates/relay/.env   # gitignored
just relay                                                  # PORT 8787
docker build -f crates/relay/Dockerfile .                   # deployable image
```

`HOST`, `PORT` and `RUST_LOG` are read from the environment. Run one
instance: the hub keeps sockets in memory.

## Validation

```sh
cargo test -p termy_relay
cargo clippy -p termy_relay --all-targets -- -D warnings
```

The hub tests cover routing within an account, refusing other accounts, and
a replaced socket not unregistering its successor. The account API needs a
database; exercise it against a Neon branch with `just relay` and curl.

## Forbidden Dependencies

- No `termy_core`, `termy_pty` or GPUI: the relay never runs or parses a
  terminal, it only routes sealed bytes.
- No dependency on a crate that could decrypt sessions; the relay holds no
  session keys by design.
