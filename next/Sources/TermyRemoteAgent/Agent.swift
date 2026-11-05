import Foundation
import Termy
import TermyRemoteKit

/// Serves this computer's session host to the account's phones through the
/// relay. Each phone gets one `SecureChannel`; every screen and key inside it
/// is sealed end to end.
@MainActor
final class Agent {
    struct Options {
        var muxRoot: URL?
        /// Trust phones the first time they connect without asking.
        var trustNew = false
    }

    private let store: AgentStore
    private let credentials: AgentStore.Credentials
    private let identity: DeviceIdentity
    private let options: Options
    private var trust: TrustStore
    private var multiplexer: Multiplexer?
    private var connection: RelayConnection?
    private var peers: [UUID: Peer] = [:]
    /// Phones waiting for the user to answer the trust prompt.
    private var pendingTrust: Set<UUID> = []
    private var lastSessions: RemoteSessionList?

    init(store: AgentStore, credentials: AgentStore.Credentials, identity: DeviceIdentity, options: Options) {
        self.store = store
        self.credentials = credentials
        self.identity = identity
        self.options = options
        trust = TrustStore(url: store.trustURL)
    }

    private var api: RelayAPI { RelayAPI(baseURL: credentials.relayURL, token: credentials.token) }

    // MARK: Running

    func run() async throws {
        multiplexer = try await connectMultiplexer()
        log("Serving \(hostName) to your phones. This computer's key: \(identity.fingerprint)")
        Task { await self.watchSessions() }
        var delay: UInt64 = 1
        while true {
            let connection = RelayConnection(api: api)
            self.connection = connection
            var wasReady = false
            for await event in connection.events {
                switch event {
                case .ready:
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
            _ = try await api.me()
            return true
        } catch let error as RelayAPIError where error.status == 401 {
            return false
        } catch {
            return true
        }
    }

    private func connectMultiplexer() async throws -> Multiplexer {
        // Inside Termy Next.app the host sits next to us in Contents/Helpers.
        let sibling = Bundle.main.executableURL?.resolvingSymlinksInPath().deletingLastPathComponent()
            .appendingPathComponent("termy-session-host")
        let executable = sibling.flatMap { FileManager.default.isExecutableFile(atPath: $0.path) ? $0 : nil }
            ?? Multiplexer.bundledHostExecutable
        guard let executable else { throw AgentError.missingHost }
        let root = options.muxRoot
        return try await Task.detached {
            if let root {
                try FileManager.default.createDirectory(
                    at: root.deletingLastPathComponent(), withIntermediateDirectories: true
                )
            }
            return try Multiplexer.connect(root: root, hostExecutable: executable)
        }.value
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
        guard !pendingTrust.contains(deviceID) else { return }
        do {
            let clientKey = try SecureChannel.clientStaticKey(inInit: initPacket)
            // The phone must be a device of this account, with this key.
            let devices = try await api.devices()
            guard let device = devices.first(where: { $0.id == deviceID }), device.kind == .client,
                  device.publicKeyData == clientKey
            else {
                log("Refused a connection from an unknown device.")
                return
            }
            switch trust.verdict(deviceID: deviceID, publicKey: clientKey) {
            case .trusted:
                break
            case .changed:
                log("""
                Refused \(device.name): its key changed since you trusted it. If you signed in again on that \
                phone, run `termy remote untrust \(deviceID)` and connect again.
                """)
                return
            case .unknown:
                pendingTrust.insert(deviceID)
                let allowed = options.trustNew ? true : await askTrust(device)
                pendingTrust.remove(deviceID)
                guard allowed else {
                    log("Did not trust \(device.name).")
                    return
                }
                try trust.trust(deviceID: deviceID, name: device.name, publicKey: clientKey)
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

    private func askTrust(_ device: RelayDevice) async -> Bool {
        guard isatty(STDIN_FILENO) != 0 else {
            log("""
            \(device.name) wants to open your sessions. Its key: \(device.fingerprint)
            Run `termy remote serve` in a terminal to approve it, or `termy remote serve --trust-new`.
            """)
            return false
        }
        print("""

        \(device.name) wants to open your terminal sessions.
        Check that the key below matches Settings → This iPhone in the Termy app:
            \(device.fingerprint)
        Allow it? [y/N]
        """, terminator: "")
        fflush(stdout)
        let answer = await Task.detached { readLine() }.value
        return answer?.lowercased().hasPrefix("y") == true
    }

    // MARK: Sessions

    func send(_ packet: Data, to device: UUID) {
        connection?.send(packet, to: device)
    }

    func attach(paneID: String) async throws -> Terminal {
        guard let multiplexer else { throw AgentError.missingHost }
        // A new terminal has no other owner yet, so handing it over is safe.
        return try await Task.detached { Handoff(try multiplexer.attachTerminal(paneID: paneID)) }.value.value
    }

    func createPane() async throws -> String {
        guard let multiplexer else { throw AgentError.missingHost }
        let terminal = try await Task.detached {
            Handoff(try multiplexer.createTerminal(size: TerminalSize(cols: 80, rows: 24)))
        }.value.value
        // Releasing the terminal detaches and leaves the shell running.
        return terminal.paneID ?? ""
    }

    func closePane(_ paneID: String) async throws {
        guard let multiplexer else { throw AgentError.missingHost }
        try await Task.detached { try multiplexer.close(paneID: paneID) }.value
    }

    func currentSessions() -> RemoteSessionList {
        guard let multiplexer else { return RemoteSessionList(groups: [], panes: []) }
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
        while true {
            try? await Task.sleep(nanoseconds: 2_000_000_000)
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
    private weak var agent: Agent?
    private var streams: [Int: Stream] = [:]

    init(id: UUID, name: String, channel: SecureChannel, agent: Agent) {
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
        for stream in streams.values { stream.close() }
        streams.removeAll()
    }

    func handle(_ message: ClientMessage) {
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
        let rows: [Int] = pendingFull ? Array(0 ..< store.rows) : (dirty ?? []).filter { $0 < store.rows }.sorted()
        let full = pendingFull
        pendingFull = false
        dirty = nil
        let updates = rows.map { RowUpdate(row: $0, runs: FrameEncoder.runs(store.cells(row: $0))) }
        let background = (0 ..< store.rows).lazy.compactMap { row -> UInt32? in
            store.cells(row: row).first(where: \.usesDefaultBackground)?.background.rgba
        }.first ?? 0x000000ff
        peer?.send(.frame(RemoteFrame(
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
        case .signedOut: "This computer was signed out. Run `termy remote login`."
        }
    }
}

func log(_ message: String) {
    let time = Date().formatted(date: .omitted, time: .standard)
    FileHandle.standardError.write(Data("[\(time)] \(message)\n".utf8))
}
