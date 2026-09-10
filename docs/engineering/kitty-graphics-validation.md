# Kitty graphics validation

Validated on macOS on 2026-09-10. Protocol reference:
[Kitty graphics specification](https://sw.kovidgoyal.net/kitty/graphics-protocol/).

## Implementation

Both terminal engines now decode uploads into shared RGBA images. PNG encoding
is deferred until export, and native drawing reuses bounded GPU textures.
The protocol implementation includes chunked RGB/RGBA/PNG uploads, zlib, file
and shared-memory transfers, image numbers, placement and deletion selectors,
Unicode placeholders, relative placements, animation frames and composition.
Placement geometry retains natural pixel dimensions and aspect ratios; screen
and margin scrolling, offsets, crops, image layers, and hit testing use the
same placement information. Native drawing uses repeated texture borders and
explicit paint bounds so magnified one-pixel images have solid edges.

## Automated checks

The affected crates have **1,591 passing tests**, with nine existing integration
tests ignored because they require a live tmux environment. The public runtime
regressions also pass with the Alacritty backend explicitly selected. Checks:

```sh
cargo test --locked -p termy -p termy_core -p termy_terminal_ui -p termy_ffi -p tmon
TERMY_CORE_TEST_BACKEND=alacritty cargo test --locked -p termy_core
cargo clippy --locked -p termy -p termy_core -p termy_terminal_ui -p termy_ffi -p tmon --all-targets -- -D warnings
cargo check --locked --workspace
cargo build --locked -p termy --bin termy
bash scripts/check-boundaries.sh
```

The boundary check retains existing allowlisted large-file warnings. Tests cover
chunk completion, UTF-8 interception, storage limits, deletion, placeholder
holes and repeated instances, signed relative origins, partial-region clipping,
resize geometry, frame blending, animation deadlines/loops, and shared-memory
range handling. The C header/layout guard passes.

## Upload benchmark

The same debug-build harness uploads eight 1024×1024 RGBA images and reads a
placement snapshot after each upload. Commands are constructed before timing;
Base64 decoding, image processing, storage, and snapshot work are timed.

| Version | Time for eight uploads |
| --- | ---: |
| Before rewrite, `d1dd82a4` | 3.144 s |
| After rewrite | 0.201 s |

This is approximately **15.6× faster for this upload workload**. It is a local
single-workload measurement, not a release-build, GPU, whole-application, or
frame-rate benchmark. To run the current harness:

```sh
cargo test --locked -p termy_core kitty_graphics::conformance::measure_raw_upload_cost -- --nocapture
```

## Native visual check

Run this inside Termy:

```sh
python3 crates/desktop_app/examples/kitty_graphics_conformance.py
python3 crates/desktop_app/examples/kitty_graphics_conformance.py --scroll
```

The isolated native test window visibly verified natural sizing, aspect ratio,
crops, offsets, all three z layers, placeholder gaps and repeated instances,
left-edge clipping, advancing animation colors, margin scrolling, a fixed
footer image, and the final cursor position of a chunked upload. The magnified
one-pixel image was checked for a solid fill. Window resize was used to refresh
the surface during automation; automated keyboard page switching was not
verified. The demo can start on either page without keyboard input.

## Compatibility boundaries

- Cursor, Grok, and other third-party applications still need individual live
  acceptance runs. These checks use deterministic protocol fixtures.
- Windows shared-memory code is implemented but was not executed on Windows;
  Unix shared-memory reads and unlinking were exercised on macOS.
- The C placement struct has new fields. Embedders must rebuild against the
  updated header. PNG export ownership remains unchanged.
- The protocol keeps bounded memory, image dimensions, placement counts, and
  relative-placement depth. Resource limits are documented in the core/Tmon
  modules and return protocol errors when exceeded.

## Grok Build synchronized preview regression (2026-09-11)

Captured the image-preview output from installed Grok Build 1.0.25 using the
bundled `kitty-demo.png`, without submitting a prompt. Grok wraps cursor
movement and chunked image uploads in DEC synchronized updates (mode 2026).
The Alacritty backend previously applied intercepted Kitty commands before
VTE replayed that cursor movement: a minimized regression placed the preview
at zero-based `(79, 23)` instead of the requested `(29, 4)`.

Both Alacritty ingestion paths now replay preceding text through the graphics
tracking handler before applying a Kitty command, then resume synchronization.
The native PTY loop defers redraw wakeups until synchronization ends. This
preserves the ordering of cursor movement, screen switches, clears, scrolling,
and graphics while retaining the synchronized-update watchdog.

The public runtime regressions pass with both engines, including whole-frame,
seven-byte, and single-byte input splits. Full core tests, core Clippy, and the
debug application build pass. Native replay of the captured Grok frame in the
rebuilt Alacritty-backed app visibly places the complete image inside its
preview panel. A live Grok startup was also checked; automated keyboard/paste
input in the isolated benchmark window did not work, so the image-placement
visual evidence is the captured-frame replay, not a live interactive paste.

The repeatable native smoke test also supports synchronized redraws:

```sh
python3 crates/desktop_app/examples/kitty_graphics_conformance.py --sync
python3 crates/desktop_app/examples/kitty_graphics_conformance.py --sync --scroll
```
