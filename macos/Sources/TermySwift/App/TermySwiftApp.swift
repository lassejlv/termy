import AppKit
import SwiftUI

enum AppMetadata {
    static let displayName = "Termy"
    static let bundleIdentifier = "com.lassevestergaard.termy"
}

private enum NativeBenchmarkLaunch {
    static var task: TermyTaskConfiguration? {
        guard let command = ProcessInfo.processInfo.environment["TERMY_BENCHMARK_COMMAND"],
              !command.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        else {
            return nil
        }
        return TermyTaskConfiguration(
            name: "Native benchmark",
            command: command,
            layout: nil,
            workingDirectory: nil
        )
    }
}

@MainActor
enum TermyNativeAppActions {
    static func openConfigFileInEditor() -> Bool {
        guard let configPath = TermyConfigurationStore.shared.configuration.configPath, !configPath.isEmpty else {
            return false
        }

        let url = URL(fileURLWithPath: configPath)
        do {
            let directory = url.deletingLastPathComponent()
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
            if !FileManager.default.fileExists(atPath: url.path) {
                try "# Termy config\n".write(to: url, atomically: true, encoding: .utf8)
            }
            return NSWorkspace.shared.open(url)
        } catch {
            TermyErrorPresenter.present("Couldn't open the config file", error: error)
            return false
        }
    }

    static func prettifyConfig() -> Bool {
        do {
            try SettingsBridge.prettifyConfig()
            TermyConfigurationStore.shared.reload()
            NotificationCenter.default.post(name: .termySettingsChanged, object: nil)
            return true
        } catch {
            TermyErrorPresenter.present("Couldn't prettify the config file", error: error)
            return false
        }
    }

    static func showAppInfo() {
        NSApp.orderFrontStandardAboutPanel(nil)
    }

    /// Opens settings — or the raw config file in simple mode.
    ///
    /// SwiftUI's `openSettings` is only reachable from inside a view, so the
    /// menu item passes it in and the AppKit paths (deeplink, notification)
    /// fall back to the responder-chain action AppKit sends for the standard
    /// Settings… item.
    static func presentSettings(using openSettings: (() -> Void)? = nil) {
        if TermyConfigurationStore.shared.configuration.native.simpleMode,
           openConfigFileInEditor() {
            NSApp.activate(ignoringOtherApps: true)
            return
        }

        if let openSettings {
            openSettings()
        } else if !NSApp.sendAction(Selector(("showSettingsWindow:")), to: nil, from: nil) {
            _ = NSApp.sendAction(Selector(("showPreferencesWindow:")), to: nil, from: nil)
        }
        NSApp.activate(ignoringOtherApps: true)
    }

    static func installCLI() {
        do {
            let message = try SettingsBridge.installCLI()
            TermyToastCenter.shared.show(message, kind: .success)
        } catch {
            TermyErrorPresenter.present("Couldn't install the command line tool", error: error)
        }
    }

    static func restartApp() {
        let bundleURL = Bundle.main.bundleURL
        let configuration = NSWorkspace.OpenConfiguration()
        NSWorkspace.shared.openApplication(at: bundleURL, configuration: configuration) { _, _ in
            Task { @MainActor in
                NSApp.terminate(nil)
            }
        }
    }

    static func toggleNativeTabBarVisibility(for window: NSWindow?) -> Bool {
        NativeTabWindowManager.shared.showNativeTabBar(for: window)
    }
}

