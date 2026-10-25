# termy_core::multiplexer

## Owner

Owns the background session host, private local connection, PTY lifetime,
remote terminal client, and opaque desktop session layout. The host uses
`termy_core`; it has no display-server or tmux dependency. The desktop app
provides its own executable as the host entry point for installed builds.

The host keeps the authoritative parser, both screen buffers, scrollback,
images, and live processes. Clients cache the viewport and request history,
search, and links when needed. Dropping a client detaches it; closing a pane
is an explicit host operation. Clipboard operations use the attached UI's
existing permission policy; terminal protocol replies continue when detached.
Client and host prove knowledge of the `endpoint.json` token to each other
(HMAC challenge-response) before any request, so a process that binds the
port after a host crash neither learns the token nor receives input.

Protocol version 4 requires that challenge-response; the plaintext-token
greeting and the capability fallbacks for older hosts are gone. Clients reject
a host of another version before connecting, and `connect_or_start` never
replaces one while it holds `host.lock`. `shutdown_host` (`termy mux shutdown`)
still stops it: `escape.rs` encodes the handshake and shutdown frames by hand,
and every version must keep their variant indices and shapes (see the wire rules
on `protocol::Request`). `kill_host` (`termy mux shutdown --force`) signals the
PID in `endpoint.json` for a host that no longer answers. Both check
`host.lock` first: without a live host, `endpoint.json` is stale, so they
remove it and report that no host is running. The desktop falls back to plain
local terminals (not tmux) with a toast when the host cannot be used or does
not answer within 8 s.

The `wire_golden` test hashes a sample of every wire type and variant. When the
hash changes, bump `VERSION` (unless the unreleased change already did) and
update `WIRE_FINGERPRINT`. `endpoint.json` records the host's build ID; a client
of the same `VERSION` but another build logs a warning. New `endpoint.json`
fields must be `#[serde(default)]`, since `shutdown_host` reads older files.

Hosts write `log` output and panics to `host.log` in the private directory,
rotating it to `host.log.1` past 1 MiB. A message repeated more than 5 times a
minute is counted instead of written, and a panic report is cut at 16 KiB.
`connect_or_start` appends the host's stderr to the same file, and start-up
errors quote its last lines.

The pre-authentication handshake has one 5 s deadline in total, so a slow or
trickling client loses its slot. At most 32 connections can be in the
handshake; another one closes the oldest, so a client that reconnects as soon
as it is dropped cannot lock out real clients, whose handshake takes
milliseconds.

## Validation

```sh
cargo test -p termy_core
cargo clippy -p termy_core --all-targets -- -D warnings
```

The lifecycle test launches separate client processes. It checks background
progress after client exit, reattaches to the same shell PID, and verifies
both screens, split parser input, colors, input modes, graphics, links,
scrollback, search, layout storage, and explicit child termination.

Set `TERMY_MUX_TEST_HOST_BINARY` to a built Termy executable to run these tests
through the installed app's internal host entry point instead of the test
host executable. IPC tests check authentication, bounded pre-authentication
messages, clipboard bridging, and terminal queries without an attached client.

## Boundaries

GPUI and desktop application state. Shared helpers are sibling core modules.
