import AppKit
import SwiftUI
import UniformTypeIdentifiers

struct TerminalWorkspaceView: View {
    @StateObject private var store: TerminalWorkspaceStore
    @ObservedObject private var configurationStore = TermyConfigurationStore.shared
    @State private var tmuxControlModel: TmuxControlWorkspaceModel?
    @State private var appConfigurationError = TermyAppConfiguration.loadErrorMessage
    @State private var tmuxFallbackMessage: String?
    @State private var workspacePersistenceError: String?
    @State private var didRestoreWorkspace = false
    @State private var didConfigurePerformancePanes = false
    @State private var didScheduleBenchmarkExit = false
    @State private var persistenceSaveTask: Task<Void, Never>?
    private let workspacePersistence = TerminalWorkspacePersistence()
    private let shouldRestorePersistedWorkspace: Bool

    init(initialTask: TermyTaskConfiguration? = nil) {
        _store = StateObject(wrappedValue: TerminalWorkspaceStore(initialTask: initialTask))
        _tmuxControlModel = State(initialValue: Self.makeTmuxControlModel(initialTask: initialTask))
        shouldRestorePersistedWorkspace = initialTask == nil
    }

    var body: some View {
        terminalContent
            .background(TerminalWorkspaceRoutingView(
                store: store
            ))
            .focusedValue(\.terminalCommands, commandSet)
            .onAppear {
                TerminalCommandRouter.shared.activate(store)
                configurePerformancePanesIfNeeded()
                scheduleBenchmarkExitIfNeeded()
                if tmuxControlModel == nil {
                    restoreWorkspaceIfNeeded()
                } else {
                    didRestoreWorkspace = true
                }
            }
            .onDisappear {
                if tmuxControlModel == nil {
                    persistWorkspace()
                }
            }
            .onReceive(store.objectWillChange) { _ in
                scheduleWorkspacePersistence()
            }
            .onReceive(NotificationCenter.default.publisher(for: NSApplication.willTerminateNotification)) { _ in
                persistWorkspace()
            }
            .onReceive(NotificationCenter.default.publisher(for: .termyNativeTabsChanged)) { _ in
                NativeTabWindowManager.shared.applyFocusedTerminalChrome(for: store)
            }
            .onReceive(configurationStore.$loadErrorMessage) { message in
                appConfigurationError = message
            }
            .onReceive(configurationStore.$configuration) { _ in
                NativeTabWindowManager.shared.applyNativeTabPlacementToVisibleWindows()
            }
    }

    private func configurePerformancePanesIfNeeded() {
        guard !didConfigurePerformancePanes else {
            return
        }
        didConfigurePerformancePanes = true
        guard let rawCount = ProcessInfo.processInfo.environment["TERMY_PERFORMANCE_PANE_COUNT"],
              let requestedCount = Int(rawCount),
              requestedCount > 1
        else {
            return
        }
        Task { @MainActor in
            for index in 1..<min(requestedCount, 8) {
                store.splitFocused(index.isMultiple(of: 2) ? .vertical : .horizontal)
                try? await Task.sleep(nanoseconds: 100_000_000)
            }
            if let readyPath = ProcessInfo.processInfo.environment["TERMY_PERFORMANCE_PANE_READY_FILE"] {
                try? "\(store.paneCount)\n".write(
                    toFile: readyPath,
                    atomically: true,
                    encoding: .utf8
                )
            }
        }
    }

    private func scheduleBenchmarkExitIfNeeded() {
        guard !didScheduleBenchmarkExit,
              ProcessInfo.processInfo.environment["TERMY_BENCHMARK_EXIT_ON_COMPLETE"] == "1",
              ProcessInfo.processInfo.environment["TERMY_BENCHMARK_COMMAND"] != nil,
              let rawDuration = ProcessInfo.processInfo.environment["TERMY_BENCHMARK_DURATION_SECS"],
              let duration = UInt64(rawDuration),
              duration > 0
        else {
            return
        }
        didScheduleBenchmarkExit = true
        Task { @MainActor in
            // Start-up time is not part of the driver duration. Wait for the
            // benchmark PTY to exit, with a generous fallback for a stuck child.
            let deadline = Date().addingTimeInterval(Double(duration) + 30)
            while store.focusedTerminal?.isExited != true, Date() < deadline {
                try? await Task.sleep(nanoseconds: 100_000_000)
            }
            NSApp.terminate(nil)
        }
    }