@main
struct TermySwiftApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
    @FocusedValue(\.terminalCommands) private var terminalCommands
    @StateObject private var configurationStore = TermyConfigurationStore.shared
    @StateObject private var commandRouter = TerminalCommandRouter.shared

    init() {
        // Runs the headless render benchmark and exits when `--benchmark` is
        // passed, before any window is created.
        TermyBenchmarkRunner.runIfRequested()
    }

    private var effectiveTerminalCommands: TerminalCommandSet? {
        guard terminalCommands != nil || commandRouter.hasTerminalStore else {
            return nil
        }
        return terminalCommands ?? commandRouter.focusedCommandSet()
    }

    var body: some Scene {
        // Terminal windows are never SwiftUI-owned: every tab, the first one
        // included, is built by `NativeTabWindowManager` so they all share one
        // window class and one content wrapper. A `WindowGroup` here would add
        // a plain `NSWindow` as tab 1, which cannot take the titlebar-tabs
        // treatment and so lays its tab strip out as an extra row below the
        // titlebar — pushing that tab's content down. `Settings` is the one
        // scene kind AppKit does not present at launch.
        Settings {
            SettingsRootView()
                .termySettingsUIFont()
        }
        .defaultSize(width: 860, height: 600)
        .windowResizability(.contentMinSize)
        .commands {
            CommandGroup(after: .appInfo) {
                Button("Check for Updates…") {
                    Task { await AppUpdater.shared.checkForUpdates(userInitiated: true) }
                }
                Button("Install Command Line Tool…") {
                    TermyNativeAppActions.installCLI()
                }
                Button("Export Diagnostics…") {
                    TermyDiagnosticsExporter.shared.export()
                }
            }

            CommandGroup(replacing: .appSettings) {
                OpenSettingsButton()
            }

            CommandGroup(replacing: .newItem) {
                Button("New Tab") {
                    if let effectiveTerminalCommands {
                        effectiveTerminalCommands.execute(.newTab)
                    } else {
                        NativeTabWindowManager.shared.openNativeTab()
                    }
                }
                .keyboardShortcut("t", modifiers: [.command])
            }

            CommandMenu("Terminal") {
                ForEach(1...9, id: \.self) { tabNumber in
                    Button("Select Tab \(tabNumber)") {
                        NativeTabWindowManager.shared.selectNativeTab(number: tabNumber)
                    }
                    .keyboardShortcut(KeyEquivalent(Character(String(tabNumber))), modifiers: [.command])
                }

                Divider()

                Button("Previous Tab") {
                    NativeTabWindowManager.shared.selectRelativeNativeTab(offset: -1)
                }

                Button("Next Tab") {
                    NativeTabWindowManager.shared.selectRelativeNativeTab(offset: 1)
                }

                Button("Move Tab Left") {
                    NativeTabWindowManager.shared.moveSelectedNativeTab(offset: -1)
                }

                Button("Move Tab Right") {
                    NativeTabWindowManager.shared.moveSelectedNativeTab(offset: 1)
                }

                Divider()

                Button("Split Right") {
                    if !TerminalCommandRouter.shared.splitFocused(.horizontal) {
                        effectiveTerminalCommands?.execute(.splitPaneVertical)
                    }
                }
                .keyboardShortcut("d", modifiers: [.command])
                .disabled(effectiveTerminalCommands == nil)

                Button("Split Down") {
                    if !TerminalCommandRouter.shared.splitFocused(.vertical) {
                        effectiveTerminalCommands?.execute(.splitPaneHorizontal)
                    }
                }
                .keyboardShortcut("d", modifiers: [.command, .shift])
                .disabled(effectiveTerminalCommands == nil)

                Divider()

                Button("Close Pane or Tab") {
                    effectiveTerminalCommands?.execute(.closePaneOrTab)
                }
                .keyboardShortcut("w", modifiers: [.command])
                .disabled(effectiveTerminalCommands == nil)

                Button("Close Pane") {
                    effectiveTerminalCommands?.execute(.closePane)
                }
                .keyboardShortcut("w", modifiers: [.command, .shift])
                .disabled(effectiveTerminalCommands == nil)

                Divider()

                Button("Next Pane") {
                    effectiveTerminalCommands?.execute(.focusPaneNext)
                }
                .keyboardShortcut("o", modifiers: [.command])
                .disabled(effectiveTerminalCommands == nil)

                Button("Previous Pane") {
                    effectiveTerminalCommands?.execute(.focusPanePrevious)
                }
                .keyboardShortcut("o", modifiers: [.command, .shift])
                .disabled(effectiveTerminalCommands == nil)

                Button("Focus Pane Left") {
                    effectiveTerminalCommands?.execute(.focusPane(.left))
                }
                .keyboardShortcut(.leftArrow, modifiers: [.command, .option])
                .disabled(effectiveTerminalCommands == nil)

                Button("Focus Pane Right") {
                    effectiveTerminalCommands?.execute(.focusPane(.right))
                }
                .keyboardShortcut(.rightArrow, modifiers: [.command, .option])
                .disabled(effectiveTerminalCommands == nil)

                Button("Focus Pane Up") {
                    effectiveTerminalCommands?.execute(.focusPane(.up))
                }
                .keyboardShortcut(.upArrow, modifiers: [.command, .option])
                .disabled(effectiveTerminalCommands == nil)

                Button("Focus Pane Down") {
                    effectiveTerminalCommands?.execute(.focusPane(.down))
                }
                .keyboardShortcut(.downArrow, modifiers: [.command, .option])
                .disabled(effectiveTerminalCommands == nil)

                Divider()

                Button("Resize Pane Left") {
                    effectiveTerminalCommands?.execute(.resizePane(.left))
                }
                .keyboardShortcut(.leftArrow, modifiers: [.command, .option, .shift])
                .disabled(effectiveTerminalCommands == nil)

                Button("Resize Pane Right") {
                    effectiveTerminalCommands?.execute(.resizePane(.right))
                }
                .keyboardShortcut(.rightArrow, modifiers: [.command, .option, .shift])
                .disabled(effectiveTerminalCommands == nil)

                Button("Resize Pane Up") {
                    effectiveTerminalCommands?.execute(.resizePane(.up))
                }
                .keyboardShortcut(.upArrow, modifiers: [.command, .option, .shift])
                .disabled(effectiveTerminalCommands == nil)

                Button("Resize Pane Down") {
                    effectiveTerminalCommands?.execute(.resizePane(.down))
                }
                .keyboardShortcut(.downArrow, modifiers: [.command, .option, .shift])
                .disabled(effectiveTerminalCommands == nil)

                Button("Toggle Pane Zoom") {
                    effectiveTerminalCommands?.execute(.togglePaneZoom)
                }
                .keyboardShortcut(.return, modifiers: [.command])
                .disabled(effectiveTerminalCommands == nil)

                Divider()

                Button("Increase Font Size") {
                    effectiveTerminalCommands?.execute(.increaseFontSize)
                }
                .keyboardShortcut("=", modifiers: [.command])
                .disabled(effectiveTerminalCommands == nil)

                Button("Decrease Font Size") {
                    effectiveTerminalCommands?.execute(.decreaseFontSize)
                }
                .keyboardShortcut("-", modifiers: [.command])
                .disabled(effectiveTerminalCommands == nil)

                Button("Reset Font Size") {
                    effectiveTerminalCommands?.execute(.resetFontSize)
                }
                .keyboardShortcut("0", modifiers: [.command])
                .disabled(effectiveTerminalCommands == nil)

                Divider()

                if !configurationStore.configuration.tasks.isEmpty {
                    Menu("Tasks") {
                        ForEach(configurationStore.configuration.tasks) { task in
                            Button(task.name) {
                                NativeTabWindowManager.shared.openNativeTab(startupTask: task)
                            }
                        }
                    }

                    Divider()
                }

                Button("Send Interrupt") {
                    effectiveTerminalCommands?.execute(.sendInterrupt)
                }
                .keyboardShortcut("c", modifiers: [.control])
                .disabled(effectiveTerminalCommands == nil)
            }

            CommandGroup(after: .textEditing) {
                Button("Find") {
                    effectiveTerminalCommands?.execute(.openSearch)
                }
                .keyboardShortcut("f", modifiers: [.command])
                .disabled(effectiveTerminalCommands == nil)

                Button("Find Next") {
                    effectiveTerminalCommands?.execute(.searchNext)
                }
                .keyboardShortcut("g", modifiers: [.command])
                .disabled(effectiveTerminalCommands == nil)

                Button("Find Previous") {
                    effectiveTerminalCommands?.execute(.searchPrevious)
                }
                .keyboardShortcut("g", modifiers: [.command, .shift])
                .disabled(effectiveTerminalCommands == nil)

                Button("Case Sensitive") {
                    effectiveTerminalCommands?.execute(.toggleSearchCaseSensitive)
                }
                .keyboardShortcut("c", modifiers: [.command, .option])
                .disabled(effectiveTerminalCommands == nil)

                Button("Regex") {
                    effectiveTerminalCommands?.execute(.toggleSearchRegex)
                }
                .keyboardShortcut("r", modifiers: [.command, .option])
                .disabled(effectiveTerminalCommands == nil)

                Button("Close Search") {
                    effectiveTerminalCommands?.execute(.closeSearch)
                }
                .keyboardShortcut(.escape, modifiers: [])
                .disabled(effectiveTerminalCommands == nil)
            }
        }
    }
}

