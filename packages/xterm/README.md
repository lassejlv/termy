# @termysh/xterm

The xterm.js `Terminal` API on Termy's WebAssembly terminal engine. In most
apps, migrating from `@xterm/xterm` means changing imports.

Pre-1.0: pin a version and test upgrades.

## Migrate

```sh
npm uninstall @xterm/xterm @xterm/addon-fit @xterm/addon-web-links @xterm/addon-search
npm install @termysh/xterm
```

```diff
-import { Terminal } from '@xterm/xterm'
-import { FitAddon } from '@xterm/addon-fit'
-import { WebLinksAddon } from '@xterm/addon-web-links'
-import '@xterm/xterm/css/xterm.css'
+import { Terminal, FitAddon, WebLinksAddon } from '@termysh/xterm'

 const term = new Terminal({ cursorBlink: true, fontSize: 13 })
 const fit = new FitAddon()
 term.loadAddon(fit)
 term.loadAddon(new WebLinksAddon())
 term.open(container)
 fit.fit()
 term.onData((data) => socket.send(data))
```

No stylesheet is needed. An unthemed terminal uses xterm.js's default colors,
so it looks the same after the switch.

## Addons

| xterm.js addon | With @termysh/xterm |
| --- | --- |
| `@xterm/addon-fit` | `FitAddon` (same API) |
| `@xterm/addon-web-links` | `WebLinksAddon` (same API) |
| `@xterm/addon-search` | `SearchAddon`: `findNext`, `findPrevious`, `onDidChangeResults` |
| `@xterm/addon-webgl`, `@xterm/addon-canvas` | `WebglAddon`, `CanvasAddon`: no-ops, Termy has its own renderer |
| `@xterm/addon-unicode11` | `Unicode11Addon`: no-op, the engine measures widths |
| `@xterm/addon-image` | Not needed for kitty graphics, which are built in. Sixel is not supported yet |

Third-party addons that reach into xterm.js internals (`terminal._core`) do
not work.

## Differences from xterm.js

- The WebAssembly engine loads in the background. Writes made before it is
  ready are queued; `await term.ready` when you need the engine immediately.
- Not supported yet: `registerDecoration`, custom `registerLinkProvider`
  providers and character joiners (accepted, do nothing).
- There is no `term.parser`, so `parser.register*Handler` hooks are not
  available. For common OSC sequences, use the events on `term.termy`
  (`onCwdChange`, `onProgress`, `onClipboard`, `onShellIntegration`).
- `buffer.normal` cannot read the main screen while a full-screen app uses the
  alternate screen.

## Termy extras

| Option | Description |
| --- | --- |
| `termyTheme` | Start from a bundled Termy theme (`'tokyo-night'`, `'dracula'`, ...); `theme` then overrides individual colors |
| `images` | Render kitty graphics (default `true`) |
| `allowClipboardWrite` | Let OSC 52 write the clipboard |
| `autoFit` | Follow the container without `FitAddon` (default `false`, as in xterm.js) |
| `padding`, `copyOnSelect` | As in `@termysh/web` |

`term.termy` is the underlying [`@termysh/web`](https://www.npmjs.com/package/@termysh/web)
terminal.

## Loading the wasm

`termy.wasm` ships in `@termysh/core`. Vite 8 and Next.js with Turbopack need
no setup. Vite 7 and earlier need `optimizeDeps: { exclude: ['@termysh/core'] }`,
and webpack 5 needs
`new webpack.IgnorePlugin({ resourceRegExp: /^node:fs\/promises$/ })` in the
browser build. See
[Frameworks and bundlers](https://termy.sh/docs/developer/web/frameworks).

## Documentation

- [Migrate from xterm.js](https://termy.sh/docs/developer/web/xterm)
- [Connect to a shell](https://termy.sh/docs/developer/web/connect)
- [Troubleshooting](https://termy.sh/docs/developer/web/troubleshooting)

Every docs page is also available as Markdown by adding `.md` to its URL, and
[`llms-full.txt`](https://termy.sh/llms-full.txt) has all of them in one file.

## License

MIT
