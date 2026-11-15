import TermyRemoteKit
import XCTest

@testable import Termy

/// The relay side of a `HostLink`: records packets, answers trust questions.
@MainActor private final class FakeTransport: HostTransport {
    let identity = DeviceIdentity()
    var verdict = TrustStore.Verdict.trusted
    var relayConnected = true
    var sent: [Data] = []
    var savedConfiguration: RemoteConfiguration?

    func verdict(for host: RelayDevice) -> TrustStore.Verdict { verdict }
    func isConnected(to host: UUID) -> Bool { relayConnected }
    func send(_ packet: Data, to host: UUID) { sent.append(packet) }
    func configuration(for host: UUID) -> RemoteConfiguration? { nil }
    func saveConfiguration(_ configuration: RemoteConfiguration, for host: UUID) { savedConfiguration = configuration }
    func sessionsChanged(for host: UUID) {}
}

/// A Mac at the other end of the encrypted channel.
@MainActor private struct FakeHost {
    let identity = DeviceIdentity()
    var channel: SecureChannel?

    var device: RelayDevice {
        let json = """
            {"id":"\(Self.id.uuidString)","name":"Mac","kind":"host",
             "publicKey":"\(identity.publicKey.base64EncodedString())","online":true,"createdAt":0}
            """
        return try! JSONDecoder().decode(RelayDevice.self, from: Data(json.utf8))
    }
    static let id = UUID()

    /// Answers the link's handshake init.
    mutating func accept(_ initPacket: Data, on link: HostLink) throws {
        let (channel, response) = try SecureChannel.respond(identity: identity, initPacket: initPacket)
        self.channel = channel
        link.receive(response)
    }

    mutating func send(_ message: HostMessage, to link: HostLink) throws {
        link.receive(try channel!.seal(try MessageCodec.encode(message)))
    }

    /// Decodes everything the phone sent since the handshake.
    mutating func read(_ transport: FakeTransport) throws -> [ClientMessage] {
        defer { transport.sent.removeAll() }
        return try transport.sent.map { try MessageCodec.decode(ClientMessage.self, from: try channel!.open(packet: $0)) }
    }
}

@MainActor final class HostLinkTests: XCTestCase {
    private var transport: FakeTransport!
    private var host: FakeHost!
    private var link: HostLink!

    override func setUp() async throws {
        transport = FakeTransport()
        host = FakeHost()
        link = HostLink(device: host.device, model: transport)
    }

    private func connect() throws {
        link.connect()
        XCTAssertEqual(link.state, .connecting)
        XCTAssertEqual(transport.sent.count, 1)
        let initPacket = transport.sent.removeFirst()
        try host.accept(initPacket, on: link)
    }

    func testUnpinnedOrChangedKeysNeverHandshake() {
        transport.verdict = .unknown
        link.connect()
        XCTAssertEqual(link.state, .needsTrust)
        transport.verdict = .changed
        link.connect()
        XCTAssertEqual(link.state, .keyChanged)
        XCTAssertTrue(transport.sent.isEmpty)
    }

    func testHandshakeConnectsAndAsksForSessions() throws {
        try connect()
        XCTAssertEqual(link.state, .connected)
        XCTAssertEqual(try host.read(transport), [.listSessions])
    }

    func testWaitsForRelayBeforeHandshake() {
        transport.relayConnected = false
        link.connect()
        XCTAssertEqual(link.state, .connecting)
        XCTAssertTrue(transport.sent.isEmpty)
        transport.relayConnected = true
        link.relayReady()
        XCTAssertEqual(transport.sent.count, 1)
    }

    func testReconnectReattachesOpenScreens() throws {
        try connect()
        let screen = link.open(RemotePane(id: "p1", title: nil, workingDirectory: nil, exited: false))
        _ = try host.read(transport)
        try host.send(.frame(Self.frame(stream: screen.stream, text: "hi")), to: link)
        XCTAssertTrue(screen.live)

        link.relayWentAway()
        XCTAssertEqual(link.state, .offline)
        XCTAssertFalse(screen.live)

        link.relayReady()
        try host.accept(transport.sent.removeFirst(), on: link)
        XCTAssertEqual(link.state, .connected)
        XCTAssertEqual(try host.read(transport), [.listSessions, .attach(stream: screen.stream, paneID: "p1")])
    }

    func testShortcutCommandRunsInTheCreatedPane() throws {
        try connect()
        _ = try host.read(transport)
        link.commandForNextCreation = "make test"
        link.send(.createSession(name: "Shortcuts"))
        XCTAssertTrue(link.creating)
        try host.send(.created(paneID: "new"), to: link)
        XCTAssertFalse(link.creating)
        XCTAssertEqual(link.createdPaneID, "new")

        let screen = link.open(RemotePane(id: "new", title: nil, workingDirectory: nil, exited: false))
        _ = try host.read(transport)
        try host.send(.frame(Self.frame(stream: screen.stream, text: "$ ")), to: link)
        XCTAssertEqual(
            try host.read(transport),
            [.paste(stream: screen.stream, text: "make test"), .key(stream: screen.stream, key: RemoteKey(key: "enter"))])

        // Only once, and only for that pane.
        try host.send(.frame(Self.frame(stream: screen.stream, text: "$ make test")), to: link)
        XCTAssertEqual(try host.read(transport), [])
    }