/// Opens settings while preserving the standard shortcut.
private struct OpenSettingsButton: View {
    @Environment(\.openSettings) private var openSettings

    var body: some View {
        Button("Settings…") {
            TermyNativeAppActions.presentSettings { openSettings() }
        }
        .termyUIFont()
        .keyboardShortcut(",", modifiers: .command)
    }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private var closePaneEventMonitor: LocalEventMonitor?
    private var settingsObserver: NSObjectProtocol?
    private var openSettingsObserver: NSObjectProtocol?

    func applicationDidFinishLaunching(_ notification: Notification) {
        TermyNativeLog.lifecycle.notice("Application finished launching")
        NSApp.setActivationPolicy(.regular)
        NSWindow.allowsAutomaticWindowTabbing = true
        AppLogoManager.shared.applyToDock()
        if TermyConfigurationStore.shared.configuration.native.autoUpdate {
            Task { await AppUpdater.shared.checkForUpdates(userInitiated: false) }
        }
        settingsObserver = NotificationCenter.default.addObserver(
            forName: .termySettingsChanged,
            object: nil,
            queue: .main
        ) { _ in
            Task { @MainActor in
                AppLogoManager.shared.reloadFromConfig()
            }
        }
        openSettingsObserver = NotificationCenter.default.addObserver(
            forName: .termyOpenSettingsRequested,
            object: nil,
            queue: .main
        ) { _ in
            Task { @MainActor in
                TermyNativeAppActions.presentSettings()
            }
        }
        if let monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown, handler: { event in
            if ConfiguredKeybindRouter.shared.handle(event) {
                return nil
            }

            if Self.handleTerminalLineEditingShortcut(event) {
                return nil
            }

            if Self.handleDefaultFontZoomShortcut(event) {
                return nil
            }

            guard event.modifierFlags.contains(.command),
                  !event.modifierFlags.contains(.shift),
                  event.charactersIgnoringModifiers?.lowercased() == "w"
            else {
                return event
            }

            return TerminalCommandRouter.shared.closeFocusedPaneIfSplit(for: event) ? nil : event
        }) {
            closePaneEventMonitor = LocalEventMonitor(monitor)
        }
        NSApp.activate(ignoringOtherApps: true)
        NativeTabWindowManager.shared.openNativeTab(startupTask: NativeBenchmarkLaunch.task)
        NativeSoakRunner.shared.startIfRequested()
    }

    func applicationWillTerminate(_ notification: Notification) {
        TermyNativeLog.lifecycle.notice("Application will terminate")
        closePaneEventMonitor?.invalidate()
        for observer in [settingsObserver, openSettingsObserver].compactMap(\.self) {
            NotificationCenter.default.removeObserver(observer)
        }
        settingsObserver = nil
        openSettingsObserver = nil
    }

    /// Deeplinks are delivered here rather than through a SwiftUI `onOpenURL`:
    /// no scene hosts a terminal, so there is no view to receive them.
    func application(_ application: NSApplication, open urls: [URL]) {
        for url in urls {
            TermyDeeplinkRouter.handle(url)
        }
    }

    /// Clicking the Dock icon with every terminal closed reopens one, the way
    /// the SwiftUI window group used to.
    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows: Bool) -> Bool {
        if !NativeTabWindowManager.shared.hasVisibleTerminalWindow {
            NativeTabWindowManager.shared.openNativeTab()
        }
        return true
    }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        if NativeSoakRunner.isRequested {
            return .terminateNow
        }
        let safety = TermySafetyConfiguration.loadCurrent()
        let hasRunningProcess = TerminalCommandRouter.shared.hasRunningTerminalProcess()
        guard safety.warnOnQuit || (safety.warnOnQuitWithRunningProcess && hasRunningProcess) else {
            return .terminateNow
        }

        let alert = NSAlert()
        alert.messageText = hasRunningProcess ? "Quit Termy with running processes?" : "Quit Termy?"
        alert.informativeText = hasRunningProcess
            ? "One or more terminal panes still have a running process."
            : "The safety setting requires confirmation before quitting."
        alert.alertStyle = .warning
        alert.addButton(withTitle: "Quit")
        alert.addButton(withTitle: "Cancel")
        return alert.runModal() == .alertFirstButtonReturn ? .terminateNow : .terminateCancel
    }

    @objc func newWindowForTab(_ sender: Any?) {
        NativeTabWindowManager.shared.openNativeTab()
    }

    private static func handleTerminalLineEditingShortcut(_ event: NSEvent) -> Bool {
        let window = event.window ?? NSApp.keyWindow
        guard let inputView = window?.firstResponder as? KeyboardCaptureView,
              inputView.isInputEnabled,
              let keyInput = NativeKeyEventClassifier.terminalLineEditingInput(for: event),
              let store = TerminalCommandRouter.shared.focusedStore(for: event),
              !store.isSearchInputFocused,
              !store.isCommandPaletteVisible,
              let terminal = store.focusedTerminal
        else {
            return false
        }

        terminal.sendKey(keyInput)
        return true
    }

    private static func handleDefaultFontZoomShortcut(_ event: NSEvent) -> Bool {
        let flags = event.modifierFlags
        guard flags.contains(.command),
              !flags.contains(.control),
              !flags.contains(.option)
        else {
            return false
        }

        guard let terminal = TerminalCommandRouter.shared.focusedStore(for: event)?.focusedTerminal else {
            return false
        }

        switch fontZoomShortcut(for: event) {
        case .increase:
            terminal.increaseFontSize()
        case .decrease:
            terminal.decreaseFontSize()
        case .reset:
            terminal.resetFontSize()
        case nil:
            return false
        }
        return true
    }

    private enum FontZoomShortcut {
        case increase
        case decrease
        case reset
    }

    private static func fontZoomShortcut(for event: NSEvent) -> FontZoomShortcut? {
        let characters = [
            event.characters,
            event.charactersIgnoringModifiers
        ].compactMap { $0?.lowercased() }

        if characters.contains("+") || characters.contains("=") {
            return .increase
        }
        if characters.contains("-") {
            return .decrease
        }
        if characters.contains("0") {
            return .reset
        }

        switch event.keyCode {
        case 24, 69:
            return .increase
        case 27, 78:
            return .decrease
        case 29, 82:
            return .reset
        default:
            return nil
        }
    }
}

