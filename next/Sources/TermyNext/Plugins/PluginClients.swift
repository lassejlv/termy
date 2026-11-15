import AppKit
import Termy
import TermyNextCore

/// Who talks to the plugin API: the Termy Next app itself, and connections
/// that stay open (`rex events`, scripts that listen). Rex calls them clients.
@MainActor
enum PluginClients {
    struct Client {
        var id: String
        var kind: String
        var name: String
        var label: String?
        var connectedAt = Date()
        var sessions: Set<String> = []
        var attachedAll = false
    }

    /// The app, Rex's "app" client: it shows sessions and owns their panes.
    static let appClientID: String = {
        let key = "pluginAppClientID"
        if let id = UserDefaults.standard.string(forKey: key) { return id }
        let id = PluginID.make("client", UUID())
        UserDefaults.standard.set(id, forKey: key)
        return id
    }()

    static var connected: [String: Client] = [:]
    static var appLabel: String?

    static func callerID(_ context: PluginAPI.Context) -> String { context.clientID ?? "client:cli" }

    static func connect(_ client: Client) {
        connected[client.id] = client
        PluginEvents.emit("client_joined", ["client_id": client.id, "client": info(client)])
    }

    static func disconnect(_ id: String) {
        guard let client = connected.removeValue(forKey: id) else { return }
        for session in client.sessions { PluginEvents.emit("client_disconnected", ["client_id": id, "session_id": session]) }
        PluginEvents.emit("client_left", ["client_id": id])
        // What it owned goes back to the app.
        for session in Workspace.shared.sessions {
            for tab in session.tabs { for (pane, owner) in tab.owners where owner == id { tab.owners[pane] = nil } }
        }
    }

    static var app: Client {
        Client(id: appClientID, kind: "app", name: "Termy Next", label: appLabel,
               sessions: Set(AppDelegate.shared.controllers.map { PluginID.make("session", $0.session.id) }))
    }

    static var all: [Client] { [app] + connected.values.sorted { $0.connectedAt < $1.connectedAt } }

    static func info(_ client: Client) -> [String: Any] {
        var info: [String: Any] = [
            "client_id": client.id,
            "info": ["kind": client.kind, "platform": "macos", "name": client.name,
                     "version": Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "dev"],
            "connected_at": ISO8601DateFormatter().string(from: client.connectedAt),
            "connection_state": "connected",
        ]
        if let label = client.label { info["label"] = label }
        return info
    }

    /// A chord from the key object Rex's clients send (`key_g`, `mods`).
    static func chord(fromRexKey object: [String: Any]) -> KeyChord? {
        var key = object["key"] as? String ?? ""
        if key.hasPrefix("key_") { key = String(key.dropFirst(4)) }
        if key.hasPrefix("digit_") { key = String(key.dropFirst(6)) }
        if key.isEmpty, let code = object["unshifted_codepoint"] as? Int, let scalar = UnicodeScalar(code) { key = String(Character(scalar)) }
        let mods = (object["mods"] as? String ?? "").split(whereSeparator: { $0 == "+" || $0 == "," || $0 == " " }).map(String.init)
        return KeyChord((mods + [key]).joined(separator: "+"))
    }
}

// MARK: - Hosts

/// Other machines running Termy Next, which `rex.server(label)` and
/// `rex -S label` reach over ssh. Stored in hosts.json; config files can
/// declare more with `rex.host{…}`.
@MainActor
enum PluginHosts {
    static var file: URL { PluginPaths.paths.supportDirectory.appendingPathComponent("hosts.json") }

    private struct Stored: Codable {
        var hosts: [[String: AnyCodable]] = []
        var removed: [String] = []
    }

    private static func load() -> Stored {
        guard let data = try? Data(contentsOf: file), let stored = try? JSONDecoder().decode(Stored.self, from: data) else { return Stored() }
        return stored
    }

    private static func save(_ stored: Stored) {
        try? FileManager.default.createDirectory(at: file.deletingLastPathComponent(), withIntermediateDirectories: true)
        if let data = try? JSONEncoder().encode(stored) { try? data.write(to: file, options: .atomic) }
    }

    /// Every host: those stored, then those the config declares that weren't replaced or removed.
    static func list() -> [[String: Any]] {
        let stored = load()
        var hosts = stored.hosts.map { $0.mapValues(\.value) }
        let ids = Set(hosts.compactMap { $0["id"] as? String })
        for declared in PluginHost.shared.snapshot.declaredHosts {
            guard let id = declared["id"] as? String, !ids.contains(id), !stored.removed.contains(id) else { continue }
            hosts.append(declared)
        }
        return hosts
    }

    static var hash: String {
        let data = (try? JSONSerialization.data(withJSONObject: list(), options: [.sortedKeys])) ?? Data()
        var value: UInt64 = 0xcbf2_9ce4_8422_2325
        for byte in data { value = (value ^ UInt64(byte)) &* 0x100_0000_01b3 }
        return String(format: "%016llx", value)
    }

    static func put(_ host: [String: Any]) throws -> [String: Any] {
        var host = host
        guard let label = host["label"] as? String, !label.isEmpty, label.count <= 64, label.trimmingCharacters(in: .whitespaces) == label else {
            throw PluginAPI.Failure("bad_request", "host.label can't be empty, start or end with white space, or pass 64 characters")
        }
        guard let endpoints = host["endpoints"] as? [String], !endpoints.isEmpty else {
            throw PluginAPI.Failure("bad_request", "host.endpoints needs at least one endpoint, such as user@machine")
        }
        if endpoints.contains(where: { $0.hasPrefix("/") || $0.hasPrefix("unix:") }) {
            throw PluginAPI.Failure("bad_request", "host.endpoints can't be Unix sockets")
        }
        let id = (host["id"] as? String).flatMap { $0.isEmpty ? nil : $0 } ?? PluginID.make("host", UUID())
        if let other = list().first(where: { ($0["label"] as? String)?.lowercased() == label.lowercased() && $0["id"] as? String != id }) {
            throw PluginAPI.Failure("conflict", "host \(other["id"] ?? "") is already labeled \(label)")
        }
        host["id"] = id
        host["source"] = nil
        host["updated_at"] = ISO8601DateFormatter().string(from: Date())
        var stored = load()
        let before = list()
        stored.hosts.removeAll { $0["id"]?.value as? String == id }
        stored.hosts.append(host.mapValues(AnyCodable.init))
        stored.removed.removeAll { $0 == id }
        save(stored)
        if !NSDictionary(dictionary: ["h": before]).isEqual(to: ["h": list()]) {
            PluginEvents.emit("hosts_changed", ["hash": hash])
        }
        return ["host": host, "hash": hash]
    }