    func testShellEventsReachTheirScreenOnly() throws {
        try connect()
        let one = link.open(RemotePane(id: "p1", title: nil, workingDirectory: nil, exited: false))
        let two = link.open(RemotePane(id: "p2", title: nil, workingDirectory: nil, exited: false))
        try host.send(.shell(stream: one.stream, event: .executing), to: link)
        XCTAssertEqual(one.shellPhase, .running)
        XCTAssertEqual(two.shellPhase, .unknown)
        try host.send(.shell(stream: one.stream, event: .finished(exitCode: 1)), to: link)
        XCTAssertEqual(one.shellPhase, .prompt)
    }

    func testPaneCommandStateArrivesWithTheSessionList() throws {
        try connect()
        let state = RemoteCommandState(running: true, finished: 2, exitCode: 0, runningSince: Date())
        try host.send(
            .sessions(
                RemoteSessionList(
                    groups: [], panes: [RemotePane(id: "a", title: nil, workingDirectory: nil, exited: false, command: state)])),
            to: link)
        XCTAssertEqual(link.pane("a")?.command, state)
    }

    func testCommandOutputNeedsAVersionFiveHostAndRoutesTheReply() throws {
        try connect()
        let screen = link.open(RemotePane(id: "p1", title: nil, workingDirectory: nil, exited: false))
        _ = try host.read(transport)
        var replies: [RemoteCommandOutput?] = []
        // A version 4 host: nothing is asked.
        try host.send(.hello(hostName: "Mac", version: 4), to: link)
        _ = try host.read(transport)
        link.requestOutput(of: screen) { replies.append($0) }
        XCTAssertEqual(replies, [nil])
        XCTAssertEqual(try host.read(transport), [])

        try host.send(.hello(hostName: "Mac", version: 5), to: link)
        _ = try host.read(transport)
        XCTAssertTrue(link.followsPrompts)
        link.requestOutput(of: screen, promptBufferRow: 7) { replies.append($0) }
        // A newer request replaces the one before; that one hears nil.
        link.requestOutput(of: screen) { replies.append($0) }
        XCTAssertEqual(
            try host.read(transport),
            [.commandOutput(stream: screen.stream, promptBufferRow: 7), .commandOutput(stream: screen.stream, promptBufferRow: nil)])
        let output = RemoteCommandOutput(command: "ls", text: "a\nb", exitCode: 0, truncated: false)
        try host.send(.commandOutput(stream: screen.stream, output: output), to: link)
        XCTAssertEqual(replies, [nil, nil, output])
    }

    func testFramesCarryPromptRows() throws {
        try connect()
        let screen = link.open(RemotePane(id: "p1", title: nil, workingDirectory: nil, exited: false))
        var frame = Self.frame(stream: screen.stream, text: "$ ls")
        frame.promptRows = [0]
        frame.historySize = 50
        frame.displayOffset = 10
        try host.send(.frame(frame), to: link)
        XCTAssertEqual(screen.promptRows, [0])
        XCTAssertEqual(screen.bufferRow(ofScreenRow: 1), 41)
        try host.send(.frame(Self.frame(stream: screen.stream, text: "$ ls")), to: link)
        XCTAssertEqual(screen.promptRows, [])
    }

    func testFailedCreationDropsTheCommand() throws {
        try connect()
        link.commandForNextCreation = "rm -rf build"
        link.send(.createSession(name: "Shortcuts"))
        try host.send(.error("No"), to: link)
        XCTAssertNil(link.commandForNextCreation)
        XCTAssertEqual(link.lastError, "No")
    }

    func testDamagedMessageIsDroppedNotFatal() throws {
        try connect()
        link.receive(Data([2, 0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3]))
        XCTAssertEqual(link.state, .connected)
        XCTAssertEqual(link.lastError, "Dropped a damaged message.")
    }

    func testConfigurationIsSavedAndApplied() throws {
        try connect()
        let screen = link.open(RemotePane(id: "p1", title: nil, workingDirectory: nil, exited: false))
        var config = RemoteConfiguration()
        config.fontSize = 17
        try host.send(.configuration(config), to: link)
        XCTAssertEqual(transport.savedConfiguration?.fontSize, 17)
        XCTAssertEqual(screen.configuration.fontSize, 17)
    }

    func testOrderedPanesFollowSessionsThenLoosePanes() throws {
        try connect()
        let panes = ["a", "b", "c", "d"].map { RemotePane(id: $0, title: $0, workingDirectory: nil, exited: $0 == "c") }
        var group = RemoteSessionGroup(name: "Work", tabs: [.init(title: nil, paneIDs: ["b", "a"]), .init(title: nil, paneIDs: ["c"])])
        group.windowID = "w"
        try host.send(.sessions(RemoteSessionList(groups: [group], panes: panes)), to: link)
        XCTAssertEqual(link.orderedPanes.map(\.id), ["b", "a", "d"])
        XCTAssertEqual(link.group(containing: "a")?.name, "Work")
        XCTAssertNil(link.group(containing: "d"))
    }

    static func frame(stream: Int, text: String) -> RemoteFrame {
        RemoteFrame(
            stream: stream, cols: 20, rows: 2, full: true,
            updates: [RowUpdate(row: 0, runs: [CellRun(col: 0, cells: text.count, text: text, fg: 0, bg: 0, flags: 0)])],
            cursor: RemoteCursor(row: 0, col: text.count, visible: true, style: "block"), background: 0)
    }
}
