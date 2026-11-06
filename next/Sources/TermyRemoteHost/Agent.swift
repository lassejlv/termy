import Foundation
import Termy
import TermyRemoteKit

/// Serves this computer's session host to its paired phones through the
/// relay. Each phone gets one `SecureChannel`; every screen and key inside it
/// is sealed end to end.
@MainActor
public final class RemoteHostAgent {
    public enum ConnectionState: Sendable { case connecting, connected, offline }
    public var onConnectionState: ((ConnectionState) -> Void)?
    private let credentials: DeviceCredentials
    private let identity: DeviceIdentity
    private let trustedDevices: [UUID: Data]
    private let multiplexer: Multiplexer
    private var connection: RelayConnection?
    private var peers: [UUID: Peer] = [:]
    private var running = false
    public var configurationProvider: () -> RemoteConfiguration = { RemoteConfiguration() }
    public var configPath: String?
    public var launchEnvironment: [String: String] = ["TERM_PROGRAM": "TermyNext"]
    private var lastConfiguration: RemoteConfiguration?
    var configuration: RemoteConfiguration { lastConfiguration ?? configurationProvider() }
    private var lastSessions: RemoteSessionList?

    public init(
        multiplexer: Multiplexer, credentials: DeviceCredentials, identity: DeviceIdentity, trustedDevices: [UUID: Data]
    ) {
        self.multiplexer = multiplexer
        self.credentials = credentials
        self.identity = identity
        self.trustedDevices = trustedDevices
    }
    private var api: RelayAPI { credentials.api }

    public func stop() {
        running = false
        connection?.close()
        connection = nil
        for peer in peers.values { peer.close() }
        peers.removeAll()
    }

    // MARK: Running

    public func run() async throws {
        running = true
        lastConfiguration = configurationProvider()
        log("Serving \(hostName) to your phones. This computer's key: \(identity.fingerprint)")
        let watcher = Task { await self.watchSessions() }
        defer {
            watcher.cancel()
            stop()
        }
        var delay: UInt64 = 1
        while !Task.isCancelled {
            onConnectionState?(.connecting)
            let connection = RelayConnection(api: api)
            self.connection = connection
            var wasReady = false
            for await event in connection.events {
                switch event {
                case .ready:
                    onConnectionState?(.connected)
                    wasReady = true
                    delay = 1
                    log("Connected to \(credentials.relayURL.host ?? "the relay").")
                case .packet(let from, let packet):
                    await handle(packet, from: from)
                case .presence(let device, let online):
                    if !online, let peer = peers.removeValue(forKey: device) {
                        peer.close()
                        log("\(peer.name) disconnected.")
                    }
                case .undeliverable(let device):
                    peers.removeValue(forKey: device)?.close()
                case .closed(let error):
                    if let error { log("Relay connection closed: \(error.localizedDescription)") }
                }
            }
            onConnectionState?(.offline)
            for peer in peers.values { peer.close() }
            peers.removeAll()
            if !wasReady, try await !sessionIsValid() {
                throw AgentError.signedOut
            }
            try await Task.sleep(nanoseconds: delay * 1_000_000_000)
            delay = min(delay * 2, 30)
        }
    }

    private func sessionIsValid() async throws -> Bool {
        do {
            _ = try await api.devices()
            return true
        } catch let error as RelayAPIError where error.status == 401 {
            return false
        } catch {
            return true
        }
    }

    var hostName: String {
        Host.current().localizedName ?? ProcessInfo.processInfo.hostName
    }

    // MARK: Handshakes

    private func handle(_ packet: Data, from device: UUID) async {
        if SecureChannel.isHandshakeInit(packet) {
            await accept(packet, from: device)
            return
        }
        guard let peer = peers[device] else { return }
        do {
            let message = try MessageCodec.decode(ClientMessage.self, from: try peer.channel.open(packet: packet))
            peer.handle(message)
        } catch {
            log("Dropped a packet from \(peer.name): \(error)")
        }
    }