enum NativeKeyEventClassifier {
    static func canonicalTriggers(for event: NSEvent) -> Set<String> {
        guard let key = keyName(for: event) else {
            return []
        }

        let flags = event.modifierFlags
        var baseModifiers: [String] = []
        if flags.contains(.control) {
            baseModifiers.append("ctrl")
        }
        if flags.contains(.option) {
            baseModifiers.append("alt")
        }
        if flags.contains(.shift) {
            baseModifiers.append("shift")
        }

        var triggers = Set<String>()
        if flags.contains(.command) {
            triggers.insert((baseModifiers + ["cmd", key]).joined(separator: "-"))
            triggers.insert((baseModifiers + ["secondary", key]).joined(separator: "-"))
        } else {
            triggers.insert((baseModifiers + [key]).joined(separator: "-"))
        }
        return triggers
    }

    static func keyName(for event: NSEvent) -> String? {
        if let digit = digitName(for: event.keyCode) {
            return digit
        }

        switch MacKeyCode(rawValue: event.keyCode) {
        case .returnKey, .keypadEnter:
            return "enter"
        case .tab:
            return "tab"
        case .escape:
            return "escape"
        case .space:
            return "space"
        case .leftArrow:
            return "left"
        case .rightArrow:
            return "right"
        case .downArrow:
            return "down"
        case .upArrow:
            return "up"
        case .home:
            return "home"
        case .end:
            return "end"
        case .deleteBackward:
            return "backspace"
        case .forwardDelete:
            return "delete"
        case nil, .pageUp, .pageDown, .f1, .f2, .f3, .f4, .f5, .f6, .f7, .f8, .f9, .f10, .f11, .f12:
            break
        }

        guard let characters = event.charactersIgnoringModifiers?.lowercased(),
              let scalar = characters.unicodeScalars.first
        else {
            return nil
        }
        return String(scalar)
    }

    static func terminalLineEditingInput(for event: NSEvent) -> TerminalKeyInput? {
        let flags = event.modifierFlags
        guard flags.contains(.command),
              !flags.contains(.control),
              !flags.contains(.option),
              !flags.contains(.shift),
              let key = terminalLineEditingKeyName(for: event.keyCode)
        else {
            return nil
        }

        return TerminalKeyInput(
            key: key,
            platform: true,
            eventKind: event.isARepeat ? .repeat : .press
        )
    }

    private static func digitName(for keyCode: UInt16) -> String? {
        switch keyCode {
        case 18, 83:
            return "1"
        case 19, 84:
            return "2"
        case 20, 85:
            return "3"
        case 21, 86:
            return "4"
        case 23, 87:
            return "5"
        case 22, 88:
            return "6"
        case 26, 89:
            return "7"
        case 28, 91:
            return "8"
        case 25, 92:
            return "9"
        case 29, 82:
            return "0"
        default:
            return nil
        }
    }

    private static func terminalLineEditingKeyName(for keyCode: UInt16) -> String? {
        switch MacKeyCode(rawValue: keyCode) {
        case .leftArrow, .home:
            return "left"
        case .rightArrow, .end:
            return "right"
        case .deleteBackward:
            return "backspace"
        case .forwardDelete:
            return "delete"
        case nil, .returnKey, .keypadEnter, .tab, .escape, .pageUp, .pageDown, .downArrow, .upArrow,
             .f1, .f2, .f3, .f4, .f5, .f6, .f7, .f8, .f9, .f10, .f11, .f12, .space:
            return nil
        }
    }
}

@MainActor
private final class ConfiguredKeybindRouter {
    static let shared = ConfiguredKeybindRouter()

    private var configuration = TermyConfigurationStore.shared.configuration
    private var settingsObserver: NSObjectProtocol?

    private init() {
        settingsObserver = NotificationCenter.default.addObserver(
            forName: .termySettingsChanged,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            Task { @MainActor in
                self?.configuration = TermyConfigurationStore.shared.reload()
            }
        }
    }

    func handle(_ event: NSEvent) -> Bool {
        let triggers = NativeKeyEventClassifier.canonicalTriggers(for: event)
        guard !triggers.isEmpty,
              let keybind = configuration.keybinds.first(where: { triggers.contains($0.trigger) })
        else {
            return false
        }

        return execute(keybind.keybindAction, event: event)
    }

    /// Runs `body` against the window's focused store, reporting whether a store
    /// was present (i.e. whether the keybind was handled).
    private func withFocusedStore(_ event: NSEvent, _ body: (TerminalWorkspaceStore) -> Void) -> Bool {
        guard let store = TerminalCommandRouter.shared.focusedStore(for: event) else {
            return false
        }
        body(store)
        return true
    }