    private var terminalContent: some View {
        ZStack {
            if let tmuxControlModel {
                TmuxControlWorkspaceView(model: tmuxControlModel) { errorMessage in
                    handleTmuxControlFailure(errorMessage)
                }
            } else if let zoomedPane = store.zoomedPane {
                TerminalPaneLeafView(pane: zoomedPane, store: store)
            } else {
                TerminalPaneNodeView(node: store.root, store: store)
            }

            if let appConfigurationError {
                dismissibleBanner(appConfigurationError, color: .red) {
                    self.appConfigurationError = nil
                }
            }

            if let tmuxFallbackMessage {
                dismissibleBanner(tmuxFallbackMessage, color: .orange) {
                    self.tmuxFallbackMessage = nil
                }
            }

            if let workspacePersistenceError {
                dismissibleBanner(
                    workspacePersistenceError,
                    color: .orange,
                    actionTitle: "Reset Workspace",
                    onAction: resetWorkspacePersistence
                ) {
                    self.workspacePersistenceError = nil
                }
            }

            if store.isSearchVisible, store.isSearchInputFocused {
                Color.clear
                    .contentShape(Rectangle())
                    .onTapGesture {
                        store.setSearchInputFocused(false)
                    }
                    .zIndex(9)
            }

            if store.isSearchVisible, let terminal = store.focusedTerminal {
                TerminalSearchPanel(
                    terminal: terminal,
                    options: $store.searchOptions,
                    focusRequest: store.searchFocusRequest,
                    onFocusChanged: store.setSearchInputFocused,
                    onClose: store.hideSearch
                )
                .padding(10)
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topTrailing)
                .transition(.move(edge: .bottom).combined(with: .opacity))
                .zIndex(10)
            }

            if store.isCommandPaletteVisible {
                commandPaletteOverlay
                    .zIndex(12)
            }

            TermyToastOverlay()
                .zIndex(20)
        }
    }

    /// A top-leading error banner with a dismiss button, overlaid on the workspace.
    private func dismissibleBanner(
        _ message: String,
        color: Color,
        actionTitle: String? = nil,
        onAction: (() -> Void)? = nil,
        onDismiss: @escaping () -> Void
    ) -> some View {
        HStack(spacing: 8) {
            Text(message)
                .font(.system(size: 12, design: .monospaced))
                .foregroundStyle(color)
            if let actionTitle, let onAction {
                Button(actionTitle, action: onAction)
                    .buttonStyle(.bordered)
                    .controlSize(.small)
            }
            Button(action: onDismiss) {
                Image(systemName: "xmark")
            }
            .buttonStyle(.borderless)
        }
        .padding(8)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 8))
        .padding(10)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .zIndex(11)
    }

    private var commandPaletteOverlay: some View {
        ZStack {
            Color.black.opacity(0.12)
                .ignoresSafeArea()
                .contentShape(Rectangle())
                .onTapGesture {
                    store.hideCommandPalette()
                }

            VStack(spacing: 0) {
                TerminalCommandPalette(
                    commandSet: commandSet,
                    configuration: configurationStore.configuration,
                    onClose: store.hideCommandPalette
                )
                Spacer(minLength: 0)
            }
            .padding(.top, 60)
        }
    }

    private var commandSet: TerminalCommandSet {
        TerminalCommandSet(
            newTab: {
                NativeTabWindowManager.shared.openNativeTab()
            },
            closePaneOrTab: {
                if !store.closeFocusedPaneIfSplit() {
                    NSApp.keyWindow?.performClose(nil)
                }
                scheduleWorkspacePersistence()
            },
            splitRight: {
                store.splitFocused(.horizontal)
                scheduleWorkspacePersistence()
            },
            splitDown: {
                store.splitFocused(.vertical)
                scheduleWorkspacePersistence()
            },
            closePane: {
                store.closeFocusedPane()
                scheduleWorkspacePersistence()
            },
            focusPane: { direction in
                _ = store.focusPane(in: direction)
            },
            focusNextPane: store.focusNextPane,
            focusPreviousPane: store.focusPreviousPane,
            resizePane: { direction in
                if store.resizeFocusedPane(in: direction) {
                    scheduleWorkspacePersistence()
                }
            },
            togglePaneZoom: store.toggleFocusedPaneZoom,
            increaseFontSize: {
                store.focusedTerminal?.increaseFontSize()
            },
            decreaseFontSize: {
                store.focusedTerminal?.decreaseFontSize()
            },
            resetFontSize: {
                store.focusedTerminal?.resetFontSize()
            },
            copy: {
                store.focusedTerminal?.copySelection() ?? false
            },
            paste: {
                guard let text = NSPasteboard.general.string(forType: .string) else {
                    return
                }
                store.focusedTerminal?.paste(text)
            },
            clearScrollback: {
                store.focusedTerminal?.clearScrollback()
            },
            showSearch: store.showSearch,
            hideSearch: store.hideSearch,
            searchNext: {
                store.focusedTerminal?.selectNextSearchMatch()
            },
            searchPrevious: {
                store.focusedTerminal?.selectPreviousSearchMatch()
            },
            toggleSearchCaseSensitive: {
                store.toggleSearchCaseSensitive()
            },
            toggleSearchRegex: {
                store.toggleSearchRegex()
            },
            sendInterrupt: { store.focusedTerminal?.sendControlC() },
            toggleCommandPalette: store.toggleCommandPalette
        )
    }

    private func restoreWorkspaceIfNeeded() {
        guard !didRestoreWorkspace else {
            return
        }
        didRestoreWorkspace = true
        let native = configurationStore.configuration.native
        guard native.nativeTabPersistence || native.nativeLayoutAutosave else {
            workspacePersistenceError = nil
            return
        }
        guard shouldRestorePersistedWorkspace else {
            workspacePersistenceError = nil
            return
        }

        do {
            let snapshot = try native.nativeTabPersistence
                ? workspacePersistence.loadLastSession()
                : workspacePersistence.loadAutosavedLayout()
            if store.restore(from: snapshot) {
                workspacePersistenceError = nil
            }
        } catch TerminalWorkspacePersistenceError.missingLastSession {
            workspacePersistenceError = nil
        } catch {
            TermyNativeLog.lifecycle.error(
                "Workspace restore failed: \(String(reflecting: type(of: error)), privacy: .public)"
            )
            workspacePersistenceError = "Could not restore workspace: \(error)"
        }
    }

    private func scheduleWorkspacePersistence() {
        guard didRestoreWorkspace,
              shouldPersistWorkspace
        else {
            return
        }
        persistenceSaveTask?.cancel()
        persistenceSaveTask = Task {
            try? await Task.sleep(nanoseconds: 250_000_000)
            guard !Task.isCancelled else {
                return
            }
            await MainActor.run {
                persistWorkspace()
            }
        }
    }

    private func persistWorkspace() {
        guard didRestoreWorkspace,
              shouldPersistWorkspace
        else {
            return
        }
        do {
            let native = configurationStore.configuration.native
            let snapshot = store.snapshot(includeBuffers: native.nativeBufferPersistence)
            if native.nativeTabPersistence {
                try workspacePersistence.saveLastSession(snapshot)
            }
            if native.nativeLayoutAutosave {
                try workspacePersistence.saveAutosavedLayout(snapshot)
            }
            workspacePersistenceError = nil
        } catch {
            TermyNativeLog.lifecycle.error(
                "Workspace save failed: \(String(reflecting: type(of: error)), privacy: .public)"
            )
            workspacePersistenceError = "Could not save workspace: \(error)"
        }
    }

    private func resetWorkspacePersistence() {
        do {
            try workspacePersistence.reset()
            TermyNativeLog.lifecycle.notice("Workspace persistence reset")
            workspacePersistenceError = nil
        } catch {
            TermyNativeLog.lifecycle.error(
                "Workspace reset failed: \(String(reflecting: type(of: error)), privacy: .public)"
            )
            workspacePersistenceError = "Could not reset workspace: \(error)"
        }
    }

    private func handleTmuxControlFailure(_ errorMessage: String) {
        if configurationStore.configuration.tmux.exclusive {
            restartExclusiveTmux(reason: errorMessage)
        } else {
            fallBackFromTmux(errorMessage)
        }
    }

    private func restartExclusiveTmux(reason: String) {
        TermyNativeLog.lifecycle.notice(
            "tmux_exclusive restart after control-mode exit: \(reason, privacy: .public)"
        )
        tmuxControlModel?.stop()
        tmuxControlModel = nil
        do {
            let model = try TmuxControlWorkspaceModel()
            tmuxControlModel = model
            tmuxFallbackMessage =
                "tmux control mode exited; restarted because tmux_exclusive is enabled."
        } catch {
            TermyNativeLog.lifecycle.error(
                "tmux_exclusive restart failed: \(String(describing: error), privacy: .public)"
            )
            tmuxFallbackMessage =
                "tmux_exclusive is enabled, but control mode could not restart (\(error))."
        }
    }

    private func fallBackFromTmux(_ errorMessage: String) {
        guard tmuxControlModel != nil else {
            return
        }
        TermyNativeLog.lifecycle.error(
            "Native tmux startup failed; using shell fallback: \(errorMessage, privacy: .public)"
        )
        tmuxControlModel?.stop()
        tmuxControlModel = nil
        didRestoreWorkspace = false
        restoreWorkspaceIfNeeded()
        tmuxFallbackMessage = "Native tmux couldn't start (\(errorMessage)). Using the shell fallback."
    }

    private var shouldPersistWorkspace: Bool {
        guard tmuxControlModel == nil else {
            return false
        }
        let native = configurationStore.configuration.native
        return native.nativeTabPersistence || native.nativeLayoutAutosave
    }

    private static func makeTmuxControlModel(initialTask: TermyTaskConfiguration?) -> TmuxControlWorkspaceModel? {
        guard initialTask == nil,
              TermyConfigurationStore.shared.configuration.tmux.enabled
        else {
            return nil
        }
        return try? TmuxControlWorkspaceModel()
    }

}

private struct TerminalCommandPalette: View {
    let commandSet: TerminalCommandSet
    let configuration: TermyAppConfiguration
    let onClose: () -> Void

    @State private var query = ""
    @State private var selectedIndex = 0
    @FocusState private var isSearchFocused: Bool