    private func accept(_ initPacket: Data, from deviceID: UUID) async {
        do {
            let clientKey = try SecureChannel.clientStaticKey(inInit: initPacket)
            // Both the relay link and the locally pinned phone key must authorize access.
            let devices = try await api.devices()
            guard running, !Task.isCancelled else { return }
            guard let device = devices.first(where: { $0.id == deviceID }), device.kind == .client,
                device.publicKeyData == clientKey
            else {
                log("Refused a connection from an unknown device.")
                return
            }
            guard trustedDevices[deviceID] == clientKey else {
                log("Refused a phone that has not been paired with this Mac.")
                return
            }
            let (channel, response) = try SecureChannel.respond(identity: identity, initPacket: initPacket)
            peers.removeValue(forKey: deviceID)?.close()
            let peer = Peer(id: deviceID, name: device.name, channel: channel, agent: self)
            peers[deviceID] = peer
            connection?.send(response, to: deviceID)
            peer.send(.hello(hostName: hostName, version: RemoteProtocol.version))
            peer.send(.configuration(configuration))
            peer.send(.sessions(currentSessions()))
            log("\(device.name) connected.")
        } catch {
            log("Handshake with a phone failed: \(error)")
        }
    }

    // MARK: Sessions

    func send(_ packet: Data, to device: UUID) {
        connection?.send(packet, to: device)
    }

    func attach(paneID: String) async throws -> Terminal {
        // A new terminal has no other owner yet, so handing it over is safe.
        return try await Task.detached { [multiplexer] in Handoff(try multiplexer.attachTerminal(paneID: paneID)) }
            .value.value
    }

    enum Creation {
        case session(String)
        case tab(String)
        case split(String, Bool)
    }
    func create(_ destination: Creation) async throws -> String {
        let path = configPath
        let environment = launchEnvironment
        return try await Task.detached { [multiplexer] in
            let config = try path.map { try TermyConfig(path: $0) } ?? TermyConfig.loadDefault()
            var directory = config.workingDirectory
            switch destination {
            case .session: break
            case .tab(let anchor), .split(let anchor, _):
                _ = try RemoteLayout.locate(multiplexer.layout(), pane: anchor)
                if let cwd = try multiplexer.panes().first(where: { $0.id == anchor })?.workingDirectory {
                    directory = cwd.hasPrefix("file://") ? URL(string: cwd)?.path : cwd
                }
            }
            let render = try config.renderConfig()
            let terminal = try multiplexer.createTerminal(
                size: TerminalSize(cols: 80, rows: 24, cellWidth: render.cellWidth, cellHeight: render.cellHeight),
                options: TerminalOptions(config: config, workingDirectory: directory, environment: environment))
            guard let id = terminal.paneID else { throw RemoteLayout.EditError.missingSession }
            do {
                try RemoteLayout.edit(multiplexer) { layout in
                    switch destination {
                    case .session(let name): try RemoteLayout.addSession(&layout, pane: id, name: name)
                    case .tab(let anchor):
                        let (w, s, _) = try RemoteLayout.locate(layout, pane: anchor)
                        layout.windows[w].workspaces[s].tabs.append(.init(activePane: id, tree: .leaf(id)))
                        layout.windows[w].workspaces[s].activeTab = layout.windows[w].workspaces[s].tabs.count - 1
                    case .split(let anchor, let horizontal):
                        let (w, s, t) = try RemoteLayout.locate(layout, pane: anchor)
                        let old = layout.windows[w].workspaces[s].tabs[t].tree
                        layout.windows[w].workspaces[s].tabs[t].tree = RemoteLayout.split(
                            old, at: anchor, pane: id, horizontal: horizontal)
                        layout.windows[w].workspaces[s].tabs[t].activePane = id
                    }
                }
            } catch {
                try? multiplexer.close(paneID: id)
                throw error
            }
            return id
        }.value
    }
    func rename(pane: String, name: String, session: Bool) throws {
        try RemoteLayout.edit(multiplexer) { layout in
            let (w, s, t) = try RemoteLayout.locate(layout, pane: pane)
            let value = try RemoteLayout.name(name)
            if session {
                guard
                    !layout.windows[w].workspaces.enumerated().contains(where: {
                        $0.offset != s && $0.element.name == value
                    })
                else { throw RemoteLayout.EditError.invalidName }
                layout.windows[w].workspaces[s].name = value
            } else {
                layout.windows[w].workspaces[s].tabs[t].title = value
            }
        }
    }
    func closePane(_ paneID: String) async throws {
        try await Task.detached { [multiplexer] in try multiplexer.close(paneID: paneID) }.value
    }

