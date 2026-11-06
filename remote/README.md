# TermyRemoteKit

Reusable device pairing, trust, messages and encrypted relay transport shared
by the iPhone app (`ios/`) and the Mac host library
(`next/Sources/TermyRemoteHost`). Termy Next owns the host's settings and lifetime;
no command-line process or account is required.

- `PairingCode`: versioned QR payload, HTTPS relay validation, and HMAC-bound
  invitation/handshake verification (QR version 3). The QR secret never goes to the server.
- `PrivateMetadata`: padded ChaCha20-Poly1305 device names, with an HKDF key
  derived only from the local private identity. Names exchanged during pairing
  use a separate QR-secret-derived key authenticated to the handshake.
- `DeviceStore`: Keychain-backed identity and relay credentials.
- `SecureChannel`: X25519 ephemeral/static handshake, HKDF-SHA256,
  ChaCha20-Poly1305, directional counters and replay rejection. It is the
  existing Termy protocol, not an implementation of a standard Noise protocol.
- `RelayAPI`: accountless device registration, expiring invitations, explicit
  approval, paired-device discovery and revocation.
- `RelayConnection`: authenticated WebSocket transport and presence events.
- `Messages`: logical keys, paste, resize, scroll, session operations and
  changed-row screen frames. The terminal engine stays on the Mac.
- `TrustStore`: existing file-backed pinning utility, retained for embedders.
  The apps persist pairing pins through `DeviceStore` instead.

A phone may have separate credentials for multiple relay servers. Both sides
pin peer keys after QR pairing and refuse substituted keys on later connects.
Readable device names come from the QR or authenticated encrypted pairing
claim and remain in each app's Keychain. Relay discovery returns generic
labels and must not replace these locally verified names.
See [the relay README](../crates/relay/README.md) for the protocol, rollout,
and the Docker Compose VPS deployment.

```sh
just test-remote
```

The package does not import Next's UI, AppKit, or libtermy; a future client can
reuse it. `TermyRemoteHost` adds the Mac-specific terminal/session bridge as a
separate library product in `next/Package.swift`.
