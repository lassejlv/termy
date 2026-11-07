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
        frame.rows = 301
        #expect(!frame.isValid)
        frame.rows = 24
        frame.updates = [.init(row: -1, runs: [])]
        #expect(!frame.isValid)
        frame.updates = [.init(row: 0, runs: [.init(col: 79, cells: 2, text: "界", fg: 0, bg: 0, flags: 0)])]
        #expect(!frame.isValid)
    }
}
