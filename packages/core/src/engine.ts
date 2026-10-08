import { CellRows } from './cells.ts'
import { assertInitialized } from './init.ts'
import { TermyEngine, cellStride, placementStride } from './wasm/termy_wasm.js'

export type CursorShape = 'block' | 'bar' | 'underline'

export interface CursorState {
  row: number
  col: number
  visible: boolean
  shape: CursorShape
  blinking: boolean
}

export interface TerminalModes {
  applicationCursor: boolean
  applicationKeypad: boolean
  bracketedPaste: boolean
  focusEvents: boolean
  mouseTracking: boolean
  synchronizedUpdate: boolean
  alternateScreen: boolean
  cursorVisible: boolean
  /** Active kitty keyboard protocol flags (`CSI > flags u`). */
  kittyKeyboardFlags: number
}

export interface ViewportScroll {
  top: number
  /** Exclusive. */
  bottom: number
  /** Positive moves rows up. */
  lines: number
}

export interface DirtySpan {
  row: number
  start: number
  /** Exclusive. */
  end: number
}

export interface Damage {
  full: boolean
  scrolls: ViewportScroll[]
  spans: DirtySpan[]
}

export type ProgressKind = 'clear' | 'progress' | 'error' | 'indeterminate' | 'warning'

/** OSC 7501 snapshot record. `app` includes nearest-ancestor inheritance. */
export interface ProgramStatusRecord {
  id: string
  state: 'idle' | 'working' | 'done' | 'blocked' | 'error'
  kind: 'permission' | 'question' | 'auth' | null
  progress: number | null
  app: string | null
  title: string | null
  msg: string | null
}

export type TermyEvent =
  | { type: 'programStatus'; records: ProgramStatusRecord[] }
  | { type: 'bell' }
  | { type: 'title'; title: string }
  | { type: 'resetTitle' }
  | { type: 'cwd'; cwd: string }
  | { type: 'shellIntegration'; payload: string }
  | { type: 'progress'; state: ProgressKind; value: number }
  | { type: 'clipboard'; selection: string; data: string }

export interface GraphicsPlacement {
  /** Index for `graphicsImage()` within the read that produced it. */
  index: number
  imageId: number
  placementId: number
  /** Changes when the image pixels (or animation frame) change. */
  imageGeneration: number
  imageWidth: number
  imageHeight: number
  viewportRow: number
  col: number
  colOffset: number
  sourceX: number
  sourceY: number
  sourceWidth: number
  sourceHeight: number
  displayCols: number | undefined
  displayRows: number | undefined
  occupiedCols: number
  occupiedRows: number
  clipTopRows: number
  clipBottomRows: number
  xOffset: number
  yOffset: number
  zIndex: number
  placementSerial: number
  /** Pixel box the image is clipped to, for the cell size given to `readGraphics`. */
  clip: PixelRect
  /** Where the source rectangle is drawn, before clipping. */
  draw: PixelRect
}

export interface PixelRect {
  left: number
  top: number
  width: number
  height: number
}

export interface GraphicsImage {
  format: 'rgba' | 'png'
  data: Uint8Array
}

export const Modifier = { Ctrl: 1, Alt: 2, Shift: 4, Meta: 8 } as const
export type KeyEventKind = 'press' | 'repeat' | 'release'
export type MouseEventKind =
  | 'press'
  | 'release'
  | 'drag'
  | 'move'
  | 'wheelUp'
  | 'wheelDown'
  | 'wheelLeft'
  | 'wheelRight'
export type MouseButton = 'left' | 'middle' | 'right'

export interface TermyCoreOptions {
  cols?: number
  rows?: number
  /** Scrollback lines kept in history. Default 1000. */
  scrollback?: number
}

const KEY_KINDS: Record<KeyEventKind, number> = { press: 0, repeat: 1, release: 2 }
const MOUSE_KINDS: Record<MouseEventKind, number> = {
  press: 0,
  release: 1,
  drag: 2,
  move: 3,
  wheelUp: 4,
  wheelDown: 5,
  wheelLeft: 6,
  wheelRight: 7,
}
const MOUSE_BUTTONS: Record<MouseButton, number> = { left: 0, middle: 1, right: 2 }
const CURSOR_SHAPES: readonly CursorShape[] = ['block', 'bar', 'underline']
const encoder = new TextEncoder()
const EMPTY = new Uint8Array(0)

/**
 * A headless terminal backed by Termy's Rust engine. Feed it output with
 * `write`, read cells and damage for rendering, and encode user input into
 * bytes for the host. It performs no I/O and has no DOM dependency.
 */
export class TermyCore {
  #engine: TermyEngine
  #disposed = false

