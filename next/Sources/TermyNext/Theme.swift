import AppKit
import Termy

enum TabStyle: String, CaseIterable { case above, left, right }
enum PaneStyle: String, CaseIterable { case comfortable, compact }
/// Classic draws solid chrome; Liquid puts every surface on Liquid Glass.
enum InterfaceStyle: String, CaseIterable { case classic, liquid }

/// View settings that persist across launches, like Rex's Customize View menu.
@MainActor
final class Preferences {
    static let shared = Preferences()
    static let didChange = Notification.Name("TermyNextPreferencesDidChange")

    private let defaults = UserDefaults.standard

    var tabStyle: TabStyle {
        get { TabStyle(rawValue: defaults.string(forKey: "tabStyle") ?? "") ?? .above }
        set { defaults.set(newValue.rawValue, forKey: "tabStyle"); changed() }
    }
    var paneStyle: PaneStyle {
        get { PaneStyle(rawValue: defaults.string(forKey: "paneStyle") ?? "") ?? .comfortable }
        set { defaults.set(newValue.rawValue, forKey: "paneStyle"); changed() }
    }
    var interfaceStyle: InterfaceStyle {
        get {
            let raw = defaults.string(forKey: "interfaceStyle")
            // "glass" was the translucent style before Liquid replaced it.
            return raw == "glass" ? .liquid : InterfaceStyle(rawValue: raw ?? "") ?? .liquid
        }
        set { defaults.set(newValue.rawValue, forKey: "interfaceStyle"); changed() }
    }
    var sidebarWidth: CGFloat {
        get { CGFloat(defaults.object(forKey: "sidebarWidth") as? Double ?? 240) }
        set { defaults.set(Double(newValue), forKey: "sidebarWidth") }
    }
    /// Run shells in the Termy session host, so they outlive the app. Off
    /// runs new panes inside the app instead.
    var usesSessionHost: Bool {
        get { defaults.object(forKey: "usesSessionHost") as? Bool ?? true }
        set { defaults.set(newValue, forKey: "usesSessionHost"); changed() }
    }

    /// zsh plugins: the completion popup and the ⇧⌘E command box. Off by
    /// default; applies to panes opened after it's turned on.
    var shellPlugins: Bool {
        get { defaults.bool(forKey: "shellPlugins") }
        set { defaults.set(newValue, forKey: "shellPlugins"); changed() }
    }
    /// The size and place of the window last moved or resized, for new
    /// windows and the next launch.
    var lastWindowFrame: NSRect? {
        get { defaults.string(forKey: "lastWindowFrame").map(NSRectFromString).flatMap { $0.width > 0 ? $0 : nil } }
        set { defaults.set(newValue.map(NSStringFromRect), forKey: "lastWindowFrame") }
    }
    /// Whether the window was in full screen when the app last quit.
    var lastWindowFullScreen: Bool {
        get { defaults.bool(forKey: "lastWindowFullScreen") }
        set { defaults.set(newValue, forKey: "lastWindowFullScreen") }
    }
    /// ssh sends COLORTERM along (zsh and Add Remote Host), so programs on
    /// the server use 24-bit color. On by default.
    var sshSendsColor: Bool {
        get { defaults.object(forKey: "sshSendsColor") as? Bool ?? true }
        set { defaults.set(newValue, forKey: "sshSendsColor") }
    }
    /// Experimental: the completion popup also suggests subcommands,
    /// options and arguments from Termy Next's command specs (git, brew,
    /// cargo, rustup, rustc, bun). Off by default; needs the shell plugins.
    var commandCompletions: Bool {
        get { defaults.bool(forKey: "commandCompletions") }
        set { defaults.set(newValue, forKey: "commandCompletions"); changed() }
    }
    /// ↑ and ↓ move through the completion list as soon as it shows. Off,
    /// they go to the shell (history) until ⌃N / ⌃P picks a row. On by default.
    var completionArrowKeys: Bool {
        get { defaults.object(forKey: "completionArrowKeys") as? Bool ?? true }
        set { defaults.set(newValue, forKey: "completionArrowKeys") }
    }
    /// The agent the command box asks, a `ShellAgent.id` such as "claude:haiku".
    var shellAgent: String {
        get { defaults.string(forKey: "shellAgent") ?? "claude:haiku" }
        set { defaults.set(newValue, forKey: "shellAgent") }
    }

    // Notifications (OSC 7501). These don't restyle windows, so setting
    // them posts no change.
    private func flag(_ key: String) -> Bool { defaults.object(forKey: key) as? Bool ?? true }
    /// Desktop notifications for program status at all.
    var notificationsEnabled: Bool {
        get { flag("notificationsEnabled") }
        set { defaults.set(newValue, forKey: "notificationsEnabled") }
    }
    var notifyWhenBlocked: Bool {
        get { flag("notifyWhenBlocked") }
        set { defaults.set(newValue, forKey: "notifyWhenBlocked") }
    }
    var notifyWhenDone: Bool {
        get { flag("notifyWhenDone") }
        set { defaults.set(newValue, forKey: "notifyWhenDone") }
    }
    var notifyWhenFailed: Bool {
        get { flag("notifyWhenFailed") }
        set { defaults.set(newValue, forKey: "notifyWhenFailed") }
    }
    var notificationSound: Bool {
        get { flag("notificationSound") }
        set { defaults.set(newValue, forKey: "notificationSound") }
    }

