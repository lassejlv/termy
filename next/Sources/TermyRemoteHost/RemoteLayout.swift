import Foundation
import Termy

/// All edits use the host's compare-and-swap revision. Next's existing layout
/// watcher adopts these exact sessions/tabs/splits, including when no window is open.
enum RemoteLayout {
    enum EditError: Error { case missingSession, busy, invalidName }
    static func edit(_ mux: Multiplexer, _ change: (inout MultiplexerLayout) throws -> Void) throws {
        for _ in 0..<8 {
            var layout = try mux.layout()
            try change(&layout)
            if try mux.setLayout(layout).updated { return }
        }
        throw EditError.busy
    }
    static func name(_ proposed: String) throws -> String {
        let name = String(proposed.trimmingCharacters(in: .whitespacesAndNewlines).prefix(80))
        guard !name.contains(where: { $0.isNewline || $0.asciiValue.map { $0 < 32 } == true }) else {
            throw EditError.invalidName
        }
        return name.isEmpty ? "Session" : name
    }
    static func addSession(_ layout: inout MultiplexerLayout, pane: String, name proposed: String) throws {
        let base = try name(proposed)
        let taken = Set(layout.windows.flatMap(\.workspaces).map(\.name))
        var name = base
        var suffix = 2
        while taken.contains(name) {
            name = "\(base) \(suffix)"
            suffix += 1
        }
        let workspace = MultiplexerLayout.Workspace(name: name, tabs: [.init(activePane: pane, tree: .leaf(pane))])
        if layout.windows.isEmpty {
            layout.windows.append(.init(id: UUID().uuidString, workspaces: [workspace]))
        } else {
            layout.windows[0].workspaces.append(workspace)
        }
    }
    static func locate(_ layout: MultiplexerLayout, pane: String) throws -> (Int, Int, Int) {
        for (w, window) in layout.windows.enumerated() {
            for (s, session) in window.workspaces.enumerated() {
                if let t = session.tabs.firstIndex(where: { $0.tree.paneIDs.contains(pane) }) { return (w, s, t) }
            }
        }
        throw EditError.missingSession
    }
    static func split(_ node: MultiplexerLayout.Node, at anchor: String, pane: String, horizontal: Bool)
        -> MultiplexerLayout.Node
    {
        switch node {
        case .leaf(let id):
            return id == anchor
                ? .split(axis: horizontal ? .horizontal : .vertical, ratio: 0.5, first: node, second: .leaf(pane))
                : node
        case .split(let axis, let ratio, let a, let b):
            return .split(
                axis: axis, ratio: ratio, first: split(a, at: anchor, pane: pane, horizontal: horizontal),
                second: split(b, at: anchor, pane: pane, horizontal: horizontal))
        }
    }
}
