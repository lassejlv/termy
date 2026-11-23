import AppKit
import Termy
import TermyNextCore

/// Rex's control API over Termy Next: sessions are sessions, Rex's windows
/// are tabs, blocks are panes, and floating layers are floating panes. Lua
/// plugins (`termy.session.*`, `rex.session.*`, `rex.block.call`) and the
/// `rex` command in panes both come here, on the main thread.
@MainActor
final class PluginAPI {
    static let shared = PluginAPI()

    struct Failure: Error, CustomStringConvertible, LocalizedError {
        var code: String
        var message: String
        init(_ code: String, _ message: String) {
            self.code = code
            self.message = message
        }
        var description: String { "\(message) (code=\(code))" }
        var errorDescription: String? { description }
    }

    /// Who is asking: the session and block an action or command runs in,
    /// and the pane a `rex` command was typed in.
    struct Context {
        var sessionID: String?
        var blockID: String?
        var callerBlockID: String?
        var origin = "api"
        /// The connection asking, when it keeps one open; else none.
        var clientID: String?

        init(sessionID: String? = nil, blockID: String? = nil, callerBlockID: String? = nil, origin: String = "api") {
            self.sessionID = sessionID
            self.blockID = blockID
            self.callerBlockID = callerBlockID
            self.origin = origin
        }

        init(json: [String: Any]?) {
            sessionID = json?["session_id"] as? String
            blockID = json?["block_id"] as? String
            callerBlockID = json?["caller_block_id"] as? String
            origin = json?["origin"] as? String ?? "api"
            clientID = json?["client_id"] as? String
        }
    }

    /// Goes up with every change to sessions, tabs and panes, as Rex's
    /// `revision` does.
    var revision: UInt64 = 0
    var observer: NSObjectProtocol?

    func start() {
        observer = NotificationCenter.default.addObserver(forName: Workspace.didChange, object: nil, queue: .main) { _ in
            MainActor.assumeIsolated {
                PluginAPI.shared.revision += 1
                PluginEvents.workspaceChanged()
            }
        }
    }

    // MARK: Dispatch

    func call(_ method: String, _ params: [String: Any], context: Context) throws -> Any {
        if let short = PluginFlavor.terminalMethod(method) {
            var forwarded = params
            forwarded["method"] = short
            if forwarded["args"] == nil {
                forwarded["args"] = params.filter { !["session_id", "block_id"].contains($0.key) }
            }
            return try call("block.method", forwarded, context: context)
        }
        switch method {
        case "session.list": return sessionList()
        case "session.view": return view(try session(params, context))
        case "session.list_blocks": return listBlocks(try session(params, context))
        case "session.create": return try createSession(params, context)
        case "session.destroy": return try destroy(try session(params, context))
        case "session.destroy_many":
            for raw in params["session_ids"] as? [String] ?? [] { _ = try destroy(try session(["session_id": raw], context)) }
            return ["revision": revision]
        case "session.new_window": return try newWindow(params, context)
        case "session.new_split": return try newSplit(params, context)
        case "session.new_layer": return try newLayer(params, context)
        case "session.focus_block": return try focusBlock(params, context)
        case "session.focus_window": return try focusWindow(params, context)
        case "session.focus_next_pane": return try cycleFocus(params, context, by: 1)
        case "session.focus_previous_pane": return try cycleFocus(params, context, by: -1)
        case "session.focus_next_window": return try cycleWindow(params, context, by: 1)
        case "session.focus_previous_window": return try cycleWindow(params, context, by: -1)
        case "session.set_label": return try setSessionLabel(params, context)
        case "session.set_window_label": return try setWindowLabel(params, context)
        case "session.set_block_label": return try setBlockLabel(params, context)
        case "session.close_window": return try closeWindow(params, context)
        case "session.close_layer": return try closeLayer(params, context)
        case "session.swap_blocks": return try swapBlocks(params, context)
        case "session.float_block": return try floatBlock(params, context)
        case "session.resize_layer": return try resizeLayer(params, context)
        case "block.close": return try closeBlock(params, context)
        default:
            if let result = try blockCall(method, params, context) { return result }
            if let result = try layoutCall(method, params, context) { return result }
            if let result = try clientCall(method, params, context) { return result }
            throw Failure("method_not_found", "no method named \(method)")
        }
    }

    // MARK: Lookup