    func currentSessions() -> RemoteSessionList {
        let panes = (try? multiplexer.panes()) ?? []
        let layout = try? multiplexer.layout()
        var groups: [RemoteSessionGroup] = []
        for window in layout?.windows ?? [] {
            for workspace in window.workspaces {
                let tabs = workspace.tabs.map { tab in
                    var remote = RemoteSessionGroup.Tab(title: tab.title, paneIDs: tab.tree.paneIDs)
                    remote.activePaneID = tab.activePane
                    return remote
                }
                if !tabs.isEmpty {
                    var group = RemoteSessionGroup(name: workspace.name, tabs: tabs)
                    group.windowID = window.id
                    groups.append(group)
                }
            }
        }
        return RemoteSessionList(
            groups: groups,
            panes: panes.map {
                RemotePane(id: $0.id, title: $0.title, workingDirectory: $0.workingDirectory, exited: $0.exited)
            }
        )
    }

    /// Tells phones when panes come and go or are renamed.
    private func watchSessions() async {
        while !Task.isCancelled {
            do { try await Task.sleep(nanoseconds: 2_000_000_000) } catch { return }
            guard !peers.isEmpty else { continue }
            let configuration = configurationProvider()
            if configuration != lastConfiguration {
                lastConfiguration = configuration
                for peer in peers.values { peer.configure(configuration) }
            }
            broadcastSessionsIfChanged()
        }
    }

    func broadcastSessionsIfChanged() {
        let sessions = currentSessions()
        guard sessions != lastSessions else { return }
        lastSessions = sessions
        for peer in peers.values { peer.send(.sessions(sessions)) }
    }
}

/// One connected phone and the panes it streams.
@MainActor
final class Peer {
    let id: UUID
    let name: String
    var channel: SecureChannel
    private weak var agent: RemoteHostAgent?
    private var streams: [Int: Stream] = [:]
    private var active = true

    init(id: UUID, name: String, channel: SecureChannel, agent: RemoteHostAgent) {
        self.id = id
        self.name = name
        self.channel = channel
        self.agent = agent
    }

    func send(_ message: HostMessage) {
        do {
            let packet = try channel.seal(try MessageCodec.encode(message))
            agent?.send(packet, to: id)
        } catch {
            log("Couldn't send to \(name): \(error)")
        }
    }

    func configure(_ configuration: RemoteConfiguration) {
        send(.configuration(configuration))
        for stream in streams.values { stream.configure() }
    }
    func close() {
        active = false
        for stream in streams.values { stream.close() }
        streams.removeAll()
    }

    func handle(_ message: ClientMessage) {
        guard active else { return }
        switch message {
        case .listSessions:
            if let agent { send(.sessions(agent.currentSessions())) }
        case .attach(let stream, let paneID):
            Task { await attach(stream: stream, paneID: paneID) }
        case .detach(let stream):
            streams.removeValue(forKey: stream)?.close()
        case .key(let stream, let key):
            streams[stream]?.send(key)
        case .paste(let stream, let text):
            try? streams[stream]?.session.paste(text)
        case .resize(let stream, let cols, let rows):
            streams[stream]?.resize(cols: cols, rows: rows)
        case .scroll(let stream, let lines):
            streams[stream]?.scroll(lines: lines)
        case .createPane: create(.session("Session"))
        case .createSession(let name): create(.session(name))
        case .createTab(let anchor): create(.tab(anchor))
        case .splitPane(let pane, let horizontal): create(.split(pane, horizontal))
        case .renameSession(let pane, let name): rename(pane, name: name, session: true)
        case .renameTab(let pane, let title): rename(pane, name: title, session: false)
        case .mouse(let id, let col, let row, let action): streams[id]?.mouse(col: col, row: row, action: action)
        case .scrollToBottom(let id): streams[id]?.session.scrollToBottom()
        case .search(let id, let query):
            guard let stream = streams[id] else { return }
            do {
                let matches = query.isEmpty ? [] : try stream.session.terminal.search(String(query.prefix(512)))
                send(
                    .searchResults(
                        stream: id,
                        matches: matches.prefix(500).map {
                            RemoteSearchMatch(row: $0.row, startCol: $0.startCol, endCol: $0.endCol, line: $0.line)
                        }))
            } catch { send(.error("Couldn't search the terminal.")) }
        case .revealSearch(let id, let bufferRow):
            guard let stream = streams[id], bufferRow >= 0 else { return }
            let frame = stream.session.frame
            let desired = max(0, frame.historySize - bufferRow + frame.rows / 2)
            stream.session.scroll(lines: Int32(clamping: desired - frame.displayOffset))
        case .closePane(let paneID):
            Task {
                do { try await agent?.closePane(paneID) } catch { send(.error("Couldn’t close the pane: \(error)")) }
                agent?.broadcastSessionsIfChanged()
            }
        }
    }

