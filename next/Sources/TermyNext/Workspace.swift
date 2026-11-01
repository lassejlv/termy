import AppKit
import TermyNextCore

/// A tab ("window" in Rex's model): a split layout of panes, plus panes
/// floating above it.
@MainActor
final class TerminalTab {
    let id = UUID()
    var customName: String?
    var tree: SplitTree<UUID>
    private(set) var panes: [UUID: PaneView] = [:]
    /// Floating panes and their frames as fractions of the tab's area.
    var floating: [UUID: CGRect] = [:]
    var focusedID: UUID
    var zoomedID: UUID?
    /// Most recently focused first, for focus after close.
    private var focusHistory: [UUID] = []
    weak var session: TerminalSessionGroup?

    init(pane: PaneView) {
        tree = .leaf(pane.id)
        focusedID = pane.id
        panes[pane.id] = pane
        focusHistory = [pane.id]
    }

    /// A restored tab: `tree` and `floating` over the `panes` given.
    init(tree: SplitTree<UUID>, panes: [PaneView], floating: [UUID: CGRect]) {
        self.tree = tree
        self.floating = floating
        for pane in panes {
            self.panes[pane.id] = pane
            pane.isFloating = floating[pane.id] != nil
        }
        focusedID = tree.leaves.first ?? panes[0].id
        focusHistory = [focusedID]
    }

    var focusedPane: PaneView? { panes[focusedID] }
    var allPanes: [PaneView] { (tree.leaves + floating.keys.sorted { $0.uuidString < $1.uuidString }).compactMap { panes[$0] } }
    var isEmpty: Bool { panes.isEmpty }

    /// The pane whose OSC 7501 report the tab shows, when any pane reports one.
    var statusPane: PaneView? {
        allPanes.filter { $0.activity != .idle }.max { $0.activity.urgency < $1.activity.urgency }
    }

    /// A custom name wins; then the title a program reports; then the focused pane's label.
    var title: String { customName ?? statusPane?.status?.title ?? statusPane?.label.name ?? focusedPane?.label.name ?? "shell" }
    /// What a waiting or finished program says ("Needs permission · …"), else
    /// the directory. Working tabs keep the directory; the loader says enough.
    var detail: String? {
        if let pane = statusPane, pane.activity != .working, let detail = pane.statusDetail { return detail }
        return customName == nil ? focusedPane?.label.detail : nil
    }
    var program: String? { statusPane?.foregroundProgram ?? focusedPane?.foregroundProgram }

    /// Up to three distinct programs in this tab for its icon stack: the
    /// reporting pane first, then the focused one, then the rest.
    var icons: [(program: String?, executable: String?)] {
        var seen = Set<String>()
        var result: [(program: String?, executable: String?)] = []
        for pane in [statusPane, focusedPane].compactMap({ $0 }) + allPanes {
            let key = AppIcons.normalized(pane.foregroundProgram ?? "shell")
            guard seen.insert(key).inserted else { continue }
            result.append((pane.foregroundProgram, pane.foregroundExecutable))
            if result.count == 3 { break }
        }
        return result
    }

    /// The most urgent activity of any pane, for the tab badge.
    var activity: PaneActivity {
        allPanes.map(\.activity).max { $0.urgency < $1.urgency } ?? .idle
    }

    func setFocus(_ id: UUID) {
        guard panes[id] != nil else { return }
        focusedID = id
        focusHistory.removeAll { $0 == id }
        focusHistory.insert(id, at: 0)
    }

    func insert(_ pane: PaneView, nextTo anchor: UUID, direction: Direction) {
        panes[pane.id] = pane
        if tree.contains(anchor) {
            tree = tree.inserting(pane.id, at: anchor, direction: direction)
        } else if let first = tree.leaves.last {
            tree = tree.inserting(pane.id, at: first, direction: direction)
        }
        zoomedID = nil
        setFocus(pane.id)
    }

    /// Detaches a pane without stopping it. Returns false when it was the last one.
    @discardableResult
    func remove(_ id: UUID) -> PaneView? {
        guard let pane = panes.removeValue(forKey: id) else { return nil }
        floating[id] = nil
        if let rest = tree.removing(id) {
            tree = rest
        } else if let firstFloating = floating.keys.first {
            // The layout emptied: dock a floating pane.
            floating[firstFloating] = nil
            tree = .leaf(firstFloating)
        }
        if zoomedID == id { zoomedID = nil }
        focusHistory.removeAll { $0 == id }
        if focusedID == id, let next = focusHistory.first(where: { panes[$0] != nil }) ?? tree.leaves.first {
            focusedID = next
        }
        return pane
    }

    /// Puts `pane` where the pane `old` was: same split slot or floating
    /// frame, same focus and zoom. Returns the pane it replaced.
    @discardableResult
    func replace(_ old: UUID, with pane: PaneView) -> PaneView? {
        guard let previous = panes.removeValue(forKey: old) else { return nil }
        panes[pane.id] = pane
        if let rect = floating.removeValue(forKey: old) {
            floating[pane.id] = rect
            pane.isFloating = true
        }
        tree = tree.compactMap { $0 == old ? pane.id : $0 } ?? .leaf(pane.id)
        if focusedID == old { focusedID = pane.id }
        if zoomedID == old { zoomedID = pane.id }
        focusHistory = focusHistory.map { $0 == old ? pane.id : $0 }
        return previous
    }