    /// Commands matching the query, ranked by fuzzy score (ties keep the
    /// catalog order).
    private var filteredCommands: [(command: PaletteCommand, match: CommandPaletteMatch)] {
        let needle = query.trimmingCharacters(in: .whitespacesAndNewlines)
        let matches = paletteCommands.compactMap { command in
            CommandPaletteFilter.match(query: needle, title: command.title, action: command.action.identifier)
                .map { (command: command, match: $0) }
        }
        guard !needle.isEmpty else {
            return matches
        }
        return matches.enumerated()
            .sorted { lhs, rhs in
                if lhs.element.match.score != rhs.element.match.score {
                    return lhs.element.match.score > rhs.element.match.score
                }
                return lhs.offset < rhs.offset
            }
            .map(\.element)
    }

    var body: some View {
        let filtered = filteredCommands
        let clampedSelection = min(selectedIndex, max(0, filtered.count - 1))

        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Image(systemName: "command")
                    .foregroundStyle(.secondary)
                TextField("Type a command…", text: $query)
                    .textFieldStyle(.plain)
                    .focused($isSearchFocused)
                    .onSubmit {
                        execute(filtered[safe: clampedSelection]?.command)
                    }
                    .onExitCommand {
                        onClose()
                    }
                    .onKeyPress(.downArrow) {
                        selectedIndex = min(clampedSelection + 1, max(0, filtered.count - 1))
                        return .handled
                    }
                    .onKeyPress(.upArrow) {
                        selectedIndex = max(clampedSelection - 1, 0)
                        return .handled
                    }
                Button {
                    onClose()
                } label: {
                    Image(systemName: "xmark")
                }
                .buttonStyle(.borderless)
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 10)

            Divider()

            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(spacing: 1) {
                        ForEach(Array(filtered.enumerated()), id: \.element.command.id) { index, entry in
                            paletteRow(
                                entry.command,
                                match: entry.match,
                                isSelected: index == clampedSelection
                            )
                            .id(entry.command.id)
                            .onHover { hovering in
                                if hovering {
                                    selectedIndex = index
                                }
                            }
                        }

                        if filtered.isEmpty {
                            Text("No matching commands")
                                .foregroundStyle(.secondary)
                                .padding(.vertical, 14)
                        }
                    }
                    .padding(6)
                }
                .frame(maxHeight: 320)
                .onChange(of: clampedSelection) { _, index in
                    guard let id = filtered[safe: index]?.command.id else {
                        return
                    }
                    proxy.scrollTo(id)
                }
            }
        }
        .frame(width: 430)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 10))
        .overlay {
            RoundedRectangle(cornerRadius: 10)
                .stroke(.separator.opacity(0.8), lineWidth: 1)
        }
        .shadow(radius: 18)
        .onChange(of: query) { _, _ in
            selectedIndex = 0
        }
        .onAppear {
            isSearchFocused = true
        }
        .task {
            // The terminal's keyboard view may still hold first responder
            // when the palette mounts; re-assert once the window settles
            // (same pattern as the tab rename sheet).
            try? await Task.sleep(nanoseconds: 10_000_000)
            isSearchFocused = true
        }
    }

    private func paletteRow(
        _ command: PaletteCommand,
        match: CommandPaletteMatch,
        isSelected: Bool
    ) -> some View {
        Button {
            execute(command)
        } label: {
            HStack(spacing: 10) {
                Image(systemName: command.systemImage)
                    .foregroundStyle(isSelected ? Color.accentColor : Color.secondary)
                    .frame(width: 18)

                Text(highlightedTitle(command.title, matchedIndices: match.matchedTitleIndices))
                    .lineLimit(1)

                Spacer()

                if configuration.native.commandPaletteShowKeybinds,
                   let shortcut = shortcutLabel(for: command.action) {
                    Text(shortcut)
                        .font(.caption.monospaced())
                        .foregroundStyle(.secondary)
                }
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 7)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .background(
            isSelected ? Color.accentColor.opacity(0.16) : Color.clear,
            in: RoundedRectangle(cornerRadius: 6)
        )
    }

    private func highlightedTitle(_ title: String, matchedIndices: [Int]) -> AttributedString {
        var attributed = AttributedString(title)
        for offset in matchedIndices {
            guard offset < title.count else {
                continue
            }
            let start = attributed.index(attributed.startIndex, offsetByCharacters: offset)
            let end = attributed.index(start, offsetByCharacters: 1)
            attributed[start..<end].inlinePresentationIntent = .stronglyEmphasized
            attributed[start..<end].foregroundColor = .accentColor
        }
        return attributed
    }

    private func execute(_ command: PaletteCommand?) {
        guard let command else {
            return
        }
        onClose()
        command.execute(commandSet)
    }

    private func shortcutLabel(for action: TerminalKeybindAction) -> String? {
        guard let keybind = configuration.keybinds.first(where: { $0.keybindAction == action }) else {
            return nil
        }
        return keybind.trigger
            .replacingOccurrences(of: "secondary", with: "cmd")
            .replacingOccurrences(of: "cmd", with: "⌘")
            .replacingOccurrences(of: "ctrl", with: "⌃")
            .replacingOccurrences(of: "alt", with: "⌥")
            .replacingOccurrences(of: "shift", with: "⇧")
            .replacingOccurrences(of: "-", with: " ")
    }

    private var paletteCommands: [PaletteCommand] {
        [
            PaletteCommand(title: "New Tab", action: .newTab, systemImage: "plus") { $0.execute(.newTab) },
            PaletteCommand(title: "Switch Tab Left", action: .switchTabLeft, systemImage: "chevron.left") { _ in
                NativeTabWindowManager.shared.selectRelativeNativeTab(offset: -1)
            },
            PaletteCommand(title: "Switch Tab Right", action: .switchTabRight, systemImage: "chevron.right") { _ in
                NativeTabWindowManager.shared.selectRelativeNativeTab(offset: 1)
            },
            PaletteCommand(title: "Move Tab Left", action: .moveTabLeft, systemImage: "arrow.left.to.line") { _ in
                NativeTabWindowManager.shared.moveSelectedNativeTab(offset: -1)
            },
            PaletteCommand(title: "Move Tab Right", action: .moveTabRight, systemImage: "arrow.right.to.line") { _ in
                NativeTabWindowManager.shared.moveSelectedNativeTab(offset: 1)
            },
            PaletteCommand(title: "Split Right", action: .splitPaneVertical, systemImage: "rectangle.split.2x1") { $0.execute(.splitPaneVertical) },
            PaletteCommand(title: "Split Down", action: .splitPaneHorizontal, systemImage: "rectangle.split.1x2") { $0.execute(.splitPaneHorizontal) },
            PaletteCommand(title: "Close Pane or Tab", action: .closePaneOrTab, systemImage: "xmark") { $0.execute(.closePaneOrTab) },
            PaletteCommand(title: "Close Pane", action: .closePane, systemImage: "rectangle.badge.xmark") { $0.execute(.closePane) },
            PaletteCommand(title: "Next Pane", action: .focusPaneNext, systemImage: "arrow.right") { $0.execute(.focusPaneNext) },
            PaletteCommand(title: "Previous Pane", action: .focusPanePrevious, systemImage: "arrow.left") { $0.execute(.focusPanePrevious) },
            PaletteCommand(title: "Toggle Pane Zoom", action: .togglePaneZoom, systemImage: "arrow.up.left.and.arrow.down.right") { $0.execute(.togglePaneZoom) },
            PaletteCommand(title: "Increase Font Size", action: .increaseFontSize, systemImage: "textformat.size.larger") { $0.execute(.increaseFontSize) },
            PaletteCommand(title: "Decrease Font Size", action: .decreaseFontSize, systemImage: "textformat.size.smaller") { $0.execute(.decreaseFontSize) },
            PaletteCommand(title: "Reset Font Size", action: .resetFontSize, systemImage: "textformat") { $0.execute(.resetFontSize) },
            PaletteCommand(title: "Find", action: .openSearch, systemImage: "magnifyingglass") { $0.execute(.openSearch) },
            PaletteCommand(title: "Find Next", action: .searchNext, systemImage: "chevron.down") { $0.execute(.searchNext) },
            PaletteCommand(title: "Find Previous", action: .searchPrevious, systemImage: "chevron.up") { $0.execute(.searchPrevious) },
            PaletteCommand(title: "Toggle Case Sensitive Search", action: .toggleSearchCaseSensitive, systemImage: "textformat") { $0.execute(.toggleSearchCaseSensitive) },
            PaletteCommand(title: "Toggle Regex Search", action: .toggleSearchRegex, systemImage: "asterisk") { $0.execute(.toggleSearchRegex) },
            PaletteCommand(title: "Copy", action: .copy, systemImage: "doc.on.doc") { $0.execute(.copy) },
            PaletteCommand(title: "Paste", action: .paste, systemImage: "doc.on.clipboard") { $0.execute(.paste) },
            PaletteCommand(title: "Clear Scrollback", action: .clearScrollback, systemImage: "trash") { $0.execute(.clearScrollback) },
            PaletteCommand(title: "Send Interrupt", action: .sendInterrupt, systemImage: "exclamationmark.octagon") { $0.execute(.sendInterrupt) },
            PaletteCommand(title: "Open Config", action: .openConfig, systemImage: "doc.text") { _ in
                _ = TermyNativeAppActions.openConfigFileInEditor()
            },
            PaletteCommand(title: "Prettify Config", action: .prettifyConfig, systemImage: "wand.and.stars") { _ in
                _ = TermyNativeAppActions.prettifyConfig()
            },
            PaletteCommand(title: "App Info", action: .appInfo, systemImage: "info.circle") { _ in
                TermyNativeAppActions.showAppInfo()
            },
            PaletteCommand(title: "Restart App", action: .restartApp, systemImage: "arrow.clockwise") { _ in
                TermyNativeAppActions.restartApp()
            },
        ] + configuration.tasks.map { task in
            PaletteCommand(title: "Run \(task.name)", action: .runTask, systemImage: "play") { _ in
                NativeTabWindowManager.shared.openNativeTab(startupTask: task)
            }
        }
    }
}

