import CoreGraphics
import Foundation

// The plain parts of Termy Next's plugin API (`termy.*`, and Rex's `rex.*`
// under the same names): IDs, key specs, layouts and shell quoting. No
// AppKit, no Lua.

/// Rex's ID shapes, `kind:<uuid>`, for what Termy Next calls sessions, tabs
/// ("windows" in Rex), panes ("blocks"), floating panes ("layers") and splits.
public enum PluginID {
    public static func make(_ kind: String, _ id: UUID) -> String { "\(kind):\(id.uuidString.lowercased())" }

    /// The UUID in `text`, with or without its `kind:` prefix.
    public static func uuid(_ text: String, kind: String? = nil) -> UUID? {
        var body = Substring(text.trimmingCharacters(in: .whitespaces))
        if let colon = body.firstIndex(of: ":") {
            if let kind, body[..<colon] != kind { return nil }
            body = body[body.index(after: colon)...]
        }
        return UUID(uuidString: String(body))
    }
}

/// The creator and flavor names Rex plugins ask for. Termy Next's own names
/// work too.
public enum PluginFlavor {
    public static let terminalCreator = "com.superlogical.terminal"
    public static let shell = "com.superlogical.terminal.shell"
    public static let termyCreator = "sh.termy.terminal"
    public static let termyShell = "sh.termy.terminal.shell"

    public static func isTerminalCreator(_ name: String) -> Bool {
        [terminalCreator, termyCreator, "terminal", "termy"].contains(name)
    }

    public static func isShell(_ flavor: String) -> Bool {
        [shell, termyShell, terminalCreator, termyCreator, "shell", "terminal"].contains(flavor)
    }

    /// `write` from `com.superlogical.terminal.write` or `sh.termy.terminal.write`;
    /// nil for a name of another creator.
    public static func terminalMethod(_ name: String) -> String? {
        for prefix in [terminalCreator + ".", termyCreator + "."] where name.hasPrefix(prefix) {
            return String(name.dropFirst(prefix.count))
        }
        return nil
    }
}

// MARK: - Keys

/// A key binding as plugins write it: `shift+cmd+a`, `ctrl+g`, `alt+enter`.
public struct KeyChord: Hashable, Sendable, CustomStringConvertible {
    public struct Modifiers: OptionSet, Hashable, Sendable {
        public let rawValue: UInt8
        public init(rawValue: UInt8) { self.rawValue = rawValue }
        public static let control = Modifiers(rawValue: 1)
        public static let option = Modifiers(rawValue: 2)
        public static let shift = Modifiers(rawValue: 4)
        public static let command = Modifiers(rawValue: 8)
    }

    /// A lowercase key name: a character such as `a` or `/`, or `enter`,
    /// `tab`, `escape`, `space`, `backspace`, `delete`, `up`, `down`, `left`,
    /// `right`, `home`, `end`, `pageup`, `pagedown`, `f1`…`f20`.
    public var key: String
    public var modifiers: Modifiers

    public init(key: String, modifiers: Modifiers) {
        self.key = key
        self.modifiers = modifiers
    }

    private static let modifierNames: [String: Modifiers] = [
        "cmd": .command, "command": .command, "super": .command, "meta": .command, "⌘": .command,
        "shift": .shift, "⇧": .shift,
        "alt": .option, "opt": .option, "option": .option, "⌥": .option,
        "ctrl": .control, "control": .control, "⌃": .control,
    ]

    private static let keyAliases: [String: String] = [
        "return": "enter", "cr": "enter", "esc": "escape", "bs": "backspace",
        "del": "delete", "pgup": "pageup", "pgdn": "pagedown", "page_up": "pageup", "page_down": "pagedown",
        "arrowup": "up", "arrowdown": "down", "arrowleft": "left", "arrowright": "right",
        "plus": "+", "minus": "-", "equal": "=", "comma": ",", "period": ".", "slash": "/",
        "backslash": "\\", "semicolon": ";", "quote": "'", "backquote": "`", "grave": "`",
        "leftbracket": "[", "rightbracket": "]", "bracketleft": "[", "bracketright": "]",
    ]

