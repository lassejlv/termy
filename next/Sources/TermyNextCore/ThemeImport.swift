import Foundation

/// A terminal another theme can be imported from.
public enum ThemeImportSource: String, CaseIterable, Sendable {
    case ghostty, alacritty, kitty, rex

    public var title: String {
        switch self {
        case .ghostty: "Ghostty"
        case .alacritty: "Alacritty"
        case .kitty: "kitty"
        case .rex: "Rex"
        }
    }
}

/// Colors read from another terminal's config, as `#rrggbb`.
public struct ImportedTheme: Equatable, Sendable {
    public var name: String
    public var source: ThemeImportSource
    public var foreground: String?
    public var background: String?
    public var cursor: String?
    /// ANSI colors 0–15.
    public var palette: [Int: String] = [:]
    /// The theme the config names but whose file isn't on this Mac, such as
    /// one of Ghostty's built-in themes: Termy may have it under that name.
    public var namedTheme: String?

    public init(name: String, source: ThemeImportSource) {
        self.name = name
        self.source = source
    }

    public var hasColors: Bool { foreground != nil || background != nil || !palette.isEmpty }

    /// Colors set here win over `base`'s.
    func overlaying(_ base: ImportedTheme) -> ImportedTheme {
        var merged = base
        merged.foreground = foreground ?? base.foreground
        merged.background = background ?? base.background
        merged.cursor = cursor ?? base.cursor
        merged.palette.merge(palette) { _, new in new }
        return merged
    }

    /// The ANSI color names Termy's theme files use, in palette order.
    public static let paletteKeys = [
        "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
        "bright_black", "bright_red", "bright_green", "bright_yellow",
        "bright_blue", "bright_magenta", "bright_cyan", "bright_white",
    ]

    /// Ghostty's defaults, for colors a config leaves out.
    private static let defaultPalette = [
        "#1d1f21", "#cc6666", "#b5bd68", "#f0c674", "#81a2be", "#b294bb", "#8abeb7", "#c5c8c6",
        "#666666", "#d54e53", "#b9ca4a", "#e7c547", "#7aa6da", "#c397d8", "#70c0b1", "#eaeaea",
    ]

    /// A Termy theme file: every color it needs, missing ones filled in
    /// (a missing bright color takes its normal one).
    public func termyColors() -> [String: String] {
        let foreground = foreground ?? "#ffffff"
        var colors = ["foreground": foreground, "background": background ?? "#282c34", "cursor": cursor ?? foreground]
        for (index, key) in Self.paletteKeys.enumerated() {
            colors[key] = palette[index] ?? (index >= 8 ? palette[index - 8] : nil) ?? Self.defaultPalette[index]
        }
        return colors
    }

    public func termyJSON() -> Data {
        (try? JSONSerialization.data(withJSONObject: termyColors(), options: [.prettyPrinted, .sortedKeys])) ?? Data()
    }
}

public enum ThemeImportError: Error, LocalizedError, Equatable {
    case notFound(ThemeImportSource, searched: [String])
    case noColors(String)
    case unknownFormat(String)

    public var errorDescription: String? {
        switch self {
        case .notFound(let source, let searched):
            "No \(source.title) config found. Looked in:\n" + searched.map { ($0 as NSString).abbreviatingWithTildeInPath }.joined(separator: "\n")
        case .noColors(let path):
            "\((path as NSString).abbreviatingWithTildeInPath) doesn't set any colors."
        case .unknownFormat(let path):
            "\((path as NSString).lastPathComponent) isn't a Ghostty, Alacritty, kitty or Termy theme."
        }
    }
}

/// Reads the theme a Ghostty, Alacritty, kitty or Rex config uses.
public struct ThemeImporter {
    public var home: URL
    public var environment: [String: String]
    /// Which half of a `light:…,dark:…` theme pair to take.
    public var prefersDark: Bool
    /// Folders with Ghostty's own themes, such as the app bundle's.
    public var ghosttyThemeDirectories: [URL]

    public init(home: URL = FileManager.default.homeDirectoryForCurrentUser,
                environment: [String: String] = ProcessInfo.processInfo.environment,
                prefersDark: Bool = true, ghosttyThemeDirectories: [URL] = []) {
        self.home = home
        self.environment = environment
        self.prefersDark = prefersDark
        self.ghosttyThemeDirectories = ghosttyThemeDirectories
    }

