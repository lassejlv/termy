# termy_engine_ffi

`libtermy_engine`: Termy's terminal engine as a C library, for hosts that own
their transport, renderer and input, such as a client mirroring a remote
multiplexer's panes. The C header is [`include/termy_engine.h`](include/termy_engine.h).

## Owner

This crate is a thin C ABI over `termy_core::terminal_engine` built with
`default-features = false`: the parser, screens, scrollback, input encoders
and snapshots. It has no PTY, config files, threads, timers, fonts, network,
plugins or multiplexer. Keep exported functions and structs synchronized with
the header; `tests/c_abi.rs` fails when they drift.

It builds `libtermy_engine.a` (static), a shared library (not on musl) and an
rlib. The native desktop ABI (`crates/core/include/termy.h`, which needs the
`native` feature) is separate; the two can be linked into one program.

### What hosts get

- **Snapshots.** `termy_engine_snapshot_encode` captures the full state:
  screens and scrollback with grapheme clusters, styles and links, cursors and
  saved cursors, margins, tab stops, charsets, every mode, kitty keyboard
  stacks, palette overrides, title stack, working directory and the parser's
  in-progress sequence. `decode(encode(t))` fed bytes B ends where `t` fed B
  does. The format is versioned and little-endian; see the header.
- **Replica mode.** `TERMY_ENGINE_REPLIES_DISCARD` parses queries silently.
  In `TERMY_ENGINE_REPLIES_RETURN` mode replies come back from
  `termy_engine_feed` synchronously and in byte order.
- **Cells.** Grapheme clusters through a per-frame arena, wide-cell roles
  (including spacer heads), every SGR attribute, underline styles and colors,
  hyperlinks, `STYLED`/`BLANK` flags, and cursor shape, blink, visibility and
  wide-tail position.
- **Queries.** DECRQM for any mode, mouse/focus/alternate-scroll state, cursor,
  title and directory getters, scrollbar values and viewport scrolling, default
  colors and the effective palette.
- **Encoders** that return bytes: keys (legacy, DECCKM, every kitty flag,
  left/right Option as Alt), mouse (`?9`, `?1000`, `?1002`, `?1003`; X10,
  UTF-8, SGR and SGR-pixel encodings; buttons 1–11), paste and focus.
- **Text.** Ranges and whole-buffer captures as plain text, VT (SGR and OSC 8
  kept) or HTML, plus word and logical-line selection helpers.
- **Frames** with partial damage and documented full-frame rules, borrowing
  handle-owned buffers so steady-state rendering does not allocate.
- **Kitty graphics** as placements with pixel rectangles worked out (crop,
  fit, letterbox, scroll clipping, Unicode placeholders) plus each image's
  RGBA or PNG pixels, so a host draws one textured rectangle per placement.

## Packaging

```sh
scripts/package-libtermy-engine.sh                      # host target
scripts/package-libtermy-engine.sh x86_64-unknown-linux-musl aarch64-unknown-linux-musl
scripts/package-libtermy-engine.sh aarch64-apple-darwin  # also an .xcframework on macOS
```

Each target produces `dist/libtermy-engine-<target>.tar.gz` with
`lib/libtermy_engine.a`, `include/termy_engine.h`, a Clang module map and the
linker flags the static library needs (`lib/link-flags.txt`). On macOS the
script also assembles `TermyEngine.xcframework`, which Swift packages consume
as a `binaryTarget` without unsafe linker flags.

## Validation

```sh
cargo test -p termy_engine_ffi
cargo build --release -p termy_engine_ffi --lib --target x86_64-unknown-linux-musl
```

`tests/c_abi.rs` builds the static library, then compiles, links and runs a C
program against it.

## Forbidden Dependencies

GPUI, keyring, ureq, fontdb, flume, polling, dirs, uuid and the desktop
`termy` crate (`scripts/check-boundaries.sh`). Never enable `termy_core`'s
`native` or `pty` features here.
