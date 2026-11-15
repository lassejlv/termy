import AppKit
import TermyNextCore

/// A tab ("window" in Rex's model): a split layout of panes, plus panes
/// floating above it.
@MainActor
final class TerminalTab {
    /// Kept across relaunches through its panes in the session host.
    let id: UUID
    var customName: String?
    var tree: SplitTree<UUID>
    private(set) var panes: [UUID: PaneView] = [:]
    /// Floating panes and their frames as fractions of the tab's area.
    var floating: [UUID: CGRect] = [:] {
        didSet {
            // A pane that starts floating goes on top of the others.
            floatingOrder.removeAll { floating[$0] == nil }
            for id in floating.keys.sorted(by: { $0.uuidString < $1.uuidString }) where !floatingOrder.contains(id) {
                floatingOrder.append(id)
            }
        }
    }
    /// Floating panes back to front.
    private(set) var floatingOrder: [UUID] = []
    /// Floating panes that move and stack as one (a Rex layer holding a
    /// split), by layer ID, with the area each layer covers. A floating pane
    /// in no group is a layer of its own, whose ID is the pane's.
    private(set) var layerTrees: [UUID: SplitTree<UUID>] = [:]
    private(set) var layerBounds: [UUID: CGRect] = [:]
    /// Layers a plugin minimized: their panes keep running, out of sight.
    var minimizedLayers: Set<UUID> = []
    /// Whoever the plugin API says owns each pane; the app when unset.
    var owners: [UUID: String] = [:]
    var focusedID: UUID
    var zoomedID: UUID?
    /// Most recently focused first, for focus after close.
    private var focusHistory: [UUID] = []
    weak var session: TerminalSessionGroup?

    init(pane: PaneView) {
        id = PluginIdentity.tabID(for: [pane]) ?? UUID()
        tree = .leaf(pane.id)
        focusedID = pane.id
        panes[pane.id] = pane
        focusHistory = [pane.id]
    }

    /// A restored tab: `tree` and `floating` over the `panes` given.
    init(tree: SplitTree<UUID>, panes: [PaneView], floating: [UUID: CGRect]) {
        id = PluginIdentity.tabID(for: panes) ?? UUID()
        self.tree = tree
        self.floating = floating
        floatingOrder = floating.keys.sorted { $0.uuidString < $1.uuidString }
        for pane in panes {
            self.panes[pane.id] = pane
            pane.isFloating = floating[pane.id] != nil
        }
        focusedID = tree.leaves.first ?? panes[0].id
        focusHistory = [focusedID]
    }

    var focusedPane: PaneView? { panes[focusedID] }
    /// Whether any pane runs in the session host.
    var isHosted: Bool { panes.values.contains { $0.isHosted } }
    var allPanes: [PaneView] { (tree.leaves + floatingOrder).compactMap { panes[$0] } }

    /// Brings a floating pane, and the layer it's in, in front of the others.
    func raiseFloating(_ id: UUID) {
        guard let layer = layerID(of: id) else { return }
        raiseLayer(layer)
    }

    // MARK: Layers

    /// The layer a floating pane is in; nil for a tiled pane.
    func layerID(of pane: UUID) -> UUID? {
        guard floating[pane] != nil else { return nil }
        return layerTrees.first { $0.value.contains(pane) }?.key ?? pane
    }

    /// The floating layers back to front.
    var layerOrder: [UUID] {
        var seen = Set<UUID>()
        return floatingOrder.compactMap { pane in
            guard let layer = layerID(of: pane), seen.insert(layer).inserted else { return nil }
            return layer
        }
    }

    func layerPanes(_ layer: UUID) -> [UUID] { layerTrees[layer]?.leaves ?? (floating[layer] != nil ? [layer] : []) }
    func layerTree(_ layer: UUID) -> SplitTree<UUID>? { layerTrees[layer] ?? (floating[layer] != nil ? .leaf(layer) : nil) }
    func bounds(ofLayer layer: UUID) -> CGRect? { layerBounds[layer] ?? floating[layer] }
    func isMinimized(_ pane: UUID) -> Bool { layerID(of: pane).map { minimizedLayers.contains($0) } ?? false }

    /// Adds `newPanes` floating as one layer laid out by `tree` over `bounds`.
    func addLayer(_ layer: UUID, tree: SplitTree<UUID>, panes newPanes: [PaneView], bounds: CGRect) {
        for pane in newPanes {
            panes[pane.id] = pane
            pane.isFloating = true
        }
        if tree.leaves.count == 1, let only = tree.leaves.first {
            floating[only] = bounds
            return
        }
        layerTrees[layer] = tree
        layerBounds[layer] = bounds
        relayoutLayer(layer)
    }

