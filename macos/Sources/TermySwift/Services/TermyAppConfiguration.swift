import CTermy
import CoreGraphics
import Foundation

enum TermyAppConfigurationError: Error, CustomStringConvertible {
    case missingConfig

    var description: String {
        switch self {
        case .missingConfig:
            return "libtermy did not return a config handle"
        }
    }
}

/// A config-parse diagnostic reported by the core, mirroring `ConfigDiagnosticKind`
/// in `crates/config_core`. Surfaced to the user so config typos are visible.
struct TermyConfigDiagnostic: Equatable {
    enum Kind: UInt32 {
        case unknown = 0
        case unknownSection = 1
        case unknownRootKey = 2
        case unknownColorKey = 3
        case invalidSyntax = 4
        case invalidValue = 5
        case duplicateRootKey = 6
    }

    var lineNumber: Int
    var kind: Kind
    var message: String
}

struct TermyAppConfiguration {
    var windowWidth: CGFloat
    var windowHeight: CGFloat
    var safety: TermySafetyConfiguration
    var tmux: TermyTmuxConfiguration
    var native: TermyNativeConfiguration
    var uiFontFamily: String
    /// True when the user explicitly set `ui_font_family` in their config (i.e.
    /// the resolved value differs from the shipped default). Used to keep the
    /// Settings UI on the native system font unless a UI font was chosen.
    /// Detected by default-value comparison because the FFI exposes no per-key
    /// presence signal — an explicit `ui_font_family = Menlo` reads as
    /// not-set, which is acceptable (system font is the desired default there).
    var isUIFontExplicitlySet: Bool = false
    var configPath: String?
    var tasks: [TermyTaskConfiguration]
    var keybinds: [TermyKeybindConfiguration]
    var diagnostics: [TermyConfigDiagnostic] = []
    /// Scrollback line cap; command marks are tracked only while history stays
    /// below it (eviction begins at the cap and would otherwise drift marks).
    var scrollbackHistory: Int = 0
    /// Optional lower scrollback cap applied while a native tab/window is
    /// occluded to reduce background memory.
    var inactiveTabScrollback: Int?

    var windowSize: CGSize {
        CGSize(width: windowWidth, height: windowHeight)
    }

    /// A human-readable summary of config-parse diagnostics, or nil when the
    /// config parsed cleanly. Feeds the config error banner.
    var configIssueMessage: String? {
        guard !diagnostics.isEmpty else {
            return nil
        }
        let details = diagnostics.map { diagnostic -> String in
            diagnostic.lineNumber > 0
                ? "line \(diagnostic.lineNumber): \(diagnostic.message)"
                : diagnostic.message
        }
        let heading = diagnostics.count == 1 ? "1 config issue" : "\(diagnostics.count) config issues"
        return ([heading] + details).joined(separator: "\n")
    }

    private static let defaultConfiguration = TermyAppConfiguration(
        windowWidth: 1280,
        windowHeight: 820,
        safety: .default,
        tmux: .default,
        native: .default,
        uiFontFamily: "Menlo",
        configPath: nil,
        tasks: [],
        keybinds: []
    )

    private static let loadedConfiguration = Result {
        try load()
    }

    static let current: TermyAppConfiguration = {
        cachedLoadedOrDefault()
    }()

    static func loadFreshOrDefault() -> TermyAppConfiguration {
        (try? load()) ?? defaultConfiguration
    }

    static func loadFresh() throws -> TermyAppConfiguration {
        try load()
    }

    static func load(contents: String) throws -> TermyAppConfiguration {
        var config: OpaquePointer?
        let bytes = Array(contents.utf8)
        let status = bytes.withUnsafeBufferPointer { buffer in
            termy_config_from_contents(buffer.baseAddress, buffer.count, &config)
        }
        try TermyFfiBridge.requireOK("termy_config_from_contents", status)
        guard let config else {
            throw TermyAppConfigurationError.missingConfig
        }
        defer {
            _ = termy_config_free(config)
        }
        return try load(from: config)
    }

    static let loadErrorMessage: String? = {
        switch loadedConfiguration {
        case .success(let configuration):
            return configuration.configIssueMessage
        case .failure(let error):
            return String(describing: error)
        }
    }()

    private static func cachedLoadedOrDefault() -> TermyAppConfiguration {
        switch loadedConfiguration {
        case .success(let configuration):
            return configuration
        case .failure:
            return defaultConfiguration
        }
    }

