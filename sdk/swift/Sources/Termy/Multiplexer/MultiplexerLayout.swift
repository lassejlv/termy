import Foundation
internal import CTermy

/// The window, workspace, tab and split layout a session host shares with
/// Termy and `termy mux`. `termy mux split`, `tab`, `window`, `workspace` and
/// `create --window` edit it; an app that shows the host's panes reads it,
/// applies it, and writes its own changes back.
public struct MultiplexerLayout: Codable, Equatable, Sendable {
    /// Increments with every accepted write.
    public var revision: UInt64
    public var windows: [Window]

    public init(revision: UInt64 = 0, windows: [Window] = []) {
        self.revision = revision
        self.windows = windows
    }

    public struct Window: Codable, Equatable, Sendable {
        public var id: String
        public var activeWorkspace: Int
        public var workspaces: [Workspace]

        public init(id: String, activeWorkspace: Int = 0, workspaces: [Workspace]) {
            self.id = id
            self.activeWorkspace = activeWorkspace
            self.workspaces = workspaces
        }

        enum CodingKeys: String, CodingKey {
            case id, workspaces
            case activeWorkspace = "active_workspace"
        }
    }

    public struct Workspace: Codable, Equatable, Sendable {
        public var name: String
        public var pinned: Bool
        public var activeTab: Int
        public var tabs: [Tab]

        public init(name: String, pinned: Bool = false, activeTab: Int = 0, tabs: [Tab]) {
            self.name = name
            self.pinned = pinned
            self.activeTab = activeTab
            self.tabs = tabs
        }

        enum CodingKeys: String, CodingKey {
            case name, pinned, tabs
            case activeTab = "active_tab"
        }
    }

    public struct Tab: Codable, Equatable, Sendable {
        /// A name the user gave the tab.
        public var title: String?
        public var pinned: Bool
        /// Whether the active pane fills the tab.
        public var zoomed: Bool
        public var activePane: String?
        /// Font zoom steps for panes that are zoomed.
        public var zoom: [String: Int16]
        /// The tab's size in cells.
        public var cols: UInt16?
        public var rows: UInt16?
        public var tree: Node

        public init(title: String? = nil, pinned: Bool = false, zoomed: Bool = false, activePane: String? = nil,
                    zoom: [String: Int16] = [:], cols: UInt16? = nil, rows: UInt16? = nil, tree: Node) {
            self.title = title
            self.pinned = pinned
            self.zoomed = zoomed
            self.activePane = activePane
            self.zoom = zoom
            self.cols = cols
            self.rows = rows
            self.tree = tree
        }

        enum CodingKeys: String, CodingKey {
            case title, pinned, zoomed, zoom, cols, rows, tree
            case activePane = "active_pane"
        }

        public init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            title = try c.decodeIfPresent(String.self, forKey: .title)
            pinned = try c.decodeIfPresent(Bool.self, forKey: .pinned) ?? false
            zoomed = try c.decodeIfPresent(Bool.self, forKey: .zoomed) ?? false
            activePane = try c.decodeIfPresent(String.self, forKey: .activePane)
            zoom = try c.decodeIfPresent([String: Int16].self, forKey: .zoom) ?? [:]
            cols = try c.decodeIfPresent(UInt16.self, forKey: .cols)
            rows = try c.decodeIfPresent(UInt16.self, forKey: .rows)
            tree = try c.decode(Node.self, forKey: .tree)
        }
    }

    /// A split tree of pane IDs.
    public indirect enum Node: Codable, Equatable, Sendable {
        case leaf(String)
        /// `horizontal` places the children side by side, `vertical` stacks them.
        case split(axis: Axis, ratio: Double, first: Node, second: Node)

        public enum Axis: String, Codable, Sendable { case horizontal, vertical }

        public var paneIDs: [String] {
            switch self {
            case .leaf(let id): [id]
            case .split(_, _, let first, let second): first.paneIDs + second.paneIDs
            }
        }

        enum CodingKeys: String, CodingKey { case leaf, axis, ratio, first, second }

        public init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            if let id = try c.decodeIfPresent(String.self, forKey: .leaf) {
                self = .leaf(id)
            } else {
                self = .split(axis: try c.decode(Axis.self, forKey: .axis), ratio: try c.decode(Double.self, forKey: .ratio),
                              first: try c.decode(Node.self, forKey: .first), second: try c.decode(Node.self, forKey: .second))
            }
        }

        public func encode(to encoder: Encoder) throws {
            var c = encoder.container(keyedBy: CodingKeys.self)
            switch self {
            case .leaf(let id):
                try c.encode(id, forKey: .leaf)
            case .split(let axis, let ratio, let first, let second):
                try c.encode(axis, forKey: .axis)
                try c.encode(ratio, forKey: .ratio)
                try c.encode(first, forKey: .first)
                try c.encode(second, forKey: .second)
            }
        }
    }
}

