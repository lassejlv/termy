import { loadNative } from './native.ts'
import { Terminal } from './terminal.ts'
import type { IPty, IPtyForkOptions, IWindowsPtyForkOptions } from './types.ts'

export type {
  IBasePtyForkOptions,
  IDisposable,
  IEvent,
  IExitEvent,
  IPty,
  IPtyForkOptions,
  IWindowsPtyForkOptions,
} from './types.ts'
export { Terminal } from './terminal.ts'

/**
 * Start `file` on a new pseudo-terminal.
 *
 * ```ts
 * import { spawn } from '@termysh/pty'
 *
 * const shell = spawn(process.env.SHELL ?? 'bash', [], { name: 'xterm-256color', cols: 80, rows: 24 })
 * shell.onData((data) => process.stdout.write(data))
 * shell.onExit(({ exitCode }) => console.log('exited', exitCode))
 * shell.write('ls\r')
 * ```
 *
 * Throws when the program cannot be started (not found, not executable, or
 * `cwd` does not exist).
 */
export function spawn(
  file: string,
  args: string[] | string = [],
  options: IPtyForkOptions | IWindowsPtyForkOptions = {},
): IPty {
  return new Terminal(file, args, options)
}

/** @deprecated Alias of `spawn`, kept for node-pty compatibility. */
export const fork: typeof spawn = spawn
/** @deprecated Alias of `spawn`, kept for node-pty compatibility. */
export const createTerminal: typeof spawn = spawn

/** Whether this machine can create pseudo-terminals (Windows needs 10 1809 or later). */
export function isSupported(): boolean {
  return loadNative().available()
}

const pty: {
  spawn: typeof spawn
  fork: typeof spawn
  createTerminal: typeof spawn
  isSupported: typeof isSupported
} = { spawn, fork, createTerminal, isSupported }

/** For `import pty from '@termysh/pty'`, as node-pty is often imported. */
export default pty
