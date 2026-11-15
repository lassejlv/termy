import SwiftUI
import TermyRemoteKit
import UIKit

/// What a key bar key does.
enum KeyAction: Equatable {
    case key(RemoteKey)
    case text(String)
    case control
    case alt
    case paste
    case hideKeyboard
    case functionRow
    /// Asks the SwiftUI host, like a ⌘ shortcut does.
    case command(TerminalCommand)
}

/// A key the bar can show. Users choose and order them in Settings.
enum KeyBarKey: String, Codable, CaseIterable, Identifiable {
    case escape, control, alt, tab, shiftTab
    case left, up, down, right
    case home, end, pageUp, pageDown
    case tilde, pipe, slash, dash, dollar, star
    case functionKeys, snippets, paste, hideKeyboard
    case previousPrompt, nextPrompt
    case voice, photo

    var id: String { rawValue }

    static let defaultLayout: [KeyBarKey] = [
        .escape, .control, .alt, .tab, .left, .up, .down, .right, .tilde, .pipe, .slash, .dash, .dollar, .star,
        .functionKeys, .snippets, .voice, .paste, .hideKeyboard,
    ]

    var title: String {
        switch self {
        case .escape: "esc"
        case .control: "ctrl"
        case .alt: "alt"
        case .tab: "tab"
        case .shiftTab: "⇧tab"
        case .home: "home"
        case .end: "end"
        case .pageUp: "pg↑"
        case .pageDown: "pg↓"
        case .tilde: "~"
        case .pipe: "|"
        case .slash: "/"
        case .dash: "-"
        case .dollar: "$"
        case .star: "*"
        case .functionKeys: "fn"
        case .left, .up, .down, .right, .snippets, .paste, .hideKeyboard, .previousPrompt, .nextPrompt, .voice, .photo: ""
        }
    }

    var symbol: String? {
        switch self {
        case .left: "arrow.left"
        case .up: "arrow.up"
        case .down: "arrow.down"
        case .right: "arrow.right"
        case .snippets: "text.badge.plus"
        case .paste: "doc.on.clipboard"
        case .hideKeyboard: "keyboard.chevron.compact.down"
        case .previousPrompt: "chevron.up.2"
        case .nextPrompt: "chevron.down.2"
        case .voice: "mic"
        case .photo: "photo"
        default: nil
        }
    }

    /// The name VoiceOver reads and Settings lists.
    var label: String {
        switch self {
        case .escape: "Escape"
        case .control: "Control"
        case .alt: "Option"
        case .tab: "Tab"
        case .shiftTab: "Shift-Tab"
        case .left: "Left Arrow"
        case .up: "Up Arrow"
        case .down: "Down Arrow"
        case .right: "Right Arrow"
        case .home: "Home"
        case .end: "End"
        case .pageUp: "Page Up"
        case .pageDown: "Page Down"
        case .tilde: "Tilde"
        case .pipe: "Pipe"
        case .slash: "Slash"
        case .dash: "Dash"
        case .dollar: "Dollar"
        case .star: "Asterisk"
        case .functionKeys: "Function Keys"
        case .snippets: "Snippets"
        case .paste: "Paste"
        case .hideKeyboard: "Hide Keyboard"
        case .previousPrompt: "Previous Prompt"
        case .nextPrompt: "Next Prompt"
        case .voice: "Speak a Command"
        case .photo: "Send Photo"
        }
    }

    var primary: KeyAction {
        switch self {
        case .escape: .key(RemoteKey(key: "escape"))
        case .control: .control
        case .alt: .alt
        case .tab: .key(RemoteKey(key: "tab"))
        case .shiftTab: .key(RemoteKey(key: "tab", shift: true))
        case .left: .key(RemoteKey(key: "left"))
        case .up: .key(RemoteKey(key: "up"))
        case .down: .key(RemoteKey(key: "down"))
        case .right: .key(RemoteKey(key: "right"))
        case .home: .key(RemoteKey(key: "home"))
        case .end: .key(RemoteKey(key: "end"))
        case .pageUp: .key(RemoteKey(key: "pageup"))
        case .pageDown: .key(RemoteKey(key: "pagedown"))
        case .tilde: .text("~")
        case .pipe: .text("|")
        case .slash: .text("/")
        case .dash: .text("-")
        case .dollar: .text("$")
        case .star: .text("*")
        case .functionKeys: .functionRow
        case .snippets: .functionRow  // Handled as a menu; see `KeyBar`.
        case .paste: .paste
        case .hideKeyboard: .hideKeyboard
        case .previousPrompt: .command(.previousPrompt)
        case .nextPrompt: .command(.nextPrompt)
        case .voice: .command(.dictate)
        case .photo: .command(.sendPhoto)
        }
    }

