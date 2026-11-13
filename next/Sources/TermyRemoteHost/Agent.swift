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
    /// Pane sizes phones asked for, across every phone, so the last one to
    /// leave a pane gives it back the size it had before any phone came.
    let paneSizes = PaneSizes()

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
            peer.send(.sessions(await currentSessions()))
            log("\(device.name) connected.")
        } catch {
            log("Handshake with a phone failed: \(error)")
        }
    }

    // MARK: Sessions

    func send(_ packet: Data, to device: UUID) {
        connection?.send(packet, to: device)
    }

    /// Bytes queued on the relay socket and not yet written.
    var sendBacklog: Int { connection?.pendingSendBytes ?? 0 }

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

    /// The sessions as the host has them now. The host calls block, so they
    /// run off the main actor.
    func currentSessions() async -> RemoteSessionList {
        await Task.detached { [multiplexer] in
            Self.sessions(panes: (try? multiplexer.panes()) ?? [], layout: try? multiplexer.layout())
        }.value
    }

    nonisolated static func sessions(panes: [MultiplexerPane], layout: MultiplexerLayout?) -> RemoteSessionList {
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
            await broadcastSessionsIfChanged()
        }
    }

    private var sessionReads: UInt64 = 0
    private var sessionReadShown: UInt64 = 0

    func broadcastSessionsIfChanged() async {
        sessionReads += 1
        let read = sessionReads
        let sessions = await currentSessions()
        // Reads overlap; one that finished after a newer one is stale.
        guard read > sessionReadShown else { return }
        sessionReadShown = read
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
    /// The attach each stream is waiting for. A detach, or a newer attach of
    /// the same stream, replaces it, so the older one is dropped on arrival
    /// instead of streaming a pane the phone no longer shows.
    private var attaching: [Int: UUID] = [:]
    /// Each stream's latest search, which the next one waits for.
    private var searches: [Int: Task<Void, Never>] = [:]
    /// The glyphs this phone keeps (protocol 4).
    private var glyphs = GlyphLedger()
    private var active = true
    var streamCount: Int { streams.count }
    /// The largest payload sealed into one packet. The relay closes a socket
    /// that sends a larger message, which would drop every phone.
    static let maxPayloadBytes = RelayWire.maxMessageBytes - 1024
    /// Screen updates wait while more than this is queued on the relay socket.
    static let maxBacklogBytes = 1024 * 1024
    var isCongested: Bool { (agent?.sendBacklog ?? 0) > Self.maxBacklogBytes }
    private var clientVersion = 1
    /// Whether the phone understands `HostMessage.image` and `.images`.
    var showsImages: Bool { clientVersion >= 3 }
    /// Whether the phone keeps glyphs by key and takes larger grids.
    var keepsGlyphs: Bool { clientVersion >= 4 }

    init(id: UUID, name: String, channel: SecureChannel, agent: RemoteHostAgent) {
        self.id = id
        self.name = name
        self.channel = channel
        self.agent = agent
    }

    func send(_ message: HostMessage) {
        // A closed peer's channel is gone on the phone; its packets would
        // only fail to open there.
        guard active else { return }
        var message = message
        if case .frame(var frame) = message {
            if keepsGlyphs {
                frame = frame.clipped(cols: RemoteFrame.maxCols, rows: RemoteFrame.maxRows)
                glyphs.prepare(&frame)
            } else {
                // Older phones drop larger grids and know no glyph keys.
                frame = frame.clipped(cols: RemoteFrame.legacyMaxCols, rows: RemoteFrame.legacyMaxRows)
                for update in frame.updates.indices {
                    for run in frame.updates[update].runs.indices { frame.updates[update].runs[run].glyphKey = nil }
                }
            }
            message = .frame(frame)
        }
        do {
            var strippedGlyphs = false
            let payloads = try FrameEncoder.payloads(for: message, limit: Self.maxPayloadBytes, strippedGlyphs: &strippedGlyphs)
            // The phone didn't get the PNGs the ledger counted as sent.
            if strippedGlyphs { glyphs.reset() }
            for payload in payloads {
                agent?.send(try channel.seal(payload), to: id)
            }
        } catch {
            log("Couldn't send to \(name): \(error)")
        }
    }

    func configure(_ configuration: RemoteConfiguration) {
        if clientVersion >= 2 { send(.configuration(configuration)) }
        for stream in streams.values { stream.configure() }
    }
    func close() {
        active = false
        for stream in streams.values { stream.close() }
        streams.removeAll()
        attaching.removeAll()
        for search in searches.values { search.cancel() }
        searches.removeAll()
    }

    func handle(_ message: ClientMessage) {
        guard active else { return }
        switch message {
        case .clientHello(let version):
            clientVersion = max(1, min(RemoteProtocol.version, version))
            if clientVersion >= 2 { send(.configuration(configuration)) }
        case .listSessions:
            Task {
                guard let sessions = await agent?.currentSessions() else { return }
                send(.sessions(sessions))
            }
        case .attach(let stream, let paneID):
            let token = UUID()
            attaching[stream] = token
            streams.removeValue(forKey: stream)?.close()
            Task { await attach(stream: stream, paneID: paneID, token: token) }
        case .detach(let stream):
            attaching[stream] = nil
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
            // Off the main actor, one after another, so results come back in
            // the order the phone asked.
            let previous = searches[id]
            searches[id] = Task { [weak self] in
                await previous?.value
                let result: [SearchMatch]?
                do {
                    result = query.isEmpty ? [] : try await stream.session.search(String(query.prefix(512)))
                } catch {
                    result = nil
                }
                guard let self, self.active else { return }
                guard let matches = result else { return self.send(.error("Couldn't search the terminal.")) }
                self.send(
                    .searchResults(
                        stream: id,
                        matches: matches.prefix(500).map {
                            // The phone shows a few lines of each; full lines of a
                            // wide pane could push the list past the relay's limit.
                            RemoteSearchMatch(
                                row: $0.row, startCol: $0.startCol, endCol: $0.endCol, line: String($0.line.prefix(512)))
                        }))
            }
        case .revealSearch(let id, let bufferRow):
            guard let stream = streams[id], bufferRow >= 0 else { return }
            let frame = stream.session.frame
            let desired = max(0, frame.historySize - bufferRow + frame.rows / 2)
            stream.session.scroll(lines: Int32(clamping: desired - frame.displayOffset))
        case .closePane(let paneID):
            Task {
                do { try await agent?.closePane(paneID) } catch { send(.error("Couldn’t close the pane: \(error)")) }
                await agent?.broadcastSessionsIfChanged()
            }
        }
    }

    private func create(_ destination: RemoteHostAgent.Creation) {
        Task {
            do {
                guard let id = try await agent?.create(destination) else { return }
                send(.created(paneID: id))
                await agent?.broadcastSessionsIfChanged()
            } catch { send(.error("Couldn't create a terminal: \(error)")) }
        }
    }
    private func rename(_ pane: String, name: String, session: Bool) {
        do {
            try agent?.rename(pane: pane, name: name, session: session)
            Task { await agent?.broadcastSessionsIfChanged() }
        } catch { send(.error("Couldn't rename. The item may have moved or that name is already used.")) }
    }
    var configuration: RemoteConfiguration { agent?.configuration ?? RemoteConfiguration() }
    var configPath: String? { agent?.configPath }
    private func attach(stream id: Int, paneID: String, token: UUID) async {
        do {
            guard let terminal = try await agent?.attach(paneID: paneID) else { return }
            guard active, attaching[id] == token else { return }
            if let path = agent?.configPath {
                try terminal.applyColors(
                    from: TermyConfig(path: path), appearance: configuration.appearance == "light" ? .light : .dark)
            }
            let frame = try terminal.snapshot()
            let size = TerminalSize(cols: UInt16(frame.cols), rows: UInt16(frame.rows))
            attaching[id] = nil
            guard let sizes = agent?.paneSizes else { return }
            streams[id] = Stream(
                id: id, session: TerminalSession(terminal: terminal, size: size), peer: self, sizes: sizes)
        } catch {
            guard attaching[id] == token else { return }
            attaching[id] = nil
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
    private let sizes: PaneSizes
    private let paneID: String
    private var dirty: Set<Int>? = nil
    private var pendingFull = true
    private var sendScheduled = false
    private var images = ImageEncoder()
    private var imagesChanged = true
    /// A frame's glyphs and images are being drawn off the main actor; the
    /// next frame waits for it, so frames reach the phone in order.
    private var encoding = false
    private var closed = false

    init(id: Int, session: TerminalSession, peer: Peer, sizes: PaneSizes) {
        self.id = id
        self.session = session
        self.peer = peer
        self.sizes = sizes
        paneID = session.terminal.paneID ?? "stream-\(id)"
        sizes.join(pane: paneID, stream: ObjectIdentifier(self), size: session.size)
        session.onFrameChange = { [weak self] change in self?.changed(change) }
        session.onGraphicsChange = { [weak self] in
            self?.imagesChanged = true
            self?.scheduleSend()
        }
        session.onEvent = { [weak self] event in self?.event(event) }
        session.poll(forceFull: true)
        scheduleSend()
    }

    func close() {
        closed = true
        session.onFrameChange = nil
        session.onGraphicsChange = nil
        session.onEvent = nil
        // Another phone still on the pane keeps the size it asked for.
        let size = sizes.leave(pane: paneID, stream: ObjectIdentifier(self))
        if let size, session.size != size { try? session.resize(size) }
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
        var requested: TerminalSize?
        if let cols, let rows, cols > 1, rows > 1 {
            requested = TerminalSize(cols: UInt16(min(cols, 500)), rows: UInt16(min(rows, 300)))
        }
        let size = sizes.request(pane: paneID, stream: ObjectIdentifier(self), size: requested)
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
        // On a slow link, sends queue in memory without limit and the phone
        // falls further behind. Keep collecting dirty rows instead and send
        // them as one frame once the socket catches up.
        guard !closed else { return }
        if encoding || peer?.isCongested == true {
            scheduleSend()
            return
        }
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
            let cells = store.cells(row: row)
            var update = RowUpdate(
                row: row,
                runs: FrameEncoder.runs(
                    cells, configuration: config, row: row, cols: store.cols, glyphs: glyphs, rasterize: false))
            // Phones read a missing flag as false; leave it out of most rows.
            update.wrapped = cells.last?.isLineWrapped == true ? true : nil
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
        let slots = FrameEncoder.attachGlyphs(to: &frame, configuration: config, glyphs: glyphs)
        var pendingImages: ImageEncoder.Pending?
        if imagesChanged || full, let peer, peer.showsImages {
            imagesChanged = false
            pendingImages = images.prepare(stream: id, terminal: session.terminal, configuration: config)
        }
        guard !slots.isEmpty || pendingImages?.missing.isEmpty == false else {
            return send(frame, images: pendingImages, pngs: [:])
        }
        // New glyphs and images are drawn and encoded off the main actor.
        encoding = true
        let jobs = slots.map(\.job)
        let work = Handoff((jobs, pendingImages))
        Task { [weak self] in
            let (drawn, pngs) = await Task.detached {
                let (jobs, pending) = work.value
                return Handoff((jobs.map(GlyphRasterizer.render), pending.map(ImageEncoder.encode) ?? [:]))
            }.value.value
            guard let self else { return }
            self.encoding = false
            guard !self.closed else { return }
            var frame = frame
            for (slot, png) in zip(slots, drawn) {
                guard let png else { continue }
                frame.updates[slot.update].runs[slot.run].glyphPNG = png
                frame.updates[slot.update].runs[slot.run].glyphKey = GlyphRasterizer.glyphKey(slot.job.key)
            }
            self.send(frame, images: pendingImages, pngs: pngs)
        }
    }

    private func send(_ frame: RemoteFrame, images pending: ImageEncoder.Pending?, pngs: [KittyImageKey: Data]) {
        peer?.send(.frame(frame))
        // After the frame, so an image never shows over the text it replaced.
        if let pending, let peer {
            for message in images.finish(pending, pngs: pngs) { peer.send(message) }
        }
    }
}

/// The size each pane had before phones resized it, and what each phone
/// watching it asked for. Two phones can share a pane, and the second one
/// attaches after the first already resized it, so neither stream alone
/// knows the size to give back.
@MainActor
final class PaneSizes {
    private struct Pane {
        var original: TerminalSize
        /// Streams in the order they joined, with the size each asked for.
        var streams: [(id: ObjectIdentifier, size: TerminalSize?)]

        /// The newest size a phone still asks for, else the original.
        var current: TerminalSize {
            streams.last(where: { $0.size != nil })?.size ?? original
        }
    }

    private var panes: [String: Pane] = [:]

    /// A stream starts watching `pane`, which is `size` now. The first one
    /// records that as the size to give back.
    func join(pane: String, stream: ObjectIdentifier, size: TerminalSize) {
        panes[pane, default: Pane(original: size, streams: [])].streams.append((stream, nil))
    }

    /// A stream's phone asks for `size`, or nil to stop asking. Returns the
    /// size the pane should have now.
    func request(pane: String, stream: ObjectIdentifier, size: TerminalSize?) -> TerminalSize {
        guard var entry = panes[pane], let index = entry.streams.firstIndex(where: { $0.id == stream })
        else { return size ?? TerminalSize(cols: 80, rows: 24) }
        entry.streams.remove(at: index)
        entry.streams.append((stream, size))
        panes[pane] = entry
        return entry.current
    }

    /// A stream stops watching. Returns the size the pane should have now:
    /// the newest one another phone asked for, or the original once no phone
    /// is left. Nil when the stream was unknown.
    func leave(pane: String, stream: ObjectIdentifier) -> TerminalSize? {
        guard var entry = panes[pane], let index = entry.streams.firstIndex(where: { $0.id == stream })
        else { return nil }
        let asked = entry.streams[index].size != nil
        entry.streams.remove(at: index)
        if entry.streams.isEmpty {
            panes[pane] = nil
            return entry.original
        }
        panes[pane] = entry
        // A stream that never resized leaves the size as the others set it.
        return asked ? entry.current : nil
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
