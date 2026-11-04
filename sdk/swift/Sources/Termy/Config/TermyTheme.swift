import Foundation

/// A theme a config can name with `theme = <id>`: one of Termy's built-in
/// themes, the bundled collection, or a local file in the `themes` folder
/// next to the config.
public struct TermyTheme: Hashable, Sendable, Identifiable {
    /// The id config files use, such as `catppuccin-mocha`.
    public var id: String
    /// The display name, such as `Catppuccin Mocha`.
    public var name: String
    /// Whether the theme can be applied. False for a configured id that no
    /// built-in theme or local file provides.
    public var isInstalled: Bool
    /// Preview colors: background, foreground, red, green, blue, magenta.
    /// Empty for themes without fixed colors, such as the shell-decided one.
    public var swatches: [TermyColor]
}

extension TermyConfig {
    /// Every theme this config can use, in Termy's order: Termy's own themes,
    /// then the collection, then local theme files.
    public func themes() throws(TermyError) -> [TermyTheme] {
        try TermyTheme.parse(settingsSchemaJSON())
    }

    /// The `theme` value this config sets, if any.
    public func themeSetting() throws(TermyError) -> String? {
        try TermyTheme.themeSetting(in: settingsSchemaJSON())
    }

    /// The value root setting `key` has in this config, such as
    /// `theme_mode`'s `system` or `theme_dark`'s theme id; Termy's default
    /// when the file doesn't set it. Nil for a key Termy doesn't know.
    public func settingValue(_ key: String) throws(TermyError) -> String? {
        try TermyTheme.settingValue(key, in: settingsSchemaJSON())
    }
}

extension TermyTheme {
    private struct Schema: Decodable {
        struct Section: Decodable { var groups: [Group]? }
        struct Group: Decodable { var settings: [Setting] }
        struct Setting: Decodable {
            var key: String
            var value: String?
            var choices: [Choice]?
        }
        struct Choice: Decodable {
            var value: String
            var label: String
            var installed: Bool?
            var swatches: [String]?
        }
        var sections: [Section]
    }

    private static func themeEntry(in json: Data) throws(TermyError) -> Schema.Setting? {
        try entry("theme", in: json)
    }

    private static func entry(_ key: String, in json: Data) throws(TermyError) -> Schema.Setting? {
        let schema: Schema
        do {
            schema = try JSONDecoder().decode(Schema.self, from: json)
        } catch {
            throw TermyError(code: .invalidArgument, operation: "decode settings schema")
        }
        return schema.sections.lazy
            .flatMap { $0.groups ?? [] }
            .flatMap(\.settings)
            .first { $0.key == key }
    }

    static func parse(_ json: Data) throws(TermyError) -> [TermyTheme] {
        (try themeEntry(in: json)?.choices ?? []).map { choice in
            TermyTheme(
                id: choice.value, name: choice.label, isInstalled: choice.installed ?? true,
                swatches: (choice.swatches ?? []).compactMap(TermyColor.init(hex:)))
        }
    }

    static func settingValue(_ key: String, in json: Data) throws(TermyError) -> String? {
        try entry(key, in: json)?.value
    }

    static func themeSetting(in json: Data) throws(TermyError) -> String? {
        try themeEntry(in: json)?.value.flatMap { $0.isEmpty ? nil : $0 }
    }
}

extension TermyColor {
    /// A color from `#rrggbb` or `rrggbb`.
    public init?(hex: String) {
        var digits = Substring(hex)
        if digits.hasPrefix("#") { digits = digits.dropFirst() }
        guard digits.count == 6, let value = UInt32(digits, radix: 16) else { return nil }
        self.init(r: UInt8(value >> 16 & 0xff), g: UInt8(value >> 8 & 0xff), b: UInt8(value & 0xff))
    }
}
