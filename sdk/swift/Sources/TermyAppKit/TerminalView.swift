#if os(macOS)
import AppKit
import Termy

/// A window that can keep its terminals' grids at their size while its frame
/// changes in passing, such as while the app quits or a window restores full
/// screen. Programs then get one resize, to the final size, instead of one per
/// step. Call `fitGridToView()` on its terminals when it lets go.
@MainActor
public protocol TerminalGridHolding: AnyObject {
    var holdsTerminalGrids: Bool { get }
}

/// Receives terminal events a host app usually reflects in its chrome.
@MainActor
public protocol TerminalViewDelegate: AnyObject {
    func terminalView(_ view: TerminalView, didChangeTitle title: String?)
    func terminalView(_ view: TerminalView, didReceive event: TerminalEvent)
    /// Return false to block a link. The default opens allowed schemes.
    func terminalView(_ view: TerminalView, shouldOpen url: URL) -> Bool
    /// Return true to let a program replace the clipboard with OSC 52.
    ///
    /// Any output can carry OSC 52, including `cat` of an untrusted file or a
    /// remote host, so the default denies. Hosts that allow it should usually
    /// require `view.hasKeyboardFocus`.
    func terminalView(_ view: TerminalView, shouldWriteClipboard text: String) -> Bool
    func terminalViewDidExit(_ view: TerminalView)
    /// The user copied the selection, with ⌘C or the Copy menu item. Hosts can
    /// acknowledge it, for example by animating `view.selectionRects`.
    func terminalView(_ view: TerminalView, didCopy text: String)
    /// Something that can be dropped is over the view (true) or left it.
    func terminalView(_ view: TerminalView, isDropTarget: Bool)
    /// The view is about to show its right-click menu, which holds the link
    /// items for a link under the pointer, then Copy, Paste and Select All.
    /// Hosts can add their own items, such as splitting or closing a pane.
    func terminalView(_ view: TerminalView, willShowMenu menu: NSMenu)
}

extension TerminalViewDelegate {
    public func terminalView(_ view: TerminalView, didChangeTitle title: String?) {}
    public func terminalView(_ view: TerminalView, didReceive event: TerminalEvent) {}
    public func terminalView(_ view: TerminalView, shouldOpen url: URL) -> Bool { true }
    public func terminalView(_ view: TerminalView, shouldWriteClipboard text: String) -> Bool { false }
    public func terminalViewDidExit(_ view: TerminalView) {}
    public func terminalView(_ view: TerminalView, didCopy text: String) {}
    public func terminalView(_ view: TerminalView, isDropTarget: Bool) {}
    public func terminalView(_ view: TerminalView, willShowMenu menu: NSMenu) {}
}

/// How a `TerminalView` draws.
public enum TerminalRenderingBackend: Hashable, Sendable {
    /// Metal when a GPU is available, otherwise CoreGraphics.
    case automatic
    /// GPU rendering with a glyph atlas. Falls back to CoreGraphics without a Metal device.
    case metal
    /// CPU rendering with CoreGraphics and CoreText.
    case coreGraphics
}

/// An AppKit view that renders and drives a `TerminalSession`.
///
/// The view sizes the terminal grid to fit its bounds, redraws only damaged
/// cells, and handles keyboard (including IME), mouse reporting, scrollback,
/// selection, copy/paste, OSC 8 links, and Kitty graphics. It renders with
/// Metal by default.
@MainActor
public final class TerminalView: NSView {
    public let session: TerminalSession
    public weak var delegate: TerminalViewDelegate?
    /// The backend in use: `.metal` or `.coreGraphics`, never `.automatic`.
    public let renderingBackend: TerminalRenderingBackend

    /// Treat Option as Alt/Meta instead of typing composed characters.
    public var optionAsAlt = false
    /// Link schemes opened with Command-click, compared case-insensitively.
    ///
    /// OSC 8 links show text the program chooses, not their target, so the
    /// default excludes `file` and `ftp`. Even when `file` is allowed, apps,
    /// packages, scripts and executables are revealed in Finder, not opened.
    public var allowedLinkSchemes: Set<String> = ["http", "https", "mailto"]
    /// Open file paths printed in the text (`./src/main.rs`, `~/notes.md`,
    /// absolute paths) with Command-click, whatever `allowedLinkSchemes`
    /// says about `file`. Only paths that exist are links, relative ones
    /// resolved against the shell's working directory.
    public var opensFilePaths = true
    /// Play the system beep on BEL.
    public var audibleBell = true
    /// The pasteboard used for copy, paste and allowed OSC 52 writes.
    public var pasteboard: NSPasteboard = .general

    public private(set) var renderConfig: RenderConfig
    /// Follows the text it covers as output scrolls it or history is
    /// scrolled, and goes away when that text does (erased, gone from
    /// history, or a screen switch).
    public internal(set) var selection: TerminalSelection? {
        didSet {
            guard oldValue != selection else { return }
            invalidateSelection(oldValue)
            selectedText = selection.flatMap { selection in
                let frame = session.frame
                return selection.start.row >= 0 && selection.end.row < frame.rows ? selection.text(in: frame) : nil
            }
        }
    }
    /// The text the selection covered when last seen whole on screen.
    private var selectedText: String?
    /// The scrollback position of the frame the selection was placed on.
    private var shownHistory = 0
    private var shownOffset = 0

