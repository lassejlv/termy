import AppKit
import Foundation
import Termy
import TermyRemoteKit
import Testing

@testable import TermyRemoteHost

@MainActor struct MobileFeatureTests {
    @Test func unicodeCellsUseHostGlyphsAndKeepColumns() throws {
        let config = try TermyConfig(contents: "font_size = 14\n")
        let render = try config.renderConfig()
        let terminal = try Terminal.display(size: TerminalSize(cols: 32, rows: 4))
        try terminal.feed(Data("\u{1b}[31m╭──╮ A界 ✓ \u{f120}\u{1b}[0m".utf8))
        let store = FrameStore()
        _ = try terminal.updateFrame(store, forceFull: true)
        let glyphs = try store.glyphPlan(metrics: render.glyphMetrics)
        let indexed = Dictionary(uniqueKeysWithValues: glyphs.map { ($0.cellIndex, $0) })
        let runs = FrameEncoder.runs(
            store.cells(row: 0), configuration: RemoteConfiguration(render: render, contents: ""), row: 0,
            cols: store.cols, glyphs: indexed)
        #expect(runs.map(\.cells).reduce(0, +) == 32)
        let wide = try #require(runs.first(where: { $0.text == "界" }))
        #expect(wide.cells == 2)
        #expect(wide.glyphPNG != nil)
        for run in runs where run.text.unicodeScalars.contains(where: { !$0.isASCII }) {
            let data = try #require(run.glyphPNG)
            let image = try #require(NSBitmapImageRep(data: data))
            #expect(image.pixelsWide > 0)
            #expect(image.pixelsHigh > 0)
            let alpha = (0..<image.pixelsWide).contains { x in
                (0..<image.pixelsHigh).contains { y in (image.colorAt(x: x, y: y)?.alphaComponent ?? 0) > 0 }
            }
            // The configured Mac font may not include every private-use glyph, but box/CJK must have pixels.
            if !run.text.unicodeScalars.contains(where: { (0xe000...0xf8ff).contains($0.value) }) { #expect(alpha) }
        }
        let red = try #require(runs.first)
        #expect(red.fg != render.foreground.rgba)
        if let image = runs.first?.glyphPNG {
            try image.write(to: URL(fileURLWithPath: "/tmp/termy-mobile-corner.png"))
        }
    }
    /// Glyphs a frame hasn't drawn before are drawn off the main actor, and
    /// come out as they do on it, each named by its content.
    @Test func glyphsDrawnOffTheMainActorMatchAndAreNamed() async throws {
        let config = try TermyConfig(contents: "font_size = 13\n")
        let render = try config.renderConfig()
        let remote = RemoteConfiguration(render: render, contents: "")
        let terminal = try Terminal.display(size: TerminalSize(cols: 20, rows: 2))
        // Glyphs no other test draws, so the cache doesn't have them yet.
        try terminal.feed(Data("\u{1b}[35m\u{2654}\u{2655} \u{2656}\u{1b}[0m".utf8))
        let store = FrameStore()
        _ = try terminal.updateFrame(store, forceFull: true)
        let runs = FrameEncoder.runs(store.cells(row: 0), configuration: remote, row: 0, cols: 20, rasterize: false)
        #expect(runs.allSatisfy { $0.glyphPNG == nil })
        var frame = RemoteFrame(
            stream: 1, cols: 20, rows: 2, full: true, updates: [RowUpdate(row: 0, runs: runs)],
            cursor: RemoteCursor(row: 0, col: 0, visible: true, style: "block"), background: 0)
        let slots = FrameEncoder.attachGlyphs(to: &frame, configuration: remote, glyphs: [:])
        #expect(!slots.isEmpty)
        let jobs = Handoff(slots.map(\.job))
        let drawn = await Task.detached { Handoff(jobs.value.map(GlyphRasterizer.render)) }.value.value
        for (slot, png) in zip(slots, drawn) {
            let run = frame.updates[slot.update].runs[slot.run]
            let onMain = GlyphRasterizer.png(
                text: run.text, cells: run.cells, flags: run.flags, color: run.fg, config: remote, plan: nil)
            #expect(png != nil && png == onMain)
            #expect(GlyphRasterizer.glyphKey(slot.job.key).count == 32)
        }
        // Drawn once, a glyph is in the cache for the next frame.
        var again = frame
        #expect(FrameEncoder.attachGlyphs(to: &again, configuration: remote, glyphs: [:]).isEmpty)
        #expect(again.updates[0].runs.contains { $0.glyphKey != nil && $0.glyphPNG != nil })
    }

    @Test func asciiCellsJoinOneRunPerStyle() throws {
        let terminal = try Terminal.display(size: TerminalSize(cols: 300, rows: 2))
        try terminal.feed(Data((String(repeating: "x", count: 200) + "\u{1b}[1mBOLD\u{1b}[0m 界z").utf8))
        let store = FrameStore()
        _ = try terminal.updateFrame(store, forceFull: true)
        let runs = FrameEncoder.runs(store.cells(row: 0))
        #expect(runs.map(\.text) == [String(repeating: "x", count: 200), "BOLD", " ", "界", "z" + String(repeating: " ", count: 92)])
        #expect(runs.map(\.cells) == [200, 4, 1, 2, 93])
        #expect(runs.map(\.col) == [0, 200, 204, 205, 207])
    }

    /// The relay closes a socket that sends more than 4 MiB, dropping every
    /// phone; a frame that large must go as several that rebuild the screen.
    @Test func oversizedFramesSplitIntoRowPatches() throws {
        let cursor = RemoteCursor(row: 0, col: 0, visible: true, style: "block")
        let rows = (0..<40).map { row in
            RowUpdate(row: row, runs: [.init(col: 0, cells: 1, text: "\(row)-" + UUID().uuidString, fg: 0, bg: 0, flags: 0)])
        }
        let frame = RemoteFrame(stream: 1, cols: 80, rows: 40, full: true, updates: rows, cursor: cursor, background: 0)
        let payloads = try FrameEncoder.payloads(for: .frame(frame), limit: 400)
        #expect(payloads.count > 1)
        var seen: [Int] = []
        for (index, payload) in payloads.enumerated() {
            #expect(payload.count <= 400)
            guard case .frame(let part) = try MessageCodec.decode(HostMessage.self, from: payload) else {
                Issue.record("not a frame")
                continue
            }
            #expect(part.full == (index == 0))
            #expect(part.isValid)
            seen += part.updates.map(\.row)
        }
        #expect(seen == Array(0..<40))
        #expect(try FrameEncoder.payloads(for: .frame(frame), limit: 1 << 20).count == 1)
    }

    /// A detach, or a second attach of the same stream, that arrives while an
    /// attach is in flight used to leave a stream nobody watched running and
    /// sending frames until the phone disconnected.
    @Test(.timeLimit(.minutes(1))) func detachingDuringAnAttachLeavesNoStream() async throws {
        let host = try #require(Multiplexer.bundledHostExecutable)
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("termy-remote-\(UUID().uuidString.prefix(8))")
        let mux = try Multiplexer.connect(root: root, hostExecutable: host)
        defer {
            try? mux.shutdown()
            try? FileManager.default.removeItem(at: root)
        }
        let pane = try #require(
            try mux.createTerminal(
                size: TerminalSize(cols: 40, rows: 5), options: TerminalOptions(startupCommand: "/bin/sh -c 'sleep 30'")
            ).paneID)
        let identity = DeviceIdentity()
        let phone = DeviceIdentity()
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        let device = try decoder.decode(
            RelayDevice.self,
            from: Data(
                #"{"id":"\#(UUID().uuidString)","name":"Mac","kind":"host","publicKey":"\#(identity.publicKey.base64EncodedString())","online":true,"createdAt":"2026-01-01T00:00:00Z"}"#
                    .utf8))
        let agent = RemoteHostAgent(
            multiplexer: mux,
            credentials: DeviceCredentials(relayURL: URL(string: "http://127.0.0.1:9")!, token: "", device: device),
            identity: identity, trustedDevices: [:])
        let handshake = SecureChannel.startClient(identity: phone, hostStaticKey: identity.publicKey)
        let (channel, _) = try SecureChannel.respond(identity: identity, initPacket: handshake.initPacket)
        let peer = Peer(id: UUID(), name: "iPhone", channel: channel, agent: agent)
        defer { peer.close() }

        peer.handle(.attach(stream: 1, paneID: pane))
        peer.handle(.detach(stream: 1))
        peer.handle(.attach(stream: 2, paneID: pane))
        peer.handle(.attach(stream: 2, paneID: pane))
        let deadline = Date().addingTimeInterval(10)
        while peer.streamCount < 1, Date() < deadline { try await Task.sleep(for: .milliseconds(20)) }
        try await Task.sleep(for: .milliseconds(300))
        #expect(peer.streamCount == 1)
    }

