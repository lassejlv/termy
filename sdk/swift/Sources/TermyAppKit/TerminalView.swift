#if os(macOS)
import AppKit
import Termy

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
}

extension TerminalViewDelegate {
    public func terminalView(_ view: TerminalView, didChangeTitle title: String?) {}
    public func terminalView(_ view: TerminalView, didReceive event: TerminalEvent) {}
    public func terminalView(_ view: TerminalView, shouldOpen url: URL) -> Bool { true }
    public func terminalView(_ view: TerminalView, shouldWriteClipboard text: String) -> Bool { false }
    public func terminalViewDidExit(_ view: TerminalView) {}
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
    /// Play the system beep on BEL.
    public var audibleBell = true
    /// The pasteboard used for copy, paste and allowed OSC 52 writes.
    public var pasteboard: NSPasteboard = .general

    public private(set) var renderConfig: RenderConfig
    public internal(set) var selection: TerminalSelection? {
        didSet { if oldValue != selection { invalidateSelection(oldValue) } }
    }

    let renderer = TerminalRenderer()
    let metalRenderer: MetalTerminalRenderer?
    var glyphPlan: [Int: GlyphPlanEntry] = [:]
    var images: [KittyImage] = []
    var markedText: String?
    var gesture = MouseGestureState()
    var cursorBlinkVisible = true
    private var imageRevision: UInt64?
    private var decodedImages: [String: CGImage] = [:]
    private var blinkTimer: Timer?
    private var metalFrameScheduled = false
    private var animationTimer: Timer?

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
        session.onFrameChange = { [weak self] change in self?.frameDidChange(change) }
        session.onEvent = { [weak self] event in self?.handle(event) }
        session.poll(forceFull: true)
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

    /// The grid size that fits the current bounds.
    public var fittingGridSize: TerminalSize {
        TerminalSize.fitting(width: bounds.width, height: bounds.height, renderConfig: renderConfig)
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
        needsDisplay = true
    }

    private func resizeGridToFit() {
        guard bounds.width > 0, bounds.height > 0 else { return }
        try? session.resize(fittingGridSize)
    }

    var renderState: RenderState {
        RenderState(
            frame: session.frame, config: renderConfig, glyphPlan: glyphPlan, selection: selection,
            images: images, isFocused: hasKeyboardFocus,
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

    private func scheduleMetalFrame() {
        guard metalRenderer != nil, !metalFrameScheduled else { return }
        metalFrameScheduled = true
        // Common modes keep frames coming during live resize and mouse tracking.
        RunLoop.main.perform(inModes: [.common]) { [weak self] in
            MainActor.assumeIsolated {
                guard let self else { return }
                self.metalFrameScheduled = false
                self.renderMetalFrame()
            }
        }
    }

    private func renderMetalFrame() {
        guard let metalRenderer, let layer = layer as? CAMetalLayer, window != nil else { return }
        let scale = layer.contentsScale
        let size = CGSize(width: (bounds.width * scale).rounded(), height: (bounds.height * scale).rounded())
        if layer.drawableSize != size { layer.drawableSize = size }
        layer.isOpaque = isOpaque
        metalRenderer.draw(renderState, in: layer)
    }

    private func frameDidChange(_ change: FrameChange) {
        switch change.damage {
        case .full:
            rebuildGlyphPlan(spans: nil)
            metalRenderer?.invalidateAll()
            needsDisplay = true
        case .partial(let spans):
            rebuildGlyphPlan(spans: spans)
            metalRenderer?.invalidate(rows: spans.map(\.row))
            if change.resized || change.viewportChanged {
                needsDisplay = true
            } else {
                // Repaint whole rows: wide glyphs and glyph geometry can overhang.
                for row in Set(spans.map(\.row)) {
                    setNeedsDisplay(renderer.rowRect(row, cols: session.frame.cols))
                }
            }
        case .none:
            if change.resized || change.viewportChanged { needsDisplay = true }
        }
        if change.cursorChanged {
            invalidateCursor(change.previousCursor)
            invalidateCursor(change.cursor)
            resetCursorBlink()
        }
        refreshImagesIfNeeded()
    }

    func invalidateCursor(_ cursor: TerminalCursor) {
        setNeedsDisplay(renderer.cellRect(row: cursor.row, col: cursor.col, width: 2).insetBy(dx: -1, dy: -1))
    }

    private func invalidateSelection(_ previous: TerminalSelection?) {
        let rows = [previous?.rows, selection?.rows].compactMap { $0 }
        for range in rows {
            for row in range { setNeedsDisplay(renderer.rowRect(row, cols: session.frame.cols)) }
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
        guard let snapshot = try? session.terminal.kittyGraphics() else { return }
        var live: [String: CGImage] = [:]
        images = snapshot.placements.compactMap { placement in
            let key = "\(placement.imageID):\(placement.imageGeneration)"
            guard let image = decodedImages[key] ?? Self.decodePNG(placement.png) else { return nil }
            live[key] = image
            return KittyImage(placement: placement, image: image)
        }
        decodedImages = live
        needsDisplay = true

        animationTimer?.invalidate()
        if let delay = snapshot.nextFrameDelay {
            animationTimer = Timer.scheduledTimer(withTimeInterval: Double(delay) / 1000, repeats: false) { [weak self] _ in
                MainActor.assumeIsolated { self?.reloadImages() }
            }
        }
    }

    private static func decodePNG(_ data: Data) -> CGImage? {
        guard let provider = CGDataProvider(data: data as CFData) else { return nil }
        return CGImage(pngDataProviderSource: provider, decode: nil, shouldInterpolate: true, intent: .defaultIntent)
    }

    // MARK: - Focus and blink

    public override func becomeFirstResponder() -> Bool {
        restartBlinkTimer()
        invalidateCursor(session.frame.cursor)
        return true
    }

    public override func resignFirstResponder() -> Bool {
        blinkTimer?.invalidate()
        cursorBlinkVisible = true
        invalidateCursor(session.frame.cursor)
        return true
    }

    public override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        if let window { layer?.contentsScale = window.backingScaleFactor }
        resizeGridToFit()
        restartBlinkTimer()
    }

    private func restartBlinkTimer() {
        blinkTimer?.invalidate()
        cursorBlinkVisible = true
        guard renderConfig.cursorBlink, window != nil else { return }
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
    }

    /// What Command-clicking the cell would do, or nil when the link is blocked.
    func linkAction(at position: GridPosition) -> LinkAction? {
        guard let target = linkTarget(at: position),
              allowedLinkSchemes.contains(where: { $0.lowercased() == target.scheme?.lowercased() }),
              delegate?.terminalView(self, shouldOpen: target) ?? true
        else { return nil }
        return target.isFileURL && Self.isLaunchable(target) ? .reveal(target) : .open(target)
    }

    /// The parsed OSC 8 target under a cell, if it is a well-formed URL.
    func linkTarget(at position: GridPosition) -> URL? {
        guard let link = try? session.terminal.hyperlink(at: position),
              !link.uri.unicodeScalars.contains(where: { $0.value < 0x20 || (0x7F...0x9F).contains($0.value) })
        else { return nil }
        return URL(string: link.uri)
    }

    func openLink(at position: GridPosition) -> Bool {
        switch linkAction(at: position) {
        case .open(let url): NSWorkspace.shared.open(url)
        case .reveal(let url): NSWorkspace.shared.activateFileViewerSelecting([url])
        case nil: return false
        }
        return true
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