    let renderer = TerminalRenderer()
    let metalRenderer: MetalTerminalRenderer?
    var glyphPlan: [Int: GlyphPlanEntry] = [:]
    var images: [KittyImage] = []
    var markedText: String?
    var gesture = MouseGestureState()
    var cursorBlinkVisible = true
    /// Keys whose press went to the program, so only they report a release.
    var pressedKeyCodes: Set<UInt16> = []
    /// The link underlined under the pointer while Command is held.
    var hoveredLink: LinkHit? {
        didSet { hoveredLinkDidChange(from: oldValue) }
    }
    var linkUnderline: LinkUnderlineView?
    private var imageRevision: UInt64?
    /// Decoded images and the reload that last showed each. Images nothing
    /// shows are kept up to `idleImageBytes`, most recently shown first, so
    /// the terminal sends an animation's frames and a scrolled-away image once.
    private var decodedImages: [KittyImageKey: (image: CGImage, reload: UInt64)] = [:]
    private var imageReload: UInt64 = 0
    /// The terminal's cell size when `images` were read; zero before then.
    private var imageCellSize = CGSize.zero
    private static let idleImageBytes = 64 * 1024 * 1024
    private var blinkTimer: Timer?
    private var metalFrameScheduled = false
    /// The scheduled frame draws in this run-loop turn, not at the display link.
    private var metalFrameRunsThisTurn = false
    /// A frame was skipped because every frame in flight was on the GPU.
    private var metalFrameWaitsForGPU = false
    /// Paces Metal frames to the display: a `CADisplayLink` (macOS 14 and
    /// later), paused while idle.
    private var frameLink: NSObject?
    private var animationTimer: Timer?
    /// How long to wait before checking again for a frame that was due but
    /// hasn't arrived: an attached pane's frames come from its host.
    private var animationRetryDelay: TimeInterval = 0
    /// An animation frame came due while the window was hidden.
    private var animationPaused = false
    /// Render wakeups are off because nobody can see the view.
    private(set) var wakeupsPaused = false
    private var windowObservers: [NSObjectProtocol] = []

    /// Kitty animations are capped at this rate however short their frame delay.
    private static let minimumAnimationDelay: TimeInterval = 1.0 / 60
    private static let maximumAnimationRetryDelay: TimeInterval = 0.25

    /// Wraps an existing session.
    public init(session: TerminalSession, renderConfig: RenderConfig, backend: TerminalRenderingBackend = .automatic) {
        self.session = session
        self.renderConfig = renderConfig
        metalRenderer = backend == .coreGraphics ? nil : MetalTerminalRenderer()
        renderingBackend = metalRenderer == nil ? .coreGraphics : .metal
        super.init(frame: NSRect(x: 0, y: 0, width: 640, height: 400))
        renderer.configure(renderConfig)
        wantsLayer = true
        layerContentsRedrawPolicy = .onSetNeedsDisplay
        // The view moves drawn rows and glyph geometry along with scrolls.
        session.replaysScrolls = true
        session.onFrameChange = { [weak self] change in self?.frameDidChange(change) }
        session.onGraphicsChange = { [weak self] in self?.refreshImagesIfNeeded() }
        session.onEvent = { [weak self] event in self?.handle(event) }
        session.poll(forceFull: true)
        registerForDraggedTypes(Self.droppableTypes)
    }

    /// Spawns a shell configured by `config` (the user's Termy config by default).
    public convenience init(
        config: TermyConfig? = nil, options: TerminalOptions = TerminalOptions(),
        backend: TerminalRenderingBackend = .automatic
    ) throws {
        let config = try config ?? TermyConfig.loadDefault()
        let appearance: Appearance = NSApp?.effectiveAppearance.isDark == false ? .light : .dark
        let renderConfig = try config.renderConfig(for: appearance)
        var options = options
        options.config = options.config ?? config
        let size = TerminalSize(cols: 80, rows: 24, cellWidth: renderConfig.cellWidth, cellHeight: renderConfig.cellHeight)
        self.init(session: try TerminalSession(size: size, options: options), renderConfig: renderConfig, backend: backend)
        optionAsAlt = (try? config.native().macosOptionAsAlt) ?? false
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    isolated deinit {
        blinkTimer?.invalidate()
        animationTimer?.invalidate()
        stopDisplayLink()
        windowObservers.forEach { NotificationCenter.default.removeObserver($0) }
        session.stop()
    }

    public override var isFlipped: Bool { true }
    public override var acceptsFirstResponder: Bool { true }
    public override var isOpaque: Bool { renderConfig.backgroundOpacity >= 1 }
    public override var wantsUpdateLayer: Bool { metalRenderer != nil }

    public override func makeBackingLayer() -> CALayer {
        guard let metalRenderer else { return super.makeBackingLayer() }
        let layer = CAMetalLayer()
        metalRenderer.configure(layer)
        return layer
    }

    /// The grid size that fits the current bounds. Its cell size is in the
    /// window's device pixels, which is what programs measure images in: on
    /// a Retina display they then send, and the view draws, every pixel.
    public var fittingGridSize: TerminalSize {
        var size = TerminalSize.fitting(width: bounds.width, height: bounds.height, renderConfig: renderConfig)
        let scale = Float(window?.backingScaleFactor ?? 1)
        size.cellWidth *= scale
        size.cellHeight *= scale
        return size
    }

    /// The terminal's pixels per point, by axis: the cell size its images
    /// were laid out for over the cell size drawn. Images are drawn with it,
    /// so they cover the cells the terminal counted, whoever sized it.
    private var imagePixelsPerPoint: CGSize {
        guard renderConfig.cellWidth > 0, renderConfig.cellHeight > 0, imageCellSize.width > 0, imageCellSize.height > 0
        else { return CGSize(width: 1, height: 1) }
        return CGSize(
            width: imageCellSize.width / CGFloat(renderConfig.cellWidth),
            height: imageCellSize.height / CGFloat(renderConfig.cellHeight))
    }

    /// The view size that fits a grid exactly, for sizing windows.
    public func size(cols: Int, rows: Int) -> NSSize {
        NSSize(
            width: CGFloat(cols) * CGFloat(renderConfig.cellWidth) + 2 * CGFloat(renderConfig.paddingX),
            height: CGFloat(rows) * CGFloat(renderConfig.cellHeight) + 2 * CGFloat(renderConfig.paddingY))
    }

    /// Applies new render settings, such as after an appearance change.
    public func apply(_ config: RenderConfig) {
        renderConfig = config
        renderer.configure(config)
        resizeGridToFit()
        restartBlinkTimer()
        rebuildGlyphPlan(spans: nil)
        metalRenderer?.invalidateAll()
        needsDisplay = true
    }

    // MARK: - Layout and drawing

    public override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        resizeGridToFit()
    }

