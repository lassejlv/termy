import AppKit
import Metal
import Testing
import Termy
@testable import TermyAppKit

@Suite("Metal renderer", .enabled(if: MTLCreateSystemDefaultDevice() != nil))
@MainActor
struct MetalRendererTests {
    private static let scale: CGFloat = 2

    private struct Pixels {
        var bytes: [UInt8]
        var width: Int
        var height: Int

        /// sRGB components at a pixel, 0...255.
        func rgb(x: Int, y: Int) -> (r: Int, g: Int, b: Int) {
            let index = (y * width + x) * 4
            return (Int(bytes[index + 2]), Int(bytes[index + 1]), Int(bytes[index]))
        }

        /// The brightest green component inside a rect of pixels.
        func brightest(in rect: CGRect) -> Int {
            var best = 0
            for y in Int(rect.minY)..<min(Int(rect.maxY), height) {
                for x in Int(rect.minX)..<min(Int(rect.maxX), width) { best = max(best, rgb(x: x, y: y).g) }
            }
            return best
        }
    }

    private func makeView(feeding output: String = "", cols: Int = 20, rows: Int = 5) throws -> TerminalView {
        let config = try TermyConfig(contents: "padding_x = 4\npadding_y = 4\ncursor_blink = false\n")
        let render = try config.renderConfig(for: .dark)
        let session = try TerminalSession.display(
            size: TerminalSize(cols: UInt16(cols), rows: UInt16(rows), cellWidth: render.cellWidth, cellHeight: render.cellHeight))
        let view = TerminalView(session: session, renderConfig: render, backend: .metal)
        view.frame = NSRect(origin: .zero, size: view.size(cols: cols, rows: rows))
        if !output.isEmpty { try session.feed(Data(output.utf8)) }
        return view
    }

    private func render(_ view: TerminalView) throws -> Pixels {
        let renderer = try #require(view.metalRenderer)
        let width = Int((view.bounds.width * Self.scale).rounded())
        let height = Int((view.bounds.height * Self.scale).rounded())
        let bytes = try #require(renderer.renderPixels(view.renderState, width: width, height: height, scale: Self.scale))
        return Pixels(bytes: bytes, width: width, height: height)
    }

    private func cellPixels(_ view: TerminalView, row: Int, col: Int, width: Int = 1) -> CGRect {
        let rect = view.renderer.cellRect(row: row, col: col, width: width)
        return CGRect(x: rect.minX * Self.scale, y: rect.minY * Self.scale,
                      width: rect.width * Self.scale, height: rect.height * Self.scale)
    }

    @Test func usesMetalWhenAvailable() throws {
        let view = try makeView()
        #expect(view.renderingBackend == .metal)
        #expect(view.layer is CAMetalLayer)
    }

    @Test func drawsGlyphsInTheirCells() throws {
        let view = try makeView(feeding: "\u{1b}[37mHH")
        let pixels = try render(view)
        #expect(pixels.brightest(in: cellPixels(view, row: 0, col: 0, width: 2)) > 128)
        #expect(pixels.brightest(in: cellPixels(view, row: 2, col: 0, width: 2)) < 64)
    }

    @Test func fillsCellBackgrounds() throws {
        let view = try makeView(feeding: "\u{1b}[41m  \u{1b}[0m")
        let pixels = try render(view)
        let cell = cellPixels(view, row: 0, col: 0)
        let red = pixels.rgb(x: Int(cell.midX), y: Int(cell.midY))
        #expect(red.r > 128 && red.g < 100)

        let blank = cellPixels(view, row: 3, col: 10)
        let background = pixels.rgb(x: Int(blank.midX), y: Int(blank.midY))
        let expected = view.renderConfig.background
        #expect(abs(background.r - Int(expected.r)) <= 2 && abs(background.b - Int(expected.b)) <= 2)
    }

    @Test func drawsSpecialGlyphGeometry() throws {
        let view = try makeView(feeding: "\u{1b}[37m─╭█")
        let pixels = try render(view)
        // A box-drawing line crosses the middle of its cell.
        let line = cellPixels(view, row: 0, col: 0)
        #expect(pixels.rgb(x: Int(line.midX), y: Int(line.midY)).g > 128)
        // A rounded corner is a stroke from the atlas.
        #expect(pixels.brightest(in: cellPixels(view, row: 0, col: 1)) > 128)
        // A full block fills its cell.
        let block = cellPixels(view, row: 0, col: 2)
        #expect(pixels.rgb(x: Int(block.midX), y: Int(block.minY) + 2).g > 128)
    }