    private func execute(_ action: TerminalKeybindAction, event: NSEvent) -> Bool {
        switch action {
        case .appInfo:
            TermyNativeAppActions.showAppInfo()
            return true
        case .restartApp:
            TermyNativeAppActions.restartApp()
            return true
        case .openConfig:
            return TermyNativeAppActions.openConfigFileInEditor()
        case .prettifyConfig:
            return TermyNativeAppActions.prettifyConfig()
        case .toggleTabBarVisibility:
            return TermyNativeAppActions.toggleNativeTabBarVisibility(for: event.window)
        case .moveTabLeft:
            NativeTabWindowManager.shared.moveSelectedNativeTab(offset: -1)
            return true
        case .moveTabRight:
            NativeTabWindowManager.shared.moveSelectedNativeTab(offset: 1)
            return true
        case .switchTabLeft:
            NativeTabWindowManager.shared.selectRelativeNativeTab(offset: -1)
            return true
        case .switchTabRight:
            NativeTabWindowManager.shared.selectRelativeNativeTab(offset: 1)
            return true
        case .toggleCommandPalette:
            guard let store = TerminalCommandRouter.shared.focusedStore(for: event),
                  !configuration.native.simpleMode
            else {
                return false
            }
            store.toggleCommandPalette()
            return true
        case .newTab:
            NativeTabWindowManager.shared.openNativeTab()
            return true
        case .closeTab:
            (event.window ?? NSApp.keyWindow)?.performClose(nil)
            return true
        case .closePaneOrTab:
            if TerminalCommandRouter.shared.closeFocusedPaneIfSplit(for: event) {
                return true
            }
            (event.window ?? NSApp.keyWindow)?.performClose(nil)
            return true
        case .closePane:
            return withFocusedStore(event) { $0.closeFocusedPane() }
        case .splitPaneVertical:
            return TerminalCommandRouter.shared.splitFocused(.horizontal, for: event.window)
        case .splitPaneHorizontal:
            return TerminalCommandRouter.shared.splitFocused(.vertical, for: event.window)
        case .focusPaneNext:
            return withFocusedStore(event) { $0.focusNextPane() }
        case .focusPanePrevious:
            return withFocusedStore(event) { $0.focusPreviousPane() }
        case .focusPane(let direction):
            return TerminalCommandRouter.shared.focusedStore(for: event)?.focusPane(in: direction) ?? false
        case .resizePane(let direction):
            return TerminalCommandRouter.shared.focusedStore(for: event)?.resizeFocusedPane(in: direction) ?? false
        case .togglePaneZoom:
            return withFocusedStore(event) { $0.toggleFocusedPaneZoom() }
        case .increaseFontSize:
            return withFocusedStore(event) { $0.focusedTerminal?.increaseFontSize() }
        case .decreaseFontSize:
            return withFocusedStore(event) { $0.focusedTerminal?.decreaseFontSize() }
        case .resetFontSize:
            return withFocusedStore(event) { $0.focusedTerminal?.resetFontSize() }
        case .copy:
            return TerminalCommandRouter.shared.focusedStore(for: event)?.focusedTerminal?.copySelection() ?? false
        case .paste:
            guard let text = NSPasteboard.general.string(forType: .string) else {
                return false
            }
            TerminalCommandRouter.shared.focusedStore(for: event)?.focusedTerminal?.paste(text)
            return true
        case .openSearch:
            return withFocusedStore(event) { $0.showSearch() }
        case .closeSearch:
            return withFocusedStore(event) { $0.hideSearch() }
        case .searchNext:
            TerminalCommandRouter.shared.focusedStore(for: event)?.focusedTerminal?.selectNextSearchMatch()
            return true
        case .searchPrevious:
            TerminalCommandRouter.shared.focusedStore(for: event)?.focusedTerminal?.selectPreviousSearchMatch()
            return true
        case .toggleSearchCaseSensitive:
            TerminalCommandRouter.shared.focusedStore(for: event)?.toggleSearchCaseSensitive()
            return true
        case .toggleSearchRegex:
            TerminalCommandRouter.shared.focusedStore(for: event)?.toggleSearchRegex()
            return true
        case .switchToTab(let number):
            NativeTabWindowManager.shared.selectNativeTab(number: number)
            return true
        case .minimizeWindow:
            (event.window ?? NSApp.keyWindow)?.miniaturize(nil)
            return true
        case .quit:
            NSApp.terminate(nil)
            return true
        case .clearScrollback, .sendInterrupt, .runTask, .unknown:
            // Not bound as keybinds (palette-only or task payload required).
            return false
        }
    }
}

private final class LocalEventMonitor {
    private var invalidateHandler: (() -> Void)?

    init<Token>(_ token: Token) {
        invalidateHandler = {
            NSEvent.removeMonitor(token)
        }
    }

    func invalidate() {
        invalidateHandler?()
        invalidateHandler = nil
    }

    deinit {
        invalidate()
    }
}

@MainActor
struct NativeTabDescriptor: Identifiable {
    var id: ObjectIdentifier
    var index: Int
    var title: String
    var isSelected: Bool
    var isPinned: Bool
    var hasManualTitle: Bool
    fileprivate weak var window: NSWindow?
}

@MainActor
final class NativeTabWindowManager: NSObject, NSWindowDelegate {
    static let shared = NativeTabWindowManager()

    private var retainedWindows: [NSWindow] = []
    private var configuredWindowIDs = Set<ObjectIdentifier>()
    private var windowLifecycleObservers: [ObjectIdentifier: [NSObjectProtocol]] = [:]
    private let tabbingIdentifier = "\(AppMetadata.bundleIdentifier).native-tabs"
    private var entranceTabID: ObjectIdentifier?
    private var entranceIncludesBar = false
    private var entranceDeadline = Date.distantPast

    func configure(_ window: NSWindow) {
        window.tabbingMode = .preferred
        window.tabbingIdentifier = tabbingIdentifier
        window.collectionBehavior.insert(.fullScreenPrimary)
        // Terminal colors are 24-bit sRGB; without a pinned color space the
        // Tahoe compositor backs the window with half-float (8 bytes/px)
        // drawables, doubling window-surface memory (~42 MB → ~15 MB each
        // for a full-screen window).
        window.colorSpace = .sRGB

        let identifier = ObjectIdentifier(window)
        registerWindowLifecycleObservers(for: window)
        guard !configuredWindowIDs.contains(identifier) else {
            applyNativeTabPlacement(for: window)
            applyFocusedTerminalChrome(for: window)
            return
        }
        configuredWindowIDs.insert(identifier)
        if window.title.isEmpty || window.title == "Window" {
            window.title = AppMetadata.displayName
        }
        TerminalWindowChromeApplier.applyFocusedChrome(
            TerminalWindowChromeState(
                title: window.title,
                isFocused: true,
                background: TerminalRenderConfig.default.background,
                backgroundOpacity: TerminalRenderConfig.default.backgroundOpacity,
                backgroundBlur: TerminalRenderConfig.default.backgroundBlur
            ),
            to: window
        )
        applyNativeTabPlacement(for: window)
        window.setContentSize(TermyConfigurationStore.shared.configuration.windowSize)
        window.center()
        if NativeBenchmarkLaunch.task != nil {
            window.level = .floating
            window.orderFrontRegardless()
        }
        postTabsChanged()
    }

