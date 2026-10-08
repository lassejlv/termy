# termy_pty

Node-API addon behind the `@termysh/pty` npm package in `packages/pty`.

## Owner

This crate owns one small native class over `termy_core::pty` (built with
`default-features = false, features = ["pty"]`): spawn a program on a Unix PTY
or Windows ConPTY, write input, resize, signal, pause and resume, and deliver
output and exit events to JavaScript in order through one bounded threadsafe
function. A full queue blocks the PTY reader, which applies backpressure to the
program.

Process handling (fork, exec, ConPTY, writer limits, exit status) belongs to
`termy_core::pty`. The node-pty-compatible API, environment defaults, encodings
and binary loading belong to the TypeScript package.

## Validation

```sh
cargo test -p termy_core --lib terminal_engine::transport
cargo clippy -p termy_pty -- -D warnings
cd packages && bun run --filter '@termysh/pty' build:native && bun run test
```

The addon links against Node-API symbols that exist only inside a Node process,
so it has no Rust test harness; `packages/pty/test` drives it from Node.

## Forbidden Dependencies

- `gpui`
- `termy` / `crates/desktop_app`
- `termy_core` default features (keyring, fonts, networking and SSH must not
  reach the npm package)