    public static let namedKeys: Set<String> = Set([
        "enter", "tab", "escape", "space", "backspace", "delete", "up", "down", "left", "right",
        "home", "end", "pageup", "pagedown", "insert",
    ]).union((1...20).map { "f\($0)" })

    /// Parses `spec`; nil when it names no key or an unknown one.
    public init?(_ spec: String) {
        let trimmed = spec.trimmingCharacters(in: .whitespaces)
        guard !trimmed.isEmpty else { return nil }
        // "cmd++" binds plus: a trailing "+" after a separator is the key.
        var parts = trimmed.split(separator: "+", omittingEmptySubsequences: false).map(String.init)
        if trimmed.hasSuffix("++") { parts.removeLast(2); parts.append("+") }
        guard let last = parts.popLast(), !last.isEmpty else { return nil }
        var modifiers: Modifiers = []
        for part in parts {
            guard let modifier = Self.modifierNames[part.lowercased()] else { return nil }
            modifiers.insert(modifier)
        }
        var key = last.count == 1 ? last : last.lowercased()
        if let alias = Self.keyAliases[key.lowercased()] { key = alias }
        // A physical key, such as [KeyD], whatever the keyboard layout types.
        if last.hasPrefix("["), last.hasSuffix("]"), Self.physicalKeys[String(last.dropFirst().dropLast())] != nil {
            self.key = last
            self.modifiers = modifiers
            return
        }
        if key.count == 1 {
            // "A" means shift+a, as Rex reads it.
            if key != key.lowercased() { modifiers.insert(.shift); key = key.lowercased() }
        } else if !Self.namedKeys.contains(key) {
            return nil
        }
        self.key = key
        self.modifiers = modifiers
    }

    /// macOS virtual key codes of the physical keys plugins can name, as
    /// `[KeyA]`, `[Digit1]`, `[BracketLeft]` (the W3C `code` names).
    public static let physicalKeys: [String: UInt16] = {
        var keys: [String: UInt16] = [
            "KeyA": 0, "KeyS": 1, "KeyD": 2, "KeyF": 3, "KeyH": 4, "KeyG": 5, "KeyZ": 6, "KeyX": 7, "KeyC": 8,
            "KeyV": 9, "KeyB": 11, "KeyQ": 12, "KeyW": 13, "KeyE": 14, "KeyR": 15, "KeyY": 16, "KeyT": 17,
            "Digit1": 18, "Digit2": 19, "Digit3": 20, "Digit4": 21, "Digit6": 22, "Digit5": 23, "Equal": 24,
            "Digit9": 25, "Digit7": 26, "Minus": 27, "Digit8": 28, "Digit0": 29, "BracketRight": 30, "KeyO": 31,
            "KeyU": 32, "BracketLeft": 33, "KeyI": 34, "KeyP": 35, "Enter": 36, "KeyL": 37, "KeyJ": 38, "Quote": 39,
            "KeyK": 40, "Semicolon": 41, "Backslash": 42, "Comma": 43, "Slash": 44, "KeyN": 45, "KeyM": 46,
            "Period": 47, "Tab": 48, "Space": 49, "Backquote": 50, "Backspace": 51, "Escape": 53,
            "ArrowLeft": 123, "ArrowRight": 124, "ArrowDown": 125, "ArrowUp": 126,
        ]
        keys["IntlBackslash"] = 10
        return keys
    }()

    /// The spec in a stable order: `ctrl+alt+shift+cmd+key`.
    public var description: String {
        var parts: [String] = []
        if modifiers.contains(.control) { parts.append("ctrl") }
        if modifiers.contains(.option) { parts.append("alt") }
        if modifiers.contains(.shift) { parts.append("shift") }
        if modifiers.contains(.command) { parts.append("cmd") }
        return (parts + [key]).joined(separator: "+")
    }