    /// Swipe up on the key for this; its title shows in the key's corner.
    var secondary: (title: String, action: KeyAction)? {
        switch self {
        case .tab: ("⇧", .key(RemoteKey(key: "tab", shift: true)))
        case .left: ("home", .key(RemoteKey(key: "home")))
        case .right: ("end", .key(RemoteKey(key: "end")))
        case .up: ("pg↑", .key(RemoteKey(key: "pageup")))
        case .down: ("pg↓", .key(RemoteKey(key: "pagedown")))
        case .tilde: ("`", .text("`"))
        case .pipe: ("&", .text("&"))
        case .slash: ("\\", .text("\\"))
        case .dash: ("_", .text("_"))
        case .dollar: ("#", .text("#"))
        case .star: ("^", .text("^"))
        default: nil
        }
    }

    /// Held down, the key repeats like a keyboard key.
    var repeats: Bool {
        switch self {
        case .left, .up, .down, .right, .pageUp, .pageDown: true
        default: false
        }
    }
}

enum KeyBarLayout {
    static let defaultsKey = "keyBarKeys"

    static func load(_ defaults: UserDefaults = .standard) -> [KeyBarKey] {
        guard let raw = defaults.stringArray(forKey: defaultsKey) else { return KeyBarKey.defaultLayout }
        var seen = Set<KeyBarKey>()
        let keys = raw.compactMap(KeyBarKey.init(rawValue:)).filter { seen.insert($0).inserted }
        return keys.isEmpty ? KeyBarKey.defaultLayout : keys
    }

    static func save(_ keys: [KeyBarKey], _ defaults: UserDefaults = .standard) {
        defaults.set(keys.map(\.rawValue), forKey: defaultsKey)
    }
}

/// Repeats an action while a key is held: a pause, then a steady rate.
@MainActor
final class KeyRepeater {
    private var timer: Timer?
    private(set) var fired = false

    func start(delay: TimeInterval = 0.4, interval: TimeInterval = 0.06, action: @escaping @MainActor () -> Void) {
        stop()
        fired = false
        timer = Timer.scheduledTimer(withTimeInterval: delay, repeats: false) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self else { return }
                self.fired = true
                action()
                self.timer = Timer.scheduledTimer(withTimeInterval: interval, repeats: true) { _ in
                    MainActor.assumeIsolated { action() }
                }
            }
        }
    }

    func stop() {
        timer?.invalidate()
        timer = nil
    }
}

/// Sticky modifier state: tap arms it for one key, double-tap locks it on.
enum ModifierState {
    case off, armed, locked
}

/// One key cap. Tap for the key, swipe up for the one in its corner, hold to repeat.
final class KeyCap: UIView {
    let key: KeyBarKey?
    private let title = UILabel()
    private let corner = UILabel()
    private let icon = UIImageView()
    private let repeater = KeyRepeater()
    private var start: CGPoint?
    private var swiped = false
    private let primary: () -> Void
    private let secondary: (() -> Void)?
    private let repeats: Bool

    var modifier: ModifierState = .off { didSet { restyle() } }
    private var pressed = false { didSet { restyle() } }

