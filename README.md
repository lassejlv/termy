# Tmon

Tmon is a small, fast terminal engine and native macOS terminal written in Rust. It uses a real PTY, a headless VT parser, Kitty keyboard protocol negotiation, damage-scoped frame updates, and a Metal-only `wgpu` renderer on macOS.

## Run

Requirements: macOS, Rust 1.96+, and the Xcode command-line tools.

```sh
cargo run --release -p tmon
```

The app starts `$SHELL` (falling back to `/bin/zsh`) in the current directory. Child processes receive the macOS-provided `xterm-256color` terminfo contract plus truecolor and Tmon identity variables; Kitty keyboard support is negotiated independently at runtime. Pass a program and arguments to run something else:

```sh
cargo run --release -p tmon -- /usr/bin/top
```

For an ordinary native `.app` development build, use `script/build_and_run.sh`. A release-quality
universal bundle and checksum are produced with:

```sh
bash script/package_macos.sh --universal
```

Useful shortcuts:

- `Command-V`: paste (bracketed paste is honored)
- `Command-C`: copy the current mouse selection
- `Command-F`: focus the top-right search field; type to refine, Enter/Shift-Enter moves between matches, Escape closes
- `Command-G` / `Command-Shift-G`: next/previous search match
- `Command-T`: new native macOS tab in the active terminal's OSC 7 working directory
- `Command-W`: close the active native tab
- `Command-Shift-[` / `Command-Shift-]`: previous/next native tab; `Command-1` through `Command-9` selects a tab
- `Command-+` / `Command--`: change font size
- `Command-0`: reset font size
- `Command-,`: create the active config file if needed and open it in its associated macOS editor
- `Command-Q`: detach the native app; every tab and process keeps running in the multiplexer
- Either `Option`: enter characters from the active macOS keyboard layout, including `@`, `|`, braces, and other localized symbols
- Option-modified navigation and unchanged Option chords remain terminal Alt/Meta keybindings
- Mouse drag: select characters; double-click selects terminal words/paths; triple-click selects lines
- Hold `Shift` to select while an application owns the mouse
- `Command`-click an OSC 8 hyperlink to open it; linked cells use the pointer cursor
- Mouse wheel: smooth history scrolling, or report to applications that enabled mouse tracking

## Config

Tmon reads `~/Library/Application Support/Tmon/config.toml`, or the file named by
`TMON_CONFIG`. Missing config is fine; malformed or unsafe values fail with a clear startup
error. Existing `MetalTerm` config paths remain a fallback during the rename. The native app
defaults to 5,000 history rows to keep daily memory use modest.

```toml
font-family = "Menlo"
font-size = 15
padding = 8
scrollback-limit = 5000
inactive-scrollback-limit = 1000
shell = "/bin/zsh"
working-directory = "~/Code"

[colors]
foreground = "#d8dee9"
background = "#0b0d0f"
cursor = "#486597"
selection-background = "#4c6eaf"
search-background = "#1e222a"
search-foreground = "#eceff4"
search-border = "#4c566a"
search-no-match = "#bf616a"

black = "#2e3440"
red = "#bf616a"
green = "#a3be8c"
yellow = "#ebcb8b"
blue = "#81a1c1"
magenta = "#b48ead"
cyan = "#88c0d0"
white = "#d8dee9"
bright-black = "#4c566a"
bright-red = "#bf616a"
bright-green = "#a3be8c"
bright-yellow = "#ebcb8b"
bright-blue = "#81a1c1"
bright-magenta = "#b48ead"
bright-cyan = "#8fbcbb"
bright-white = "#eceff4"
```

`font-size` accepts 8–36, `padding` accepts 0–64, and both scrollback limits accept 0–100,000.
Every color accepts `#RRGGBB`; omit any entry to keep Tmon's default for that slot. Terminal OSC
color changes still override the configured foreground, background, or cursor for that session and
reset back to the configured theme. Theme changes take effect on the next launch.
Tabs are real AppKit window tabs. Inactive tabs keep draining their bounded PTYs but retain fewer
history rows and do not render.
Both Option keys preserve macOS layout characters such as Danish `@`; Option-modified navigation
and unchanged Option chords remain terminal Alt/Meta.

## Persistent sessions

