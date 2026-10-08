# @termysh/web

A browser terminal on Termy's WebAssembly engine. The parser, grid, keyboard
and mouse encoding, kitty keyboard and graphics protocols, box-drawing
geometry and themes are the same Rust code the
[Termy](https://termy.sh) desktop app runs.

- Damage-driven canvas rendering
- Kitty keyboard and graphics protocols, OSC 8 links, OSC 52 clipboard
- Selection, IME, scrollback, themes, and options that change at runtime
- No I/O and no stylesheet: you connect it to any transport

Pre-1.0: pin a version and test upgrades.

## Install

```sh
npm install @termysh/web
```

## Quick start

```html
<div id="terminal" style="height: 400px"></div>
```

```ts
import { Terminal } from '@termysh/web'

const term = new Terminal({ theme: 'tokyo-night', fontSize: 13 })
term.open(document.getElementById('terminal')!)
term.write('Hello from \x1b[1;32mTermy\x1b[0m\r\n')
```

## Connect to a shell

Run the shell in a PTY on a server, send its output to `write`, and send
`onData` and `onResize` back:

```ts
const socket = new WebSocket('wss://example.com/pty')
socket.binaryType = 'arraybuffer'

socket.onmessage = (event) => term.write(new Uint8Array(event.data)) // server sends output as binary frames
term.onData((data) => socket.send(JSON.stringify({ type: 'input', data })))
term.onResize(({ cols, rows }) => socket.send(JSON.stringify({ type: 'resize', cols, rows })))
```

Run the PTY with [`@termysh/pty`](https://www.npmjs.com/package/@termysh/pty)
(a drop-in `node-pty` replacement). A complete Node server with `ws` is in
[Connect to a shell](https://termy.sh/docs/developer/web/connect).

## Essentials

1. **The parent needs a size.** The terminal fills the element passed to
   `open`. A parent with no height renders nothing.
2. **The constructor is synchronous; the engine is not.** Calls made before
   the wasm module loads are queued. `await term.ready`, or use
   `await Terminal.create(options)`, before reading state like `getLine` or
   `cursor`.
3. **Send everything from `onData` to the host.** It includes protocol
   replies (cursor reports, device attributes) that programs wait for.
4. **Send every `onResize` to the host**, so the PTY's size matches.
5. **Call `term.dispose()`** when you are done, for example in a React effect
   cleanup.
6. **Write raw output.** Do not parse escape sequences; use events such as
   `onTitleChange` and `onCwdChange` for metadata.

## Loading the wasm

`termy.wasm` ships in `@termysh/core` and is loaded with
`new URL('./termy.wasm', import.meta.url)`.

| Bundler | Setup |
| --- | --- |
| Vite 8, Next.js with Turbopack | None |
| Vite 7 and earlier | `optimizeDeps: { exclude: ['@termysh/core'] }` |
| webpack 5, Next.js with `--webpack` | `new webpack.IgnorePlugin({ resourceRegExp: /^node:fs\/promises$/ })` in the browser build |
| esbuild | `--format=esm --external:node:fs/promises`, then copy `termy.wasm` next to the bundle |

Or serve the file yourself: copy
`node_modules/@termysh/core/dist/termy.wasm` to your static assets and pass
`new Terminal({ wasm: '/assets/termy.wasm' })`.
Details: [Frameworks and bundlers](https://termy.sh/docs/developer/web/frameworks).

## API at a glance

```ts
term.open(parent) / term.dispose()
term.write(data, callback?) / term.writeln(data)   // string or Uint8Array
term.paste(text) / term.input(text)
term.resize(cols, rows) / term.fit()
term.setOptions({ fontSize: 15, theme: 'nord' })
term.focus() / term.blur() / term.clear() / term.reset()
term.scrollLines(n) / term.scrollToBottom()
term.select(col, line, length) / term.getSelection()
term.getLine(line) / term.cursor / term.historySize / term.cols / term.rows
term.processExited()                                // the host process ended
```

Events return a disposable: `onData`, `onBytes`, `onBinary`, `onResize`,
`onTitleChange`, `onBell`, `onCwdChange`, `onProgress`, `onProgramStatus`,
`onClipboard`, `onShellIntegration`, `onKey`, `onScroll`, `onRender`,
`onSelectionChange`, `onCursorMove`, `onWriteParsed`, `onFocus`, `onBlur`.

Themes: a bundled id (`termy`, `tokyo-night`, `catppuccin-mocha`, `dracula`,
`nord`, ...), an xterm.js `ITheme` object, or `{ extends: 'nord', ...overrides }`.

## Documentation

- [Web terminal overview](https://termy.sh/docs/developer/web)
- [Connect to a shell](https://termy.sh/docs/developer/web/connect)
- [Frameworks and bundlers](https://termy.sh/docs/developer/web/frameworks)
- [Options](https://termy.sh/docs/developer/web/options) and
  [themes](https://termy.sh/docs/developer/web/themes)
- [Troubleshooting](https://termy.sh/docs/developer/web/troubleshooting)

Every docs page is also available as Markdown by adding `.md` to its URL, and
[`llms-full.txt`](https://termy.sh/llms-full.txt) has all of them in one file.

Related packages: [`@termysh/xterm`](https://www.npmjs.com/package/@termysh/xterm)
(xterm.js-compatible API),
[`@termysh/core`](https://www.npmjs.com/package/@termysh/core) (headless
engine) and [`@termysh/pty`](https://www.npmjs.com/package/@termysh/pty)
(server-side pseudo-terminals).

## License

MIT