    private func changed() {
        NotificationCenter.default.post(name: Self.didChange, object: nil)
    }
}

/// Termy Next's own config: a Termy-format config file of its own at
/// `~/.config/termy-next/config.txt`, separate from the Termy app's, with
/// local themes in the `themes` folder beside it. Render settings and chrome
/// colors are derived from it.
@MainActor
final class AppConfig {
    static let shared = AppConfig()

    static var directory: URL {
        if let home = ProcessInfo.processInfo.environment["TERMY_NEXT_HOME"] {
            return URL(fileURLWithPath: home).appendingPathComponent("config", isDirectory: true)
        }
        return FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".config/termy-next", isDirectory: true)
    }
    static var configURL: URL { directory.appendingPathComponent("config.txt") }
    static var themesURL: URL { directory.appendingPathComponent("themes", isDirectory: true) }
    /// The Termy app's config, offered for import.
    static var termyConfigURL: URL {
        FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".config/termy/config.txt")
    }

    /// The config as it is on disk; themes and settings come from this one.
    private(set) var fileConfig: TermyConfig
    /// The config terminals start from. Each pane adds its own ⌘+ / ⌘− zoom.
    private(set) var termyConfig: TermyConfig
    private(set) var renderConfig: RenderConfig
    private(set) var theme: Theme
    /// The font size the config file sets, before any pane's zoom.
    private(set) var baseFontSize: Float = 13
    private var cachedThemes: [TermyTheme]?
    /// Render settings per zoomed font size, so panes zoomed alike share one.
    private var zoomedRenderConfigs: [Float: RenderConfig] = [:]

    private init() {
        let config = try! TermyConfig(contents: "")
        fileConfig = config
        termyConfig = config
        renderConfig = try! config.renderConfig(for: .dark)
        theme = Theme(renderConfig)
        reload()
    }

    /// Creates the config file on first launch, empty so Termy's defaults apply.
    private static func ensureConfigFile() {
        let fm = FileManager.default
        guard !fm.fileExists(atPath: configURL.path) else { return }
        try? fm.createDirectory(at: directory, withIntermediateDirectories: true)
        let header = """
            # Termy Next config. Same format as Termy's config.txt; see
            # https://termy.run/docs for every setting. Change the common ones
            # in Settings (⌘,), or import your Termy config there.

            """
        try? header.write(to: configURL, atomically: true, encoding: .utf8)
    }

    /// Re-reads the config file.
    func reload() {
        Self.ensureConfigFile()
        let base = (try? TermyConfig(path: Self.configURL.path)) ?? fileConfig
        fileConfig = base
        termyConfig = base
        cachedThemes = nil
        zoomedRenderConfigs = [:]
        if let render = try? base.renderConfig(for: appearance) { renderConfig = render }
        baseFontSize = renderConfig.fontSize
        theme = Theme(renderConfig)
    }

    private var appearance: Appearance {
        NSApp?.effectiveAppearance.isDarkAqua == false ? .light : .dark
    }

    /// The font size a pane zoomed by `zoom` points shows, within 6–72.
    func fontSize(zoom: Float) -> Float { max(6, min(72, baseFontSize + zoom)) }

    /// Every theme the config can use: the built-in library and local files.
    var themes: [TermyTheme] {
        if let cachedThemes { return cachedThemes }
        let themes = (try? fileConfig.themes()) ?? []
        cachedThemes = themes
        return themes
    }

    /// The `theme` the config file sets, if any.
    var themeSetting: String? { try? fileConfig.themeSetting() }

    /// Writes a root setting such as `theme` or `font_size` to the config file.
    func set(_ key: String, to value: String?) throws {
        if let value {
            try TermySettings.set(key, to: value, inConfigAt: Self.configURL.path)
        } else {
            try TermySettings.reset(key, inConfigAt: Self.configURL.path)
        }
    }

    /// Replaces Termy Next's config with the one at `source`, keeping the old
    /// file as `config.txt.bak`, and copies the local themes next to it that
    /// Termy Next doesn't have yet.
    func importConfig(from source: URL) throws {
        let fm = FileManager.default
        let contents = try String(contentsOf: source, encoding: .utf8)
        try fm.createDirectory(at: Self.directory, withIntermediateDirectories: true)
        let backup = Self.directory.appendingPathComponent("config.txt.bak")
        if fm.fileExists(atPath: Self.configURL.path) {
            try? fm.removeItem(at: backup)
            try fm.copyItem(at: Self.configURL, to: backup)
        }
        try contents.write(to: Self.configURL, atomically: true, encoding: .utf8)
        let sourceThemes = source.deletingLastPathComponent().appendingPathComponent("themes", isDirectory: true)
        if let files = try? fm.contentsOfDirectory(at: sourceThemes, includingPropertiesForKeys: nil) {
            try fm.createDirectory(at: Self.themesURL, withIntermediateDirectories: true)
            for file in files where file.pathExtension == "json" {
                let target = Self.themesURL.appendingPathComponent(file.lastPathComponent)
                if !fm.fileExists(atPath: target.path) { try? fm.copyItem(at: file, to: target) }
            }
        }
    }

    /// Render settings for a terminal view under the current interface style.
    /// Liquid lets the pane's glass show through the default background.
    var terminalRenderConfig: RenderConfig { terminalRenderConfig(zoom: 0) }

    /// Render settings for a pane zoomed by `zoom` points from the config's size.
    func terminalRenderConfig(zoom: Float) -> RenderConfig {
        var config = renderConfig(fontSize: fontSize(zoom: zoom))
        if Preferences.shared.interfaceStyle == .liquid {
            config.backgroundOpacity = 0
            config.backgroundOpacityAffectsCells = false
        }
        return config
    }

    /// Render settings at another font size; cell metrics follow the font.
    private func renderConfig(fontSize size: Float) -> RenderConfig {
        guard size != baseFontSize else { return renderConfig }
        if let cached = zoomedRenderConfigs[size] { return cached }
        let contents = (try? String(contentsOf: Self.configURL, encoding: .utf8)) ?? ""
        let sized = (try? TermyConfig(contents: contents + "\nfont_size = \(size)\n"))
            .flatMap { try? $0.renderConfig(for: appearance) } ?? renderConfig
        zoomedRenderConfigs[size] = sized
        return sized
    }
}

