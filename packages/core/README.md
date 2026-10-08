# @termysh/core

Termy's terminal engine compiled to WebAssembly, with a typed headless API.
It parses output, keeps the grid and scrollback, encodes keys, mouse and paste
for the program, and reports titles, working directories and other OSC
metadata. It runs in browsers, web workers, Node and Bun, and has no DOM and no
I/O.

For a ready-made browser terminal, use
[`@termysh/web`](https://www.npmjs.com/package/@termysh/web). To replace
xterm.js, use [`@termysh/xterm`](https://www.npmjs.com/package/@termysh/xterm).
Use this package when you render yourself, run in a worker or on a server, or
test what a program draws.

Pre-1.0: pin a version and test upgrades.

## Install

```sh
npm install @termysh/core
```

## Quick start

```ts
import { init, TermyCore } from '@termysh/core'

await init() // loads termy.wasm once; required before anything else
const term = new TermyCore({ cols: 80, rows: 24, scrollback: 1000 })

term.write('\x1b[1;32mready\x1b[0m\r\n$ ')
term.lineText(0) // 'ready'
term.cursor()    // { row: 1, col: 2, visible: true, shape: 'block', blinking: false }
term.dispose()   // frees wasm memory
```

In Node and Bun, `init()` reads `termy.wasm` from disk. In a browser it is
resolved with `new URL('./termy.wasm', import.meta.url)`; pass a URL, a
`Response`, bytes or a compiled `WebAssembly.Module` to load it from elsewhere.
In a worker, `initSync(bytes)` loads it synchronously.

## Driving a program

Write what the program prints, and send three things back to it: encoded
input, protocol replies, and size changes.

```ts
import { Modifier } from '@termysh/core'

pty.onData((bytes) => {
  term.write(bytes)                  // string or Uint8Array
  const replies = term.takeReplies() // cursor reports, device attributes, color queries
  if (replies.length) pty.write(replies)
})

pty.write(term.encodeKey('up', undefined, 0)!)         // mode-aware: '\x1b[A' or '\x1bOA'
pty.write(term.encodeKey('c', 'c', Modifier.Ctrl)!)    // '\x03'
pty.write(term.encodePaste(text))                      // bracketed when the program enabled it

term.resize(120, 40)
pty.resize(120, 40)
```

Encoders return `Uint8Array` (or `undefined` when there is nothing to send).
`encodeMouse(kind, button, col, row, modifiers)` returns `undefined` unless
the program tracks the mouse.

## Reading the screen

```ts
term.lineText(0)               // first live-screen line; negative numbers are scrollback
term.lineText(-1)              // newest scrollback line
term.historySize               // scrollback lines
term.modes()                   // { alternateScreen, bracketedPaste, mouseTracking, ... }

const damage = term.takeDamage()          // { full, scrolls, spans } since the last call
const rows = term.readRows(0, term.rows)  // flat cell data for the viewport
rows.text(0, 0)                           // grapheme at row 0, col 0
```

Each cell is six `u32` slots (`Slot.Text`, `Foreground`, `Background`,
`UnderlineColor`, `Style`, `Link`). Use `decodeColor`, `attributes`,
`underlineStyle` and `widthFlags` to read them. Reads are copies, so they stay
valid after later writes.

## Events

```ts
for (const event of term.takeEvents()) {
  // event.type: 'title' | 'resetTitle' | 'bell' | 'cwd' | 'progress'
  //           | 'programStatus' | 'clipboard' | 'shellIntegration'
}
```

Call `term.processExited()` when the program exits.

To run the program itself from Node.js or Bun, use
[`@termysh/pty`](https://www.npmjs.com/package/@termysh/pty) and feed its
output to `write`; its README shows the full loop.

## Testing terminal output

```ts
import { init, TermyCore } from '@termysh/core'
import { expect, test } from 'vitest'

await init()

test('renders a progress line', () => {
  const term = new TermyCore({ cols: 40, rows: 5 })
  term.write('Downloading...\r\x1b[2KDone\r\n')
  expect(term.lineText(0)).toBe('Done')
  term.dispose()
})
```

## Documentation

- [Headless core guide](https://termy.sh/docs/developer/web/core): rendering
  loop, input, images and glyphs
- [Frameworks and bundlers](https://termy.sh/docs/developer/web/frameworks):
  loading `termy.wasm` with Vite, webpack, esbuild and Next.js
- [Troubleshooting](https://termy.sh/docs/developer/web/troubleshooting)

Every docs page is also available as Markdown by adding `.md` to its URL, and
[`llms-full.txt`](https://termy.sh/llms-full.txt) has all of them in one file.
The type declarations document every method.

## License

MIT