    init(
        key: KeyBarKey?, title text: String, symbol: String?, corner cornerText: String?, label: String,
        repeats: Bool, primary: @escaping () -> Void, secondary: (() -> Void)?
    ) {
        self.key = key
        self.primary = primary
        self.secondary = secondary
        self.repeats = repeats
        super.init(frame: .zero)
        layer.cornerRadius = 7
        layer.cornerCurve = .continuous
        isAccessibilityElement = true
        accessibilityLabel = label
        accessibilityTraits = [.keyboardKey]
        if let secondary, let cornerText {
            accessibilityCustomActions = [
                UIAccessibilityCustomAction(name: cornerText) { _ in
                    secondary()
                    return true
                }
            ]
        }
        let content: UIView
        if let symbol {
            icon.image = UIImage(
                systemName: symbol, withConfiguration: UIImage.SymbolConfiguration(pointSize: 13, weight: .semibold))
            icon.contentMode = .center
            content = icon
        } else {
            title.text = text
            title.font = .monospacedSystemFont(ofSize: 14, weight: .medium)
            title.textAlignment = .center
            content = title
        }
        content.translatesAutoresizingMaskIntoConstraints = false
        addSubview(content)
        corner.text = cornerText
        corner.font = .monospacedSystemFont(ofSize: 8, weight: .regular)
        corner.textColor = .secondaryLabel
        corner.translatesAutoresizingMaskIntoConstraints = false
        addSubview(corner)
        NSLayoutConstraint.activate([
            content.centerXAnchor.constraint(equalTo: centerXAnchor),
            content.centerYAnchor.constraint(equalTo: centerYAnchor, constant: cornerText == nil ? 0 : 2),
            content.leadingAnchor.constraint(greaterThanOrEqualTo: leadingAnchor, constant: 10),
            widthAnchor.constraint(greaterThanOrEqualToConstant: 40),
            corner.topAnchor.constraint(equalTo: topAnchor, constant: 2),
            corner.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -4),
        ])
        restyle()
    }

    @available(*, unavailable) required init?(coder: NSCoder) { fatalError() }

    private func restyle() {
        let active = modifier != .off
        backgroundColor =
            active ? .tintColor : (pressed ? UIColor.systemGray3 : UIColor.secondarySystemFill)
        title.textColor = active ? .white : .label
        icon.tintColor = .label
        title.attributedText = NSAttributedString(
            string: title.text ?? "",
            attributes: [
                .font: UIFont.monospacedSystemFont(ofSize: 14, weight: modifier == .locked ? .heavy : .medium),
                .underlineStyle: modifier == .locked ? NSUnderlineStyle.single.rawValue : 0,
                .foregroundColor: active ? UIColor.white : UIColor.label,
            ])
        accessibilityValue = modifier == .locked ? "Locked" : (modifier == .armed ? "On" : nil)
    }

    override func accessibilityActivate() -> Bool {
        primary()
        return true
    }

    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent?) {
        start = touches.first?.location(in: self)
        swiped = false
        pressed = true
        if repeats { repeater.start { [weak self] in self?.primary() } }
    }

    override func touchesMoved(_ touches: Set<UITouch>, with event: UIEvent?) {
        guard let start, let point = touches.first?.location(in: self), secondary != nil else { return }
        if !swiped, start.y - point.y > 16 {
            swiped = true
            repeater.stop()
            corner.textColor = .label
            UISelectionFeedbackGenerator().selectionChanged()
        }
    }

    override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent?) {
        let repeated = repeater.fired
        finish()
        if swiped, let secondary {
            secondary()
        } else if !repeated {
            primary()
        }
    }

    override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent?) { finish() }

    private func finish() {
        repeater.stop()
        start = nil
        pressed = false
        corner.textColor = .secondaryLabel
    }
}

/// The row above the keyboard with the keys a phone keyboard lacks.
final class KeyBar: UIInputView, UIInputViewAudioFeedback {
    private weak var terminal: TerminalUIView?
    private let scroll = UIScrollView()
    private let stack = UIStackView()
    private(set) var layout: [KeyBarKey] = []
    private var showingFunctionRow = false
    private var controlCap: KeyCap?
    private var altCap: KeyCap?

    var enableInputClicksWhenVisible: Bool { true }

