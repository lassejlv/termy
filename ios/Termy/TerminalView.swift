import SwiftUI
import TermyRemoteKit
import UIKit

/// Things the terminal asks its SwiftUI host to do, from ⌘ shortcuts and edge swipes.
enum TerminalCommand {
    case find, newTab, splitRight, splitDown, nextPane, previousPane, close
}

/// Draws a `TerminalScreen` and turns touches and keys into `ClientMessage`s.
///
/// Uses the Mac’s cell metrics and fits the host grid to the phone by default.
/// Restoring the Mac size scales that grid to fit without changing its columns.
final class TerminalUIView: UIView, UIKeyInput, UITextInputTraits, UIGestureRecognizerDelegate,
    @preconcurrency UIEditMenuInteractionDelegate
{
    var screen: TerminalScreen? {
        didSet {
            guard oldValue !== screen else { return }
            oldValue?.onChange = nil
            screen?.onChange = { [weak self] in self?.invalidateRows() }
            clearSelection()
            rowElements = nil
            setNeedsDisplay()
        }
    }

    var onKey: ((RemoteKey) -> Void)?
    var onPaste: ((String) -> Void)?
    var onScroll: ((Int) -> Void)?
    var onMouse: ((Int, Int, String) -> Void)?
    var onGridChange: ((Int, Int) -> Void)?
    var onCommand: ((TerminalCommand) -> Void)?
    /// Called when this view starts taking keys, so a side-by-side host knows which pane is focused.
    var onFocus: (() -> Void)?
    var mouseMode = false
    private var zoom: CGFloat = 1
    private var blinkTimer: Timer?
    private var cursorOn = true
    private var lastGrid: (Int, Int)?
    private var resizeWork: DispatchWorkItem?
    private static let imageCache: NSCache<NSData, UIImage> = {
        let cache = NSCache<NSData, UIImage>()
        cache.totalCostLimit = 12 * 1024 * 1024
        return cache
    }()

    /// Sticky modifiers from the key bar, applied to the next key (or every key, when locked).
    private(set) var controlState = ModifierState.off { didSet { keyBar.setModifiers(control: controlState, alt: altState) } }
    private(set) var altState = ModifierState.off { didSet { keyBar.setModifiers(control: controlState, alt: altState) } }
    private var lastModifierTap: (control: Bool, at: Date)?
    private lazy var keyBar = KeyBar(view: self)
    private var scrollRemainder: CGFloat = 0

    /// Hardware keys held down repeat; UIKit sends one `pressesBegan` per press.
    private let hardwareRepeater = KeyRepeater()
    private var repeatingKeyCode: UIKeyboardHIDUsage?

    // Selection, drawn on the grid.
    private var selection: (start: GridPoint, end: GridPoint)?
    private var selectionWord: (GridPoint, GridPoint)?
    private enum Handle { case start, end }
    private var draggingHandle: Handle?
    private lazy var editMenu = UIEditMenuInteraction(delegate: self)

    // Two fingers either zoom or move the cursor like a trackpad, decided when they start moving.
    private enum TwoFingerMode { case undecided, zoom, trackpad }
    private var twoFingerMode = TwoFingerMode.undecided
    private var pinchScale: CGFloat = 1
    private var trackpadRemainder = CGPoint.zero
    private let trackpad = UIPanGestureRecognizer()
    private let pinch = UIPinchGestureRecognizer()

    private var fontCache: [FontKey: UIFont] = [:]
    private var glyphSizes: [GlyphKey: CGSize] = [:]
    private var cachedFamily: String?
    private var rowElements: [UIAccessibilityElement]?

    private var predictiveEcho: Bool { UserDefaults.standard.object(forKey: "predictiveEcho") as? Bool ?? true }

    override init(frame: CGRect) {
        super.init(frame: frame)
        isOpaque = true
        contentMode = .redraw
        addGestureRecognizer(UITapGestureRecognizer(target: self, action: #selector(tapped(_:))))
        let pan = UIPanGestureRecognizer(target: self, action: #selector(panned(_:)))
        pan.maximumNumberOfTouches = 1
        addGestureRecognizer(pan)
        trackpad.minimumNumberOfTouches = 2
        trackpad.maximumNumberOfTouches = 2
        trackpad.addTarget(self, action: #selector(twoFingerPanned(_:)))
        trackpad.delegate = self
        addGestureRecognizer(trackpad)
        pinch.addTarget(self, action: #selector(pinched(_:)))
        pinch.delegate = self
        addGestureRecognizer(pinch)
        addGestureRecognizer(UILongPressGestureRecognizer(target: self, action: #selector(longPressed(_:))))
        for edge in [UIRectEdge.left, .right] {
            let swipe = UIScreenEdgePanGestureRecognizer(target: self, action: #selector(edgeSwiped(_:)))
            swipe.edges = edge
            addGestureRecognizer(swipe)
            pan.require(toFail: swipe)
        }
        addInteraction(editMenu)
        isAccessibilityElement = false
    }

    override func didMoveToWindow() {
        super.didMoveToWindow()
        if window == nil {
            blinkTimer?.invalidate()
            blinkTimer = nil
            resizeWork?.cancel()
            hardwareRepeater.stop()
        } else if blinkTimer == nil {
            startBlinking()
        }
    }

    private func startBlinking() {
        blinkTimer = Timer.scheduledTimer(withTimeInterval: 0.6, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self, self.window != nil else { return }
                if self.screen?.expirePredictions() == true { self.setNeedsDisplay() }
                guard self.isFirstResponder, self.config.cursorBlink, !self.config.reduceMotion,
                    !UIAccessibility.isReduceMotionEnabled
                else { return }
                self.cursorOn.toggle()
                self.setNeedsDisplay(self.cellRect(self.cursorPoint))
            }
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    // MARK: Layout

    private var config: RemoteConfiguration { screen?.configuration ?? RemoteConfiguration() }
    private var baseSize: CGFloat { CGFloat(min(36, max(8, config.fontSize))) * zoom }

    private struct FontKey: Hashable {
        var size: CGFloat
        var bold: Bool
        var italic: Bool
    }

    private struct GlyphKey: Hashable {
        var font: FontKey
        var scalar: UInt32
    }

    /// Fonts are looked up once per size and style, not once per glyph.
    private func font(size: CGFloat, bold: Bool = false, italic: Bool = false) -> UIFont {
        if cachedFamily != config.fontFamily {
            cachedFamily = config.fontFamily
            fontCache.removeAll()
            glyphSizes.removeAll()
        }
        let key = FontKey(size: size, bold: bold, italic: italic)
        if let font = fontCache[key] { return font }
        if fontCache.count > 32 {
            fontCache.removeAll()
            glyphSizes.removeAll()
        }
        var font =
            UIFont(name: config.fontFamily, size: size) ?? UIFont.monospacedSystemFont(ofSize: size, weight: .regular)
        var traits = font.fontDescriptor.symbolicTraits
        if bold { traits.insert(.traitBold) }
        if italic { traits.insert(.traitItalic) }
        if let descriptor = font.fontDescriptor.withSymbolicTraits(traits) {
            font = UIFont(descriptor: descriptor, size: size)
        }
        fontCache[key] = font
        return font
    }

    private func glyphSize(_ scalar: Unicode.Scalar, font key: FontKey, attributes: [NSAttributedString.Key: Any])
        -> CGSize
    {
        let glyph = GlyphKey(font: key, scalar: scalar.value)
        if let size = glyphSizes[glyph] { return size }
        if glyphSizes.count > 8192 { glyphSizes.removeAll() }
        let size = (String(scalar) as NSString).size(withAttributes: attributes)
        glyphSizes[glyph] = size
        return size
    }

    private func cellSize(fontSize: CGFloat) -> CGSize {
        let scale = fontSize / CGFloat(max(1, config.fontSize))
        return CGSize(
            width: CGFloat(max(1, config.cellWidth)) * scale, height: CGFloat(max(1, config.cellHeight)) * scale)
    }
    private var padding: CGSize {
        CGSize(width: CGFloat(min(40, max(0, config.paddingX))), height: CGFloat(min(40, max(0, config.paddingY))))
    }
    private var fittingFontSize: CGFloat {
        guard let screen, screen.cols > 0, bounds.width > 0 else { return baseSize }
        let cell = cellSize(fontSize: baseSize)
        let ratio = min(
            (bounds.width - 2 * padding.width) / (CGFloat(screen.cols) * cell.width),
            (bounds.height - 2 * padding.height) / (CGFloat(max(screen.rows, 1)) * cell.height), 1)
        return max(0.5, baseSize * ratio)
    }
    private var cell: CGSize { cellSize(fontSize: fittingFontSize) }
    private var gridOrigin: CGPoint { CGPoint(x: padding.width, y: padding.height) }
    private func cellRect(_ point: GridPoint, span: Int = 1) -> CGRect {
        let cell = cell
        return CGRect(
            x: gridOrigin.x + CGFloat(point.col) * cell.width, y: gridOrigin.y + CGFloat(point.row) * cell.height,
            width: cell.width * CGFloat(span), height: cell.height)
    }
    private func rowRect(_ row: Int) -> CGRect {
        CGRect(x: 0, y: gridOrigin.y + CGFloat(row) * cell.height, width: bounds.width, height: cell.height + 1)
    }
    /// Where the cursor shows: after any predicted text.
    private var cursorPoint: GridPoint {
        if let last = shownPredictions.last { return GridPoint(row: last.row, col: last.col + 1) }
        let cursor = screen?.cursor
        return GridPoint(row: cursor?.row ?? 0, col: cursor?.col ?? 0)
    }
    private var shownPredictions: [EchoPredictor.Prediction] { screen?.predictions(enabled: predictiveEcho) ?? [] }

    var readableGrid: (cols: Int, rows: Int) {
        let cell = cellSize(fontSize: baseSize)
        return (
            max(2, min(500, Int(max(0, bounds.width - 2 * padding.width) / cell.width))),
            max(2, min(300, Int(max(0, bounds.height - 2 * padding.height) / cell.height)))
        )
    }
    override func layoutSubviews() {
        super.layoutSubviews()
        rowElements = nil
        updateGrid()
    }
    func updateGrid() {
        guard screen?.fitted == true, bounds.width > 20, bounds.height > 20 else { return }
        let grid = readableGrid
        guard
            lastGrid?.0 != grid.cols || lastGrid?.1 != grid.rows
                || (screen?.live == true && (screen?.cols != grid.cols || screen?.rows != grid.rows))
        else { return }
        lastGrid = (grid.cols, grid.rows)
        resizeWork?.cancel()
        let work = DispatchWorkItem { [weak self] in self?.onGridChange?(grid.cols, grid.rows) }
        resizeWork = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.12, execute: work)
    }
    func resetFit() {
        lastGrid = nil
        updateGrid()
        setNeedsDisplay()
    }
    private func invalidateRows() {
        let appearance: UIKeyboardAppearance = config.appearance == "light" ? .light : .dark
        if keyboardAppearance != appearance {
            keyboardAppearance = appearance
            if isFirstResponder { reloadInputViews() }
        }
        rowElements = nil
        updateGrid()
        guard let rows = screen?.changedRows, selection == nil else {
            setNeedsDisplay()
            return
        }
        for row in rows { setNeedsDisplay(rowRect(row)) }
    }
    private func setZoom(_ value: CGFloat) {
        zoom = min(2.5, max(0.6, value))
        updateGrid()
        setNeedsDisplay()
    }

    // MARK: Drawing

    private static func color(_ rgba: UInt32) -> UIColor {
        UIColor(
            red: CGFloat(rgba >> 24 & 0xff) / 255, green: CGFloat(rgba >> 16 & 0xff) / 255,
            blue: CGFloat(rgba >> 8 & 0xff) / 255, alpha: CGFloat(rgba & 0xff) / 255
        )
    }

    override func draw(_ rect: CGRect) {
        guard let context = UIGraphicsGetCurrentContext() else { return }
        let background = Self.color(screen?.background ?? 0x1014_21ff).withAlphaComponent(1)
        context.setFillColor(background.cgColor)
        context.fill(bounds)
        guard let screen, screen.cols > 0 else { return }

        let size = fittingFontSize
        let cell = cellSize(fontSize: size)
        let origin = gridOrigin
        drawImages(screen, below: true, cell: cell, origin: origin, in: context)
        for (row, runs) in screen.lines.enumerated() {
            let y = origin.y + CGFloat(row) * cell.height
            guard y < rect.maxY, y + cell.height > rect.minY else { continue }
            for run in runs {
                let frame = CGRect(
                    x: origin.x + CGFloat(run.col) * cell.width, y: y,
                    width: CGFloat(run.cells) * cell.width, height: cell.height)
                guard frame.intersects(rect) else { continue }
                if run.flags & CellRun.defaultBackground == 0 {
                    context.setFillColor(Self.color(run.bg).cgColor)
                    context.fill(frame)
                }
                if let data = run.glyphPNG {
                    let key = data as NSData
                    if let image = Self.imageCache.object(forKey: key) ?? UIImage(data: data) {
                        Self.imageCache.setObject(
                            image, forKey: key, cost: Int(image.size.width * image.size.height * 4))
                        image.draw(in: frame)
                    }
                } else {
                    drawText(
                        run.text, cells: run.cells, in: frame, size: size, cell: cell, color: Self.color(run.fg),
                        bold: run.flags & CellRun.bold != 0, italic: run.flags & CellRun.italic != 0, context: context)
                }
                context.setFillColor(Self.color(run.fg).cgColor)
                if run.flags & CellRun.underline != 0 {
                    context.fill(
                        CGRect(
                            x: frame.minX, y: frame.maxY - max(1, size / 12), width: frame.width,
                            height: max(1, size / 14)))
                }
                if run.flags & CellRun.strikethrough != 0 {
                    context.fill(CGRect(x: frame.minX, y: frame.midY, width: frame.width, height: max(1, size / 14)))
                }
            }
        }
        drawImages(screen, below: false, cell: cell, origin: origin, in: context)
        drawPredictions(size: size, cell: cell, background: background, context: context)
        drawSelection(context: context)
        drawCursor(screen, cell: cell, context: context)
    }

    private func drawText(
        _ text: String, cells: Int, in frame: CGRect, size: CGFloat, cell: CGSize, color: UIColor, bold: Bool,
        italic: Bool, context: CGContext
    ) {
        let fontKey = FontKey(size: size, bold: bold, italic: italic)
        let attributes: [NSAttributedString.Key: Any] = [
            .font: font(size: size, bold: bold, italic: italic), .foregroundColor: color, .ligature: 0,
        ]
        let scalars = Array(text.unicodeScalars)
        for (index, scalar) in scalars.prefix(cells).enumerated() where scalar != " " {
            // Every scalar starts at its own cell; a wide scalar owns the whole run.
            let span = scalars.count == 1 ? cells : 1
            let x = frame.minX + CGFloat(index) * cell.width
            let measured = glyphSize(scalar, font: fontKey, attributes: attributes)
            let width = CGFloat(span) * cell.width
            context.saveGState()
            context.clip(to: CGRect(x: x, y: frame.minY, width: width, height: cell.height))
            context.translateBy(x: x, y: frame.minY + max(0, (cell.height - measured.height) / 2))
            context.scaleBy(x: width / max(0.1, measured.width), y: 1)
            (String(scalar) as NSString).draw(at: .zero, withAttributes: attributes)
            context.restoreGState()
            if span > 1 { break }
        }
    }

    /// Local echo: typed characters the Mac hasn't drawn yet, underlined faintly.
    private func drawPredictions(size: CGFloat, cell: CGSize, background: UIColor, context: CGContext) {
        let foreground = Self.color(config.foreground).withAlphaComponent(1)
        for prediction in shownPredictions {
            let frame = cellRect(GridPoint(row: prediction.row, col: prediction.col))
            context.setFillColor(background.cgColor)
            context.fill(frame)
            drawText(
                String(prediction.character), cells: 1, in: frame, size: size, cell: cell, color: foreground,
                bold: false, italic: false, context: context)
            context.setFillColor(foreground.withAlphaComponent(0.4).cgColor)
            context.fill(CGRect(x: frame.minX, y: frame.maxY - 1, width: frame.width, height: 1))
        }
    }

    private func drawCursor(_ screen: TerminalScreen, cell: CGSize, context: CGContext) {
        let cursor = screen.cursor
        let point = cursorPoint
        guard cursor.visible,
            cursorOn || !config.cursorBlink || !isFirstResponder || config.reduceMotion
                || UIAccessibility.isReduceMotionEnabled, point.row >= 0, point.col >= 0, point.row < screen.rows,
            point.col < screen.cols
        else { return }
        let frame = cellRect(point)
        context.setFillColor(
            Self.color(config.cursorColor).withAlphaComponent(isFirstResponder ? 0.65 : 0.3).cgColor)
        switch cursor.style {
        case "bar": context.fill(CGRect(x: frame.minX, y: frame.minY, width: max(1, cell.width / 6), height: cell.height))
        case "underline": context.fill(CGRect(x: frame.minX, y: frame.maxY - 2, width: cell.width, height: 2))
        default: context.fill(frame)
        }
    }

    private func drawSelection(context: CGContext) {
        guard let rects = selectionRects(), let selection else { return }
        context.setFillColor(tintColor.withAlphaComponent(0.35).cgColor)
        for rect in rects { context.fill(rect) }
        // Handles, like the system's text selection.
        context.setFillColor(tintColor.cgColor)
        let (start, end) = ordered(selection)
        let first = cellRect(start)
        let last = cellRect(end)
        context.fill(CGRect(x: first.minX - 1, y: first.minY, width: 2, height: first.height))
        context.fillEllipse(in: CGRect(x: first.minX - 4, y: first.minY - 7, width: 8, height: 8))
        context.fill(CGRect(x: last.maxX - 1, y: last.minY, width: 2, height: last.height))
        context.fillEllipse(in: CGRect(x: last.maxX - 4, y: last.maxY - 1, width: 8, height: 8))
    }

    /// Draws the screen's Kitty graphics that go below text, or above it.
    private func drawImages(
        _ screen: TerminalScreen, below: Bool, cell: CGSize, origin: CGPoint, in context: CGContext
    ) {
        func rect(_ cells: [Double]) -> CGRect {
            CGRect(
                x: origin.x + CGFloat(cells[0]) * cell.width, y: origin.y + CGFloat(cells[1]) * cell.height,
                width: CGFloat(cells[2]) * cell.width, height: CGFloat(cells[3]) * cell.height)
        }
        for placement in screen.images where (placement.z < 0) == below {
            guard let image = screen.image(placement.key) else { continue }
            let crop = CGRect(
                x: placement.crop[0] * Double(image.width), y: placement.crop[1] * Double(image.height),
                width: placement.crop[2] * Double(image.width), height: placement.crop[3] * Double(image.height))
            guard let cropped = image.cropping(to: crop) else { continue }
            let frame = rect(placement.frame)
            context.saveGState()
            context.clip(to: rect(placement.clip))
            // CGImage draws bottom-up; flip into the view's top-down space.
            context.translateBy(x: 0, y: frame.maxY + frame.minY)
            context.scaleBy(x: 1, y: -1)
            context.draw(cropped, in: frame)
            context.restoreGState()
        }
    }

    // MARK: Touch

    @objc private func tapped(_ gesture: UITapGestureRecognizer) {
        if selection != nil {
            clearSelection()
            return
        }
        guard screen?.live == true else { return }
        if mouseMode {
            let point = position(gesture.location(in: self))
            onMouse?(point.col, point.row, "press")
            onMouse?(point.col, point.row, "release")
        } else {
            _ = becomeFirstResponder()
        }
    }

    private func position(_ point: CGPoint) -> (col: Int, row: Int) {
        let cell = cell
        return (max(0, Int((point.x - gridOrigin.x) / cell.width)), max(0, Int((point.y - gridOrigin.y) / cell.height)))
    }

    private func gridPoint(_ point: CGPoint) -> GridPoint {
        let position = position(point)
        return GridPoint(
            row: min(max(0, (screen?.rows ?? 1) - 1), position.row), col: min(max(0, (screen?.cols ?? 1) - 1), position.col))
    }

    @objc private func panned(_ gesture: UIPanGestureRecognizer) {
        if gesture.state == .began, let handle = handle(near: gesture.location(in: self)) {
            draggingHandle = handle
            editMenu.dismissMenu()
        }
        if let handle = draggingHandle {
            moveHandle(handle, to: gridPoint(gesture.location(in: self)))
            if gesture.state == .ended || gesture.state == .cancelled {
                draggingHandle = nil
                selectionFinished()
            }
            return
        }
        guard screen?.live == true else { return }
        if mouseMode {
            let point = position(gesture.location(in: self))
            let action =
                gesture.state == .began
                ? "press" : (gesture.state == .ended || gesture.state == .cancelled ? "release" : "drag")
            onMouse?(point.col, point.row, action)
            return
        }
        if gesture.state == .began { clearSelection() }
        let lineHeight = cell.height / CGFloat(max(0.1, config.scrollMultiplier))
        scrollRemainder += gesture.translation(in: self).y
        gesture.setTranslation(.zero, in: self)
        let lines = Int(scrollRemainder / lineHeight)
        if lines != 0 {
            scrollRemainder -= CGFloat(lines) * lineHeight
            onScroll?(lines)
        }
        if gesture.state == .ended { scrollRemainder = 0 }
    }

    @objc private func edgeSwiped(_ gesture: UIScreenEdgePanGestureRecognizer) {
        guard gesture.state == .ended else { return }
        let distance = gesture.translation(in: self).x
        guard abs(distance) > 60 else { return }
        UIImpactFeedbackGenerator(style: .light).impactOccurred()
        onCommand?(gesture.edges == .left ? .previousPane : .nextPane)
    }

    func gestureRecognizer(_ a: UIGestureRecognizer, shouldRecognizeSimultaneouslyWith b: UIGestureRecognizer) -> Bool {
        (a === pinch && b === trackpad) || (a === trackpad && b === pinch)
    }

    @objc private func pinched(_ gesture: UIPinchGestureRecognizer) {
        switch gesture.state {
        case .began:
            pinchScale = 1
        case .changed:
            pinchScale *= gesture.scale
            if twoFingerMode == .undecided, abs(log(pinchScale)) > 0.08 { twoFingerMode = .zoom }
            if twoFingerMode == .zoom { setZoom(zoom * gesture.scale) }
            gesture.scale = 1
        default:
            endTwoFingers()
        }
    }

    /// Two fingers moving together send arrow keys, a cell at a time.
    @objc private func twoFingerPanned(_ gesture: UIPanGestureRecognizer) {
        switch gesture.state {
        case .began:
            trackpadRemainder = .zero
        case .changed:
            let translation = gesture.translation(in: self)
            gesture.setTranslation(.zero, in: self)
            trackpadRemainder.x += translation.x
            trackpadRemainder.y += translation.y
            if twoFingerMode == .undecided, hypot(trackpadRemainder.x, trackpadRemainder.y) > 14 {
                twoFingerMode = .trackpad
            }
            guard twoFingerMode == .trackpad, screen?.live == true else { return }
            let cell = cell
            let step = CGSize(width: max(6, cell.width * 1.2), height: max(10, cell.height))
            while abs(trackpadRemainder.x) >= step.width {
                let right = trackpadRemainder.x > 0
                send(RemoteKey(key: right ? "right" : "left"))
                trackpadRemainder.x -= right ? step.width : -step.width
            }
            while abs(trackpadRemainder.y) >= step.height {
                let down = trackpadRemainder.y > 0
                send(RemoteKey(key: down ? "down" : "up"))
                trackpadRemainder.y -= down ? step.height : -step.height
            }
        default:
            endTwoFingers()
        }
    }

    private func endTwoFingers() {
        guard pinch.state != .changed, trackpad.state != .changed else { return }
        twoFingerMode = .undecided
    }

    // MARK: Selection

    @objc private func longPressed(_ gesture: UILongPressGestureRecognizer) {
        guard let screen, screen.cols > 0 else { return }
        let point = gridPoint(gesture.location(in: self))
        switch gesture.state {
        case .began:
            editMenu.dismissMenu()
            let word = screen.word(at: point)
            selectionWord = word
            selection = (word.0, word.1)
            UISelectionFeedbackGenerator().selectionChanged()
            setNeedsDisplay()
        case .changed:
            guard let word = selectionWord else { return }
            selection = point < word.0 ? (point, word.1) : (word.0, max(point, word.1))
            setNeedsDisplay()
        case .ended:
            selectionFinished()
        default:
            break
        }
    }

    private func ordered(_ selection: (start: GridPoint, end: GridPoint)) -> (GridPoint, GridPoint) {
        selection.start <= selection.end ? (selection.start, selection.end) : (selection.end, selection.start)
    }

    private func handle(near location: CGPoint) -> Handle? {
        guard let selection else { return nil }
        let (start, end) = ordered(selection)
        let first = cellRect(start)
        let last = cellRect(end)
        if hypot(location.x - first.minX, location.y - first.midY) < 24 { return .start }
        if hypot(location.x - last.maxX, location.y - last.midY) < 24 { return .end }
        return nil
    }

    private func moveHandle(_ handle: Handle, to point: GridPoint) {
        guard let current = selection else { return }
        let (start, end) = ordered(current)
        selection = handle == .start ? (min(point, end), end) : (start, max(point, start))
        setNeedsDisplay()
    }

    private func selectionFinished() {
        guard let selection else { return }
        if config.copyOnSelect { copySelection() }
        let rect = cellRect(ordered(selection).0)
        editMenu.presentEditMenu(
            with: UIEditMenuConfiguration(identifier: nil, sourcePoint: CGPoint(x: rect.midX, y: rect.minY)))
    }

    private func selectionRects() -> [CGRect]? {
        guard let selection, let screen else { return nil }
        let (start, end) = ordered(selection)
        return (start.row...end.row).map { row in
            let from = row == start.row ? start.col : 0
            let to = row == end.row ? end.col : screen.cols - 1
            return cellRect(GridPoint(row: row, col: from), span: max(1, to - from + 1))
        }
    }

    var selectedText: String? {
        guard let selection, let screen else { return nil }
        return screen.text(from: selection.start, to: selection.end)
    }

    func clearSelection() {
        guard selection != nil else { return }
        selection = nil
        selectionWord = nil
        editMenu.dismissMenu()
        setNeedsDisplay()
    }

    private func copySelection() {
        guard let text = selectedText, !text.isEmpty else { return }
        UIPasteboard.general.string = text
    }

    func editMenuInteraction(
        _ interaction: UIEditMenuInteraction, menuFor configuration: UIEditMenuConfiguration,
        suggestedActions: [UIMenuElement]
    ) -> UIMenu? {
        guard selection != nil else { return nil }
        return UIMenu(children: [
            UIAction(title: "Copy", image: UIImage(systemName: "doc.on.doc")) { [weak self] _ in
                self?.copySelection()
                self?.clearSelection()
            },
            UIAction(title: "Select All", image: UIImage(systemName: "selection.pin.in.out")) { [weak self] _ in
                guard let self, let screen = self.screen, screen.rows > 0 else { return }
                self.selection = (
                    GridPoint(row: 0, col: 0), GridPoint(row: screen.rows - 1, col: screen.cols - 1)
                )
                self.setNeedsDisplay()
                self.selectionFinished()
            },
            UIAction(title: "Share…", image: UIImage(systemName: "square.and.arrow.up")) { [weak self] _ in
                guard let self, let text = self.selectedText else { return }
                let share = UIActivityViewController(activityItems: [text], applicationActivities: nil)
                share.popoverPresentationController?.sourceView = self
                share.popoverPresentationController?.sourceRect = self.selectionRects()?.first ?? self.bounds
                self.present(share)
            },
            UIAction(title: "Open as Text", image: UIImage(systemName: "doc.plaintext")) { [weak self] _ in
                self?.showText()
            },
        ])
    }

    func editMenuInteraction(_ interaction: UIEditMenuInteraction, targetRectFor configuration: UIEditMenuConfiguration)
        -> CGRect
    {
        selectionRects()?.reduce(CGRect.null) { $0.union($1) } ?? .null
    }

    /// The whole screen as selectable text, for long passages.
    private func showText() {
        guard let screen else { return }
        clearSelection()
        present(
            UINavigationController(
                rootViewController: TerminalTextController(text: screen.visibleText, copyOnSelect: config.copyOnSelect)))
    }

    private func present(_ controller: UIViewController) {
        var responder: UIResponder? = self
        while let next = responder?.next {
            if let presenter = next as? UIViewController {
                presenter.present(controller, animated: !config.reduceMotion)
                return
            }
            responder = next
        }
    }

    // MARK: Keyboard

    override var canBecomeFirstResponder: Bool { true }
    override var inputAccessoryView: UIView? { keyBar }

    override func becomeFirstResponder() -> Bool {
        defer { setNeedsDisplay() }
        keyBar.reloadIfNeeded()
        let became = super.becomeFirstResponder()
        if became { onFocus?() }
        return became
    }

    override func resignFirstResponder() -> Bool {
        defer { setNeedsDisplay() }
        hardwareRepeater.stop()
        return super.resignFirstResponder()
    }

    var autocorrectionType: UITextAutocorrectionType = .no
    var autocapitalizationType: UITextAutocapitalizationType = .none
    var spellCheckingType: UITextSpellCheckingType = .no
    var smartQuotesType: UITextSmartQuotesType = .no
    var smartDashesType: UITextSmartDashesType = .no
    var smartInsertDeleteType: UITextSmartInsertDeleteType = .no
    var keyboardAppearance: UIKeyboardAppearance = .dark
    var keyboardType: UIKeyboardType = .default
    var returnKeyType: UIReturnKeyType = .default

    var hasText: Bool { true }

    func insertText(_ text: String) {
        switch text {
        case "\n": send(RemoteKey(key: "enter"))
        case "\t": send(RemoteKey(key: "tab"))
        default:
            if text.count == 1 {
                send(.text(text))
            } else {
                // Suggestions and dictation: modifiers stay for the next real key.
                send(.text(text), modifiers: false)
            }
        }
    }

    func deleteBackward() {
        send(RemoteKey(key: "backspace"))
    }

    func send(_ key: RemoteKey, modifiers: Bool = true) {
        guard let screen, screen.live, !screen.exited else { return }
        cursorOn = true
        var key = key
        if modifiers {
            if controlState != .off {
                key.control = true
                key.text = nil
                if key.key.count == 1 { key.key = key.key.lowercased() }
                if controlState == .armed { controlState = .off }
            }
            if altState != .off {
                key.alt = true
                if altState == .armed { altState = .off }
            }
        }
        let before = cursorPoint
        if predictiveEcho { screen.noteSent(key) } else { screen.noteSent(RemoteKey(key: "")) }
        onKey?(key)
        setNeedsDisplay(rowRect(before.row))
        setNeedsDisplay(rowRect(cursorPoint.row))
    }

    /// Ctrl and alt: tap for the next key, double-tap to lock, tap again to release.
    func tapModifier(control: Bool) {
        let state = control ? controlState : altState
        let now = Date()
        let doubleTap =
            lastModifierTap.map { $0.control == control && now.timeIntervalSince($0.at) < 0.35 } ?? false
        let next: ModifierState
        switch state {
        case .off: next = .armed
        case .armed: next = doubleTap ? .locked : .off
        case .locked: next = .off
        }
        lastModifierTap = (control, now)
        if control { controlState = next } else { altState = next }
        UIImpactFeedbackGenerator(style: next == .locked ? .medium : .light).impactOccurred()
    }

    func paste() {
        guard screen?.live == true, let text = UIPasteboard.general.string, !text.isEmpty else { return }
        screen?.notePasted()
        onPaste?(text)
    }

    /// Types a saved command, pressing Return when the snippet says so.
    func type(_ snippet: Snippet) {
        guard let screen, screen.live, !screen.exited, !snippet.command.isEmpty else { return }
        screen.notePasted()
        onPaste?(snippet.command)
        if snippet.runs { send(RemoteKey(key: "enter"), modifiers: false) }
    }

    /// Hardware keyboards: keys UIKeyInput doesn't deliver, and modifier combos.
    override func pressesBegan(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        var unhandled = Set<UIPress>()
        for press in presses {
            guard let key = press.key, let remote = Self.remoteKey(for: key) else {
                unhandled.insert(press)
                continue
            }
            send(remote)
            if Self.repeats(remote) {
                repeatingKeyCode = key.keyCode
                hardwareRepeater.start { [weak self] in self?.send(remote) }
            }
        }
        if !unhandled.isEmpty { super.pressesBegan(unhandled, with: event) }
    }

    override func pressesEnded(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        stopRepeating(presses)
        super.pressesEnded(presses, with: event)
    }

    override func pressesCancelled(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        stopRepeating(presses)
        super.pressesCancelled(presses, with: event)
    }

    private func stopRepeating(_ presses: Set<UIPress>) {
        guard let code = repeatingKeyCode, presses.contains(where: { $0.key?.keyCode == code }) else { return }
        repeatingKeyCode = nil
        hardwareRepeater.stop()
    }

    static func repeats(_ key: RemoteKey) -> Bool {
        ["up", "down", "left", "right", "pageup", "pagedown", "delete"].contains(key.key)
    }

    static func remoteKey(for key: UIKey) -> RemoteKey? {
        let flags = key.modifierFlags
        let control = flags.contains(.control)
        let alt = flags.contains(.alternate)
        let shift = flags.contains(.shift)
        let named: String? =
            switch key.keyCode {
            case .keyboardUpArrow: "up"
            case .keyboardDownArrow: "down"
            case .keyboardLeftArrow: "left"
            case .keyboardRightArrow: "right"
            case .keyboardEscape: "escape"
            case .keyboardHome: "home"
            case .keyboardEnd: "end"
            case .keyboardPageUp: "pageup"
            case .keyboardPageDown: "pagedown"
            case .keyboardF1: "f1"
            case .keyboardF2: "f2"
            case .keyboardF3: "f3"
            case .keyboardF4: "f4"
            case .keyboardF5: "f5"
            case .keyboardF6: "f6"
            case .keyboardF7: "f7"
            case .keyboardF8: "f8"
            case .keyboardF9: "f9"
            case .keyboardF10: "f10"
            case .keyboardF11: "f11"
            case .keyboardF12: "f12"
            case .keyboardDeleteForward: "delete"
            case .keyboardTab where control || alt || shift: "tab"
            default: nil
            }
        guard !flags.contains(.command) else { return nil }
        if let named { return RemoteKey(key: named, control: control, alt: alt, shift: shift) }
        guard control || alt else { return nil }
        let character = key.charactersIgnoringModifiers
        guard character.count == 1 else { return nil }
        return RemoteKey(
            key: character.lowercased(), text: control ? nil : character, control: control, alt: alt, shift: shift)
    }

    // MARK: ⌘ shortcuts

    override var keyCommands: [UIKeyCommand]? {
        func command(_ title: String, _ input: String, _ action: Selector, _ flags: UIKeyModifierFlags = .command)
            -> UIKeyCommand
        {
            let command = UIKeyCommand(title: title, action: action, input: input, modifierFlags: flags)
            command.wantsPriorityOverSystemBehavior = true
            return command
        }
        return [
            command("Paste", "v", #selector(pasteCommand)),
            command("Copy", "c", #selector(copyCommand)),
            command("Find in Scrollback", "f", #selector(findCommand)),
            command("New Tab", "t", #selector(newTabCommand)),
            command("Split Side by Side", "d", #selector(splitRightCommand)),
            command("Split Above and Below", "d", #selector(splitDownCommand), [.command, .shift]),
            command("Next Pane", "]", #selector(nextPaneCommand), [.command, .shift]),
            command("Previous Pane", "[", #selector(previousPaneCommand), [.command, .shift]),
            command("Close Terminal", "w", #selector(closeCommand)),
            command("Bigger", "=", #selector(zoomInCommand)),
            command("Smaller", "-", #selector(zoomOutCommand)),
            command("Actual Size", "0", #selector(zoomResetCommand)),
        ]
    }

    @objc private func pasteCommand() { paste() }
    @objc private func copyCommand() {
        copySelection()
        clearSelection()
    }
    @objc private func findCommand() { onCommand?(.find) }
    @objc private func newTabCommand() { onCommand?(.newTab) }
    @objc private func splitRightCommand() { onCommand?(.splitRight) }
    @objc private func splitDownCommand() { onCommand?(.splitDown) }
    @objc private func nextPaneCommand() { onCommand?(.nextPane) }
    @objc private func previousPaneCommand() { onCommand?(.previousPane) }
    @objc private func closeCommand() { onCommand?(.close) }
    @objc private func zoomInCommand() { setZoom(zoom * 1.1) }
    @objc private func zoomOutCommand() { setZoom(zoom / 1.1) }
    @objc private func zoomResetCommand() { setZoom(1) }

    // MARK: Accessibility

    /// VoiceOver reads the screen row by row; activating a row shows the keyboard.
    override var accessibilityElements: [Any]? {
        get {
            if let rowElements { return rowElements }
            guard let screen else { return [] }
            var elements: [UIAccessibilityElement] = []
            for row in 0..<screen.rows {
                let text = screen.text(from: GridPoint(row: row, col: 0), to: GridPoint(row: row, col: screen.cols - 1))
                    .trimmingCharacters(in: .whitespaces)
                guard !text.isEmpty else { continue }
                let element = TerminalRowElement(accessibilityContainer: self)
                element.accessibilityLabel = text
                element.accessibilityTraits = .staticText
                element.accessibilityFrameInContainerSpace = rowRect(row)
                element.onActivate = { [weak self] in _ = self?.becomeFirstResponder() }
                if row == screen.cursor.row { element.accessibilityHint = "Cursor line. Double-tap to type." }
                elements.append(element)
            }
            if elements.isEmpty {
                let element = TerminalRowElement(accessibilityContainer: self)
                element.accessibilityLabel = "Empty terminal"
                element.accessibilityFrameInContainerSpace = bounds
                element.onActivate = { [weak self] in _ = self?.becomeFirstResponder() }
                elements.append(element)
            }
            rowElements = elements
            return elements
        }
        set {}
    }
}

private final class TerminalRowElement: UIAccessibilityElement {
    var onActivate: (() -> Void)?
    override func accessibilityActivate() -> Bool {
        onActivate?()
        return true
    }
}

/// SwiftUI host for `TerminalUIView`.
struct TerminalViewRepresentable: UIViewRepresentable {
    let screen: TerminalScreen
    let link: HostLink
    var focusOnAppear = true
    /// Set by the view so SwiftUI can ask for the readable grid.
    var onReady: ((TerminalUIView) -> Void)?
    var onCommand: ((TerminalCommand) -> Void)?
    var onFocus: (() -> Void)?

    func makeUIView(context: Context) -> TerminalUIView {
        let view = TerminalUIView()
        let stream = screen.stream
        view.onKey = { [weak link] key in link?.send(.key(stream: stream, key: key)) }
        view.onPaste = { [weak link] text in link?.send(.paste(stream: stream, text: text)) }
        view.onScroll = { [weak link] lines in link?.send(.scroll(stream: stream, lines: lines)) }
        view.onMouse = { [weak link] col, row, action in
            link?.send(.mouse(stream: stream, col: col, row: row, action: action))
        }
        view.onGridChange = { [weak link, weak screen] cols, rows in
            guard screen?.fitted == true, link?.state == .connected else { return }
            link?.send(.resize(stream: stream, cols: cols, rows: rows))
        }
        view.onCommand = onCommand
        view.onFocus = onFocus
        view.screen = screen
        onReady?(view)
        if focusOnAppear { DispatchQueue.main.async { _ = view.becomeFirstResponder() } }
        return view
    }

    func updateUIView(_ view: TerminalUIView, context: Context) {
        if view.screen !== screen { view.screen = screen }
        view.onCommand = onCommand
        view.onFocus = onFocus
        view.updateGrid()
    }
}

/// Native text selection with standard copy/share actions and optional copy on select.
private final class TerminalTextController: UIViewController, UITextViewDelegate {
    private let text: String
    private let copyOnSelect: Bool
    init(text: String, copyOnSelect: Bool) {
        self.text = text
        self.copyOnSelect = copyOnSelect
        super.init(nibName: nil, bundle: nil)
    }
    @available(*, unavailable) required init?(coder: NSCoder) { fatalError() }
    override func loadView() {
        let textView = UITextView()
        textView.text = text
        textView.font = .monospacedSystemFont(ofSize: 14, weight: .regular)
        textView.isEditable = false
        textView.isSelectable = true
        textView.delegate = self
        textView.textContainerInset = UIEdgeInsets(top: 16, left: 12, bottom: 16, right: 12)
        view = textView
        title = "Select Text"
        navigationItem.rightBarButtonItem = UIBarButtonItem(
            systemItem: .done, primaryAction: UIAction { [weak self] _ in self?.dismiss(animated: true) })
    }
    func textViewDidChangeSelection(_ textView: UITextView) {
        guard copyOnSelect, let range = textView.selectedTextRange, let text = textView.text(in: range), !text.isEmpty
        else { return }
        UIPasteboard.general.string = text
    }
}