    static func remove(_ id: String) -> [String: Any] {
        var stored = load()
        let existed = list().contains { $0["id"] as? String == id }
        stored.hosts.removeAll { $0["id"]?.value as? String == id }
        if PluginHost.shared.snapshot.declaredHosts.contains(where: { $0["id"] as? String == id }), !stored.removed.contains(id) {
            stored.removed.append(id)
        }
        save(stored)
        if existed { PluginEvents.emit("hosts_changed", ["hash": hash]) }
        return ["removed": existed, "hash": hash]
    }

    /// The host labeled or identified `name`.
    static func find(_ name: String) -> [String: Any]? {
        list().first { ($0["label"] as? String)?.lowercased() == name.lowercased() || $0["id"] as? String == name }
    }
}

/// Any JSON value, for storing hosts with fields this app doesn't know.
struct AnyCodable: Codable {
    let value: Any

    init(_ value: Any) { self.value = value }

    init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() { value = NSNull() } else if let bool = try? container.decode(Bool.self) { value = bool }
        else if let int = try? container.decode(Int.self) { value = int } else if let double = try? container.decode(Double.self) { value = double }
        else if let string = try? container.decode(String.self) { value = string }
        else if let array = try? container.decode([AnyCodable].self) { value = array.map(\.value) }
        else { value = try container.decode([String: AnyCodable].self).mapValues(\.value) }
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        switch value {
        case let number as NSNumber where CFGetTypeID(number) == CFBooleanGetTypeID(): try container.encode(number.boolValue)
        case let int as Int: try container.encode(int)
        case let double as Double: try container.encode(double)
        case let string as String: try container.encode(string)
        case let array as [Any]: try container.encode(array.map(AnyCodable.init))
        case let dict as [String: Any]: try container.encode(dict.mapValues(AnyCodable.init))
        default: try container.encodeNil()
        }
    }
}

// MARK: - Events

/// Events plugins hear with `termy.on` / `rex.on`, and `rex events`
/// prints. Changes to sessions, tabs and panes are found by comparing
/// before and after, so they're reported whatever made them.
@MainActor
enum PluginEvents {
    private static var seq: UInt64 = 0

    private struct SessionState: Equatable {
        var label: String
        var order: Int
        var windows: [UUID: String]
        var windowOrder: [UUID]
        var active: UUID?
        var blocks: Set<UUID>
    }

    private static var last: [UUID: SessionState]?

    static func emit(_ type: String, _ payload: [String: Any]) {
        seq += 1
        var event = payload
        event["type"] = type
        event["seq"] = seq
        event["time"] = ISO8601DateFormatter().string(from: Date())
        PluginHost.shared.dispatch(event: event)
        PluginServer.shared.broadcast(event)
    }

    static var listening: Bool { PluginHost.shared.hasHandlers || PluginServer.shared.hasSubscribers }

    static func blockEvent(_ pane: PaneView, _ name: String, _ data: [String: Any]) {
        guard listening else { return }
        var event: [String: Any] = ["block_id": PluginID.make("block", pane.id), "event": name, "data": data]
        if let found = PluginAPI.shared.findAnyPane(PluginID.make("block", pane.id)) {
            event["session_id"] = PluginID.make("session", found.session.id)
            if let tab = found.tab { event["window_id"] = PluginID.make("window", tab.id) }
        }
        emit("block_event", event)
    }

    private static func snapshot() -> [UUID: SessionState] {
        var states: [UUID: SessionState] = [:]
        for (order, session) in Workspace.shared.sessions.enumerated() {
            states[session.id] = SessionState(
                label: session.name, order: order,
                windows: Dictionary(uniqueKeysWithValues: session.tabs.map { ($0.id, $0.customName ?? $0.title) }),
                windowOrder: session.tabs.map(\.id), active: session.selectedTab?.id,
                blocks: Set(session.tabs.flatMap { $0.allPanes.map(\.id) } + session.detached.map(\.id)))
        }
        return states
    }

    /// Compares the workspace with how it last was and reports the difference.
    static func workspaceChanged() {
        PluginIdentity.rememberTabs()
        let now = snapshot()
        defer { last = now }
        guard let before = last else { return }
        let revision = PluginAPI.shared.revision
        for (id, state) in now {
            let session = PluginID.make("session", id)
            guard let old = before[id] else {
                emit("session_created", ["session_id": session, "label": state.label])
                for block in state.blocks { emit("block_created", ["session_id": session, "block_id": PluginID.make("block", block)]) }
                emit("session_view_changed", ["session_id": session, "revision": revision])
                continue
            }
            guard old != state else { continue }
            if old.label != state.label { emit("session_label_changed", ["session_id": session, "label": state.label]) }
            if old.order != state.order { emit("session_moved", ["session_id": session, "index": state.order]) }
            for (window, label) in state.windows where old.windows[window] == nil {
                emit("window_created", ["session_id": session, "window_id": PluginID.make("window", window), "label": label])
            }
            for (window, label) in state.windows where old.windows[window] != nil && old.windows[window] != label {
                emit("window_label_changed", ["session_id": session, "window_id": PluginID.make("window", window), "label": label])
            }
            for window in old.windows.keys where state.windows[window] == nil {
                emit("window_closed", ["session_id": session, "window_id": PluginID.make("window", window)])
            }
            if old.active != state.active, let active = state.active {
                emit("active_window_changed", ["session_id": session, "window_id": PluginID.make("window", active)])
            }
            for block in state.blocks.subtracting(old.blocks) {
                emit("block_created", ["session_id": session, "block_id": PluginID.make("block", block)])
            }
            for block in old.blocks.subtracting(state.blocks) {
                emit("block_closed", ["session_id": session, "block_id": PluginID.make("block", block)])
            }
            emit("session_view_changed", ["session_id": session, "revision": revision])
        }
        for (id, old) in before where now[id] == nil {
            let session = PluginID.make("session", id)
            for block in old.blocks { emit("block_closed", ["session_id": session, "block_id": PluginID.make("block", block)]) }
            emit("session_destroyed", ["session_id": session])
        }
    }
}

