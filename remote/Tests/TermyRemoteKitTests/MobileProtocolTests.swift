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
    /// Protocol 5: shell events travel, and pane lists stay readable both ways
    /// between phones and hosts of either version.
    @Test func shellStateTravelsAndOlderPeersStillDecodePanes() throws {
        let events: [HostMessage] = [
            .shell(stream: 3, event: .prompt), .shell(stream: 3, event: .input),
            .shell(stream: 3, event: .executing), .shell(stream: 3, event: .finished(exitCode: 2)),
            .shell(stream: 3, event: .finished(exitCode: nil)),
        ]
        for message in events {
            #expect(try MessageCodec.decode(HostMessage.self, from: MessageCodec.encode(message)) == message)
        }
        let state = RemoteCommandState(
            running: false, finished: 4, exitCode: 1, runningSince: Date(timeIntervalSinceReferenceDate: 10))
        #expect(state.failed)
        #expect(!RemoteCommandState(running: true, finished: 4, exitCode: 1).failed)
        #expect(!RemoteCommandState(running: false, finished: 0, exitCode: 1).failed)
        let pane = RemotePane(id: "p", title: "zsh", workingDirectory: nil, exited: false, command: state)
        #expect(try JSONDecoder().decode(RemotePane.self, from: JSONEncoder().encode(pane)) == pane)

        // A version 4 host sends no command; a version 4 phone ignores it.
        let older = try JSONDecoder().decode(RemotePane.self, from: Data(#"{"id":"p","exited":false}"#.utf8))
        #expect(older.command == nil)
        struct VersionFourPane: Decodable { var id: String; var exited: Bool }
        #expect(try JSONDecoder().decode(VersionFourPane.self, from: JSONEncoder().encode(pane)).id == "p")
    }
    @Test func promptRowsJumpsAndCommandOutputTravel() throws {
        let messages: [HostMessage] = [
            .commandOutput(stream: 2, output: RemoteCommandOutput(command: "ls", text: "a\nb", exitCode: 0, truncated: false)),
            .commandOutput(stream: 2, output: nil),
        ]
        for message in messages {
            #expect(try MessageCodec.decode(HostMessage.self, from: MessageCodec.encode(message)) == message)
        }
        for command: ClientMessage in [
            .jumpToPrompt(stream: 1, forward: false), .commandOutput(stream: 1, promptBufferRow: 40),
            .commandOutput(stream: 1, promptBufferRow: nil),
        ] {
            #expect(try MessageCodec.decode(ClientMessage.self, from: MessageCodec.encode(command)) == command)
        }
        let cursor = RemoteCursor(row: 0, col: 0, visible: true, style: "bar")
        var frame = RemoteFrame(stream: 1, cols: 80, rows: 24, full: true, updates: [], cursor: cursor, background: 0)
        frame.promptRows = [0, 10, 23]
        #expect(frame.isValid)
        #expect(frame.clipped(cols: 80, rows: 12).promptRows == [0, 10])
        frame.promptRows = [24]
        #expect(!frame.isValid)
        frame.promptRows = [-1]
        #expect(!frame.isValid)
        // Older frames have none.
        #expect(try JSONDecoder().decode(RemoteFrame.self, from: JSONEncoder().encode(
            RemoteFrame(stream: 1, cols: 2, rows: 2, full: true, updates: [], cursor: cursor, background: 0))).promptRows == nil)
    }
    @Test func uploadsAreNamedSafelyAndQuoted() throws {
        let png = Data([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a])
        for message: ClientMessage in [.upload(stream: 1, name: "Photo 1.png", data: png)] {
            #expect(try MessageCodec.decode(ClientMessage.self, from: MessageCodec.encode(message)) == message)
        }
        #expect(try MessageCodec.decode(HostMessage.self, from: MessageCodec.encode(HostMessage.uploaded(stream: 1, path: "/tmp/a.png"))) == .uploaded(stream: 1, path: "/tmp/a.png"))
        #expect(RemoteUpload.fileName("../../etc/passwd") == "etc-passwd.bin")
        #expect(RemoteUpload.fileName(".hidden.png") == "hidden.png")
        #expect(RemoteUpload.fileName("IMG 0001.HEIC") == "IMG-0001.HEIC")
        #expect(RemoteUpload.fileName("", fallbackExtension: "jpg") == "upload.jpg")
        #expect(RemoteUpload.fileName(String(repeating: "a", count: 200) + ".png").count <= 84)
        #expect(RemoteUpload.sniffedExtension(png) == "png")
        #expect(RemoteUpload.sniffedExtension(Data([0xff, 0xd8, 0xff, 0xe0])) == "jpg")
        #expect(RemoteUpload.sniffedExtension(Data("hello".utf8)) == nil)
        #expect(RemoteUpload.shellQuoted("/tmp/a.png") == "/tmp/a.png")
        #expect(RemoteUpload.shellQuoted("/tmp/Termy Uploads/it's.png") == #"'/tmp/Termy Uploads/it'\''s.png'"#)
    }
    @Test func handoffNamesTheComputerAndPane() {
        let host = UUID()
        let parsed = RemoteHandoff.parse(RemoteHandoff.userInfo(host: host, pane: "p-1"))
        #expect(parsed?.host == host && parsed?.pane == "p-1")
        #expect(RemoteHandoff.parse(["host": "nope", "pane": "p"]) == nil)
        #expect(RemoteHandoff.parse(["host": host.uuidString]) == nil)
        #expect(RemoteHandoff.parse(nil) == nil)
    }
    @Test func longOutputKeepsItsEnd() {
        let text = String(repeating: "x", count: RemoteCommandOutput.maxBytes) + "THE END"
        let output = RemoteCommandOutput.bounded(command: "make", text: text, exitCode: 2)
        #expect(output.truncated)
        #expect(output.text.utf8.count <= RemoteCommandOutput.maxBytes)
        #expect(output.text.hasSuffix("THE END"))
        let short = RemoteCommandOutput.bounded(command: nil, text: "ok", exitCode: 0)
        #expect(!short.truncated && short.text == "ok")
        let wide = RemoteCommandOutput.bounded(
            command: nil, text: String(repeating: "界", count: RemoteCommandOutput.maxBytes / 2), exitCode: nil)
        #expect(wide.text.utf8.count <= RemoteCommandOutput.maxBytes && wide.text.allSatisfy { $0 == "界" })
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
