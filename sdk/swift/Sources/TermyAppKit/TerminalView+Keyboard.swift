#if os(macOS)
import AppKit
import Termy

extension TerminalView: @preconcurrency NSTextInputClient {
    public override func keyDown(with event: NSEvent) {
        selection = nil
        if markedText != nil || KeyMapping.routesThroughInputMethod(event, optionAsAlt: optionAsAlt) {
            // Let the input method compose text (IME, dead keys); it calls insertText.
            inputContext?.handleEvent(event)
            return
        }
        guard let keystroke = KeyMapping.keystroke(for: event) else { return }
        sendPress(keystroke, keyCode: event.keyCode)
    }

    public override func keyUp(with event: NSEvent) {
        // Release events are only reported in Kitty keyboard modes; the encoder
        // returns nothing otherwise. A key whose press didn't reach the
        // program, such as a shortcut, has nothing to release.
        guard pressedKeyCodes.remove(event.keyCode) != nil, markedText == nil,
              var keystroke = KeyMapping.keystroke(for: event) else { return }
        keystroke.kind = .release
        send(keystroke)
    }

    public override func performKeyEquivalent(with event: NSEvent) -> Bool {
        // AppKit opens the context menu for Control-Return; programs get the key instead.
        if window?.firstResponder === self, event.type == .keyDown,
           KeyMapping.namedKey(forKeyCode: event.keyCode) == "enter",
           event.modifierFlags.contains(.control), !event.modifierFlags.contains(.command) {
            keyDown(with: event)
            return true
        }
        guard window?.firstResponder === self, event.type == .keyDown,
              event.modifierFlags.intersection(.deviceIndependentFlagsMask) == .command
        else { return super.performKeyEquivalent(with: event) }
        switch event.charactersIgnoringModifiers {
        case "c": copy(nil)
        case "v": paste(nil)
        case "a": selectAll(nil)
        case "k": clearScrollback(nil)
        default: return super.performKeyEquivalent(with: event)
        }
        return true
    }

    func send(_ keystroke: Keystroke) {
        try? session.send(keystroke, optionAsAlt: optionAsAlt)
    }

    /// Sends a key's press and remembers it, so its release is reported.
    private func sendPress(_ keystroke: Keystroke, keyCode: UInt16) {
        pressedKeyCodes.insert(keyCode)
        send(keystroke)
    }

    // MARK: Edit actions

    @objc public func copy(_ sender: Any?) {
        guard let selection else { return }
        let text = selection.text(in: session.frame)
        pasteboard.clearContents()
        pasteboard.setString(text, forType: .string)
        delegate?.terminalView(self, didCopy: text)
    }

    @objc public func paste(_ sender: Any?) {
        guard let text = Self.pasteText(from: pasteboard), !text.isEmpty else { return }
        try? session.paste(text)
    }

    /// The text a paste types: copied files become shell-quoted paths, so a
    /// file name can't inject commands or split into several arguments.
    static func pasteText(from pasteboard: NSPasteboard) -> String? {
        let files = pasteboard.readObjects(
            forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL] ?? []
        if !files.isEmpty {
            return files.map { shellQuoted($0.path) }.joined(separator: " ")
        }
        if let string = pasteboard.string(forType: .string) { return string }
        let urls = pasteboard.readObjects(forClasses: [NSURL.self]) as? [URL] ?? []
        return urls.isEmpty ? nil : urls.map(\.absoluteString).joined(separator: " ")
    }

    /// Quotes a word for POSIX shells, leaving plain paths readable.
    static func shellQuoted(_ word: String) -> String {
        let plain = CharacterSet(charactersIn: "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_@%+=:,./-")
        if !word.isEmpty, word.unicodeScalars.allSatisfy({ plain.contains($0) }) { return word }
        return "'" + word.replacingOccurrences(of: "'", with: "'\\''") + "'"
    }

    @objc public override func selectAll(_ sender: Any?) {
        let frame = session.frame
        guard !frame.isEmpty else { return }
        selection = TerminalSelection(
            anchor: GridPosition(row: 0, col: 0), head: GridPosition(row: frame.rows - 1, col: frame.cols - 1))
    }

    @objc public func clearScrollback(_ sender: Any?) {
        if (try? session.terminal.clearScrollback()) == true { session.poll() }
    }

    // MARK: NSTextInputClient

    public func insertText(_ string: Any, replacementRange: NSRange) {
        let text = (string as? NSAttributedString)?.string ?? (string as? String) ?? ""
        clearMarkedText()
        guard !text.isEmpty else { return }
        // Keep the physical key when the input method passed a keypress through,
        // so enhanced keyboard modes report it accurately.
        if let event = NSApp.currentEvent, event.type == .keyDown, event.characters == text,
           let keystroke = KeyMapping.keystroke(for: event) {
            sendPress(keystroke, keyCode: event.keyCode)
        } else {
            send(.text(text))
        }
    }

    public func setMarkedText(_ string: Any, selectedRange: NSRange, replacementRange: NSRange) {
        let text = (string as? NSAttributedString)?.string ?? (string as? String) ?? ""
        markedText = text.isEmpty ? nil : text
        invalidateCursorRow()
    }

    public func unmarkText() {
        clearMarkedText()
    }

    private func clearMarkedText() {
        guard markedText != nil else { return }
        markedText = nil
        invalidateCursorRow()
    }

    private func invalidateCursorRow() {
        setNeedsDisplay(renderer.rowRect(session.frame.cursor.row, cols: session.frame.cols))
    }

    public func hasMarkedText() -> Bool { markedText != nil }

    public func markedRange() -> NSRange {
        markedText.map { NSRange(location: 0, length: ($0 as NSString).length) } ?? NSRange(location: NSNotFound, length: 0)
    }

    public func selectedRange() -> NSRange { NSRange(location: 0, length: 0) }

    public func validAttributesForMarkedText() -> [NSAttributedString.Key] { [] }

    public func attributedSubstring(forProposedRange range: NSRange, actualRange: NSRangePointer?) -> NSAttributedString? {
        nil
    }

    public func characterIndex(for point: NSPoint) -> Int { NSNotFound }

    /// Positions the IME candidate window at the cursor.
    public func firstRect(forCharacterRange range: NSRange, actualRange: NSRangePointer?) -> NSRect {
        let cursor = session.frame.cursor
        let rect = renderer.cellRect(row: cursor.row, col: cursor.col)
        guard let window else { return .zero }
        return window.convertToScreen(convert(rect, to: nil))
    }

    public override func doCommand(by selector: Selector) {
        // Non-text keys are encoded in keyDown, except while text is being
        // composed: the input method then gets them, and passes back the ones
        // it doesn't use, such as Return or Tab after a dead key.
        guard let event = NSApp.currentEvent, event.type == .keyDown,
              KeyMapping.namedKey(forKeyCode: event.keyCode) != nil,
              let keystroke = KeyMapping.keystroke(for: event)
        else { return }
        sendPress(keystroke, keyCode: event.keyCode)
    }
}
#endif
