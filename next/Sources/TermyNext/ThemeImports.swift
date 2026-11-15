import AppKit
import Termy
import TermyNextCore

/// Imports the theme Ghostty, Alacritty, kitty or Rex uses into Termy Next's
/// `themes` folder, as a Termy theme file.
@MainActor
enum ThemeImports {
    static let rexDefaultsDomain = "com.superlogical.rex"

    static var importer: ThemeImporter {
        var directories: [URL] = []
        if let ghostty = NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.mitchellh.ghostty") {
            directories.append(ghostty.appendingPathComponent("Contents/Resources/ghostty/themes"))
        }
        return ThemeImporter(prefersDark: AppConfig.shared.appearance == .dark, ghosttyThemeDirectories: directories)
    }

    /// What importing from `source` would bring in: one theme for a config
    /// file, Rex's custom themes for Rex.
    static func themes(from source: ThemeImportSource) -> Result<[ImportedTheme], Error> {
        let importer = importer
        do {
            switch source {
            case .ghostty: return .success([try importer.ghostty()])
            case .alacritty: return .success([try importer.alacritty()])
            case .kitty: return .success([try importer.kitty()])
            case .rex: return .success(rexThemes())
            }
        } catch {
            return .failure(error)
        }
    }

    /// Rex's custom themes, the ones it shows now first.
    static func rexThemes() -> [ImportedTheme] {
        guard let defaults = UserDefaults(suiteName: rexDefaultsDomain) else { return [] }
        let raw = defaults.object(forKey: "Workspace.customThemes")
        guard let data = (raw as? Data) ?? (raw as? String)?.data(using: .utf8) else { return [] }
        let themes = ThemeImporter.rex(data)
        let ids = ThemeImporter.rexIDs(data)
        let active = [defaults.string(forKey: "Workspace.darkThemeID"), defaults.string(forKey: "Workspace.lightThemeID")]
        let inUse = Set(zip(ids, themes.indices).filter { active.contains($0.0) }.map(\.1))
        return themes.indices.sorted { inUse.contains($0) && !inUse.contains($1) }.map { themes[$0] }
    }

    /// The source's logo, sized for a menu.
    static func logo(for source: ThemeImportSource, dark: Bool) -> NSImage? {
        let image: NSImage?
        if source == .rex {
            image = NSWorkspace.shared.urlForApplication(withBundleIdentifier: rexDefaultsDomain)
                .map { NSWorkspace.shared.icon(forFile: $0.path) }
                ?? NSImage(systemSymbolName: "terminal", accessibilityDescription: nil)
        } else {
            image = ProgramLogos.image(for: source.rawValue, dark: dark)
        }
        guard let copy = image?.copy() as? NSImage else { return nil }
        copy.size = NSSize(width: 16, height: 16)
        return copy
    }

    /// Saves `theme` as a theme file and returns its id. A theme the config
    /// only names (Ghostty's built-ins) is taken from Termy's own themes
    /// when Termy has it.
    static func install(_ theme: ImportedTheme, existing: [TermyTheme]) throws -> String {
        if let named = theme.namedTheme {
            let id = ThemeImporter.themeID(named)
            if existing.contains(where: { $0.id == id && $0.isInstalled }) { return id }
            if !theme.hasColors {
                throw ImportFailure("\(theme.source.title)'s theme “\(named)” isn't on this Mac, and Termy has no theme by that name.")
            }
        }
        let folder = AppConfig.themesURL
        var name = theme.name
        var id = ThemeImporter.themeID(name)
        // Don't shadow a built-in theme of the same name; re-importing
        // replaces the file an earlier import wrote.
        let isBuiltIn = { (id: String) in
            existing.contains { $0.id == id } && !FileManager.default.fileExists(atPath: folder.appendingPathComponent("\(id).json").path)
        }
        if id.isEmpty || isBuiltIn(id) {
            name = "\(theme.name) (\(theme.source.title))"
            id = ThemeImporter.themeID(name)
        }
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        try theme.termyJSON().write(to: folder.appendingPathComponent("\(id).json"), options: .atomic)
        return id
    }

    /// Imports a theme file of any of the terminals, or a Termy theme as is.
    static func installFile(_ url: URL, existing: [TermyTheme]) throws -> String {
        if let theme = try importer.file(url) { return try install(theme, existing: existing) }
        let folder = AppConfig.themesURL
        let id = ThemeImporter.themeID(url.deletingPathExtension().lastPathComponent)
        guard !id.isEmpty else { throw ThemeImportError.unknownFormat(url.path) }
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let target = folder.appendingPathComponent("\(id).json")
        guard target.standardizedFileURL != url.standardizedFileURL else { return id }
        try? FileManager.default.removeItem(at: target)
        try FileManager.default.copyItem(at: url, to: target)
        return id
    }

    struct ImportFailure: LocalizedError {
        let message: String
        init(_ message: String) { self.message = message }
        var errorDescription: String? { message }
    }
}
