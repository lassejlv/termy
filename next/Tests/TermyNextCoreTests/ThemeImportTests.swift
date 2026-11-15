import Foundation
import Testing
@testable import TermyNextCore

@Suite struct ThemeImportTests {
    /// A fake home folder with `files` at their relative paths.
    private func home(_ files: [String: String]) throws -> URL {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("theme-import-\(UUID().uuidString)")
        for (path, text) in files {
            let url = root.appendingPathComponent(path)
            try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
            try text.write(to: url, atomically: true, encoding: .utf8)
        }
        return root
    }

    @Test func themeIDsMatchLibtermy() {
        #expect(ThemeImporter.themeID("  Tokyo_Night  ") == "tokyo-night")
        #expect(ThemeImporter.themeID("gruvbox---dark") == "gruvbox-dark")
        #expect(ThemeImporter.themeID("Dracula+ High Contrast") == "dracula-plus-high-contrast")
        #expect(ThemeImporter.themeID("Ember (Ghostty)") == "ember-ghostty")
    }

    @Test func hexValues() {
        #expect(ThemeImporter.hex("#1A1B26") == "#1a1b26")
        #expect(ThemeImporter.hex("1a1b26") == "#1a1b26")
        #expect(ThemeImporter.hex("'0x1a1b26'") == "#1a1b26")
        #expect(ThemeImporter.hex("#fff") == "#ffffff")
        #expect(ThemeImporter.hex("CellForeground") == nil)
        #expect(ThemeImporter.hex("none") == nil)
    }

    @Test func ghosttyThemeWithOverrides() throws {
        let root = try home([
            ".config/ghostty/config": """
                # comment
                theme = light:Paper,dark:Ember
                background = 101010
                config-file = ?extra.conf
                """,
            ".config/ghostty/extra.conf": "palette = 1=#ff0000\n",
            ".config/ghostty/themes/Ember": """
                background = #121212
                foreground = #d0d0d0
                cursor-color = #b8bcf5
                palette = 0=#1e1e1e
                palette = 1=#e8697d
                palette = 9=#f28b9b
                """,
        ])
        let theme = try ThemeImporter(home: root, environment: [:], prefersDark: true).ghostty()
        #expect(theme.name == "Ember")
        #expect(theme.background == "#101010")
        #expect(theme.foreground == "#d0d0d0")
        #expect(theme.cursor == "#b8bcf5")
        #expect(theme.palette[1] == "#ff0000")
        #expect(theme.palette[9] == "#f28b9b")
        let light = try ThemeImporter(home: root, environment: [:], prefersDark: false).ghostty()
        #expect(light.name == "Paper")
        #expect(light.namedTheme == "Paper")
    }

    @Test func ghosttyMissingConfig() throws {
        let root = try home([:])
        #expect(throws: ThemeImportError.self) { try ThemeImporter(home: root, environment: [:]).ghostty() }
    }

    @Test func alacrittyImportsAndTables() throws {
        let root = try home([
            ".config/alacritty/alacritty.toml": """
                [general]
                import = [
                  "~/.config/alacritty/themes/tokyo_night.toml", # the theme
                ]

                [colors.primary]
                background = '#000000'
                """,
            ".config/alacritty/themes/tokyo_night.toml": """
                [colors.primary]
                background = '#1a1b26'
                foreground = '#c0caf5'

                [colors.normal]
                black = '#15161e'
                red = "0xf7768e"

                [colors.bright]
                red = '#ff7a93'

                [colors.cursor]
                text = 'CellBackground'
                cursor = '#c0caf5'
                """,
        ])
        let theme = try ThemeImporter(home: root, environment: [:]).alacritty()
        #expect(theme.name == "Tokyo Night")
        #expect(theme.background == "#000000")
        #expect(theme.foreground == "#c0caf5")
        #expect(theme.cursor == "#c0caf5")
        #expect(theme.palette[0] == "#15161e")
        #expect(theme.palette[1] == "#f7768e")
        #expect(theme.palette[9] == "#ff7a93")
    }

    @Test func tomlInlineTablesAndDottedKeys() {
        let entries = ThemeImporter.toml("""
            colors.primary = { background = "#111111", foreground = "#eeeeee" }
            [colors]
            normal.red = "#aa0000"
            """)
        #expect(entries.contains { $0 == ("colors.primary.background", "\"#111111\"") })
        #expect(entries.contains { $0 == ("colors.normal.red", "\"#aa0000\"") })
    }

    @Test func kittyIncludesAndThemeName() throws {
        let root = try home([
            ".config/kitty/kitty.conf": """
                font_size 13
                # BEGIN_KITTY_THEME
                # Tokyo Night
                include current-theme.conf
                # END_KITTY_THEME
                cursor #ffffff
                """,
            ".config/kitty/current-theme.conf": """
                ## name: Tokyo Night Storm
                foreground #c0caf5
                background   #24283b
                color0 #1d202f
                color15 #c0caf5
                """,
        ])
        let theme = try ThemeImporter(home: root, environment: [:]).kitty()
        #expect(theme.name == "Tokyo Night Storm")
        #expect(theme.background == "#24283b")
        #expect(theme.cursor == "#ffffff")
        #expect(theme.palette[0] == "#1d202f")
        #expect(theme.palette[15] == "#c0caf5")
    }

    @Test func rexCustomThemes() throws {
        let channel = { (v: Double) in ["red": v, "green": v, "blue": v, "alpha": 1, "colorSpace": "srgb"] as [String: Any] }
        let json = try JSONSerialization.data(withJSONObject: [[
            "name": "Imported Theme", "id": "custom-1", "appearance": "dark",
            "palette": ["foreground": channel(1), "background": channel(0), "cursor": channel(0.5),
                        "ansi": (0..<16).map { _ in channel(0.2) }],
        ]])
        let themes = ThemeImporter.rex(json)
        #expect(themes.count == 1)
        #expect(themes[0].foreground == "#ffffff")
        #expect(themes[0].background == "#000000")
        #expect(themes[0].cursor == "#808080")
        #expect(themes[0].palette[15] == "#333333")
        #expect(ThemeImporter.rexIDs(json) == ["custom-1"])
    }

    @Test func termyFileFillsEverything() throws {
        var theme = ImportedTheme(name: "x", source: .kitty)
        theme.foreground = "#eeeeee"
        theme.palette[1] = "#aa0000"
        let colors = theme.termyColors()
        #expect(colors.count == 19)
        #expect(colors["cursor"] == "#eeeeee")
        #expect(colors["bright_red"] == "#aa0000")
        #expect(colors["background"] != nil)
    }

    @Test func detectsFileFormats() throws {
        let root = try home([
            "a.toml": "[colors.primary]\nbackground = '#111111'\n",
            "b": "background = #222222\npalette = 0=#000000\n",
            "c.conf": "background #333333\ncolor1 #aa0000\n",
            "d.json": ##"{"foreground":"#ffffff","bright_white":"#ffffff"}"##,
        ])
        let importer = ThemeImporter(home: root, environment: [:])
        #expect(try importer.file(root.appendingPathComponent("a.toml"))?.source == .alacritty)
        #expect(try importer.file(root.appendingPathComponent("b"))?.background == "#222222")
        #expect(try importer.file(root.appendingPathComponent("c.conf"))?.source == .kitty)
        #expect(try importer.file(root.appendingPathComponent("d.json")) == nil)
    }
}
