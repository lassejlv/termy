import SwiftUI
import TermyRemoteKit
import UIKit

/// Draws a `TerminalScreen` and turns touches and keys into `ClientMessage`s.
///
/// Uses the Mac’s cell metrics and fits the host grid to the phone by default.
/// Restoring the Mac size scales that grid to fit without changing its columns.
final class TerminalUIView: UIView, UIKeyInput, UITextInputTraits {
    var screen: TerminalScreen? {
        didSet {
            oldValue?.onChange = nil
            screen?.onChange = { [weak self] in self?.invalidateRows() }
            setNeedsDisplay()
        }
    }

    var onKey: ((RemoteKey) -> Void)?
    var onPaste: ((String) -> Void)?
    var onScroll: ((Int) -> Void)?
    var onMouse: ((Int, Int, String) -> Void)?
    var onGridChange: ((Int, Int) -> Void)?
    var mouseMode = false
    private var zoom: CGFloat = 1
    private var blinkTimer: Timer?
    private var cursorOn = true
    private var lastGrid: (Int, Int)?
    private var resizeWork: DispatchWorkItem?
    private static let imageCache = NSCache<NSData, UIImage>()

    /// Sticky modifiers from the key bar, applied to the next key.
    private var controlArmed = false { didSet { keyBar.setArmed(control: controlArmed, alt: altArmed) } }
    private var altArmed = false { didSet { keyBar.setArmed(control: controlArmed, alt: altArmed) } }
    private lazy var keyBar = KeyBar(view: self)
    private var scrollRemainder: CGFloat = 0

