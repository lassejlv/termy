// Public types. They match node-pty's (`node-pty.d.ts`) so code written for
// node-pty type-checks unchanged.

export interface IDisposable {
  dispose(): void
}

/** Subscribe with a listener; dispose the result to unsubscribe. */
export interface IEvent<T> {
  (listener: (event: T) => unknown): IDisposable
}

export interface IExitEvent {
  /** Exit code, or `0` when a signal ended the process. */
  exitCode: number
  /** The terminating signal number on Unix (`0` for a normal exit). Absent on Windows. */
  signal?: number
}

export interface IBasePtyForkOptions {
  /** Terminal name, exported as `TERM` on Unix. Defaults to `env.TERM`, then `xterm`. */
  name?: string
  /** Columns. Default 80. */
  cols?: number
  /** Rows. Default 24. */
  rows?: number
  /** Working directory. Default `process.cwd()`. */
  cwd?: string
  /** The child's whole environment. Default `process.env`, minus multiplexer variables. */
  env?: { [key: string]: string | undefined }
  /**
   * Decode output with this encoding and emit strings (default `utf8`). With
   * `null`, `onData` receives `Buffer`s.
   */
  encoding?: BufferEncoding | null
  /** Treat writes of `flowControlPause` / `flowControlResume` as pause and resume. */
  handleFlowControl?: boolean
  /** Default `\x13` (XOFF). */
  flowControlPause?: string
  /** Default `\x11` (XON). */
  flowControlResume?: string
}

export interface IPtyForkOptions extends IBasePtyForkOptions {
  /** Not supported: throws. Run the parent as the target user instead. */
  uid?: number
  /** Not supported: throws. */
  gid?: number
}

export interface IWindowsPtyForkOptions extends IBasePtyForkOptions {
  /** Accepted for compatibility. ConPTY is always used; winpty is not supported. */
  useConpty?: boolean
  /** Accepted for compatibility and ignored. */
  useConptyDll?: boolean
  /** Accepted for compatibility and ignored. */
  conptyInheritCursor?: boolean
}

export interface IPty {
  /** Process id of the program. */
  readonly pid: number
  readonly cols: number
  readonly rows: number
  /**
   * The program in the foreground of the terminal, such as `vim` while it
   * runs in a shell. The spawned file name on Windows.
   */
  readonly process: string
  /** Whether writes of the flow-control strings pause and resume output. */
  handleFlowControl: boolean
  /** Output from the program. A `Buffer` when spawned with `encoding: null`. */
  readonly onData: IEvent<string>
  /** Fires once, after all output. */
  readonly onExit: IEvent<IExitEvent>
  /** Resize the terminal. `pixelSize` is the whole terminal's size in pixels. */
  resize(columns: number, rows: number, pixelSize?: { width: number; height: number }): void
  /** Windows only in node-pty; does nothing here. */
  clear(): void
  /** Send input to the program. Input after the program exits is dropped. */
  write(data: string | Buffer): void
  /** Signal the program (`SIGHUP` by default). Windows terminates it and does not accept a signal. */
  kill(signal?: string): void
  /** Stop reading output. The program blocks once the terminal's buffer fills. */
  pause(): void
  /** Continue reading output after `pause`. */
  resume(): void
}
