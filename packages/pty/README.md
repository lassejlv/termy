# @termysh/pty

Pseudo-terminals for Node.js and Bun, on the native PTY layer the
[Termy](https://termy.sh) terminal app uses for its own sessions. It is a
drop-in replacement for [`node-pty`](https://github.com/microsoft/node-pty):
same `spawn` API, same events, same types.

- **Prebuilt binaries**, no `node-gyp`, no compiler, no install scripts:
  macOS (arm64, x64), Linux glibc 2.17+ and musl (x64, arm64), Windows 10
  1809+ (x64, arm64).
- **Backpressure.** Output waits for JavaScript instead of piling up in
  memory, and `pause()` really stops reading, so a fast program blocks rather
  than flooding your process.
- **Ordered exit.** `onExit` fires once, after the last byte of output, with
  the exit code and the terminating signal.
- **Node.js 18+ and Bun**, ESM and CommonJS.

Pre-1.0: pin a version and test upgrades.

## Install

```sh
npm install @termysh/pty
```

npm installs the binary for your platform as an optional dependency
(`@termysh/pty-<platform>`). Installing with `--omit=optional` or
`--no-optional` skips it.

## Usage

```ts
import { spawn } from '@termysh/pty'

const shell = spawn(process.env.SHELL ?? 'bash', [], {
  name: 'xterm-256color',
  cols: 80,
  rows: 24,
  cwd: process.env.HOME,
  env: { ...process.env, COLORTERM: 'truecolor' },
})

shell.onData((data) => process.stdout.write(data))
shell.onExit(({ exitCode, signal }) => console.log(`exited: ${exitCode} (signal ${signal})`))

shell.write('echo hello\r')
shell.resize(120, 40)
```

## Migrating from node-pty

```sh
npm uninstall node-pty
npm install @termysh/pty
```

```diff
-import * as pty from 'node-pty'
+import * as pty from '@termysh/pty'
```

`spawn`, `fork`, `createTerminal`, the default export, `IPty`,
`IPtyForkOptions`, `IWindowsPtyForkOptions`, `IDisposable` and `IEvent` match
node-pty. Differences:

- `uid` and `gid` are not supported and throw. Run the parent process as the
  target user instead.
- `open()` (a PTY pair without a process) is not provided.
- Windows always uses ConPTY. winpty is not supported, so `useConpty`,
  `useConptyDll` and `conptyInheritCursor` are accepted and ignored.
- `write` after the program has exited is dropped silently, and more than
  8 MiB of unwritten input throws.

## API

```ts
spawn(file: string, args?: string[] | string, options?: IPtyForkOptions | IWindowsPtyForkOptions): IPty
isSupported(): boolean // false on Windows before 10 1809
```

`spawn` throws if the program can't start (not found, not executable, or
`cwd` missing). On Windows, `args` may be a single command-line string.

### Options

| Option | Default | Description |
| --- | --- | --- |
| `name` | `env.TERM`, then `xterm` | Exported as `TERM` on Unix |
| `cols`, `rows` | `80`, `24` | Initial size |
| `cwd` | `process.cwd()` | Working directory; also exported as `PWD` on Unix |
| `env` | `process.env` | The child's whole environment. When defaulted, `TMUX`, `TMUX_PANE`, `STY`, `WINDOW`, `WINDOWID`, `TERMCAP`, `COLUMNS` and `LINES` are removed |
| `encoding` | `'utf8'` | Decode output to strings. `null` emits `Buffer`s |
| `handleFlowControl` | `false` | Writes of `flowControlPause` (`\x13`) and `flowControlResume` (`\x11`) pause and resume output |

### IPty

| Member | Description |
| --- | --- |
| `pid`, `cols`, `rows` | Process id and current size |
| `process` | Foreground program, such as `vim` while it runs in a shell. The spawned file name on Windows |
| `onData(listener)` | Output. Returns a disposable |
| `onExit(listener)` | `{ exitCode, signal }` once, after all output. `signal` is `0` for a normal exit and absent on Windows |
| `write(data)` | Input, as a string (UTF-8) or `Buffer` |
| `resize(cols, rows, pixelSize?)` | Resize; `pixelSize` is `{ width, height }` of the whole terminal |
| `kill(signal?)` | Signal the program, `SIGHUP` by default. Windows terminates it and throws if given a signal |
| `pause()`, `resume()` | Stop and restart reading output |
| `on('data' \| 'exit', listener)` | EventEmitter-style listeners, as in node-pty |

## With a terminal

Pair it with [`@termysh/web`](https://www.npmjs.com/package/@termysh/web) in
the browser, or read the screen on the server with
[`@termysh/core`](https://www.npmjs.com/package/@termysh/core):

```ts
import { init, TermyCore } from '@termysh/core'
import { spawn } from '@termysh/pty'

await init()
const screen = new TermyCore({ cols: 80, rows: 24 })
const app = spawn('htop', [], { name: 'xterm-256color', cols: 80, rows: 24, encoding: null })
app.onData((bytes) => {
  screen.write(bytes)
  const replies = screen.takeReplies() // answer the program's terminal queries
  if (replies.length) app.write(Buffer.from(replies))
})
```

A complete WebSocket server and browser client is in
[Connect to a shell](https://termy.sh/docs/developer/web/connect).

## Bundling

The addon is a native `.node` file, so keep `@termysh/pty` external to
bundlers (esbuild `--external:@termysh/pty`, webpack `externals`). In
Electron, unpack it from the asar archive (`asarUnpack: ['**/*.node']`). To
load a binary from a custom location, set `TERMY_PTY_NATIVE` to its absolute
path.

## Documentation

- [PTY for Node.js](https://termy.sh/docs/developer/web/pty)
- [Connect to a shell](https://termy.sh/docs/developer/web/connect)

Every docs page is also available as Markdown by adding `.md` to its URL, and
[`llms-full.txt`](https://termy.sh/llms-full.txt) has all of them in one file.

## License

MIT