    private static func load() throws -> TermyAppConfiguration {
        var config: OpaquePointer?
        try TermyFfiBridge.requireOK("termy_config_load_default", termy_config_load_default(&config))
        guard let config else {
            throw TermyAppConfigurationError.missingConfig
        }
        defer {
            _ = termy_config_free(config)
        }

        return try load(from: config)
    }

    private static func load(from config: OpaquePointer) throws -> TermyAppConfiguration {
        var width: Float = Float(defaultConfiguration.windowWidth)
        var height: Float = Float(defaultConfiguration.windowHeight)
        try TermyFfiBridge.requireOK(
            "termy_config_window_size",
            termy_config_window_size(config, &width, &height)
        )

        var safety = TermyFfiSafetyConfig()
        try TermyFfiBridge.requireOK(
            "termy_config_safety",
            termy_config_safety(config, &safety)
        )

        var native = TermyFfiNativeConfig()
        try TermyFfiBridge.requireOK(
            "termy_config_native",
            termy_config_native(config, &native)
        )

        let tmuxBinary = try readBytes("termy_config_tmux_binary") {
            termy_config_tmux_binary(config, &$0)
        }
        let uiFontFamily = try readBytes("termy_config_ui_font_family") {
            termy_config_ui_font_family(config, &$0)
        }
        let trimmedUIFont = (uiFontFamily ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        let isUIFontExplicitlySet = !trimmedUIFont.isEmpty
            && trimmedUIFont != defaultConfiguration.uiFontFamily
        let configPath = try readBytes("termy_config_path") {
            termy_config_path(config, &$0)
        }

        let tasksJSON = try readBytes("termy_config_tasks_json") {
            termy_config_tasks_json(config, &$0)
        }
        let tasks = try JSONDecoder().decode(
            [TermyTaskConfiguration].self,
            from: Data((tasksJSON ?? "").utf8)
        )

        let keybindsJSON = try readBytes("termy_config_keybinds_json") {
            termy_config_keybinds_json(config, &$0)
        }
        let keybinds = try JSONDecoder().decode(
            [TermyKeybindConfiguration].self,
            from: Data((keybindsJSON ?? "").utf8)
        )

        return TermyAppConfiguration(
            windowWidth: CGFloat(max(320, width)),
            windowHeight: CGFloat(max(240, height)),
            safety: TermySafetyConfiguration(safety),
            tmux: TermyTmuxConfiguration(native, binary: tmuxBinary ?? "tmux"),
            native: TermyNativeConfiguration(native),
            uiFontFamily: Self.normalizedUIFontFamily(uiFontFamily ?? defaultConfiguration.uiFontFamily),
            isUIFontExplicitlySet: isUIFontExplicitlySet,
            configPath: configPath,
            tasks: tasks,
            keybinds: keybinds,
            diagnostics: try readDiagnostics(from: config),
            scrollbackHistory: Int(termy_config_runtime_scrollback_history(config)),
            inactiveTabScrollback: try readInactiveTabScrollback(from: config)
        )
    }

    private static func readInactiveTabScrollback(from config: OpaquePointer) throws -> Int? {
        var enabled = false
        var value = 0
        try TermyFfiBridge.requireOK(
            "termy_config_runtime_inactive_tab_scrollback",
            termy_config_runtime_inactive_tab_scrollback(config, &enabled, &value)
        )
        return enabled ? Int(value) : nil
    }

    /// Reads the parser's diagnostics for `config` (unknown keys, invalid values,
    /// etc.) so they can be surfaced instead of silently ignored.
    private static func readDiagnostics(from config: OpaquePointer) throws -> [TermyConfigDiagnostic] {
        var batch = TermyFfiConfigDiagnosticBatch()
        try TermyFfiBridge.requireOK(
            "termy_config_diagnostics",
            termy_config_diagnostics(config, &batch)
        )
        defer {
            _ = termy_config_diagnostics_free(&batch)
        }
        guard let ptr = batch.diagnostics_ptr, batch.diagnostics_len > 0 else {
            return []
        }
        return UnsafeBufferPointer(start: ptr, count: batch.diagnostics_len).map { diagnostic in
            TermyConfigDiagnostic(
                lineNumber: Int(diagnostic.line_number),
                kind: TermyConfigDiagnostic.Kind(rawValue: diagnostic.kind) ?? .unknown,
                message: TermyFfiBridge.string(from: diagnostic.message) ?? ""
            )
        }
    }

    /// Reads an FFI byte buffer via `call`, frees it, and returns its UTF-8 string.
    private static func readBytes(
        _ operation: String,
        _ call: (inout TermyFfiBytes) -> TermyFfiStatus
    ) throws -> String? {
        var bytes = TermyFfiBytes()
        try TermyFfiBridge.requireOK(operation, call(&bytes))
        defer {
            if bytes.ptr != nil {
                _ = termy_buffer_free(bytes)
            }
        }
        return TermyFfiBridge.string(from: bytes)
    }

    private static func normalizedUIFontFamily(_ value: String) -> String {
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? defaultConfiguration.uiFontFamily : trimmed
    }

}

struct TermyTmuxConfiguration {
    var enabled: Bool
    var persistence: Bool
    var exclusive: Bool
    var binary: String
    var showActivePaneBorder: Bool

