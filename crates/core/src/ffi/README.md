# termy_core::ffi

C-compatible libtermy surface.

## Owner

This module is a thin FFI wrapper over `termy_core`. Keep exported structs and functions synchronized with `crates/core/include/termy.h`, and rebuild this module before validating native host examples after ABI changes.

`termy_cells_build_glyph_render_plan` exposes core's special-glyph semantics as
one sparse batch over a retained full frame and optional dirty spans. Hosts own
the retained frame and final pixel snapping. Every successful plan must be
released with `termy_glyph_render_plan_free`.

`termy_terminal_drain_events_with_clipboard` exposes Kitty OSC 5522 through
synchronous host callbacks. Requests and write content are callback-scoped; the
read reply callback copies host response slices before returning. Hosts own
clipboard permissions and must not re-enter the terminal from a callback.
Display-only hosts forward bytes received by the protocol-reply callback to
their external transport.

The Kitty placement struct now includes signed column offsets, Unicode cell
coordinates, margin clipping, and `next_frame_delay_ms` (zero means static).
The PNG buffer remains independently owned and is released with the batch.
Hosts must rebuild against the updated header; the placement struct ABI changed.

Display terminals can mirror another terminal. `termy_terminal_snapshot_encode`
and `termy_terminal_snapshot_decode`/`_restore` carry the engine's full state
(see `terminal_engine/snapshot.rs`). `termy_terminal_set_protocol_replies_enabled`
turns a display terminal into a replica that parses queries without answering,
and `termy_terminal_feed_output_with_replies` returns replies synchronously, in
the order the engine produced them. A NULL `protocol_reply_callback` drops
queued replies without changing other state.

`ffi/multiplexer.rs` exposes the session host behind `termy mux`.
`termy_multiplexer_connect` starts or reaches a host in a session directory
(the host refuses one other users can read), and
`termy_multiplexer_create_terminal` / `_attach_terminal` return ordinary
`TermyFfiTerminal` handles viewing a host pane. Freeing such a handle only
detaches; `termy_multiplexer_close` ends the program. Statuses
`TERMY_FFI_HOST_UNAVAILABLE`, `_VERSION_MISMATCH` and `_NOT_FOUND` report host
problems. The host binary is `termy-session-host`; hosts ship it beside the
library.

The host also owns the window, workspace, tab and split layout that Termy and
`termy mux` share. `termy_multiplexer_layout_json` returns it as a tree of
pane IDs (`{"leaf": id}` or `{"axis", "ratio", "first", "second"}` per tab),
`termy_multiplexer_set_layout_json` writes one back if the layout is still at
the caller's revision (libtermy derives the stored cell rectangles and split
tree), and `termy_multiplexer_watch_layout` calls back on a thread of its own
with each new revision until `termy_layout_watch_free`.

The `termy_settings_*` calls edit the Termy app's config file. The `_at`
variants (`termy_settings_set_root_at`, `termy_settings_reset_root_at`) take a
config path, so a host app can keep a config of its own. A config loaded from
that path lists its themes, including local ones beside it, in
`termy_settings_schema_json`.

The library is also built as a `staticlib`. Hosts that need only the engine,
without the `native` feature's PTY, config, keyring, font and network code,
link `libtermy_engine` from `crates/engine_ffi` instead.

## Validation

```sh
cargo test -p termy_core
```

## Boundaries

GPUI and desktop application state. Shared helpers are sibling core modules.