    @Test func colorGlyphsKeepTheirColors() throws {
        let view = try makeView(feeding: "\u{1F7E5}")
        let pixels = try render(view)
        let cell = cellPixels(view, row: 0, col: 0, width: 2)
        var reddest = 0
        for y in Int(cell.minY)..<Int(cell.maxY) {
            for x in Int(cell.minX)..<Int(cell.maxX) {
                let pixel = pixels.rgb(x: x, y: y)
                reddest = max(reddest, pixel.r - max(pixel.g, pixel.b))
            }
        }
        #expect(reddest > 100)
    }

    @Test func drawsKittyImages() throws {
        // A 1x1 opaque red RGBA image stretched over two cells.
        let view = try makeView(feeding: "\u{1b}_Ga=T,f=32,s=1,v=1,c=2,r=1,q=2;/wAA/w==\u{1b}\\")
        #expect(view.images.count == 1)
        let pixels = try render(view)
        let cell = cellPixels(view, row: 0, col: 1)
        let pixel = pixels.rgb(x: Int(cell.midX), y: Int(cell.midY))
        #expect(pixel.r > 200 && pixel.g < 60 && pixel.b < 60)
    }

    @Test func downscalesKittyImagesWiderThanATexture() throws {
        let view = try makeView()
        let renderer = try #require(view.metalRenderer)
        // An opaque red RGBA row wider than the GPU's largest texture, sent in
        // base64 chunks the way kitty does. Only r is given, so it keeps its
        // aspect ratio as a one-cell-tall strip across the row.
        let width = renderer.maxTextureDimension + 1_000
        let pixels = Data((0..<width).flatMap { _ in [UInt8(255), 0, 0, 255] }).base64EncodedString()
        let chunks = stride(from: 0, to: pixels.count, by: 4_096).map { start -> Substring in
            let begin = pixels.index(pixels.startIndex, offsetBy: start)
            return pixels[begin..<pixels.index(begin, offsetBy: min(4_096, pixels.count - start))]
        }
        var output = ""
        for (index, chunk) in chunks.enumerated() {
            let more = index < chunks.count - 1 ? 1 : 0
            let control = index == 0 ? "a=T,f=32,s=\(width),v=1,r=1,q=2,m=\(more)" : "m=\(more)"
            output += "\u{1b}_G\(control);\(chunk)\u{1b}\\"
        }
        try view.session.feed(Data(output.utf8))
        #expect(view.images.count == 1)

        for _ in 0..<2 {
            let rendered = try render(view)
            let cell = cellPixels(view, row: 0, col: 1)
            let pixel = rendered.rgb(x: Int(cell.midX), y: Int(cell.midY))
            #expect(pixel.r > 200 && pixel.g < 60 && pixel.b < 60)
        }
    }

    @Test func unicodePlaceholderCellsShowTheirTileOfTheImage() throws {
        // A 2x1 image (red, then green) shown through two placeholder cells.
        let view = try makeView(feeding:
            "\u{1b}_Ga=T,f=32,s=2,v=1,i=7,U=1,c=2,r=1,q=2;/wAA/wD/AP8=\u{1b}\\"
            + "\u{1b}[38;5;7m\u{10EEEE}\u{0305}\u{0305}\u{10EEEE}\u{0305}\u{030D}\u{1b}[39m")
        #expect(view.images.count == 2)
        let pixels = try render(view)
        let left = cellPixels(view, row: 0, col: 0)
        let right = cellPixels(view, row: 0, col: 1)
        let leftPixel = pixels.rgb(x: Int(left.midX), y: Int(left.midY))
        let rightPixel = pixels.rgb(x: Int(right.midX), y: Int(right.midY))
        #expect(leftPixel.r > 200 && leftPixel.g < 60)
        #expect(rightPixel.g > 200 && rightPixel.r < 60)
        // Nothing spills into the next cell.
        let next = cellPixels(view, row: 0, col: 2)
        let nextPixel = pixels.rgb(x: Int(next.midX), y: Int(next.midY))
        #expect(nextPixel.r < 100 && nextPixel.g < 100)
    }