    func openNativeTab(startupTask: TermyTaskConfiguration? = nil) {
        let window = makeWindow(startupTask: startupTask)
        retainedWindows.append(window)

        let anchorWindow = NSApp.keyWindow ?? NSApp.mainWindow
        noteTabEntrance(for: window, anchorWindow: anchorWindow)
        if let currentWindow = anchorWindow {
            configure(currentWindow)
            currentWindow.addTabbedWindow(window, ordered: .above)
        }

        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        applyNativeTabPlacement(for: window)
        applyFocusedTerminalChrome(for: window)
        applyFocusedTerminalChromeSoon(for: window)
        postTabsChanged()
    }

    /// Marks a freshly opened tab so the chrome can play its entrance
    /// animation. Each native tab is its own window with its own chrome view,
    /// so opening a tab swaps to a freshly mounted chrome where a plain
    /// SwiftUI insertion transition never fires — the marker travels with the
    /// tab instead and expires shortly after the open.
    func noteTabEntrance(for window: NSWindow, anchorWindow: NSWindow?) {
        entranceTabID = ObjectIdentifier(window)
        let previousTabCount = anchorWindow.map { nativeTabWindows(for: $0).count } ?? 0
        let autoHideTabbar = TermyConfigurationStore.shared.configuration.native.autoHideTabbar
        entranceIncludesBar = autoHideTabbar && previousTabCount <= 1
        entranceDeadline = Date().addingTimeInterval(0.8)
    }

    /// Whether `tabID` was just opened and its chrome should animate it in.
    func shouldAnimateTabEntrance(for tabID: ObjectIdentifier) -> Bool {
        tabID == entranceTabID && Date() < entranceDeadline
    }

    /// Whether the whole tab bar should slide in on `window`: only when the
    /// just-opened tab made the auto-hidden bar visible for the first time.
    func shouldAnimateBarEntrance(for window: NSWindow?) -> Bool {
        guard let window else {
            return false
        }
        return entranceIncludesBar
            && ObjectIdentifier(window) == entranceTabID
            && Date() < entranceDeadline
    }

    func tabDescriptors(for sourceWindow: NSWindow?) -> [NativeTabDescriptor] {
        let sourceWindow = sourceWindow.flatMap { isNativeTerminalTabWindow($0) ? $0 : nil }
            ?? nativeTabSourceWindow()
        guard let sourceWindow else {
            return []
        }

        let selectedWindow = NSApp.keyWindow ?? NSApp.mainWindow
        return nativeTabWindows(for: sourceWindow).enumerated().map { index, window in
            let store = TerminalCommandRouter.shared.store(forWindow: window)
            let trimmedTitle = (store?.tabDisplayTitle ?? window.title).trimmingCharacters(in: .whitespacesAndNewlines)
            return NativeTabDescriptor(
                id: ObjectIdentifier(window),
                index: index,
                title: trimmedTitle.isEmpty ? AppMetadata.displayName : trimmedTitle,
                isSelected: window === selectedWindow,
                isPinned: store?.tabPinned ?? false,
                hasManualTitle: store?.tabManualTitle != nil,
                window: window
            )
        }
    }

    func selectNativeTab(_ descriptor: NativeTabDescriptor) {
        guard let window = descriptor.window else {
            return
        }
        if window.isMiniaturized {
            window.deminiaturize(nil)
        }
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        applyNativeTabPlacement(for: window)
        applyFocusedTerminalChrome(for: window)
        applyFocusedTerminalChromeSoon(for: window)
        postTabsChanged()
    }

    func closeNativeTab(_ descriptor: NativeTabDescriptor) {
        descriptor.window?.performClose(nil)
        postTabsChanged()
    }

    func setNativeTabPinned(_ descriptor: NativeTabDescriptor, pinned: Bool) {
        guard let window = descriptor.window,
              let store = TerminalCommandRouter.shared.store(forWindow: window)
        else {
            return
        }
        store.setTabPinned(pinned)
        postTabsChanged()
    }

    func renameNativeTab(_ descriptor: NativeTabDescriptor, title: String) {
        guard let window = descriptor.window,
              let store = TerminalCommandRouter.shared.store(forWindow: window)
        else {
            return
        }
        store.renameTab(title)
        window.title = store.tabDisplayTitle
        postTabsChanged()
    }

    /// Brings a tabbed window to the front, restoring it if miniaturized.
    private func activateTabbedWindow(_ window: NSWindow) {
        if window.isMiniaturized {
            window.deminiaturize(nil)
        }
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        applyNativeTabPlacement(for: window)
        applyFocusedTerminalChrome(for: window)
        postTabsChanged()
    }

    func selectNativeTab(number: Int) {
        let index = number - 1
        guard index >= 0,
              let sourceWindow = nativeTabSourceWindow()
        else {
            return
        }

        let tabbedWindows = nativeTabWindows(for: sourceWindow)
        guard tabbedWindows.indices.contains(index) else {
            return
        }

        activateTabbedWindow(tabbedWindows[index])
    }

    func selectRelativeNativeTab(offset: Int) {
        guard offset != 0,
              let sourceWindow = nativeTabSourceWindow()
        else {
            return
        }

        let tabbedWindows = nativeTabWindows(for: sourceWindow)
        guard !tabbedWindows.isEmpty else {
            return
        }

        let selectedWindow = NSApp.keyWindow ?? NSApp.mainWindow
        let currentIndex = tabbedWindows.firstIndex { $0 === selectedWindow } ?? 0
        let targetIndex = (currentIndex + offset + tabbedWindows.count) % tabbedWindows.count
        activateTabbedWindow(tabbedWindows[targetIndex])
    }

