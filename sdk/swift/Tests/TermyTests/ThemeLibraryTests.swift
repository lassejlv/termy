import Foundation
import Testing
@testable import Termy

@Suite("Theme library and own configs")
struct ThemeLibraryTests {
    @Test func listsBuiltInAndLocalThemes() throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("termy-themes-\(UUID().uuidString.prefix(8))")
        defer { try? FileManager.default.removeItem(at: dir) }
        try FileManager.default.createDirectory(at: dir.appendingPathComponent("themes"), withIntermediateDirectories: true)
        let theme = ##"{"foreground":"#eeeeee","background":"#101010","cursor":"#ff00ff","black":"#000000","red":"#ff0000","green":"#00ff00","yellow":"#ffff00","blue":"#0000ff","magenta":"#ff00ff","cyan":"#00ffff","white":"#ffffff","brightBlack":"#555555","brightRed":"#ff5555","brightGreen":"#55ff55","brightYellow":"#ffff55","brightBlue":"#5555ff","brightMagenta":"#ff55ff","brightCyan":"#55ffff","brightWhite":"#ffffff"}"##
        try theme.write(to: dir.appendingPathComponent("themes/my-own.json"), atomically: true, encoding: .utf8)
        let configPath = dir.appendingPathComponent("config.txt").path
        try "theme = dracula\n".write(toFile: configPath, atomically: true, encoding: .utf8)

        let config = try TermyConfig(path: configPath)
        let themes = try config.themes()
        #expect(themes.count > 100)
        let dracula = try #require(themes.first { $0.id == "dracula" })
        #expect(dracula.name == "Dracula")
        #expect(dracula.swatches.count == 6)
        #expect(themes.contains { $0.id == "my-own" && $0.isInstalled })
        #expect(try config.themeSetting() == "dracula")
    }

    @Test func editsAConfigAtAPath() throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("termy-own-\(UUID().uuidString.prefix(8))")
        defer { try? FileManager.default.removeItem(at: dir) }
        let path = dir.appendingPathComponent("config.txt").path
        try TermySettings.set("theme", to: "nord", inConfigAt: path)
        try TermySettings.set("font_size", to: "15", inConfigAt: path)
        var config = try TermyConfig(path: path)
        #expect(try config.themeSetting() == "nord")
        #expect(try config.renderConfig().fontSize == 15)
        try TermySettings.reset("font_size", inConfigAt: path)
        config = try TermyConfig(path: path)
        #expect(try config.renderConfig().fontSize != 15)
        #expect(throws: TermyError.self) { try TermySettings.set("not_a_key", to: "1", inConfigAt: path) }
    }

    @Test func parsesHexColors() {
        #expect(TermyColor(hex: "#ff8000") == TermyColor(r: 255, g: 128, b: 0))
        #expect(TermyColor(hex: "zz") == nil)
    }
}