    struct Located {
        let session: TerminalSessionGroup
        let tab: TerminalTab
        let pane: PaneView
    }

    /// Owners of detached panes, which have no tab to keep them.
    var detachedOwner: [UUID: String] = [:]

    /// A pane anywhere: in a tab, or detached in its session.
    func findAnyPane(_ raw: String?) -> (session: TerminalSessionGroup, tab: TerminalTab?, pane: PaneView)? {
        if let located = findPane(raw) { return (located.session, located.tab, located.pane) }
        guard let raw, !raw.isEmpty else { return nil }
        let id = PluginID.uuid(raw, kind: "block")
        let suffix = raw.lowercased().replacingOccurrences(of: "block:", with: "")
        let matches = sessions.flatMap { session in
            session.detached.filter { $0.id == id || (suffix.count >= 4 && $0.id.uuidString.lowercased().hasSuffix(suffix)) }
                .map { (session, $0) }
        }
        return matches.count == 1 ? (matches[0].0, nil, matches[0].1) : nil
    }

    /// The pane `raw` names, detached ones too, else the focused one.
    func resolveAnyPane(_ raw: String?, _ context: Context) throws -> (session: TerminalSessionGroup, tab: TerminalTab?, pane: PaneView) {
        if let raw, !raw.isEmpty {
            guard let found = findAnyPane(raw) else { throw Failure("not_found", "no block \(raw)") }
            return found
        }
        let located = try resolvePane(nil, context)
        return (located.session, located.tab, located.pane)
    }

    var sessions: [TerminalSessionGroup] { Workspace.shared.sessions }

    func controller(for session: TerminalSessionGroup) -> WindowController? {
        AppDelegate.shared.controllers.first { $0.session === session }
    }

    /// The session `params` or the context names. A session ID that no
    /// longer exists (a shell's REX_SESSION from before a rename) falls back
    /// to the session of the block asked about, or of the caller.
    func session(_ params: [String: Any], _ context: Context) throws -> TerminalSessionGroup {
        let raw = params["session_id"] as? String ?? context.sessionID
        if let raw, !raw.isEmpty {
            if let found = findSession(raw) { return found }
            for block in [params["block_id"] as? String, params["anchor_id"] as? String, context.blockID, context.callerBlockID] {
                if let located = findPane(block) { return located.session }
            }
            throw Failure("session_not_found", "no session \(raw)")
        }
        for block in [params["block_id"] as? String, context.blockID, context.callerBlockID] {
            if let located = findPane(block) { return located.session }
        }
        if let key = AppDelegate.shared.keyController?.session ?? AppDelegate.shared.controllersFrontToBack.first?.session {
            return key
        }
        guard let first = sessions.first else { throw Failure("conflict", "there are no sessions") }
        return first
    }

    func findSession(_ raw: String) -> TerminalSessionGroup? {
        if let id = PluginID.uuid(raw, kind: "session"), let found = sessions.first(where: { $0.id == id }) { return found }
        if let found = sessions.first(where: { $0.name == raw }) { return found }
        let suffix = raw.lowercased().replacingOccurrences(of: "session:", with: "")
        let matches = sessions.filter { $0.id.uuidString.lowercased().hasSuffix(suffix) }
        return suffix.count >= 4 && matches.count == 1 ? matches[0] : nil
    }

    func findPane(_ raw: String?) -> Located? {
        guard let raw, !raw.isEmpty else { return nil }
        let id = PluginID.uuid(raw, kind: "block")
        let suffix = raw.lowercased().replacingOccurrences(of: "block:", with: "")
        var bySuffix: [Located] = []
        for session in sessions {
            for tab in session.tabs {
                for pane in tab.allPanes {
                    if pane.id == id { return Located(session: session, tab: tab, pane: pane) }
                    if suffix.count >= 4, pane.id.uuidString.lowercased().hasSuffix(suffix) {
                        bySuffix.append(Located(session: session, tab: tab, pane: pane))
                    }
                }
            }
        }
        return bySuffix.count == 1 ? bySuffix[0] : nil
    }

    /// The pane `raw` names, or the focused pane of the context's session.
    func resolvePane(_ raw: String?, _ context: Context, session given: TerminalSessionGroup? = nil) throws -> Located {
        if let raw, !raw.isEmpty {
            guard let found = findPane(raw) else { throw Failure("not_found", "no block \(raw)") }
            return found
        }
        let session = try given ?? session([:], context)
        guard let tab = session.selectedTab, let pane = tab.focusedPane else { throw Failure("conflict", "the session has no focused block") }
        return Located(session: session, tab: tab, pane: pane)
    }