    private func create(_ destination: RemoteHostAgent.Creation) {
        Task {
            do {
                guard let id = try await agent?.create(destination) else { return }
                send(.created(paneID: id))
                agent?.broadcastSessionsIfChanged()
            } catch { send(.error("Couldn't create a terminal: \(error)")) }
        }
    }
    private func rename(_ pane: String, name: String, session: Bool) {
        do {
            try agent?.rename(pane: pane, name: name, session: session)
            agent?.broadcastSessionsIfChanged()
        } catch { send(.error("Couldn't rename. The item may have moved or that name is already used.")) }
    }
    var configuration: RemoteConfiguration { agent?.configuration ?? RemoteConfiguration() }
    var configPath: String? { agent?.configPath }
    private func attach(stream id: Int, paneID: String) async {
        streams.removeValue(forKey: id)?.close()
        do {
            guard let terminal = try await agent?.attach(paneID: paneID) else { return }
            guard active else { return }
            if let path = agent?.configPath {
                try terminal.applyColors(
                    from: TermyConfig(path: path), appearance: configuration.appearance == "light" ? .light : .dark)
            }
            let frame = try terminal.snapshot()
            let size = TerminalSize(cols: UInt16(frame.cols), rows: UInt16(frame.rows))
            streams[id] = Stream(id: id, session: TerminalSession(terminal: terminal, size: size), peer: self)
        } catch {
            send(.error("Couldn't open the session: \(error)"))
        }
    }
}

/// One pane a phone is watching. Coalesces screen changes into at most one
/// frame every 33 ms.
@MainActor
final class Stream {
    let id: Int
    let session: TerminalSession
    private weak var peer: Peer?
    private let originalSize: TerminalSize
    private var dirty: Set<Int>? = nil
    private var pendingFull = true
    private var sendScheduled = false

    init(id: Int, session: TerminalSession, peer: Peer) {
        self.id = id
        self.session = session
        self.peer = peer
        originalSize = session.size
        session.onFrameChange = { [weak self] change in self?.changed(change) }
        session.onEvent = { [weak self] event in self?.event(event) }
        session.poll(forceFull: true)
        scheduleSend()
    }

    func close() {
        session.onFrameChange = nil
        session.onEvent = nil
        if session.size != originalSize { try? session.resize(originalSize) }
        session.stop()
    }

    func send(_ key: RemoteKey) {
        var modifiers: KeyModifiers = []
        if key.control { modifiers.insert(.control) }
        if key.alt { modifiers.insert(.alt) }
        if key.shift { modifiers.insert(.shift) }
        try? session.send(Keystroke(key: key.key, text: key.text, modifiers: modifiers), optionAsAlt: true)
    }

    /// Fits the pane to the phone, or restores it when `cols` is nil.
    func resize(cols: Int?, rows: Int?) {
        let size: TerminalSize
        if let cols, let rows, cols > 1, rows > 1 {
            size = TerminalSize(cols: UInt16(min(cols, 500)), rows: UInt16(min(rows, 300)))
        } else {
            size = originalSize
        }
        try? session.resize(size)
        pendingFull = true
        scheduleSend()
    }