    static let `default` = TermyTmuxConfiguration(
        enabled: false,
        persistence: true,
        exclusive: false,
        binary: "tmux",
        showActivePaneBorder: true
    )

    init(
        enabled: Bool,
        persistence: Bool,
        exclusive: Bool,
        binary: String,
        showActivePaneBorder: Bool
    ) {
        self.enabled = enabled
        self.persistence = persistence
        self.exclusive = exclusive
        self.binary = binary
        self.showActivePaneBorder = showActivePaneBorder
    }

    init(_ ffiConfig: TermyFfiNativeConfig, binary: String) {
        enabled = ffiConfig.tmux_enabled
        persistence = ffiConfig.tmux_persistence
        exclusive = ffiConfig.tmux_exclusive
        self.binary = binary
        showActivePaneBorder = ffiConfig.tmux_show_active_pane_border
    }
}

struct TermySafetyConfiguration {
    var warnOnQuit: Bool
    var warnOnQuitWithRunningProcess: Bool

    static let `default` = TermySafetyConfiguration(
        warnOnQuit: false,
        warnOnQuitWithRunningProcess: true
    )

    init(warnOnQuit: Bool, warnOnQuitWithRunningProcess: Bool) {
        self.warnOnQuit = warnOnQuit
        self.warnOnQuitWithRunningProcess = warnOnQuitWithRunningProcess
    }

    init(_ ffiConfig: TermyFfiSafetyConfig) {
        warnOnQuit = ffiConfig.warn_on_quit
        warnOnQuitWithRunningProcess = ffiConfig.warn_on_quit_with_running_process
    }

    static func loadCurrent() -> TermySafetyConfiguration {
        do {
            return try TermyAppConfiguration.loadFresh().safety
        } catch {
            return .default
        }
    }
}

struct TermyNativeConfiguration {
    var autoUpdate: Bool
    var simpleMode: Bool
    var nativeTabPersistence: Bool
    var nativeLayoutAutosave: Bool
    var nativeBufferPersistence: Bool
    var showDebugOverlay: Bool
    var onboardingComplete: Bool
    var tabCloseVisibility: TermyTabCloseVisibility
    var tabWidthMode: TermyTabWidthMode
    var tabBarPosition: TermyTabBarPosition
    var nativeTabPlacement: TermyNativeTabPlacement
    var tabSwitchModifierHints: Bool
    var chromeContrast: Bool
    var commandPaletteShowKeybinds: Bool
    var appIcon: TermyAppIcon
    var shellIntegrationEnabled: Bool
    var progressIndicatorEnabled: Bool
    var autoHideTabbar: Bool
    var showTermyInTitlebar: Bool
    var macosOptionAsAlt: Bool

    static let `default` = TermyNativeConfiguration(
        autoUpdate: true,
        simpleMode: false,
        nativeTabPersistence: false,
        nativeLayoutAutosave: false,
        nativeBufferPersistence: false,
        showDebugOverlay: false,
        onboardingComplete: true,
        tabCloseVisibility: .hover,
        tabWidthMode: .uniform,
        tabBarPosition: .top,
        nativeTabPlacement: .nativeTabbar,
        tabSwitchModifierHints: true,
        chromeContrast: false,
        commandPaletteShowKeybinds: true,
        appIcon: .default,
        shellIntegrationEnabled: true,
        progressIndicatorEnabled: true,
        autoHideTabbar: true,
        showTermyInTitlebar: true,
        macosOptionAsAlt: false
    )

