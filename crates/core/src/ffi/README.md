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

## Validation

```sh
cargo test -p termy_core
```

## Boundaries

GPUI and desktop application state. Shared helpers are sibling core modules.
