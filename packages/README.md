# @termysh packages

Termy's terminal engine on the web. The parser, grid, scrollback, keyboard and
mouse encoders, kitty protocols, glyph geometry and themes are the same Rust
code the desktop app runs, compiled to WebAssembly.

| Package | What it is | Use it when |
| --- | --- | --- |
| [`@termysh/core`](core) | wasm engine + typed headless API | You render yourself, run in a worker/Node, or test terminal output |
| [`@termysh/web`](web) | Browser terminal: canvas renderer, input, selection, links, images | New code |
| [`@termysh/xterm`](xterm) | xterm.js-compatible `Terminal` on top of `@termysh/web` | Replacing `@xterm/xterm` in existing code |

## Quick start

```ts
import { Terminal } from '@termysh/web'

const term = new Terminal({ theme: 'tokyo-night', fontSize: 13, cursorBlink: true })
term.open(document.getElementById('terminal')!)

const socket = new WebSocket('wss://example.com/pty')
socket.binaryType = 'arraybuffer'
socket.onmessage = (event) => term.write(new Uint8Array(event.data))
term.onData((data) => socket.send(data))
term.onResize(({ cols, rows }) => socket.send(JSON.stringify({ cols, rows })))
```

Replacing xterm.js:

```diff
-import { Terminal } from '@xterm/xterm'
-import { FitAddon } from '@xterm/addon-fit'
-import '@xterm/xterm/css/xterm.css'
+import { Terminal, FitAddon } from '@termysh/xterm'
```

The constructor is synchronous. The wasm module loads in the background and
calls made before it is ready (`write`, `resize`, `paste`, ...) are queued.
`await term.ready` when you need the engine immediately.

### Loading the wasm