Tmon starts a private per-user multiplexer automatically. The multiplexer, rather than the native
window process, owns every PTY and a shadow copy of every terminal emulator. `Command-Q`, closing
the final window, or relaunching the app therefore only detaches and reconnects the UI. Running
programs, tab order, the selected tab, terminal buffers, titles, working directories, colors, and
protocol modes are restored on the next launch.

`Command-W` still permanently closes the selected tab when another tab remains. To replace the
final tab, create a new tab first and then close the old one. A shell that exits remains visible
with its exit status so its buffer can still be inspected.

The daemon listens only on a mode-`0600` Unix socket under
`~/Library/Application Support/Tmon/runtime`; its runtime directory is mode `0700`. It continuously
drains bounded PTY queues while no window is attached, so output-producing jobs continue instead of
blocking behind a disconnected UI. Sessions survive app closure within the current login session;
reboot/login restoration is intentionally not claimed.

## Workspace

- `engine`: terminal grid, VT/OSC parser, modes, scrollback, damage, Kitty keyboard and mouse/input encoders, plus PTY child lifecycle and coalesced asynchronous I/O
- `mux`: private Unix-socket daemon, tab/process ownership, emulator snapshots, and reconnect protocol
- `pty`: direct macOS `forkpty`/`execve` process layer with safe command, descriptor, resize, signal, and wait types
- `ffi`: versioned C ABI with opaque terminal/PTY handles, packed borrowed frame and event views, panic containment, and a Clang/Swift module map
- `render`: retained row text plans, pixel-aligned block/box-drawing geometry, instanced cell backgrounds, cursor, and macOS Metal presentation
- `tmon`: winit application and platform event wiring

The engine has no windowing or GPU dependencies. Tmon's `pty` crate talks directly to macOS:
`forkpty` establishes the session, process group, and controlling terminal; a prepared `execve`
launch keeps the post-fork path async-signal-safe; direct close-on-exec `File` descriptors handle
I/O; and `TIOCSWINSZ`, process-group signalling, and `waitpid` cover the remaining lifecycle. The
small unsafe syscall boundary is isolated in `crates/pty/src/native.rs`; callers use safe builder
and ownership types, while the existing `engine::pty::PtySession` API adds asynchronous delivery.
There is no `portable-pty` dependency or dynamic dispatch in the PTY read/write/resize path.

PTY reads use a lossless 512 KiB queued-output budget plus one fixed 32 KiB read buffer: wakeups are edge-coalesced, the multiplexer continuously drains each queue into its shadow emulator, and `PtySession::drain_output_into` swaps reusable bounded producer/consumer allocations without retaining oversized caller storage. Attached GUI output is bounded separately; a client that falls behind receives a fresh emulator snapshot instead of growing the daemon indefinitely. macOS teardown uses an explicit cancellation descriptor, so idle reads add no polling timer and shutdown can still interrupt a blocked reader before joining it. Ordinary edits copy exact dirty cell spans rather than whole rows or full grids. Each attached terminal session owns a native AppKit window in one tab group, while the selected tab reuses the same Metal device, glyph atlas, font system, and scratch storage by retargeting its presentation surface. The renderer retains shaped text and static geometry per row, moves those GPU caches with terminal scroll operations, uses a conservative monospace ASCII shaping path, and bypasses font shaping for common TUI borders so adjacent cells meet on exact device pixels. Cursor and selection use a small dynamic overlay. Presentation remains vsynced; bursty output is folded into the next redraw, PTY geometry changes immediately during live resize, and occluded or inactive tabs do no shaping, upload, or presentation work.

## Embed from C or Swift

Build the ABI library and run its real C consumer check:

```sh
cargo build --release -p ffi
bash script/test_ffi_c.sh
```

The public header and Swift-compatible module map live in `crates/ffi/include`. The ABI
keeps terminal logic inside `engine`, exposes opaque terminal and PTY handles, and returns
reusable borrowed views for partial frames, events, encoded input, selected text, and PTY output.
See `crates/ffi/README.md` for the host loop, ownership windows, threading contract, and a
minimal C integration.

## Performance check

Run the visible end-to-end Metal benchmark in release mode from an unobscured Tmon window:

```sh
bash script/benchmark_metal.sh --samples 30
```

It prints a short table and writes a versioned JSON report containing p50/p95/p99 latency, stage
timings, display and hardware metadata, CPU/RSS observations, renderer/engine work counts, and
idle/occluded/inactive-tab checks. The full release gate adds workspace, C ABI, sustained engine,
and retained-renderer checks before the native workload:

