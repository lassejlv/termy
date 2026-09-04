# Snapshot and protocol fixtures

`snapshots.json` locks the terminal snapshot codec as part of the daemon/app compatibility contract.
Both fixtures use the deterministic stream from `crates/engine/examples/snapshot_fixture.rs`:
an 8x2 terminal, three retained scrollback rows, fixed pixel dimensions, ASCII text, SGR red, and
bracketed-paste mode.

- `terminal-snapshot-v2-bincode` was generated from immutable revision
  `1f57984df61b2ca54b069e3cf84d0b1286674e87`, whose mux protocol and snapshot version were both 2.
- `terminal-snapshot-v3-postcard` is the current bounded postcard format. The format version was
  deliberately advanced because a codec change is an on-wire compatibility change even when the
  Rust schema is unchanged.

Current code must reproduce and accept the v3 bytes and must reject the v2 bytes. Tmon does not
silently translate an old daemon's in-memory terminal state. Versioned daemon sockets let N-1 keep
owning its live PTYs; the user can launch N-1 to retrieve them or explicitly terminate them after
reviewing `tmon --session-status`.

Never update a golden byte string just to make a test pass. Change the snapshot and mux protocol
versions together, retain the prior fixture, document migration/fallback behavior, and run the
installed N-1/N upgrade and rollback smoke.