By default `termy.wasm` is resolved next to `@termysh/core`'s module via
`new URL('./termy.wasm', import.meta.url)`. Vite 8, Next.js (Turbopack) and
Node/Bun need no setup; Vite 7, webpack 5 and esbuild need one setting each
(see [Frameworks and bundlers](https://termy.sh/docs/developer/web/frameworks),
source in `website/content/docs/developer/web/`). To host it yourself:

```ts
import wasmUrl from '@termysh/core/termy.wasm?url' // Vite
new Terminal({ wasm: wasmUrl })
// or: await init(fetch('/assets/termy.wasm')); initSync(bytes) in workers
```

## Design

```
┌───────────────────────────── @termysh/xterm ─────────────────────────────┐
│ xterm.js API: Terminal, options proxy, buffer.active.getLine().getCell(),│
│ FitAddon, WebLinksAddon, SearchAddon, no-op Webgl/Canvas/Unicode11       │
└───────────────────────────────────┬───────────────────────────────────────┘
┌──────────────────────────────── @termysh/web ─────────────────────────────┐
│ Terminal: lifecycle, options, events, selection, links, scrollbar, a11y   │
│ CanvasRenderer (damage-driven rows) · ImageLayer (kitty graphics)         │
│ textarea input (keys, IME, paste) · mouse (selection / reports / wheel)   │
└───────────────────────────────────┬───────────────────────────────────────┘
┌──────────────────────────────── @termysh/core ────────────────────────────┐
│ TermyCore: write · resize · takeDamage · readRows · encodeKey/Mouse/Paste │
│ takeReplies · takeEvents · readGraphics · themes · glyphPlan              │
└───────────────────────────────────┬───────────────────────────────────────┘
                   crates/wasm (wasm-bindgen) → termy_core (no `native` feature)
```

Principles:

- **One engine.** Terminal semantics live in Rust only. TypeScript never parses
  escape sequences or encodes keys; it calls `encodeKey` / `encodeMouse` /
  `encodePaste`, so web and desktop send identical bytes (legacy, application
  cursor and the kitty keyboard protocol).
- **Copy-based, flat reads.** `readRows` returns a `Uint32Array` with six slots
  per cell (text, fg, bg, underline color, style bits, link) plus a string table
  for graphemes and OSC 8 URIs. No views into wasm memory survive a call, so
  memory growth can never detach them.
- **Damage-driven rendering.** The renderer repaints only rows the engine
  reports dirty (plus cursor, selection and hover changes) once per animation
  frame. Synchronized updates (mode 2026) are honored with the engine's
  deadline.
- **Host-agnostic.** The engine does no I/O. Bytes in through `write`, bytes out
  through `onData`/`onBytes` (including protocol replies such as DA and cursor
  reports), so any transport works: WebSocket, WebRTC, a worker, an in-page shell.

## API (`@termysh/web`)

```ts
const term = new Terminal(options?)        // or: await Terminal.create(options)
term.open(parent)                          // fills parent; autoFit follows its size
term.write(data, callback?) / writeln(data)
term.paste(text) / term.input(text)        // input = as if typed
term.resize(cols, rows) / term.fit() / term.proposeDimensions()
term.setOptions({ ... }) / setOption(key, value) / getOption(key)
term.focus() / blur() / clear() / reset() / dispose()
term.scrollLines(n) / scrollPages(n) / scrollToTop() / scrollToBottom() / scrollToLine(line)
term.select(col, line, length) / selectLines(a, b) / selectAll() / clearSelection()
term.getSelection() / hasSelection() / getSelectionPosition()
term.getLine(line) / isLineWrapped(line) / cursor / historySize / viewportY / modes
term.attachCustomKeyEventHandler((event) => boolean)
term.core                                  // the TermyCore, for advanced use
```

Events return an `IDisposable`:
`onData`, `onBinary`, `onBytes`, `onResize`, `onTitleChange`, `onBell`,
`onSelectionChange`, `onScroll`, `onRender`, `onKey`, `onCursorMove`,
`onWriteParsed`, `onCwdChange` (OSC 7), `onProgress` (OSC 9;4),
`onProgramStatus` (OSC 7501), `onClipboard` (OSC 52), `onShellIntegration` (OSC 133), `onFocus`, `onBlur`.

Lines are absolute: `0` is the oldest scrollback line and `historySize` is the
first live-screen line (the same convention as xterm.js `buffer` rows).

## Customization

Every option can change at runtime with `setOptions`; fonts, padding and DPR
re-measure and re-fit, everything else repaints.

| Area | Options |
| --- | --- |
| Font | `fontFamily`, `fontSize`, `fontWeight`, `fontWeightBold`, `lineHeight`, `letterSpacing` |
| Colors | `theme`, `drawBoldTextInBrightColors`, `minimumContrastRatio`, `allowTransparency` |
| Cursor | `cursorStyle` (`block`/`bar`/`underline`), `cursorBlink`, `cursorWidth`, `cursorInactiveStyle` |
| Layout | `cols`, `rows`, `autoFit`, `padding`, `scrollback`, `scrollbar`, `devicePixelRatio` |
| Rendering | `customGlyphs` (Termy box/block/sextant/Braille geometry), `images` (kitty graphics) |
| Scrolling | `scrollSensitivity`, `fastScrollSensitivity`, `fastScrollModifier`, `alternateScroll`, `scrollOnUserInput` |
| Input | `macOptionIsMeta`, `macOptionClickForcesSelection`, `rightClickSelectsWord`, `altClickMovesCursor`, `disableStdin`, `ignoreBracketedPasteMode`, `convertEol` |
| Selection | `wordSeparator`, `copyOnSelect` |
| Links | `linkHandler` (`activate`/`hover`/`leave`, `allowNonHttpProtocols`), `linkDetection` |
| Integration | `allowClipboardWrite` (OSC 52), `bellStyle`, `screenReaderMode`, `wasm` |

Applications can still override the cursor shape with DECSCUSR; `cursorStyle`
is the default it returns to.

### Themes

`theme` accepts three forms:

```ts
new Terminal({ theme: 'catppuccin-mocha' })                       // bundled Termy theme
new Terminal({ theme: { background: '#000', foreground: '#ddd' } }) // xterm.js ITheme shape
new Terminal({ theme: { extends: 'nord', cursor: '#ff0' } })      // bundled + overrides
```

Bundled ids come from `builtinThemeIds()`: `termy`, `termy-light`,
`tokyo-night`, `catppuccin-mocha`, `dracula`, `gruvbox-dark`, `nord`,
`solarized-dark`, `one-dark`, `monokai`, `material-dark`, `palenight`,
`tomorrow-night`, `oceanic-next`. Theme keys match xterm.js (`ITheme`), plus
`scrollbarThumb`. Colors accept any CSS color. The 16 ANSI colors come from the
theme, 16-255 from `extendedAnsi` or the standard cube, and applications can
still change them with OSC 4/10/11/12; query replies report the active theme.

`@termysh/xterm` starts from xterm.js's default colors so an unthemed terminal
looks the same after migrating; pass `termyTheme: 'dracula'` to start from a
Termy theme instead.

## Protocol support

Inherited from the desktop engine: VT100-VT520 and xterm control sequences,
256-color and truecolor, styled and colored underlines (`4:3`, `58`), OSC 8
hyperlinks, OSC 4/10/11/12 color set and query, OSC 7 cwd, OSC 52 clipboard,
OSC 133 shell integration, OSC 9;4 progress, OSC 7501 program status, synchronized output (2026),
bracketed paste, focus events, X10/normal/button/any mouse tracking with
default/UTF-8/SGR encodings, the kitty keyboard protocol (all flags, including
release events) and the kitty graphics protocol (RGBA/RGB/PNG, placements,
z-index layering, animation, Unicode placeholders).

Not yet: sixel and iTerm2 inline images, xterm.js decorations and custom link
providers, `registerOscHandler`/parser hooks, and kitty OSC 5522 clipboard.

## Project structure

```
crates/wasm/                termy_wasm: wasm-bindgen surface (cells, input, graphics, glyphs, themes)
packages/
  package.json              Bun workspace: build, typecheck, test
  core/
    scripts/build-wasm.mjs  cargo build + wasm-bindgen + wasm-opt → src/wasm, dist/termy.wasm
    src/                    init, TermyCore, cell layout, themes, glyph plans
  web/src/
    terminal.ts             Terminal: lifecycle, options, events, input, selection, links
    renderer/               canvas.ts (rows), images.ts (kitty), glyphs.ts, metrics.ts
    theme.ts, options.ts, selection.ts, links.ts, input/keys.ts
  xterm/src/                terminal.ts (adapter), buffer.ts, addons.ts, options.ts, types.ts
  demo/                     side-by-side web/xterm demo page
```

## Development

Requires Rust with `wasm32-unknown-unknown`, `wasm-bindgen-cli` 0.2.128
(matching `crates/wasm/Cargo.toml`), Bun, and optionally `wasm-opt`.

```sh
cd packages
bun install
bun run build        # wasm + all three packages
bun run typecheck
bun run test
cargo test -p termy_wasm
python3 -m http.server -d . 4719   # then open http://localhost:4719/demo/
```

## Releasing

Publishing is manual and independent of desktop releases: run
**Publish npm packages** (`.github/workflows/npm-publish.yml`) from the Actions
tab with a version and dist-tag. It sets all three package versions, builds
with `wasm-opt`, runs the tests and publishes with provenance using the
`NPM_TOKEN` secret. Use `dry_run` to check the tarballs first.

See [program status](../docs/program-status.md) for OSC 7501 record semantics.
Web hosts call `term.processExited()` when the attached process exits to expire
working/blocked records while preserving completed results.