    public override func viewDidChangeBackingProperties() {
        super.viewDidChangeBackingProperties()
        layer?.contentsScale = window?.backingScaleFactor ?? NSScreen.main?.backingScaleFactor ?? 2
        // The cell size in device pixels changed with the display.
        resizeGridToFit()
        needsDisplay = true
    }

    /// Resizes the grid to the view, telling the program. Only for a view
    /// that's showing in a visible window that isn't holding its grids:
    /// layout passes before a window opens, or of a hidden pane, would
    /// otherwise send full-screen programs sizes they never get to show.
    private func resizeGridToFit() {
        guard bounds.width > 0, bounds.height > 0, let window, window.isVisible, !isHiddenOrHasHiddenAncestor,
              (window as? TerminalGridHolding)?.holdsTerminalGrids != true
        else { return }
        try? session.resize(fittingGridSize)
    }

    /// Fits the grid to the view now, such as after its window stops holding grids.
    public func fitGridToView() { resizeGridToFit() }

    public override func viewDidUnhide() {
        super.viewDidUnhide()
        resizeGridToFit()
        updateWakeups()
        // Frames were skipped while hidden.
        needsDisplay = true
    }

    public override func viewDidHide() {
        super.viewDidHide()
        updateWakeups()
    }

    /// A view nobody can see (out of a window, hidden, or in a hidden or
    /// covered window) takes no frames: a session host stops sending them and
    /// a local terminal stops waking the main thread for output, while
    /// titles, status and exit still arrive. Showing it again catches up.
    private func updateWakeups() {
        let visible = isOnScreen && !isHiddenOrHasHiddenAncestor
        guard visible == wakeupsPaused else { return }
        wakeupsPaused = !visible
        try? session.terminal.setWakeupEnabled(visible)
        if visible { needsDisplay = true }
    }

    var renderState: RenderState {
        RenderState(
            frame: session.frame, config: renderConfig, glyphPlan: glyphPlan, selection: selection,
            images: images, imagePixelsPerPoint: imagePixelsPerPoint, isFocused: hasKeyboardFocus,
            cursorBlinkVisible: cursorBlinkVisible, markedText: markedText)
    }

    /// Whether the view is first responder in the key window.
    public var hasKeyboardFocus: Bool {
        window?.isKeyWindow == true && window?.firstResponder === self
    }

    public override func draw(_ dirtyRect: NSRect) {
        guard let context = NSGraphicsContext.current?.cgContext else { return }
        renderer.draw(renderState, in: context, dirtyRect: dirtyRect, bounds: bounds)
    }

    // AppKit doesn't run its display cycle for a CAMetalLayer, so invalidation
    // schedules a Metal frame directly, coalesced to one per run-loop turn.
    public override var needsDisplay: Bool {
        get { super.needsDisplay }
        set {
            super.needsDisplay = newValue
            if newValue { scheduleMetalFrame() }
        }
    }

    public override func setNeedsDisplay(_ invalidRect: NSRect) {
        super.setNeedsDisplay(invalidRect)
        scheduleMetalFrame()
    }

    public override func updateLayer() {
        renderMetalFrame()
    }

    /// Frames wait for the display's next refresh, so every pane of a window
    /// draws once per refresh, in one Core Animation commit, however often
    /// its program redraws. A frame that changes the view's size draws in
    /// this run-loop turn instead, together with the new size.
    private func scheduleMetalFrame() {
        guard metalRenderer != nil else { return }
        let resizing = inLiveResize || drawableSizeChanges
        // A frame waiting for the display link still draws now when the size changes.
        guard !metalFrameScheduled || (resizing && !metalFrameRunsThisTurn) else { return }
        metalFrameScheduled = true
        if resizing || !startDisplayLink() {
            metalFrameRunsThisTurn = true
            // Common modes keep frames coming during live resize and mouse tracking.
            RunLoop.main.perform(inModes: [.common]) { [weak self] in
                MainActor.assumeIsolated { self?.renderScheduledMetalFrame() }
            }
        }
    }