    /// As macOS menus draw it, such as ⇧⌘A.
    public var symbolLabel: String {
        var s = ""
        if modifiers.contains(.control) { s += "⌃" }
        if modifiers.contains(.option) { s += "⌥" }
        if modifiers.contains(.shift) { s += "⇧" }
        if modifiers.contains(.command) { s += "⌘" }
        let names: [String: String] = [
            "enter": "↩", "tab": "⇥", "escape": "⎋", "space": "Space", "backspace": "⌫", "delete": "⌦",
            "up": "↑", "down": "↓", "left": "←", "right": "→", "home": "↖", "end": "↘",
            "pageup": "⇞", "pagedown": "⇟",
        ]
        return s + (names[key] ?? key.uppercased())
    }
}

// MARK: - Bindings

/// What `rex.bind` takes as a key: a chord, a sequence written
/// `ctrl+b>c`, either one in a mode written `resize/left`.
public struct KeySpec: Hashable, Sendable, CustomStringConvertible {
    public var mode: String?
    public var chords: [KeyChord]

    public init(mode: String? = nil, chords: [KeyChord]) {
        self.mode = mode
        self.chords = chords
    }

    public init?(_ spec: String) {
        var text = Substring(spec.trimmingCharacters(in: .whitespaces))
        var mode: String?
        // "resize/left": a mode name has only lowercase letters, digits, _ and -.
        if let slash = text.firstIndex(of: "/"), slash != text.startIndex,
           text[..<slash].allSatisfy({ $0.isLowercase || $0.isNumber || $0 == "_" || $0 == "-" }) {
            mode = String(text[..<slash])
            text = text[text.index(after: slash)...]
        }
        let parts = text.split(separator: ">", omittingEmptySubsequences: false).map(String.init)
        var chords: [KeyChord] = []
        for part in parts {
            guard let chord = KeyChord(part) else { return nil }
            chords.append(chord)
        }
        guard !chords.isEmpty, mode != "root" else { return nil }
        self.mode = mode
        self.chords = chords
    }

    public static func isValidMode(_ name: String) -> Bool {
        !name.isEmpty && name != "root" && name.allSatisfy { $0.isLowercase || $0.isNumber || $0 == "_" || $0 == "-" }
    }

    /// The keys alone, `ctrl+b>c`, without the mode.
    public var keys: String { chords.map(\.description).joined(separator: ">") }

    public var description: String { (mode.map { $0 + "/" } ?? "") + keys }
}

// MARK: - Layouts

/// What a `block` node in a layout starts.
public struct PluginBlockSpec: Equatable, Sendable {
    public var flavor: String
    public var label: String?
    /// The program and its arguments; nil runs the login shell.
    public var command: [String]?
    public var cwd: String?
    public var environment: [String: String]
    /// Keeps the pane after its program exits.
    public var keepOpen: Bool
    /// Runs `command` in the login shell's environment (Rex's default), or
    /// directly with `shell = "none"`.
    public var loginShell: Bool

    public init(flavor: String = PluginFlavor.shell, label: String? = nil, command: [String]? = nil,
                cwd: String? = nil, environment: [String: String] = [:], keepOpen: Bool = false, loginShell: Bool = true) {
        self.flavor = flavor
        self.label = label
        self.command = command
        self.cwd = cwd
        self.environment = environment
        self.keepOpen = keepOpen
        self.loginShell = loginShell
    }
}

public struct PluginLayoutError: Error, Equatable, CustomStringConvertible {
    public var description: String
    public init(_ description: String) { self.description = description }
}

