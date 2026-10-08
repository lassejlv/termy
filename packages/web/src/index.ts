export {
  Terminal,
  type ClipboardEvent,
  type CustomKeyEventHandler,
  type KeyEvent,
  type ProgressEvent,
  type TerminalSize,
} from './terminal.ts'
export {
  DEFAULT_FONT_FAMILY,
  DEFAULT_OPTIONS,
  type CursorInactiveStyle,
  type CursorStyle,
  type FontWeight,
  type LinkHandler,
  type ResolvedOptions,
  type ScrollModifier,
  type TerminalOptions,
} from './options.ts'
export {
  DEFAULT_THEME,
  parseColor,
  resolveTheme,
  type ResolvedTheme,
  type Rgba,
  type Theme,
  type ThemeInput,
} from './theme.ts'
export { Emitter, toDisposable, type IDisposable, type IEvent } from './emitter.ts'
export type { BufferPoint, SelectionRange } from './selection.ts'
export { translateKey, type TermyKey } from './input/keys.ts'
export { builtinTheme, builtinThemeIds, init, initSync, isInitialized, type WasmSource } from '@termysh/core'

export type { ProgramStatusRecord } from '@termysh/core'