    private var configHome: URL {
        environment["XDG_CONFIG_HOME"].map { URL(fileURLWithPath: $0, isDirectory: true) }
            ?? home.appendingPathComponent(".config", isDirectory: true)
    }

    private func exists(_ url: URL) -> Bool {
        var isDirectory: ObjCBool = false
        return FileManager.default.fileExists(atPath: url.path, isDirectory: &isDirectory) && !isDirectory.boolValue
    }

    private func read(_ url: URL) -> String? { try? String(contentsOf: url, encoding: .utf8) }

    /// `path` from a config in `directory`: `~` expanded, relative paths
    /// against the config's folder.
    private func resolve(_ path: String, in directory: URL) -> URL {
        var path = path
        if path == "~" || path.hasPrefix("~/") { path = home.path + path.dropFirst() }
        return path.hasPrefix("/") ? URL(fileURLWithPath: path) : directory.appendingPathComponent(path)
    }

    // MARK: Where each config lives

    /// Config files in the order the terminal reads them; empty when none exists.
    public func configFiles(for source: ThemeImportSource) -> [URL] {
        let found = candidates(for: source).filter(exists)
        // Ghostty reads every file; Alacritty and kitty only the first.
        return source == .ghostty ? found : Array(found.prefix(1))
    }

    public func candidates(for source: ThemeImportSource) -> [URL] {
        let support = home.appendingPathComponent("Library/Application Support", isDirectory: true)
        switch source {
        case .ghostty:
            // Ghostty reads all of these, later ones over earlier ones.
            let xdg = configHome.appendingPathComponent("ghostty")
            let mac = support.appendingPathComponent("com.mitchellh.ghostty")
            return [xdg.appendingPathComponent("config"), xdg.appendingPathComponent("config.ghostty"),
                    mac.appendingPathComponent("config"), mac.appendingPathComponent("config.ghostty")]
        case .alacritty:
            let urls = [configHome.appendingPathComponent("alacritty/alacritty.toml"),
                        configHome.appendingPathComponent("alacritty.toml"),
                        home.appendingPathComponent(".config/alacritty/alacritty.toml"),
                        home.appendingPathComponent(".alacritty.toml")]
            return urls.reduce(into: []) { if !$0.contains($1) { $0.append($1) } }
        case .kitty:
            var urls: [URL] = []
            if let directory = environment["KITTY_CONFIG_DIRECTORY"] {
                urls.append(URL(fileURLWithPath: directory).appendingPathComponent("kitty.conf"))
            }
            urls += [configHome.appendingPathComponent("kitty/kitty.conf"),
                     home.appendingPathComponent(".config/kitty/kitty.conf"),
                     home.appendingPathComponent("Library/Preferences/kitty/kitty.conf")]
            return urls.reduce(into: []) { if !$0.contains($1) { $0.append($1) } }
        case .rex:
            return []
        }
    }

    // MARK: Ghostty

    /// The theme Ghostty shows: its `theme`'s colors with the config's own
    /// colors over them.
    public func ghostty() throws -> ImportedTheme {
        let files = configFiles(for: .ghostty)
        guard !files.isEmpty else { throw ThemeImportError.notFound(.ghostty, searched: candidates(for: .ghostty).map(\.path)) }
        var own = ImportedTheme(name: "Ghostty", source: .ghostty)
        var themeName: String?
        var visited = Set<URL>()
        func load(_ url: URL, depth: Int) {
            guard depth < 8, visited.insert(url.standardizedFileURL).inserted, let text = read(url) else { return }
            for (key, value) in Self.keyValues(text) {
                switch key {
                case "theme": themeName = value
                case "config-file":
                    let optional = value.hasPrefix("?")
                    let path = optional ? String(value.dropFirst()) : value
                    load(resolve(Self.unquoted(path), in: url.deletingLastPathComponent()), depth: depth + 1)
                default: Self.applyGhostty(key, value, to: &own)
                }
            }
        }
        files.forEach { load($0, depth: 0) }

        guard let themeName = themeName.map(pick) else {
            guard own.hasColors else { throw ThemeImportError.noColors(files[0].path) }
            return own
        }
        var theme = ImportedTheme(name: themeName, source: .ghostty)
        if let file = ghosttyTheme(named: themeName, near: files), let text = read(file) {
            theme = Self.ghostty(text, name: (themeName as NSString).lastPathComponent)
        } else {
            theme.name = (themeName as NSString).lastPathComponent
            theme.namedTheme = theme.name
        }
        return own.overlaying(theme)
    }

