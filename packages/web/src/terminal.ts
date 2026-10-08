import {
  type CursorState,
  type MouseButton,
  type MouseEventKind,
  type ProgressKind,
  type ProgramStatusRecord,
  TermyCore,
  type TermyEvent,
  type TerminalModes,
  init,
  isInitialized,
} from '@termysh/core'
import { Emitter, type IDisposable, type IEvent } from './emitter.ts'
import { isMac, modifiersOf, translateKey } from './input/keys.ts'
import { type LinkMatch, isSafeLink, linkAt } from './links.ts'
import {
  METRIC_OPTIONS,
  type ResolvedOptions,
  type TerminalOptions,
  resolveOptions,
} from './options.ts'
import { CanvasRenderer, type LinkHighlight } from './renderer/canvas.ts'
import { ImageLayer } from './renderer/images.ts'
import { type CellMetrics, measureCell } from './renderer/metrics.ts'
import { type BufferPoint, Selection, type SelectionRange } from './selection.ts'
import { type ResolvedTheme, queryColors, resolveTheme } from './theme.ts'

export interface TerminalSize {
  cols: number
  rows: number
}

export interface KeyEvent {
  /** Bytes the key produced, decoded as text. */
  key: string
  domEvent: KeyboardEvent
}

export interface ProgressEvent {
  state: ProgressKind
  value: number
}

export interface ClipboardEvent {
  /** OSC 52 selection targets, e.g. `c` or `p`. */
  selection: string
  text: string
}

/** Return `false` to stop the terminal from handling a key event. */
export type CustomKeyEventHandler = (event: KeyboardEvent) => boolean

const BLINK_INTERVAL = 600
const COMPACT_DELAY = 250
const utf8 = new TextDecoder('utf-8', { fatal: true })
const encoder = new TextEncoder()

/**
 * A browser terminal backed by Termy's WebAssembly engine.
 *
 * ```ts
 * const term = new Terminal({ theme: 'tokyo-night', fontSize: 13 })
 * term.open(document.getElementById('terminal')!)
 * term.onData((data) => socket.send(data))
 * socket.onmessage = (event) => term.write(event.data)
 * ```
 *
 * The constructor is synchronous. Until the wasm module is loaded, writes and
 * resizes are queued; `await terminal.ready` when you need the engine.
 */
export class Terminal implements IDisposable {
  /** Resolves once the wasm engine is loaded and the terminal is usable. */
  readonly ready: Promise<void>

  #options: ResolvedOptions
  #core: TermyCore | undefined
  #pending: Array<() => void> = []
  #disposed = false
  #disposables: Array<() => void> = []

  #root: HTMLDivElement | undefined
  #screen: HTMLDivElement | undefined
  #textarea: HTMLTextAreaElement | undefined
  #scrollbar: HTMLDivElement | undefined
  #thumb: HTMLDivElement | undefined
  #liveRegion: HTMLDivElement | undefined
  #renderer: CanvasRenderer | undefined
  #images: ImageLayer | undefined
  #metrics: CellMetrics | undefined
  #theme: ResolvedTheme

  #frame = 0
  #syncTimer: ReturnType<typeof setTimeout> | undefined
  #compactTimer: ReturnType<typeof setTimeout> | undefined
  #imageTimer: ReturnType<typeof setTimeout> | undefined
  #blinkTimer: ReturnType<typeof setInterval> | undefined
  #blinkOn = true
  #focused = false
  #composing = false
  #lastCursor: CursorState | undefined
  #lastViewport = -1
  #title = ''
  #selection: Selection
  #selecting = false
  #mouseButton: MouseButton | undefined
  #lastMouseCell: [number, number] | undefined
  #hoverLink: LinkMatch | undefined
  #wheelRemainder = 0
  #customKeyHandler: CustomKeyEventHandler | undefined

  readonly #onData = new Emitter<string>()
  readonly #onBinary = new Emitter<string>()
  readonly #onBytes = new Emitter<Uint8Array>()
  readonly #onResize = new Emitter<TerminalSize>()
  readonly #onTitleChange = new Emitter<string>()
  readonly #onBell = new Emitter<void>()
  readonly #onSelectionChange = new Emitter<void>()
  readonly #onScroll = new Emitter<number>()
  readonly #onRender = new Emitter<{ start: number; end: number }>()
  readonly #onKey = new Emitter<KeyEvent>()
  readonly #onCursorMove = new Emitter<void>()
  readonly #onWriteParsed = new Emitter<void>()
  readonly #onCwdChange = new Emitter<string>()
  readonly #onProgress = new Emitter<ProgressEvent>()
  readonly #onProgramStatus = new Emitter<ProgramStatusRecord[]>()
  readonly #onClipboard = new Emitter<ClipboardEvent>()
  readonly #onShellIntegration = new Emitter<string>()
  readonly #onFocus = new Emitter<void>()
  readonly #onBlur = new Emitter<void>()