```sh
bash script/performance_gate.sh --samples 30
```

See `performance/README.md` for reference budgets, run conditions, baseline/final reports, and the
GPU-timestamp limitation.

Run the repeatable headless workloads in release mode:

```sh
cargo run --release -p engine --example throughput
cargo run --release -p engine --example interaction_latency
cargo run --release -p engine --example stability
cargo test --release -p engine oversized_output_is_lossless_ordered_bounded_and_rearms_wakes -- --nocapture
cargo test --release -p engine sustained_output_clears_the_debug_throughput_floor -- --nocapture
cargo test --release -p engine dropping_ -- --nocapture
cargo test --release -p render retained_history_scroll_benchmark -- --ignored --nocapture
cargo test --release -p render retained_dense_cjk_benchmark -- --ignored --nocapture
```

`Terminal::metrics()` exposes feed calls, bytes, frame requests, damaged/full frames, row moves,
rows moved, row updates, and copied cells so integration workloads can verify that optimizations
are reducing real work rather than only moving it around. `Terminal::memory_stats()` reports
retained row/cell capacities, and `Terminal::set_scrollback_limit()` applies a new history limit
immediately while releasing rows above it.

`PtySession::buffer_metrics()` exposes pending bytes/capacity, high-water bytes, total buffered/drained bytes, producer waits, allocation growths, drains, and wakeups. `PtySession::io_metrics()` reports requested, issued, and suppressed PTY resizes. These counters are in-memory only and do not add syscalls to the hot path.

The `stability` example fills the configured history, continues sustained output, churns history scrolling and resize, and fails if retained cell capacity keeps growing after the scrollback limit is full. Focused PTY tests stall a stream larger than the userspace budget to verify ordered lossless backpressure, and stream 8 MiB end-to-end with an 8 MiB/s debug-build floor to catch major PTY hot-path regressions.

## Supported protocol surface

The engine implements the common VT100/xterm control set used by shells and TUIs: cursor motion, erase/insert/delete operations, scrolling regions, main/alternate screen, SGR including indexed and RGB color, bracketed paste, focus and SGR cell/pixel mouse modes, device and mode reports, dynamic foreground/background/cursor colors, synchronized output, OSC title/current-directory/hyperlink/clipboard/pointer events, and text selection. Keyboard input is layout-aware and supports committed IME text, legacy VT/xterm keys and C0 aliases, application cursor/keypad modes, xterm `modifyOtherKeys`/`formatOtherKeys`, and all Kitty progressive keyboard flags for host-visible keys, including event types, alternate keys, report-all, and associated text. Lock-modifier reporting is best-effort because macOS/Winit does not expose a reliable initial Caps Lock or Num Lock state.

This is an intentionally compact engine, not yet a compatibility replacement for mature terminals. Image protocols, sixel, bidi shaping, block selection, and shell integration are natural next layers.

## Distribution and updates

`package_macos.sh` builds both Apple Silicon and Intel slices, combines them with `lipo`, applies
the hardened runtime, validates the bundle, and emits a versioned zip plus SHA-256 checksum under
`dist`. With no configuration it uses an ad hoc signature suitable only for local use. For a public
release, set `TMON_SIGN_IDENTITY` to a Developer ID Application identity, package again, store
notary credentials with `xcrun notarytool store-credentials`, then run:

```sh
APPLE_NOTARY_PROFILE=tmon bash script/notarize_macos.sh
```

The release zip is the deliberately small update contract for now: verify its adjacent checksum,
replace `Tmon.app`, and keep the config file in Application Support. There is no privileged
installer or background updater.

## Compatibility matrix

| Target | Status |
| --- | --- |
| Apple Silicon, macOS 26 | Runtime, native tabs, PTY lifecycle, input, resize, idle CPU/RSS, and Metal rendering verified |
| Intel, macOS 14+ | Universal x86_64 slice builds; runtime hardware validation remains required |
| macOS 14–15 on Apple Silicon | Declared deployment target; runtime validation remains required |
| Linux and Windows app | Not supported; the reusable engine remains window/GPU independent |

Terminal coverage includes the protocol surface listed above. Kitty graphics, sixel, bidi shaping,
rectangular selection, ligature controls, and automatic updates are not claimed as supported.