private struct PaletteCommand: Identifiable {
    let title: String
    let action: TerminalKeybindAction
    let systemImage: String
    let execute: (TerminalCommandSet) -> Void

    /// Stable across renders (the command list is recomputed per body
    /// evaluation) so ForEach identity, selection, and scroll targets hold.
    /// Task commands share the "run_task" action, hence the title suffix.
    var id: String {
        "\(action.identifier):\(title)"
    }
}

private struct TerminalWorkspaceRoutingView: NSViewRepresentable {
    @ObservedObject var store: TerminalWorkspaceStore

    func makeNSView(context: Context) -> RoutingRegistrationView {
        RoutingRegistrationView(store: store)
    }

    func updateNSView(_ view: RoutingRegistrationView, context: Context) {
        view.store = store
        view.registerCurrentWindow()
    }
}

private final class RoutingRegistrationView: NSView {
    weak var store: TerminalWorkspaceStore?
    private weak var registeredWindow: NSWindow?

    init(store: TerminalWorkspaceStore) {
        self.store = store
        super.init(frame: .zero)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        nil
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        registerCurrentWindow()
    }

    func registerCurrentWindow() {
        if let registeredWindow, registeredWindow !== window {
            TerminalCommandRouter.shared.unregister(window: registeredWindow)
            self.registeredWindow = nil
        }

        guard let window, let store else {
            return
        }
        registeredWindow = window
        TerminalCommandRouter.shared.register(store, for: window)
        NativeTabWindowManager.shared.applyFocusedTerminalChrome(for: window)
    }
}

private struct TmuxControlWorkspaceView: View {
    @ObservedObject var model: TmuxControlWorkspaceModel
    @ObservedObject private var configurationStore = TermyConfigurationStore.shared
    let onUnavailable: (String) -> Void

    var body: some View {
        ZStack {
            if let layout = model.layout {
                TmuxControlLayoutView(node: layout, model: model)
            } else {
                Text("Starting tmux…")
                    .font(.system(size: 12, design: .monospaced))
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                    .padding(10)
            }

            if let errorMessage = model.errorMessage {
                dismissibleBanner(errorMessage, color: .red) {
                    model.clearError()
                }
            }

            if model.isSearchVisible, model.isSearchInputFocused {
                Color.clear
                    .contentShape(Rectangle())
                    .onTapGesture {
                        model.setSearchInputFocused(false)
                    }
                    .zIndex(9)
            }

            if model.isSearchVisible, let terminal = model.focusedTerminal {
                TerminalSearchPanel(
                    terminal: terminal,
                    options: $model.searchOptions,
                    focusRequest: model.searchFocusRequest,
                    onFocusChanged: model.setSearchInputFocused,
                    onClose: model.hideSearch
                )
                .padding(10)
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topTrailing)
                .transition(.move(edge: .bottom).combined(with: .opacity))
                .zIndex(10)
            }

            if configurationStore.configuration.native.showDebugOverlay,
               model.focusedTerminal == nil {
                Text("No tmux pane")
                    .font(.system(size: 12, design: .monospaced))
                    .foregroundStyle(.secondary)
                    .padding(8)
                    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topTrailing)
            }
        }
        .onAppear {
            if !model.start() {
                onUnavailable(model.errorMessage ?? "tmux control mode failed to start")
            }
        }
        .onChange(of: model.errorMessage) { _, message in
            guard let message, message == "tmux control session exited" else {
                return
            }
            guard configurationStore.configuration.tmux.exclusive else {
                return
            }
            onUnavailable(message)
        }
        .onDisappear {
            model.stop()
        }
    }

    private func dismissibleBanner(
        _ message: String,
        color: Color,
        onDismiss: @escaping () -> Void
    ) -> some View {
        HStack(spacing: 8) {
            Text(message)
                .font(.system(size: 12, design: .monospaced))
                .foregroundStyle(color)
            Button(action: onDismiss) {
                Image(systemName: "xmark")
            }
            .buttonStyle(.borderless)
        }
        .padding(8)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 8))
        .padding(10)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .zIndex(11)
    }
}

private struct TmuxControlLayoutView: View {
    let node: TmuxLayoutNode
    @ObservedObject var model: TmuxControlWorkspaceModel

    var body: some View {
        switch node {
        case let .pane(id, _, _, _, _):
            TmuxControlPaneView(paneID: id, model: model)
        case let .horizontal(children):
            split(children: children, axis: .horizontal)
        case let .vertical(children):
            split(children: children, axis: .vertical)
        }
    }