  /** Text input for the host, UTF-8 decoded (keys, paste, protocol replies). */
  readonly onData: IEvent<string> = this.#onData.event
  /** Non-UTF-8 input (legacy X10 mouse reports) as a binary string. */
  readonly onBinary: IEvent<string> = this.#onBinary.event
  /** Every byte sent to the host, regardless of encoding. */
  readonly onBytes: IEvent<Uint8Array> = this.#onBytes.event
  readonly onResize: IEvent<TerminalSize> = this.#onResize.event
  readonly onTitleChange: IEvent<string> = this.#onTitleChange.event
  readonly onBell: IEvent<void> = this.#onBell.event
  readonly onSelectionChange: IEvent<void> = this.#onSelectionChange.event
  /** Fires with the new top line (absolute, 0 = oldest scrollback). */
  readonly onScroll: IEvent<number> = this.#onScroll.event
  readonly onRender: IEvent<{ start: number; end: number }> = this.#onRender.event
  readonly onKey: IEvent<KeyEvent> = this.#onKey.event
  readonly onCursorMove: IEvent<void> = this.#onCursorMove.event
  readonly onWriteParsed: IEvent<void> = this.#onWriteParsed.event
  /** OSC 7 working directory. */
  readonly onCwdChange: IEvent<string> = this.#onCwdChange.event
  /** OSC 9;4 progress. */
  readonly onProgress: IEvent<ProgressEvent> = this.#onProgress.event
  /** Coalesced OSC 7501 record snapshot, including inherited apps. */
  readonly onProgramStatus: IEvent<ProgramStatusRecord[]> = this.#onProgramStatus.event
  /** OSC 52 clipboard writes, whether or not `allowClipboardWrite` is on. */
  readonly onClipboard: IEvent<ClipboardEvent> = this.#onClipboard.event
  /** OSC 133 shell integration marks. */
  readonly onShellIntegration: IEvent<string> = this.#onShellIntegration.event
  readonly onFocus: IEvent<void> = this.#onFocus.event
  readonly onBlur: IEvent<void> = this.#onBlur.event

  constructor(options: TerminalOptions = {}) {
    this.#options = resolveOptions(options)
    this.#selection = new Selection(this.#options.wordSeparator)
    this.#theme = { palette: [] } as unknown as ResolvedTheme
    this.ready = (isInitialized() ? Promise.resolve() : init(this.#options.wasm)).then(() => {
      if (this.#disposed) return
      this.#core = new TermyCore({
        cols: this.#options.cols,
        rows: this.#options.rows,
        scrollback: this.#options.scrollback,
      })
      this.#applyTheme()
      this.#core.setDefaultCursorShape(this.#options.cursorStyle)
      if (this.#root) this.#attach()
      const pending = this.#pending
      this.#pending = []
      for (const operation of pending) operation()
    })
  }

  /** Construct and wait for the engine. */
  static async create(options: TerminalOptions = {}): Promise<Terminal> {
    const terminal = new Terminal(options)
    await terminal.ready
    return terminal
  }

  // ── State ────────────────────────────────────────────────────────────

  get cols(): number {
    return this.#core?.cols ?? this.#options.cols
  }

  get rows(): number {
    return this.#core?.rows ?? this.#options.rows
  }

  /** The headless engine, for advanced integrations. Undefined until `ready`. */
  get core(): TermyCore | undefined {
    return this.#core
  }

  get element(): HTMLElement | undefined {
    return this.#root
  }

  get textarea(): HTMLTextAreaElement | undefined {
    return this.#textarea
  }

