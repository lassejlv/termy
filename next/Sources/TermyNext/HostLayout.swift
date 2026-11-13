import AppKit
import Termy
import TermyNextCore

/// Keeps Termy Next's windows, sessions, tabs and splits in step with the
/// layout the session host shares with `termy mux`, so the CLI sees and edits
/// what is on screen:
///
/// - each Termy Next window is a host window (`termy mux layout`),
/// - each session is a workspace of the window that shows it (sessions shown
///   nowhere belong to the window they were last in, else the front one),
/// - tabs and their split trees, titles, focus, zoom and font zoom match.
///
/// Changes made here are written to the host; `termy mux create`, `split`,
/// `tab`, `window` and `workspace` changes arrive through a watch and are
/// applied live. A live pane is never dropped because a layout misses it:
/// one that isn't listed yet (just split here, say) stays where it is.
@MainActor
final class HostLayoutSync {
    static let shared = HostLayoutSync()

    /// The host revision this app last wrote or applied.
    private(set) var revision: UInt64 = 0
    /// Set while quitting, so windows going away aren't written as closed.
    var isQuitting = false
    private var watch: LayoutWatch?
    private var applying = false
    private var writeScheduled = false
    /// Workspaces without tabs that the host lists (made with `termy mux
    /// window … create`), kept on write since nothing here shows them.
    private var emptyWorkspaces: [String: [MultiplexerLayout.Workspace]] = [:]
    private let queue = DispatchQueue(label: "sh.termy.next.layout")

    /// Frames of floating panes by pane ID. The host's layout has no floating
    /// panes, so they are stored there as splits and float again from this.
    private var floatingFrames: [String: CGRect] {
        get {
            let raw = UserDefaults.standard.dictionary(forKey: "floatingPanes") as? [String: [Double]] ?? [:]
            return raw.compactMapValues { $0.count == 4 ? CGRect(x: $0[0], y: $0[1], width: $0[2], height: $0[3]) : nil }
        }
        set {
            UserDefaults.standard.set(newValue.mapValues { [$0.minX, $0.minY, $0.width, $0.height] }, forKey: "floatingPanes")
        }
    }

    // MARK: Connecting

    /// Called once the host is connected: reads its layout and starts
    /// watching. Returns the layout when it lists windows, for the caller to
    /// open; nil means the host has none yet.
    func attach(_ multiplexer: Multiplexer) -> MultiplexerLayout? {
        watch = nil
        revision = 0
        let layout = try? multiplexer.layout()
        revision = layout?.revision ?? 0
        watch = try? multiplexer.watchLayout { revision in
            DispatchQueue.main.async {
                MainActor.assumeIsolated { HostLayoutSync.shared.hostChanged(revision) }
            }
        }
        return layout.flatMap { $0.windows.isEmpty ? nil : $0 }
    }

    private func hostChanged(_ newRevision: UInt64?) {
        guard let newRevision, newRevision != revision, !SessionHost.shared.isRestarting else { return }
        fetchAndApply()
    }

