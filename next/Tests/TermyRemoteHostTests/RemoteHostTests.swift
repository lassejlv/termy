import Foundation
import Network
import Termy
import Testing

@testable import TermyRemoteHost
@testable import TermyRemoteKit

/// A phone that streams, resizes and edits sessions over a real direct connection.
@Suite("Remote terminal", .serialized)
@MainActor struct RemoteHostTests {
    @Test(.timeLimit(.minutes(1)))
    func pairedPhoneStreamsResizesAndEditsSessions() async throws {
        let hostIdentity = DeviceIdentity()
        let phoneIdentity = DeviceIdentity()
        let phoneID = UUID()
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("termy-remote-test-\(UUID())")
        let mux = try Multiplexer.connect(root: root, hostExecutable: #require(Multiplexer.bundledHostExecutable))
        defer {
            try? mux.shutdown()
            try? FileManager.default.removeItem(at: root)
        }
        let agent = RemoteHostAgent(
            multiplexer: mux, hostID: UUID(), identity: hostIdentity,
            trustedDevices: [phoneID: phoneIdentity.publicKey])
        agent.directPort = nil
        agent.interfaces = { [NetworkInterface(name: "utun8", address: IPv4("100.81.122.65")!)] }
        var advertised: [DirectEndpoint] = []
        agent.onDirectEndpoints = { advertised = $0 }
        let configURL = root.appendingPathComponent("config.txt")
        try "font_size = 14\nshell = /bin/sh\nshell_integration_enabled = false\nscrollback_history = 1000\n".write(
            to: configURL, atomically: true, encoding: .utf8)
        let parsed = try TermyConfig(path: configURL.path)
        var synced = RemoteConfiguration(
            render: try parsed.renderConfig(),
            contents:
                "font_size = 14\nshell = /bin/sh\nshell_integration_enabled = false\nscrollback_history = 1000\n")
        agent.configPath = configURL.path
        agent.configurationProvider = { synced }
        let serving = Task { try await agent.run() }
        defer {
            serving.cancel()
            agent.stop()
        }
        try await eventually { !advertised.isEmpty }
        let client = TestPhone(port: advertised[0].port, hostKey: hostIdentity.publicKey, identity: phoneIdentity)
        client.negotiateConfiguration = false
        client.start()
        defer { client.stop() }
        try await eventually { client.channel != nil }
        try client.send(.listSessions)
        try await eventually { client.sessionResponses > 0 }
        try await Task.sleep(for: .milliseconds(2200))
        #expect(client.configuration == nil)  // Version-1 phones receive no unknown config messages.
        client.negotiateConfiguration = true
        try client.send(.clientHello(version: RemoteProtocol.version))
        try await eventually { client.configuration?.fontSize == 14 }
        try client.send(.createSession(name: "Mobile Work"))
        try await eventually { client.createdPane != nil }
        let paneID = try #require(client.createdPane)
        #expect(try mux.layout().windows[0].workspaces[0].name == "Mobile Work")
        client.createdPane = nil
        try client.send(.createTab(inSessionContaining: paneID))
        try await eventually { client.createdPane != nil }
        let tabPane = try #require(client.createdPane)
        #expect(try mux.layout().windows[0].workspaces[0].tabs.count == 2)
        client.createdPane = nil
        try client.send(.splitPane(paneID: tabPane, horizontal: true))
        try await eventually { client.createdPane != nil }
        #expect(try mux.layout().windows[0].workspaces[0].tabs[1].tree.paneIDs.count == 2)
        try client.send(.renameSession(containing: paneID, name: "Phone Session"))
        try client.send(.renameTab(containing: tabPane, title: "Tools"))
        try await eventually {
            client.sessions.groups.first?.name == "Phone Session"
                && client.sessions.groups.first?.tabs.last?.title == "Tools"
        }
        try client.send(.attach(stream: 1, paneID: paneID))
        try await eventually { !client.frames.isEmpty }
        // Octal output ensures this marker comes from the program, not shell input echo.
        try client.send(
            .paste(stream: 1, text: "printf '\\124\\105\\122\\115\\131\\137\\120\\101\\111\\122\\105\\104\\n'\n"))
        try await eventually {
            client.frames.contains { $0.updates.contains { $0.runs.map(\.text).joined().contains("TERMY_PAIRED") } }
        }
        try client.send(.search(stream: 1, query: "TERMY_PAIRED"))
        try await eventually { !client.matches.isEmpty }
        synced.fontSize = 16
        synced.contents =
            "font_size = 16\nshell = /bin/sh\nshell_integration_enabled = false\nscrollback_history = 1000\n"
        try "font_size = 16\nshell = /bin/sh\nshell_integration_enabled = false\nscrollback_history = 1000\n".write(
            to: configURL, atomically: true, encoding: .utf8)
        try await eventually { client.configuration?.fontSize == 16 }
        try client.send(.paste(stream: 1, text: "printf '\u{1b}[31m╭──╮ ✓ 界\u{1b}[0m\\n'\n"))
        try await eventually {
            client.frames.contains {
                $0.updates.contains { $0.runs.contains { $0.text == "界" && $0.glyphPNG != nil } }
            }
        }
        try client.send(.resize(stream: 1, cols: 42, rows: 12))
        try await eventually { client.frames.contains { $0.cols == 42 && $0.rows == 12 && $0.full } }
        try client.send(
            .paste(stream: 1, text: "i=0; while [ $i -lt 80 ]; do echo mobile-line-$i; i=$((i+1)); done\n"))
        do { try await eventually { client.frames.contains { ($0.historySize ?? 0) > 20 } } } catch {
            let snapshot = try mux.attachTerminal(paneID: paneID).snapshot()
            print(
                "SCROLL DIAGNOSTIC", snapshot.historySize, client.frames.last?.historySize as Any,
                (0..<snapshot.rows).map {
                    snapshot.cells[$0 * snapshot.cols..<($0 + 1) * snapshot.cols].compactMap(\.text).joined()
                })
            throw error
        }
        try client.send(.scroll(stream: 1, lines: 8))
        try await eventually { (client.frames.last?.displayOffset ?? 0) > 0 }
        try client.send(.scrollToBottom(stream: 1))
        try await eventually { client.frames.last?.displayOffset == 0 }
        if let claude = ProcessInfo.processInfo.environment["TERMY_TEST_CLAUDE"] {
            let quoted = "'" + claude.replacingOccurrences(of: "'", with: "'\"'\"'") + "'"
            let directory = "'" + root.path.replacingOccurrences(of: "'", with: "'\"'\"'") + "'"
            try client.send(.paste(stream: 1, text: "cd \(directory); \(quoted)\n"))
            try await eventually {
                client.visibleText.localizedCaseInsensitiveContains("Claude Code")
                    || client.visibleText.localizedCaseInsensitiveContains("trust this")
            }
            try await Task.sleep(for: .seconds(2))
            if let last = client.frames.last {
                var snapshot = last
                snapshot.full = true
                snapshot.updates = client.retainedRows.enumerated().map {
                    RowUpdate(row: $0.offset, runs: $0.element)
                }
                try JSONEncoder().encode(snapshot).write(to: URL(fileURLWithPath: "/tmp/termy-claude-frame.json"))
            }
        }
        #expect(client.errors.isEmpty)
        try client.send(.detach(stream: 1))
        try await eventually { (try? mux.attachTerminal(paneID: paneID).snapshot().cols) == 80 }
    }
    private func eventually(file: StaticString = #fileID, line: UInt = #line, _ predicate: () throws -> Bool)
        async throws
    {
        let deadline = Date().addingTimeInterval(12)
        while Date() < deadline {
            if try predicate() { return }
            try await Task.sleep(for: .milliseconds(30))
        }
        throw WaitError.timedOut(String(describing: file), line)
    }
    private enum WaitError: Error { case timedOut(String, UInt) }
}
@MainActor private final class TestPhone {
    let hostKey: Data
    let identity: DeviceIdentity
    var channel: SecureChannel?
    var frames: [RemoteFrame] = []
    var retainedRows: [[CellRun]] = []
    var visibleText: String { retainedRows.map { $0.map(\.text).joined() }.joined(separator: "\n") }
    var createdPane: String?
    var negotiateConfiguration = true
    var sessionResponses = 0
    var configuration: RemoteConfiguration?
    var sessions = RemoteSessionList(groups: [], panes: [])
    var matches: [RemoteSearchMatch] = []
    var errors: [String] = []
    private var handshake: SecureChannel.ClientHandshake?
    private let connection: DirectConnection
    private var task: Task<Void, Never>?
    init(port: Int, hostKey: Data, identity: DeviceIdentity) {
        self.hostKey = hostKey
        self.identity = identity
        // Loopback isn't an address a phone dials, so the test builds its connection itself.
        connection = DirectConnection(
            NWConnection(
                host: "127.0.0.1", port: NWEndpoint.Port(rawValue: UInt16(port))!,
                using: DirectConnection.parameters()),
            remote: nil, timeout: 3)
    }
    func start() {
        task = Task {
            for await event in connection.events {
                do {
                    switch event {
                    case .ready: connect()
                    case .packet(let packet):
                        if let handshake {
                            channel = try handshake.finish(packet)
                            self.handshake = nil
                            // The Mac takes the phone as paired once its first sealed message opens.
                            try send(.listSessions)
                        } else if var c = channel {
                            let message = try MessageCodec.decode(HostMessage.self, from: c.open(packet: packet))
                            channel = c
                            switch message {
                            case .hello(_, let version):
                                if version >= 2, negotiateConfiguration {
                                    try send(.clientHello(version: RemoteProtocol.version))
                                }
                            case .created(let id): createdPane = id
                            case .frame(let f):
                                frames.append(f)
                                if f.full || retainedRows.count != f.rows {
                                    retainedRows = Array(repeating: [], count: f.rows)
                                }
                                for update in f.updates where retainedRows.indices.contains(update.row) {
                                    retainedRows[update.row] = update.runs
                                }
                            case .configuration(let config): configuration = config
                            case .sessions(let value):
                                sessions = value
                                sessionResponses += 1
                            case .searchResults(_, let value): matches = value
                            case .error(let message): errors.append(message)
                            default: break
                            }
                        }
                    case .closed: break
                    }
                } catch { Issue.record(error) }
            }
        }
    }
    private func connect() {
        guard channel == nil, handshake == nil else { return }
        let h = SecureChannel.startClient(identity: identity, hostStaticKey: hostKey)
        handshake = h
        connection.send(h.initPacket)
    }
    func send(_ message: ClientMessage) throws {
        guard var c = channel else { throw ChannelError.unexpected }
        let packet = try c.seal(MessageCodec.encode(message))
        channel = c
        connection.send(packet)
    }
    func stop() {
        task?.cancel()
        connection.close()
    }
}
