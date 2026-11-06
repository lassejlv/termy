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
