import AppKit
import Termy
import TermyNextCore

/// Termy Next always runs its shells in a Termy session host (the multiplexer
/// behind `termy mux`), never in the app: quitting leaves every program
/// running, and the next launch attaches to them again in the same layout.
///
/// The host lives in Termy Next's own directory. Termy desktop adopts every
/// pane of the shared host it doesn't know, so sharing would pull these
/// panes into its windows. `termy mux` still reaches it from inside a pane
/// through `TERMY_MUX_DIR`.
@MainActor
final class SessionHost {
    static let shared = SessionHost()

    private(set) var multiplexer: Multiplexer?
    /// True while the host restarts; panes whose program ends then are
    /// replaced instead of closed.
    private(set) var isRestarting = false
    private var saveScheduled = false

    static var root: URL {
        NextPaths.current.multiplexerDirectory
    }

    private static var layoutURL: URL {
        NextPaths.current.layoutFile
    }

    /// Connects off the main thread (starting a host takes seconds), then
    /// calls `completion` on the main thread.
    func connect(completion: @escaping @MainActor (Result<Void, Error>) -> Void) {
        guard let executable = Multiplexer.bundledHostExecutable else {
            completion(.failure(HostError.missingExecutable))
            return
        }
        let root = Self.root
        DispatchQueue.global(qos: .userInitiated).async {
            let result = Result { () throws -> Multiplexer in
                // The host creates its directory private and refuses one others can read.
                let fm = FileManager.default
                try fm.createDirectory(at: root.deletingLastPathComponent(), withIntermediateDirectories: true)
                if fm.fileExists(atPath: root.path) {
                    try fm.setAttributes([.posixPermissions: 0o700], ofItemAtPath: root.path)
                }
                return try Multiplexer.connect(root: root, hostExecutable: executable)
            }
            DispatchQueue.main.async {
                switch result {
                case .success(let multiplexer):
                    self.multiplexer = multiplexer
                    completion(.success(()))
                case .failure(let error):
                    completion(.failure(error))
                }
            }
        }
    }

    /// Shuts the host down, which ends every program in it, then starts a
    /// new one. Call `finishRestart()` once the panes are replaced.
    func restart(completion: @escaping @MainActor (Result<Void, Error>) -> Void) {
        isRestarting = true
        let old = multiplexer
        multiplexer = nil
        DispatchQueue.global(qos: .userInitiated).async {
            try? old?.shutdown()
            DispatchQueue.main.async {
                self.connect { result in
                    guard case .failure = result else { return completion(result) }
                    // The old host can still hold its socket for a moment.
                    DispatchQueue.main.asyncAfter(deadline: .now() + 1) { self.connect(completion: completion) }
                }
            }
        }
    }

    func finishRestart() {
        isRestarting = false
    }

    enum HostError: LocalizedError {
        case missingExecutable
        case notConnected

        var errorDescription: String? {
            switch self {
            case .missingExecutable: "termy-session-host is missing. Run sdk/swift/scripts/build-libtermy.sh, or rebuild the app bundle."
            case .notConnected: "The session host is not connected."
            }
        }
    }

    func requireMultiplexer() throws -> Multiplexer {
        guard let multiplexer else { throw HostError.notConnected }
        return multiplexer
    }

    /// The program the host started for `paneID`, usually the shell.
    func childPID(for paneID: String?) -> pid_t? {
        guard let paneID, let panes = try? multiplexer?.panes() else { return nil }
        return panes.first { $0.id == paneID }?.childPID
    }

    /// Ends a pane's program in the host.
    func close(paneID: String?) {
        guard let paneID else { return }
        try? multiplexer?.close(paneID: paneID)
    }

    // MARK: - Layout

    struct SavedLayout: Codable {
        var sessions: [SavedSession]
        /// Sessions that had a window, front first.
        var openSessions: [String]
    }

    struct SavedSession: Codable {
        var name: String
        var selectedIndex: Int
        var tabs: [SavedTab]
    }

    struct SavedTab: Codable {
        var customName: String?
        var tree: SplitTree<String>
        var floating: [String: CGRect]
        var focused: String?
        var zoomed: String?
        /// Font zoom per pane id, for panes that aren't at the config's size.
        var fontZoom: [String: Float]?
    }

    /// Saves soon, coalescing bursts of changes.
    func scheduleSave() {
        HostLayoutSync.shared.scheduleWrite()
        guard !saveScheduled else { return }
        saveScheduled = true
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { [weak self] in
            self?.saveScheduled = false
            self?.save()
        }
    }