    private func renderScheduledMetalFrame() {
        guard metalFrameScheduled else { return }
        metalFrameScheduled = false
        metalFrameRunsThisTurn = false
        renderMetalFrame()
    }

    /// The drawable size that fits the view now.
    private var fittingDrawableSize: CGSize {
        let scale = layer?.contentsScale ?? 1
        return CGSize(width: (bounds.width * scale).rounded(), height: (bounds.height * scale).rounded())
    }

    private var drawableSizeChanges: Bool {
        (layer as? CAMetalLayer)?.drawableSize != fittingDrawableSize
    }

    private func renderMetalFrame() {
        // Hidden windows and views skip frames; becoming visible redraws.
        guard let metalRenderer, let layer = layer as? CAMetalLayer, isOnScreen, !isHiddenOrHasHiddenAncestor
        else { return }
        let size = fittingDrawableSize
        let resized = layer.drawableSize != size
        if resized { layer.drawableSize = size }
        layer.isOpaque = isOpaque
        let drawn = metalRenderer.draw(
            renderState, in: layer, synchronized: resized || inLiveResize,
            onFrameCompleted: { [weak self] in
                DispatchQueue.main.async { MainActor.assumeIsolated { self?.metalSlotFreed() } }
            })
        // Every frame in flight is still on the GPU: draw when one finishes.
        if !drawn { metalFrameWaitsForGPU = true }
    }

    private func metalSlotFreed() {
        guard metalFrameWaitsForGPU else { return }
        metalFrameWaitsForGPU = false
        scheduleMetalFrame()
    }

    // MARK: - Display link

