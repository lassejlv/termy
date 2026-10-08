# Terminal engine review fixes — 2026-10-08

This change fixes four findings reproduced against `d801370d`.

- **Shared-memory truncation:** Linux copies Kitty shared-memory transfers with
  bounded descriptor reads, so truncation after validation returns an error
  instead of delivering SIGBUS during a mapped copy. macOS shared-memory
  descriptors do not support `pread`; that path uses `mach_vm_read_overwrite`
  to copy through the kernel without dereferencing externally owned pages in
  Rust. Names are still unlinked on completion or failure, and existing size
  and offset limits remain enforced.
- **Images after height resize:** history trimming emits its graphics effect
  regardless of whether it comes from a history-limit change or resize. Image
  anchors move when the oldest rows are evicted. Tests cover zero history,
  full history, spare history capacity, and resizing while the primary screen
  is parked behind the alternate screen.
- **Grapheme damage:** a width change marks the entire cleared cell span before
  writing the new glyph. A hidden cursor no longer leaves an erased wide spacer
  out of incremental updates. Tests cover both cursor visibility states and
  glyphs ending at the right margin.
- **Placement lookup:** per-screen virtual-placement counts replace a linear
  scan on each text feed. Insertion, replacement, deletion, image removal,
  capacity eviction and reset keep those counts current. Tests also check that
  clearing or scrolling direct placements preserves virtual prototypes.

## Validation

On Linux with Rust 1.99.0:

- `cargo test -p termy_core --offline`: 828 tests passed, two ignored. This includes
  all 217 terminal-engine tests and the graphics, FFI, IPC and display suites.
  Existing Unix-socket fixtures required execution outside the sandbox.
- The original standalone shared-memory race probe returned
  `EIO:unable to read shared-memory range` instead of SIGBUS. Both standalone
  rendering regression tests now pass.
- Linux core-library Clippy with `-D warnings` passed.
- The macOS headless core passed both cross-compilation and Clippy with
  `-D warnings` for `aarch64-apple-darwin`. Native macOS execution was unavailable;
  the platform-specific copy path still needs the normal macOS CI run.
- Formatting, diff whitespace checks and repository boundary checks passed.
- All six warmed 32 MiB allocation workloads reported zero allocations.

The core test build retains an existing unused-import warning in
`keyboard.rs:961`; library Clippy is clean. The two ignored tests require tmux.
No native Windows or macOS runtime tests were executed in this Linux environment.

## Retained-placement benchmark

The checked-in [graphics-text probe](../../scripts/terminal-engine-audit/graphics-text.rs)
performs 100,000 two-byte `\rx` feeds on a 120×40 alternate screen, with all
images retained on the inactive primary screen. Uploads, placement creation,
snapshot validation, switching screens and warmup precede timing.

Both executables were compiled from identical source with `rustc -O`, linked
against release core libraries built without default features. Runs used
baseline/candidate/candidate/baseline order after all builds and tests finished;
each run supplied five samples per case with ascending/descending case order
alternated. Results below are medians of ten samples per revision.

| Inactive direct placements | Baseline | Fixed | Speedup |
| ---: | ---: | ---: | ---: |
| 0 | 5.344 ms | 5.075 ms | 1.05× |
| 64 | 6.962 ms | 5.331 ms | 1.31× |
| 512 | 30.269 ms | 5.110 ms | 5.92× |
| 4,096 | 258.030 ms | 5.147 ms | 50.13× |

This measures small-feed parsing and graphics bookkeeping, not GPU presentation,
PTY latency or bulk-output throughput. No performance claim is made for the new
shared-memory copy paths. The
[raw measurement data](terminal-engine-review-results-2026-10-08.json)
includes all samples, source/binary hashes and allocation-benchmark output.