// MARK: - Methods

extension PluginAPI {
    /// Sessions' layout methods beyond creating and closing.
    func layoutCall(_ method: String, _ params: [String: Any], _ context: Context) throws -> Any? {
        switch method {
        case "session.attach":
            let session = try session(params, context)
            PluginClients.connected[PluginClients.callerID(context)]?.sessions.insert(PluginID.make("session", session.id))
            return ["client_id": PluginClients.callerID(context), "view": view(session), "seq": 0]
        case "session.attach_all":
            if let id = context.clientID { PluginClients.connected[id]?.attachedAll = true }
            return ["sessions": (sessionList()["sessions"] as? [Any]) ?? []]
        case "session.bootstrap":
            var attached: [[String: Any]] = []
            for raw in params["attach_session_ids"] as? [String] ?? [] { if let s = findSession(raw) { attached.append(view(s)) } }
            var result: [String: Any] = [
                "client_id": PluginClients.callerID(context), "server_status": status(context),
                "attach_all": ["sessions": (sessionList()["sessions"] as? [Any]) ?? []],
                "attached_sessions": attached, "data_tickets": [Any](),
            ]
            if let preferred = (params["preferred_session_id"] as? String).flatMap(findSession) ?? (try? session([:], context)) {
                result["selected_session"] = view(preferred)
            }
            return result
        case "session.detach":
            let session = try session(params, context)
            PluginClients.connected[PluginClients.callerID(context)]?.sessions.remove(PluginID.make("session", session.id))
            return [String: Any]()
        case "session.list_clients":
            let session = try session(params, context)
            let id = PluginID.make("session", session.id)
            return ["clients": PluginClients.all.filter { $0.sessions.contains(id) || $0.attachedAll }.map(PluginClients.info)]
        case "session.describe_block":
            let target = try resolveAnyPane(params["block_id"] as? String, context)
            var block = blockInfo(target.pane, tab: target.tab)
            block["session_id"] = PluginID.make("session", target.session.id)
            return ["block": block, "api": blockAPI()]
        case "session.new_block": return try newBlock(params, context)
        case "session.detach_block": return try detachBlock(params, context)
        case "session.move_block": return try moveBlock(params, context)
        case "session.own_block", "session.disown_block": return try own(params, context, take: method == "session.own_block")
        case "session.raise_layer", "session.lower_layer": return try stackLayer(params, context, raise: method == "session.raise_layer")
        case "session.set_layer_state": return try setLayerState(params, context)
        case "session.move":
            let session = try session(params, context)
            let before = (params["before_session_id"] as? String).flatMap(findSession)
            Workspace.shared.move(session, before: before)
            return [String: Any]()
        case "session.move_window":
            let session = try session(params, context)
            let tab = try findTab(params["window_id"] as? String, in: session)
            guard let from = session.tabs.firstIndex(where: { $0 === tab }) else { throw Failure("not_found", "no window") }
            var to = session.tabs.count - 1
            if let raw = params["before_window_id"] as? String {
                let before = try findTab(raw, in: session)
                to = session.tabs.firstIndex { $0 === before } ?? to
                if to > from { to -= 1 }
            }
            let selected = session.selectedTab
            session.move(from: from, to: to)
            if let selected { session.selectedIndex = session.tabs.firstIndex { $0 === selected } ?? session.selectedIndex }
            controller(for: session)?.pluginRefresh(tab: nil)
            Workspace.shared.changed()
            return ["revision": revision]
        case "session.resize_pane": return try resizePane(params, context)
        case "session.resize_block":
            let located = try resolvePane(params["block_id"] as? String, context)
            let tab = located.tab
            for (axis, key) in [(SplitAxis.horizontal, "width"), (.vertical, "height")] {
                guard let share = (params[key] as? NSNumber).map({ CGFloat($0.doubleValue) }) else { continue }
                if let tree = tab.tree.resizing(located.pane.id, axis: axis, share: share) {
                    tab.tree = tree
                } else if let layer = tab.layerID(of: located.pane.id), var rect = tab.bounds(ofLayer: layer) {
                    if axis == .horizontal { rect.size.width = share } else { rect.size.height = share }
                    tab.setBounds(rect, ofLayer: layer)
                } else {
                    throw Failure("conflict", "the block has no \(key) to change: nothing sits beside it that way")
                }
            }
            controller(for: located.session)?.pluginRefresh(tab: tab)
            Workspace.shared.changed()
            return ["revision": revision]
        case "session.zoom_block": return try zoom(params, context)
        case "session.focus_direction": return try focusDirection(params, context)
        default: return nil
        }
    }

    private func blockAPI() -> [String: Any] {
        [
            "methods": Self.blockMethods.map { ["name": $0, "full_name": PluginFlavor.terminalCreator + "." + $0, "description": Self.describeBlockMethod($0)] },
            "events": ["program_status", "title_changed", "child_exited", "selection_copied"],
            "actions": [Any](),
        ]
    }

    private func newBlock(_ params: [String: Any], _ context: Context) throws -> [String: Any] {
        let session = try session(params, context)
        var block: [String: Any] = ["flavor": params["flavor"] as? String ?? PluginFlavor.shell]
        if let options = params["options"] { block["options"] = options }
        if let label = params["label"] { block["label"] = label }
        let layout = try layout(["layout": ["block": block]])
        let panes = try makePanes(layout, session: session, near: session.selectedTab?.focusedPane)
        session.detached.append(contentsOf: panes)
        for pane in panes { detachedOwner[pane.id] = PluginClients.callerID(context) }
        Workspace.shared.changed()
        return ["block_id": PluginID.make("block", panes[0].id), "revision": revision]
    }