    /// Starts the display link, or returns false where there is none (before
    /// macOS 14, or outside a window).
    private func startDisplayLink() -> Bool {
        guard #available(macOS 14.0, *), window != nil else { return false }
        let link: CADisplayLink
        if let existing = frameLink as? CADisplayLink {
            link = existing
        } else {
            link = displayLink(target: DisplayLinkTarget(self), selector: #selector(DisplayLinkTarget.tick(_:)))
            link.add(to: .main, forMode: .common)
            frameLink = link
        }
        link.isPaused = false
        return true
    }

    fileprivate func displayLinkTick() {
        // Idle views don't keep the display link running.
        if #available(macOS 14.0, *) { (frameLink as? CADisplayLink)?.isPaused = true }
        renderScheduledMetalFrame()
    }

    private func stopDisplayLink() {
        if #available(macOS 14.0, *) { (frameLink as? CADisplayLink)?.invalidate() }
        frameLink = nil
    }

    private func frameDidChange(_ change: FrameChange) {
        if let hovered = hoveredLink, linkMayHaveMoved(hovered.link, by: change) { hoveredLink = nil }
        followSelection(change)
        switch change.damage {
        case .full:
            rebuildGlyphPlan(spans: nil)
            metalRenderer?.invalidateAll()
            needsDisplay = true
        case .partial(let spans):
            if !change.scrolls.isEmpty {
                // Move what was built for the rows that scrolled, instead
                // of building every row again.
                moveGlyphPlan(change.scrolls)
                metalRenderer?.replay(change.scrolls, rowCount: session.frame.rows)
            }
            rebuildGlyphPlan(spans: spans)
            metalRenderer?.invalidate(rows: spans.map(\.row))
            if change.resized || change.viewportChanged || !change.scrolls.isEmpty {
                needsDisplay = true
            } else {
                // Repaint whole rows: wide glyphs and glyph geometry can overhang.
                for row in Set(spans.map(\.row)) {
                    setNeedsDisplay(renderer.rowRect(row, cols: session.frame.cols))
                }
            }
        case .none:
            if !change.scrolls.isEmpty {
                moveGlyphPlan(change.scrolls)
                metalRenderer?.replay(change.scrolls, rowCount: session.frame.rows)
            }
            if change.resized || change.viewportChanged || !change.scrolls.isEmpty { needsDisplay = true }
        }
        if change.cursorChanged {
            invalidateCursor(change.previousCursor)
            invalidateCursor(change.cursor)
            resetCursorBlink()
        }
        refreshImagesIfNeeded()
    }

    /// Moves the selection with the text it covers. Rows count from the top
    /// of the viewport; a selection may reach above it into history.
    private func followSelection(_ change: FrameChange) {
        let frame = session.frame
        defer {
            shownHistory = frame.historySize
            shownOffset = frame.displayOffset
        }
        guard let current = selection else { return }
        guard !change.resized else { return selection = nil }
        // History rows above the viewport's top moved by this many: output
        // scrolling into history, or the viewport scrolling through it.
        let historyShift = (frame.historySize - frame.displayOffset) - (shownHistory - shownOffset)
        var intoHistory = 0
        var saturated = false
        var moved: [ViewportScroll] = []
        for scroll in change.scrolls {
            if scroll.top == 0, scroll.bottom == frame.rows - 1, scroll.direction == .up, frame.displayOffset == 0 {
                intoHistory += scroll.count
                // Everything left the screen; how far isn't known.
                saturated = saturated || scroll.count == frame.rows
            } else {
                moved.append(scroll)
            }
        }
        // Lines a full history let go are counted by the scrolls, not the size.
        let shift = historyShift + max(0, intoHistory - (frame.historySize - shownHistory))
        func follow(_ position: GridPosition) -> GridPosition? {
            var row = position.row
            for scroll in moved {
                guard let destination = scroll.destination(ofRow: row) else { return nil }
                row = destination
            }
            return GridPosition(row: row - shift, col: position.col)
        }
        guard let anchor = follow(current.anchor), let head = follow(current.head) else { return selection = nil }
        var next = current
        next.anchor = anchor
        next.head = head
        let top = -(frame.historySize - frame.displayOffset)
        // Gone from history, or below the screen, or too far to tell.
        if next.end.row < top || next.start.row >= frame.rows || (saturated && next.end.row < 0) {
            return selection = nil
        }
        // Still on screen: the text must be what was selected.
        let touched: Bool = switch change.damage {
        case .full: true
        case .partial(let spans): shift != 0 || spans.contains { next.rows.contains($0.row) }
        case .none: shift != 0
        }
        if touched, let selectedText, next.start.row >= 0, next.end.row < frame.rows,
           next.text(in: frame) != selectedText {
            return selection = nil
        }
        if let anchor = gesture.selectionAnchor { gesture.selectionAnchor = follow(anchor) }
        if next != current {
            let text = selectedText
            selection = next
            // A selection that scrolled partly off screen keeps the text it had.
            if selectedText == nil { selectedText = text }
        }
    }

    /// Whether output or scrolling may have changed the text under a link.
    private func linkMayHaveMoved(_ link: TerminalLink, by change: FrameChange) -> Bool {
        if change.resized || change.viewportChanged || !change.scrolls.isEmpty { return true }
        switch change.damage {
        case .full: return true
        case .partial(let spans): return spans.contains { (link.start.row...link.end.row).contains($0.row) }
        case .none: return false
        }
    }

    func invalidateCursor(_ cursor: TerminalCursor) {
        setNeedsDisplay(renderer.cellRect(row: cursor.row, col: cursor.col, width: 2).insetBy(dx: -1, dy: -1))
    }

    /// Where the selection highlight is drawn, one rectangle per row, in this
    /// view's (flipped) coordinates. Empty without a selection.
    public var selectionRects: [CGRect] {
        let frame = session.frame
        guard let selection = selection?.expanded(in: frame), !frame.isEmpty else { return [] }
        let rows = max(0, selection.start.row)...min(frame.rows - 1, selection.end.row)
        guard !rows.isEmpty else { return [] }
        return rows.map { row in
            let from = row == selection.start.row ? selection.start.col : 0
            let to = row == selection.end.row ? selection.end.col : frame.cols - 1
            return renderer.cellRect(row: row, col: from, width: max(1, to - from + 1))
        }
    }

    /// The cursor's cell in this view, or nil while the view is scrolled back
    /// from it. Hosts anchor popups at the prompt with it.
    public var cursorRect: CGRect? {
        let frame = session.frame
        guard !frame.isEmpty, frame.displayOffset == 0 else { return nil }
        let cursor = frame.cursor
        return renderer.cellRect(row: max(0, min(cursor.row, frame.rows - 1)), col: max(0, min(cursor.col, frame.cols - 1)))
    }

    /// Where a search match is drawn in this view, or nil while it is
    /// scrolled out of the viewport.
    public func rect(for match: SearchMatch) -> CGRect? {
        let frame = session.frame
        guard !frame.isEmpty,
              let row = match.viewportRow(historySize: frame.historySize, displayOffset: frame.displayOffset, rows: frame.rows)
        else { return nil }
        let start = max(0, min(match.startCol, frame.cols - 1))
        let end = max(start, min(match.endCol, frame.cols - 1))
        return renderer.cellRect(row: row, col: start, width: end - start + 1)
    }

    /// Scrolls history so `match` is in view, near the middle when it was
    /// off screen. Does nothing when it is already visible.
    public func reveal(_ match: SearchMatch) {
        let frame = session.frame
        guard !frame.isEmpty, rect(for: match) == nil else { return }
        let target = max(0, min(frame.historySize, frame.rows / 2 - match.row + frame.historySize))
        session.scroll(lines: Int32(target - frame.displayOffset))
    }

    private func invalidateSelection(_ previous: TerminalSelection?) {
        let rows = [previous?.rows, selection?.rows].compactMap { $0 }
        for range in rows {
            for row in range { setNeedsDisplay(renderer.rowRect(row, cols: session.frame.cols)) }
        }
    }

    /// Moves the special-glyph geometry of rows that scrolled with them. A
    /// row's geometry depends only on that row's cells.
    private func moveGlyphPlan(_ scrolls: [ViewportScroll]) {
        let cols = session.frame.cols
        guard cols > 0 else { return }
        for scroll in scrolls {
            var moved: [Int: GlyphPlanEntry] = [:]
            moved.reserveCapacity(glyphPlan.count)
            for (index, entry) in glyphPlan {
                guard let row = scroll.destination(ofRow: index / cols) else { continue }
                var entry = entry
                entry.cellIndex = row * cols + index % cols
                moved[entry.cellIndex] = entry
            }
            glyphPlan = moved
        }
    }

    /// Libtermy's special-glyph geometry, rebuilt for whole changed rows
    /// because Braille and box-drawing runs depend on their neighbors.
    private func rebuildGlyphPlan(spans: [DirtySpan]?) {
        let frame = session.frame
        guard !frame.isEmpty else { return }
        if let spans {
            let rows = Set(spans.map(\.row))
            glyphPlan = glyphPlan.filter { !rows.contains($0.key / frame.cols) }
            let rowSpans = rows.map { DirtySpan(row: $0, leftCol: 0, rightCol: frame.cols - 1) }
            for entry in (try? frame.glyphPlan(metrics: renderConfig.glyphMetrics, spans: rowSpans)) ?? [] {
                glyphPlan[entry.cellIndex] = entry
            }
        } else {
            let entries = (try? frame.glyphPlan(metrics: renderConfig.glyphMetrics)) ?? []
            glyphPlan = Dictionary(entries.map { ($0.cellIndex, $0) }, uniquingKeysWith: { _, new in new })
        }
    }

    // MARK: - Events

    private func handle(_ event: TerminalEvent) {
        switch event {
        case .title, .resetTitle:
            delegate?.terminalView(self, didChangeTitle: session.title)
        case .bell:
            if audibleBell { NSSound.beep() }
        case .clipboardStore(let text):
            if delegate?.terminalView(self, shouldWriteClipboard: text) == true {
                pasteboard.clearContents()
                pasteboard.setString(text, forType: .string)
            }
        case .exit:
            delegate?.terminalViewDidExit(self)
        default:
            break
        }
        delegate?.terminalView(self, didReceive: event)
    }

    // MARK: - Kitty graphics

    private func refreshImagesIfNeeded() {
        guard let revision = try? session.terminal.kittyGraphicsRevision(), revision != imageRevision else { return }
        imageRevision = revision
        reloadImages()
    }

    private func reloadImages() {
        guard let snapshot = try? session.terminal.kittyGraphics(holding: Set(decodedImages.keys)) else { return }
        imageReload &+= 1
        imageCellSize = CGSize(width: CGFloat(snapshot.cellWidth), height: CGFloat(snapshot.cellHeight))
        images = snapshot.placements.compactMap { placement in
            let key = placement.imageKey
            guard let image = decodedImages[key]?.image ?? Self.decode(placement) else { return nil }
            decodedImages[key] = (image, imageReload)
            return KittyImage(placement: placement, image: image)
        }
        evictIdleImages()
        needsDisplay = true
        animationRetryDelay = 0
        scheduleAnimation(after: snapshot.nextFrameDelay.map { Double($0 + 1) / 1000 })
    }

    /// Drops the least recently shown images nothing shows now, once they
    /// hold more than `idleImageBytes`.
    private func evictIdleImages() {
        var idle = decodedImages.filter { $0.value.reload != imageReload }
            .map { (key: $0.key, reload: $0.value.reload, bytes: $0.value.image.width * $0.value.image.height * 4) }
        var bytes = idle.reduce(0) { $0 + $1.bytes }
        guard bytes > Self.idleImageBytes else { return }
        idle.sort { $0.reload < $1.reload }
        for entry in idle where bytes > Self.idleImageBytes {
            decodedImages[entry.key] = nil
            bytes -= entry.bytes
        }
    }

    /// Wakes for the next animation frame, `delay` seconds from now.
    private func scheduleAnimation(after delay: TimeInterval?) {
        animationTimer?.invalidate()
        animationTimer = nil
        animationPaused = false
        guard let delay else { return }
        // Don't advance animations nobody can see; resume when visible.
        guard isOnScreen else {
            animationPaused = true
            return
        }
        let timer = Timer(timeInterval: max(delay, Self.minimumAnimationDelay), repeats: false) { [weak self] _ in
            MainActor.assumeIsolated { self?.animationFrameIsDue() }
        }
        // Common modes keep animations running during scrolling and menus.
        RunLoop.main.add(timer, forMode: .common)
        animationTimer = timer
    }

    private func animationFrameIsDue() {
        animationTimer = nil
        let shown = imageRevision
        // Asking for the revision advances a local terminal's animations.
        refreshImagesIfNeeded()
        guard imageRevision == shown, animationTimer == nil else { return }
        // No new frame yet: an attached pane's host sends it, which reloads
        // the images by itself. Look again later, less and less often, and
        // then ask whether the animation is still running at all.
        guard animationRetryDelay < Self.maximumAnimationRetryDelay else { return reloadImages() }
        animationRetryDelay = min(
            max(animationRetryDelay * 2, Self.minimumAnimationDelay), Self.maximumAnimationRetryDelay)
        scheduleAnimation(after: animationRetryDelay)
    }

    private static func decode(_ placement: KittyGraphicsPlacement) -> CGImage? {
        if let rgba = placement.rgba {
            return image(rgba: rgba, width: placement.imageWidth, height: placement.imageHeight)
        }
        guard !placement.png.isEmpty, let provider = CGDataProvider(data: placement.png as CFData) else { return nil }
        return CGImage(pngDataProviderSource: provider, decode: nil, shouldInterpolate: true, intent: .defaultIntent)
    }

    /// An image over straight-alpha RGBA8 pixels, without copying them.
    private static func image(rgba: Data, width: Int, height: Int) -> CGImage? {
        guard width > 0, height > 0, rgba.count == width * height * 4,
              let provider = CGDataProvider(data: rgba as CFData) else { return nil }
        return CGImage(
            width: width, height: height, bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: width * 4,
            space: CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB(),
            bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.last.rawValue),
            provider: provider, decode: nil, shouldInterpolate: true, intent: .defaultIntent)
    }