    /// Two phones on one pane: the second attaches after the first resized
    /// it, so it must not take the first phone's size for the Mac's.
    @Test @MainActor func theLastPhoneToLeaveRestoresTheSizeFromBeforeAnyPhone() {
        final class Token {}
        let sizes = PaneSizes()
        let (a, b) = (Token(), Token())
        let mac = TerminalSize(cols: 120, rows: 40, cellWidth: 8, cellHeight: 16)
        let phoneA = TerminalSize(cols: 50, rows: 30, cellWidth: 8, cellHeight: 16)
        let phoneB = TerminalSize(cols: 45, rows: 28, cellWidth: 8, cellHeight: 16)
        sizes.join(pane: "p", stream: ObjectIdentifier(a), size: mac)
        #expect(sizes.request(pane: "p", stream: ObjectIdentifier(a), size: phoneA) == phoneA)
        // B joins while the pane is at A's size.
        sizes.join(pane: "p", stream: ObjectIdentifier(b), size: phoneA)
        #expect(sizes.request(pane: "p", stream: ObjectIdentifier(b), size: phoneB) == phoneB)
        // A leaves: B keeps its size.
        #expect(sizes.leave(pane: "p", stream: ObjectIdentifier(a)) == phoneB)
        // B leaves last: the Mac's size comes back, not A's.
        #expect(sizes.leave(pane: "p", stream: ObjectIdentifier(b)) == mac)
        #expect(sizes.leave(pane: "p", stream: ObjectIdentifier(b)) == nil)

        // A phone that stops asking hands the pane to the other's size.
        sizes.join(pane: "q", stream: ObjectIdentifier(a), size: mac)
        sizes.join(pane: "q", stream: ObjectIdentifier(b), size: mac)
        _ = sizes.request(pane: "q", stream: ObjectIdentifier(a), size: phoneA)
        _ = sizes.request(pane: "q", stream: ObjectIdentifier(b), size: phoneB)
        #expect(sizes.request(pane: "q", stream: ObjectIdentifier(b), size: nil) == phoneA)
    }

    @Test func sessionTabAndSplitEditsPreserveOtherPanes() throws {
        var layout = MultiplexerLayout()
        try RemoteLayout.addSession(&layout, pane: "one", name: "Work")
        try RemoteLayout.addSession(&layout, pane: "two", name: "Work")
        #expect(layout.windows[0].workspaces.map(\.name) == ["Work", "Work 2"])
        let (w, s, t) = try RemoteLayout.locate(layout, pane: "one")
        let old = layout.windows[w].workspaces[s].tabs[t].tree
        layout.windows[w].workspaces[s].tabs[t].tree = RemoteLayout.split(
            old, at: "one", pane: "three", horizontal: true)
        #expect(layout.windows[w].workspaces[s].tabs[t].tree.paneIDs == ["one", "three"])
        #expect(layout.windows[0].workspaces[1].tabs[0].tree.paneIDs == ["two"])
        #expect(throws: (any Error).self) { try RemoteLayout.locate(layout, pane: "missing") }
        #expect(throws: (any Error).self) { try RemoteLayout.name("bad\nname") }
    }
}
