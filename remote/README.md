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
- `DirectConnection`, `DirectListener`, `DirectEndpoint`: the same sealed packets
  over TCP between the two devices, without the relay (see Direct connections).
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

## Direct connections

The relay is the fallback, not the only way. With **Settings → Mobile → Connect
directly** on (the default), Termy Next listens on TCP port 47126 (any free port
when that one is taken) and, from protocol version 6, tells each connected phone
inside the encrypted channel which addresses it has: its Wi-Fi/Ethernet addresses
on private networks and its Tailscale address (100.64.0.0/10 on a `utun`
interface). The phone keeps the list in its Keychain with the Mac's pin and dials
an address only when its own network reaches it: a local one from the same
subnet, the Tailscale one when the phone is on a tailnet. The same network is
tried before the tailnet.

The connection carries the same `SecureChannel` packets, length-prefixed, so the
network is trusted with nothing: the Mac answers only a handshake naming a key
pinned on it, and a phone becomes a session only once its first sealed message
opens, so knowing a phone's public key replaces nobody's session. Whatever
answers at a stale address can't prove the Mac's key and the phone goes back to
the relay. Connections from outside private, Tailscale and loopback ranges are
closed unread. The relay never sees the addresses.

A channel through the relay moves to the direct path when one opens, and keeps
working until the direct one is up; a lost direct path falls back to the relay.
The Mac serves the direct path whether or not the relay is reachable. Reaching
the Mac from another network without Tailscale (NAT traversal) is not attempted.
iOS asks for Local Network access the first time a local address is dialed;
without it the relay is used.

## Without a relay

With **Settings → Mobile → Use a relay server** off, Termy Next registers
nowhere and contacts no server. It makes its own device ID, kept in its
Keychain, and its pairing QR code is version 4: no relay URL, but the Mac's
addresses, its key, and the same one-time 256-bit secret. The phone connects
to one of the addresses and sends a claim (`DirectPairing`): the ID it chose
for itself, its handshake, its sealed name and an HMAC of all of it made with
the QR secret. The Mac shows the approval sheet only for a claim that verifies,
and on approval pins the phone and answers with the handshake response, which
proves to the phone that the Mac holds the key the QR code showed. An invitation
lasts two minutes and pairs one phone.

After that the phone reaches the Mac at the addresses it knows, which the Mac
keeps current inside the encrypted channel. Since no relay can say where the Mac
went, the Mac also advertises its ID as a Bonjour service (`_termy._tcp`) and the
phone looks it up by that name on the local network when the addresses fail.
Tailscale addresses don't change. A phone that can reach neither keeps trying
while the Mac is open on screen and says why.

Everything inside the channel works as through a relay: sessions, panes,
uploads, search, config sync, Shortcuts, the widget and Handoff. What a relay
does and nothing else can:

- **Notifications while Termy is closed.** Apple delivers them only from a
  server holding the app's push key. A connected phone still notifies itself.
- **Reaching the Mac from another network** without Tailscale.
- **Telling the phone it was removed.** Removing a phone on the Mac forgets its
  key at once; the phone only finds it can no longer connect.

A Mac serves either through a relay or without one, and phones pair again when
it switches: their IDs and pins belong to the mode they were made in. One phone
can hold Macs of both kinds.

## Mobile terminal and session controls

Clients announce support after the encrypted host hello; version-1 clients
continue receiving only the messages they understand.

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

Protocol version 5 adds shell integration. The session host already follows
each pane's OSC 133 lifecycle; every `RemotePane` now carries it as
`command` (running, commands finished, last exit code, and when the host first
saw the running command). Version-4 phones ignore the field and version-4
hosts never send it. Attached panes also send `HostMessage.shell` for each
prompt, input, executing and finished step, only to phones that said hello
with version 5. The phone shows running and failed commands in the session
list and pane strip, announces commands that ran five seconds or more when
they finish, and turns predictive typing off while a command runs. Panes
without shell integration report nothing.

Version 5 also carries prompt positions. The engine marks the row the cursor
is on at OSC 133;A (prompt) and 133;C (output), and the marks follow their
rows into history and through reflow. Each frame lists the screen rows where
a prompt starts (`RemoteFrame.promptRows`); the phone draws a bar beside
them. `ClientMessage.jumpToPrompt` scrolls the Mac's view to the previous or
next prompt, and `ClientMessage.commandOutput` asks for what a command
printed, the newest or the one whose prompt starts at a buffer row, read from
the Mac's history. The reply keeps the last `RemoteCommandOutput.maxBytes`.
A session host of another build can't report marks, so the Mac sends no
prompt rows and answers output requests with nothing until the host restarts.

Phones of version 5 can also send files into a pane (`ClientMessage.upload`,
at most `RemoteUpload.maxBytes`): a photo from the library, a pasted
screenshot or a dropped image. The Mac saves it with a safe, dated name in a
`TermyUploads` folder only the user can read under its temporary directory,
deletes uploads after a week, and pastes the shell-quoted path into the pane,
as dropping a file onto a terminal does, so programs such as Claude Code pick
the image up.

`RemoteHandoff` is the Handoff activity both apps share. Termy Next offers
the focused session-host pane, named by its relay device ID and pane ID, and
the iPhone opens that pane; the iPhone offers the pane on screen and Termy
Next brings it to the front. Handoff needs Mobile access on and both apps
signed by the same team, so locally ad-hoc signed builds of Termy Next don't
take part.

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