    override init(frame: CGRect) {
        super.init(frame: frame)
        isOpaque = true
        contentMode = .redraw
        addGestureRecognizer(UITapGestureRecognizer(target: self, action: #selector(tapped(_:))))
        let pan = UIPanGestureRecognizer(target: self, action: #selector(panned(_:)))
        pan.maximumNumberOfTouches = 1
        addGestureRecognizer(pan)
        addGestureRecognizer(UIPinchGestureRecognizer(target: self, action: #selector(pinched(_:))))
        let copy = UILongPressGestureRecognizer(target: self, action: #selector(selectText(_:)))
        addGestureRecognizer(copy)
        accessibilityLabel = "Remote terminal"
        accessibilityHint = "Tap to type. Drag to scroll. Pinch to change text size. Hold to select text."

    }

    override func didMoveToWindow() {
        super.didMoveToWindow()
        if window == nil {
            blinkTimer?.invalidate()
            blinkTimer = nil
            resizeWork?.cancel()
        } else if blinkTimer == nil {
            startBlinking()
        }
    }
    private func startBlinking() {
        blinkTimer = Timer.scheduledTimer(withTimeInterval: 0.6, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self, self.window != nil, self.isFirstResponder, self.config.cursorBlink,
                    !self.config.reduceMotion, !UIAccessibility.isReduceMotionEnabled
                else { return }
                self.cursorOn.toggle()
                guard let cursor = self.screen?.cursor else { return }
                let cell = self.cellSize(fontSize: self.fittingFontSize)
                self.setNeedsDisplay(
                    CGRect(
                        x: self.gridOrigin.x + CGFloat(cursor.col) * cell.width,
                        y: self.gridOrigin.y + CGFloat(cursor.row) * cell.height, width: cell.width, height: cell.height
                    ))
            }
        }

    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    // MARK: Layout

    private var config: RemoteConfiguration { screen?.configuration ?? RemoteConfiguration() }
    private var baseSize: CGFloat { CGFloat(min(36, max(8, config.fontSize))) * zoom }
    private func font(size: CGFloat, bold: Bool = false, italic: Bool = false) -> UIFont {
        var font =
            UIFont(name: config.fontFamily, size: size) ?? UIFont.monospacedSystemFont(ofSize: size, weight: .regular)
        var traits = font.fontDescriptor.symbolicTraits
        if bold { traits.insert(.traitBold) }
        if italic { traits.insert(.traitItalic) }
        if let descriptor = font.fontDescriptor.withSymbolicTraits(traits) {
            font = UIFont(descriptor: descriptor, size: size)
        }
        return font
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
    private var gridOrigin: CGPoint { CGPoint(x: padding.width, y: padding.height) }
    var readableGrid: (cols: Int, rows: Int) {
        let cell = cellSize(fontSize: baseSize)
        return (
            max(2, min(500, Int(max(0, bounds.width - 2 * padding.width) / cell.width))),
            max(2, min(300, Int(max(0, bounds.height - 2 * padding.height) / cell.height)))
        )
    }
    override func layoutSubviews() {
        super.layoutSubviews()
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
        updateGrid()
        guard let rows = screen?.changedRows else {
            setNeedsDisplay()
            return
        }
        let cell = cellSize(fontSize: fittingFontSize)
        for row in rows {
            setNeedsDisplay(
                CGRect(x: 0, y: gridOrigin.y + CGFloat(row) * cell.height, width: bounds.width, height: cell.height + 1)
            )
        }
    }
    @objc private func pinched(_ gesture: UIPinchGestureRecognizer) {
        guard gesture.state == .changed else { return }
        zoom = min(2.5, max(0.6, zoom * gesture.scale))
        gesture.scale = 1
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
                if run.flags & CellRun.defaultBackground == 0 {
                    context.setFillColor(Self.color(run.bg).cgColor)
                    context.fill(frame)
                }
                if let data = run.glyphPNG {
                    let key = data as NSData
                    let image: UIImage? = Self.imageCache.object(forKey: key) ?? UIImage(data: data)
                    if let image {
                        Self.imageCache.totalCostLimit = 12 * 1024 * 1024
                        Self.imageCache.setObject(
                            image, forKey: key, cost: Int(image.size.width * image.size.height * 4))
                        image.draw(in: frame)
                    }
                } else {
                    let scalars = Array(run.text.unicodeScalars)
                    for (index, scalar) in scalars.prefix(run.cells).enumerated() where scalar != " " {
                        // Every scalar starts at its own cell; a wide scalar owns the whole run.
                        let span = scalars.count == 1 ? run.cells : 1
                        let x = frame.minX + CGFloat(index) * cell.width
                        let attributes: [NSAttributedString.Key: Any] = [
                            .font: font(
                                size: size, bold: run.flags & CellRun.bold != 0, italic: run.flags & CellRun.italic != 0
                            ),
                            .foregroundColor: Self.color(run.fg), .ligature: 0,
                        ]
                        let text = String(scalar) as NSString
                        let measured = text.size(withAttributes: attributes)
                        let width = CGFloat(span) * cell.width
                        context.saveGState()
                        context.clip(to: CGRect(x: x, y: y, width: width, height: cell.height))
                        context.translateBy(x: x, y: y + max(0, (cell.height - measured.height) / 2))
                        context.scaleBy(x: width / max(0.1, measured.width), y: 1)
                        text.draw(at: .zero, withAttributes: attributes)
                        context.restoreGState()
                        if span > 1 { break }
                    }
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
        let cursor = screen.cursor
        if cursor.visible,
            cursorOn || !config.cursorBlink || !isFirstResponder || config.reduceMotion
                || UIAccessibility.isReduceMotionEnabled, cursor.row >= 0, cursor.col >= 0, cursor.row < screen.rows,
            cursor.col < screen.cols
        {
            let x = origin.x + CGFloat(cursor.col) * cell.width
            let y = origin.y + CGFloat(cursor.row) * cell.height
            context.setFillColor(
                Self.color(config.cursorColor).withAlphaComponent(isFirstResponder ? 0.65 : 0.3).cgColor)
            switch cursor.style {
            case "bar": context.fill(CGRect(x: x, y: y, width: max(1, cell.width / 6), height: cell.height))
            case "underline": context.fill(CGRect(x: x, y: y + cell.height - 2, width: cell.width, height: 2))
            default: context.fill(CGRect(x: x, y: y, width: cell.width, height: cell.height))
            }
        }
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
        let cell = cellSize(fontSize: fittingFontSize)
        return (max(0, Int((point.x - gridOrigin.x) / cell.width)), max(0, Int((point.y - gridOrigin.y) / cell.height)))
    }
    @objc private func selectText(_ gesture: UILongPressGestureRecognizer) {
        guard gesture.state == .began, let screen else { return }
        let controller = UINavigationController(
            rootViewController: TerminalTextController(text: screen.visibleText, copyOnSelect: config.copyOnSelect))
        var responder: UIResponder? = self
        while let next = responder?.next {
            if let presenter = next as? UIViewController {
                presenter.present(controller, animated: !config.reduceMotion)
                break
            }
            responder = next
        }
    }

    @objc private func panned(_ gesture: UIPanGestureRecognizer) {
        guard screen?.live == true else { return }
        if mouseMode {
            let point = position(gesture.location(in: self))
            let action =
                gesture.state == .began
                ? "press" : (gesture.state == .ended || gesture.state == .cancelled ? "release" : "drag")
            onMouse?(point.col, point.row, action)
            return
        }
        let lineHeight = cellSize(fontSize: fittingFontSize).height / CGFloat(max(0.1, config.scrollMultiplier))
        scrollRemainder += gesture.translation(in: self).y
        gesture.setTranslation(.zero, in: self)
        let lines = Int(scrollRemainder / lineHeight)
        if lines != 0 {
            scrollRemainder -= CGFloat(lines) * lineHeight
            onScroll?(lines)
        }
        if gesture.state == .ended { scrollRemainder = 0 }
    }

    // MARK: Keyboard

    override var canBecomeFirstResponder: Bool { true }
    override var inputAccessoryView: UIView? { keyBar }

    override func becomeFirstResponder() -> Bool {
        defer { setNeedsDisplay() }
        return super.becomeFirstResponder()
    }

    override func resignFirstResponder() -> Bool {
        defer { setNeedsDisplay() }
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
            if controlArmed || altArmed, text.count == 1 {
                send(RemoteKey(key: text.lowercased(), text: text))
            } else {
                send(.text(text))
            }
        }
    }

    func deleteBackward() {
        send(RemoteKey(key: "backspace"))
    }

    func send(_ key: RemoteKey) {
        guard screen?.live == true, screen?.exited == false else { return }
        cursorOn = true
        var key = key
        if controlArmed {
            key.control = true
            key.text = nil
            controlArmed = false
        }
        if altArmed {
            key.alt = true
            altArmed = false
        }
        onKey?(key)
    }

    func toggleControl() { controlArmed.toggle() }
    func toggleAlt() { altArmed.toggle() }

    func paste() {
        guard screen?.live == true, let text = UIPasteboard.general.string, !text.isEmpty else { return }
        onPaste?(text)
    }

    /// Hardware keyboards: keys UIKeyInput doesn't deliver, and modifier combos.
    override func pressesBegan(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        var handled = false
        for press in presses {
            guard let key = press.key, let remote = Self.remoteKey(for: key) else { continue }
            onKey?(remote)
            handled = true
        }
        if !handled { super.pressesBegan(presses, with: event) }
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
        if let named { return RemoteKey(key: named, control: control, alt: alt, shift: shift) }
        guard control || alt, !flags.contains(.command) else { return nil }
        let character = key.charactersIgnoringModifiers
        guard character.count == 1 else { return nil }
        return RemoteKey(
            key: character.lowercased(), text: control ? nil : character, control: control, alt: alt, shift: shift)
    }
}

/// The row above the keyboard with the keys a phone keyboard lacks.
final class KeyBar: UIInputView {
    private weak var terminal: TerminalUIView?
    private var controlButton: UIButton?
    private var altButton: UIButton?

    init(view: TerminalUIView) {
        terminal = view
        super.init(frame: CGRect(x: 0, y: 0, width: 0, height: 44), inputViewStyle: .keyboard)
        allowsSelfSizing = true
        let scroll = UIScrollView()
        scroll.showsHorizontalScrollIndicator = false
        scroll.translatesAutoresizingMaskIntoConstraints = false
        addSubview(scroll)
        let stack = UIStackView()
        stack.axis = .horizontal
        stack.spacing = 6
        stack.translatesAutoresizingMaskIntoConstraints = false
        scroll.addSubview(stack)
        NSLayoutConstraint.activate([
            scroll.leadingAnchor.constraint(equalTo: leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: trailingAnchor),
            scroll.topAnchor.constraint(equalTo: topAnchor),
            scroll.bottomAnchor.constraint(equalTo: bottomAnchor),
            heightAnchor.constraint(equalToConstant: 44),
            stack.leadingAnchor.constraint(equalTo: scroll.contentLayoutGuide.leadingAnchor, constant: 8),
            stack.trailingAnchor.constraint(equalTo: scroll.contentLayoutGuide.trailingAnchor, constant: -8),
            stack.centerYAnchor.constraint(equalTo: scroll.centerYAnchor),
            stack.heightAnchor.constraint(equalToConstant: 34),
        ])

        func button(
            _ title: String, symbol: String? = nil, label: String? = nil, action: @escaping (TerminalUIView) -> Void
        ) -> UIButton {
            var config = UIButton.Configuration.gray()
            if let symbol {
                config.image = UIImage(
                    systemName: symbol, withConfiguration: UIImage.SymbolConfiguration(pointSize: 13, weight: .semibold)
                )
            } else {
                config.attributedTitle = AttributedString(
                    title, attributes: .init([.font: UIFont.monospacedSystemFont(ofSize: 14, weight: .medium)]))
            }
            config.contentInsets = NSDirectionalEdgeInsets(top: 4, leading: 10, bottom: 4, trailing: 10)
            config.cornerStyle = .medium
            let button = UIButton(
                configuration: config,
                primaryAction: UIAction { [weak self] _ in
                    guard let terminal = self?.terminal else { return }
                    UIDevice.current.playInputClick()
                    action(terminal)
                })
            button.accessibilityLabel = label ?? title
            stack.addArrangedSubview(button)
            return button
        }

        _ = button("esc", label: "Escape") { $0.send(RemoteKey(key: "escape")) }
        controlButton = button("ctrl", label: "Control") { $0.toggleControl() }
        altButton = button("alt", label: "Option") { $0.toggleAlt() }
        _ = button("tab", label: "Tab") { $0.send(RemoteKey(key: "tab")) }
        _ = button("", symbol: "arrow.left", label: "Left") { $0.send(RemoteKey(key: "left")) }
        _ = button("", symbol: "arrow.up", label: "Up") { $0.send(RemoteKey(key: "up")) }
        _ = button("", symbol: "arrow.down", label: "Down") { $0.send(RemoteKey(key: "down")) }
        _ = button("", symbol: "arrow.right", label: "Right") { $0.send(RemoteKey(key: "right")) }
        for character in ["~", "|", "/", "-", "_", "\\", "`", "$", "*"] {
            _ = button(character) { $0.send(.text(character)) }
        }
        for (title, key) in [("home", "home"), ("end", "end"), ("pg↑", "pageup"), ("pg↓", "pagedown"), ("⇧tab", "tab")]
        {
            _ = button(title) { $0.send(RemoteKey(key: key, shift: title == "⇧tab")) }
        }
        for number in 1...12 { _ = button("F\(number)") { $0.send(RemoteKey(key: "f\(number)")) } }
        _ = button("", symbol: "doc.on.clipboard", label: "Paste") { $0.paste() }
        _ = button("", symbol: "keyboard.chevron.compact.down", label: "Hide keyboard") {
            _ = $0.resignFirstResponder()
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    func setArmed(control: Bool, alt: Bool) {
        controlButton?.configuration?.baseBackgroundColor = control ? .tintColor : nil
        controlButton?.configuration?.baseForegroundColor = control ? .white : nil
        altButton?.configuration?.baseBackgroundColor = alt ? .tintColor : nil
        altButton?.configuration?.baseForegroundColor = alt ? .white : nil
    }
}

/// SwiftUI host for `TerminalUIView`.
struct TerminalViewRepresentable: UIViewRepresentable {
    let screen: TerminalScreen
    let link: HostLink
    /// Set by the view so SwiftUI can ask for the readable grid.
    var onReady: ((TerminalUIView) -> Void)?

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
        view.screen = screen
        onReady?(view)
        DispatchQueue.main.async { _ = view.becomeFirstResponder() }
        return view
    }

    func updateUIView(_ view: TerminalUIView, context: Context) {
        if view.screen !== screen { view.screen = screen }
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