    func findTab(_ raw: String?, in session: TerminalSessionGroup) throws -> TerminalTab {
        guard let raw, !raw.isEmpty else {
            guard let tab = session.selectedTab else { throw Failure("conflict", "the session has no windows") }
            return tab
        }
        if let id = PluginID.uuid(raw, kind: "window") {
            for s in sessions { if let tab = s.tabs.first(where: { $0.id == id }) { return tab } }
        }
        if let index = Int(raw), session.tabs.indices.contains(index - 1) { return session.tabs[index - 1] }
        if let tab = session.tabs.first(where: { $0.title == raw || $0.customName == raw }) { return tab }
        let suffix = raw.lowercased().replacingOccurrences(of: "window:", with: "")
        let matches = sessions.flatMap(\.tabs).filter { $0.id.uuidString.lowercased().hasSuffix(suffix) }
        if suffix.count >= 4, matches.count == 1 { return matches[0] }
        throw Failure("not_found", "no window \(raw)")
    }

    func owner(of tab: TerminalTab) -> TerminalSessionGroup? {
        sessions.first { $0.tabs.contains { $0 === tab } }
    }

    // MARK: Describing

    func sessionList() -> [String: Any] {
        let list: [[String: Any]] = sessions.map { session in
            var entry: [String: Any] = [
                "session_id": PluginID.make("session", session.id),
                "label": session.name,
                "window_count": session.tabs.count,
                "created_at": ISO8601DateFormatter().string(from: session.created),
                "attached": controller(for: session) != nil,
            ]
            if let tab = session.selectedTab { entry["active_window_id"] = PluginID.make("window", tab.id) }
            return entry
        }
        return ["sessions": list]
    }

    func blockInfo(_ pane: PaneView, tab: TerminalTab?) -> [String: Any] {
        var info: [String: Any] = [
            "block_id": PluginID.make("block", pane.id),
            "label": pane.customLabel ?? pane.label.name,
            "creator_name": PluginFlavor.terminalCreator,
            "flavor": PluginFlavor.shell,
            "host_id": PluginAPI.hostID,
            "placement": tab == nil ? "detached" : "placed",
        ]
        info["owner"] = tab?.owners[pane.id] ?? (detachedOwner[pane.id]) ?? PluginClients.appClientID
        if let tab { info["window_id"] = PluginID.make("window", tab.id) }
        if let screen = screenGeometry(of: pane) { info["screen"] = screen }
        return info
    }

    /// Where a pane is on screen, for a plugin that places a window of its
    /// own over it (such as a browser). Only for a pane that's showing.
    /// Rects are `[x, y, width, height]` in points with the origin at the
    /// top left of the main display, as the window server reports window
    /// bounds; `window_number` is the window's number there.
    func screenGeometry(of pane: PaneView) -> [String: Any]? {
        guard let window = pane.window, window.isVisible, !pane.isHiddenOrHasHiddenAncestor,
              pane.bounds.width > 0, pane.bounds.height > 0 else { return nil }
        let top = NSScreen.screens.first?.frame.maxY ?? 0
        func onScreen(_ view: NSView) -> [Double] {
            let rect = window.convertToScreen(view.convert(view.bounds, to: nil))
            return [rect.minX, top - rect.maxY, rect.width, rect.height].map { (Double($0) * 100).rounded() / 100 }
        }
        return [
            "window_number": window.windowNumber,
            "frame": onScreen(pane),
            "content_frame": onScreen(pane.terminal),
            "scale": Double(window.backingScaleFactor),
            "visible": window.occlusionState.contains(.visible),
            "key": window.isKeyWindow,
        ]
    }

    static let hostID: String = {
        let key = "pluginHostID"
        if let id = UserDefaults.standard.string(forKey: key) { return id }
        let id = UUID().uuidString.lowercased()
        UserDefaults.standard.set(id, forKey: key)
        return id
    }()

    func listBlocks(_ session: TerminalSessionGroup) -> [String: Any] {
        let blocks = (session.tabs.flatMap { tab in tab.allPanes.map { blockInfo($0, tab: tab) } }
            + session.detached.map { blockInfo($0, tab: nil) })
            .sorted { ($0["block_id"] as! String) < ($1["block_id"] as! String) }
        return ["blocks": blocks]
    }

