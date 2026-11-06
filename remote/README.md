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

## Mobile terminal and session controls

Encrypted protocol version 2 syncs the active Mac render settings and a copy
of `config.txt` on connect and after config changes. The phone saves this
per-computer copy in Keychain, applies font metrics, colors, padding, cursor,
scroll speed, appearance, reduced motion and copy-on-select, and offers the
original config in the terminal menu. Mac-only settings continue to run on
the Mac. Font files are not transferred: the iPhone uses an available font
for ASCII; visible Unicode, Nerd Font symbols, emoji and libtermy's special
glyph geometry are rasterized with the Mac font into cached transparent
cell images. These images are encrypted like the rest of the frame.

The mobile + menu can create a named session or a tab in a chosen existing
session. Session headers offer new-tab/rename actions; tabs expand to show
their panes. The terminal menu switches panes, creates a tab or split in the
current session, searches scrollback, toggles mouse mode, restores the Mac
size, and ends a pane with confirmation. Session/tab/split edits use the
multiplexer's revision-checked layout so Next adopts them live. Failed layout
writes close their just-created shell instead of leaving an orphan pane.

Terminals fit to the phone by default, refit for keyboard/orientation changes,
and restore their original size on detach. Pinch changes the mobile text
size. Drag scrolls history, or sends wheel events when a TUI has mouse
tracking enabled. Mouse mode reports taps/drags to the program. Hold opens
native text selection, with wrapped rows joined for copying. The keyboard
accessory includes sticky Control/Option, Escape, Tab, Shift-Tab, arrows,
Home/End, Page Up/Down, F1–F12 and shell symbols. Hardware keys use the same
host-side libtermy encoder. Scrollback search runs on the Mac, and the Live
button returns to the current prompt.

Validation includes real encrypted creation/rename/split/search/scroll and
config updates through the relay, host glyph/color tests, and iOS simulator
row-retention, render-image and keyboard/landscape grid tests:

```sh
swift test --package-path remote
swift test --package-path next
# After xcodegen generate --spec ios/project.yml:
xcodebuild -project ios/Termy.xcodeproj -scheme Termy \
  -destination 'platform=iOS Simulator,name=Termy App Store Screenshots' test
```

Version-1 hosts remain viewable, but require updating for the new configuration
and session controls. The protocol still transports the engine's cell screen;
it does not stream the Mac's window chrome or arbitrary application windows.
