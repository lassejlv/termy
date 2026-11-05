import SwiftUI
import TermyRemoteKit
import UIKit

/// Draws a `TerminalScreen` and turns touches and keys into `ClientMessage`s.
///
/// The grid is scaled to fit the view's width. "Fit to iPhone" instead resizes
/// the pane on the host to the grid that fits here at a readable size.
final class TerminalUIView: UIView, UIKeyInput, UITextInputTraits {
    var screen: TerminalScreen? {
        didSet {
            oldValue?.onChange = nil
            screen?.onChange = { [weak self] in self?.setNeedsDisplay() }
            setNeedsDisplay()
        }
    }

    var onKey: ((RemoteKey) -> Void)?
    var onPaste: ((String) -> Void)?
    var onScroll: ((Int) -> Void)?

    /// Sticky modifiers from the key bar, applied to the next key.
    private var controlArmed = false { didSet { keyBar.setArmed(control: controlArmed, alt: altArmed) } }
    private var altArmed = false { didSet { keyBar.setArmed(control: controlArmed, alt: altArmed) } }
    private lazy var keyBar = KeyBar(view: self)
    private var scrollRemainder: CGFloat = 0

    static let readableFontSize: CGFloat = 12

    override init(frame: CGRect) {
        super.init(frame: frame)
        isOpaque = true
        contentMode = .redraw
        addGestureRecognizer(UITapGestureRecognizer(target: self, action: #selector(tapped)))
        let pan = UIPanGestureRecognizer(target: self, action: #selector(panned(_:)))
        pan.maximumNumberOfTouches = 1
        addGestureRecognizer(pan)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    // MARK: Layout

    private func font(size: CGFloat, bold: Bool = false, italic: Bool = false) -> UIFont {
        var font = UIFont.monospacedSystemFont(ofSize: size, weight: bold ? .bold : .regular)
        if italic, let descriptor = font.fontDescriptor.withSymbolicTraits(.traitItalic) {
            font = UIFont(descriptor: descriptor, size: size)
        }
        return font
    }

    private func cellSize(fontSize: CGFloat) -> CGSize {
        let font = font(size: fontSize)
        let width = ("M" as NSString).size(withAttributes: [.font: font]).width
        return CGSize(width: width, height: ceil(font.lineHeight))
    }

    /// The font size that fits the screen's columns in the view.
    private var fittingFontSize: CGFloat {
        guard let screen, screen.cols > 0, bounds.width > 0 else { return Self.readableFontSize }
        let unit = cellSize(fontSize: 10).width / 10
        let byWidth = bounds.width / (CGFloat(screen.cols) * unit)
        let unitHeight = cellSize(fontSize: 10).height / 10
        let byHeight = bounds.height / (CGFloat(max(screen.rows, 1)) * unitHeight)
        return max(4, (min(byWidth, byHeight, 18) * 2).rounded(.down) / 2)
    }

    /// Columns and rows that fit here at a readable size, for "Fit to iPhone".
    var readableGrid: (cols: Int, rows: Int) {
        let cell = cellSize(fontSize: Self.readableFontSize)
        return (max(20, Int(bounds.width / cell.width)), max(5, Int(bounds.height / cell.height)))
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
        let background = Self.color(screen?.background ?? 0x101421ff).withAlphaComponent(1)
        context.setFillColor(background.cgColor)
        context.fill(bounds)
        guard let screen, screen.cols > 0 else { return }

        let size = fittingFontSize
        let cell = cellSize(fontSize: size)
        let regular = font(size: size)
        let origin = CGPoint(x: max(0, (bounds.width - cell.width * CGFloat(screen.cols)) / 2), y: 0)
        for (row, runs) in screen.lines.enumerated() {
            let y = origin.y + CGFloat(row) * cell.height
            guard y < rect.maxY, y + cell.height > rect.minY else { continue }
            for run in runs {
                let frame = CGRect(
                    x: origin.x + CGFloat(run.col) * cell.width, y: y,
                    width: CGFloat(run.cells) * cell.width, height: cell.height
                )
                if run.flags & CellRun.defaultBackground == 0 {
                    context.setFillColor(Self.color(run.bg).cgColor)
                    context.fill(frame)
                }
                guard run.text.contains(where: { $0 != " " }) else { continue }
                let bold = run.flags & CellRun.bold != 0, italic = run.flags & CellRun.italic != 0
                var attributes: [NSAttributedString.Key: Any] = [
                    .font: bold || italic ? font(size: size, bold: bold, italic: italic) : regular,
                    .foregroundColor: Self.color(run.fg),
                ]
                if run.flags & CellRun.underline != 0 { attributes[.underlineStyle] = NSUnderlineStyle.single.rawValue }
                if run.flags & CellRun.strikethrough != 0 {
                    attributes[.strikethroughStyle] = NSUnderlineStyle.single.rawValue
                }
                let scalars = Array(run.text.unicodeScalars)
                if scalars.count == run.cells, scalars.allSatisfy(\.isASCII) {
                    (run.text as NSString).draw(at: frame.origin, withAttributes: attributes)
                } else if scalars.count == run.cells {
                    // Fallback fonts can be wider than a cell: place each one.
                    for (index, scalar) in scalars.enumerated() where scalar != " " {
                        let point = CGPoint(x: frame.minX + CGFloat(index) * cell.width, y: y)
                        (String(Character(scalar)) as NSString).draw(at: point, withAttributes: attributes)
                    }
                } else {
                    (run.text as NSString).draw(at: frame.origin, withAttributes: attributes)
                }
            }
        }

        let cursor = screen.cursor
        if cursor.visible, cursor.row < screen.rows, cursor.col < screen.cols {
            let x = origin.x + CGFloat(cursor.col) * cell.width, y = origin.y + CGFloat(cursor.row) * cell.height
            context.setFillColor(UIColor.white.withAlphaComponent(isFirstResponder ? 0.55 : 0.3).cgColor)
            switch cursor.style {
            case "bar": context.fill(CGRect(x: x, y: y, width: max(1.5, cell.width / 6), height: cell.height))
            case "underline": context.fill(CGRect(x: x, y: y + cell.height - 2, width: cell.width, height: 2))
            default: context.fill(CGRect(x: x, y: y, width: cell.width, height: cell.height))
            }
        }
    }

    // MARK: Touch

    @objc private func tapped() {
        if isFirstResponder { resignFirstResponder() } else { becomeFirstResponder() }
    }

    @objc private func panned(_ gesture: UIPanGestureRecognizer) {
        let lineHeight = cellSize(fontSize: fittingFontSize).height
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
    var keyboardType: UIKeyboardType = .asciiCapable
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
        guard let text = UIPasteboard.general.string, !text.isEmpty else { return }
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
        let control = flags.contains(.control), alt = flags.contains(.alternate), shift = flags.contains(.shift)
        let named: String? = switch key.keyCode {
        case .keyboardUpArrow: "up"
        case .keyboardDownArrow: "down"
        case .keyboardLeftArrow: "left"
        case .keyboardRightArrow: "right"
        case .keyboardEscape: "escape"
        case .keyboardHome: "home"
        case .keyboardEnd: "end"
        case .keyboardPageUp: "pageup"
        case .keyboardPageDown: "pagedown"
        case .keyboardDeleteForward: "delete"
        case .keyboardTab where control || alt || shift: "tab"
        default: nil
        }
        if let named { return RemoteKey(key: named, control: control, alt: alt, shift: shift) }
        guard control || alt, !flags.contains(.command) else { return nil }
        let character = key.charactersIgnoringModifiers
        guard character.count == 1 else { return nil }
        return RemoteKey(key: character.lowercased(), text: control ? nil : character, control: control, alt: alt, shift: shift)
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

        func button(_ title: String, symbol: String? = nil, label: String? = nil, action: @escaping (TerminalUIView) -> Void) -> UIButton {
            var config = UIButton.Configuration.gray()
            if let symbol {
                config.image = UIImage(systemName: symbol, withConfiguration: UIImage.SymbolConfiguration(pointSize: 13, weight: .semibold))
            } else {
                config.attributedTitle = AttributedString(title, attributes: .init([.font: UIFont.monospacedSystemFont(ofSize: 14, weight: .medium)]))
            }
            config.contentInsets = NSDirectionalEdgeInsets(top: 4, leading: 10, bottom: 4, trailing: 10)
            config.cornerStyle = .medium
            let button = UIButton(configuration: config, primaryAction: UIAction { [weak self] _ in
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
        _ = button("", symbol: "doc.on.clipboard", label: "Paste") { $0.paste() }
        _ = button("", symbol: "keyboard.chevron.compact.down", label: "Hide keyboard") { _ = $0.resignFirstResponder() }
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
        view.screen = screen
        onReady?(view)
        DispatchQueue.main.async { _ = view.becomeFirstResponder() }
        return view
    }

    func updateUIView(_ view: TerminalUIView, context: Context) {
        if view.screen !== screen { view.screen = screen }
        view.setNeedsDisplay()
    }
}