    func save() {
        guard multiplexer != nil else { return }
        let workspace = Workspace.shared
        let sessions = workspace.sessions.map { session in
            SavedSession(name: session.name, selectedIndex: session.selectedIndex, tabs: session.tabs.compactMap { tab in
                let ids = Dictionary(uniqueKeysWithValues: tab.allPanes.compactMap { pane in pane.paneID.map { (pane.id, $0) } })
                guard let tree = tab.tree.compactMap({ ids[$0] }) else { return nil }
                return SavedTab(
                    customName: tab.customName, tree: tree,
                    floating: Dictionary(uniqueKeysWithValues: tab.floating.compactMap { id, rect in ids[id].map { ($0, rect) } }),
                    focused: ids[tab.focusedID], zoomed: tab.zoomedID.flatMap { ids[$0] },
                    fontZoom: Dictionary(uniqueKeysWithValues: tab.allPanes.compactMap { pane in
                        pane.zoom == 0 ? nil : pane.paneID.map { ($0, pane.zoom) }
                    }))
            })
        }
        let open = AppDelegate.shared.controllersFrontToBack.map(\.session.name)
        let layout = SavedLayout(sessions: sessions, openSessions: open)
        do {
            let data = try JSONEncoder().encode(layout)
            try data.write(to: Self.layoutURL, options: .atomic)
        } catch {
            NSLog("Termy Next: cannot save layout: \(error)")
        }
    }

    /// Closes panes whose program exited while no window showed them.
    func closeExitedPanes() {
        guard let multiplexer, let panes = try? multiplexer.panes() else { return }
        for pane in panes where pane.exited { try? multiplexer.close(paneID: pane.id) }
    }

    /// Live panes the shared layout doesn't list, such as ones from before
    /// it existed, open as tabs in the front window's session.
    func adoptUnlistedPanes() {
        guard let multiplexer, let panes = try? multiplexer.panes() else { return }
        let shown = Set(Workspace.shared.sessions.flatMap(\.tabs).flatMap(\.allPanes).compactMap(\.paneID))
        guard let session = AppDelegate.shared.controllersFrontToBack.first?.session ?? Workspace.shared.sessions.first else { return }
        var adopted = false
        for pane in panes where !pane.exited && !shown.contains(pane.id) {
            guard let view = try? PaneView(attaching: pane.id) else { continue }
            session.append(TerminalTab(pane: view), select: false)
            adopted = true
        }
        if adopted { Workspace.shared.changed() }
    }

    /// Rebuilds the saved sessions from the host's live panes. Panes whose
    /// program exited are closed; live panes the layout doesn't mention come
    /// back in a "recovered" session. Returns the sessions that had windows.
    func restore() -> [TerminalSessionGroup] {
        guard let multiplexer else { return [] }
        let panes = (try? multiplexer.panes()) ?? []
        for pane in panes where pane.exited { try? multiplexer.close(paneID: pane.id) }
        var live = Set(panes.filter { !$0.exited }.map(\.id))
        let saved = (try? Data(contentsOf: Self.layoutURL)).flatMap { try? JSONDecoder().decode(SavedLayout.self, from: $0) }

        var restored: [TerminalSessionGroup] = []
        for savedSession in saved?.sessions ?? [] {
            let session = TerminalSessionGroup(name: savedSession.name)
            for savedTab in savedSession.tabs {
                var views: [String: PaneView] = [:]
                let tree = savedTab.tree.compactMap { paneID -> UUID? in
                    guard live.contains(paneID), let pane = try? PaneView(attaching: paneID) else { return nil }
                    views[paneID] = pane
                    live.remove(paneID)
                    return pane.id
                }
                var floating: [UUID: CGRect] = [:]
                for (paneID, rect) in savedTab.floating where live.contains(paneID) {
                    if let pane = try? PaneView(attaching: paneID) {
                        views[paneID] = pane
                        floating[pane.id] = rect
                        live.remove(paneID)
                    }
                }
                guard let layoutTree = tree ?? floating.keys.first.map({ .leaf($0) }) else { continue }
                floating[layoutTree.leaves.first!] = nil
                let tab = TerminalTab(tree: layoutTree, panes: Array(views.values), floating: floating)
                tab.customName = savedTab.customName
                if let focused = savedTab.focused.flatMap({ views[$0] }) { tab.setFocus(focused.id) }
                tab.zoomedID = savedTab.zoomed.flatMap { views[$0]?.id }
                for (paneID, zoom) in savedTab.fontZoom ?? [:] { views[paneID]?.zoom = zoom }
                session.append(tab, select: false)
            }
            guard !session.tabs.isEmpty else { continue }
            session.selectedIndex = min(savedSession.selectedIndex, session.tabs.count - 1)
            restored.append(session)
        }

        // Live panes nothing points at, such as ones `termy mux create` started.
        let orphans = panes.filter { live.contains($0.id) }
        if !orphans.isEmpty {
            let session = TerminalSessionGroup(name: Naming.uniqued("recovered", taken: Set(restored.map(\.name))))
            for orphan in orphans {
                if let pane = try? PaneView(attaching: orphan.id) { session.append(TerminalTab(pane: pane), select: false) }
            }
            if !session.tabs.isEmpty { restored.append(session) }
        }

        restored.forEach(Workspace.shared.adopt)
        let open = (saved?.openSessions ?? []).compactMap { name in restored.first { $0.name == name } }
        return open.isEmpty ? Array(restored.prefix(1)) : open
    }
}