/// A layout as Rex's session methods take it: `{ block = {…} }` or
/// `{ split = { direction, ratio, before, after } }`.
public indirect enum PluginLayout: Equatable, Sendable {
    case block(PluginBlockSpec)
    /// `horizontal` puts `before` left of `after`; otherwise above it.
    case split(horizontal: Bool, ratio: Double, before: PluginLayout, after: PluginLayout)

    public init(json: Any?) throws {
        guard let node = json as? [String: Any] else { throw PluginLayoutError("layout must be an object with block or split") }
        if let block = node["block"] {
            guard let block = block as? [String: Any] else { throw PluginLayoutError("layout.block must be an object") }
            let flavor = block["flavor"] as? String ?? PluginFlavor.shell
            guard PluginFlavor.isShell(flavor) else { throw PluginLayoutError("no creator provides flavor \(flavor)") }
            let options = block["options"] as? [String: Any] ?? [:]
            var command: [String]?
            if let argv = options["command"] as? [Any] {
                command = try argv.map { part in
                    if let s = part as? String { return s }
                    if let n = part as? NSNumber { return n.stringValue }
                    throw PluginLayoutError("options.command must be a list of strings")
                }
                if command?.isEmpty == true { command = nil }
            } else if let line = options["command"] as? String, !line.isEmpty {
                command = ["/bin/sh", "-c", line]
            }
            var environment: [String: String] = [:]
            for (key, value) in options["env"] as? [String: Any] ?? [:] {
                environment[key] = (value as? String) ?? (value as? NSNumber)?.stringValue
            }
            let keepOpen = (options["keep_open"] as? Bool) ?? (options["keepOpen"] as? Bool) ?? false
            self = .block(PluginBlockSpec(flavor: flavor, label: block["label"] as? String, command: command,
                                          cwd: (options["cwd"] as? String) ?? (block["cwd"] as? String),
                                          environment: environment, keepOpen: keepOpen,
                                          loginShell: (options["shell"] as? String) != "none"))
        } else if let split = node["split"] as? [String: Any] {
            let direction = split["direction"] as? String ?? "horizontal"
            guard direction == "horizontal" || direction == "vertical" else {
                throw PluginLayoutError("split.direction must be horizontal or vertical")
            }
            let ratio = (split["ratio"] as? NSNumber)?.doubleValue ?? 0.5
            guard ratio > 0, ratio < 1 else { throw PluginLayoutError("split.ratio must be between 0 and 1") }
            self = .split(horizontal: direction == "horizontal", ratio: ratio,
                          before: try PluginLayout(json: split["before"]), after: try PluginLayout(json: split["after"]))
        } else {
            throw PluginLayoutError("a layout node holds block or split")
        }
    }

    /// The blocks in layout order: each split's before side first.
    public var blocks: [PluginBlockSpec] {
        switch self {
        case .block(let spec): [spec]
        case .split(_, _, let before, let after): before.blocks + after.blocks
        }
    }

    /// Each block's rectangle when the layout fills `rect`, in layout order.
    public func rects(in rect: CGRect) -> [CGRect] {
        switch self {
        case .block: return [rect]
        case .split(let horizontal, let ratio, let before, let after):
            let (a, b): (CGRect, CGRect) = horizontal
                ? (CGRect(x: rect.minX, y: rect.minY, width: rect.width * ratio, height: rect.height),
                   CGRect(x: rect.minX + rect.width * ratio, y: rect.minY, width: rect.width * (1 - ratio), height: rect.height))
                : (CGRect(x: rect.minX, y: rect.minY, width: rect.width, height: rect.height * ratio),
                   CGRect(x: rect.minX, y: rect.minY + rect.height * ratio, width: rect.width, height: rect.height * (1 - ratio)))
            return before.rects(in: a) + after.rects(in: b)
        }
    }

    /// The same arrangement as a split tree over `ids`, given in layout order.
    public func tree<ID: Hashable & Sendable>(ids: [ID]) -> SplitTree<ID> {
        var iterator = ids.makeIterator()
        func build(_ node: PluginLayout) -> SplitTree<ID> {
            switch node {
            case .block: return .leaf(iterator.next()!)
            case .split(let horizontal, let ratio, let before, let after):
                let first = build(before)
                return .split(axis: horizontal ? .horizontal : .vertical, ratio: ratio, first: first, second: build(after))
            }
        }
        return build(self)
    }
}

// MARK: - Shell quoting

public enum ShellQuote {
    /// `word` quoted for sh, left bare when nothing in it needs quoting.
    public static func quote(_ word: String) -> String {
        let safe = CharacterSet(charactersIn: "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789@%_+=:,./-")
        if !word.isEmpty, word.unicodeScalars.allSatisfy({ safe.contains($0) }) { return word }
        return "'" + word.replacingOccurrences(of: "'", with: "'\\''") + "'"
    }

    /// A line for `/bin/sh -c` that runs `argv` in the login shell's
    /// environment, as Rex runs a block's command, and replaces itself with it.
    public static func loginCommand(_ argv: [String], shell: String) -> String {
        let inner = "exec " + argv.map(quote).joined(separator: " ")
        return "exec \(quote(shell)) -l -c \(quote(inner))"
    }
}