    // MARK: - Focus and blink

    public override func becomeFirstResponder() -> Bool {
        // AppKit records the new first responder only after this returns.
        restartBlinkTimer(becomingFirstResponder: true)
        invalidateCursor(session.frame.cursor)
        return true
    }

    public override func resignFirstResponder() -> Bool {
        // Releases of keys still held go to whatever takes the focus.
        pressedKeyCodes.removeAll()
        blinkTimer?.invalidate()
        blinkTimer = nil
        cursorBlinkVisible = true
        invalidateCursor(session.frame.cursor)
        return true
    }

    public override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        // A view's display link follows the screen of its window.
        stopDisplayLink()
        if metalFrameScheduled {
            metalFrameScheduled = false
            metalFrameRunsThisTurn = false
            if window != nil { scheduleMetalFrame() }
        }
        if let window { layer?.contentsScale = window.backingScaleFactor }
        observeWindow()
        resizeGridToFit()
        restartBlinkTimer()
        updateWakeups()
    }

    /// Whether the view is in a window that is at least partly visible.
    var isOnScreen: Bool {
        guard let window else { return false }
        return window.occlusionState.contains(.visible)
    }

    /// Whether the cursor blink timer is running.
    var isBlinking: Bool { blinkTimer != nil }

    /// Tracks key and visibility changes so idle views stop blinking,
    /// animating and rendering, and catch up when they come back.
    private func observeWindow() {
        windowObservers.forEach { NotificationCenter.default.removeObserver($0) }
        windowObservers = []
        guard let window else { return }
        let names: [Notification.Name] = [
            NSWindow.didBecomeKeyNotification, NSWindow.didResignKeyNotification,
            NSWindow.didChangeOcclusionStateNotification,
        ]
        windowObservers = names.map { name in
            NotificationCenter.default.addObserver(forName: name, object: window, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated { self?.windowStateDidChange() }
            }
        }
    }

    private func windowStateDidChange() {
        // Opening (or coming back from the Dock) makes the window visible.
        resizeGridToFit()
        restartBlinkTimer()
        invalidateCursor(session.frame.cursor)
        updateWakeups()
        if isOnScreen {
            needsDisplay = true
            if animationPaused { reloadImages() }
        }
    }

    private func restartBlinkTimer(becomingFirstResponder: Bool = false) {
        blinkTimer?.invalidate()
        blinkTimer = nil
        cursorBlinkVisible = true
        // An unfocused cursor is drawn hollow and never blinks.
        let focused = window?.isKeyWindow == true && (becomingFirstResponder || window?.firstResponder === self)
        guard renderConfig.cursorBlink, focused, isOnScreen else { return }
        blinkTimer = Timer.scheduledTimer(withTimeInterval: 0.53, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self else { return }
                self.cursorBlinkVisible.toggle()
                self.invalidateCursor(self.session.frame.cursor)
            }
        }
    }

    private func resetCursorBlink() {
        guard renderConfig.cursorBlink, !cursorBlinkVisible || blinkTimer != nil else { return }
        restartBlinkTimer()
    }

    // MARK: - Links

    enum LinkAction: Equatable {
        case open(URL)
        /// Show the file in Finder instead of launching it.
        case reveal(URL)

        var url: URL {
            switch self {
            case .open(let url), .reveal(let url): url
            }
        }
    }

    /// A link under the pointer that Command-click would open.
    struct LinkHit: Equatable {
        var link: TerminalLink
        var action: LinkAction
    }

    /// What Command-clicking the cell would do, or nil when there is no link
    /// or it is blocked.
    func linkAction(at position: GridPosition) -> LinkAction? {
        linkHit(at: position)?.action
    }

    /// The link under a cell, OSC 8 or found in the text, if it may be opened.
    func linkHit(at position: GridPosition) -> LinkHit? {
        guard let link = try? session.terminal.link(at: position),
              let target = Self.url(fromUntrusted: link.uri)
        else { return nil }
        let scheme = target.scheme?.lowercased()
        // A path found in the text shows exactly what opens; an OSC 8 target
        // is hidden behind text the program chose, so it needs `file` allowed.
        let isHyperlink = (try? session.terminal.hyperlink(at: position)) != nil
        let allowed = allowedLinkSchemes.contains(where: { $0.lowercased() == scheme })
            || (scheme == "file" && opensFilePaths && !isHyperlink)
        guard allowed, delegate?.terminalView(self, shouldOpen: target) ?? true else { return nil }
        let action: LinkAction = target.isFileURL && Self.isLaunchable(target) ? .reveal(target) : .open(target)
        return LinkHit(link: link, action: action)
    }

    /// The parsed OSC 8 target under a cell, if it is a well-formed URL.
    func linkTarget(at position: GridPosition) -> URL? {
        guard let link = try? session.terminal.hyperlink(at: position) else { return nil }
        return Self.url(fromUntrusted: link.uri)
    }

    private static func url(fromUntrusted uri: String) -> URL? {
        guard !uri.unicodeScalars.contains(where: { $0.value < 0x20 || (0x7F...0x9F).contains($0.value) })
        else { return nil }
        return URL(string: uri)
    }

    func openLink(at position: GridPosition) -> Bool {
        guard let action = linkAction(at: position) else { return false }
        open(action)
        return true
    }

    func open(_ action: LinkAction) {
        switch action {
        case .open(let url): NSWorkspace.shared.open(url)
        case .reveal(let url): NSWorkspace.shared.activateFileViewerSelecting([url])
        }
    }

    /// Underlines the link under the pointer while Command is held, and
    /// shows the pointing hand, as a cue that clicking opens it.
    func updateHoveredLink(at position: GridPosition?, modifiers: NSEvent.ModifierFlags) {
        guard let position, modifiers.contains(.command), !modifiers.contains(.option) else {
            hoveredLink = nil
            return
        }
        if let hoveredLink, hoveredLink.link.contains(position) { return }
        hoveredLink = linkHit(at: position)
    }

    /// The cell under the pointer, or nil when it is outside the view.
    func pointerPosition() -> GridPosition? {
        guard let window else { return nil }
        let point = convert(window.mouseLocationOutsideOfEventStream, from: nil)
        guard bounds.contains(point) else { return nil }
        let frame = session.frame
        return renderer.position(at: point, cols: frame.cols, rows: frame.rows)
    }

    func hoveredLinkDidChange(from old: LinkHit?) {
        guard old != hoveredLink else { return }
        let rects = hoveredLink.map { underlineRects(for: $0.link) } ?? []
        if rects.isEmpty {
            linkUnderline?.removeFromSuperview()
            linkUnderline = nil
        } else {
            let underline = linkUnderline ?? LinkUnderlineView(frame: bounds)
            if linkUnderline == nil {
                underline.autoresizingMask = [.width, .height]
                addSubview(underline)
                linkUnderline = underline
            }
            underline.frame = bounds
            underline.color = renderer.color(renderConfig.foreground)
            underline.rects = rects
        }
        if (old == nil) != (hoveredLink == nil) {
            (hoveredLink == nil ? NSCursor.arrow : NSCursor.pointingHand).set()
        }
    }

    /// One underline per row of the link, at the font's underline position.
    private func underlineRects(for link: TerminalLink) -> [CGRect] {
        let frame = session.frame
        guard !frame.isEmpty else { return [] }
        let first = max(0, link.start.row), last = min(frame.rows - 1, link.end.row)
        guard first <= last else { return [] }
        let baselineOffset = renderer.fonts.baselineOffset(cellHeight: renderer.cellHeight)
        return (first...last).map { row in
            let from = row == link.start.row ? link.start.col : 0
            let to = row == link.end.row ? link.end.col : frame.cols - 1
            let cell = renderer.cellRect(row: row, col: from, width: max(1, to - from + 1))
            let y = (cell.minY + baselineOffset + max(renderer.fonts.underlinePosition, 1)).rounded()
            return CGRect(x: cell.minX, y: y, width: cell.width, height: 1)
        }
    }

    /// Extensions macOS opens by running something: apps, installers,
    /// scripts, and files that launch another target.
    private static let launchableExtensions: Set<String> = [
        "app", "action", "command", "fileloc", "inetloc", "jar", "mpkg", "pkg", "prefpane",
        "scpt", "scptd", "shortcut", "terminal", "tool", "webloc", "workflow",
    ]

    /// Whether opening a file URL could run code instead of showing a document.
    static func isLaunchable(_ url: URL) -> Bool {
        let url = url.resolvingSymlinksInPath()
        if launchableExtensions.contains(url.pathExtension.lowercased()) { return true }
        let values = try? url.resourceValues(forKeys: [.isApplicationKey, .isPackageKey, .isDirectoryKey, .isExecutableKey])
        if values?.isApplication == true || values?.isPackage == true { return true }
        return values?.isDirectory == false && values?.isExecutable == true
    }
}

extension NSAppearance {
    var isDark: Bool { bestMatch(from: [.darkAqua, .aqua]) == .darkAqua }
}
#endif

#if os(macOS)
/// The display link's target. A display link retains its target, so it
/// holds the view weakly, and the view invalidates the link when it goes.
@MainActor
private final class DisplayLinkTarget: NSObject {
    weak var view: TerminalView?

    init(_ view: TerminalView) {
        self.view = view
    }

    @available(macOS 14.0, *)
    @objc func tick(_ link: CADisplayLink) {
        guard let view else {
            link.invalidate()
            return
        }
        view.displayLinkTick()
    }
}
#endif

#if os(macOS)
/// Draws the underline of the link under the pointer above the terminal.
final class LinkUnderlineView: NSView {
    var color: CGColor = .white { didSet { needsDisplay = true } }
    var rects: [CGRect] = [] { didSet { if rects != oldValue { needsDisplay = true } } }

    override var isFlipped: Bool { true }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    override func draw(_ dirtyRect: NSRect) {
        guard let context = NSGraphicsContext.current?.cgContext else { return }
        context.setFillColor(color)
        context.fill(rects)
    }
}
#endif