  constructor(options: TermyCoreOptions = {}) {
    assertInitialized()
    this.#engine = new TermyEngine(options.cols ?? 80, options.rows ?? 24, options.scrollback ?? 1000)
  }

  get cols(): number {
    return this.#engine.cols()
  }

  get rows(): number {
    return this.#engine.rows()
  }

  /** Increments whenever output or a viewport change may alter a read. */
  get generation(): number {
    return this.#engine.generation()
  }

  write(data: string | Uint8Array): void {
    if (typeof data === 'string') this.#engine.feed_str(data)
    else this.#engine.feed(data)
  }

  resize(cols: number, rows: number): void {
    this.#engine.resize(cols, rows)
  }

  /** Cell size in CSS pixels, used for pixel size reports and image layout. */
  setCellPixels(width: number, height: number): void {
    this.#engine.set_cell_pixels(width, height)
  }

  setScrollback(lines: number): void {
    this.#engine.set_scrollback(lines)
  }

  /** Protocol replies (device attributes, cursor reports, color queries). */
  takeReplies(): Uint8Array {
    return this.#engine.take_replies()
  }

  /** Expire active OSC 7501 records when the host process exits. */
  processExited(): void {
    this.#engine.process_exited()
  }

  takeEvents(): TermyEvent[] {
    return this.#engine.take_events() as TermyEvent[]
  }

  modes(): TerminalModes {
    const bits = this.#engine.mode_bits()
    return {
      applicationCursor: (bits & 1) !== 0,
      applicationKeypad: (bits & 2) !== 0,
      bracketedPaste: (bits & 4) !== 0,
      focusEvents: (bits & 8) !== 0,
      mouseTracking: (bits & 16) !== 0,
      synchronizedUpdate: (bits & 32) !== 0,
      alternateScreen: (bits & 64) !== 0,
      cursorVisible: (bits & 128) !== 0,
      kittyKeyboardFlags: this.#engine.keyboard_flags(),
    }
  }

  cursor(): CursorState {
    const [row = 0, col = 0, visible = 1, shape = 0, blinking = 0] = this.#engine.cursor()
    return {
      row,
      col,
      visible: visible !== 0,
      shape: CURSOR_SHAPES[shape] ?? 'block',
      blinking: blinking !== 0,
    }
  }

  /** Default cursor shape until an application sets one with DECSCUSR. */
  setDefaultCursorShape(shape: CursorShape): void {
    this.#engine.set_default_cursor_shape(CURSOR_SHAPES.indexOf(shape))
  }

  takeDamage(): Damage {
    const raw = this.#engine.take_damage()
    const scrollCount = raw[1] ?? 0
    const scrolls: ViewportScroll[] = []
    let index = 2
    for (let i = 0; i < scrollCount; i++, index += 3) {
      scrolls.push({ top: raw[index]!, bottom: raw[index + 1]!, lines: raw[index + 2]! | 0 })
    }
    const spans: DirtySpan[] = []
    for (; index + 2 < raw.length; index += 3) {
      spans.push({ row: raw[index]!, start: raw[index + 1]!, end: raw[index + 2]! })
    }
    return { full: raw[0] === 1, scrolls, spans }
  }

  /** Cells for viewport rows `[start, end)`. */
  readRows(start = 0, end: number = this.rows): CellRows {
    const data = this.#engine.read_rows(start, end)
    const strings = this.#engine.read_strings()
    return new CellRows(data, this.cols, start, strings)
  }