    func moveSelectedNativeTab(offset: Int) {
        guard offset != 0,
              let sourceWindow = nativeTabSourceWindow()
        else {
            return
        }

        let tabbedWindows = nativeTabWindows(for: sourceWindow)
        guard tabbedWindows.count > 1 else {
            return
        }

        let selectedWindow = NSApp.keyWindow ?? NSApp.mainWindow
        guard let currentIndex = tabbedWindows.firstIndex(where: { $0 === selectedWindow }) else {
            return
        }
        let targetIndex = max(0, min(tabbedWindows.count - 1, currentIndex + offset))
        guard targetIndex != currentIndex else {
            return
        }

        let movingWindow = tabbedWindows[currentIndex]
        let anchorWindow = tabbedWindows[targetIndex]
        anchorWindow.addTabbedWindow(movingWindow, ordered: offset < 0 ? .below : .above)
        movingWindow.makeKeyAndOrderFront(nil)
        applyNativeTabPlacement(for: movingWindow)
        applyFocusedTerminalChrome(for: movingWindow)
        applyFocusedTerminalChromeSoon(for: movingWindow)
        postTabsChanged()
    }

    /// Moves a specific tab to `targetIndex`, used by drag-to-reorder in the
    /// custom tab chrome.
    func moveNativeTab(_ descriptor: NativeTabDescriptor, toIndex targetIndex: Int) {
        guard let movingWindow = descriptor.window else {
            return
        }

        let tabbedWindows = nativeTabWindows(for: movingWindow)
        guard let currentIndex = tabbedWindows.firstIndex(where: { $0 === movingWindow }) else {
            return
        }
        let clamped = max(0, min(tabbedWindows.count - 1, targetIndex))
        guard clamped != currentIndex else {
            return
        }

        let anchorWindow = tabbedWindows[clamped]
        anchorWindow.addTabbedWindow(movingWindow, ordered: clamped < currentIndex ? .below : .above)
        movingWindow.makeKeyAndOrderFront(nil)
        applyNativeTabPlacement(for: movingWindow)
        applyFocusedTerminalChrome(for: movingWindow)
        applyFocusedTerminalChromeSoon(for: movingWindow)
        postTabsChanged()
    }

    func showNativeTabBar(for window: NSWindow?) -> Bool {
        guard let window = window ?? NSApp.keyWindow ?? NSApp.mainWindow,
              isNativeTerminalTabWindow(window)
        else {
            return false
        }
        applyNativeTabPlacement(for: window)
        applyFocusedTerminalChrome(for: window)
        return true
    }

    func windowWillClose(_ notification: Notification) {
        guard let window = notification.object as? NSWindow else {
            return
        }
        handleWindowWillClose(window)
    }

    private func handleWindowWillClose(_ window: NSWindow) {
        retainedWindows.removeAll { $0 === window }
        configuredWindowIDs.remove(ObjectIdentifier(window))
        unregisterWindowLifecycleObservers(for: window)
        postTabsChanged()
    }

    func windowDidBecomeKey(_ notification: Notification) {
        if let window = notification.object as? NSWindow {
            handleWindowDidBecomeKey(window)
        }
    }

    private func handleWindowDidBecomeKey(_ window: NSWindow) {
        applyNativeTabPlacement(for: window)
        applyFocusedTerminalChrome(for: window)
        applyFocusedTerminalChromeSoon(for: window)
        for tabWindow in nativeTabWindows(for: window) {
            (tabWindow as? TitlebarTabsWindow)?.refreshTitlebarTabsLayout()
        }
        postTabsChanged()
    }

    /// Suspend refresh polling for a window's terminals while it is fully
    /// occluded (e.g. a background native tab), and resume when it becomes
    /// visible again. Keeps occluded tabs from competing for the main run loop.
    func windowDidChangeOcclusionState(_ notification: Notification) {
        guard let window = notification.object as? NSWindow,
              let store = TerminalCommandRouter.shared.store(forWindow: window)
        else {
            return
        }
        handleWindowDidChangeOcclusionState(window, store: store)
    }

    private func handleWindowDidChangeOcclusionState(_ window: NSWindow, store: TerminalWorkspaceStore) {
        if window.occlusionState.contains(.visible) || NativeBenchmarkLaunch.task != nil {
            store.resumeRefresh()
            applyFocusedTerminalChrome(for: window)
            applyFocusedTerminalChromeSoon(for: window)
        } else {
            store.suspendRefresh()
        }
    }