    /// Reads a Ghostty theme file or config's colors.
    public static func ghostty(_ text: String, name: String) -> ImportedTheme {
        var theme = ImportedTheme(name: name, source: .ghostty)
        for (key, value) in keyValues(text) { applyGhostty(key, value, to: &theme) }
        return theme
    }

    private static func applyGhostty(_ key: String, _ value: String, to theme: inout ImportedTheme) {
        switch key {
        case "background": theme.background = hex(value) ?? theme.background
        case "foreground": theme.foreground = hex(value) ?? theme.foreground
        case "cursor-color": theme.cursor = hex(value) ?? theme.cursor
        case "palette":
            let parts = value.split(separator: "=", maxSplits: 1).map { $0.trimmingCharacters(in: .whitespaces) }
            guard parts.count == 2, let index = Int(parts[0]), (0..<16).contains(index), let color = hex(parts[1]) else { return }
            theme.palette[index] = color
        default: break
        }
    }

    /// `light:A,dark:B` → the one for the current appearance.
    private func pick(_ value: String) -> String {
        let pairs = value.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }
        var light: String?, dark: String?
        for pair in pairs {
            if pair.hasPrefix("light:") { light = String(pair.dropFirst(6)) }
            if pair.hasPrefix("dark:") { dark = String(pair.dropFirst(5)) }
        }
        guard light != nil || dark != nil else { return Self.unquoted(value) }
        return Self.unquoted((prefersDark ? dark ?? light : light ?? dark)!)
    }

    private func ghosttyTheme(named name: String, near files: [URL]) -> URL? {
        if name.hasPrefix("/") || name.hasPrefix("~") {
            let url = resolve(name, in: home)
            return exists(url) ? url : nil
        }
        let directories = files.map { $0.deletingLastPathComponent().appendingPathComponent("themes") }
            + [configHome.appendingPathComponent("ghostty/themes")]
            + ghosttyThemeDirectories
            + [URL(fileURLWithPath: "/Applications/Ghostty.app/Contents/Resources/ghostty/themes")]
        return directories.lazy.map { $0.appendingPathComponent(name) }.first(where: exists)
    }

    /// `key = value` lines, comments and blanks skipped.
    static func keyValues(_ text: String) -> [(String, String)] {
        text.split(whereSeparator: \.isNewline).compactMap { line in
            let line = line.trimmingCharacters(in: .whitespaces)
            guard !line.isEmpty, !line.hasPrefix("#"), let equals = line.firstIndex(of: "=") else { return nil }
            let key = line[..<equals].trimmingCharacters(in: .whitespaces)
            let value = line[line.index(after: equals)...].trimmingCharacters(in: .whitespaces)
            return (key, unquoted(value))
        }
    }

    // MARK: Alacritty

    public func alacritty() throws -> ImportedTheme {
        guard let file = configFiles(for: .alacritty).first else {
            let legacy = [configHome.appendingPathComponent("alacritty/alacritty.yml"), home.appendingPathComponent(".alacritty.yml")]
            if let yaml = legacy.first(where: exists) {
                throw ThemeImportError.unknownFormat(yaml.path + " (run `alacritty migrate` to convert it to TOML)")
            }
            throw ThemeImportError.notFound(.alacritty, searched: candidates(for: .alacritty).map(\.path))
        }
        let theme = alacritty(file: file, depth: 0)
        guard theme.hasColors else { throw ThemeImportError.noColors(file.path) }
        return theme
    }

    /// A file's imports first, then its own colors over them.
    private func alacritty(file: URL, depth: Int) -> ImportedTheme {
        let entries = Self.toml(read(file) ?? "")
        var base = ImportedTheme(name: depth == 0 ? "Alacritty" : Self.displayName(file), source: .alacritty)
        for (key, value) in entries where depth < 8 && (key == "import" || key == "general.import") {
            for path in Self.tomlStrings(value) {
                let url = resolve(path, in: file.deletingLastPathComponent())
                guard exists(url) else { continue }
                let imported = alacritty(file: url, depth: depth + 1)
                guard imported.hasColors else { continue }
                base = imported.overlaying(base)
                base.name = imported.name
            }
        }
        return Self.alacritty(entries: entries, name: base.name).overlaying(base)
    }

    public static func alacritty(_ text: String, name: String) -> ImportedTheme {
        alacritty(entries: toml(text), name: name)
    }

    private static func alacritty(entries: [(String, String)], name: String) -> ImportedTheme {
        var theme = ImportedTheme(name: name, source: .alacritty)
        for (key, value) in entries {
            guard let color = hex(unquoted(value)) else { continue }
            switch key {
            case "colors.primary.background": theme.background = color
            case "colors.primary.foreground": theme.foreground = color
            case "colors.cursor.cursor": theme.cursor = color
            default:
                for (prefix, offset) in [("colors.normal.", 0), ("colors.bright.", 8)] where key.hasPrefix(prefix) {
                    if let index = paletteNames.firstIndex(of: String(key.dropFirst(prefix.count))) {
                        theme.palette[index + offset] = color
                    }
                }
            }
        }
        return theme
    }

    private static let paletteNames = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"]

    /// Enough TOML for a terminal config: `[tables]`, dotted and quoted keys,
    /// strings, and arrays over several lines. Keys come back fully dotted.
    static func toml(_ text: String) -> [(String, String)] {
        var entries: [(String, String)] = []
        var table = ""
        var pending: (key: String, value: String)?
        for rawLine in text.split(whereSeparator: \.isNewline) {
            let line = stripComment(String(rawLine)).trimmingCharacters(in: .whitespaces)
            if var open = pending {
                open.value += " " + line
                if line.contains("]") { entries.append((open.key, open.value)); pending = nil } else { pending = open }
                continue
            }
            guard !line.isEmpty else { continue }
            if line.hasPrefix("[") {
                guard !line.hasPrefix("[[") else { table = ""; continue }
                table = tomlKey(String(line.dropFirst().prefix { $0 != "]" }))
                continue
            }
            guard let equals = line.firstIndex(of: "=") else { continue }
            let key = tomlKey(String(line[..<equals]))
            let fullKey = table.isEmpty ? key : table + "." + key
            let value = line[line.index(after: equals)...].trimmingCharacters(in: .whitespaces)
            if value.hasPrefix("{") {
                // An inline table: `primary = { background = "#…" }`.
                let inner = value.dropFirst().prefix { $0 != "}" }
                for part in inner.split(separator: ",") {
                    guard let eq = part.firstIndex(of: "=") else { continue }
                    entries.append((fullKey + "." + tomlKey(String(part[..<eq])),
                                    part[part.index(after: eq)...].trimmingCharacters(in: .whitespaces)))
                }
            } else if value.hasPrefix("["), !value.contains("]") {
                pending = (fullKey, value)
            } else {
                entries.append((fullKey, value))
            }
        }
        return entries
    }

    private static func tomlKey(_ key: String) -> String {
        key.split(separator: ".").map { unquoted($0.trimmingCharacters(in: .whitespaces)) }.joined(separator: ".")
    }

    /// The strings in a TOML array or string.
    static func tomlStrings(_ value: String) -> [String] {
        guard let regex = try? NSRegularExpression(pattern: #""([^"]*)"|'([^']*)'"#) else { return [] }
        return regex.matches(in: value, range: NSRange(value.startIndex..., in: value)).compactMap { match in
            [1, 2].lazy.compactMap { Range(match.range(at: $0), in: value) }.first.map { String(value[$0]) }
        }
    }

    /// The line up to a `#` that isn't inside quotes.
    private static func stripComment(_ line: String) -> String {
        var quote: Character?
        for index in line.indices {
            let char = line[index]
            if let open = quote {
                if char == open { quote = nil }
            } else if char == "\"" || char == "'" {
                quote = char
            } else if char == "#" {
                return String(line[..<index])
            }
        }
        return line
    }

    // MARK: kitty

    public func kitty() throws -> ImportedTheme {
        guard let file = configFiles(for: .kitty).first else {
            throw ThemeImportError.notFound(.kitty, searched: candidates(for: .kitty).map(\.path))
        }
        var theme = ImportedTheme(name: "kitty", source: .kitty)
        var visited = Set<URL>()
        func load(_ url: URL, depth: Int) {
            guard depth < 8, visited.insert(url.standardizedFileURL).inserted, let text = read(url) else { return }
            var afterThemeMarker = false
            for rawLine in text.split(whereSeparator: \.isNewline) {
                let line = rawLine.trimmingCharacters(in: .whitespaces)
                // `kitten themes` writes `# BEGIN_KITTY_THEME`, then `# <name>`;
                // theme files carry `## name: <name>`.
                if line.hasPrefix("## name:") {
                    theme.name = line.dropFirst(8).trimmingCharacters(in: .whitespaces)
                    continue
                }
                if line == "# BEGIN_KITTY_THEME" { afterThemeMarker = true; continue }
                if afterThemeMarker, line.hasPrefix("#") {
                    let name = line.drop { $0 == "#" }.trimmingCharacters(in: .whitespaces)
                    if !name.isEmpty { theme.name = name }
                    afterThemeMarker = false
                    continue
                }
                afterThemeMarker = false
                guard !line.isEmpty, !line.hasPrefix("#") else { continue }
                let parts = line.split(maxSplits: 1, whereSeparator: \.isWhitespace)
                guard parts.count == 2 else { continue }
                let key = String(parts[0]), value = parts[1].trimmingCharacters(in: .whitespaces)
                if key == "include" || key == "globinclude" {
                    let directory = url.deletingLastPathComponent()
                    if key == "include" {
                        load(resolve(value, in: directory), depth: depth + 1)
                    } else {
                        let pattern = resolve(value, in: directory)
                        let folder = pattern.deletingLastPathComponent()
                        let names = (try? FileManager.default.contentsOfDirectory(atPath: folder.path)) ?? []
                        for name in names.sorted() where fnmatch(pattern.lastPathComponent, name, 0) == 0 {
                            load(folder.appendingPathComponent(name), depth: depth + 1)
                        }
                    }
                    continue
                }
                Self.applyKitty(key, value, to: &theme)
            }
        }
        load(file, depth: 0)
        guard theme.hasColors else { throw ThemeImportError.noColors(file.path) }
        return theme
    }

    public static func kitty(_ text: String, name: String) -> ImportedTheme {
        var theme = ImportedTheme(name: name, source: .kitty)
        for rawLine in text.split(whereSeparator: \.isNewline) {
            let line = rawLine.trimmingCharacters(in: .whitespaces)
            if line.hasPrefix("## name:") { theme.name = line.dropFirst(8).trimmingCharacters(in: .whitespaces) }
            guard !line.hasPrefix("#") else { continue }
            let parts = line.split(maxSplits: 1, whereSeparator: \.isWhitespace)
            guard parts.count == 2 else { continue }
            applyKitty(String(parts[0]), parts[1].trimmingCharacters(in: .whitespaces), to: &theme)
        }
        return theme
    }

    private static func applyKitty(_ key: String, _ value: String, to theme: inout ImportedTheme) {
        guard let color = hex(value) else { return }
        switch key {
        case "foreground": theme.foreground = color
        case "background": theme.background = color
        case "cursor": theme.cursor = color
        default:
            if key.hasPrefix("color"), let index = Int(key.dropFirst(5)), (0..<16).contains(index) {
                theme.palette[index] = color
            }
        }
    }

    // MARK: Rex

    /// Rex's custom themes, from its `Workspace.customThemes` default.
    public static func rex(_ json: Data) -> [ImportedTheme] {
        guard let list = try? JSONSerialization.jsonObject(with: json) as? [[String: Any]] else { return [] }
        func color(_ value: Any?) -> String? {
            guard let value = value as? [String: Any], let r = value["red"] as? Double,
                  let g = value["green"] as? Double, let b = value["blue"] as? Double else { return nil }
            let byte = { (c: Double) in Int((max(0, min(1, c)) * 255).rounded()) }
            return String(format: "#%02x%02x%02x", byte(r), byte(g), byte(b))
        }
        return list.compactMap { entry in
            guard let palette = entry["palette"] as? [String: Any] else { return nil }
            var theme = ImportedTheme(name: entry["name"] as? String ?? "Rex", source: .rex)
            theme.foreground = color(palette["foreground"])
            theme.background = color(palette["background"])
            theme.cursor = color(palette["cursor"])
            for (index, value) in ((palette["ansi"] as? [Any]) ?? []).prefix(16).enumerated() {
                theme.palette[index] = color(value)
            }
            return theme.hasColors ? theme : nil
        }
    }

    /// The ids of Rex's custom themes, in the same order as `rex(_:)`.
    public static func rexIDs(_ json: Data) -> [String] {
        guard let list = try? JSONSerialization.jsonObject(with: json) as? [[String: Any]] else { return [] }
        return list.compactMap { $0["palette"] is [String: Any] ? ($0["id"] as? String ?? "") : nil }
    }

    // MARK: A theme file

    /// Reads a theme file from any of the terminals, telling them apart by
    /// their contents. Termy's own JSON comes back as `nil`: copy it as is.
    public func file(_ url: URL) throws -> ImportedTheme? {
        guard let text = read(url) else { throw ThemeImportError.unknownFormat(url.path) }
        let name = Self.displayName(url)
        let ext = url.pathExtension.lowercased()
        if ext == "json" {
            if let data = text.data(using: .utf8), let object = try? JSONSerialization.jsonObject(with: data) {
                if let colors = object as? [String: Any], colors["foreground"] is String, colors["bright_white"] is String { return nil }
                if object is [Any], let theme = Self.rex(data).first { return theme }
            }
            throw ThemeImportError.unknownFormat(url.path)
        }
        let theme: ImportedTheme
        if ext == "toml" || text.contains("[colors") {
            theme = Self.alacritty(text, name: name)
        } else if Self.keyValues(text).contains(where: { ["palette", "background", "foreground", "cursor-color"].contains($0.0) }) {
            theme = Self.ghostty(text, name: name)
        } else {
            theme = Self.kitty(text, name: name)
        }
        guard theme.hasColors else { throw ThemeImportError.noColors(url.path) }
        return theme
    }

    // MARK: Values

    /// `#rrggbb`, `rrggbb`, `0xrrggbb` or `#rgb` as `#rrggbb`; nil for
    /// anything else, such as a color name.
    public static func hex(_ value: String) -> String? {
        var digits = Substring(unquoted(value).lowercased())
        if digits.hasPrefix("#") { digits = digits.dropFirst() } else if digits.hasPrefix("0x") { digits = digits.dropFirst(2) }
        guard digits.allSatisfy(\.isHexDigit) else { return nil }
        if digits.count == 3 { return "#" + digits.map { "\($0)\($0)" }.joined() }
        return digits.count == 6 ? "#" + digits : nil
    }

    static func unquoted(_ value: String) -> String {
        let value = value.trimmingCharacters(in: .whitespaces)
        guard value.count >= 2, let first = value.first, first == value.last, first == "\"" || first == "'" else { return value }
        return String(value.dropFirst().dropLast())
    }

    /// The theme id Termy gives a file named `name`, as libtermy's
    /// `normalize_theme_id` does: `Tokyo_Night` → `tokyo-night`.
    public static func themeID(_ name: String) -> String {
        var id = ""
        var lastDash = false
        for char in name.trimmingCharacters(in: .whitespaces).lowercased() {
            switch char {
            case "a"..."z", "0"..."9":
                id.append(char)
                lastDash = false
            case "-", "_", " ":
                guard !id.isEmpty, !lastDash else { continue }
                id.append("-")
                lastDash = true
            case "+":
                if !id.isEmpty && !lastDash { id.append("-") }
                id += "plus"
                lastDash = false
            default: break
            }
        }
        while id.hasSuffix("-") { id.removeLast() }
        return id
    }

    /// `tokyo_night.toml` → `Tokyo Night`.
    static func displayName(_ url: URL) -> String {
        url.deletingPathExtension().lastPathComponent
            .split(whereSeparator: { $0 == "_" || $0 == "-" || $0 == " " })
            .map { $0.prefix(1).uppercased() + $0.dropFirst() }
            .joined(separator: " ")
    }
}