  get options(): Readonly<ResolvedOptions> {
    return { ...this.#options }
  }

  get title(): string {
    return this.#title
  }

  get modes(): TerminalModes | undefined {
    return this.#core?.modes()
  }

  /** Cell size in CSS pixels. */
  get cellSize(): { width: number; height: number } | undefined {
    return this.#metrics ? { width: this.#metrics.width, height: this.#metrics.height } : undefined
  }

  getOption<K extends keyof ResolvedOptions>(key: K): ResolvedOptions[K] {
    return this.#options[key]
  }

  setOption<K extends keyof TerminalOptions>(key: K, value: TerminalOptions[K]): void {
    this.setOptions({ [key]: value } as TerminalOptions)
  }

  /** Update options live; fonts, theme, cursor and scrollback apply immediately. */
  setOptions(options: TerminalOptions): void {
    const changed = new Set<keyof TerminalOptions>()
    for (const [key, value] of Object.entries(options) as Array<[keyof TerminalOptions, unknown]>) {
      if (this.#options[key as keyof ResolvedOptions] !== value) changed.add(key)
    }
    this.#options = resolveOptions({ ...this.#options, ...options })
    if (changed.size === 0) return
    this.#whenReady(() => {
      const core = this.#core!
      if (changed.has('theme')) this.#applyTheme()
      if (changed.has('scrollback')) core.setScrollback(this.#options.scrollback)
      if (changed.has('cursorStyle')) core.setDefaultCursorShape(this.#options.cursorStyle)
      if (changed.has('wordSeparator')) this.#selection.wordSeparator = this.#options.wordSeparator
      if (changed.has('cursorBlink')) this.#restartBlink()
      if (changed.has('scrollbar')) this.#updateScrollbar()
      if ((changed.has('cols') || changed.has('rows')) && !this.#options.autoFit) {
        this.resize(this.#options.cols, this.#options.rows)
      }
      if ([...changed].some((key) => METRIC_OPTIONS.has(key))) this.#remeasure()
      else this.#configureRenderer()
      if (changed.has('autoFit') && this.#options.autoFit) this.fit()
      this.#scheduleRender()
    })
  }

  // ── Lifecycle ────────────────────────────────────────────────────────

  /** Mount into `parent`. The terminal fills it and, with `autoFit`, follows its size. */
  open(parent: HTMLElement): void {
    if (this.#root) throw new Error('@termysh/web: terminal is already open')
    const root = document.createElement('div')
    root.className = 'termy'
    Object.assign(root.style, {
      position: 'relative',
      width: '100%',
      height: '100%',
      overflow: 'hidden',
      outline: 'none',
      cursor: 'text',
      userSelect: 'none',
      webkitUserSelect: 'none',
      contain: 'strict',
    } satisfies Partial<CSSStyleDeclaration>)
    parent.appendChild(root)
    this.#root = root
    if (this.#core) this.#attach()
  }

  dispose(): void {
    if (this.#disposed) return
    this.#disposed = true
    cancelAnimationFrame(this.#frame)
    clearTimeout(this.#syncTimer)
    clearTimeout(this.#compactTimer)
    clearTimeout(this.#imageTimer)
    clearInterval(this.#blinkTimer)
    for (const dispose of this.#disposables) dispose()
    this.#images?.dispose()
    this.#root?.remove()
    this.#core?.dispose()
    for (const emitter of [
      this.#onData,
      this.#onBinary,
      this.#onBytes,
      this.#onResize,
      this.#onTitleChange,
      this.#onBell,
      this.#onSelectionChange,
      this.#onScroll,
      this.#onRender,
      this.#onKey,
      this.#onCursorMove,
      this.#onWriteParsed,
      this.#onCwdChange,
      this.#onProgress,
      this.#onProgramStatus,
      this.#onClipboard,
      this.#onShellIntegration,
      this.#onFocus,
      this.#onBlur,
    ]) {
      emitter.dispose()
    }
  }

  // ── Output ───────────────────────────────────────────────────────────

  /** Write program output. `callback` runs once it has been parsed. */
  write(data: string | Uint8Array, callback?: () => void): void {
    this.#whenReady(() => {
      const core = this.#core!
      const converted = this.#options.convertEol ? convertEol(data) : data
      core.write(converted)
      if (this.#options.screenReaderMode) this.#announce(converted)
      this.#afterOutput()
      this.#onWriteParsed.fire()
      callback?.()
    })
  }

  writeln(data: string | Uint8Array, callback?: () => void): void {
    if (typeof data === 'string') this.write(`${data}\r\n`, callback)
    else {
      this.write(data)
      this.write('\r\n', callback)
    }
  }

  // ── Input ────────────────────────────────────────────────────────────

  /** Send text as if the user typed it (fires `onData`). */
  input(data: string, wasUserInput = true): void {
    this.#emit(encoder.encode(data), wasUserInput)
  }

  /** Paste text, honoring bracketed paste mode. */
  paste(text: string): void {
    this.#whenReady(() => {
      const bracketed = !this.#options.ignoreBracketedPasteMode
      this.#emit(this.#core!.encodePaste(text, bracketed), true)
    })
  }

  attachCustomKeyEventHandler(handler: CustomKeyEventHandler | undefined): void {
    this.#customKeyHandler = handler
  }

  focus(): void {
    this.#textarea?.focus({ preventScroll: true })
  }

  blur(): void {
    this.#textarea?.blur()
  }

  get hasFocus(): boolean {
    return this.#focused
  }

  // ── Size ─────────────────────────────────────────────────────────────

  resize(cols: number, rows: number): void {
    cols = Math.max(2, Math.floor(cols))
    rows = Math.max(1, Math.floor(rows))
    this.#whenReady(() => {
      const core = this.#core!
      if (core.cols === cols && core.rows === rows) return
      core.resize(cols, rows)
      this.#options.cols = cols
      this.#options.rows = rows
      this.#renderer?.resize()
      this.#layout()
      this.#onResize.fire({ cols, rows })
      this.#afterOutput()
    })
  }

  /** The grid size that fits the container, or undefined when not measurable. */
  proposeDimensions(): TerminalSize | undefined {
    const root = this.#root
    const metrics = this.#metrics
    if (!root || !metrics) return undefined
    const padding = this.#options.padding * 2
    const width = root.clientWidth - padding
    const height = root.clientHeight - padding
    if (width <= 0 || height <= 0) return undefined
    return {
      cols: Math.max(2, Math.floor(width / metrics.width)),
      rows: Math.max(1, Math.floor(height / metrics.height)),
    }
  }

  /** Resize the grid to the container. */
  fit(): TerminalSize | undefined {
    const size = this.proposeDimensions()
    if (size) this.resize(size.cols, size.rows)
    return size
  }

  // ── Buffer ───────────────────────────────────────────────────────────

  /** Lines of scrollback above the live screen. */
  get historySize(): number {
    return this.#core?.historySize ?? 0
  }

  /** Absolute line at the top of the viewport (0 = oldest scrollback). */
  get viewportY(): number {
    const core = this.#core
    return core ? core.historySize - core.displayOffset : 0
  }

  /** Text of an absolute buffer line. */
  getLine(line: number, trimEnd = true): string | undefined {
    const core = this.#core
    return core?.lineText(line - core.historySize, trimEnd)
  }

  isLineWrapped(line: number): boolean {
    const core = this.#core
    return core ? core.lineWrapped(line - core.historySize) : false
  }

  get cursor(): CursorState | undefined {
    return this.#core?.cursor()
  }

  /** Clear scrollback and the screen, keeping the cursor line at the top. */
  clear(): void {
    this.#whenReady(() => {
      const core = this.#core!
      const { row } = core.cursor()
      if (row > 0 && !core.modes().alternateScreen) core.write(`\x1b[${row}S\x1b[${row}A`)
      core.clearScrollback()
      this.clearSelection()
      this.#renderer?.invalidate()
      this.#afterOutput()
    })
  }

  /** Call after process exit; preserves done/error records and emits the new snapshot. */
  processExited(): void {
    this.#whenReady(() => {
      this.#core!.processExited()
      this.#afterOutput()
    })
  }

  /** Full terminal reset (RIS). */
  reset(): void {
    this.#whenReady(() => {
      this.#core!.write('\x1bc')
      this.#core!.clearScrollback()
      this.clearSelection()
      this.#afterOutput()
    })
  }

  /** Scroll by lines; positive moves toward the bottom (xterm.js convention). */
  scrollLines(amount: number): void {
    const core = this.#core
    if (core && amount !== 0 && core.scrollDisplay(-Math.trunc(amount))) this.#afterScroll()
  }

  scrollPages(pages: number): void {
    this.scrollLines(pages * (this.rows - 1))
  }

  scrollToTop(): void {
    this.scrollLines(-this.historySize)
  }

  scrollToBottom(): void {
    if (this.#core?.scrollToBottom()) this.#afterScroll()
  }

  /** Scroll so absolute `line` is at the top of the viewport. */
  scrollToLine(line: number): void {
    this.scrollLines(line - this.viewportY)
  }

  // ── Selection ────────────────────────────────────────────────────────

  hasSelection(): boolean {
    return this.#selection.active
  }

  getSelection(): string {
    return this.#core ? this.#selection.text(this.#core) : ''
  }

  getSelectionPosition(): SelectionRange | undefined {
    return this.#core && this.#selection.active ? this.#selection.range(this.#core) : undefined
  }

  /** Select `length` cells starting at absolute `line`, `col`, wrapping across lines. */
  select(col: number, line: number, length: number): void {
    const cols = this.cols
    const endOffset = col + length
    this.#setSelection({
      start: { line, col },
      end: { line: line + Math.floor(endOffset / cols), col: endOffset % cols },
    })
  }

  selectLines(start: number, end: number): void {
    this.#setSelection({ start: { line: start, col: 0 }, end: { line: end, col: this.cols } })
  }

  selectAll(): void {
    this.#setSelection({
      start: { line: 0, col: 0 },
      end: { line: this.historySize + this.rows - 1, col: this.cols },
    })
  }

  clearSelection(): void {
    if (!this.#selection.active) return
    this.#selection.clear()
    this.#renderer?.invalidate()
    this.#scheduleRender()
    this.#onSelectionChange.fire()
  }

  // ── Internals ────────────────────────────────────────────────────────

  #whenReady(operation: () => void): void {
    if (this.#disposed) return
    if (this.#core) operation()
    else this.#pending.push(operation)
  }

  #setSelection(range: SelectionRange): void {
    this.#selection.set(range)
    this.#renderer?.invalidate()
    this.#scheduleRender()
    this.#onSelectionChange.fire()
  }

  #applyTheme(): void {
    this.#theme = resolveTheme(this.#options.theme)
    this.#core?.setQueryColors(queryColors(this.#theme))
    if (this.#root) this.#root.style.background = this.#options.allowTransparency ? 'transparent' : this.#theme.background.css
    if (this.#thumb) this.#thumb.style.background = this.#theme.scrollbarThumb.css
    this.#renderer?.setTheme(this.#theme)
  }

  #attach(): void {
    const root = this.#root!
    const core = this.#core!
    const screen = document.createElement('div')
    screen.style.position = 'absolute'
    root.appendChild(screen)
    this.#screen = screen

    this.#images = new ImageLayer(() => {
      if (this.#metrics) this.#images?.repaint(this.#metrics)
    })
    this.#renderer = new CanvasRenderer(core)
    screen.append(this.#images.under, this.#renderer.canvas, this.#images.over)

    const textarea = document.createElement('textarea')
    textarea.setAttribute('aria-label', 'Terminal input')
    textarea.setAttribute('autocorrect', 'off')
    textarea.setAttribute('autocapitalize', 'off')
    textarea.setAttribute('spellcheck', 'false')
    textarea.tabIndex = 0
    Object.assign(textarea.style, {
      position: 'absolute',
      opacity: '0',
      width: '1px',
      height: '1px',
      padding: '0',
      border: '0',
      margin: '0',
      resize: 'none',
      overflow: 'hidden',
      whiteSpace: 'nowrap',
      zIndex: '-5',
    } satisfies Partial<CSSStyleDeclaration>)
    root.appendChild(textarea)
    this.#textarea = textarea

    const scrollbar = document.createElement('div')
    Object.assign(scrollbar.style, {
      position: 'absolute',
      top: '0',
      right: '0',
      bottom: '0',
      width: '10px',
      opacity: '0',
      transition: 'opacity 150ms',
      cursor: 'default',
    } satisfies Partial<CSSStyleDeclaration>)
    const thumb = document.createElement('div')
    Object.assign(thumb.style, {
      position: 'absolute',
      right: '2px',
      width: '6px',
      borderRadius: '3px',
    } satisfies Partial<CSSStyleDeclaration>)
    scrollbar.appendChild(thumb)
    root.appendChild(scrollbar)
    this.#scrollbar = scrollbar
    this.#thumb = thumb

    const live = document.createElement('div')
    live.setAttribute('aria-live', 'polite')
    live.setAttribute('role', 'log')
    Object.assign(live.style, {
      position: 'absolute',
      width: '1px',
      height: '1px',
      overflow: 'hidden',
      clipPath: 'inset(50%)',
    } satisfies Partial<CSSStyleDeclaration>)
    root.appendChild(live)
    this.#liveRegion = live

    this.#applyTheme()
    this.#remeasure()
    this.#bindInput(root, screen, textarea)
    this.#bindScrollbar(scrollbar, thumb)

    if (typeof ResizeObserver !== 'undefined') {
      const observer = new ResizeObserver(() => {
        if (this.#options.autoFit) this.fit()
      })
      observer.observe(root)
      this.#disposables.push(() => observer.disconnect())
    }
    if (typeof window !== 'undefined' && typeof matchMedia === 'function') {
      // Re-measure when the window moves to a display with a different DPR.
      let query: MediaQueryList | undefined
      const watch = (): void => {
        query?.removeEventListener('change', onChange)
        query = matchMedia(`(resolution: ${window.devicePixelRatio}dppx)`)
        query.addEventListener('change', onChange)
      }
      const onChange = (): void => {
        this.#remeasure()
        watch()
      }
      watch()
      this.#disposables.push(() => query?.removeEventListener('change', onChange))
    }
    if (typeof document !== 'undefined' && document.fonts) {
      const onFonts = (): void => this.#remeasure()
      document.fonts.addEventListener('loadingdone', onFonts)
      this.#disposables.push(() => document.fonts.removeEventListener('loadingdone', onFonts))
    }
    if (this.#options.autoFit) this.fit()
    this.#restartBlink()
    this.#scheduleRender()
  }

  #remeasure(): void {
    if (!this.#renderer || !this.#core) return
    const dpr = this.#options.devicePixelRatio ?? (typeof window === 'undefined' ? 1 : window.devicePixelRatio || 1)
    this.#metrics = measureCell(this.#options, dpr)
    this.#core.setCellPixels(this.#metrics.width, this.#metrics.height)
    this.#configureRenderer()
    this.#renderer.setMetrics(this.#metrics)
    this.#layout()
    if (this.#options.autoFit) this.fit()
    this.#scheduleRender()
  }

  #configureRenderer(): void {
    const options = this.#options
    this.#renderer?.setOptions({
      cursorStyle: options.cursorStyle,
      cursorInactiveStyle: options.cursorInactiveStyle,
      cursorWidth: options.cursorWidth,
      drawBoldTextInBrightColors: options.drawBoldTextInBrightColors,
      minimumContrastRatio: options.minimumContrastRatio,
      customGlyphs: options.customGlyphs,
      transparentBackground: options.allowTransparency || (this.#images?.hasUnderImages ?? false),
    })
    if (this.#root) this.#root.style.background = options.allowTransparency ? 'transparent' : this.#theme.background.css
  }

  #layout(): void {
    const metrics = this.#metrics
    const screen = this.#screen
    if (!metrics || !screen || !this.#core) return
    const padding = this.#options.padding
    screen.style.left = `${padding}px`
    screen.style.top = `${padding}px`
    const width = this.#core.cols * metrics.deviceWidth
    const height = this.#core.rows * metrics.deviceHeight
    this.#images?.setSize(width, height, width / metrics.dpr, height / metrics.dpr)
    screen.style.width = `${width / metrics.dpr}px`
    screen.style.height = `${height / metrics.dpr}px`
  }

  #afterOutput(): void {
    const core = this.#core!
    const replies = core.takeReplies()
    if (replies.length > 0) this.#emit(replies, false)
    for (const event of core.takeEvents()) this.#handleEvent(event)

    clearTimeout(this.#syncTimer)
    const deadline = core.syncDeadline()
    if (deadline >= 0) {
      this.#syncTimer = setTimeout(() => {
        if (this.#core?.flushSync()) this.#afterOutput()
      }, deadline + 1)
    }
    clearTimeout(this.#compactTimer)
    this.#compactTimer = setTimeout(() => this.#core?.compactHistory(), COMPACT_DELAY)
    this.#scheduleRender()
  }

  #afterScroll(): void {
    this.#renderer?.invalidate()
    this.#scheduleRender()
  }

  #handleEvent(event: TermyEvent): void {
    switch (event.type) {
      case 'title':
        this.#title = event.title
        this.#onTitleChange.fire(event.title)
        break
      case 'resetTitle':
        this.#title = ''
        this.#onTitleChange.fire('')
        break
      case 'bell':
        this.#onBell.fire()
        if (this.#options.bellStyle === 'visual') this.#flash()
        break
      case 'cwd':
        this.#onCwdChange.fire(event.cwd)
        break
      case 'programStatus':
        this.#onProgramStatus.fire(event.records)
        break
      case 'progress':
        this.#onProgress.fire({ state: event.state, value: event.value })
        break
      case 'shellIntegration':
        this.#onShellIntegration.fire(event.payload)
        break
      case 'clipboard': {
        if (event.data === '?') break
        const text = decodeBase64(event.data)
        if (text === undefined) break
        this.#onClipboard.fire({ selection: event.selection, text })
        if (this.#options.allowClipboardWrite) void navigator.clipboard?.writeText(text).catch(() => {})
        break
      }
    }
  }

  #emit(bytes: Uint8Array, user: boolean): void {
    if (bytes.length === 0) return
    if (user) {
      if (this.#options.disableStdin) return
      if (this.#options.scrollOnUserInput && this.#core?.scrollToBottom()) this.#afterScroll()
    }
    this.#onBytes.fire(bytes)
    let text: string | undefined
    try {
      text = utf8.decode(bytes)
    } catch {
      this.#onBinary.fire(String.fromCharCode(...bytes))
      return
    }
    this.#onData.fire(text)
  }

  #scheduleRender(): void {
    if (this.#frame || !this.#renderer || this.#disposed) return
    const raf = typeof requestAnimationFrame === 'function' ? requestAnimationFrame : (fn: FrameRequestCallback) => setTimeout(() => fn(0), 16) as unknown as number
    this.#frame = raf(() => {
      this.#frame = 0
      this.#render()
    })
  }

  #render(): void {
    const core = this.#core
    const renderer = this.#renderer
    const metrics = this.#metrics
    if (!core || !renderer || !metrics) return

    const images = this.#images!
    const hadUnder = images.hasUnderImages
    images.update(core, metrics, this.#options.images)
    if (images.hasUnderImages !== hadUnder) this.#configureRenderer()
    clearTimeout(this.#imageTimer)
    const imageDeadline = images.deadline(core)
    if (imageDeadline >= 0) this.#imageTimer = setTimeout(() => this.#scheduleRender(), Math.max(8, imageDeadline))

    const cursor = core.cursor()
    const blinking = cursor.blinking || this.#options.cursorBlink
    const range = this.#selection.active ? this.#selection.range(core) : undefined
    const hover = this.#hoverLink
    const link: LinkHighlight | undefined = hover ? { row: hover.row, start: hover.start, end: hover.end } : undefined
    const painted = renderer.render(
      { cursor: { ...cursor, blinking }, focused: this.#focused, blinkOn: this.#blinkOn },
      range ? (row, col) => this.#selection.contains(core, range, row, col) : undefined,
      link,
    )
    if (painted > 0) this.#onRender.fire({ start: 0, end: core.rows - 1 })

    const last = this.#lastCursor
    if (!last || last.row !== cursor.row || last.col !== cursor.col) {
      this.#lastCursor = cursor
      this.#positionTextarea(cursor)
      if (last) this.#onCursorMove.fire()
    }
    const viewport = core.historySize - core.displayOffset
    if (viewport !== this.#lastViewport) {
      this.#lastViewport = viewport
      this.#onScroll.fire(viewport)
    }
    this.#updateScrollbar()
  }

  #positionTextarea(cursor: CursorState): void {
    const metrics = this.#metrics
    const textarea = this.#textarea
    if (!metrics || !textarea) return
    const padding = this.#options.padding
    textarea.style.left = `${padding + cursor.col * metrics.width}px`
    textarea.style.top = `${padding + cursor.row * metrics.height}px`
    textarea.style.height = `${metrics.height}px`
    textarea.style.lineHeight = `${metrics.height}px`
    textarea.style.fontSize = `${this.#options.fontSize}px`
  }

  #restartBlink(): void {
    clearInterval(this.#blinkTimer)
    this.#blinkOn = true
    this.#blinkTimer = setInterval(() => {
      const core = this.#core
      if (!core || !this.#focused) return
      if (!(this.#options.cursorBlink || core.cursor().blinking)) return
      this.#blinkOn = !this.#blinkOn
      this.#renderer?.invalidateRow(core.cursor().row + core.displayOffset)
      this.#scheduleRender()
    }, BLINK_INTERVAL)
  }

  #updateScrollbar(): void {
    const scrollbar = this.#scrollbar
    const thumb = this.#thumb
    const core = this.#core
    if (!scrollbar || !thumb || !core) return
    const history = core.historySize
    if (!this.#options.scrollbar || history === 0 || core.modes().alternateScreen) {
      scrollbar.style.display = 'none'
      return
    }
    scrollbar.style.display = 'block'
    const height = scrollbar.clientHeight
    const total = history + core.rows
    const thumbHeight = Math.max(20, (core.rows / total) * height)
    const top = ((history - core.displayOffset) / history) * (height - thumbHeight)
    thumb.style.height = `${thumbHeight}px`
    thumb.style.top = `${top}px`
  }

  #bindScrollbar(scrollbar: HTMLDivElement, thumb: HTMLDivElement): void {
    const root = this.#root!
    const show = (): void => {
      scrollbar.style.opacity = '1'
    }
    const hide = (): void => {
      scrollbar.style.opacity = '0'
    }
    this.#listen(root, 'mouseenter', show)
    this.#listen(root, 'mouseleave', hide)
    this.#listen(thumb, 'pointerdown', (event: PointerEvent) => {
      event.preventDefault()
      event.stopPropagation()
      const core = this.#core
      if (!core) return
      thumb.setPointerCapture(event.pointerId)
      const startY = event.clientY
      const startOffset = core.displayOffset
      const move = (moveEvent: PointerEvent): void => {
        const history = core.historySize
        const track = scrollbar.clientHeight - thumb.clientHeight
        if (track <= 0) return
        const target = Math.round(startOffset - ((moveEvent.clientY - startY) / track) * history)
        const clamped = Math.max(0, Math.min(history, target))
        if (core.scrollDisplay(clamped - core.displayOffset)) this.#afterScroll()
      }
      const up = (): void => {
        thumb.removeEventListener('pointermove', move)
        thumb.removeEventListener('pointerup', up)
      }
      thumb.addEventListener('pointermove', move)
      thumb.addEventListener('pointerup', up)
    })
  }

  #listen<K extends keyof HTMLElementEventMap>(
    target: HTMLElement | Window,
    type: K,
    listener: (event: HTMLElementEventMap[K]) => void,
    options?: AddEventListenerOptions,
  ): void {
    target.addEventListener(type, listener as EventListener, options)
    this.#disposables.push(() => target.removeEventListener(type, listener as EventListener, options))
  }

  #bindInput(root: HTMLDivElement, screen: HTMLDivElement, textarea: HTMLTextAreaElement): void {
    this.#listen(textarea, 'focus', () => {
      this.#focused = true
      this.#blinkOn = true
      const focusBytes = this.#core?.encodeFocus(true)
      if (focusBytes) this.#emit(focusBytes, false)
      this.#renderer?.invalidate()
      this.#scheduleRender()
      this.#onFocus.fire()
    })
    this.#listen(textarea, 'blur', () => {
      this.#focused = false
      const focusBytes = this.#core?.encodeFocus(false)
      if (focusBytes) this.#emit(focusBytes, false)
      this.#renderer?.invalidate()
      this.#scheduleRender()
      this.#onBlur.fire()
    })
    this.#listen(textarea, 'keydown', (event) => this.#keyDown(event))
    this.#listen(textarea, 'keyup', (event) => this.#keyUp(event))
    this.#listen(textarea, 'compositionstart', () => {
      this.#composing = true
    })
    this.#listen(textarea, 'compositionend', (event) => {
      this.#composing = false
      if (event.data) this.input(event.data)
      textarea.value = ''
    })
    this.#listen(textarea, 'input', (event) => {
      const input = event as InputEvent
      if (this.#composing || input.isComposing) return
      if ((input.inputType === 'insertText' || input.inputType === 'insertReplacementText') && input.data) {
        this.input(input.data)
      }
      textarea.value = ''
    })
    this.#listen(textarea, 'paste', (event) => {
      event.preventDefault()
      const text = event.clipboardData?.getData('text/plain')
      if (text) this.paste(text)
    })
    this.#listen(textarea, 'copy', (event) => {
      if (!this.hasSelection()) return
      event.preventDefault()
      event.clipboardData?.setData('text/plain', this.getSelection())
    })
    this.#listen(root, 'mousedown', (event) => this.#mouseDown(event, screen))
    this.#listen(root, 'mousemove', (event) => this.#hover(event, screen))
    this.#listen(root, 'mouseleave', (event) => this.#setHoverLink(undefined, event))
    this.#listen(root, 'wheel', (event) => this.#wheel(event, screen), { passive: false })
    this.#listen(root, 'contextmenu', (event) => {
      if (this.#options.rightClickSelectsWord && this.#core) {
        const [col, row] = this.#cellAt(event, screen)
        this.#selection.start(this.#core, this.#bufferPoint(col, row), 'word')
        this.#afterSelectionChange()
      }
    })
  }

  #keyDown(event: KeyboardEvent): void {
    const core = this.#core
    if (!core || this.#options.disableStdin) return
    if (this.#customKeyHandler && this.#customKeyHandler(event) === false) return
    if (event.isComposing || event.keyCode === 229 || this.#composing) return

    const mac = isMac()
    const lower = event.key.toLowerCase()
    const clipboardShortcut = mac
      ? event.metaKey && !event.ctrlKey && !event.altKey
      : event.ctrlKey && event.shiftKey && !event.altKey
    if (clipboardShortcut && (lower === 'c' || lower === 'v' || lower === 'x')) return
    if (clipboardShortcut && lower === 'a') {
      event.preventDefault()
      this.selectAll()
      return
    }
    // Leave Cmd shortcuts to the browser on macOS.
    if (mac && event.metaKey) return

    const translated = translateKey(event, this.#options.macOptionIsMeta)
    if (!translated) return
    let modifiers = translated.modifiers
    // macOS Option composes characters unless it is configured as Meta.
    if (mac && event.altKey && !translated.optionAsAlt && translated.text) modifiers &= ~2
    const bytes = core.encodeKey(
      translated.key,
      translated.text,
      modifiers,
      event.repeat ? 'repeat' : 'press',
      translated.optionAsAlt,
    )
    if (!bytes) return
    event.preventDefault()
    event.stopPropagation()
    this.#onKey.fire({ key: new TextDecoder().decode(bytes), domEvent: event })
    if (this.#selection.active) this.clearSelection()
    this.#emit(bytes, true)
  }

  #keyUp(event: KeyboardEvent): void {
    const core = this.#core
    if (!core || this.#options.disableStdin) return
    // Only the kitty "report event types" flag wants releases.
    if ((core.modes().kittyKeyboardFlags & 2) === 0) return
    if (this.#customKeyHandler && this.#customKeyHandler(event) === false) return
    const translated = translateKey(event, this.#options.macOptionIsMeta)
    if (!translated) return
    const bytes = core.encodeKey(translated.key, translated.text, translated.modifiers, 'release', translated.optionAsAlt)
    if (bytes) this.#emit(bytes, true)
  }

  #cellAt(event: MouseEvent, screen: HTMLElement): [number, number] {
    const metrics = this.#metrics
    const core = this.#core
    if (!metrics || !core) return [0, 0]
    const rect = screen.getBoundingClientRect()
    const col = Math.floor((event.clientX - rect.left) / metrics.width)
    const row = Math.floor((event.clientY - rect.top) / metrics.height)
    return [Math.max(0, Math.min(core.cols - 1, col)), Math.max(0, Math.min(core.rows - 1, row))]
  }

  #bufferPoint(col: number, row: number): BufferPoint {
    const core = this.#core!
    return { line: core.historySize - core.displayOffset + row, col }
  }

  #mouseTracking(event: MouseEvent): boolean {
    const core = this.#core
    if (!core || !core.modes().mouseTracking || event.shiftKey) return false
    return !(this.#options.macOptionClickForcesSelection && event.altKey && isMac())
  }

  #mouseDown(event: MouseEvent, screen: HTMLElement): void {
    const core = this.#core
    if (!core) return
    if (event.target === this.#thumb) return
    event.preventDefault()
    this.focus()
    const [col, row] = this.#cellAt(event, screen)
    const button: MouseButton = event.button === 1 ? 'middle' : event.button === 2 ? 'right' : 'left'

    if (this.#mouseTracking(event)) {
      this.#mouseButton = button
      this.#sendMouse('press', button, col, row, event)
      this.#trackDrag(screen, (move) => {
        const [c, r] = this.#cellAt(move, screen)
        this.#sendMouse('drag', button, c, r, move)
      }, (up) => {
        const [c, r] = this.#cellAt(up, screen)
        this.#sendMouse('release', button, c, r, up)
        this.#mouseButton = undefined
      })
      return
    }
    if (button !== 'left') return

    if (this.#hoverLink && event.detail === 1 && this.#options.linkHandler) {
      const link = this.#hoverLink
      let moved = false
      this.#trackDrag(screen, () => {
        moved = true
      }, (up) => {
        if (!moved) this.#activateLink(link, up)
      })
      return
    }

    if (event.altKey && this.#options.altClickMovesCursor && !core.modes().alternateScreen) {
      const cursor = core.cursor()
      if (row - core.displayOffset === cursor.row && col !== cursor.col) {
        const key = col > cursor.col ? 'right' : 'left'
        const bytes = core.encodeKey(key, undefined, 0)
        if (bytes) for (let i = 0; i < Math.abs(col - cursor.col); i++) this.#emit(bytes, true)
        return
      }
    }

    const point = this.#bufferPoint(col, row)
    if (event.shiftKey && this.#selection.active) this.#selection.extend(core, point)
    else this.#selection.start(core, point, event.detail >= 3 ? 'line' : event.detail === 2 ? 'word' : 'char')
    this.#selecting = true
    this.#afterSelectionChange(false)
    this.#trackDrag(screen, (move) => {
      const metrics = this.#metrics
      const rect = screen.getBoundingClientRect()
      if (metrics && move.clientY < rect.top) this.scrollLines(-1)
      else if (metrics && move.clientY > rect.bottom) this.scrollLines(1)
      const [c, r] = this.#cellAt(move, screen)
      const inside = move.clientX - rect.left
      const edge = metrics && inside > (c + 0.5) * metrics.width ? c + 1 : c
      this.#selection.extend(core, this.#bufferPoint(edge, r))
      this.#afterSelectionChange(false)
    }, () => {
      this.#selecting = false
      this.#afterSelectionChange()
    })
  }

  #afterSelectionChange(final = true): void {
    this.#renderer?.invalidate()
    this.#scheduleRender()
    if (!final) return
    this.#onSelectionChange.fire()
    if (this.#options.copyOnSelect && this.hasSelection()) {
      void navigator.clipboard?.writeText(this.getSelection()).catch(() => {})
    }
  }

  #trackDrag(_screen: HTMLElement, move: (event: MouseEvent) => void, up: (event: MouseEvent) => void): void {
    const onMove = (event: MouseEvent): void => move(event)
    const onUp = (event: MouseEvent): void => {
      window.removeEventListener('mousemove', onMove)
      window.removeEventListener('mouseup', onUp)
      up(event)
    }
    window.addEventListener('mousemove', onMove)
    window.addEventListener('mouseup', onUp)
  }

  #sendMouse(kind: MouseEventKind, button: MouseButton, col: number, row: number, event: MouseEvent): void {
    const bytes = this.#core?.encodeMouse(kind, button, col, row, modifiersOf(event))
    if (bytes) this.#emit(bytes, true)
  }

  #hover(event: MouseEvent, screen: HTMLElement): void {
    const core = this.#core
    if (!core || this.#selecting || event.buttons !== 0) return
    const [col, row] = this.#cellAt(event, screen)
    const last = this.#lastMouseCell
    if (last && last[0] === col && last[1] === row) return
    this.#lastMouseCell = [col, row]
    if (this.#mouseTracking(event) && this.#mouseButton === undefined) {
      this.#sendMouse('move', 'left', col, row, event)
    }
    const handler = this.#options.linkHandler
    const link = handler ? linkAt(core, row, col, this.#options.linkDetection) : undefined
    this.#setHoverLink(link && isSafeLink(link.uri, handler?.allowNonHttpProtocols ?? false) ? link : undefined, event)
  }

  #setHoverLink(link: LinkMatch | undefined, event: MouseEvent): void {
    const previous = this.#hoverLink
    if (previous?.uri === link?.uri && previous?.row === link?.row && previous?.start === link?.start) return
    if (event.type === 'mouseleave') this.#lastMouseCell = undefined
    this.#hoverLink = link
    if (this.#root) this.#root.style.cursor = link ? 'pointer' : 'text'
    if (previous) {
      this.#options.linkHandler?.leave?.(event, previous.uri)
      this.#renderer?.invalidateRow(previous.row)
    }
    if (link) {
      this.#options.linkHandler?.hover?.(event, link.uri)
      this.#renderer?.invalidateRow(link.row)
    }
    this.#scheduleRender()
  }

  #activateLink(link: LinkMatch, event: MouseEvent): void {
    const handler = this.#options.linkHandler
    if (handler && isSafeLink(link.uri, handler.allowNonHttpProtocols ?? false)) handler.activate(event, link.uri)
  }

  #wheel(event: WheelEvent, screen: HTMLElement): void {
    const core = this.#core
    const metrics = this.#metrics
    if (!core || !metrics || event.deltaY === 0) return
    event.preventDefault()
    const fast = this.#options.fastScrollModifier
    const isFast =
      (fast === 'alt' && event.altKey) || (fast === 'ctrl' && event.ctrlKey) || (fast === 'shift' && event.shiftKey)
    const sensitivity = isFast ? this.#options.fastScrollSensitivity : this.#options.scrollSensitivity
    const pixels =
      event.deltaMode === WheelEvent.DOM_DELTA_LINE
        ? event.deltaY * metrics.height
        : event.deltaMode === WheelEvent.DOM_DELTA_PAGE
          ? event.deltaY * metrics.height * core.rows
          : event.deltaY
    this.#wheelRemainder += (pixels / metrics.height) * sensitivity
    const lines = Math.trunc(this.#wheelRemainder)
    if (lines === 0) return
    this.#wheelRemainder -= lines

    if (this.#mouseTracking(event)) {
      const [col, row] = this.#cellAt(event, screen)
      const kind: MouseEventKind = lines < 0 ? 'wheelUp' : 'wheelDown'
      for (let i = 0; i < Math.min(Math.abs(lines), 10); i++) this.#sendMouse(kind, 'left', col, row, event)
      return
    }
    if (core.modes().alternateScreen && this.#options.alternateScroll) {
      const bytes = core.encodeKey(lines < 0 ? 'up' : 'down', undefined, 0)
      if (bytes) for (let i = 0; i < Math.abs(lines); i++) this.#emit(bytes, true)
      return
    }
    this.scrollLines(lines)
  }

  #flash(): void {
    const root = this.#root
    if (!root) return
    const overlay = document.createElement('div')
    Object.assign(overlay.style, {
      position: 'absolute',
      inset: '0',
      background: this.#theme.foreground.css,
      opacity: '0.15',
      pointerEvents: 'none',
      transition: 'opacity 150ms',
    } satisfies Partial<CSSStyleDeclaration>)
    root.appendChild(overlay)
    requestAnimationFrame(() => {
      overlay.style.opacity = '0'
      setTimeout(() => overlay.remove(), 200)
    })
  }

  #announce(data: string | Uint8Array): void {
    const live = this.#liveRegion
    if (!live) return
    const text = (typeof data === 'string' ? data : new TextDecoder().decode(data))
      // biome-ignore lint/suspicious/noControlCharactersInRegex: stripping terminal escapes
      .replace(/\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b[@-_]/g, '')
      .replace(/\r/g, '')
    if (!text.trim()) return
    const line = document.createElement('div')
    line.textContent = text
    live.appendChild(line)
    while (live.childElementCount > 20) live.firstElementChild?.remove()
  }
}

function convertEol(data: string | Uint8Array): string | Uint8Array {
  if (typeof data === 'string') return data.replace(/\r?\n/g, '\r\n')
  const out: number[] = []
  for (let i = 0; i < data.length; i++) {
    const byte = data[i]!
    if (byte === 0x0a && data[i - 1] !== 0x0d) out.push(0x0d)
    out.push(byte)
  }
  return new Uint8Array(out)
}

function decodeBase64(value: string): string | undefined {
  try {
    const binary = atob(value)
    return new TextDecoder().decode(Uint8Array.from(binary, (ch) => ch.charCodeAt(0)))
  } catch {
    return undefined
  }
}