    @ViewBuilder
    private func split(children: [TmuxLayoutNode], axis: TerminalSplitAxis) -> some View {
        if children.isEmpty {
            Text("tmux returned an empty layout")
                .font(.system(size: 12, design: .monospaced))
                .foregroundStyle(.red)
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                .padding(10)
        } else if children.count == 1, let only = children.first {
            TmuxControlLayoutView(node: only, model: model)
        } else if let first = children.first {
            let rest = Array(children.dropFirst())
            let restNode: TmuxLayoutNode = axis == .horizontal ? .horizontal(rest) : .vertical(rest)
            StableSplitView(axis: axis, ratio: Self.ratio(first: first, rest: restNode, axis: axis)) {
                TmuxControlLayoutView(node: first, model: model)
            } second: {
                TmuxControlLayoutView(node: restNode, model: model)
            }
        }
    }

    private static func ratio(first: TmuxLayoutNode, rest: TmuxLayoutNode, axis: TerminalSplitAxis) -> Double {
        let firstSize = size(of: first)
        let restSize = size(of: rest)
        let firstLength = axis == .horizontal ? firstSize.width : firstSize.height
        let restLength = axis == .horizontal ? restSize.width : restSize.height
        let total = max(1, firstLength + restLength)
        return min(0.9, max(0.1, Double(firstLength) / Double(total)))
    }

    private static func size(of node: TmuxLayoutNode) -> (width: Int, height: Int) {
        switch node {
        case let .pane(_, width, height, _, _):
            return (max(1, width), max(1, height))
        case let .horizontal(children):
            let sizes = children.map(size)
            return (
                sizes.reduce(0) { $0 + $1.width },
                sizes.map(\.height).max() ?? 1
            )
        case let .vertical(children):
            let sizes = children.map(size)
            return (
                sizes.map(\.width).max() ?? 1,
                sizes.reduce(0) { $0 + $1.height }
            )
        }
    }
}

private struct TmuxControlPaneView: View {
    let paneID: Int
    @ObservedObject var model: TmuxControlWorkspaceModel

    var body: some View {
        if let terminal = model.terminal(forPane: paneID) {
            TerminalSurfaceView(
                terminal: terminal,
                isFocused: model.focusedPaneID == paneID,
                showsFocusBorder: model.paneCount > 1,
                isInputEnabled: !model.isSearchInputFocused,
                isSearchVisible: model.isSearchVisible,
                windowTitle: model.tabDisplayTitle,
                onFocus: {
                    model.focusPane(paneID)
                },
                onSplitRight: {
                    model.focusPane(paneID)
                    model.splitFocused(.horizontal)
                },
                onSplitDown: {
                    model.focusPane(paneID)
                    model.splitFocused(.vertical)
                },
                onClosePane: {
                    model.focusPane(paneID)
                    model.closeFocusedPane()
                },
                onClosePaneIfSplit: {
                    model.focusPane(paneID)
                    return model.closeFocusedPaneIfSplit()
                },
                onFocusNextPane: model.focusNextPane,
                onShowSearch: model.showSearch,
                onDismissSearch: {
                    model.setSearchInputFocused(false)
                },
                sendBytesOverride: { bytes in
                    model.send(bytes: bytes, toPane: paneID)
                },
                sendKeyOverride: { keyInput in
                    model.send(keyInput: keyInput, toPane: paneID)
                },
                sendMouseOverride: { mouseInput in
                    model.send(mouseInput: mouseInput, toPane: paneID)
                },
                pasteOverride: { text in
                    model.paste(text, toPane: paneID)
                }
            )
            .id(paneID)
            .frame(minWidth: 240, minHeight: 120)
        } else {
            Text("tmux pane %\(paneID) is unavailable")
                .font(.system(size: 12, design: .monospaced))
                .foregroundStyle(.red)
                .frame(minWidth: 240, minHeight: 120, alignment: .topLeading)
                .padding(10)
        }
    }
}

private struct TerminalPaneNodeView: View {
    @ObservedObject var node: TerminalPaneNode
    @ObservedObject var store: TerminalWorkspaceStore

    var body: some View {
        switch node.kind {
        case .leaf(let pane):
            TerminalPaneLeafView(pane: pane, store: store)
        case .split(let axis, let first, let second):
            StableSplitView(
                axis: axis,
                ratio: node.splitRatio
            ) {
                TerminalPaneNodeView(node: first, store: store)
            } second: {
                TerminalPaneNodeView(node: second, store: store)
            }
        }
    }
}

private struct StableSplitView<First: View, Second: View>: NSViewControllerRepresentable {
    let axis: TerminalSplitAxis
    let ratio: Double
    let first: First
    let second: Second

    init(
        axis: TerminalSplitAxis,
        ratio: Double,
        @ViewBuilder first: () -> First,
        @ViewBuilder second: () -> Second
    ) {
        self.axis = axis
        self.ratio = ratio
        self.first = first()
        self.second = second()
    }

    func makeNSViewController(context: Context) -> StableSplitViewController<First, Second> {
        StableSplitViewController(
            axis: axis,
            ratio: ratio,
            first: first,
            second: second
        )
    }

    func updateNSViewController(
        _ splitViewController: StableSplitViewController<First, Second>,
        context: Context
    ) {
        splitViewController.update(axis: axis, ratio: ratio, first: first, second: second)
    }
}

private final class StableSplitViewController<First: View, Second: View>: NSSplitViewController {
    private let firstHostingController: NSHostingController<First>
    private let secondHostingController: NSHostingController<Second>
    private var didApplyInitialDividerPosition = false
    private var axis: TerminalSplitAxis
    private var ratio: Double

    init(
        axis: TerminalSplitAxis,
        ratio: Double,
        first: First,
        second: Second
    ) {
        self.axis = axis
        self.ratio = ratio
        firstHostingController = NSHostingController(rootView: first)
        secondHostingController = NSHostingController(rootView: second)
        super.init(nibName: nil, bundle: nil)

        splitView = StableDividerSplitView()
        splitView.dividerStyle = .thin
        splitView.isVertical = axis == .horizontal

        addSplitViewItem(item(for: firstHostingController))
        addSplitViewItem(item(for: secondHostingController))
        updateMinimumThickness()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    func update(axis: TerminalSplitAxis, ratio: Double, first: First, second: Second) {
        firstHostingController.rootView = first
        secondHostingController.rootView = second

        if self.axis != axis {
            self.axis = axis
            splitView.isVertical = axis == .horizontal
            splitView.window?.invalidateCursorRects(for: splitView)
            didApplyInitialDividerPosition = false
            updateMinimumThickness()
        }

        if abs(self.ratio - ratio) > 0.0001 {
            self.ratio = ratio
            didApplyInitialDividerPosition = false
        }
    }

    override func viewDidLayout() {
        super.viewDidLayout()
        applyInitialDividerPositionIfNeeded()
    }

    private func item<Content: View>(for hostingController: NSHostingController<Content>) -> NSSplitViewItem {
        let item = NSSplitViewItem(viewController: hostingController)
        item.canCollapse = false
        item.holdingPriority = .defaultLow
        return item
    }

    private func updateMinimumThickness() {
        let minimum: CGFloat = axis == .horizontal ? 220 : 120
        splitViewItems.forEach { item in
            item.minimumThickness = minimum
        }
    }

    private func applyInitialDividerPositionIfNeeded() {
        guard !didApplyInitialDividerPosition else {
            return
        }

        let length = splitView.isVertical ? splitView.bounds.width : splitView.bounds.height
        guard length > 0 else {
            return
        }

        splitView.setPosition(length * ratio, ofDividerAt: 0)
        didApplyInitialDividerPosition = true
    }

    // Panes are intentionally NOT suspended while the divider drags or the
    // window resizes: every resize step re-grids and re-presents the affected
    // terminals live, so content tracks the divider/window edge instead of
    // freezing on the stale grid and snapping after the drag ends. Per-step
    // cost is bounded — `TerminalViewModel.resize` only refreshes when the
    // col/row count actually changes, and the refresh itself is throttled.
    // Occluded background tabs are still suspended via the window occlusion
    // path in TermySwiftApp.
    override func splitViewDidResizeSubviews(_ notification: Notification) {
        splitView.window?.invalidateCursorRects(for: splitView)
    }

    override func splitView(
        _ splitView: NSSplitView,
        effectiveRect proposedEffectiveRect: NSRect,
        forDrawnRect drawnRect: NSRect,
        ofDividerAt dividerIndex: Int
    ) -> NSRect {
        guard let splitView = splitView as? StableDividerSplitView else {
            return proposedEffectiveRect
        }
        return splitView.expandedDividerRect(forDrawnRect: drawnRect)
    }
}

private final class StableDividerSplitView: NSSplitView {
    private static let dividerHitThickness: CGFloat = 12