extension NSAppearance {
    var isDarkAqua: Bool { bestMatch(from: [.darkAqua, .aqua]) == .darkAqua }
}

/// Chrome colors derived from the terminal theme.
@MainActor
struct Theme {
    let terminalBackground: NSColor
    let foreground: NSColor
    let accent: NSColor
    let isDark: Bool

    init(_ config: RenderConfig) {
        terminalBackground = NSColor(config.background)
        foreground = NSColor(config.foreground)
        accent = NSColor(config.cursorColor)
        isDark = terminalBackground.luminance < 0.5
    }

    /// Solid window color for the classic style: a step darker than the terminal.
    var chrome: NSColor { terminalBackground.mixed(with: .black, by: isDark ? 0.28 : 0.06) }
    var isLiquid: Bool { Preferences.shared.interfaceStyle == .liquid }
    /// Tint over the window's backdrop blur in Liquid.
    var backdropTint: NSColor { chrome.withAlphaComponent(isDark ? 0.68 : 0.45) }
    /// Glass tints: the pane keeps enough of the terminal color to read text on.
    var paneGlassTint: NSColor { terminalBackground.withAlphaComponent(isDark ? 0.64 : 0.55) }
    var focusedPaneGlassTint: NSColor { terminalBackground.mixed(with: foreground, by: 0.04).withAlphaComponent(isDark ? 0.58 : 0.5) }
    var selectionGlassTint: NSColor { terminalBackground.mixed(with: foreground, by: isDark ? 0.22 : 0.1).withAlphaComponent(0.7) }
    var paneFill: NSColor { terminalBackground }
    var paneStroke: NSColor { (isDark ? NSColor.white : NSColor.black).withAlphaComponent(isDark ? 0.09 : 0.1) }
    var paneOuterStroke: NSColor { NSColor.black.withAlphaComponent(isDark ? 0.35 : 0.08) }
    var textPrimary: NSColor { foreground.withAlphaComponent(0.95) }
    var textSecondary: NSColor { foreground.withAlphaComponent(0.55) }
    var textTertiary: NSColor { foreground.withAlphaComponent(0.35) }
    var hover: NSColor { foreground.withAlphaComponent(0.06) }
    var selected: NSColor { foreground.withAlphaComponent(0.1) }
    var popover: NSColor { terminalBackground.mixed(with: isDark ? .white : .black, by: isDark ? 0.1 : 0.03) }
    var selectionFill: NSColor { accent.withAlphaComponent(isDark ? 0.75 : 0.85) }
    var selectionText: NSColor { accent.luminance > 0.55 ? NSColor.black.withAlphaComponent(0.85) : .white }
    var blocked: NSColor { NSColor.systemBlue }
    var working: NSColor { accent }
    var done: NSColor { NSColor.systemGreen }
    var error: NSColor { NSColor.systemRed }
    var appearance: NSAppearance? { NSAppearance(named: isDark ? .darkAqua : .aqua) }
}

extension NSColor {
    convenience init(_ color: TermyColor) {
        self.init(srgbRed: CGFloat(color.r) / 255, green: CGFloat(color.g) / 255, blue: CGFloat(color.b) / 255, alpha: 1)
    }

    var luminance: CGFloat {
        guard let c = usingColorSpace(.sRGB) else { return 0 }
        return 0.2126 * c.redComponent + 0.7152 * c.greenComponent + 0.0722 * c.blueComponent
    }

    func mixed(with other: NSColor, by amount: CGFloat) -> NSColor {
        blended(withFraction: amount, of: other) ?? self
    }
}
