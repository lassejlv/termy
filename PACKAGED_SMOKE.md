# Packaged-app compatibility smoke

Run the automated portion against the exact candidate archive before doing the visual matrix:

```sh
bash script/packaged_smoke.sh \
  --archive dist/Tmon-0.1.0-1-macos-universal.zip \
  --output release/evidence/packaged-smoke.json
```

The script copies and checksum-verifies the archive away from the build tree, extracts its app,
validates bundle metadata and code signing, starts the packaged executable with an isolated home and
multiplexer, proves a real PTY child survives app detachment, reattaches without spawning a duplicate
session, exercises the privacy-safe support command, and explicitly terminates only the isolated
test session. A Developer ID candidate must also pass Gatekeeper while quarantined. For an internal
ad hoc candidate, `--allow-adhoc` permits the runtime portion but records
`distribution_ready: false`; that result cannot satisfy the public release gate.

The automated run deliberately does not touch the user's normal config, diagnostic log, socket, or
daemon-owned sessions. The JSON omits all temporary paths and terminal contents.

## Manual visual matrix

Use the extracted candidate on every OS/architecture row still claimed for the release. Keep the
window visible and record pass/fail, app version/build, OS/build, architecture, GPU, display scale,
refresh rate, font, keyboard layout, and a screenshot or short recording for failures. Never attach
private terminal content to a public issue without reviewing it first.

| Area | Exact exercise | Pass condition |
| --- | --- | --- |
| Login shells | Launch default zsh, then configured bash; run `printf 'ascii æøå 界 e\u0301 ⏺️ █⣿\n'`. | Prompt, cursor, Unicode, emoji, block, and Braille cells remain aligned. |
| SSH | Connect to a disposable test host, resize repeatedly, detach the app, relaunch, and exit. | Remote input/output survives resize and local app detachment without duplicated input. |
| tmux | Start a fresh tmux server, split panes, switch panes, use copy mode, resize, detach, reattach. | Borders/cursors stay aligned; keys and mouse route once. |
| Editors | Open Vim/Neovim in a disposable file; insert composed text, search, split, scroll, undo, quit. | No stale rows, cursor drift, lost key releases, or committed pre-edit before IME commit. |
| Pager | Pipe colored, wrapped Unicode output through `less -R`; search and jump to start/end. | Alternate-screen restoration, search, scroll, and wide cells are correct. |
| Mouse TUI | Run a disposable full-screen mouse application; click, drag, wheel, focus away/back. | Reports target the intended cells and stop on focus loss. |
| Claude Code | Reproduce the stored greeting/record-symbol fixture, stream output, interrupt, and resize. | `⏺`/`⏺️` consumes one cell and following text never shifts. |
| OpenCode | Stream a response until the footer spinner advances through several frames. | Spinner replacement changes only its own fixed cell; every footer item stays stationary. |
| IME | Compose multi-codepoint text in the terminal and search field; move/replace the marked range. | Pre-edit, selection, caret, and candidate window remain cell-anchored; PTY receives only commit. |
| Clipboard/links | Verify ordinary copy/paste, bracketed paste, default-disabled OSC 52, and an HTTPS link. | Policy matches config; no inactive-tab clipboard write; unsafe schemes stay closed. |
| Native tabs | Create, switch, reorder, close, detach, and restore multiple tabs with active output. | Each PTY and title maps to one native tab; inactive output causes no redraw loop. |

## Display and lifecycle matrix

The native Metal benchmark automatically covers rapid grid resize, surface-only resize, 1x/2x
scale-dependent font/grid rebuilds, same-window surface recreation, native-tab retargeting, idle
zero-work, synthetic occlusion, and normal surface retry accounting. The following remain
physical/manual because a deterministic harness cannot truthfully simulate the external display or
system transition itself:

| Event | Exercise | Pass condition |
| --- | --- | --- |
| Retina and non-Retina | Move the same window between both display classes and type/scroll. | Scale changes remeasure once; glyphs stay sharp and cells/PTY pixels agree. |
| Refresh-rate change | Switch the display between 60 Hz and its highest supported rate. | No stale frame or redraw storm; cursor cadence remains stable. |
| Sleep/wake | Leave output active, sleep for at least 30 seconds, wake, type, and resize. | Session survives, queued output drains in order, and Metal presentation resumes. |
| Occlusion/minimize | Cover and minimize for 30 seconds while output continues, then reveal. | Hidden window does no presentation work and reveals the latest complete state. |
| Display disconnect | Place the app on an external display, disconnect it, then resize on the survivor. | Window and Metal surface recover without process or session loss. |
| Surface loss/reconfiguration | Exercise sleep/wake, display disconnect, and rapid full-screen transitions. | Retried/recreated surfaces present a complete frame with no terminal-state reset. |

Mark a row **pass** only from the exact packaged candidate on the named target. Use **blocked** when
hardware, credentials, a disposable SSH host, or an external application is unavailable; do not
translate compile-time or emulator tests into a visual pass.