// MARK: - Split IDs

extension SplitTree where ID == UUID {
    /// A split's ID is the first pane of its `after` side, which no other
    /// split starts with.
    public func splitKeys() -> [UUID] {
        guard case .split(_, _, let first, let second) = self else { return [] }
        return [second.leaves[0]] + first.splitKeys() + second.splitKeys()
    }

    /// Replaces the split whose `after` side starts with `key` by `transform` of it.
    public func replacingSplit(_ key: UUID, with transform: (SplitTree) -> SplitTree) -> SplitTree? {
        guard case .split(let axis, let ratio, let first, let second) = self else { return nil }
        if second.leaves.first == key { return transform(self) }
        if let replaced = first.replacingSplit(key, with: transform) {
            return .split(axis: axis, ratio: ratio, first: replaced, second: second)
        }
        if let replaced = second.replacingSplit(key, with: transform) {
            return .split(axis: axis, ratio: ratio, first: first, second: replaced)
        }
        return nil
    }

    /// Each node's rectangle in `rect` with no gaps, for Rex's `session.view`.
    /// Unlike `frames(in:gap:)` it doesn't round to whole points.
    public func unitRects(in rect: CGRect = CGRect(x: 0, y: 0, width: 1, height: 1)) -> [UUID: CGRect] {
        switch self {
        case .leaf(let id):
            return [id: rect]
        case .split(let axis, let ratio, let first, let second):
            let (a, b) = Self.unitDivide(rect, axis: axis, ratio: ratio)
            return first.unitRects(in: a).merging(second.unitRects(in: b)) { x, _ in x }
        }
    }

    public static func unitDivide(_ rect: CGRect, axis: SplitAxis, ratio: CGFloat) -> (CGRect, CGRect) {
        switch axis {
        case .horizontal:
            let w = rect.width * ratio
            return (CGRect(x: rect.minX, y: rect.minY, width: w, height: rect.height),
                    CGRect(x: rect.minX + w, y: rect.minY, width: rect.width - w, height: rect.height))
        case .vertical:
            let h = rect.height * ratio
            return (CGRect(x: rect.minX, y: rect.minY, width: rect.width, height: h),
                    CGRect(x: rect.minX, y: rect.minY + h, width: rect.width, height: rect.height - h))
        }
    }
}

extension SplitTree where ID == UUID {
    /// Sets the ratio of the split whose `before` side holds `before` and
    /// whose `after` side holds `after`, along `axis`. Nil when no split does.
    public func settingRatio(_ ratio: CGFloat, between before: UUID, and after: UUID, axis: SplitAxis) -> SplitTree? {
        guard case .split(let a, let r, let first, let second) = self else { return nil }
        if a == axis, first.contains(before), second.contains(after) {
            return .split(axis: a, ratio: ratio, first: first, second: second)
        }
        if let changed = first.settingRatio(ratio, between: before, and: after, axis: axis) {
            return .split(axis: a, ratio: r, first: changed, second: second)
        }
        if let changed = second.settingRatio(ratio, between: before, and: after, axis: axis) {
            return .split(axis: a, ratio: r, first: first, second: changed)
        }
        return nil
    }
}

extension SplitTree where ID == UUID {
    /// Gives the side holding `id` of its nearest split along `axis` a
    /// `share` of that split. Nil when no split along `axis` holds it.
    public func resizing(_ id: UUID, axis: SplitAxis, share: CGFloat) -> SplitTree? {
        guard case .split(let a, let r, let first, let second) = self else { return nil }
        if first.contains(id), let inner = first.resizing(id, axis: axis, share: share) {
            return .split(axis: a, ratio: r, first: inner, second: second)
        }
        if second.contains(id), let inner = second.resizing(id, axis: axis, share: share) {
            return .split(axis: a, ratio: r, first: first, second: inner)
        }
        guard a == axis, contains(id) else { return nil }
        return .split(axis: a, ratio: first.contains(id) ? share : 1 - share, first: first, second: second)
    }
}