    func view(_ session: TerminalSessionGroup) -> [String: Any] {
        let windows: [[String: Any]] = session.tabs.map { tab in
            let frames = controller(for: session)?.pluginUnitFrames(of: tab) ?? tab.tree.unitRects()
            var layers: [[String: Any]] = [[
                "layer_id": PluginID.make("layer", tab.id),
                "kind": "tiled",
                "bounds": [0, 0, 1, 1],
                "layout": node(tab.tree, rect: CGRect(x: 0, y: 0, width: 1, height: 1), frames: frames),
                "blocks": tab.tree.leaves.compactMap { tab.panes[$0] }.map { blockInfo($0, tab: tab) },
            ]]
            for layer in tab.layerOrder {
                guard let tree = tab.layerTree(layer), let rect = tab.bounds(ofLayer: layer) else { continue }
                var entry: [String: Any] = [
                    "layer_id": PluginID.make("layer", layer),
                    "kind": "floating",
                    "bounds": array(rect),
                    "layout": node(tree, rect: rect, frames: tree.unitRects(in: rect)),
                    "blocks": tree.leaves.compactMap { tab.panes[$0] }.map { blockInfo($0, tab: tab) },
                ]
                if tab.minimizedLayers.contains(layer) { entry["state"] = "minimized" }
                layers.append(entry)
            }
            var window: [String: Any] = [
                "window_id": PluginID.make("window", tab.id),
                "label": tab.customName ?? tab.title,
                "focused_block_id": PluginID.make("block", tab.focusedID),
                "layers": layers,
            ]
            if let zoomed = tab.zoomedID { window["zoomed_block_id"] = PluginID.make("block", zoomed) }
            return window
        }
        var result: [String: Any] = [
            "revision": revision,
            "session_id": PluginID.make("session", session.id),
            "label": session.name,
            "windows": windows,
            "clients": controller(for: session) == nil ? [] : [PluginClients.info(PluginClients.app)],
        ]
        if let tab = session.selectedTab { result["active_window_id"] = PluginID.make("window", tab.id) }
        return result
    }

    func array(_ rect: CGRect) -> [Double] {
        [rect.minX, rect.minY, rect.width, rect.height].map { (Double($0) * 10_000).rounded() / 10_000 }
    }

    func node(_ tree: SplitTree<UUID>, rect: CGRect, frames: [UUID: CGRect]) -> [String: Any] {
        switch tree {
        case .leaf(let id):
            return ["rect": array(frames[id] ?? rect), "block_id": PluginID.make("block", id)]
        case .split(let axis, let ratio, let first, let second):
            let (a, b) = SplitTree<UUID>.unitDivide(rect, axis: axis, ratio: ratio)
            return [
                "rect": array(rect),
                "split": [
                    "split_id": PluginID.make("split", second.leaves[0]),
                    "direction": axis == .horizontal ? "horizontal" : "vertical",
                    "ratio": Double(ratio),
                    "before": node(first, rect: a, frames: frames),
                    "after": node(second, rect: b, frames: frames),
                ],
            ]
        }
    }

    // MARK: Creating

    var userShell: String {
        ProcessInfo.processInfo.environment["SHELL"].flatMap { $0.isEmpty ? nil : $0 } ?? "/bin/zsh"
    }

    /// Starts a pane for one block of a layout.
    func makePane(_ spec: PluginBlockSpec, session: TerminalSessionGroup, near: PaneView?) throws -> PaneView {
        // termy-run keeps the program in front and reports how it exited.
        let command = spec.command.map { argv in
            let wrapped = [PluginPaths.runner] + PluginPaths.translate(argv)
            return spec.loginShell ? ShellQuote.loginCommand(wrapped, shell: userShell)
                : "exec " + wrapped.map(ShellQuote.quote).joined(separator: " ")
        }
        let directory = spec.cwd.map { ($0 as NSString).expandingTildeInPath }
            ?? near?.directory ?? AppConfig.shared.startDirectory
        let pane = try PaneView(workingDirectory: directory, command: command, session: session.id,
                                environment: spec.environment, keepOpen: spec.keepOpen)
        pane.customLabel = spec.label.flatMap { $0.isEmpty ? nil : $0 }
        if let near { pane.zoom = near.zoom }
        Workspace.shared.noteDirectory(directory)
        return pane
    }

