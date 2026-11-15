import AppKit
import TermyNextCore

/// Shows a command's shortcut; click it and press keys to change it.
/// ⌫ removes the shortcut, Esc (or clicking away) keeps the old one.
@MainActor
final class ShortcutField: NSView {
    let command: Command
    private let label = NSTextField(labelWithString: "")
    private let resetButton = NSButton()
    private var recording = false { didSet { refresh() } }
    /// Modifiers held while recording, shown before the key comes.
    private var held: NSEvent.ModifierFlags = []

    static let width: CGFloat = 150

    init(command: Command) {
        self.command = command
        super.init(frame: NSRect(x: 0, y: 0, width: Self.width, height: 24))
        wantsLayer = true
        layer?.cornerRadius = 6
        layer?.cornerCurve = .continuous
        label.font = .systemFont(ofSize: 12, weight: .medium)
        label.alignment = .center
        label.lineBreakMode = .byTruncatingTail
        resetButton.image = NSImage(systemSymbolName: "arrow.uturn.backward.circle.fill", accessibilityDescription: "Use the default shortcut")
        resetButton.isBordered = false
        resetButton.imageScaling = .scaleProportionallyDown
        resetButton.contentTintColor = .tertiaryLabelColor
        resetButton.target = self
        resetButton.action = #selector(reset)
        resetButton.toolTip = "Use the default shortcut"
        for view in [label, resetButton] as [NSView] {
            view.translatesAutoresizingMaskIntoConstraints = false
            addSubview(view)
        }
        translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            widthAnchor.constraint(equalToConstant: Self.width),
            heightAnchor.constraint(equalToConstant: 24),
            label.centerYAnchor.constraint(equalTo: centerYAnchor),
            label.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 22),
            label.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -22),
            resetButton.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -4),
            resetButton.centerYAnchor.constraint(equalTo: centerYAnchor),
            resetButton.widthAnchor.constraint(equalToConstant: 14),
            resetButton.heightAnchor.constraint(equalToConstant: 14),
        ])
        NotificationCenter.default.addObserver(self, selector: #selector(refreshFromStore), name: Shortcuts.didChange, object: nil)
        refresh()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    @objc private func refreshFromStore() { refresh() }

    private func refresh() {
        let current = command.shortcutLabel
        if recording {
            label.stringValue = held.isEmpty ? "Type shortcut…" : Self.symbols(held) + "…"
            label.textColor = .labelColor
        } else {
            label.stringValue = current ?? "None"
            label.textColor = current == nil ? .tertiaryLabelColor : .labelColor
        }
        resetButton.isHidden = recording || !Shortcuts.isCustom(command)
        layer?.backgroundColor = (recording ? NSColor.controlAccentColor.withAlphaComponent(0.18) : NSColor.quaternaryLabelColor.withAlphaComponent(0.12)).cgColor
        layer?.borderWidth = recording ? 1.5 : 0
        layer?.borderColor = NSColor.controlAccentColor.cgColor
        setAccessibilityValue(recording ? "Recording" : (current ?? "None"))
    }

    private static func symbols(_ flags: NSEvent.ModifierFlags) -> String {
        var s = ""
        if flags.contains(.control) { s += "⌃" }
        if flags.contains(.option) { s += "⌥" }
        if flags.contains(.shift) { s += "⇧" }
        if flags.contains(.command) { s += "⌘" }
        return s
    }

    // MARK: Recording

    override var acceptsFirstResponder: Bool { true }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    override func mouseDown(with event: NSEvent) {
        if recording { return }
        window?.makeFirstResponder(self)
        held = []
        recording = true
    }

    override func resignFirstResponder() -> Bool {
        recording = false
        return super.resignFirstResponder()
    }

    override func flagsChanged(with event: NSEvent) {
        guard recording else { return super.flagsChanged(with: event) }
        held = event.modifierFlags.intersection([.command, .shift, .option, .control])
        refresh()
    }

    /// ⌘ keys come here before the menu sees them, so ⌘D records instead of splitting.
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        guard recording else { return super.performKeyEquivalent(with: event) }
        record(event)
        return true
    }

    override func keyDown(with event: NSEvent) {
        guard recording else {
            // Return or Space starts recording, as a button would.
            if [36, 49, 76].contains(event.keyCode) { held = []; recording = true; return }
            return super.keyDown(with: event)
        }
        record(event)
    }

    private func record(_ event: NSEvent) {
        let flags = event.modifierFlags.intersection([.command, .shift, .option, .control])
        if flags.isEmpty, event.keyCode == 53 { return finish() }          // Esc keeps it
        if flags.isEmpty, [51, 117].contains(event.keyCode) {               // ⌫ removes it
            Shortcuts.set(nil, for: command)
            return finish()
        }
        guard let chord = KeyChord(event: event), Shortcuts.isUsable(chord) else {
            NSSound.beep()
            return
        }
        if let other = Shortcuts.command(using: chord), other != command {
            confirmReplacing(other, with: chord)
        } else {
            Shortcuts.set(chord, for: command)
            finish()
        }
    }

    private func confirmReplacing(_ other: Command, with chord: KeyChord) {
        let alert = NSAlert()
        let name = { (command: Command) in command.title.hasSuffix("…") ? String(command.title.dropLast()) : command.title }
        alert.messageText = "\(chord.symbolLabel) is \(name(other))'s shortcut"
        alert.informativeText = "Use it for \(name(command)) instead? \(name(other)) won't have a shortcut."
        alert.addButton(withTitle: "Use for \(name(command))")
        alert.addButton(withTitle: "Cancel")
        let apply = { [weak self] (response: NSApplication.ModalResponse) in
            guard let self else { return }
            if response == .alertFirstButtonReturn { Shortcuts.set(chord, for: self.command) }
            self.finish()
        }
        if let window { alert.beginSheetModal(for: window, completionHandler: apply) } else { apply(alert.runModal()) }
    }

    private func finish() {
        recording = false
        window?.makeFirstResponder(nil)
    }

    @objc private func reset() { Shortcuts.reset(command) }

    // MARK: Accessibility

    override func isAccessibilityElement() -> Bool { true }
    override func accessibilityRole() -> NSAccessibility.Role? { .button }
    override func accessibilityLabel() -> String? {
        "Shortcut for " + (command.title.hasSuffix("…") ? String(command.title.dropLast()) : command.title)
    }
    override func accessibilityHelp() -> String? { "Press to record a new shortcut. Delete removes it; Escape cancels." }
    override func accessibilityPerformPress() -> Bool {
        window?.makeFirstResponder(self)
        held = []
        recording = true
        return true
    }
}
