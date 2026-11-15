import AppKit
import TermyNextCore

/// The key each command uses: its default (`Command.defaultShortcut`) unless
/// the user set another one, or none, in Settings › Keyboard. Menus, the
/// palette, tooltips and key handling all read it from here.
@MainActor
enum Shortcuts {
    typealias Binding = (key: String, modifiers: NSEvent.ModifierFlags)

    static let didChange = Notification.Name("TermyNextShortcutsDidChange")
    private static let defaultsKey = "customShortcuts"

    /// Command → chord as plugins write it (`shift+cmd+d`); "" for none.
    private static var overrides: [String: String] {
        get { UserDefaults.standard.dictionary(forKey: defaultsKey) as? [String: String] ?? [:] }
        set {
            UserDefaults.standard.set(newValue, forKey: defaultsKey)
            cache = nil
            NotificationCenter.default.post(name: didChange, object: nil)
        }
    }

    private static var cache: [Command: Binding]?

    /// The command's shortcut now.
    static func binding(for command: Command) -> Binding? {
        if cache == nil {
            var map: [Command: Binding] = [:]
            let overrides = overrides
            for command in Command.allCases {
                if let spec = overrides[command.rawValue] {
                    if let chord = KeyChord(spec), let binding = binding(chord) { map[command] = binding }
                } else if let binding = command.defaultShortcut {
                    map[command] = binding
                }
            }
            cache = map
        }
        return cache?[command]
    }

    static func chord(for command: Command) -> KeyChord? { binding(for: command).flatMap(chord) }

    /// The user changed it from the default.
    static func isCustom(_ command: Command) -> Bool { overrides[command.rawValue] != nil }

    /// The command that has `chord` now, if any.
    static func command(using chord: KeyChord) -> Command? {
        Command.allCases.first { self.chord(for: $0) == chord }
    }

    /// Gives `command` `chord` (nil for none). Another command with the same
    /// chord loses it, so a key does one thing.
    static func set(_ chord: KeyChord?, for command: Command) {
        var map = overrides
        if let chord, let other = self.command(using: chord), other != command {
            map[other.rawValue] = ""
        }
        let isDefault = chord == command.defaultShortcut.flatMap(self.chord)
        map[command.rawValue] = isDefault ? nil : (chord?.description ?? "")
        overrides = map
    }

    static func reset(_ command: Command) {
        var map = overrides
        map[command.rawValue] = nil
        // Its default may be another command's custom key now; that one keeps it.
        if let chord = command.defaultShortcut.flatMap(chord), let other = self.command(using: chord), other != command, isCustom(other) {
            map[command.rawValue] = ""
        }
        overrides = map
    }

    static func resetAll() { overrides = [:] }

    static var hasCustom: Bool { !overrides.isEmpty }

    // MARK: Chords and menu key equivalents

    private static func functionKey(_ code: Int) -> String { String(UnicodeScalar(UInt32(code))!) }

    /// Key names chords use, as menu key equivalents.
    private static let equivalents: [String: String] = {
        var map: [String: String] = [
            "enter": "\r", "tab": "\t", "escape": "\u{1b}", "space": " ", "backspace": "\u{8}",
            "delete": functionKey(NSDeleteFunctionKey), "up": functionKey(NSUpArrowFunctionKey),
            "down": functionKey(NSDownArrowFunctionKey), "left": functionKey(NSLeftArrowFunctionKey),
            "right": functionKey(NSRightArrowFunctionKey), "home": functionKey(NSHomeFunctionKey),
            "end": functionKey(NSEndFunctionKey), "pageup": functionKey(NSPageUpFunctionKey),
            "pagedown": functionKey(NSPageDownFunctionKey), "insert": functionKey(NSInsertFunctionKey),
        ]
        for n in 1...20 { map["f\(n)"] = functionKey(NSF1FunctionKey + n - 1) }
        return map
    }()

    private static let names: [String: String] = Dictionary(uniqueKeysWithValues: equivalents.map { ($1, $0) })

    /// A chord as a menu key equivalent; nil for a physical key (`[KeyD]`).
    static func binding(_ chord: KeyChord) -> Binding? {
        guard !chord.key.hasPrefix("[") else { return nil }
        var flags: NSEvent.ModifierFlags = []
        if chord.modifiers.contains(.command) { flags.insert(.command) }
        if chord.modifiers.contains(.shift) { flags.insert(.shift) }
        if chord.modifiers.contains(.option) { flags.insert(.option) }
        if chord.modifiers.contains(.control) { flags.insert(.control) }
        return (equivalents[chord.key] ?? chord.key, flags)
    }

    static func chord(_ binding: Binding) -> KeyChord {
        var modifiers: KeyChord.Modifiers = []
        if binding.modifiers.contains(.command) { modifiers.insert(.command) }
        if binding.modifiers.contains(.shift) { modifiers.insert(.shift) }
        if binding.modifiers.contains(.option) { modifiers.insert(.option) }
        if binding.modifiers.contains(.control) { modifiers.insert(.control) }
        return KeyChord(key: names[binding.key] ?? binding.key.lowercased(), modifiers: modifiers)
    }

    /// A key a shortcut can use without breaking typing: one with ⌘, ⌃ or
    /// ⌥, or a function key.
    static func isUsable(_ chord: KeyChord) -> Bool {
        !chord.modifiers.intersection([.command, .control, .option]).isEmpty
            || (chord.key.hasPrefix("f") && Int(chord.key.dropFirst()) != nil)
    }
}