    /// Puts `subtree` beside `anchor` inside the anchor's floating layer.
    func insert(_ subtree: SplitTree<UUID>, panes newPanes: [PaneView], inLayerAt anchor: UUID, direction: Direction, ratio: CGFloat) {
        guard let layer = layerID(of: anchor), let tree = layerTree(layer), let bounds = bounds(ofLayer: layer) else { return }
        for pane in newPanes {
            panes[pane.id] = pane
            pane.isFloating = true
        }
        layerTrees[layer] = tree.inserting(subtree, at: anchor, direction: direction, ratio: ratio)
        layerBounds[layer] = bounds
        relayoutLayer(layer)
        // The new panes stack with their layer.
        if let top = floatingOrder.lastIndex(where: { layerPanes(layer).contains($0) }) {
            let added = newPanes.map(\.id)
            floatingOrder.removeAll { added.contains($0) }
            floatingOrder.insert(contentsOf: added, at: min(top, floatingOrder.count))
        }
    }

    /// Moves or resizes a layer to `rect`, fractions of the tab's area.
    func setBounds(_ rect: CGRect, ofLayer layer: UUID) {
        if layerTrees[layer] != nil {
            layerBounds[layer] = rect
            relayoutLayer(layer)
        } else if floating[layer] != nil {
            floating[layer] = rect
        }
    }

    /// Moves the layer `pane` is in by `dx`, `dy` (fractions), as a header drag does.
    func moveLayer(containing pane: UUID, dx: CGFloat, dy: CGFloat) {
        guard let layer = layerID(of: pane), var rect = bounds(ofLayer: layer) else { return }
        rect.origin.x = min(max(rect.origin.x + dx, -rect.width + 0.1), 0.9)
        rect.origin.y = min(max(rect.origin.y + dy, 0), 0.9)
        setBounds(rect, ofLayer: layer)
    }

    func raiseLayer(_ layer: UUID, above other: UUID? = nil) {
        let members = layerPanes(layer)
        floatingOrder.removeAll { members.contains($0) }
        if let other, let index = floatingOrder.lastIndex(where: { layerPanes(other).contains($0) }) {
            floatingOrder.insert(contentsOf: members, at: index + 1)
        } else {
            floatingOrder.append(contentsOf: members)
        }
    }

    func lowerLayer(_ layer: UUID, below other: UUID? = nil) {
        let members = layerPanes(layer)
        floatingOrder.removeAll { members.contains($0) }
        if let other, let index = floatingOrder.firstIndex(where: { layerPanes(other).contains($0) }) {
            floatingOrder.insert(contentsOf: members, at: index)
        } else {
            floatingOrder.insert(contentsOf: members, at: 0)
        }
    }

    private func relayoutLayer(_ layer: UUID) {
        guard let tree = layerTrees[layer], let bounds = layerBounds[layer] else { return }
        for (pane, rect) in tree.unitRects(in: bounds) { floating[pane] = rect }
    }

    /// Takes `pane` out of its layer's split, which closes up around it.
    private func leaveLayer(_ pane: UUID) {
        guard let layer = layerTrees.first(where: { $0.value.contains(pane) })?.key, let tree = layerTrees[layer] else { return }
        if let rest = tree.removing(pane), rest.leaves.count > 1 {
            layerTrees[layer] = rest
            relayoutLayer(layer)
        } else {
            // One pane left: it floats on its own, where it is.
            layerTrees[layer] = nil
            layerBounds[layer] = nil
            if minimizedLayers.remove(layer) != nil, let last = tree.removing(pane)?.leaves.first { minimizedLayers.insert(last) }
        }
    }
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
        guard let pane = panes[id] else { return nil }
        leaveLayer(id)
        panes[id] = nil
        minimizedLayers.remove(id)
        owners[id] = nil
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

    /// Brings every pane of `other` into this tab, its layout placed next to
    /// `anchor` as one block, and focuses the pane that was focused there.
    func merge(_ other: TerminalTab, nextTo anchor: UUID, direction: Direction) {
        for pane in other.allPanes { panes[pane.id] = pane }
        floating.merge(other.floating) { mine, _ in mine }
        if tree.contains(anchor) {
            tree = tree.inserting(other.tree, at: anchor, direction: direction)
        } else if let last = tree.leaves.last {
            tree = tree.inserting(other.tree, at: last, direction: direction)
        } else {
            tree = other.tree
        }
        zoomedID = nil
        setFocus(other.focusedID)
    }