    override func resetCursorRects() {
        super.resetCursorRects()

        for dividerIndex in 0..<max(0, arrangedSubviews.count - 1) {
            let rect = expandedDividerRect(ofDividerAt: dividerIndex)
            addCursorRect(rect, cursor: resizeCursor)
        }
    }

    override func hitTest(_ point: NSPoint) -> NSView? {
        for dividerIndex in 0..<max(0, arrangedSubviews.count - 1) {
            if expandedDividerRect(ofDividerAt: dividerIndex).contains(point) {
                return self
            }
        }
        return super.hitTest(point)
    }

    override func mouseMoved(with event: NSEvent) {
        if isEventInsideExpandedDivider(event) {
            resizeCursor.set()
            return
        }
        super.mouseMoved(with: event)
    }

    override func mouseDown(with event: NSEvent) {
        if isEventInsideExpandedDivider(event) {
            resizeCursor.set()
        }
        super.mouseDown(with: event)
    }

    override var isVertical: Bool {
        didSet {
            window?.invalidateCursorRects(for: self)
        }
    }

    private var resizeCursor: NSCursor {
        isVertical ? .resizeLeftRight : .resizeUpDown
    }

    private func isEventInsideExpandedDivider(_ event: NSEvent) -> Bool {
        let point = convert(event.locationInWindow, from: nil)
        for dividerIndex in 0..<max(0, arrangedSubviews.count - 1) {
            if expandedDividerRect(ofDividerAt: dividerIndex).contains(point) {
                return true
            }
        }
        return false
    }

    func expandedDividerRect(forDrawnRect drawnRect: NSRect) -> NSRect {
        expandedDividerRect(expanding: drawnRect)
    }

    private func expandedDividerRect(ofDividerAt dividerIndex: Int) -> NSRect {
        guard dividerIndex >= 0, dividerIndex + 1 < arrangedSubviews.count else {
            return .zero
        }

        let leadingFrame = arrangedSubviews[dividerIndex].frame
        let trailingFrame = arrangedSubviews[dividerIndex + 1].frame
        let targetThickness = Self.dividerHitThickness
        var rect: NSRect

        if isVertical {
            let centerX = (leadingFrame.maxX + trailingFrame.minX) / 2
            rect = NSRect(
                x: centerX - (targetThickness / 2),
                y: bounds.minY,
                width: targetThickness,
                height: bounds.height
            )
        } else {
            let centerY = (leadingFrame.maxY + trailingFrame.minY) / 2
            rect = NSRect(
                x: bounds.minX,
                y: centerY - (targetThickness / 2),
                width: bounds.width,
                height: targetThickness
            )
        }

        return expandedDividerRect(expanding: rect)
    }

    private func expandedDividerRect(expanding rect: NSRect) -> NSRect {
        var rect = rect
        let targetThickness = Self.dividerHitThickness

        if isVertical {
            let delta = max(0, targetThickness - rect.width)
            rect.origin.x -= delta / 2
            rect.size.width += delta
        } else {
            let delta = max(0, targetThickness - rect.height)
            rect.origin.y -= delta / 2
            rect.size.height += delta
        }

        return rect.intersection(bounds)
    }
}

private struct TerminalPaneLeafView: View {
    @ObservedObject var pane: TerminalPane
    @ObservedObject var store: TerminalWorkspaceStore
    @ObservedObject private var configurationStore = TermyConfigurationStore.shared

    @State private var isDropTargeted = false
    @State private var activeDropPlacement: TerminalPaneDropPlacement?

    var body: some View {
        GeometryReader { proxy in
            ZStack(alignment: .top) {
                TerminalSurfaceView(
                    terminal: pane.terminal,
                    isFocused: store.focusedPaneID == pane.id,
                    showsFocusBorder: store.paneCount > 1,
                    // While an overlay owns the keyboard, the terminal must not
                    // re-steal first responder (updateNSView refocuses on every
                    // frame tick, which makes overlay text fields untypable).
                    isInputEnabled: !store.isSearchInputFocused && !store.isCommandPaletteVisible,
                    isSearchVisible: store.isSearchVisible,
                    windowTitle: store.tabDisplayTitle,
                    onFocus: {
                        store.focus(pane)
                    },
                    onSplitRight: {
                        store.focus(pane)
                        store.splitFocused(.horizontal)
                    },
                    onSplitDown: {
                        store.focus(pane)
                        store.splitFocused(.vertical)
                    },
                    onClosePane: {
                        store.focus(pane)
                        store.closeFocusedPane()
                    },
                    onClosePaneIfSplit: {
                        store.focus(pane)
                        return store.closeFocusedPaneIfSplit()
                    },
                    onFocusNextPane: store.focusNextPane,
                    onShowSearch: store.showSearch,
                    onDismissSearch: {
                        store.setSearchInputFocused(false)
                    }
                )

                // While any pane is being dragged, show the four drop bands on
                // every other pane so the available placements are visible; the
                // pane under the cursor highlights its nearest band.
                if let draggingID = store.draggingPaneID, draggingID != pane.id {
                    TerminalPaneDropPlacementOverlay(
                        activePlacement: isDropTargeted ? activeDropPlacement : nil
                    )
                    .transition(.opacity)
                    .allowsHitTesting(false)
                }

                if dragEnabled {
                    TerminalPaneDragHandle(
                        isSuppressed: store.draggingPaneID != nil,
                        onBegin: {
                            store.focus(pane)
                            store.beginDraggingPane(pane.id)
                            return TerminalPaneDragPayload.pasteboardItem(for: pane.id)
                        },
                        onEnd: {
                            store.endDraggingPane()
                        }
                    )
                    .padding(.top, 5)
                }

                if configurationStore.configuration.native.showDebugOverlay {
                    TerminalDebugOverlay(metrics: pane.terminal.debugMetrics)
                        .padding(8)
                        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topTrailing)
                        .allowsHitTesting(false)
                }
            }
            .animation(.easeOut(duration: 0.12), value: store.draggingPaneID)
            .onDrop(
                of: [UTType.termyPaneDrag],
                delegate: TerminalPaneDropDelegate(
                    targetPaneID: pane.id,
                    store: store,
                    paneSize: proxy.size,
                    isDropTargeted: $isDropTargeted,
                    activePlacement: $activeDropPlacement
                )
            )
        }
        .id(pane.id)
        .frame(minWidth: 240, minHeight: 120)
    }

