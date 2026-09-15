# termy_native_sdk

Narrow native OS integration layer.

## Owner

This crate owns reusable platform-specific primitives that are cleaner outside the desktop app crate, such as macOS AppKit/Foundation helpers, Windows shell integration helpers, file-manager "Open new Termy tab here" registration, and cross-platform MIME-aware clipboard access used by the Kitty clipboard protocol.

Keep product workflows in `crates/desktop_app/`; keep cross-platform terminal behavior in `termy_core`.

## Validation

```sh
cargo test -p termy_native_sdk
```

## Forbidden Dependencies

- `gpui`
- `termy_terminal_ui`
- `termy_ffi`
- `termy` / `crates/desktop_app`