    func configure() {
        if let path = peer?.configPath {
            try? session.terminal.applyColors(
                from: TermyConfig(path: path), appearance: peer?.configuration.appearance == "light" ? .light : .dark)
        }
        session.poll(forceFull: true)
        pendingFull = true
        scheduleSend()
    }
    func mouse(col: Int, row: Int, action: String) {
        guard (0..<session.frame.cols).contains(col), (0..<session.frame.rows).contains(row) else { return }
        let kind: MouseInput.Kind
        switch action {
        case "press": kind = .press(.left)
        case "release": kind = .release(.left)
        case "drag": kind = .drag(.left)
        default: return
        }
        _ = try? session.send(MouseInput(kind, at: GridPosition(row: row, col: col)))
    }
    func scroll(lines: Int) {
        let count = min(abs(max(-500, min(lines, 500))), 100)
        guard count > 0 else { return }
        let kind: MouseInput.Kind = lines > 0 ? .wheelUp : .wheelDown
        let reported = (try? session.send(MouseInput(kind, at: GridPosition(row: 0, col: 0)))) ?? false
        if reported {
            for _ in 1..<count { _ = try? session.send(MouseInput(kind, at: GridPosition(row: 0, col: 0))) }
        } else {
            session.scroll(lines: Int32(clamping: lines))
        }
    }
    private func changed(_ change: FrameChange) {
        if let rows = FrameEncoder.dirtyRows(change) {
            if !pendingFull { dirty = (dirty ?? []).union(rows) }
        } else {
            pendingFull = true
        }
        scheduleSend()
    }

    private func event(_ event: TerminalEvent) {
        switch event {
        case .title(let title): peer?.send(.title(stream: id, title: title))
        case .bell: peer?.send(.bell(stream: id))
        case .exit: peer?.send(.exited(stream: id))
        default: break
        }
    }

    private func scheduleSend() {
        guard !sendScheduled else { return }
        sendScheduled = true
        Task { @MainActor [weak self] in
            try? await Task.sleep(nanoseconds: 33_000_000)
            self?.flush()
        }
    }

    private func flush() {
        sendScheduled = false
        let store = session.frame
        guard !store.isEmpty else { return }
        let rows: [Int] = pendingFull ? Array(0..<store.rows) : (dirty ?? []).filter { $0 < store.rows }.sorted()
        let full = pendingFull
        pendingFull = false
        dirty = nil
        let config = peer?.configuration ?? RemoteConfiguration()
        let spans = full ? nil : rows.map { DirtySpan(row: $0, leftCol: 0, rightCol: store.cols - 1) }
        let plans =
            (try? store.glyphPlan(
                metrics: GlyphMetrics(
                    cellWidth: Float(config.cellWidth), cellHeight: Float(config.cellHeight),
                    fontSize: Float(config.fontSize)), spans: spans)) ?? []
        let glyphs = Dictionary(uniqueKeysWithValues: plans.map { ($0.cellIndex, $0) })
        let updates = rows.map { row in
            var update = RowUpdate(
                row: row,
                runs: FrameEncoder.runs(
                    store.cells(row: row), configuration: config, row: row, cols: store.cols, glyphs: glyphs))
            update.wrapped = store.cells(row: row).last?.isLineWrapped ?? false
            return update
        }
        let background =
            (0..<store.rows).lazy.compactMap { row -> UInt32? in
                store.cells(row: row).first(where: \.usesDefaultBackground)?.background.rgba
            }.first ?? config.background
        var frame = RemoteFrame(
            stream: id, cols: store.cols, rows: store.rows, full: full, updates: updates,
            cursor: FrameEncoder.cursor(store.cursor), background: background
        )
        frame.displayOffset = store.displayOffset
        frame.historySize = store.historySize
        peer?.send(.frame(frame))
    }
}

/// Moves a value made off the main actor onto it.
struct Handoff<Value>: @unchecked Sendable {
    let value: Value
    init(_ value: Value) { self.value = value }
}

enum AgentError: Error, CustomStringConvertible {
    case missingHost
    case signedOut

    var description: String {
        switch self {
        case .missingHost: "Couldn't find termy-session-host. Install Termy Next, or run from a build next to libtermy."
        case .signedOut: "This Mac’s relay registration ended. Pair your phone again in Settings → Mobile."
        }
    }
}

func log(_ message: String) {
    let time = Date().formatted(date: .omitted, time: .standard)
    FileHandle.standardError.write(Data("[\(time)] \(message)\n".utf8))
}
