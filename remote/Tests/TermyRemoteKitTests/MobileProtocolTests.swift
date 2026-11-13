import Foundation
import Testing

@testable import TermyRemoteKit

struct MobileProtocolTests {
    @Test func configAndNewCommandsRoundTripInsideEncryption() throws {
        var config = RemoteConfiguration()
        config.contents = "font_family = Example Nerd Font\ntheme = TokyoNight\n"
        config.fontFamily = "Example Nerd Font"
        let messages: [HostMessage] = [
            .configuration(config),
            .searchResults(stream: 1, matches: [.init(row: 32, startCol: 1, endCol: 4, line: "hello")]),
        ]
        for message in messages {
            #expect(try MessageCodec.decode(HostMessage.self, from: MessageCodec.encode(message)) == message)
        }
        let commands: [ClientMessage] = [
            .clientHello(version: 2),
            .createSession(name: "Work"), .createTab(inSessionContaining: "pane"),
            .splitPane(paneID: "pane", horizontal: true), .renameSession(containing: "pane", name: "Other"),
            .mouse(stream: 1, col: 2, row: 3, action: "press"), .search(stream: 1, query: "hello"),
        ]
        for command in commands {
            #expect(try MessageCodec.decode(ClientMessage.self, from: MessageCodec.encode(command)) == command)
        }
    }
    @Test func malformedFramesAreRejectedBeforeAllocation() {
        let cursor = RemoteCursor(row: 0, col: 0, visible: true, style: "bar")
        var frame = RemoteFrame(stream: 1, cols: 80, rows: 24, full: true, updates: [], cursor: cursor, background: 0)
        #expect(frame.isValid)
        frame.rows = -1
        #expect(!frame.isValid)
        frame.rows = RemoteFrame.maxRows + 1
        #expect(!frame.isValid)
        frame.rows = 24
        frame.updates = [.init(row: -1, runs: [])]
        #expect(!frame.isValid)
        frame.updates = [.init(row: 0, runs: [.init(col: 79, cells: 2, text: "界", fg: 0, bg: 0, flags: 0)])]
        #expect(!frame.isValid)
    }
    /// `wrapped` had no coding key, so it never left the host and copied text
    /// broke every wrapped line on the phone.
    @Test func wrappedRowsTravelAndOlderFramesStillDecode() throws {
        var row = RowUpdate(row: 2, runs: [.init(col: 0, cells: 1, text: "a", fg: 0, bg: 0, flags: 0)])
        row.wrapped = true
        let decoded = try JSONDecoder().decode(RowUpdate.self, from: JSONEncoder().encode(row))
        #expect(decoded.wrapped == true)
        let older = try JSONDecoder().decode(RowUpdate.self, from: Data(#"{"r":1,"u":[]}"#.utf8))
        #expect(older.row == 1 && older.wrapped == nil)
    }

    /// Sends count toward the backlog until the socket finishes with them,
    /// including when they fail.
    @Test(.timeLimit(.minutes(1))) func relaySendsAreCountedUntilTheSocketIsDone() async throws {
        let connection = RelayConnection(api: RelayAPI(baseURL: URL(string: "http://127.0.0.1:9")!))
        defer { connection.close() }
        connection.send(Data(repeating: 7, count: 1000), to: UUID())
        #expect(connection.pendingSendBytes == 1000 + 17)
        let deadline = Date().addingTimeInterval(20)
        while connection.pendingSendBytes != 0, Date() < deadline {
            try await Task.sleep(for: .milliseconds(20))
        }
        #expect(connection.pendingSendBytes == 0)
    }

    @Test func imagesRoundTripAndMalformedPlacementsAreRejected() throws {
        let placement = RemoteImagePlacement(
            key: "7:3", frame: [2, 1, 10.5, 4], clip: [2, 1, 10.5, 4], crop: [0, 0, 1, 1], z: -1)
        let messages: [HostMessage] = [
            .image(stream: 1, key: "7:3", png: Data([0x89, 0x50, 0x4e, 0x47])),
            .images(RemoteImages(stream: 1, placements: [placement], evicted: ["6:1"], full: true)),
        ]
        for message in messages {
            #expect(try MessageCodec.decode(HostMessage.self, from: MessageCodec.encode(message)) == message)
        }
        #expect(placement.isValid)
        var broken = placement
        broken.frame = [0, 0, 1]
        #expect(!broken.isValid)
        broken.frame = [0, 0, .infinity, 1]
        #expect(!broken.isValid)
        broken.frame = [0, 0, -1, 1]
        #expect(!broken.isValid)
        #expect(!RemoteImages(stream: 1, placements: [broken]).isValid)
    }
}
