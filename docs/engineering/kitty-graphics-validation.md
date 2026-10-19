# Kitty graphics validation

Validated on macOS on 2026-09-10. Protocol reference:
[Kitty graphics specification](https://sw.kovidgoyal.net/kitty/graphics-protocol/).

## Implementation

The terminal engine decodes uploads into shared RGBA images. PNG encoding
is deferred until export, and native drawing reuses bounded GPU textures.
The protocol implementation includes chunked RGB/RGBA/PNG uploads, zlib, file
and shared-memory transfers, image numbers, placement and deletion selectors,
Unicode placeholders, relative placements, animation frames and composition.
Placement geometry retains natural pixel dimensions and aspect ratios; screen
and margin scrolling, offsets, crops, image layers, and hit testing use the
same placement information. Native drawing uses repeated texture borders and
explicit paint bounds so magnified one-pixel images have solid edges.

## Automated checks

The protocol regressions also cover omitted/zero parent placement IDs in cycle
detection, silent anonymous uploads, and explicit placement boxes that keep the
full requested cursor occupancy across resize and deletion.

Behaviour follows kitty's reference implementation (and ghostty where they
agree), checked against their sources on 2026-10-19:

- A direct placement with both `c` and `r` stretches the image over that box.
  Only Unicode placeholder placements fit the image inside their box (whole
  cells, an omitted dimension taken from the natural size) and center it.
- After a placement the cursor ends on the image's last row, just past its
  right edge, scrolling at the bottom margin. An image reaching the right edge
  of the screen wraps the cursor to the start of the next line.
- A direct transmission arriving during a chunked upload continues it, whatever
  its other keys: only `m` and `q` apply, so clients that repeat every key on
  each chunk work. A chunk naming another image id or number, or another
  transmission medium, abandons the unfinished upload instead. Run the public conformance suite:

```sh
cargo test --locked -p termy_core --test kitty_conformance
```

Current validation commands:

```sh
cargo test --locked -p termy -p termy_core -p termy -p termy_core -p termy_core
cargo test --locked -p termy_core
cargo clippy --locked -p termy -p termy_core -p termy -p termy_core -p termy_core --all-targets -- -D warnings
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
  relative-placement depth. Resource limits are documented in the core graphics
  modules and return protocol errors when exceeded.

## Historical Grok Build synchronized preview regression (2026-09-11)

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