    /// Takes on a layout from the session host: `tree` over `panes`, with
    /// the floating ones on top. Focus stays where it was when that pane is
    /// still here.
    func adopt(tree: SplitTree<UUID>, panes newPanes: [PaneView], floating newFloating: [UUID: CGRect], focus: UUID?, zoomed: UUID?) {
        panes = Dictionary(uniqueKeysWithValues: newPanes.map { ($0.id, $0) })
        for pane in newPanes { pane.isFloating = newFloating[pane.id] != nil }
        self.tree = tree
        floating = newFloating
        focusHistory.removeAll { panes[$0] == nil }
        if let focus, panes[focus] != nil { setFocus(focus) } else if panes[focusedID] == nil { focusedID = tree.leaves.first ?? focusedID }
        zoomedID = zoomed.flatMap { panes[$0] != nil ? $0 : nil }
    }

    // MARK: Plugin layouts

    /// Adds `pane` floating at `frame`, fractions of the tab's area.
    func addFloating(_ pane: PaneView, frame: CGRect) {
        panes[pane.id] = pane
        floating[pane.id] = frame
        pane.isFloating = true
    }

    /// Puts `subtree`, over `newPanes`, beside `anchor` on `direction`'s side;
    /// `ratio` is the share of the first (left or top) side.
    func insert(_ subtree: SplitTree<UUID>, panes newPanes: [PaneView], at anchor: UUID, direction: Direction, ratio: CGFloat) {
        for pane in newPanes { panes[pane.id] = pane }
        tree = tree.inserting(subtree, at: anchor, direction: direction, ratio: ratio)
        zoomedID = nil
    }

    /// Takes on `newTree`, which also holds `newPanes`.
    func replaceTree(_ newTree: SplitTree<UUID>, adding newPanes: [PaneView]) {
        for pane in newPanes { panes[pane.id] = pane }
        tree = newTree
        zoomedID = nil
    }

    func toggleFloating(_ id: UUID) {
        guard let pane = panes[id] else { return }
        if floating[id] != nil {
            leaveLayer(id)
            minimizedLayers.remove(id)
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
    /// Kept by name across relaunches; plugins see it as `session:<id>`.
    let id: UUID
    var name: String {
        didSet { if oldValue != name { PluginIdentity.sessionRenamed(id, from: oldValue, to: name) } }
    }
    private(set) var tabs: [TerminalTab] = []
    var selectedIndex = 0
    let created = Date()
    /// The session host window this session belongs to (`termy mux layout`):
    /// the window showing it, or the one that last did.
    var hostWindowID: String?
    var lastActive = Date()
    /// Panes a plugin took out of every tab (Rex's detached blocks): they
    /// keep running until placed again or closed.
    var detached: [PaneView] = []

    init(name: String) {
        self.name = name
        id = PluginIdentity.sessionID(named: name, taken: Set(Workspace.shared.sessions.map(\.id)))
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

    /// Takes on the session host's tabs for this session.
    func adopt(tabs newTabs: [TerminalTab], selected: Int) {
        for tab in newTabs { tab.session = self }
        tabs = newTabs
        selectedIndex = min(max(selected, 0), max(0, tabs.count - 1))
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
        let pane = try PaneView(workingDirectory: directory, session: session.id)
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

    /// Takes on the session host's sessions, in its order, keeping sessions
    /// it doesn't know (those of in-app panes) after them.
    func adopt(sessions newSessions: [TerminalSessionGroup]) {
        let rest = sessions.filter { old in !newSessions.contains { $0 === old } && old.tabs.contains { !$0.isHosted } }
        sessions = newSessions + rest
    }

    func remove(_ session: TerminalSessionGroup) {
        for tab in session.tabs { tab.allPanes.forEach { $0.terminate() } }
        session.detached.forEach { $0.terminate() }
        session.detached.removeAll()
        sessions.removeAll { $0 === session }
        changed()
    }

    /// Puts `session` before `other` in the order sessions are listed, or last.
    func move(_ session: TerminalSessionGroup, before other: TerminalSessionGroup?) {
        guard let index = sessions.firstIndex(where: { $0 === session }) else { return }
        sessions.remove(at: index)
        let target = other.flatMap { o in sessions.firstIndex { $0 === o } } ?? sessions.count
        sessions.insert(session, at: target)
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