    init(view: TerminalUIView) {
        terminal = view
        super.init(frame: CGRect(x: 0, y: 0, width: 0, height: 44), inputViewStyle: .keyboard)
        allowsSelfSizing = true
        scroll.showsHorizontalScrollIndicator = false
        scroll.delaysContentTouches = false
        scroll.translatesAutoresizingMaskIntoConstraints = false
        addSubview(scroll)
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
        reload()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    /// Rebuilds the keys when Settings changed them.
    func reloadIfNeeded() {
        if KeyBarLayout.load() != layout { reload() }
    }

    private func reload() {
        layout = KeyBarLayout.load()
        rebuild()
    }

    private func rebuild() {
        for view in stack.arrangedSubviews { view.removeFromSuperview() }
        controlCap = nil
        altCap = nil
        if showingFunctionRow {
            add(cap(nil, title: "fn", symbol: nil, label: "Back to keys") { [weak self] in self?.toggleFunctionRow() })
            for number in 1...12 {
                add(
                    cap(nil, title: "F\(number)", symbol: nil, label: "F\(number)") { [weak self] in
                        self?.terminal?.send(RemoteKey(key: "f\(number)"))
                    })
            }
        } else {
            for key in layout {
                if key == .snippets {
                    add(snippetsButton())
                    continue
                }
                let view = cap(key, title: key.title, symbol: key.symbol, label: key.label)
                if key == .control { controlCap = view }
                if key == .alt { altCap = view }
                add(view)
            }
        }
        if let terminal { setModifiers(control: terminal.controlState, alt: terminal.altState) }
        scroll.setContentOffset(.zero, animated: false)
    }

    private func add(_ view: UIView) { stack.addArrangedSubview(view) }

    private func cap(
        _ key: KeyBarKey?, title: String, symbol: String?, label: String, action: (() -> Void)? = nil
    ) -> KeyCap {
        KeyCap(
            key: key, title: title, symbol: symbol, corner: key?.secondary?.title, label: label,
            repeats: key?.repeats ?? false,
            primary: { [weak self] in
                UIDevice.current.playInputClick()
                if let action { action() } else if let key { self?.perform(key.primary) }
            },
            secondary: key?.secondary.map { secondary in
                { [weak self] in
                    UIDevice.current.playInputClick()
                    self?.perform(secondary.action)
                }
            })
    }

    private func snippetsButton() -> UIButton {
        var config = UIButton.Configuration.gray()
        config.image = UIImage(
            systemName: "text.badge.plus", withConfiguration: UIImage.SymbolConfiguration(pointSize: 13, weight: .semibold))
        config.contentInsets = NSDirectionalEdgeInsets(top: 4, leading: 10, bottom: 4, trailing: 10)
        config.cornerStyle = .medium
        config.baseForegroundColor = .label
        let button = UIButton(configuration: config)
        button.accessibilityLabel = "Snippets"
        button.showsMenuAsPrimaryAction = true
        button.menu = UIMenu(children: [
            UIDeferredMenuElement.uncached { [weak self] completion in
                MainActor.assumeIsolated {
                    let snippets = SnippetStore.shared.snippets
                    guard !snippets.isEmpty else {
                        completion([
                            UIAction(title: "Add snippets in Settings", attributes: .disabled) { _ in }
                        ])
                        return
                    }
                    completion(
                        snippets.map { snippet in
                            UIAction(
                                title: snippet.title, subtitle: snippet.name.isEmpty ? nil : snippet.command,
                                image: UIImage(systemName: snippet.runs ? "return" : "text.cursor")
                            ) { _ in self?.terminal?.type(snippet) }
                        })
                }
            }
        ])
        return button
    }

    private func perform(_ action: KeyAction) {
        guard let terminal else { return }
        switch action {
        case .key(let key): terminal.send(key)
        case .text(let text): terminal.send(.text(text))
        case .control: terminal.tapModifier(control: true)
        case .alt: terminal.tapModifier(control: false)
        case .paste: terminal.paste()
        case .hideKeyboard: _ = terminal.resignFirstResponder()
        case .functionRow: toggleFunctionRow()
        case .command(let command): terminal.onCommand?(command)
        }
    }

    private func toggleFunctionRow() {
        showingFunctionRow.toggle()
        rebuild()
    }

    func setModifiers(control: ModifierState, alt: ModifierState) {
        controlCap?.modifier = control
        altCap?.modifier = alt
    }
}

/// Settings: which keys the bar shows, in what order.
struct KeyBarSettingsView: View {
    @State private var keys = KeyBarLayout.load()

    var body: some View {
        List {
            Section {
                ForEach(keys) { key in KeyRow(key: key) }
                    .onMove { keys.move(fromOffsets: $0, toOffset: $1) }
                    .onDelete { keys.remove(atOffsets: $0) }
            } header: {
                Text("Shown")
            } footer: {
                Text(
                    "Swipe up on a key for the character in its corner. Hold arrows to repeat. Double-tap ctrl or alt to lock it."
                )
            }
            let hidden = KeyBarKey.allCases.filter { !keys.contains($0) }
            if !hidden.isEmpty {
                Section("More Keys") {
                    ForEach(hidden) { key in
                        Button {
                            keys.append(key)
                        } label: {
                            HStack {
                                KeyRow(key: key)
                                Spacer()
                                Image(systemName: "plus.circle.fill").foregroundStyle(.green)
                            }
                        }
                        .foregroundStyle(.primary)
                    }
                }
            }
            Section {
                Button("Restore Default Keys") { keys = KeyBarKey.defaultLayout }
            }
        }
        .environment(\.editMode, .constant(.active))
        .navigationTitle("Key Bar")
        .onChange(of: keys) { _, keys in KeyBarLayout.save(keys.isEmpty ? KeyBarKey.defaultLayout : keys) }
    }
}

private struct KeyRow: View {
    let key: KeyBarKey
    var body: some View {
        HStack(spacing: 12) {
            Group {
                if let symbol = key.symbol {
                    Image(systemName: symbol)
                } else {
                    Text(key.title).font(.callout.monospaced())
                }
            }
            .frame(minWidth: 44, minHeight: 28)
            .background(.quaternary, in: RoundedRectangle(cornerRadius: 6))
            Text(key.label)
            if let secondary = key.secondary {
                Text("↑ \(secondary.title)").font(.caption.monospaced()).foregroundStyle(.secondary)
            }
        }
    }
}
