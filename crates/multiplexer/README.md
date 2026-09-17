# termy_multiplexer

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

## Validation

```sh
cargo test -p termy_multiplexer
cargo clippy -p termy_multiplexer --all-targets -- -D warnings
```

The lifecycle test launches separate client processes. It checks background
progress after client exit, reattaches to the same shell PID, and verifies
both screens, split parser input, colors, input modes, graphics, links,
scrollback, search, layout storage, and explicit child termination.

Set `TERMY_MUX_TEST_HOST_BINARY` to a built Termy executable to run these tests
through the installed app's internal host entry point instead of the test
host executable. IPC tests check authentication, bounded pre-authentication
messages, clipboard bridging, and terminal queries without an attached client.

## Forbidden Dependencies

- `gpui`
- `termy` / `crates/desktop_app`
- `termy_terminal_ui`
- `termy_tmux_control_core`
- `termy_plugin_runtime`