    /// Starts every block of `layout`, or none: a failure ends those started.
    func makePanes(_ layout: PluginLayout, session: TerminalSessionGroup, near: PaneView?) throws -> [PaneView] {
        var made: [PaneView] = []
        do {
            for spec in layout.blocks { made.append(try makePane(spec, session: session, near: near)) }
        } catch {
            made.forEach { $0.terminate() }
            throw Failure("internal", "a block failed to start: \(error.localizedDescription)")
        }
        return made
    }

    func layout(_ params: [String: Any]) throws -> PluginLayout {
        do {
            return try PluginLayout(json: params["layout"])
        } catch let error as PluginLayoutError {
            throw Failure(error.description.hasPrefix("no creator") ? "not_found" : "bad_request", error.description)
        }
    }

    func ids(_ panes: [PaneView]) -> [String] { panes.map { PluginID.make("block", $0.id) } }

    func createSession(_ params: [String: Any], _ context: Context) throws -> [String: Any] {
        let label = (params["label"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        let name = label.map { Naming.uniqued($0, taken: Set(sessions.map(\.name))) } ?? Workspace.shared.newSessionName(near: nil)
        let session = TerminalSessionGroup(name: name)
        var created: [[String: Any]] = []
        let windows = params["windows"] as? [[String: Any]] ?? [["layout": ["block": ["flavor": PluginFlavor.shell]]]]
        do {
            for window in windows {
                let layout = try layout(window)
                let panes = try makePanes(layout, session: session, near: nil)
                let tab = TerminalTab(tree: layout.tree(ids: panes.map(\.id)), panes: panes, floating: [:])
                if let label = (window["window_label"] ?? window["label"]) as? String, !label.isEmpty { tab.customName = label }
                session.append(tab, select: created.isEmpty)
                created.append(["window_id": PluginID.make("window", tab.id), "block_ids": ids(panes)])
            }
        } catch {
            session.tabs.forEach { $0.allPanes.forEach { $0.terminate() } }
            throw error
        }
        session.selectedIndex = 0
        Workspace.shared.adopt(session)
        return ["session_id": PluginID.make("session", session.id), "label": session.name, "initial_windows": created, "revision": revision]
    }

    func destroy(_ session: TerminalSessionGroup) throws -> [String: Any] {
        let id = PluginID.make("session", session.id)
        if let controller = controller(for: session) {
            controller.pluginCloseSession()
        } else {
            Workspace.shared.remove(session)
        }
        return ["revision": revision]
    }

    func newWindow(_ params: [String: Any], _ context: Context) throws -> [String: Any] {
        let session = try session(params, context)
        let layout = try layout(params)
        let near = session.selectedTab?.focusedPane
        let panes = try makePanes(layout, session: session, near: near)
        let tab = TerminalTab(tree: layout.tree(ids: panes.map(\.id)), panes: panes, floating: [:])
        if let label = (params["window_label"] ?? params["label"]) as? String, !label.isEmpty { tab.customName = label }
        let focus = params["focus"] as? Bool ?? true
        session.append(tab, select: focus || session.tabs.isEmpty)
        if let controller = controller(for: session) {
            controller.pluginRefresh(tab: tab, appearing: panes, focus: focus ? panes.first : nil, select: focus)
        }
        Workspace.shared.changed()
        return ["window_id": PluginID.make("window", tab.id), "block_ids": ids(panes), "revision": revision]
    }

    func newSplit(_ params: [String: Any], _ context: Context) throws -> [String: Any] {
        let session = try session(params, context)
        let horizontal = (params["direction"] as? String ?? "horizontal") == "horizontal"
        guard ["horizontal", "vertical"].contains(params["direction"] as? String ?? "horizontal") else {
            throw Failure("bad_request", "direction must be horizontal or vertical")
        }
        let after = (params["side"] as? String ?? "after") != "before"
        let ratio = CGFloat((params["ratio"] as? NSNumber)?.doubleValue ?? 0.5)
        guard ratio > 0, ratio < 1 else { throw Failure("bad_request", "ratio must be between 0 and 1") }
        let anchorRaw = params["anchor_id"] as? String
        if anchorRaw != nil, params["window_id"] != nil { throw Failure("bad_request", "send anchor_id or window_id, not both") }
        let layout = try layout(params)
        let direction: Direction = horizontal ? (after ? .right : .left) : (after ? .down : .up)

        // A split ID anchors the whole split, which goes on one side.
        if let anchorRaw, anchorRaw.hasPrefix("split:"), let key = PluginID.uuid(anchorRaw, kind: "split") {
            guard let tab = session.tabs.first(where: { $0.tree.splitKeys().contains(key) }) else {
                throw Failure("not_found", "no split \(anchorRaw)")
            }
            let panes = try makePanes(layout, session: session, near: tab.focusedPane)
            let subtree = layout.tree(ids: panes.map(\.id))
            guard let newTree = tab.tree.replacingSplit(key, with: { old in
                after ? .split(axis: horizontal ? .horizontal : .vertical, ratio: ratio, first: old, second: subtree)
                    : .split(axis: horizontal ? .horizontal : .vertical, ratio: ratio, first: subtree, second: old)
            }) else { throw Failure("not_found", "no split \(anchorRaw)") }
            tab.replaceTree(newTree, adding: panes)
            return finishSplit(session: session, tab: tab, panes: panes, anchor: anchorRaw, params: params)
        }

        let anchor: Located
        if let anchorRaw {
            guard let found = findPane(anchorRaw) else { throw Failure("not_found", "no block \(anchorRaw)") }
            anchor = found
        } else {
            let tab = try findTab(params["window_id"] as? String, in: session)
            guard let pane = tab.focusedPane else { throw Failure("conflict", "the window has no focused block") }
            anchor = Located(session: session, tab: tab, pane: pane)
        }
        let panes = try makePanes(layout, session: anchor.session, near: anchor.pane)
        let tab = anchor.tab
        if tab.floating[anchor.pane.id] != nil {
            // A floating layer splits within its own area.
            tab.insert(layout.tree(ids: panes.map(\.id)), panes: panes, inLayerAt: anchor.pane.id, direction: direction, ratio: ratio)
        } else {
            tab.insert(layout.tree(ids: panes.map(\.id)), panes: panes, at: anchor.pane.id, direction: direction, ratio: ratio)
        }
        return finishSplit(session: anchor.session, tab: tab, panes: panes, anchor: PluginID.make("block", anchor.pane.id), params: params)
    }

    func finishSplit(session: TerminalSessionGroup, tab: TerminalTab, panes: [PaneView], anchor: String, params: [String: Any]) -> [String: Any] {
        let focus = params["focus"] as? Bool ?? false
        if focus {
            tab.setFocus(panes[0].id)
            if let index = session.tabs.firstIndex(where: { $0 === tab }) { session.selectedIndex = index }
        }
        controller(for: session)?.pluginRefresh(tab: tab, appearing: panes, focus: focus ? panes.first : nil, select: focus)
        Workspace.shared.changed()
        let splitIDs = panes.isEmpty ? [] : [PluginID.make("split", panes[0].id)]
        return ["anchor_id": anchor, "block_ids": ids(panes), "split_ids": splitIDs, "revision": revision]
    }

    func bounds(_ raw: Any?) throws -> CGRect {
        var values: [Double] = []
        if let array = raw as? [Any] {
            values = array.compactMap { ($0 as? NSNumber)?.doubleValue }
        } else if let object = raw as? [String: Any] {
            values = ["x", "y", "w", "h"].compactMap { (object[$0] as? NSNumber)?.doubleValue }
        }
        guard values.count == 4, values[2] > 0, values[3] > 0 else {
            throw Failure("bad_request", "bounds must be [x, y, w, h] fractions of the window")
        }
        let rect = CGRect(x: values[0], y: values[1], width: values[2], height: values[3])
        let unit = CGRect(x: 0, y: 0, width: 1, height: 1)
        guard unit.insetBy(dx: -0.001, dy: -0.001).contains(rect) else { throw Failure("bad_request", "bounds must fit inside the window") }
        return rect.intersection(unit)
    }

    func newLayer(_ params: [String: Any], _ context: Context) throws -> [String: Any] {
        let session = try session(params, context)
        let rect = try bounds(params["bounds"])
        let layout = try layout(params)
        let tab = try findTab(params["window_id"] as? String, in: session)
        let panes = try makePanes(layout, session: session, near: tab.focusedPane)
        tab.addLayer(panes[0].id, tree: layout.tree(ids: panes.map(\.id)), panes: panes, bounds: rect)
        let focus = params["focus"] as? Bool ?? false
        if focus {
            tab.setFocus(panes[0].id)
            if let index = session.tabs.firstIndex(where: { $0 === tab }) { session.selectedIndex = index }
        }
        controller(for: session)?.pluginRefresh(tab: tab, appearing: panes, focus: focus ? panes.first : nil, select: focus)
        Workspace.shared.changed()
        return ["layer_id": PluginID.make("layer", panes[0].id), "block_ids": ids(panes), "revision": revision]
    }

    // MARK: Focus and layout

    func focusBlock(_ params: [String: Any], _ context: Context) throws -> [String: Any] {
        guard let raw = params["block_id"] as? String else { throw Failure("bad_request", "block_id is required") }
        guard let located = findPane(raw) else { throw Failure("not_found", "no block \(raw)") }
        focus(located)
        return ["revision": revision]
    }

    func focus(_ located: Located) {
        let tab = located.tab
        if tab.zoomedID != nil, tab.zoomedID != located.pane.id { tab.zoomedID = nil }
        tab.raiseFloating(located.pane.id)
        tab.setFocus(located.pane.id)
        if let index = located.session.tabs.firstIndex(where: { $0 === tab }) { located.session.selectedIndex = index }
        if let controller = controller(for: located.session) {
            controller.pluginRefresh(tab: tab, focus: located.pane, select: true)
        }
        Workspace.shared.changed()
    }

    func focusWindow(_ params: [String: Any], _ context: Context) throws -> [String: Any] {
        let session = try session(params, context)
        let tab = try findTab(params["window_id"] as? String, in: session)
        guard let owner = owner(of: tab), let pane = tab.focusedPane else { throw Failure("not_found", "no window") }
        focus(Located(session: owner, tab: tab, pane: pane))
        return ["revision": revision]
    }

    func cycleFocus(_ params: [String: Any], _ context: Context, by step: Int) throws -> [String: Any] {
        let session = try session(params, context)
        guard let tab = session.selectedTab else { throw Failure("conflict", "the session has no windows") }
        let order = tab.allPanes
        guard let index = order.firstIndex(where: { $0.id == tab.focusedID }), order.count > 1 else { return ["revision": revision] }
        focus(Located(session: session, tab: tab, pane: order[(index + step + order.count) % order.count]))
        return ["revision": revision]
    }

    func cycleWindow(_ params: [String: Any], _ context: Context, by step: Int) throws -> [String: Any] {
        let session = try session(params, context)
        guard session.tabs.count > 1 else { return ["revision": revision] }
        let index = (session.selectedIndex + step + session.tabs.count) % session.tabs.count
        let tab = session.tabs[index]
        if let pane = tab.focusedPane { focus(Located(session: session, tab: tab, pane: pane)) }
        return ["revision": revision]
    }

    func setSessionLabel(_ params: [String: Any], _ context: Context) throws -> [String: Any] {
        let session = try session(params, context)
        guard let label = (params["label"] as? String)?.trimmingCharacters(in: .whitespaces), !label.isEmpty else {
            throw Failure("bad_request", "label is required")
        }
        session.name = Naming.uniqued(label, taken: Set(sessions.filter { $0 !== session }.map(\.name)))
        controller(for: session)?.pluginRefresh(tab: nil)
        Workspace.shared.changed()
        return ["revision": revision]
    }

    func setWindowLabel(_ params: [String: Any], _ context: Context) throws -> [String: Any] {
        let session = try session(params, context)
        let tab = try findTab(params["window_id"] as? String, in: session)
        let label = (params["label"] as? String)?.trimmingCharacters(in: .whitespaces) ?? ""
        tab.customName = label.isEmpty ? nil : label
        if let owner = owner(of: tab) { controller(for: owner)?.pluginRefresh(tab: nil) }
        Workspace.shared.changed()
        return ["revision": revision]
    }

    func setBlockLabel(_ params: [String: Any], _ context: Context) throws -> [String: Any] {
        let located = try resolvePane(params["block_id"] as? String, context)
        let label = (params["label"] as? String)?.trimmingCharacters(in: .whitespaces) ?? ""
        located.pane.customLabel = label.isEmpty ? nil : label
        controller(for: located.session)?.pluginRefresh(tab: nil)
        return ["revision": revision]
    }

    func closeWindow(_ params: [String: Any], _ context: Context) throws -> [String: Any] {
        let session = try session(params, context)
        let tab = try findTab(params["window_id"] as? String, in: session)
        guard let owner = owner(of: tab) else { throw Failure("not_found", "no window") }
        close(tab: tab, in: owner)
        return ["revision": revision]
    }

    func close(tab: TerminalTab, in session: TerminalSessionGroup) {
        if let controller = controller(for: session) {
            controller.pluginClose(tab: tab)
        } else {
            tab.allPanes.forEach { $0.terminate(); $0.removeFromSuperview() }
            session.remove(tab)
            if session.tabs.isEmpty { Workspace.shared.remove(session) } else { Workspace.shared.changed() }
        }
    }

    func close(_ located: Located) {
        if let controller = controller(for: located.session) {
            controller.pluginClose(pane: located.pane)
        } else if located.tab.allPanes.count <= 1 {
            close(tab: located.tab, in: located.session)
            return
        } else {
            located.tab.remove(located.pane.id)
            located.pane.terminate()
            located.pane.removeFromSuperview()
            Workspace.shared.changed()
        }
    }

    func closeLayer(_ params: [String: Any], _ context: Context) throws -> [String: Any] {
        guard let raw = params["layer_id"] as? String, let id = PluginID.uuid(raw, kind: "layer") else {
            throw Failure("bad_request", "layer_id is required")
        }
        for session in sessions {
            for tab in session.tabs where tab.layerOrder.contains(id) {
                for pane in tab.layerPanes(id).compactMap({ tab.panes[$0] }) { close(Located(session: session, tab: tab, pane: pane)) }
                return ["revision": revision]
            }
        }
        throw Failure("not_found", "no floating layer \(raw)")
    }

    func closeBlock(_ params: [String: Any], _ context: Context) throws -> [String: Any] {
        let located = try resolvePane(params["block_id"] as? String, context)
        close(located)
        return ["revision": revision]
    }

    func swapBlocks(_ params: [String: Any], _ context: Context) throws -> [String: Any] {
        let a = try resolvePane(params["block_id"] as? String, context)
        guard let other = (params["other_block_id"] ?? params["with_block_id"] ?? params["target_id"]) as? String,
              let b = findPane(other) else { throw Failure("bad_request", "other_block_id names no block") }
        guard a.tab === b.tab else { throw Failure("conflict", "the blocks are in different windows") }
        a.tab.tree = a.tab.tree.swapping(a.pane.id, b.pane.id)
        controller(for: a.session)?.pluginRefresh(tab: a.tab)
        Workspace.shared.changed()
        return ["revision": revision]
    }

    func floatBlock(_ params: [String: Any], _ context: Context) throws -> [String: Any] {
        let located = try resolvePane(params["block_id"] as? String, context)
        if located.tab.floating[located.pane.id] == nil { located.tab.toggleFloating(located.pane.id) }
        if let raw = params["bounds"] { located.tab.setBounds(try bounds(raw), ofLayer: located.tab.layerID(of: located.pane.id) ?? located.pane.id) }
        controller(for: located.session)?.pluginRefresh(tab: located.tab, focus: located.pane)
        Workspace.shared.changed()
        return ["layer_id": PluginID.make("layer", located.pane.id), "revision": revision]
    }

    func resizeLayer(_ params: [String: Any], _ context: Context) throws -> [String: Any] {
        guard let raw = params["layer_id"] as? String, let id = PluginID.uuid(raw, kind: "layer") else {
            throw Failure("bad_request", "layer_id is required")
        }
        let rect = try bounds(params["bounds"])
        for session in sessions {
            for tab in session.tabs where tab.layerOrder.contains(id) {
                tab.setBounds(rect, ofLayer: id)
                controller(for: session)?.pluginRefresh(tab: tab)
                Workspace.shared.changed()
                return ["revision": revision]
            }
        }
        throw Failure("not_found", "no floating layer \(raw)")
    }

    // MARK: Context

    /// What an action run from `controller`'s window sees as `ctx`.
    func context(for controller: WindowController?, origin: String) -> [String: Any] {
        var ctx: [String: Any] = ["origin": origin, "client_id": "client:termy-next"]
        guard let controller else { return ctx }
        let session = controller.session
        ctx["session_id"] = PluginID.make("session", session.id)
        if let tab = session.selectedTab {
            ctx["window_id"] = PluginID.make("window", tab.id)
            if let pane = tab.focusedPane { ctx["block_id"] = PluginID.make("block", pane.id) }
        }
        return ctx
    }
}