  /** Cells of one buffer line: `0..rows` is the live screen, negative is scrollback. */
  readLine(line: number): CellRows {
    const data = this.#engine.read_line(line)
    return new CellRows(data, this.cols, line, this.#engine.read_strings())
  }

  /** Text of a buffer line: `0..rows` is the live screen, negative is scrollback. */
  lineText(line: number, trimEnd = true): string | undefined {
    return this.#engine.line_text(line, trimEnd)
  }

  lineTextRange(line: number, start: number, end: number, trimEnd = true): string | undefined {
    return this.#engine.line_text_range(line, start, end, trimEnd)
  }

  /** Whether `line` soft-wraps into the next one. */
  lineWrapped(line: number): boolean {
    return this.#engine.line_wrapped(line)
  }

  get historySize(): number {
    return this.#engine.history_size()
  }

  /** Lines scrolled back from the live screen. */
  get displayOffset(): number {
    return this.#engine.display_offset()
  }

  /** Positive deltas scroll back into history. Returns whether it moved. */
  scrollDisplay(delta: number): boolean {
    return this.#engine.scroll_display(delta)
  }

  scrollToBottom(): boolean {
    return this.#engine.scroll_to_bottom()
  }

  clearScrollback(): void {
    this.#engine.clear_scrollback()
  }

  /** Pack cold scrollback; call after output goes quiet. */
  compactHistory(): void {
    this.#engine.compact_history()
  }

  /** Milliseconds until a pending synchronized update (mode 2026) must flush, or -1. */
  syncDeadline(): number {
    return this.#engine.sync_deadline_ms()
  }

  flushSync(): boolean {
    return this.#engine.flush_sync()
  }

  /** OSC 4 palette overrides: 256 raw colors, 0 where the theme applies. */
  paletteOverrides(): Uint32Array {
    return this.#engine.palette_overrides()
  }

  /** OSC 10/11/12 overrides as raw colors: `[foreground, background, cursor]`. */
  dynamicColorOverrides(): Uint32Array {
    return this.#engine.dynamic_color_overrides()
  }

  get paletteRevision(): number {
    return this.#engine.palette_revision()
  }

  /** Colors reported to OSC 4/10/11/12 queries: `[fg, bg, cursor, ansi0..15]` as 0xRRGGBB. */
  setQueryColors(colors: ArrayLike<number>): void {
    this.#engine.set_query_colors(Uint32Array.from(colors))
  }

  /**
   * Encode a key with Termy's native encoder (legacy, application cursor and
   * the kitty keyboard protocol). `key` is a Termy key name such as `enter`,
   * `up`, `f5` or `a`; `text` is the character it produced.
   */
  encodeKey(
    key: string,
    text: string | undefined,
    modifiers: number,
    kind: KeyEventKind = 'press',
    optionAsAlt = false,
  ): Uint8Array | undefined {
    const bytes = this.#engine.encode_key(key, text, modifiers, KEY_KINDS[kind], optionAsAlt)
    return bytes ? new Uint8Array(bytes) : undefined
  }

  /** Encode a mouse report; undefined when the application is not tracking it. */
  encodeMouse(
    kind: MouseEventKind,
    button: MouseButton,
    col: number,
    row: number,
    modifiers: number,
  ): Uint8Array | undefined {
    const bytes = this.#engine.encode_mouse(MOUSE_KINDS[kind], MOUSE_BUTTONS[button], col, row, modifiers)
    return bytes ? new Uint8Array(bytes) : undefined
  }

  encodePaste(text: string, bracketed = true): Uint8Array {
    if (!bracketed) return encoder.encode(text.replace(/\r?\n/g, '\r'))
    return this.#engine.encode_paste(text)
  }

  encodeFocus(focused: boolean): Uint8Array {
    return this.#engine.encode_focus(focused) ?? EMPTY
  }

  /** Changes when kitty graphics placements or animation frames change. */
  get graphicsRevision(): number {
    return this.#engine.graphics_revision()
  }

  /** Visible kitty graphics placements, laid out for a cell size in CSS pixels. */
  readGraphics(cellWidth: number, cellHeight: number): GraphicsPlacement[] {
    const raw = this.#engine.read_graphics(cellWidth, cellHeight)
    const stride = placementStride()
    const placements: GraphicsPlacement[] = []
    for (let offset = 0, index = 0; offset + stride <= raw.length; offset += stride, index++) {
      const at = (slot: number): number => raw[offset + slot]!
      const optional = (slot: number): number | undefined => (at(slot) < 0 ? undefined : at(slot))
      placements.push({
        index,
        imageId: at(0),
        placementId: at(1),
        imageGeneration: at(2),
        imageWidth: at(3),
        imageHeight: at(4),
        viewportRow: at(5),
        col: at(6),
        colOffset: at(7),
        sourceX: at(8),
        sourceY: at(9),
        sourceWidth: at(10),
        sourceHeight: at(11),
        displayCols: optional(12),
        displayRows: optional(13),
        occupiedCols: at(14),
        occupiedRows: at(15),
        clipTopRows: at(16),
        clipBottomRows: at(17),
        xOffset: at(18),
        yOffset: at(19),
        zIndex: at(20),
        placementSerial: at(21),
        clip: { left: at(22), top: at(23), width: at(24), height: at(25) },
        draw: { left: at(26), top: at(27), width: at(28), height: at(29) },
      })
    }
    return placements
  }

  /** Pixels for a placement from the most recent `readGraphics()`. */
  graphicsImage(index: number): GraphicsImage | undefined {
    const data = this.#engine.graphics_image(index)
    if (!data) return undefined
    return { format: this.#engine.graphics_image_is_png(index) ? 'png' : 'rgba', data }
  }

  /** Milliseconds until the next visible animation frame, or -1. */
  graphicsDeadline(): number {
    return this.#engine.graphics_deadline_ms()
  }

  dispose(): void {
    if (this.#disposed) return
    this.#disposed = true
    this.#engine.free()
  }
}

/** Number of u32 slots per cell in `CellRows.data`. */
export function cellSlots(): number {
  return cellStride()
}
