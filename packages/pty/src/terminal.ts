import { EventEmitter } from 'node:events'
import { constants } from 'node:os'
import { basename } from 'node:path'
import { StringDecoder } from 'node:string_decoder'
import { EVENT_DATA, type NativePty, loadNative } from './native.ts'
import type {
  IDisposable,
  IEvent,
  IExitEvent,
  IPty,
  IPtyForkOptions,
  IWindowsPtyForkOptions,
} from './types.ts'

const DEFAULT_COLS = 80
const DEFAULT_ROWS = 24
const DEFAULT_NAME = 'xterm'
const isWindows = process.platform === 'win32'

// EventEmitter's listener signature.
type Listener = (...args: any[]) => void

// Variables that describe the parent's own terminal or multiplexer session and
// would mislead programs in the new one. node-pty removes the same set.
const INHERITED_SESSION_VARIABLES = ['TMUX', 'TMUX_PANE', 'STY', 'WINDOW', 'WINDOWID', 'TERMCAP', 'COLUMNS', 'LINES']

/** A program running on a pseudo-terminal. Create one with `spawn`. */
export class Terminal implements IPty {
  readonly pid: number
  handleFlowControl: boolean

  #native: NativePty
  #file: string
  #cols: number
  #rows: number
  #events = new EventEmitter()
  #decoder: StringDecoder | undefined
  #flowControlPause: string
  #flowControlResume: string
  #exited = false

  readonly onData: IEvent<string> = (listener) => this.#subscribe('data', listener)
  readonly onExit: IEvent<IExitEvent> = (listener) => this.#subscribe('exit', listener)

