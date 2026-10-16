#if os(macOS)
import CoreGraphics
import CoreText
import Metal
import Termy

/// A coverage or color bitmap, BGRA premultiplied, top row first.
struct RasterizedGlyph {
    var width: Int
    var height: Int
    var pixels: [UInt8]
    /// Added to the anchor (pen position on the baseline, or a cell's top-left
    /// corner) to get the bitmap's top-left corner, in pixels.
    var offset: SIMD2<Float>
}

/// A texture of rasterized glyphs, packed into shelves.
///
/// When it fills up it starts over on a new texture and bumps `generation`;
/// anything holding atlas coordinates must then be rebuilt.
@MainActor
final class GlyphAtlas {
    enum Key: Hashable {
        case glyph(TerminalFonts.Glyph)
        /// Stroked special-glyph geometry, rasterized for one cell size.
        case strokes([GlyphStroke])
    }

    struct Entry {
        /// Texel rect: x, y, width, height.
        var rect: SIMD4<Float>
        var offset: SIMD2<Float>
    }

    static let size = 2048
    private static let padding = 1

    let device: MTLDevice
    private(set) var texture: MTLTexture
    private(set) var generation = 0
    private var entries: [Key: Entry?] = [:]
    private var shelfX = 0
    private var shelfY = 0
    private var shelfHeight = 0

    init?(device: MTLDevice) {
        guard let texture = Self.makeTexture(device: device) else { return nil }
        self.device = device
        self.texture = texture
    }

    /// The cached entry for `key`, rasterizing it on first use. Nil for blank
    /// glyphs. Check `generation` afterwards: inserting may reset the atlas.
    func entry(for key: Key, rasterize: () -> RasterizedGlyph?) -> Entry? {
        if let cached = entries[key] { return cached }
        guard let glyph = rasterize() else {
            entries[key] = .some(nil)
            return nil
        }
        guard let origin = allocate(width: glyph.width, height: glyph.height) else {
            entries[key] = .some(nil)
            return nil
        }
        glyph.pixels.withUnsafeBytes { bytes in
            texture.replace(
                region: MTLRegionMake2D(origin.x, origin.y, glyph.width, glyph.height),
                mipmapLevel: 0, withBytes: bytes.baseAddress!, bytesPerRow: glyph.width * 4)
        }
        let entry = Entry(
            rect: SIMD4(Float(origin.x), Float(origin.y), Float(glyph.width), Float(glyph.height)),
            offset: glyph.offset)
        entries[key] = entry
        return entry
    }

    /// Drops every entry. Frames already on the GPU keep the old texture.
    func reset() {
        if let texture = Self.makeTexture(device: device) { self.texture = texture }
        entries.removeAll()
        shelfX = 0
        shelfY = 0
        shelfHeight = 0
        generation += 1
    }

    private func allocate(width: Int, height: Int) -> (x: Int, y: Int)? {
        let size = Self.size
        guard width + Self.padding <= size, height + Self.padding <= size else { return nil }
        if shelfX + width + Self.padding > size {
            shelfY += shelfHeight
            shelfX = 0
            shelfHeight = 0
        }
        if shelfY + height + Self.padding > size {
            reset()
        }
        let origin = (x: shelfX, y: shelfY)
        shelfX += width + Self.padding
        shelfHeight = max(shelfHeight, height + Self.padding)
        return origin
    }

    private static func makeTexture(device: MTLDevice) -> MTLTexture? {
        let descriptor = MTLTextureDescriptor.texture2DDescriptor(
            pixelFormat: .bgra8Unorm, width: size, height: size, mipmapped: false)
        descriptor.usage = .shaderRead
        descriptor.storageMode = device.hasUnifiedMemory ? .shared : .managed
        return device.makeTexture(descriptor: descriptor)
    }
}

// MARK: - Rasterization