    private func detachBlock(_ params: [String: Any], _ context: Context) throws -> [String: Any] {
        let located = try resolvePane(params["block_id"] as? String, context)
        guard located.session.tabs.flatMap(\.allPanes).count > 1 else {
            throw Failure("conflict", "the session's last pane in a window can't be detached")
        }
        let owner = located.tab.owners[located.pane.id]
        AppDelegate.shared.detachPane(located.pane)
        located.pane.isFloating = false
        located.session.detached.append(located.pane)
        detachedOwner[located.pane.id] = owner
        Workspace.shared.changed()
        return ["revision": revision]
    }

    private func moveBlock(_ params: [String: Any], _ context: Context) throws -> [String: Any] {
        guard let raw = params["block_id"] as? String, let moving = findAnyPane(raw) else { throw Failure("not_found", "no block") }
        guard let anchorRaw = params["anchor_block_id"] as? String, let anchor = findPane(anchorRaw) else {
            throw Failure("not_found", "anchor_block_id names no block in a window")
        }
        guard moving.pane !== anchor.pane else { throw Failure("bad_request", "a block can't move beside itself") }
        let horizontal = (params["direction"] as? String ?? "horizontal") == "horizontal"
        let after = (params["side"] as? String ?? "after") != "before"
        let ratio = CGFloat((params["ratio"] as? NSNumber)?.doubleValue ?? 0.5)
        guard ratio > 0, ratio < 1 else { throw Failure("bad_request", "ratio must be between 0 and 1") }
        let removedSplit = moving.tab.flatMap { tab in tab.tree.splitKeys().first { tab.tree.replacingSplit($0) { $0 }?.leaves.contains(moving.pane.id) == true } }
        // Out of where it is now, without ending it.
        var owner: String?
        if let index = moving.session.detached.firstIndex(where: { $0 === moving.pane }) {
            moving.session.detached.remove(at: index)
            owner = detachedOwner.removeValue(forKey: moving.pane.id)
        } else {
            owner = moving.tab?.owners[moving.pane.id]
            AppDelegate.shared.detachPane(moving.pane)
        }
        let direction: Direction = horizontal ? (after ? .right : .left) : (after ? .down : .up)
        let tab = anchor.tab
        if tab.floating[anchor.pane.id] != nil {
            tab.insert(.leaf(moving.pane.id), panes: [moving.pane], inLayerAt: anchor.pane.id, direction: direction, ratio: ratio)
        } else {
            moving.pane.isFloating = false
            tab.insert(.leaf(moving.pane.id), panes: [moving.pane], at: anchor.pane.id, direction: direction, ratio: ratio)
        }
        if let owner { tab.owners[moving.pane.id] = owner }
        controller(for: anchor.session)?.pluginRefresh(tab: tab, appearing: [moving.pane])
        Workspace.shared.changed()
        var result: [String: Any] = ["created_split_id": PluginID.make("split", after ? moving.pane.id : anchor.pane.id), "revision": revision]
        if removedSplit != nil { result["removed_split_id"] = PluginID.make("split", removedSplit!) }
        return result
    }

    private func own(_ params: [String: Any], _ context: Context, take: Bool) throws -> [String: Any] {
        let session = try session(params, context)
        let caller = PluginClients.callerID(context)
        let mode = params["mode"] as? String ?? "request"
        guard !take || ["take", "request"].contains(mode) else { throw Failure("bad_request", "mode must be take or request") }
        var ids = params["block_ids"] as? [String] ?? []
        if ids.isEmpty {
            ids = (session.tabs.flatMap(\.allPanes) + session.detached).map { PluginID.make("block", $0.id) }.sorted()
        }
        var seen = Set<String>()
        var results: [[String: Any]] = []
        let targets = try ids.map { raw -> (String, TerminalTab?, PaneView) in
            guard let found = findAnyPane(raw), found.session === session else { throw Failure("bad_request", "\(raw) isn't a block in the session") }
            return (PluginID.make("block", found.pane.id), found.tab, found.pane)
        }
        for (id, tab, pane) in targets where seen.insert(id).inserted {
            let before = tab?.owners[pane.id] ?? detachedOwner[pane.id]
            let now: String? = take ? caller : (before == caller ? nil : before)
            if let tab { tab.owners[pane.id] = now } else { detachedOwner[pane.id] = now }
            if before != now {
                PluginEvents.emit("block_owner_changed", ["session_id": PluginID.make("session", session.id), "block_id": id,
                                                           "owner": now ?? PluginClients.appClientID])
            }
            results.append(["block_id": id, "owner": now ?? PluginClients.appClientID])
        }
        Workspace.shared.changed()
        return ["blocks": results]
    }

    private func layer(_ params: [String: Any], key: String = "layer_id", _ session: TerminalSessionGroup) throws -> (TerminalTab, UUID)? {
        guard let raw = params[key] as? String else { return nil }
        guard let id = PluginID.uuid(raw, kind: "layer") else { throw Failure("bad_request", "\(key) is malformed") }
        for tab in session.tabs {
            if tab.id == id { return (tab, id) }
            if tab.layerOrder.contains(id) { return (tab, id) }
        }
        throw Failure("not_found", "no layer \(raw)")
    }

    private func stackLayer(_ params: [String: Any], _ context: Context, raise: Bool) throws -> [String: Any] {
        let session = try session(params, context)
        guard let (tab, id) = try layer(params, session) else { throw Failure("bad_request", "layer_id is required") }
        guard id != tab.id else { throw Failure("bad_request", "the tiled layer doesn't stack") }
        let other = try layer(params, key: "relative_layer_id", session)
        if let other, other.1 == id { throw Failure("bad_request", "relative_layer_id is the same as layer_id") }
        if raise { tab.raiseLayer(id, above: other?.1) } else { tab.lowerLayer(id, below: other?.1) }
        controller(for: session)?.pluginRefresh(tab: tab)
        Workspace.shared.changed()
        return ["revision": revision]
    }