    private var dragEnabled: Bool {
        store.paneCount > 1 && store.zoomedPane == nil
    }
}

private extension UTType {
    static let termyPaneDrag = UTType(exportedAs: "com.lassevestergaard.termy.pane-drag")
}

private enum TerminalPaneDragPayload {
    /// The drag carries the pane id under our custom UTType so SwiftUI's
    /// `.onDrop` validates it. `NSPasteboardItem` (unlike `NSItemProvider`)
    /// conforms to `NSPasteboardWriting`, so it can back an `NSDraggingItem`.
    static func pasteboardItem(for paneID: UUID) -> NSPasteboardItem {
        let item = NSPasteboardItem()
        item.setData(
            Data(paneID.uuidString.utf8),
            forType: NSPasteboard.PasteboardType(UTType.termyPaneDrag.identifier)
        )
        return item
    }
}

private struct TerminalPaneDropDelegate: DropDelegate {
    let targetPaneID: UUID
    let store: TerminalWorkspaceStore
    let paneSize: CGSize
    @Binding var isDropTargeted: Bool
    @Binding var activePlacement: TerminalPaneDropPlacement?

    func validateDrop(info: DropInfo) -> Bool {
        store.draggingPaneID != nil
            && store.draggingPaneID != targetPaneID
            && !info.itemProviders(for: [UTType.termyPaneDrag]).isEmpty
    }

    func dropEntered(info: DropInfo) {
        updatePlacement(for: info)
    }

    func dropUpdated(info: DropInfo) -> DropProposal? {
        updatePlacement(for: info)
        return DropProposal(operation: validateDrop(info: info) ? .move : .forbidden)
    }

    func dropExited(info: DropInfo) {
        clearPlacement()
    }

    func performDrop(info: DropInfo) -> Bool {
        defer {
            clearPlacement()
            store.endDraggingPane()
        }

        guard validateDrop(info: info),
              let sourcePaneID = store.draggingPaneID,
              let placement = activePlacement ?? Self.placement(at: info.location, in: paneSize)
        else {
            return false
        }

        return store.movePane(sourcePaneID, to: targetPaneID, placement: placement)
    }

    private func updatePlacement(for info: DropInfo) {
        guard validateDrop(info: info) else {
            clearPlacement()
            return
        }
        isDropTargeted = true
        activePlacement = Self.placement(at: info.location, in: paneSize)
    }

    private func clearPlacement() {
        isDropTargeted = false
        activePlacement = nil
    }

    private static func placement(
        at location: CGPoint,
        in size: CGSize
    ) -> TerminalPaneDropPlacement? {
        guard size.width > 0, size.height > 0 else {
            return nil
        }

        let x = min(max(location.x, 0), size.width) / size.width
        let y = min(max(location.y, 0), size.height) / size.height
        let candidates: [(TerminalPaneDropPlacement, CGFloat)] = [
            (.left, x),
            (.right, 1 - x),
            (.top, y),
            (.bottom, 1 - y)
        ]
        return candidates.min { lhs, rhs in lhs.1 < rhs.1 }?.0
    }
}

private struct TerminalPaneDragHandle: View {
    let isSuppressed: Bool
    let onBegin: () -> NSPasteboardItem
    let onEnd: () -> Void

    @State private var isHovered = false

    private var isVisible: Bool {
        isHovered && !isSuppressed
    }

    var body: some View {
        dots(opacity: 0.86)
            .frame(width: 34, height: 18)
            .opacity(isVisible ? 1 : 0)
            .scaleEffect(isVisible ? 1 : 0.94)
            .animation(.easeOut(duration: 0.1), value: isVisible)
            // A transparent AppKit view sits on top to own hit-testing, the grab
            // cursor, and the drag itself — SwiftUI's `.onDrag`/`.onHover` never
            // fire here because the terminal surface NSView consumes the mouse.
            .overlay(
                PaneDragSourceRepresentable(
                    isEnabled: !isSuppressed,
                    onHoverChange: { isHovered = $0 },
                    onBegin: onBegin,
                    onEnd: onEnd
                )
            )
            .accessibilityLabel("Move pane")
            .help("Move Pane")
    }

    private func dots(opacity: Double) -> some View {
        HStack(spacing: 3) {
            ForEach(0..<3, id: \.self) { _ in
                Circle()
                    .fill(Color.secondary.opacity(opacity))
                    .frame(width: 3, height: 3)
            }
        }
    }
}

private struct PaneDragSourceRepresentable: NSViewRepresentable {
    let isEnabled: Bool
    let onHoverChange: (Bool) -> Void
    let onBegin: () -> NSPasteboardItem
    let onEnd: () -> Void

    func makeNSView(context: Context) -> PaneDragSourceView {
        let view = PaneDragSourceView()
        apply(to: view)
        return view
    }

    func updateNSView(_ view: PaneDragSourceView, context: Context) {
        apply(to: view)
        if !isEnabled {
            view.onHoverChange(false)
        }
    }

    private func apply(to view: PaneDragSourceView) {
        view.onHoverChange = onHoverChange
        view.onBegin = onBegin
        view.onEnd = onEnd
        view.isEnabledForDrag = isEnabled
    }
}

/// Transparent overlay that starts a real AppKit drag session for a pane. Using
/// an NSView (rather than SwiftUI `.onDrag`) is required: the terminal surface
/// view consumes `mouseDown` before SwiftUI gestures see it. Being the topmost
/// sibling in the ZStack, this view wins hit-testing for its small footprint.
final class PaneDragSourceView: NSView, NSDraggingSource {
    var isEnabledForDrag = true
    var onHoverChange: (Bool) -> Void = { _ in }
    var onBegin: () -> NSPasteboardItem = { NSPasteboardItem() }
    var onEnd: () -> Void = {}

    override var isFlipped: Bool { true }

    // Allow grabbing the handle even when the window isn't key yet.
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    override func hitTest(_ point: NSPoint) -> NSView? {
        guard isEnabledForDrag, !isHidden else { return nil }
        return super.hitTest(point)
    }