    private func fetchAndApply() {
        guard let multiplexer = SessionHost.shared.multiplexer else { return }
        queue.async {
            let layout = try? multiplexer.layout()
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    guard let layout, layout.revision >= HostLayoutSync.shared.revision else { return }
                    HostLayoutSync.shared.apply(layout)
                }
            }
        }
    }

    // MARK: Writing

    /// Writes the layout soon, coalescing bursts of changes.
    func scheduleWrite() {
        guard !applying, !isQuitting, SessionHost.shared.multiplexer != nil, !writeScheduled else { return }
        writeScheduled = true
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.1) { [weak self] in
            self?.writeScheduled = false
            self?.write()
        }
    }

    /// A write is on its way to the host; the revision it answers with is
    /// the one the next write must carry.
    private var writing = false
    private var writeAgain = false
    /// The write on its way, which quitting waits for (the main thread is
    /// blocked then, so its answer can't arrive the usual way).
    private var inFlight: PendingWrite?

    private final class PendingWrite: @unchecked Sendable {
        let answered = DispatchGroup()
        private let lock = NSLock()
        private var value: (updated: Bool, revision: UInt64)?
        var answer: (updated: Bool, revision: UInt64)? {
            get { lock.withLock { value } }
            set { lock.withLock { value = newValue } }
        }
    }

    private func write() {
        // A second write sent before the first answers would carry the old
        // revision, be refused, and fetching the host's layout would then
        // undo the change it was meant to save.
        guard !writing else { writeAgain = true; return }
        guard !SessionHost.shared.isRestarting, let multiplexer = SessionHost.shared.multiplexer, var layout = snapshot() else { return }
        layout.revision = revision
        writing = true
        let pending = PendingWrite()
        pending.answered.enter()
        inFlight = pending
        queue.async {
            let result = try? multiplexer.setLayout(layout)
            pending.answer = result
            pending.answered.leave()
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    let sync = HostLayoutSync.shared
                    sync.writing = false
                    if sync.inFlight === pending { sync.inFlight = nil }
                    if sync.writeAgain {
                        sync.writeAgain = false
                        sync.scheduleWrite()
                    }
                    guard let result else { return }
                    if result.updated {
                        HostLayoutSync.shared.revision = max(HostLayoutSync.shared.revision, result.revision)
                    } else {
                        // Another client wrote first: take its layout, then write ours on top.
                        HostLayoutSync.shared.fetchAndApply()
                    }
                }
            }
        }
    }

    /// The last write, made while quitting, before windows go away.
    func writeNow() {
        guard let multiplexer = SessionHost.shared.multiplexer else { return }
        if let pending = inFlight {
            // The write still out carries an older layout; its answer has the
            // revision this one must carry. A host that doesn't answer in time
            // keeps what it has rather than hold up quitting.
            guard pending.answered.wait(timeout: .now() + 2) == .success else { return }
            if let answer = pending.answer, answer.updated { revision = max(revision, answer.revision) }
            inFlight = nil
        }
        guard var layout = snapshot() else { return }
        layout.revision = revision
        // Refused means the layout changed elsewhere meanwhile: the host's stays.
        _ = try? multiplexer.setLayout(layout)
    }

    // MARK: App → host

    /// The layout as Termy Next shows it; nil with no windows open, when the
    /// host's layout should stay as it is.
    func snapshot() -> MultiplexerLayout? {
        let controllers = AppDelegate.shared.controllersFrontToBack
        guard let front = controllers.first else { return nil }
        let open = Set(controllers.map(\.hostWindowID))
        let stored = floatingFrames
        var frames = stored
        var windows: [MultiplexerLayout.Window] = []
        for controller in controllers {
            let sessions = Workspace.shared.sessions.filter { session in
                if session === controller.session { return true }
                if AppDelegate.shared.controllers.contains(where: { $0.session === session }) { return false }
                let home = session.hostWindowID.flatMap { open.contains($0) ? $0 : nil } ?? front.hostWindowID
                return home == controller.hostWindowID
            }
            var workspaces: [MultiplexerLayout.Workspace] = []
            var active = 0
            for session in sessions {
                guard let workspace = workspace(for: session, frames: &frames) else { continue }
                if session === controller.session { active = workspaces.count }
                workspaces.append(workspace)
            }
            workspaces += emptyWorkspaces[controller.hostWindowID] ?? []
            guard !workspaces.isEmpty else { continue }
            windows.append(.init(id: controller.hostWindowID, activeWorkspace: active, workspaces: workspaces))
        }
        // Every write and every host change takes a snapshot; most move no floating pane.
        if frames != stored { floatingFrames = frames }
        return MultiplexerLayout(revision: revision, windows: windows)
    }

    private func workspace(for session: TerminalSessionGroup, frames: inout [String: CGRect]) -> MultiplexerLayout.Workspace? {
        var tabs: [MultiplexerLayout.Tab] = []
        var active = 0
        for tab in session.tabs {
            guard let stored = storedTab(tab, frames: &frames) else { continue }
            if tab === session.selectedTab { active = tabs.count }
            tabs.append(stored)
        }
        return tabs.isEmpty ? nil : .init(name: session.name, activeTab: active, tabs: tabs)
    }

    private func storedTab(_ tab: TerminalTab, frames: inout [String: CGRect]) -> MultiplexerLayout.Tab? {
        var node = tab.tree.compactMap { tab.panes[$0]?.paneID }.map(Self.node(from:))
        // Floating panes go in as splits, and float again from their frames.
        for (id, frame) in tab.floating.sorted(by: { $0.key.uuidString < $1.key.uuidString }) {
            guard let paneID = tab.panes[id]?.paneID else { continue }
            frames[paneID] = frame
            node = node.map { .split(axis: .horizontal, ratio: 0.75, first: $0, second: .leaf(paneID)) } ?? .leaf(paneID)
        }
        for pane in tab.allPanes where tab.floating[pane.id] == nil {
            if let paneID = pane.paneID { frames[paneID] = nil }
        }
        guard let node else { return nil }
        let active = tab.panes[tab.zoomedID ?? tab.focusedID]?.paneID
        let zoom = Dictionary(uniqueKeysWithValues: tab.allPanes.compactMap { pane -> (String, Int16)? in
            guard pane.zoom != 0, let id = pane.paneID else { return nil }
            return (id, Int16(pane.zoom.rounded()))
        })
        return .init(title: tab.customName, zoomed: tab.zoomedID != nil, activePane: active, zoom: zoom, tree: node)
    }

    private static func node(from tree: SplitTree<String>) -> MultiplexerLayout.Node {
        switch tree {
        case .leaf(let id): .leaf(id)
        case .split(let axis, let ratio, let first, let second):
            .split(axis: axis == .horizontal ? .horizontal : .vertical, ratio: Double(ratio),
                   first: node(from: first), second: node(from: second))
        }
    }

    // MARK: Host → app

    /// Makes Termy Next show `layout`, reusing the panes, tabs, sessions and
    /// windows it already has and attaching to panes it doesn't.
    func apply(_ layout: MultiplexerLayout) {
        if let current = snapshot(), current.windows == layout.windows {
            revision = layout.revision
            return
        }
        applying = true
        revision = layout.revision

        var views: [String: PaneView] = [:]
        var tabOfPane: [String: TerminalTab] = [:]
        for session in Workspace.shared.sessions {
            for tab in session.tabs {
                for pane in tab.allPanes {
                    guard let id = pane.paneID else { continue }
                    views[id] = pane
                    tabOfPane[id] = tab
                }
            }
        }
        let frames = floatingFrames
        var usedTabs = Set<ObjectIdentifier>()
        var usedSessions = Set<ObjectIdentifier>()
        var placed = Set<String>()
        var sessions: [TerminalSessionGroup] = []
        var shown: [(windowID: String, session: TerminalSessionGroup)] = []
        emptyWorkspaces = [:]

        for window in layout.windows {
            var firstSession: TerminalSessionGroup?
            for (index, workspace) in window.workspaces.enumerated() {
                guard !workspace.tabs.isEmpty else {
                    emptyWorkspaces[window.id, default: []].append(workspace)
                    continue
                }
                let ids = Set(workspace.tabs.flatMap(\.tree.paneIDs))
                let session = Workspace.shared.sessions
                    .filter { !usedSessions.contains(ObjectIdentifier($0)) }
                    .max { overlap($0, ids) < overlap($1, ids) }
                    .flatMap { overlap($0, ids) > 0 ? $0 : nil }
                    ?? Workspace.shared.sessions.first { !usedSessions.contains(ObjectIdentifier($0)) && $0.name == workspace.name && !$0.tabs.contains(where: \.isHosted) }
                    ?? TerminalSessionGroup(name: workspace.name)
                usedSessions.insert(ObjectIdentifier(session))
                session.name = workspace.name
                session.hostWindowID = window.id

                var tabs: [TerminalTab] = []
                for stored in workspace.tabs {
                    guard let tab = buildTab(stored, views: &views, tabOfPane: tabOfPane, usedTabs: &usedTabs, frames: frames) else { continue }
                    tab.allPanes.compactMap(\.paneID).forEach { placed.insert($0) }
                    tabs.append(tab)
                }
                // Tabs of in-app panes stay with their session.
                tabs += session.tabs.filter { !$0.isHosted && !usedTabs.contains(ObjectIdentifier($0)) }
                guard !tabs.isEmpty else { continue }
                session.adopt(tabs: tabs, selected: min(workspace.activeTab, tabs.count - 1))
                sessions.append(session)
                if index == window.activeWorkspace || firstSession == nil { firstSession = session }
                if index == window.activeWorkspace { shown.removeAll { $0.windowID == window.id } }
                if index == window.activeWorkspace || !shown.contains(where: { $0.windowID == window.id }) {
                    shown.append((window.id, session))
                }
            }
        }

        // Live panes the layout doesn't list yet (made here a moment ago,
        // before the write) stay where they were: beside the panes of their
        // tab when the host kept that tab, else in their own tab and session.
        var needsWrite = false
        var keptTabs: [ObjectIdentifier: TerminalTab] = [:]
        for (id, pane) in views where !placed.contains(id) && !pane.isClosing && !pane.terminal.session.hasExited {
            needsWrite = true
            guard let old = tabOfPane[id] else { continue }
            if usedTabs.contains(ObjectIdentifier(old)) {
                if old.panes[pane.id] == nil, let anchor = old.focusedPane?.id ?? old.tree.leaves.last {
                    old.insert(pane, nextTo: anchor, direction: .right)
                }
            } else {
                keptTabs[ObjectIdentifier(old)] = old
            }
        }
        for old in keptTabs.values {
            // Keep only this tab's panes that the host didn't place elsewhere.
            let mine = old.allPanes.filter { pane in pane.paneID.map { !placed.contains($0) } ?? true }
            guard let tree = old.tree.compactMap({ id in mine.contains { $0.id == id } ? id : nil }) else { continue }
            old.adopt(tree: tree, panes: mine, floating: old.floating.filter { id, _ in mine.contains { $0.id == id } },
                      focus: old.focusedID, zoomed: old.zoomedID)
            let session = old.session ?? sessions.first ?? TerminalSessionGroup(name: Workspace.shared.newSessionName(near: nil))
            if !session.tabs.contains(where: { $0 === old }) { session.append(old, select: false) }
            if !sessions.contains(where: { $0 === session }) {
                // Keep the session's other tabs only where they still hold their panes.
                session.adopt(tabs: session.tabs.filter { tab in tab === old || keptTabs[ObjectIdentifier(tab)] != nil || usedTabs.contains(ObjectIdentifier(tab)) },
                              selected: session.selectedIndex)
                sessions.append(session)
            }
        }

        Workspace.shared.adopt(sessions: sessions)
        showWindows(shown)
        applying = false
        NotificationCenter.default.post(name: Workspace.didChange, object: nil)
        SessionHost.shared.scheduleSave()
        if needsWrite { scheduleWrite() }
    }

    private func overlap(_ session: TerminalSessionGroup, _ ids: Set<String>) -> Int {
        session.tabs.flatMap(\.allPanes).filter { $0.paneID.map(ids.contains) ?? false }.count
    }

    private func buildTab(_ stored: MultiplexerLayout.Tab, views: inout [String: PaneView], tabOfPane: [String: TerminalTab],
                          usedTabs: inout Set<ObjectIdentifier>, frames: [String: CGRect]) -> TerminalTab? {
        let ids = stored.tree.paneIDs
        for id in ids where views[id] == nil {
            if let pane = try? PaneView(attaching: id) { views[id] = pane }
        }
        guard var tree = Self.tree(from: stored.tree, views: views) else { return nil }
        var floating: [UUID: CGRect] = [:]
        for id in ids {
            guard let frame = frames[id], let pane = views[id], let rest = tree.removing(pane.id) else { continue }
            tree = rest
            floating[pane.id] = frame
        }
        let panes = ids.compactMap { views[$0] }
        // The tab that holds most of these panes already, so it keeps its identity.
        let candidates = ids.compactMap { tabOfPane[$0] }.filter { !usedTabs.contains(ObjectIdentifier($0)) }
        let existing = candidates.max { a, b in candidates.filter { $0 === a }.count < candidates.filter { $0 === b }.count }
        let localPanes = existing?.allPanes.filter { !$0.isHosted } ?? []
        let tab = existing ?? TerminalTab(tree: tree, panes: panes, floating: floating)
        usedTabs.insert(ObjectIdentifier(tab))
        let active = stored.activePane.flatMap { views[$0]?.id }
        tab.adopt(tree: tree, panes: panes, floating: floating, focus: active, zoomed: stored.zoomed ? active : nil)
        for pane in localPanes {
            if let anchor = tab.tree.leaves.last { tab.insert(pane, nextTo: anchor, direction: .right) }
        }
        tab.customName = stored.title
        for id in ids { views[id]?.zoom = Float(stored.zoom[id] ?? 0) }
        return tab
    }

    private static func tree(from node: MultiplexerLayout.Node, views: [String: PaneView]) -> SplitTree<UUID>? {
        switch node {
        case .leaf(let id):
            return views[id].map { .leaf($0.id) }
        case .split(let axis, let ratio, let first, let second):
            let a = tree(from: first, views: views), b = tree(from: second, views: views)
            switch (a, b) {
            case (let a?, let b?): return .split(axis: axis == .horizontal ? .horizontal : .vertical, ratio: CGFloat(ratio), first: a, second: b)
            case (let only?, nil), (nil, let only?): return only
            case (nil, nil): return nil
            }
        }
    }

    /// Shows each host window's active session in its Termy Next window,
    /// opening windows the host has and Termy Next doesn't.
    private func showWindows(_ shown: [(windowID: String, session: TerminalSessionGroup)]) {
        for (windowID, session) in shown {
            if let controller = AppDelegate.shared.controllers.first(where: { $0.hostWindowID == windowID }) {
                controller.showFromHost(session)
            } else if let controller = AppDelegate.shared.open(session) {
                controller.hostWindowID = windowID
                session.hostWindowID = windowID
            }
        }
        // Windows whose session the host moved elsewhere show what is left.
        for controller in AppDelegate.shared.controllers where !shown.contains(where: { $0.windowID == controller.hostWindowID }) {
            controller.showFromHost(controller.session)
        }
    }
}