    init(
        autoUpdate: Bool,
        simpleMode: Bool,
        nativeTabPersistence: Bool,
        nativeLayoutAutosave: Bool,
        nativeBufferPersistence: Bool,
        showDebugOverlay: Bool,
        onboardingComplete: Bool,
        tabCloseVisibility: TermyTabCloseVisibility,
        tabWidthMode: TermyTabWidthMode,
        tabBarPosition: TermyTabBarPosition,
        nativeTabPlacement: TermyNativeTabPlacement,
        tabSwitchModifierHints: Bool,
        chromeContrast: Bool,
        commandPaletteShowKeybinds: Bool,
        appIcon: TermyAppIcon,
        shellIntegrationEnabled: Bool,
        progressIndicatorEnabled: Bool,
        autoHideTabbar: Bool,
        showTermyInTitlebar: Bool,
        macosOptionAsAlt: Bool
    ) {
        self.autoUpdate = autoUpdate
        self.simpleMode = simpleMode
        self.nativeTabPersistence = nativeTabPersistence
        self.nativeLayoutAutosave = nativeLayoutAutosave
        self.nativeBufferPersistence = nativeBufferPersistence
        self.showDebugOverlay = showDebugOverlay
        self.onboardingComplete = onboardingComplete
        self.tabCloseVisibility = tabCloseVisibility
        self.tabWidthMode = tabWidthMode
        self.tabBarPosition = tabBarPosition
        self.nativeTabPlacement = nativeTabPlacement
        self.tabSwitchModifierHints = tabSwitchModifierHints
        self.chromeContrast = chromeContrast
        self.commandPaletteShowKeybinds = commandPaletteShowKeybinds
        self.appIcon = appIcon
        self.shellIntegrationEnabled = shellIntegrationEnabled
        self.progressIndicatorEnabled = progressIndicatorEnabled
        self.autoHideTabbar = autoHideTabbar
        self.showTermyInTitlebar = showTermyInTitlebar
        self.macosOptionAsAlt = macosOptionAsAlt
    }

    init(_ ffiConfig: TermyFfiNativeConfig) {
        autoUpdate = ffiConfig.auto_update
        simpleMode = ffiConfig.simple_mode
        nativeTabPersistence = ffiConfig.native_tab_persistence
        nativeLayoutAutosave = ffiConfig.native_layout_autosave
        nativeBufferPersistence = ffiConfig.native_buffer_persistence
        showDebugOverlay = ffiConfig.show_debug_overlay
        onboardingComplete = ffiConfig.onboarding_complete
        tabCloseVisibility = TermyTabCloseVisibility(rawValue: ffiConfig.tab_close_visibility) ?? .hover
        tabWidthMode = TermyTabWidthMode(rawValue: ffiConfig.tab_width_mode) ?? .uniform
        tabBarPosition = TermyTabBarPosition(rawValue: ffiConfig.tab_bar_position) ?? .top
        nativeTabPlacement = TermyNativeTabPlacement(rawValue: ffiConfig.native_tab_placement) ?? .nativeTabbar
        tabSwitchModifierHints = ffiConfig.tab_switch_modifier_hints
        chromeContrast = ffiConfig.chrome_contrast
        commandPaletteShowKeybinds = ffiConfig.command_palette_show_keybinds
        appIcon = TermyAppIcon(rawValue: ffiConfig.app_icon) ?? .default
        shellIntegrationEnabled = ffiConfig.shell_integration_enabled
        progressIndicatorEnabled = ffiConfig.progress_indicator_enabled
        autoHideTabbar = ffiConfig.auto_hide_tabbar
        showTermyInTitlebar = ffiConfig.show_termy_in_titlebar
        macosOptionAsAlt = ffiConfig.macos_option_as_alt
    }
}

enum TermyAppIcon: UInt32 {
    case `default` = 0
    case old = 1
}

enum TermyTabCloseVisibility: UInt32 {
    case activeHover = 0
    case hover = 1
    case always = 2
}

enum TermyTabWidthMode: UInt32 {
    case stable = 0
    case activeGrow = 1
    case activeGrowSticky = 2
    case uniform = 3
}

enum TermyTabBarPosition: UInt32 {
    case top = 0
    case right = 1
}

enum TermyNativeTabPlacement: UInt32 {
    case nativeTabbar = 0
    case sidebar = 1
}

struct TermyTaskConfiguration: Codable, Equatable, Identifiable, Hashable {
    var name: String
    var command: String
    var layout: String?
    var workingDirectory: String?

    var id: String {
        name
    }

    enum CodingKeys: String, CodingKey {
        case name
        case command
        case layout
        case workingDirectory = "working_dir"
    }
}

struct TermyKeybindConfiguration: Codable, Equatable, Hashable {
    var trigger: String
    var action: String

    var keybindAction: TerminalKeybindAction {
        TerminalKeybindAction(identifier: action)
    }
}
