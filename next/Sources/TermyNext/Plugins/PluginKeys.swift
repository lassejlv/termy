import AppKit
import TermyNextCore

extension KeyChord {
    /// The chord `event` presses, as plugins write it.
    @MainActor init?(event: NSEvent) {
        let modifiers = Self.modifiers(of: event)
        if let name = Self.namedKeyCodes[event.keyCode] {
            self.init(key: name, modifiers: modifiers)
            return
        }
        // The key as typed without modifiers, so shift+cmd+a is "a" with shift.
        guard let plain = event.characters(byApplyingModifiers: []), plain.count == 1 else { return nil }
        self.init(key: plain.lowercased(), modifiers: modifiers)
    }

    /// The physical key `event` presses, `[KeyD]`, whatever the layout types.
    @MainActor static func physical(_ event: NSEvent) -> KeyChord? {
        guard let name = physicalKeys.first(where: { $0.value == event.keyCode })?.key else { return nil }
        return KeyChord(key: "[\(name)]", modifiers: modifiers(of: event))
    }

    @MainActor private static func modifiers(of event: NSEvent) -> Modifiers {
        let flags = event.modifierFlags.intersection([.command, .shift, .option, .control])
        var modifiers: Modifiers = []
        if flags.contains(.command) { modifiers.insert(.command) }
        if flags.contains(.shift) { modifiers.insert(.shift) }
        if flags.contains(.option) { modifiers.insert(.option) }
        if flags.contains(.control) { modifiers.insert(.control) }
        return modifiers
    }

    static let namedKeyCodes: [UInt16: String] = [
        36: "enter", 76: "enter", 48: "tab", 53: "escape", 49: "space", 51: "backspace", 117: "delete",
        126: "up", 125: "down", 123: "left", 124: "right", 115: "home", 119: "end", 116: "pageup", 121: "pagedown",
        114: "insert", 122: "f1", 120: "f2", 99: "f3", 118: "f4", 96: "f5", 97: "f6", 98: "f7", 100: "f8",
        101: "f9", 109: "f10", 103: "f11", 111: "f12", 105: "f13", 107: "f14", 113: "f15", 106: "f16",
        64: "f17", 79: "f18", 80: "f19", 90: "f20",
    ]
}

/// Plugin key bindings in a window: sequences such as `ctrl+b>c` wait for
/// their next key, and modes (`client.mode.enter`) switch which bindings
/// apply, as Rex's app does.
@MainActor
final class PluginKeys {
    static let shared = PluginKeys()

    enum Outcome {
        /// A binding took the key, or it started a sequence.
        case consumed
        /// `rex.unbind` took the key from Termy Next's own shortcuts: it goes to the terminal.
        case terminal
        case none
    }

    private var pending: [ObjectIdentifier: [KeyChord]] = [:]
    private var modes: [ObjectIdentifier: [String]] = [:]

    func activeMode(in controller: WindowController) -> String? { modes[ObjectIdentifier(controller)]?.last }

    func enter(_ mode: String, in controller: WindowController) {
        modes[ObjectIdentifier(controller), default: []].append(mode)
        pending[ObjectIdentifier(controller)] = nil
    }

    func exit(in controller: WindowController) { _ = modes[ObjectIdentifier(controller)]?.popLast() }

    func exitAll(in controller: WindowController) { modes[ObjectIdentifier(controller)] = nil }

    func handle(_ event: NSEvent, controller: WindowController) -> Outcome {
        let snapshot = PluginHost.shared.snapshot
        guard let logical = KeyChord(event: event) else { return .none }
        let id = ObjectIdentifier(controller)
        let waiting = pending[id] ?? []
        let mode = activeMode(in: controller)
        let chords = [logical, KeyChord.physical(event)].compactMap { $0 }

        func match(_ mode: String?) -> Outcome? {
            for chord in chords {
                let keys = waiting + [chord]
                if let binding = snapshot.binding(mode: mode, keys: keys) {
                    pending[id] = nil
                    run(binding, chord: logical, controller: controller)
                    return .consumed
                }
                if snapshot.startsSequence(mode: mode, keys: keys) {
                    pending[id] = keys
                    return .consumed
                }
            }
            return nil
        }

        if let mode {
            if let outcome = match(mode) { return outcome }
            if snapshot.mode(mode)?.exclusive == true {
                pending[id] = nil
                // Nothing else works in an exclusive mode; Escape leaves it.
                if logical == KeyChord(key: "escape", modifiers: []) { exit(in: controller) }
                return .consumed
            }
        }
        if let outcome = match(nil) { return outcome }
        if !waiting.isEmpty {
            // No binding finished the sequence: only this key goes on.
            pending[id] = nil
            return .none
        }
        if snapshot.removals.contains(where: { $0.mode == nil && ($0.chords == [logical] || chords.contains($0.chords.first!) && $0.chords.count == 1) }) {
            return .terminal
        }
        return .none
    }

    private func run(_ binding: PluginBinding, chord: KeyChord, controller: WindowController) {
        let context = PluginAPI.shared.context(for: controller, origin: "key")
        let pane = controller.session.selectedTab?.focusedPane
        let key: [String: Any] = ["key": binding.spec.keys, "mode": binding.spec.mode as Any]
        PluginHost.shared.press(binding.target, context: context, key: key) { [weak pane] in
            // A function that didn't take the key: it goes to the pane.
            if let pane { PluginAPI.shared.send(chord, to: pane) }
        }
    }
}
