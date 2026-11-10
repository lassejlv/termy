import { createRequire } from 'node:module'
import { fileURLToPath } from 'node:url'
import { binaryName, platformKey, platforms } from '../platforms.mjs'

export { platformKey } from '../platforms.mjs'

/** Event codes passed to the native callback. */
export const EVENT_DATA = 0
export const EVENT_EXIT = 1
/** A flush with nothing to deliver; ignored. */
export const EVENT_NONE = 2

export interface NativeSpawnOptions {
  file: string
  args: string[]
  cwd?: string
  /** Flattened `[name, value, ...]`: the child's whole environment. */
  env: string[]
  cols: number
  rows: number
}

export type NativeCallback = (
  event: number,
  data: Buffer | null,
  exitCode: number | null,
  signal: number | null,
) => void

export interface NativePty {
  readonly pid: number
  readonly process: string | null
  /** False, without queueing, while the native input backlog is full. */
  write(data: Buffer): boolean
  resize(cols: number, rows: number, pixelWidth?: number, pixelHeight?: number): void
  kill(signal?: number): void
  pause(): void
  resume(): void
  destroy(): void
}

export interface NativeBinding {
  NativePty: new (options: NativeSpawnOptions, callback: NativeCallback) => NativePty
  available(): boolean
}

const PACKAGE = '@termysh/pty'

let binding: NativeBinding | undefined

/**
 * Load the native addon. Search order: `TERMY_PTY_NATIVE` (an absolute path
 * to a `.node` file), a development build next to the package, then the
 * `@termysh/pty-<key>` package npm installed as an optional dependency.
 */
export function loadNative(): NativeBinding {
  if (binding) return binding
  const require = createRequire(import.meta.url)
  const key = platformKey()
  const file = binaryName(key)
  const candidates = [
    process.env.TERMY_PTY_NATIVE,
    fileURLToPath(new URL(`../${file}`, import.meta.url)),
    `${PACKAGE}-${key}`,
  ].filter((candidate): candidate is string => Boolean(candidate))

  const errors: string[] = []
  for (const candidate of candidates) {
    try {
      binding = require(candidate) as NativeBinding
      return binding
    } catch (error) {
      const code = (error as { code?: string }).code
      if (code !== 'MODULE_NOT_FOUND' && code !== 'ERR_MODULE_NOT_FOUND') {
        errors.push(`${candidate}: ${(error as Error).message}`)
      }
    }
  }

  const details = errors.length ? `\n${errors.join('\n')}` : ''
  throw new Error(
    `${PACKAGE}: no native binary for ${key}. Install with optional dependencies enabled ` +
      `(npm installs ${PACKAGE}-${key} automatically; --omit=optional or --no-optional skips it). ` +
      `Supported: ${platforms.map(({ key }) => key).join(', ')}.${details}`,
  )
}