    private func setLayerState(_ params: [String: Any], _ context: Context) throws -> [String: Any] {
        let session = try session(params, context)
        guard let (tab, id) = try layer(params, session) else { throw Failure("bad_request", "layer_id is required") }
        let state = params["state"] as? String ?? "normal"
        guard ["normal", "minimized"].contains(state) else { throw Failure("bad_request", "state must be normal or minimized") }
        let focusing = params["focus"] as? Bool ?? false
        let tiled = id == tab.id
        if state == "minimized" {
            guard !tiled else { throw Failure("bad_request", "the window's layout can't be minimized") }
            guard !focusing else { throw Failure("bad_request", "focus can't go with minimized") }
            guard tab.minimizedLayers.insert(id).inserted else { return ["revision": revision] }
            if tab.isMinimized(tab.focusedID), let next = tab.tree.leaves.first { tab.setFocus(next) }
            tab.zoomedID = nil
        } else {
            tab.minimizedLayers.remove(id)
            if focusing {
                let members = tiled ? tab.tree.leaves : tab.layerPanes(id)
                guard let first = members.first, let pane = tab.panes[first] else { throw Failure("bad_request", "the layer has no blocks") }
                if !tiled { tab.raiseLayer(id) }
                focus(Located(session: session, tab: tab, pane: pane))
                return ["revision": revision]
            }
        }
        controller(for: session)?.pluginRefresh(tab: tab, focus: tab.focusedPane)
        Workspace.shared.changed()
        return ["revision": revision]
    }

    private func resizePane(_ params: [String: Any], _ context: Context) throws -> [String: Any] {
        guard let beforeRaw = params["before_block_id"] as? String, let before = findPane(beforeRaw),
              let afterRaw = params["after_block_id"] as? String, let after = findPane(afterRaw), before.tab === after.tab
        else { throw Failure("not_found", "before_block_id and after_block_id must be blocks in one window") }
        let ratio = CGFloat((params["ratio"] as? NSNumber)?.doubleValue ?? 0.5)
        guard ratio > 0, ratio < 1 else { throw Failure("bad_request", "ratio must be between 0 and 1") }
        let axis: SplitAxis = (params["direction"] as? String ?? "horizontal") == "horizontal" ? .horizontal : .vertical
        let tab = before.tab
        if let tree = tab.tree.settingRatio(ratio, between: before.pane.id, and: after.pane.id, axis: axis) {
            tab.tree = tree
        } else if let layerID = tab.layerID(of: before.pane.id), let tree = tab.layerTree(layerID),
                  let changed = tree.settingRatio(ratio, between: before.pane.id, and: after.pane.id, axis: axis),
                  let bounds = tab.bounds(ofLayer: layerID) {
            tab.addLayer(layerID, tree: changed, panes: [], bounds: bounds)
        } else {
            throw Failure("not_found", "no \(axis == .horizontal ? "horizontal" : "vertical") border between those blocks")
        }
        controller(for: before.session)?.pluginRefresh(tab: tab)
        Workspace.shared.changed()
        return ["revision": revision]
    }

    private func zoom(_ params: [String: Any], _ context: Context) throws -> [String: Any] {
        let located: Located
        if params["block_id"] == nil, let window = params["window_id"] as? String {
            let session = try session(params, context)
            let tab = try findTab(window, in: session)
            guard let pane = tab.focusedPane else { throw Failure("conflict", "the window has no focused block") }
            located = Located(session: session, tab: tab, pane: pane)
        } else {
            located = try resolvePane(params["block_id"] as? String, context)
        }
        let tab = located.tab
        let wanted = (params["zoom"] as? Bool) ?? (params["zoomed"] as? Bool) ?? (tab.zoomedID != located.pane.id)
        tab.zoomedID = wanted ? located.pane.id : nil
        tab.setFocus(located.pane.id)
        controller(for: located.session)?.pluginRefresh(tab: tab, focus: located.pane)
        Workspace.shared.changed()
        return ["block_id": PluginID.make("block", located.pane.id), "zoomed": wanted, "revision": revision]
    }

    private func focusDirection(_ params: [String: Any], _ context: Context) throws -> [String: Any] {
        let session = try session(params, context)
        let direction: Direction = switch params["direction"] as? String ?? "" {
        case "left": .left
        case "right": .right
        case "up", "above": .up
        case "down", "below": .down
        default: throw Failure("bad_request", "direction must be left, right, up or down")
        }
        guard let tab = session.selectedTab else { throw Failure("conflict", "the session has no windows") }
        let frames = controller(for: session)?.pluginUnitFrames(of: tab) ?? tab.tree.unitRects()
        var target = SplitNavigation.neighbor(of: tab.focusedID, direction: direction, frames: frames)
        if target == nil, params["wrap"] as? Bool == true, let current = frames[tab.focusedID] {
            // Around to the far side, in the same row or column.
            let candidates = frames.filter { $0.key != tab.focusedID }.filter { _, rect in
                direction.axis == .horizontal ? rect.maxY > current.minY && rect.minY < current.maxY
                    : rect.maxX > current.minX && rect.minX < current.maxX
            }
            target = candidates.min { a, b in
                switch direction {
                case .right: a.value.minX < b.value.minX
                case .left: a.value.maxX > b.value.maxX
                case .down: a.value.minY < b.value.minY
                case .up: a.value.maxY > b.value.maxY
                }
            }?.key
        }
        if let target, let pane = tab.panes[target] { focus(Located(session: session, tab: tab, pane: pane)) }
        return ["revision": revision]
    }

    // MARK: Clients, hosts, server