    /// The one place a terminal window is built. Every tab goes through here,
    /// so they all share a window class, chrome, content wrapper, and minimum
    /// size — a tab whose window came from anywhere else would lay its titlebar
    /// out differently and shift its content vertically.
    private func makeWindow(startupTask: TermyTaskConfiguration? = nil) -> NSWindow {
        let windowSize = TermyConfigurationStore.shared.configuration.windowSize
        let window = TitlebarTabsWindow(
            contentRect: NSRect(x: 0, y: 0, width: windowSize.width, height: windowSize.height),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.center()
        window.contentViewController = NSHostingController(
            rootView: TerminalWorkspaceView(initialTask: startupTask).termyUIFont()
        )
        window.contentMinSize = Self.minimumContentSize
        window.isReleasedWhenClosed = false
        configure(window)
        return window
    }

    private static let minimumContentSize = NSSize(width: 760, height: 480)

    var hasVisibleTerminalWindow: Bool {
        NSApp.windows.contains { $0.isVisible && isNativeTerminalTabWindow($0) }
    }

    private func nativeTabSourceWindow() -> NSWindow? {
        for window in [NSApp.keyWindow, NSApp.mainWindow].compactMap(\.self) {
            if isNativeTerminalTabWindow(window) {
                return window
            }
        }

        return NSApp.windows.first(where: isNativeTerminalTabWindow)
    }

    private func nativeTabWindows(for sourceWindow: NSWindow) -> [NSWindow] {
        var windows: [NSWindow] = []
        var seen = Set<ObjectIdentifier>()

        func append(_ window: NSWindow) {
            let identifier = ObjectIdentifier(window)
            guard seen.insert(identifier).inserted else {
                return
            }
            windows.append(window)
        }

        if let tabGroup = sourceWindow.tabGroup {
            tabGroup.windows.forEach(append)
            NSApp.windows
                .filter { $0.tabGroup === tabGroup }
                .forEach(append)
        }
        append(sourceWindow)
        sourceWindow.tabbedWindows?.forEach(append)

        return windows.filter(isNativeTerminalTabWindow)
    }

    private func isNativeTerminalTabWindow(_ window: NSWindow) -> Bool {
        window.tabbingIdentifier == tabbingIdentifier
    }

    /// Applies the configured native-tab placement to `window`.
    ///
    /// Hiding the window title is what makes AppKit promote the native tab bar
    /// up into the traffic-light row instead of drawing it as a second strip
    /// below a visible title — that removes the standalone titlebar entirely.
    /// The system tab bar is then shown for `.nativeTabbar` (the pills are the
    /// tab UI) or hidden for `.sidebar` (tabs render in a custom sidebar).
    private func applyNativeTabPlacement(for window: NSWindow) {
        guard isNativeTerminalTabWindow(window) else {
            return
        }
        window.titleVisibility = .hidden
        window.titlebarAppearsTransparent = true

        let titlebarTabsWindow = window as? TitlebarTabsWindow
        switch TermyConfigurationStore.shared.configuration.native.nativeTabPlacement {
        case .nativeTabbar:
            // Merge the native tab bar onto the traffic-light row. Enabling this
            // installs the unified-compact toolbar; AppKit then routes the tab
            // accessory through the window's relocation override.
            titlebarTabsWindow?.titlebarTabs = true
            titlebarTabsWindow?.refreshTitlebarTabsLayout()
        case .sidebar:
            titlebarTabsWindow?.titlebarTabs = false
            if window.tabGroup?.isTabBarVisible == true {
                window.toggleTabBar(nil)
            }
        }
    }

    /// Re-applies native-tab placement to every native terminal window, so a
    /// live `native_tab_placement` config change takes effect immediately.
    func applyNativeTabPlacementToVisibleWindows() {
        for window in NSApp.windows where isNativeTerminalTabWindow(window) {
            applyNativeTabPlacement(for: window)
        }
    }

    @discardableResult
    func applyFocusedTerminalChrome(for window: NSWindow?) -> Bool {
        guard let window,
              isNativeTerminalTabWindow(window),
              let store = TerminalCommandRouter.shared.store(forWindow: window),
              let renderConfig = store.focusedTerminal?.renderConfig
        else {
            return false
        }

        return applyTerminalChrome(
            TerminalWindowChromeState(
                title: store.tabDisplayTitle,
                isFocused: true,
                background: renderConfig.background,
                backgroundOpacity: renderConfig.backgroundOpacity,
                backgroundBlur: renderConfig.backgroundBlur
            ),
            for: window
        )
    }

    @discardableResult
    func applyFocusedTerminalChrome(for store: TerminalWorkspaceStore) -> Bool {
        var didApply = false
        for window in NSApp.windows where isNativeTerminalTabWindow(window) {
            guard TerminalCommandRouter.shared.store(forWindow: window) === store else {
                continue
            }
            didApply = applyFocusedTerminalChrome(for: window) || didApply
        }
        return didApply
    }

    @discardableResult
    func applyTerminalChrome(
        _ state: TerminalWindowChromeState,
        for window: NSWindow,
        requireVisible: Bool = false
    ) -> Bool {
        guard isNativeTerminalTabWindow(window) else {
            return false
        }
        guard !requireVisible || isPresentedNativeTabWindow(window) else {
            return false
        }

        var titleChanged = false
        for target in nativeTabWindows(for: window) {
            // AppKit's native tab group can draw the shared titlebar from the
            // first/group window rather than the selected tab window. Keep the
            // titlebar appearance synchronized across the group, but only update
            // the selected tab's title so inactive tabs keep their own names.
            titleChanged = TerminalWindowChromeApplier.applyFocusedChrome(
                state,
                to: target,
                updatesTitle: target === window
            ) || titleChanged
        }
        if titleChanged {
            postTabsChanged()
        }
        return true
    }

    private func isPresentedNativeTabWindow(_ window: NSWindow) -> Bool {
        if let selectedWindow = window.tabGroup?.selectedWindow {
            return selectedWindow === window
        }
        return window.occlusionState.contains(.visible)
    }

    private func applyFocusedTerminalChromeSoon(for window: NSWindow) {
        Task { @MainActor [weak self, weak window] in
            guard let self, let window else {
                return
            }
            self.applyFocusedTerminalChrome(for: window)
        }
    }

    private func registerWindowLifecycleObservers(for window: NSWindow) {
        let identifier = ObjectIdentifier(window)
        guard windowLifecycleObservers[identifier] == nil else {
            return
        }

        let center = NotificationCenter.default
        let observers = [
            center.addObserver(
                forName: NSWindow.willCloseNotification,
                object: window,
                queue: .main
            ) { [weak self, weak window] _ in
                Task { @MainActor in
                    guard let self, let window else {
                        return
                    }
                    self.handleWindowWillClose(window)
                }
            },
            center.addObserver(
                forName: NSWindow.didBecomeKeyNotification,
                object: window,
                queue: .main
            ) { [weak self, weak window] _ in
                Task { @MainActor in
                    guard let self, let window else {
                        return
                    }
                    self.handleWindowDidBecomeKey(window)
                }
            },
            center.addObserver(
                forName: NSWindow.didChangeOcclusionStateNotification,
                object: window,
                queue: .main
            ) { [weak self, weak window] _ in
                Task { @MainActor in
                    guard let self, let window,
                          let store = TerminalCommandRouter.shared.store(forWindow: window)
                    else {
                        return
                    }
                    self.handleWindowDidChangeOcclusionState(window, store: store)
                }
            }
        ]
        windowLifecycleObservers[identifier] = observers
    }

    private func unregisterWindowLifecycleObservers(for window: NSWindow) {
        let identifier = ObjectIdentifier(window)
        guard let observers = windowLifecycleObservers.removeValue(forKey: identifier) else {
            return
        }
        for observer in observers {
            NotificationCenter.default.removeObserver(observer)
        }
    }

    private func postTabsChanged() {
        NotificationCenter.default.post(name: .termyNativeTabsChanged, object: nil)
    }
}
