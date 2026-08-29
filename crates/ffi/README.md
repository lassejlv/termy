# termy_ffi

C-compatible libtermy surface.

## Owner

This crate is a thin FFI wrapper over `termy_core`. Keep exported structs and functions synchronized with `crates/ffi/include/termy.h`, and rebuild this crate before validating native host examples after ABI changes.

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

## Validation

```sh
cargo test -p termy_ffi
```

## Forbidden Dependencies

- `gpui`
- `termy_terminal_ui`
- `termy` / `crates/desktop_app`