    func clientCall(_ method: String, _ params: [String: Any], _ context: Context) throws -> Any? {
        switch method {
        case "client.list": return ["clients": PluginClients.all.map(PluginClients.info)]
        case "client.inspect":
            let id = params["client_id"] as? String ?? PluginClients.appClientID
            guard let client = PluginClients.all.first(where: { $0.id == id || $0.label == id }) else {
                throw Failure("not_found", "no client \(id)")
            }
            let actions = client.kind == "app" ? PluginClientActions.catalog() : []
            return ["client": PluginClients.info(client), "actions": actions]
        case "client.set_label":
            let label = (params["label"] as? String).flatMap { $0.isEmpty ? nil : $0 }
            let caller = PluginClients.callerID(context)
            if caller == PluginClients.appClientID { PluginClients.appLabel = label } else { PluginClients.connected[caller]?.label = label }
            PluginEvents.emit("client_changed", ["client_id": caller])
            return [String: Any]()
        case "client.reply":
            throw Failure("not_found", "no call \(params["call_id"] as? String ?? "") is waiting for a reply")
        case "hosts.list": return ["hosts": PluginHosts.list(), "hash": PluginHosts.hash, "diagnostics": [Any]()]
        case "hosts.put":
            guard let host = params["host"] as? [String: Any] else { throw Failure("bad_request", "host is required") }
            return try PluginHosts.put(host)
        case "hosts.remove":
            guard let raw = params["id"] as? String else { throw Failure("bad_request", "id is required") }
            return PluginHosts.remove(PluginHosts.find(raw)?["id"] as? String ?? raw)
        case "server.status": return status(context)
        case "server.version":
            return ["version": Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "dev",
                    "build": Bundle.main.infoDictionary?["CFBundleVersion"] as? String ?? "dev", "name": "Termy Next"]
        case "server.whoami":
            var result: [String: Any] = [
                "principal": ["kind": "peer", "transport": "unix", "display": "uid \(getuid())", "uid": Int(getuid()), "gid": Int(getgid())],
                "account": NSUserName(),
            ]
            if let location = location(context) { result["location"] = location }
            return result
        case "server.stop":
            let idle = params["if_idle"] as? Bool ?? false
            if idle, !AppDelegate.shared.controllers.isEmpty || !PluginClients.connected.isEmpty {
                return ["stopped": false]
            }
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) { NSApp.terminate(nil) }
            return ["stopped": true]
        case "server.tailscale.enable", "server.tailscale.disable", "server.tailscale.login", "server.tailscale.logout", "server.tailscale.rename":
            throw Failure("not_supported", "Termy Next has no network server of its own; reach other Macs with hosts over ssh")
        case "help.all": return Self.helpAll()
        case "help.describe", "session.help":
            if method == "session.help" { return Self.helpAll() }
            guard let name = params["method"] as? String, let entry = (Self.helpAll()["methods"] as? [[String: Any]])?.first(where: { $0["name"] as? String == name })
            else { throw Failure("not_found", "no method \(params["method"] as? String ?? "")") }
            return entry
        case "target.resolve": return try resolveTargets(params, context)
        case "target.complete": return completeTargets(params, context)
        default: return nil
        }
    }

    func status(_ context: Context) -> [String: Any] {
        [
            "name": "Termy Next",
            "version": Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "dev",
            "instance_id": PluginAPI.hostID,
            "socket": PluginPaths.socket,
            "session_count": Workspace.shared.sessions.count,
            "client_count": PluginClients.all.count,
            "config": ["paths": PluginHost.shared.configFiles.map(\.path), "generation": PluginHost.shared.snapshot.generation,
                       "diagnostics": PluginHost.shared.snapshot.diagnostics],
            "tailscale": ["enabled": false, "supported": false],
        ]
    }

    func location(_ context: Context) -> [String: Any]? {
        guard let found = findAnyPane(context.callerBlockID) else { return nil }
        var location: [String: Any] = ["session_id": PluginID.make("session", found.session.id), "block_id": PluginID.make("block", found.pane.id)]
        if let tab = found.tab { location["window_id"] = PluginID.make("window", tab.id) }
        return location
    }

    // MARK: Targets

    private struct Candidate {
        var kind: String
        var info: [String: Any]
        var id: String
        var label: String
        var position: Int?
    }

    private func candidates(kinds: [String], sessions scope: [TerminalSessionGroup], window: TerminalTab?, many: Bool) -> [Candidate] {
        var out: [Candidate] = []
        for session in scope {
            let sessionID = PluginID.make("session", session.id)
            if kinds.contains("session") {
                out.append(Candidate(kind: "session", info: ["kind": "session", "session_id": sessionID, "label": session.name],
                                     id: sessionID, label: session.name))
            }
            for (index, tab) in session.tabs.enumerated() where window == nil || window === tab {
                let windowID = PluginID.make("window", tab.id)
                if kinds.contains("window") {
                    out.append(Candidate(kind: "window", info: ["kind": "window", "session_id": sessionID, "window_id": windowID,
                                                                 "label": tab.customName ?? tab.title, "where": many ? "in \(session.name)" : "window \(index + 1)"],
                                         id: windowID, label: tab.customName ?? tab.title, position: index + 1))
                }
                if kinds.contains("block") {
                    for pane in tab.allPanes {
                        let blockID = PluginID.make("block", pane.id)
                        out.append(Candidate(kind: "block", info: ["kind": "block", "session_id": sessionID, "window_id": windowID, "block_id": blockID,
                                                                    "label": pane.customLabel ?? pane.label.name,
                                                                    "where": "window \(index + 1)" + (many ? " of \(session.name)" : "")],
                                             id: blockID, label: pane.customLabel ?? pane.label.name))
                    }
                }
            }
            if kinds.contains("block"), window == nil {
                for pane in session.detached {
                    let blockID = PluginID.make("block", pane.id)
                    out.append(Candidate(kind: "block", info: ["kind": "block", "session_id": sessionID, "block_id": blockID,
                                                                "label": pane.customLabel ?? pane.label.name, "where": "detached" + (many ? " in \(session.name)" : "")],
                                         id: blockID, label: pane.customLabel ?? pane.label.name))
                }
            }
        }
        return out
    }

    private func resolveTargets(_ params: [String: Any], _ context: Context) throws -> [String: Any] {
        let kinds = params["kinds"] as? [String] ?? []
        let known = ["session", "window", "block", "client"]
        guard !kinds.isEmpty, Set(kinds).count == kinds.count, kinds.allSatisfy(known.contains),
              !kinds.contains("client") || kinds == ["client"]
        else { throw Failure("bad_request", "kinds must list session, window, block, or client on its own, each once") }
        let queries = params["queries"] as? [String] ?? []
        guard !queries.isEmpty else { throw Failure("bad_request", "send at least one query") }
        let ctx = params["context"] as? [String: Any] ?? [:]
        let scope = params["scope"] as? [String: Any]
        if kinds.contains("session"), scope != nil { throw Failure("bad_request", "scope doesn't go with kind session") }

        if kinds == ["client"] {
            return ["results": queries.map { query -> [String: Any] in
                let q = query.trimmingCharacters(in: .whitespaces)
                let clients = PluginClients.all
                let matches = q.isEmpty ? clients.filter { $0.kind == "app" }
                    : clients.filter { $0.id == q } .isEmpty == false ? clients.filter { $0.id == q }
                    : clients.filter { $0.label?.lowercased() == q.lowercased() }.isEmpty == false ? clients.filter { $0.label?.lowercased() == q.lowercased() }
                    : clients.filter { q.count >= 4 && $0.id.lowercased().hasSuffix(q.lowercased()) }
                return result(query, matches.map { ["kind": "client", "client_id": $0.id, "label": $0.label ?? $0.name] })
            }]
        }

        var searched = Workspace.shared.sessions
        var window: TerminalTab?
        if let scope {
            if let name = scope["session"] as? String {
                let matches = Workspace.shared.sessions.filter { $0.name.lowercased() == name.lowercased() || PluginID.make("session", $0.id) == name }
                guard matches.count == 1 else {
                    let status = matches.isEmpty ? "not_found" : "ambiguous"
                    return ["results": queries.map { ["query": $0, "status": status, "reason": "scope.session"] }]
                }
                searched = matches
            }
            if let name = scope["window"] as? String {
                guard kinds == ["block"] else { throw Failure("bad_request", "scope.window goes only with kind block") }
                window = searched.compactMap { try? findTab(name, in: $0) }.first
                if window == nil { return ["results": queries.map { ["query": $0, "status": "not_found", "reason": "scope.window"] }] }
            }
        } else if let raw = ctx["session_id"] as? String, let session = findSession(raw), !kinds.contains("session") || kinds.count > 1 {
            searched = [session]
        }
        let many = searched.count > 1
        let all = candidates(kinds: kinds, sessions: searched, window: window, many: many)
        let contextBlock = (ctx["block_id"] as? String).flatMap(findPane)

        let results = queries.map { query -> [String: Any] in
            let q = query.trimmingCharacters(in: .whitespaces)
            if q.isEmpty {
                // The default: the context's block, window or session.
                let kind = ["block", "window", "session"].first(where: kinds.contains) ?? kinds[0]
                switch kind {
                case "block":
                    if window == nil, let contextBlock, searched.contains(where: { $0 === contextBlock.session }) {
                        return result(query, all.filter { $0.id == PluginID.make("block", contextBlock.pane.id) }.map(\.info))
                    }
                    let tabs = window.map { [$0] } ?? searched.compactMap(\.selectedTab)
                    return result(query, tabs.compactMap { $0.focusedPane }.compactMap { pane in
                        all.first { $0.id == PluginID.make("block", pane.id) }?.info
                    })
                case "window":
                    if many { return result(query, searched.map { ["kind": "session", "session_id": PluginID.make("session", $0.id), "label": $0.name] }) }
                    let tab = contextBlock.map(\.tab) ?? searched.first?.selectedTab
                    return result(query, all.filter { $0.kind == "window" && $0.id == tab.map { PluginID.make("window", $0.id) } }.map(\.info))
                default:
                    if let raw = ctx["session_id"] as? String, let session = findSession(raw) {
                        return result(query, all.filter { $0.id == PluginID.make("session", session.id) }.map(\.info))
                    }
                    return result(query, all.filter { $0.kind == "session" }.map(\.info))
                }
            }
            if q.range(of: #"^(session|window|block|layer|split):"#, options: .regularExpression) != nil {
                return result(query, all.filter { $0.id == q.lowercased() }.map(\.info))
            }
            let steps: [(Candidate) -> Bool] = [
                { $0.id == q.lowercased() },
                { $0.label.lowercased() == q.lowercased() },
                { c in Int(q).map { c.position == $0 } ?? false },
                { c in (4...32).contains(q.count) && q.allSatisfy(\.isHexDigit) && c.id.replacingOccurrences(of: "-", with: "").hasSuffix(q.lowercased()) },
            ]
            for step in steps {
                let matches = all.filter(step)
                if !matches.isEmpty { return result(query, matches.map(\.info)) }
            }
            return ["query": query, "status": "not_found"]
        }
        return ["results": results]
    }

    private func result(_ query: String, _ matches: [[String: Any]]) -> [String: Any] {
        switch matches.count {
        case 0: return ["query": query, "status": "not_found"]
        case 1: return ["query": query, "status": "resolved", "target": matches[0]]
        default: return ["query": query, "status": "ambiguous", "candidates": matches]
        }
    }

    private func completeTargets(_ params: [String: Any], _ context: Context) -> [String: Any] {
        let kinds = params["kinds"] as? [String] ?? ["session", "window", "block"]
        let prefix = (params["prefix"] as? String ?? "").lowercased()
        var searched = Workspace.shared.sessions
        if let raw = (params["scope"] as? [String: Any])?["session"] as? String, let session = findSession(raw) { searched = [session] }
        let all = candidates(kinds: kinds, sessions: searched, window: nil, many: searched.count > 1)
        var completions: [[String: Any]] = []
        for candidate in all {
            for text in [candidate.label, candidate.position.map(String.init) ?? "", String(candidate.id.split(separator: ":").last ?? "").replacingOccurrences(of: "-", with: "").suffix(8).description]
            where !text.isEmpty && text.lowercased().hasPrefix(prefix) {
                var entry = candidate.info
                entry["text"] = text
                completions.append(entry)
                break
            }
        }
        return ["completions": completions]
    }

    // MARK: Help

    static func describeBlockMethod(_ name: String) -> String {
        [
            "blob_read": "Read a result that was too large for one reply, such as format with max_bytes set.",
            "clear": "Clear the screen and scrollback.",
            "clipboard_read": "Read text that a program in the terminal copied to the clipboard (kept about a minute).",
            "download_begin": "Open a file on this Mac for download; read it with data.read and the ticket.",
            "format": "Get the screen and scrollback as plain text, HTML, or escape sequences.",
            "input_mode": "Get how the terminal handles typed input: echo, canonical, signals.",
            "list_dir": "List the subdirectories of a directory, and optionally its files.",
            "modes": "List the modes programs turned on, and whether the alternate screen shows.",
            "process": "Get the processes running in the terminal and how its program exited.",
            "program_status": "List the status programs reported with OSC 7501.",
            "pwd": "Get the working directory a program last reported.",
            "reset": "Reset the terminal and clear its screen and scrollback.",
            "resize": "Request a size in columns and rows; it applies while the caller owns the block.",
            "set_theme": "Replace the terminal's default colors.",
            "size": "Get the size, its owner, and the sizes clients asked for.",
            "title": "Get the title and the process that set it.",
            "upload_begin": "Reserve a file to upload; write it with data.write, and its path is pasted once the batch is in.",
            "write": "Type text into the terminal as if the user had typed it.",
        ][name] ?? ""
    }

    static func helpAll() -> [String: Any] {
        let methods: [(String, String)] = [
            ("block.close", "Close a block and end its program."),
            ("block.event", "Send an input event, such as a key press or a paste, to a block."),
            ("block.method", "Call a method a block provides and return its result."),
            ("block.send_keys", "Press keys in a block, written as Rex key specs."),
            ("client.action", "Perform an action from the config file or Termy Next, as this client."),
            ("client.inspect", "Get one client's details and the actions it offers."),
            ("client.key", "Send a key to a block, giving Lua key bindings the first chance at it."),
            ("client.keymap", "Get what each key does: config file bindings, client scripts, modes."),
            ("client.list", "List the clients: the app, and connections that stay open."),
            ("client.perform", "Ask a client to perform one of its actions."),
            ("client.reply", "Answer a call a client asked for."),
            ("client.set_label", "Set or remove this client's label."),
            ("client.set_script", "Load Lua scripts for this client, replacing any it loaded before."),
            ("data.connect", "Get a ticket to stream a block's screen (data.open over this socket)."),
            ("data.read", "Read part of a download, as base64."),
            ("data.write", "Write part of an upload, as base64."),
            ("help.all", "Describe every method."),
            ("help.describe", "Describe one method."),
            ("hosts.list", "List the hosts: other Macs running Termy Next, reached over ssh."),
            ("hosts.put", "Add a host, or replace the one with the same ID."),
            ("hosts.remove", "Remove a host."),
            ("server.config_reload", "Read the config files again."),
            ("server.status", "Get Termy Next's identity, session count and config."),
            ("server.stop", "Quit Termy Next, or with if_idle, only when no window or connection uses it."),
            ("server.version", "Get Termy Next's version."),
            ("server.whoami", "Get who you are to Termy Next, and which pane you're in."),
            ("session.attach", "Attach to a session and get its view."),
            ("session.attach_all", "Hear every session's events and list them."),
            ("session.bootstrap", "Set up a connection in one call."),
            ("session.close_layer", "Close a floating layer and every block in it."),
            ("session.close_window", "Close a window and every block in it."),
            ("session.create", "Create a session, optionally with its first windows."),
            ("session.describe_block", "Get a block's details and its methods, events and actions."),
            ("session.destroy", "Destroy a session and close every block in it."),
            ("session.detach", "Stop hearing a session."),
            ("session.detach_block", "Take a block out of its window without closing it."),
            ("session.disown_block", "Give up the caller's claim on blocks."),
            ("session.float_block", "Move a block out of its window's layout into a floating layer."),
            ("session.focus_block", "Focus a block, and make its window active."),
            ("session.focus_direction", "Move focus to the nearest block left, right, above or below."),
            ("session.focus_next_pane", "Focus the next block in the active window."),
            ("session.focus_next_window", "Make the next window active."),
            ("session.focus_previous_pane", "Focus the previous block in the active window."),
            ("session.focus_previous_window", "Make the previous window active."),
            ("session.focus_window", "Make a window active."),
            ("session.list", "List the sessions."),
            ("session.list_blocks", "List every block in the session, detached ones too."),
            ("session.list_clients", "List the clients attached to the session."),
            ("session.lower_layer", "Move a floating layer to the back, or below another."),
            ("session.move", "Move a session in the order session.list returns."),
            ("session.move_block", "Move a block, or place a detached one, beside another block."),
            ("session.move_window", "Move a window in the session's window order."),
            ("session.new_block", "Create a block without placing it in a window."),
            ("session.new_layer", "Create blocks in a new floating layer."),
            ("session.new_split", "Create blocks in a new split beside a block or split."),
            ("session.new_window", "Create a window and the blocks in its layout."),
            ("session.own_block", "Claim blocks for the caller."),
            ("session.raise_layer", "Move a floating layer to the front, or above another."),
            ("session.resize_layer", "Move or resize a floating layer."),
            ("session.resize_pane", "Move the border between two blocks."),
            ("session.set_block_label", "Set a block's label."),
            ("session.set_label", "Set the session's label."),
            ("session.set_layer_state", "Minimize a floating layer or return it to normal."),
            ("session.set_window_label", "Set a window's label."),
            ("session.swap_blocks", "Swap the places of two blocks."),
            ("session.view", "Get the session's windows, layouts and blocks."),
            ("session.zoom_block", "Zoom a block to fill its window, or undo the zoom."),
            ("target.complete", "List labels, positions and short IDs that start with a prefix."),
            ("target.resolve", "Look up sessions, windows, blocks or clients by ID, label or position."),
        ]
        return [
            "methods": methods.map { ["name": $0.0, "description": $0.1] },
            "block_types": [[
                "creator": PluginFlavor.terminalCreator,
                "flavors": [PluginFlavor.shell, PluginFlavor.termyShell],
                "methods": blockMethods.map { ["name": $0, "description": describeBlockMethod($0)] },
            ]],
        ]
    }
}
