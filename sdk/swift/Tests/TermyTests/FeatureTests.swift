import Foundation
import Testing
@testable import Termy

@Suite("Search, links, graphics, and config")
struct FeatureTests {
    @Test func tmuxArgumentsAreQuotedForTmuxsParser() {
        #expect(TmuxControl.quoted("send-keys") == "send-keys")
        #expect(TmuxControl.quoted("%1") == "%1")
        #expect(TmuxControl.quoted("") == "''")
        #expect(TmuxControl.quoted("a b; kill-server") == "'a b; kill-server'")
        #expect(TmuxControl.quoted("it's") == "'it'\\''s'")
    }

    @Test func searchFindsHistoryAndLiveRows() throws {
        let terminal = try displayTerminal(cols: 20, rows: 3)
        try terminal.feed("needle one\r\n")
        for line in 0..<10 { try terminal.feed("filler \(line)\r\n") }
        try terminal.feed("Needle two")

        let matches = try terminal.search("needle")
        #expect(matches.count == 2)
        #expect(matches.first?.startCol == 0)
        #expect(matches.first?.endCol == 5)
        #expect(matches.first?.line == "needle one")

        #expect(try terminal.search("needle", options: SearchOptions(caseSensitive: true)).count == 1)
        #expect(try terminal.search("n[e]+dle \\w+", options: SearchOptions(regex: true)).count == 2)

        let frame = try terminal.snapshot()
        let live = try #require(matches.last)
        #expect(live.viewportRow(historySize: frame.historySize, displayOffset: 0, rows: 3) == 2)
        #expect(matches[0].viewportRow(historySize: frame.historySize, displayOffset: 0, rows: 3) == nil)
    }

    @Test func hyperlinksAtACell() throws {
        let terminal = try displayTerminal(
            feeding: "see \u{1b}]8;;https://termy.sh\u{1b}\\termy\u{1b}]8;;\u{1b}\\ docs")
        let link = try #require(try terminal.hyperlink(at: GridPosition(row: 0, col: 5)))
        #expect(link.uri == "https://termy.sh")
        #expect(link.startCol == 4)
        #expect(link.endCol == 8)
        #expect(try terminal.hyperlink(at: GridPosition(row: 0, col: 0)) == nil)
    }

    @Test func glyphPlansCoverBoxDrawing() throws {
        let terminal = try displayTerminal(feeding: "┌─┐ a")
        let store = FrameStore()
        try terminal.updateFrame(store)
        let plan = try store.glyphPlan(metrics: GlyphMetrics(cellWidth: 8, cellHeight: 16, fontSize: 13))
        #expect(Set(plan.map(\.cellIndex)) == [0, 1, 2])
        #expect(plan.allSatisfy { $0.kind == .boxDrawing })
        #expect(plan.allSatisfy { !$0.rects.isEmpty || !$0.strokes.isEmpty })

        let spans = [DirtySpan(row: 0, leftCol: 4, rightCol: 4)]
        #expect(try store.glyphPlan(metrics: GlyphMetrics(cellWidth: 8, cellHeight: 16, fontSize: 13), spans: spans).isEmpty)
    }

    @Test func kittyGraphicsPlacements() throws {
        let terminal = try displayTerminal()
        let before = try terminal.kittyGraphicsRevision()
        // Transmit and display a 1x1 opaque black RGBA pixel.
        try terminal.feed("\u{1b}_Ga=T,f=32,s=1,v=1,q=2;AAAA/w==\u{1b}\\")
        #expect(try terminal.kittyGraphicsRevision() != before)
        let snapshot = try terminal.kittyGraphics()
        let placement = try #require(snapshot.placements.first)
        #expect(placement.imageWidth == 1)
        #expect(placement.imageHeight == 1)
        #expect(placement.viewportRow == 0)
        #expect(placement.png.starts(with: [0x89, 0x50, 0x4E, 0x47]))
        #expect(placement.nextFrameDelay == nil)
    }

    @Test func configFromContents() throws {
        let config = try TermyConfig(contents: "font_size = 17\npadding_x = 6\n")
        #expect(!config.isLoadedFromDisk)
        let render = try config.renderConfig(for: .dark)
        #expect(render.fontSize == 17)
        #expect(render.paddingX == 6)
        #expect(render.cellWidth > 0)
        #expect(render.cellHeight > 0)
        #expect(try config.diagnostics().isEmpty)
        #expect(config.scrollbackHistory > 0)
        _ = try config.native()
        _ = try config.safety()
        #expect(!(try config.settingsSchemaJSON()).isEmpty)
    }

    @Test func configDiagnostics() throws {
        let config = try TermyConfig(contents: "not_a_real_key = 1\n")
        let diagnostics = try config.diagnostics()
        #expect(config.diagnosticCount == 1)
        #expect(diagnostics.first?.kind == .unknownRootKey)
        #expect(diagnostics.first?.lineNumber == 1)
    }

    @Test func sizeFitsRenderMetrics() throws {
        let render = try TermyConfig(contents: "padding_x = 10\npadding_y = 10\n").renderConfig()
        let size = TerminalSize.fitting(
            width: Double(render.cellWidth) * 80 + 20, height: Double(render.cellHeight) * 24 + 20,
            renderConfig: render)
        #expect(size.cols == 80)
        #expect(size.rows == 24)
    }

    @Test func defaults() {
        #expect(TermyDefaults.terminalScrollback > 0)
        #expect(TermyDefaults.queryForeground != nil)
        #expect(TerminalSize.default.cols > 0)
    }
}
