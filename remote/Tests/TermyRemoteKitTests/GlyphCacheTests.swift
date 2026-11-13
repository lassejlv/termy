import Foundation
import Testing
@testable import TermyRemoteKit

@Suite struct GlyphCacheTests {
    private func frame(_ keys: [Int]) -> RemoteFrame {
        let runs = keys.enumerated().map { index, key in
            var run = CellRun(col: index, cells: 1, text: "\u{2603}", fg: 0, bg: 0, flags: 0)
            run.glyphKey = "glyph-\(key)"
            run.glyphPNG = Data("png-\(key)".utf8)
            return run
        }
        return RemoteFrame(
            stream: 1, cols: max(1, keys.count), rows: 1, full: true, updates: [RowUpdate(row: 0, runs: runs)],
            cursor: RemoteCursor(row: 0, col: 0, visible: true, style: "block"), background: 0)
    }

    /// Whatever order glyphs come in, every run the host sends without its
    /// PNG names one the phone still holds.
    @Test func thePhoneHoldsEveryGlyphTheHostLeavesOut() throws {
        var ledger = GlyphLedger(capacity: 16)
        let cache = GlyphCache(capacity: 32)
        var generator = SystemRandomNumberGenerator()
        var leftOut = 0
        for _ in 0..<500 {
            let keys = (0..<Int.random(in: 1...20, using: &generator)).map { _ in Int.random(in: 0..<60, using: &generator) }
            var sent = frame(keys)
            ledger.prepare(&sent)
            leftOut += sent.updates[0].runs.filter { $0.glyphPNG == nil }.count
            // Encoded and decoded, as on the wire.
            var received = try MessageCodec.decode(
                HostMessage.self, from: MessageCodec.encode(HostMessage.frame(sent))).frame()
            cache.resolve(&received)
            for (run, key) in zip(received.updates[0].runs, keys) {
                #expect(run.glyphPNG == Data("png-\(key)".utf8))
            }
        }
        #expect(leftOut > 1000, "the ledger left out \(leftOut) PNGs")
        #expect(ledger.count <= 16 && cache.count <= 32)
    }

    @Test func olderFramesWithoutKeysStillDecode() throws {
        let json = #"{"c":0,"n":1,"s":"x","f":1,"b":2,"a":0,"g":"cG5n"}"#
        let run = try JSONDecoder().decode(CellRun.self, from: Data(json.utf8))
        #expect(run.glyphKey == nil && run.glyphPNG == Data("png".utf8))
    }

    @Test func wideFramesAreClippedForOlderPhones() {
        var wide = RemoteFrame(
            stream: 1, cols: 700, rows: 400, full: true,
            updates: [
                RowUpdate(row: 0, runs: [CellRun(col: 490, cells: 20, text: String(repeating: "a", count: 20), fg: 0, bg: 0, flags: 0)]),
                RowUpdate(row: 350, runs: [CellRun(col: 0, cells: 1, text: "b", fg: 0, bg: 0, flags: 0)]),
            ],
            cursor: RemoteCursor(row: 380, col: 10, visible: true, style: "block"), background: 0)
        // A phone of protocol 4 takes 700 by 400, not 1200 wide.
        #expect(wide.isValid)
        wide.cols = 1200
        #expect(!wide.isValid)
        wide.cols = 700
        let clipped = wide.clipped(cols: RemoteFrame.legacyMaxCols, rows: RemoteFrame.legacyMaxRows)
        #expect(clipped.cols == 500 && clipped.rows == 300)
        #expect(clipped.updates.count == 1)
        #expect(clipped.updates[0].runs[0].cells == 10 && clipped.updates[0].runs[0].text.count == 10)
        #expect(!clipped.cursor.visible)
        #expect(clipped.isValid)
    }
}

private extension HostMessage {
    func frame() throws -> RemoteFrame {
        guard case .frame(let frame) = self else { throw ChannelError.malformed }
        return frame
    }
}
