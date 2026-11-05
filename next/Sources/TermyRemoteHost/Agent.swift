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

    func createPane() async throws -> String {
        let terminal = try await Task.detached { [multiplexer] in
            Handoff(try multiplexer.createTerminal(size: TerminalSize(cols: 80, rows: 24)))
        }.value.value
        // Releasing the terminal detaches and leaves the shell running.
        return terminal.paneID ?? ""
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
                let tabs = workspace.tabs.map { RemoteSessionGroup.Tab(title: $0.title, paneIDs: $0.tree.paneIDs) }
                if !tabs.isEmpty { groups.append(RemoteSessionGroup(name: workspace.name, tabs: tabs)) }
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
            streams[stream]?.session.scroll(lines: Int32(lines))
        case .createPane:
            Task {
                do {
                    guard let paneID = try await agent?.createPane() else { return }
                    send(.created(paneID: paneID))
                    agent?.broadcastSessionsIfChanged()
                } catch {
                    send(.error("Couldn't start a session: \(error)"))
                }
            }
        case .closePane(let paneID):
            Task {
                try? await agent?.closePane(paneID)
                agent?.broadcastSessionsIfChanged()
            }
        }
    }

    private func attach(stream id: Int, paneID: String) async {
        streams.removeValue(forKey: id)?.close()
        do {
            guard let terminal = try await agent?.attach(paneID: paneID) else { return }
            guard active else { return }
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
        let updates = rows.map { RowUpdate(row: $0, runs: FrameEncoder.runs(store.cells(row: $0))) }
        let background =
            (0..<store.rows).lazy.compactMap { row -> UInt32? in
                store.cells(row: row).first(where: \.usesDefaultBackground)?.background.rgba
            }.first ?? 0x0000_00ff
        peer?.send(
            .frame(
                RemoteFrame(
                    stream: id, cols: store.cols, rows: store.rows, full: full, updates: updates,
                    cursor: FrameEncoder.cursor(store.cursor), background: background
                )))
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
