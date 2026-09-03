# Tmon FFI

`ffi` is the versioned C ABI for embedding the headless Tmon engine in Swift,
Objective-C, C, C++, Zig, Go, or any runtime that can call C. Terminal behavior remains in
`engine`; this crate only validates foreign inputs, translates stable structs, contains
panics, and owns reusable ABI buffers.

Rust applications should depend on `engine` directly.

## Build

```sh
cargo build --release -p ffi
```

On macOS this produces:

- `target/release/libtmon_ffi.dylib`
- `target/release/libtmon_ffi.a`
- `crates/ffi/include/tmon.h`
- `crates/ffi/include/module.modulemap` for Clang and Swift module imports

The header is the ABI source of truth. Check `tmon_abi_version()` against
`TMON_ABI_VERSION` before accepting a dynamically loaded library.

## Host loop

1. Start with `tmon_terminal_config_default()` and create a terminal.
2. Optionally spawn a `TmonPty`; its callback should only schedule work on the host event
   loop.
3. Drain PTY output and feed the returned bytes to the terminal.
4. Drain terminal events.
5. Request a frame update. Apply every `row_moves` operation to retained rows in array order,
   then patch the `row_updates` spans. Rebuild the whole grid only when `full` is nonzero.
6. Encode key, text, paste, mouse, and focus input through the terminal, then write the returned
   bytes to the PTY.

High-frequency return values are borrowed views backed by reusable storage on their opaque handle.
Their exact validity windows are documented beside each declaration in `tmon.h`. Copy a view
only if it must survive the next corresponding call. This avoids per-frame and per-read ownership
traffic across the ABI.

## ABI version 2 row movement

ABI version `0x00020000` adds `TmonRowMove`, `TmonFrameView.row_moves`, and the
`row_moves`/`rows_moved` terminal metrics. This is an intentional structure-layout change, so
version 1 hosts must reject it rather than reading a version 2 frame as the old layout.

Each move rotates retained rows inside `[start_row, end_row)` by `count`. Direction
`TMON_ROW_MOVE_UP` moves later rows toward `start_row`; `TMON_ROW_MOVE_DOWN` moves earlier rows
toward `end_row`. Moves are applied before cell patches. A full frame carries no moves and remains
the compatibility fallback for resize, screen changes, resynchronization, and combinations that
cannot be represented safely.

The move array is borrowed from the terminal handle under the same lifetime rules as row updates
and cells. Hosts may rotate row-local shaped text and GPU buffers along with their retained cells,
then rebuild only exposed or subsequently modified rows.

Calls for one handle must be serialized, and a handle must not be freed while a call is active. PTY
callbacks run on the reader thread; `user_data` must remain valid until `tmon_pty_free` returns.
All entry points catch Rust panics and return a `TmonStatus`. On failure,
`tmon_last_error_message()` returns the current thread's diagnostic.

## Minimal C host

```c
#include <tmon.h>

TmonTerminalConfig config = tmon_terminal_config_default();
TmonTerminal *terminal = NULL;
if (tmon_terminal_new(&config, &terminal) != TMON_OK) {
  /* tmon_last_error_message() explains the failure */
}

const uint8_t output[] = "hello";
tmon_terminal_feed(terminal, output, sizeof(output) - 1);

TmonFrameView frame = {0};
tmon_terminal_frame_update(terminal, 1, &frame);
/* Apply frame.row_moves, then render frame.row_updates and frame.cells. */

tmon_terminal_free(terminal);
```

## Verify the foreign boundary

```sh
cargo test -p ffi
bash script/test_ffi_c.sh
```

The C smoke test compiles with warnings-as-errors, links the actual dynamic library, and exercises
construction, parsing, partial frames, OSC events, keyboard encoding, metrics, and destruction.