  constructor(file: string, args: string[] | string, options: IPtyForkOptions | IWindowsPtyForkOptions = {}) {
    if ('uid' in options && options.uid !== undefined) unsupported('uid')
    if ('gid' in options && options.gid !== undefined) unsupported('gid')
    this.#cols = dimension(options.cols ?? DEFAULT_COLS, 'cols')
    this.#rows = dimension(options.rows ?? DEFAULT_ROWS, 'rows')
    this.#file = file
    const encoding = options.encoding === undefined ? 'utf8' : options.encoding
    this.#decoder = encoding === null ? undefined : new StringDecoder(encoding)
    this.handleFlowControl = options.handleFlowControl ?? false
    this.#flowControlPause = options.flowControlPause ?? '\x13'
    this.#flowControlResume = options.flowControlResume ?? '\x11'

    const cwd = options.cwd ?? process.cwd()
    const { NativePty } = loadNative()
    this.#native = new NativePty(
      {
        file,
        args: typeof args === 'string' ? parseCommandLine(args) : [...args],
        cwd,
        env: childEnvironment(options, cwd),
        cols: this.#cols,
        rows: this.#rows,
      },
      (event, data, exitCode, signal) => {
        if (event === EVENT_DATA) this.#emitData(data!)
        else this.#emitExit(exitCode, signal)
      },
    )
    this.pid = this.#native.pid
  }

  get cols(): number {
    return this.#cols
  }

  get rows(): number {
    return this.#rows
  }

  get process(): string {
    if (this.#exited) return basename(this.#file)
    return this.#native.process ?? basename(this.#file)
  }

  write(data: string | Buffer): void {
    if (this.handleFlowControl && typeof data === 'string') {
      if (data === this.#flowControlPause) return this.pause()
      if (data === this.#flowControlResume) return this.resume()
    }
    if (this.#exited) return
    this.#native.write(typeof data === 'string' ? Buffer.from(data, 'utf8') : data)
  }

  resize(columns: number, rows: number, pixelSize?: { width: number; height: number }): void {
    const cols = dimension(columns, 'cols')
    const height = dimension(rows, 'rows')
    this.#cols = cols
    this.#rows = height
    if (this.#exited) return
    this.#native.resize(cols, height, pixelSize?.width, pixelSize?.height)
  }

  clear(): void {}

  kill(signal?: string): void {
    if (isWindows && signal !== undefined) throw new Error('Signals not supported on windows.')
    if (this.#exited) return
    this.#native.kill(signal === undefined ? undefined : signalNumber(signal))
  }

  pause(): void {
    this.#native.pause()
  }

  resume(): void {
    this.#native.resume()
  }

  /** node-pty's EventEmitter-style subscription: `'data'` or `'exit'`. */
  on(event: 'data', listener: (data: string) => void): this
  on(event: 'exit', listener: (exitCode: number, signal?: number) => void): this
  on(event: 'data' | 'exit', listener: Listener): this {
    this.#events.on(event === 'exit' ? 'exit-legacy' : event, listener)
    return this
  }

  addListener(event: 'data' | 'exit', listener: Listener): this {
    this.#events.on(event === 'exit' ? 'exit-legacy' : event, listener)
    return this
  }

  once(event: 'data' | 'exit', listener: Listener): this {
    this.#events.once(event === 'exit' ? 'exit-legacy' : event, listener)
    return this
  }

  off(event: 'data' | 'exit', listener: Listener): this {
    this.#events.off(event === 'exit' ? 'exit-legacy' : event, listener)
    return this
  }

  removeListener(event: 'data' | 'exit', listener: Listener): this {
    return this.off(event, listener)
  }

  removeAllListeners(event?: 'data' | 'exit'): this {
    if (event === undefined) {
      for (const name of ['data', 'exit', 'exit-legacy']) this.#events.removeAllListeners(name)
    } else {
      this.#events.removeAllListeners(event === 'exit' ? 'exit-legacy' : event)
      if (event === 'exit') this.#events.removeAllListeners('exit')
    }
    return this
  }

  #subscribe<T>(event: string, listener: (value: T) => unknown): IDisposable {
    this.#events.on(event, listener)
    return { dispose: () => this.#events.off(event, listener) }
  }

  #emitData(chunk: Buffer): void {
    if (!this.#decoder) {
      this.#events.emit('data', chunk)
      return
    }
    const text = this.#decoder.write(chunk)
    if (text) this.#events.emit('data', text)
  }

  #emitExit(exitCode: number | null, signal: number | null): void {
    const rest = this.#decoder?.end()
    if (rest) this.#events.emit('data', rest)
    this.#exited = true
    this.#native.destroy()
    const event: IExitEvent = isWindows ? { exitCode: exitCode ?? 0 } : { exitCode: exitCode ?? 0, signal: signal ?? 0 }
    this.#events.emit('exit', event)
    this.#events.emit('exit-legacy', event.exitCode, event.signal)
  }
}

function childEnvironment(options: IPtyForkOptions | IWindowsPtyForkOptions, cwd: string): string[] {
  const inherited = options.env === undefined || options.env === process.env
  const env: Record<string, string> = {}
  for (const [name, value] of Object.entries(options.env ?? process.env)) {
    if (value !== undefined) env[name] = value
  }
  if (inherited) for (const name of INHERITED_SESSION_VARIABLES) delete env[name]
  if (!isWindows) {
    env.TERM = options.name || env.TERM || DEFAULT_NAME
    env.PWD = cwd
  }
  return Object.entries(env).flat()
}

function dimension(value: number, name: string): number {
  if (!Number.isInteger(value) || value <= 0 || value > 0xffff) {
    throw new Error(`${name} must be a positive integer up to 65535, got ${value}`)
  }
  return value
}

function signalNumber(signal: string | number): number {
  if (typeof signal === 'number') return signal
  const number = (constants.signals as Record<string, number | undefined>)[signal]
  if (number === undefined) throw new Error(`Unknown signal: ${signal}`)
  return number
}

function unsupported(option: string): never {
  throw new Error(`@termysh/pty does not support the ${option} option; run the parent process as that user instead`)
}

/**
 * Split a Windows command line into arguments with the rules of
 * `CommandLineToArgvW`, for node-pty's `spawn(file, 'raw command line')` form.
 * Arguments are quoted again when the process starts.
 */
export function parseCommandLine(commandLine: string): string[] {
  const args: string[] = []
  let current = ''
  let inQuotes = false
  let hasArgument = false
  let index = 0
  while (index < commandLine.length) {
    const char = commandLine[index]!
    if (char === '\\') {
      // Backslashes are literal unless they precede a quote: 2n then a quote
      // gives n backslashes and a delimiter, 2n+1 gives n and a literal quote.
      let slashes = 0
      while (commandLine[index] === '\\') {
        slashes++
        index++
      }
      if (commandLine[index] === '"') {
        current += '\\'.repeat(slashes >> 1)
        if (slashes % 2 === 1) {
          current += '"'
          index++
        }
      } else {
        current += '\\'.repeat(slashes)
      }
      hasArgument = true
    } else if (char === '"') {
      if (inQuotes && commandLine[index + 1] === '"') {
        current += '"'
        index += 2
      } else {
        inQuotes = !inQuotes
        index++
      }
      hasArgument = true
    } else if (!inQuotes && (char === ' ' || char === '\t')) {
      if (hasArgument) args.push(current)
      current = ''
      hasArgument = false
      index++
    } else {
      current += char
      hasArgument = true
      index++
    }
  }
  if (hasArgument) args.push(current)
  return args
}