    @Test func kittyImagesWithoutASizeKeepTheirNaturalSize() throws {
        // A 4x4 red image with neither c nor r is 4x4 points, not stretched to its cell.
        let red = Data((0..<16).flatMap { _ in [UInt8(255), 0, 0, 255] }).base64EncodedString()
        let view = try makeView(feeding: "\u{1b}_Ga=T,f=32,s=4,v=4,q=2;\(red)\u{1b}\\")
        let pixels = try render(view)
        let cell = cellPixels(view, row: 0, col: 0)
        let inside = pixels.rgb(x: Int(cell.minX + 2 * Self.scale), y: Int(cell.minY + 2 * Self.scale))
        #expect(inside.r > 200 && inside.g < 60)
        let below = pixels.rgb(x: Int(cell.minX + 2 * Self.scale), y: Int(cell.minY + 7 * Self.scale))
        #expect(below.r < 100)
    }

    @Test func glyphAtlasGrowsBeforeStartingOver() throws {
        let device = try #require(MTLCreateSystemDefaultDevice())
        let atlas = try #require(GlyphAtlas(device: device, initialSize: 64, maxSize: 128))
        func insert(_ index: Int) -> GlyphAtlas.Entry? {
            let key = GlyphAtlas.Key.glyph(TerminalFonts.Glyph(fontIndex: 0, glyph: CGGlyph(index), isColor: false))
            return atlas.entry(for: key) {
                RasterizedGlyph(
                    width: 30, height: 30, pixels: [UInt8](repeating: UInt8(index), count: 30 * 30 * 4), offset: .zero)
            }
        }
        // 64x64 holds four 30x30 glyphs (31 with padding); the fifth doubles it.
        let first = try #require(insert(1))
        for index in 2...5 { _ = try #require(insert(index)) }
        #expect(atlas.size == 128)
        #expect(atlas.texture.width == 128)
        #expect(atlas.generation == 0)
        // Glyphs keep their texels through the copy, so cached rows stay valid.
        var texel = [UInt8](repeating: 0, count: 4)
        atlas.texture.getBytes(
            &texel, bytesPerRow: 4,
            from: MTLRegionMake2D(Int(first.rect.x), Int(first.rect.y), 1, 1), mipmapLevel: 0)
        #expect(texel == [1, 1, 1, 1])

        // Full at its largest size, it skips glyphs while resets are held off...
        atlas.allowsReset = false
        var index = 6
        while insert(index) != nil { index += 1 }
        #expect(atlas.generation == 0)
        // ...and starts over once they're allowed; the skipped glyph wasn't cached as blank.
        atlas.allowsReset = true
        #expect(insert(index) != nil)
        #expect(atlas.generation == 1)
    }

    @Test func redrawsOnlyWhatChangedButStaysCorrect() throws {
        let view = try makeView(feeding: "\u{1b}[37mtop")
        _ = try render(view)
        try view.session.feed(Data("\r\n\u{1b}[37mbottom".utf8))
        let pixels = try render(view)
        #expect(pixels.brightest(in: cellPixels(view, row: 0, col: 0, width: 3)) > 128)
        #expect(pixels.brightest(in: cellPixels(view, row: 1, col: 0, width: 6)) > 128)

        try view.session.feed(Data("\u{1b}[2J\u{1b}[H".utf8))
        let cleared = try render(view)
        // Column 0 of row 0 holds the cursor now.
        #expect(cleared.brightest(in: cellPixels(view, row: 0, col: 1, width: 5).insetBy(dx: 2, dy: 0)) < 64)
        #expect(cleared.brightest(in: cellPixels(view, row: 1, col: 0, width: 6)) < 64)
    }

    @Test func drawsAHollowCursorWhenUnfocused() throws {
        let view = try makeView()
        let pixels = try render(view)
        let cell = cellPixels(view, row: 0, col: 0)
        let edge = pixels.rgb(x: Int(cell.minX) + 1, y: Int(cell.midY))
        let center = pixels.rgb(x: Int(cell.midX), y: Int(cell.midY))
        let cursor = view.renderConfig.cursorColor
        #expect(abs(edge.r - Int(cursor.r)) <= 3 && abs(edge.g - Int(cursor.g)) <= 3)
        #expect(abs(center.r - Int(view.renderConfig.background.r)) <= 3)
    }
}