    func toggleFloating(_ id: UUID) {
        guard let pane = panes[id] else { return }
        if floating[id] != nil {
            floating[id] = nil
            let anchor = focusHistory.first { $0 != id && tree.contains($0) } ?? tree.leaves.first
            if let anchor {
                tree = tree.inserting(id, at: anchor, direction: .right)
            } else {
                tree = .leaf(id)
            }
            pane.isFloating = false
        } else {
            guard tree.leaves.count > 1, let rest = tree.removing(id) else { return }
            tree = rest
            let offset = CGFloat(floating.count) * 0.04
            floating[id] = CGRect(x: 0.2 + offset, y: 0.15 + offset, width: 0.6, height: 0.62)
            pane.isFloating = true
            if zoomedID == id { zoomedID = nil }
        }
        setFocus(id)
    }
}

/// A session: a named group of tabs. Rex shows one session per window and
/// switches between them with ⌘K.
@MainActor
final class TerminalSessionGroup {
    let id = UUID()
    var name: String
    private(set) var tabs: [TerminalTab] = []
    var selectedIndex = 0
    let created = Date()
    var lastActive = Date()

    init(name: String) {
        self.name = name
    }

    var selectedTab: TerminalTab? { tabs.indices.contains(selectedIndex) ? tabs[selectedIndex] : tabs.first }

    var activity: PaneActivity {
        tabs.map(\.activity).max { $0.urgency < $1.urgency } ?? .idle
    }

    func append(_ tab: TerminalTab, select: Bool = true) {
        tab.session = self
        let index = min(selectedIndex + 1, tabs.count)
        tabs.insert(tab, at: tabs.isEmpty ? 0 : index)
        if select { selectedIndex = tabs.firstIndex { $0 === tab } ?? 0 }
    }

    func remove(_ tab: TerminalTab) {
        guard let index = tabs.firstIndex(where: { $0 === tab }) else { return }
        tabs.remove(at: index)
        if index < selectedIndex { selectedIndex -= 1 }
        selectedIndex = min(selectedIndex, max(0, tabs.count - 1))
    }

    func move(_ tab: TerminalTab, by offset: Int) {
        guard let index = tabs.firstIndex(where: { $0 === tab }) else { return }
        let target = min(max(index + offset, 0), tabs.count - 1)
        guard target != index else { return }
        tabs.remove(at: index)
        tabs.insert(tab, at: target)
        selectedIndex = target
    }

    func move(from source: Int, to destination: Int) {
        guard tabs.indices.contains(source) else { return }
        let tab = tabs.remove(at: source)
        let target = min(max(destination, 0), tabs.count)
        tabs.insert(tab, at: target)
        selectedIndex = target
    }
}

/// Every session in the app. Sessions outlive the windows that show them:
/// closing a window detaches its session, like a multiplexer.
@MainActor
final class Workspace {
    static let shared = Workspace()
    static let didChange = Notification.Name("TermyNextWorkspaceDidChange")

    private(set) var sessions: [TerminalSessionGroup] = []
    var recentDirectories: [String] = []

    func newSessionName(near directory: String?) -> String {
        Naming.sessionName(taken: Set(sessions.map(\.name)))
    }

    @discardableResult
    func createSession(name: String? = nil, directory: String?) throws -> TerminalSessionGroup {
        let session = TerminalSessionGroup(name: name.map { Naming.uniqued($0, taken: Set(sessions.map(\.name))) }
            ?? newSessionName(near: directory))
        let pane = try PaneView(workingDirectory: directory)
        session.append(TerminalTab(pane: pane))
        sessions.append(session)
        noteDirectory(directory)
        changed()
        return session
    }

    /// Registers a session built elsewhere, such as a tab moved to a new window.
    func adopt(_ session: TerminalSessionGroup) {
        session.name = Naming.uniqued(session.name, taken: Set(sessions.map(\.name)))
        sessions.append(session)
        changed()
    }

    func remove(_ session: TerminalSessionGroup) {
        for tab in session.tabs { tab.allPanes.forEach { $0.terminate() } }
        sessions.removeAll { $0 === session }
        changed()
    }

    func noteDirectory(_ path: String?) {
        guard let path, !path.isEmpty else { return }
        recentDirectories.removeAll { $0 == path }
        recentDirectories.insert(path, at: 0)
        if recentDirectories.count > 30 { recentDirectories.removeLast() }
    }

    func changed() {
        NotificationCenter.default.post(name: Self.didChange, object: nil)
        SessionHost.shared.scheduleSave()
    }

    /// Sessions by most recent use, for the session picker.
    var sessionsByRecency: [TerminalSessionGroup] {
        sessions.sorted { $0.lastActive > $1.lastActive }
    }
}