    // `cursorUpdate` runs in the window's cursor-management pass, which fires
    // after the terminal view's per-move `NSCursor.set()`, so the open hand wins.
    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        for area in trackingAreas {
            removeTrackingArea(area)
        }
        addTrackingArea(NSTrackingArea(
            rect: .zero,
            options: [.activeInActiveApp, .inVisibleRect, .mouseEnteredAndExited, .cursorUpdate],
            owner: self
        ))
    }

    override func mouseEntered(with event: NSEvent) {
        onHoverChange(true)
    }

    override func mouseExited(with event: NSEvent) {
        onHoverChange(false)
    }

    override func cursorUpdate(with event: NSEvent) {
        NSCursor.openHand.set()
    }

    override func resetCursorRects() {
        addCursorRect(bounds, cursor: .openHand)
    }

    override func mouseDown(with event: NSEvent) {
        guard isEnabledForDrag else {
            super.mouseDown(with: event)
            return
        }

        // `onBegin` focuses the pane, marks it dragging, and returns the pasteboard
        // item carrying the pane UTType that the SwiftUI `.onDrop` side validates.
        let item = NSDraggingItem(pasteboardWriter: onBegin())
        let image = Self.dotsImage()
        let local = convert(event.locationInWindow, from: nil)
        item.setDraggingFrame(
            NSRect(
                x: local.x - image.size.width / 2,
                y: local.y - image.size.height / 2,
                width: image.size.width,
                height: image.size.height
            ),
            contents: image
        )
        NSCursor.closedHand.set()
        beginDraggingSession(with: [item], event: event, source: self)
    }

    func draggingSession(
        _ session: NSDraggingSession,
        sourceOperationMaskFor context: NSDraggingContext
    ) -> NSDragOperation {
        .move
    }

    func draggingSession(_ session: NSDraggingSession, willBeginAt screenPoint: NSPoint) {
        NSCursor.closedHand.set()
    }

    // Fires on drop, cancel, and drop-outside-any-pane — the canonical teardown,
    // so the placement overlay always clears even if `performDrop` never runs.
    func draggingSession(
        _ session: NSDraggingSession,
        endedAt screenPoint: NSPoint,
        operation: NSDragOperation
    ) {
        NSCursor.openHand.set()
        onEnd()
    }

    private static func dotsImage() -> NSImage {
        let size = NSSize(width: 26, height: 12)
        return NSImage(size: size, flipped: false) { _ in
            NSColor.secondaryLabelColor.setFill()
            for index in 0..<3 {
                let x = CGFloat(index) * 9 + 4
                NSBezierPath(ovalIn: NSRect(x: x, y: 4.5, width: 3, height: 3)).fill()
            }
            return true
        }
    }
}

private struct TerminalPaneDropPlacementOverlay: View {
    let activePlacement: TerminalPaneDropPlacement?

    var body: some View {
        GeometryReader { proxy in
            ZStack {
                // A thin outline marks every pane that can receive the drop.
                RoundedRectangle(cornerRadius: 8)
                    .strokeBorder(
                        Color.accentColor.opacity(activePlacement == nil ? 0.3 : 0.55),
                        lineWidth: 1
                    )

                // On the pane under the cursor, fill the half the dragged pane
                // will occupy so the resulting split is obvious.
                if let activePlacement {
                    RoundedRectangle(cornerRadius: 6)
                        .fill(Color.accentColor.opacity(0.22))
                        .overlay {
                            RoundedRectangle(cornerRadius: 6)
                                .strokeBorder(Color.accentColor.opacity(0.85), lineWidth: 1.5)
                        }
                        .frame(
                            width: activePlacement.splitsHorizontally ? proxy.size.width / 2 : nil,
                            height: activePlacement.splitsVertically ? proxy.size.height / 2 : nil
                        )
                        .frame(
                            maxWidth: .infinity,
                            maxHeight: .infinity,
                            alignment: activePlacement.overlayAlignment
                        )
                        .transition(.opacity)
                }
            }
            .padding(5)
            .animation(.easeOut(duration: 0.1), value: activePlacement)
        }
    }
}

private extension TerminalPaneDropPlacement {
    var overlayAlignment: Alignment {
        switch self {
        case .left:
            return .leading
        case .right:
            return .trailing
        case .top:
            return .top
        case .bottom:
            return .bottom
        }
    }

    var splitsHorizontally: Bool {
        self == .left || self == .right
    }

    var splitsVertically: Bool {
        self == .top || self == .bottom
    }
}

private struct TerminalDebugOverlay: View {
    let metrics: TerminalDebugMetrics

    var body: some View {
        VStack(alignment: .trailing, spacing: 2) {
            Text("\(metrics.framesPerSecond, specifier: "%.0f") FPS")
            Text("\(metrics.cpuPercent, specifier: "%.0f")% CPU")
            Text("\(metrics.memoryMegabytes, specifier: "%.0f") MB")
            Text("\(metrics.skippedPresentsPerSecond, specifier: "%.0f") skip")
            Text("\(metrics.fullRebuildsPerSecond, specifier: "%.0f")/\(metrics.partialRebuildsPerSecond, specifier: "%.0f") full/part")
        }
        .font(.system(size: 11, weight: .medium, design: .monospaced))
        .foregroundStyle(.primary)
        .padding(.horizontal, 7)
        .padding(.vertical, 5)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 6))
        .overlay {
            RoundedRectangle(cornerRadius: 6)
                .stroke(.separator.opacity(0.6), lineWidth: 1)
        }
    }
}

private struct TerminalSearchPanel: View {
    @ObservedObject var terminal: TerminalViewModel
    @Binding var options: TerminalSearchOptions
    let focusRequest: Int
    let onFocusChanged: (Bool) -> Void
    let onClose: () -> Void

    @State private var query = ""
    @FocusState private var isFieldFocused: Bool

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: "magnifyingglass")
                .foregroundStyle(.secondary)

            TextField("Search", text: $query)
                .textFieldStyle(.plain)
                .frame(width: 220)
                .focused($isFieldFocused)
                .onSubmit {
                    terminal.selectNextSearchMatch()
                }
                .onExitCommand {
                    onClose()
                }

            Text(matchSummary)
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
                .frame(width: 64, alignment: .trailing)

            Button {
                terminal.selectPreviousSearchMatch()
            } label: {
                Image(systemName: "chevron.up")
            }
            .buttonStyle(.borderless)
            .disabled(terminal.searchMatches.isEmpty)

            Button {
                terminal.selectNextSearchMatch()
            } label: {
                Image(systemName: "chevron.down")
            }
            .buttonStyle(.borderless)
            .disabled(terminal.searchMatches.isEmpty)

            Button {
                options.caseSensitive.toggle()
            } label: {
                Text("Aa")
                    .font(.caption.weight(.semibold))
            }
            .buttonStyle(.borderless)
            .help("Case Sensitive")
            .foregroundStyle(options.caseSensitive ? Color.accentColor : Color.secondary)

            Button {
                options.usesRegex.toggle()
            } label: {
                Text(".*")
                    .font(.caption.monospaced().weight(.semibold))
            }
            .buttonStyle(.borderless)
            .help("Regex")
            .foregroundStyle(options.usesRegex ? Color.accentColor : Color.secondary)

            Button {
                onClose()
            } label: {
                Image(systemName: "xmark")
            }
            .buttonStyle(.borderless)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 7)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 8))
        .overlay {
            RoundedRectangle(cornerRadius: 8)
                .stroke(.separator.opacity(0.8), lineWidth: 1)
        }
        .onAppear {
            focusSearchField()
            terminal.updateSearch(query, options: options)
        }
        .onChange(of: focusRequest) { _, _ in
            focusSearchField()
        }
        .onChange(of: isFieldFocused) { _, isFocused in
            onFocusChanged(isFocused)
        }
        .onChange(of: query) { _, value in
            terminal.updateSearch(value, options: options)
        }
        .onChange(of: options) { _, value in
            terminal.updateSearch(query, options: value)
        }
        .onDisappear {
            onFocusChanged(false)
        }
    }

    private var matchSummary: String {
        guard !query.isEmpty else {
            return "0/0"
        }
        guard !terminal.searchMatches.isEmpty else {
            return "0/0"
        }
        return "\(terminal.activeSearchMatchIndex + 1)/\(terminal.searchMatches.count)"
    }

    private func focusSearchField() {
        onFocusChanged(true)
        isFieldFocused = true
        Task { @MainActor in
            try? await Task.sleep(nanoseconds: 10_000_000)
            onFocusChanged(true)
            isFieldFocused = true
        }
    }
}