enum GlyphRasterizer {
    /// Draws one glyph in white (or its own colors) at `scale` pixels per point.
    static func glyph(_ glyph: CGGlyph, font: CTFont, scale: CGFloat) -> RasterizedGlyph? {
        var glyph = glyph
        var bounds = CGRect.zero
        CTFontGetBoundingRectsForGlyphs(font, .horizontal, &glyph, &bounds, 1)
        guard bounds.width > 0, bounds.height > 0 else { return nil }
        let minX = (bounds.minX * scale).rounded(.down) - 1
        let maxX = (bounds.maxX * scale).rounded(.up) + 1
        let minY = (bounds.minY * scale).rounded(.down) - 1
        let maxY = (bounds.maxY * scale).rounded(.up) + 1
        let width = Int(maxX - minX)
        let height = Int(maxY - minY)
        guard width <= 1024, height <= 1024 else { return nil }
        let pixels = draw(width: width, height: height) { context in
            context.setShouldSmoothFonts(false)
            context.setFillColor(CGColor(gray: 1, alpha: 1))
            context.scaleBy(x: scale, y: scale)
            var position = CGPoint(x: -minX / scale, y: -minY / scale)
            CTFontDrawGlyphs(font, &glyph, &position, 1, context)
        }
        return RasterizedGlyph(width: width, height: height, pixels: pixels, offset: SIMD2(Float(minX), Float(-maxY)))
    }

    /// Draws libtermy's stroked geometry for one cell of `cellSize` pixels.
    static func strokes(_ strokes: [GlyphStroke], cellSize: CGSize) -> RasterizedGlyph? {
        guard !strokes.isEmpty, cellSize.width >= 1, cellSize.height >= 1 else { return nil }
        let width = Int(cellSize.width.rounded(.up)) + 2
        let height = Int(cellSize.height.rounded(.up)) + 2
        let pixels = draw(width: width, height: height) { context in
            // Top-down coordinates with the cell origin one pixel in.
            context.translateBy(x: 1, y: CGFloat(height) - 1)
            context.scaleBy(x: 1, y: -1)
            context.setStrokeColor(CGColor(gray: 1, alpha: 1))
            context.setLineCap(.butt)
            for stroke in strokes {
                let points = stroke.points.map {
                    CGPoint(x: CGFloat($0.x) * cellSize.width, y: CGFloat($0.y) * cellSize.height)
                }
                context.setLineWidth(max(1, CGFloat(stroke.width) * cellSize.width))
                context.beginPath()
                switch stroke.kind {
                case .line where points.count >= 2:
                    context.move(to: points[0])
                    context.addLine(to: points[1])
                case .roundedCorner where points.count >= 6:
                    context.move(to: points[0])
                    context.addLine(to: points[1])
                    context.addCurve(to: points[4], control1: points[2], control2: points[3])
                    context.addLine(to: points[5])
                default:
                    continue
                }
                context.strokePath()
            }
        }
        return RasterizedGlyph(width: width, height: height, pixels: pixels, offset: SIMD2(-1, -1))
    }

    /// Draws text in white on one line, `height` pixels tall, baseline at `baseline`.
    static func line(_ line: CTLine, width: Int, height: Int, baseline: CGFloat, scale: CGFloat) -> RasterizedGlyph? {
        guard width > 0, height > 0 else { return nil }
        let pixels = draw(width: width, height: height) { context in
            context.setShouldSmoothFonts(false)
            context.scaleBy(x: scale, y: scale)
            context.textPosition = CGPoint(x: 0, y: CGFloat(height) / scale - baseline)
            CTLineDraw(line, context)
        }
        return RasterizedGlyph(width: width, height: height, pixels: pixels, offset: .zero)
    }

    /// Draws a CGImage at its natural size, downscaled to fit `maxDimension`
    /// on both sides (a GPU texture limit) with its aspect ratio kept.
    static func image(_ image: CGImage, maxDimension: Int = .max) -> RasterizedGlyph? {
        guard image.width > 0, image.height > 0, maxDimension > 0 else { return nil }
        let fit = min(1, Double(maxDimension) / Double(image.width), Double(maxDimension) / Double(image.height))
        let width = min(maxDimension, max(1, Int((Double(image.width) * fit).rounded())))
        let height = min(maxDimension, max(1, Int((Double(image.height) * fit).rounded())))
        let pixels = draw(width: width, height: height) { context in
            context.interpolationQuality = .high
            context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
        }
        return RasterizedGlyph(width: width, height: height, pixels: pixels, offset: .zero)
    }

    private static func draw(width: Int, height: Int, _ body: (CGContext) -> Void) -> [UInt8] {
        var pixels = [UInt8](repeating: 0, count: width * height * 4)
        pixels.withUnsafeMutableBytes { bytes in
            guard let context = CGContext(
                data: bytes.baseAddress, width: width, height: height, bitsPerComponent: 8,
                bytesPerRow: width * 4, space: CGColorSpace(name: CGColorSpace.sRGB)!,
                bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue
            ) else { return }
            body(context)
        }
        return pixels
    }
}
#endif
