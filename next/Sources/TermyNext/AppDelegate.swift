import AppKit
import TermyAppKit

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate, NSMenuItemValidation {
    static var shared: AppDelegate { NSApp.delegate as! AppDelegate }

    private(set) var controllers: [WindowController] = []
    private var keyMonitor: Any?
    private var processTimer: Timer?
    private var spinnerTimer: Timer?

    func applicationDidFinishLaunching(_ notification: Notification) {
        // `swift run` starts an unbundled executable; make it a regular app.
        NSApp.setActivationPolicy(.regular)
        NSApp.mainMenu = buildMainMenu()
        installKeyMonitor()
        PaneNotifier.shared.start()
        processTimer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { _ in
            MainActor.assumeIsolated { AppDelegate.shared.refreshProcesses() }
        }
        spinnerTimer = Timer.scheduledTimer(withTimeInterval: 1.0 / 15, repeats: true) { _ in
            MainActor.assumeIsolated { AppDelegate.shared.controllers.forEach { $0.tick() } }
        }
        NSApp.activate(ignoringOtherApps: true)
        startSessionHost()
    }

    /// Connects to the session host, then reopens the saved sessions or a new one.
    private func startSessionHost() {
        guard Preferences.shared.usesSessionHost else {
            newWindow(directory: ProcessInfo.processInfo.environment["TERMY_NEXT_CWD"] ?? NSHomeDirectory())
            return
        }
        SessionHost.shared.connect { [weak self] result in
            guard let self else { return }
            switch result {
            case .success:
                // The host's shared layout (`termy mux layout`) wins; the
                // app's own saved layout is for a host that has none yet.
                if let multiplexer = SessionHost.shared.multiplexer, let layout = HostLayoutSync.shared.attach(multiplexer) {
                    SessionHost.shared.closeExitedPanes()
                    HostLayoutSync.shared.apply(layout)
                    SessionHost.shared.adoptUnlistedPanes()
                }
                if self.controllers.isEmpty {
                    let open = SessionHost.shared.restore()
                    if open.isEmpty {
                        self.newWindow(directory: ProcessInfo.processInfo.environment["TERMY_NEXT_CWD"] ?? NSHomeDirectory())
                    } else {
                        open.reversed().forEach { self.open($0) }
                    }
                }
                HostLayoutSync.shared.scheduleWrite()
            case .failure(let error):
                // Say why it failed rather than quietly run shells in the app.
                let alert = NSAlert()
                alert.messageText = "Termy Next can't start its session host"
                alert.informativeText = error.localizedDescription
                    + "\n\nWithout it, programs run inside Termy Next and end when you quit. You can turn the session host back on in Settings."
                alert.addButton(withTitle: "Retry")
                alert.addButton(withTitle: "Run Without Session Host")
                alert.addButton(withTitle: "Quit")
                switch alert.runModal() {
                case .alertFirstButtonReturn:
                    self.startSessionHost()
                case .alertSecondButtonReturn:
                    Preferences.shared.usesSessionHost = false
                    self.startSessionHost()
                default:
                    NSApp.terminate(nil)
                }
            }
        }
    }

    /// Our windows, frontmost first.
    var controllersFrontToBack: [WindowController] {
        NSApp.orderedWindows.compactMap { window in controllers.first { $0.window === window } }
    }

    /// Closing the last window keeps sessions alive, like a multiplexer;
    /// clicking the Dock icon brings the most recent one back.
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        if controllers.isEmpty {
            if let session = Workspace.shared.sessionsByRecency.first {
                open(session)
            } else {
                newWindow(directory: NSHomeDirectory())
            }
        }
        return true
    }

    /// Quitting detaches: every program keeps running in the session host,
    /// and the next launch attaches to it again.
    func applicationWillTerminate(_ notification: Notification) {
        HostLayoutSync.shared.isQuitting = true
        HostLayoutSync.shared.writeNow()
        SessionHost.shared.save()
        for session in Workspace.shared.sessions {
            session.tabs.forEach { $0.allPanes.forEach { $0.stop() } }
        }
    }

    // MARK: Windows

    /// The window commands act on: the key window, else the frontmost of ours
    /// (menus can run while the app is in the background).
    var keyController: WindowController? {
        if let key = controllers.first(where: { $0.window?.isKeyWindow == true || $0.window?.isMainWindow == true }) {
            return key
        }
        for window in NSApp.orderedWindows {
            if let controller = controllers.first(where: { $0.window === window }) { return controller }
        }
        return controllers.last
    }

    func newWindow(directory: String?) {
        do {
            open(try Workspace.shared.createSession(directory: directory))
        } catch {
            NSAlert(error: error).runModal()
        }
    }

    @discardableResult
    func open(_ session: TerminalSessionGroup) -> WindowController? {
        let controller = WindowController(session: session)
        controllers.append(controller)
        if let frame = keyController?.window?.frame, controllers.count > 1 {
            controller.window?.setFrame(frame.offsetBy(dx: 28, dy: -28), display: false)
        }
        controller.showWindow(nil)
        controller.window?.makeKeyAndOrderFront(nil)
        return controller
    }

    @discardableResult
    func openWindow(withTab tab: TerminalTab, name: String) -> WindowController? {
        let session = TerminalSessionGroup(name: name)
        session.append(tab)
        Workspace.shared.adopt(session)
        open(session)
        return controllers.last
    }

    /// Takes a pane out of whichever tab holds it, in a window or not.
    func detachPane(_ pane: PaneView) {
        for session in Workspace.shared.sessions {
            guard session.tabs.contains(where: { $0.panes[pane.id] != nil }) else { continue }
            if let controller = controllers.first(where: { $0.session === session }) {
                controller.detachForDrop(pane)
            } else if let tab = session.tabs.first(where: { $0.panes[pane.id] != nil }) {
                tab.remove(pane.id)
                if tab.isEmpty { session.remove(tab) }
                if session.tabs.isEmpty { Workspace.shared.remove(session) }
            }
            return
        }
    }

    /// Takes a tab out of whichever session holds it, in a window or not.
    func detachTab(_ tab: TerminalTab) {
        guard let session = Workspace.shared.sessions.first(where: { $0.tabs.contains { $0 === tab } }) else { return }
        if let controller = controllers.first(where: { $0.session === session }) {
            controller.detachTabForDrop(tab)
        } else {
            session.remove(tab)
            if session.tabs.isEmpty { Workspace.shared.remove(session) }
        }
    }

    /// Opens a tab dropped outside every window in a window of its own.
    func moveTabToNewWindow(_ tab: TerminalTab, at point: NSPoint) {
        // The only tab of a window is already a window of its own: move it.
        if let owner = controllers.first(where: { $0.session.tabs.count == 1 && $0.session.tabs[0] === tab }), let window = owner.window {
            window.setFrameTopLeftPoint(NSPoint(x: point.x - 40, y: point.y + 14))
            return
        }
        detachTab(tab)
        let name = Workspace.shared.newSessionName(near: tab.focusedPane?.directory)
        guard let controller = openWindow(withTab: tab, name: name), let window = controller.window else { return }
        window.setFrameTopLeftPoint(NSPoint(x: point.x - 40, y: point.y + 14))
        tab.focusedPane?.focus()
    }

    /// Opens a pane dropped outside every window in a window of its own,
    /// with its top-left corner near the drop point.
    func movePaneToNewWindow(_ pane: PaneView, at point: NSPoint) {
        // The only pane of the only tab is already a window of its own: move it.
        if let owner = controllers.first(where: { $0.session.tabs.count == 1 && $0.session.tabs[0].panes.count == 1 && $0.session.tabs[0].panes[pane.id] != nil }),
           let window = owner.window {
            window.setFrameTopLeftPoint(NSPoint(x: point.x - 40, y: point.y + 14))
            return
        }
        detachPane(pane)
        pane.isFloating = false
        let name = Workspace.shared.newSessionName(near: pane.directory)
        guard let controller = openWindow(withTab: TerminalTab(pane: pane), name: name), let window = controller.window else { return }
        window.setFrameTopLeftPoint(NSPoint(x: point.x - 40, y: point.y + 14))
        Motion.appear(window.contentView ?? NSView(), scale: 0.96, duration: 0.2)
        pane.focus()
    }

    /// Brings a pane to the front, such as from its notification: its
    /// window, session and tab, opening a window when none shows it.
    func revealPane(_ id: UUID) {
        for session in Workspace.shared.sessions {
            guard let tab = session.tabs.first(where: { $0.panes[id] != nil }), let pane = tab.panes[id] else { continue }
            let controller = controllers.first { $0.session === session } ?? keyController ?? open(session)
            NSApp.activate(ignoringOtherApps: true)
            controller?.window?.makeKeyAndOrderFront(nil)
            controller?.reveal(session: session, tab: tab, pane: pane)
            return
        }
    }

    func windowClosed(_ controller: WindowController) {
        controllers.removeAll { $0 === controller }
    }

    // MARK: Ticks and config

    private func refreshProcesses() {
        for session in Workspace.shared.sessions {
            for tab in session.tabs { tab.allPanes.forEach { $0.refreshProcess() } }
        }
    }

    /// Writes a setting to Termy Next's config and applies it to every pane;
    /// nil removes it. Says why when the file can't be written.
    func applySetting(_ key: String, _ value: String?) {
        do {
            try AppConfig.shared.set(key, to: value)
        } catch {
            NSAlert(error: error).runModal()
        }
        reloadConfig()
    }

    /// Sets the config's font size; zoomed panes keep their zoom on top.
    func setFontSize(_ size: Float) {
        let clamped = max(6, min(72, size))
        applySetting("font_size", clamped.rounded() == clamped ? String(Int(clamped)) : String(format: "%.1f", clamped))
    }

    /// Asks which Termy config to import, defaulting to the Termy app's, then
    /// replaces Termy Next's config with it.
    func importTermyConfig(attachedTo window: NSWindow?) {
        let panel = NSOpenPanel()
        panel.message = "Choose a Termy config to import. Termy Next's current config is kept as config.txt.bak."
        panel.prompt = "Import"
        panel.canChooseDirectories = false
        panel.showsHiddenFiles = true
        let termy = AppConfig.termyConfigURL
        panel.directoryURL = termy.deletingLastPathComponent()
        let finish: (NSApplication.ModalResponse) -> Void = { response in
            guard response == .OK, let url = panel.url else { return }
            do {
                try AppConfig.shared.importConfig(from: url)
                self.reloadConfig()
            } catch {
                NSAlert(error: error).runModal()
            }
        }
        if let window { panel.beginSheetModal(for: window, completionHandler: finish) } else { finish(panel.runModal()) }
    }

    /// Turns the session host on or off for new panes. Open panes stay where they run.
    func setUsesSessionHost(_ on: Bool, completion: (@MainActor (Error?) -> Void)? = nil) {
        guard on, SessionHost.shared.multiplexer == nil else {
            Preferences.shared.usesSessionHost = on
            completion?(nil)
            return
        }
        SessionHost.shared.connect { result in
            switch result {
            case .success:
                if let multiplexer = SessionHost.shared.multiplexer { _ = HostLayoutSync.shared.attach(multiplexer) }
                Preferences.shared.usesSessionHost = true
                completion?(nil)
            case .failure(let error):
                completion?(error)
            }
        }
    }

    /// Restarts the session host while the app stays open: asks first, since
    /// every program ends, then crashes the windows down and gives each pane
    /// a fresh shell in the same folder and the same spot in its layout.
    @objc func restartSessionHost() {
        guard Preferences.shared.usesSessionHost, SessionHost.shared.multiplexer != nil, !SessionHost.shared.isRestarting else { return }
        let alert = NSAlert()
        alert.messageText = "Restart the session host?"
        alert.informativeText = "Every program running in Termy Next ends. Each pane then starts a new shell in the same folder."
        alert.addButton(withTitle: "Restart")
        alert.addButton(withTitle: "Cancel")
        alert.buttons.first?.hasDestructiveAction = true
        guard alert.runModal() == .alertFirstButtonReturn else { return }

        // Note where every hosted pane was before its program ends.
        var hosted: [(TerminalTab, PaneView, String?)] = []
        for session in Workspace.shared.sessions {
            for tab in session.tabs {
                for pane in tab.allPanes where pane.isHosted { hosted.append((tab, pane, pane.directory)) }
            }
        }

        let group = DispatchGroup()
        var outcome: Result<Void, Error> = .success(())
        group.enter()
        SessionHost.shared.restart { result in
            outcome = result
            group.leave()
        }
        for controller in controllers {
            group.enter()
            controller.crashDown { group.leave() }
        }
        group.notify(queue: .main) {
            MainActor.assumeIsolated { self.finishRestart(hosted: hosted, outcome: outcome) }
        }
    }

    private func finishRestart(hosted: [(TerminalTab, PaneView, String?)], outcome: Result<Void, Error>) {
        if case .failure(let error) = outcome {
            // Without a host, new shells run in the app.
            Preferences.shared.usesSessionHost = false
            let alert = NSAlert()
            alert.messageText = "The session host didn't come back"
            alert.informativeText = error.localizedDescription + "\n\nNew panes run inside Termy Next for now. Turn the session host back on in Settings."
            alert.runModal()
        }
        for (tab, old, directory) in hosted {
            old.stop()
            guard let fresh = try? PaneView(workingDirectory: directory ?? NSHomeDirectory()) else {
                tab.remove(old.id)
                continue
            }
            tab.replace(old.id, with: fresh)
        }
        // Tabs that lost every pane go away; sessions keep at least one tab.
        for session in Workspace.shared.sessions {
            for tab in session.tabs where tab.isEmpty { session.remove(tab) }
        }
        if let multiplexer = SessionHost.shared.multiplexer { _ = HostLayoutSync.shared.attach(multiplexer) }
        SessionHost.shared.finishRestart()
        controllers.forEach { $0.showRestartedPanes() }
        Workspace.shared.changed()
    }

    @objc func showSettings() {
        SettingsWindowController.shared.show()
    }

    @objc func reloadConfig() {
        AppConfig.shared.reload()
        NotificationCenter.default.post(name: Preferences.didChange, object: nil)
    }

    @objc func openConfig() {
        AppConfig.shared.reload()
        // Plain text files open in the user's text editor.
        let editor = NSWorkspace.shared.urlForApplication(toOpen: URL(fileURLWithPath: "/tmp/x.txt"))
        if let editor {
            NSWorkspace.shared.open([AppConfig.configURL], withApplicationAt: editor, configuration: NSWorkspace.OpenConfiguration())
        } else {
            NSWorkspace.shared.open(AppConfig.configURL)
        }
    }

    // MARK: Keys

    /// App shortcuts run before the terminal sees the key, so ⌘K opens the
    /// session picker instead of clearing scrollback, and ⌃Tab switches tabs.
    private func installKeyMonitor() {
        keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { event in
            // Local monitors run on the main thread.
            nonisolated(unsafe) let key = event
            let consumed = MainActor.assumeIsolated { AppDelegate.shared.handleKey(key) }
            return consumed ? nil : event
        }
    }

    /// Whether an app shortcut consumed `event`.
    private func handleKey(_ event: NSEvent) -> Bool {
        guard let window = event.window, let controller = controllers.first(where: { $0.window === window }) else { return false }
        let editingText = window.firstResponder is NSTextView
        if var command = Command.matching(event) {
            // ⇧⌘G is Go to Directory, except while finding, where it goes back.
            if command == .goToDirectory, controller.isSearching { command = .findPrevious }
            if editingText, ![.commandPalette, .changeSession, .find, .findNext, .findPrevious].contains(command) { return false }
            controller.run(command)
            return true
        }
        // Rex-style line editing: ⌘← / ⌘→ / ⌘⌫ move to line start, end, delete line.
        if let terminal = window.firstResponder as? TerminalView,
           event.modifierFlags.intersection([.command, .shift, .option, .control]) == .command {
            let bytes: String? = switch event.keyCode {
            case 123: "\u{01}"
            case 124: "\u{05}"
            case 51: "\u{15}"
            default: nil
            }
            if let bytes {
                try? terminal.session.terminal.write(bytes)
                return true
            }
        }
        return false
    }

    // MARK: Menus

    @objc func runCommand(_ sender: NSMenuItem) {
        guard let raw = sender.representedObject as? String, let command = Command(rawValue: raw) else { return }
        if command == .settings { return showSettings() }
        if command == .restartSessionHost { return restartSessionHost() }
        // Close commands close the key window when it isn't a terminal window, such as Settings.
        if [.closePane, .closeTab, .closeWindow].contains(command), let key = NSApp.keyWindow,
           !controllers.contains(where: { $0.window === key }) {
            return key.performClose(nil)
        }
        if let controller = keyController {
            controller.run(command)
        } else if command == .newWindow || command == .newSession || command == .newTab {
            newWindow(directory: NSHomeDirectory())
        }
    }

    func validateMenuItem(_ item: NSMenuItem) -> Bool {
        guard let raw = item.representedObject as? String, let command = Command(rawValue: raw) else { return true }
        let prefs = Preferences.shared
        switch command {
        case .tabsAbove: item.state = prefs.tabStyle == .above ? .on : .off
        case .tabsLeft: item.state = prefs.tabStyle == .left ? .on : .off
        case .tabsRight: item.state = prefs.tabStyle == .right ? .on : .off
        case .paneComfortable: item.state = prefs.paneStyle == .comfortable ? .on : .off
        case .paneCompact: item.state = prefs.paneStyle == .compact ? .on : .off
        case .interfaceLiquid: item.state = prefs.interfaceStyle == .liquid ? .on : .off
        case .interfaceClassic: item.state = prefs.interfaceStyle == .classic ? .on : .off
        case .toggleActivity: item.title = keyController?.isActivityOpen == true ? "Hide Activity" : "Show Activity"
        default: break
        }
        if command == .restartSessionHost {
            return Preferences.shared.usesSessionHost && SessionHost.shared.multiplexer != nil && !SessionHost.shared.isRestarting
        }
        return keyController != nil || [.newWindow, .newSession, .newTab, .settings].contains(command)
    }

    private func buildMainMenu() -> NSMenu {
        let main = NSMenu()
        func submenu(_ title: String, _ items: [NSMenuItem]) -> NSMenuItem {
            let item = NSMenuItem(title: title, action: nil, keyEquivalent: "")
            let menu = NSMenu(title: title)
            items.forEach(menu.addItem)
            item.submenu = menu
            return item
        }
        func c(_ command: Command) -> NSMenuItem { command.menuItem() }
        let sep = { NSMenuItem.separator() }

        let appName = "Termy Next"
        let settings = c(.settings)
        let config = NSMenuItem(title: "Open Config File", action: #selector(openConfig), keyEquivalent: "")
        let reload = NSMenuItem(title: "Reload Config", action: #selector(reloadConfig), keyEquivalent: ",")
        reload.keyEquivalentModifierMask = [.command, .shift]
        main.addItem(submenu(appName, [
            NSMenuItem(title: "About \(appName)", action: #selector(NSApplication.orderFrontStandardAboutPanel(_:)), keyEquivalent: ""),
            sep(), settings, config, reload, c(.restartSessionHost), sep(),
            NSMenuItem(title: "Hide \(appName)", action: #selector(NSApplication.hide(_:)), keyEquivalent: "h"),
            NSMenuItem(title: "Quit \(appName)", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q"),
        ]))
        main.addItem(submenu("File", [
            c(.newWindow), c(.newSession), c(.addRemoteHost), c(.newTab), c(.goToDirectory), sep(),
            c(.closePane), c(.closeTab), c(.closeSession), c(.closeWindow),
        ]))
        main.addItem(submenu("Edit", [
            NSMenuItem(title: "Copy", action: #selector(TerminalView.copy(_:)), keyEquivalent: "c"),
            NSMenuItem(title: "Paste", action: #selector(TerminalView.paste(_:)), keyEquivalent: "v"),
            NSMenuItem(title: "Select All", action: #selector(NSResponder.selectAll(_:)), keyEquivalent: "a"),
            sep(), c(.find), c(.findNext), c(.findPrevious), sep(), c(.insertPath), c(.clearScrollback),
        ]))
        main.addItem(submenu("View", [
            c(.commandPalette), c(.changeSession), c(.renameSession), c(.renameTab), sep(),
            submenu("Pane", [
                c(.splitLeft), c(.splitRight), c(.splitUp), c(.splitDown), c(.splitBestFit), sep(),
                c(.floatPane), c(.zoomPane), c(.equalizePanes), sep(),
                c(.focusLeft), c(.focusRight), c(.focusUp), c(.focusDown), c(.focusPreviousPane), c(.focusNextPane), sep(),
                c(.swapLeft), c(.swapRight), c(.swapUp), c(.swapDown),
            ]),
            submenu("Customize View", [
                submenu("Tab Style", [c(.tabsAbove), c(.tabsLeft), c(.tabsRight)]),
                submenu("Pane Style", [c(.paneComfortable), c(.paneCompact)]),
                submenu("Interface Style", [c(.interfaceClassic), c(.interfaceLiquid)]),
            ]),
            sep(), c(.increaseFontSize), c(.decreaseFontSize), c(.resetFontSize), sep(),
            c(.toggleSidebar), c(.toggleActivity),
        ]))
        let windowMenu = submenu("Window", [
            NSMenuItem(title: "Minimize", action: #selector(NSWindow.performMiniaturize(_:)), keyEquivalent: "m"),
            NSMenuItem(title: "Zoom", action: #selector(NSWindow.performZoom(_:)), keyEquivalent: ""),
            sep(), c(.previousTab), c(.nextTab), c(.moveTabLeft), c(.moveTabRight), c(.moveTabToNewWindow), sep(),
            c(.selectTab1), c(.selectTab2), c(.selectTab3), c(.selectTab4), c(.selectTab5),
            c(.selectTab6), c(.selectTab7), c(.selectTab8), c(.selectLastTab), sep(),
            NSMenuItem(title: "Bring All to Front", action: #selector(NSApplication.arrangeInFront(_:)), keyEquivalent: ""),
        ])
        main.addItem(windowMenu)
        NSApp.windowsMenu = windowMenu.submenu
        main.addItem(submenu("Help", []))
        return main
    }
}