extension Multiplexer {
    /// The shared layout as the host has it now.
    public func layout() throws(TermyError) -> MultiplexerLayout {
        lock.lock()
        defer { lock.unlock() }
        let json = try takeBytes("termy_multiplexer_layout_json") { termy_multiplexer_layout_json(handle, $0) }
        do {
            return try JSONDecoder().decode(MultiplexerLayout.self, from: json)
        } catch {
            throw TermyError(code: .serializeFailed, operation: "termy_multiplexer_layout_json")
        }
    }

    /// Writes `layout` if the host's is still at `layout.revision`. Returns
    /// whether it was written and the host's revision afterwards; when
    /// another client wrote first, read `layout()` and apply it before
    /// writing again.
    @discardableResult
    public func setLayout(_ layout: MultiplexerLayout) throws(TermyError) -> (updated: Bool, revision: UInt64) {
        lock.lock()
        defer { lock.unlock() }
        let data: Data
        do {
            data = try JSONEncoder().encode(layout)
        } catch {
            throw TermyError(code: .serializeFailed, operation: "termy_multiplexer_set_layout_json")
        }
        var revision: UInt64 = 0
        var updated = false
        let status = data.withUnsafeBytes { bytes in
            termy_multiplexer_set_layout_json(
                handle, bytes.bindMemory(to: UInt8.self).baseAddress, bytes.count, layout.revision, &revision, &updated)
        }
        try check(status, "termy_multiplexer_set_layout_json")
        return (updated, revision)
    }

    /// Calls `onChange` with each new layout revision, the current one first,
    /// on a background thread, and with nil when the host went away. Keep the
    /// returned watch; releasing it stops watching.
    public func watchLayout(_ onChange: @escaping @Sendable (UInt64?) -> Void) throws(TermyError) -> LayoutWatch {
        let box = LayoutWatch.Callback(onChange)
        let context = Unmanaged.passRetained(box).toOpaque()
        var watch: OpaquePointer?
        let status = termy_multiplexer_watch_layout(handle, { context, revision in
            guard let context else { return }
            let box = Unmanaged<LayoutWatch.Callback>.fromOpaque(context).takeUnretainedValue()
            box.call(revision == UInt64.max ? nil : revision)
        }, context, &watch)
        do {
            try check(status, "termy_multiplexer_watch_layout")
        } catch {
            Unmanaged<LayoutWatch.Callback>.fromOpaque(context).release()
            throw error
        }
        guard let watch else {
            Unmanaged<LayoutWatch.Callback>.fromOpaque(context).release()
            throw TermyError(code: .nullPointer, operation: "termy_multiplexer_watch_layout")
        }
        return LayoutWatch(handle: watch, context: context)
    }
}

/// A running watch on the shared layout; releasing it stops the watch.
public final class LayoutWatch: @unchecked Sendable {
    final class Callback: @unchecked Sendable {
        let call: @Sendable (UInt64?) -> Void
        init(_ call: @escaping @Sendable (UInt64?) -> Void) { self.call = call }
    }

    private let handle: OpaquePointer
    private let context: UnsafeMutableRawPointer

    init(handle: OpaquePointer, context: UnsafeMutableRawPointer) {
        self.handle = handle
        self.context = context
    }

    deinit {
        // Stops the thread first, so the callback box is no longer used.
        termy_layout_watch_free(handle)
        Unmanaged<Callback>.fromOpaque(context).release()
    }
}
