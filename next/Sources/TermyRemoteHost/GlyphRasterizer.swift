import AppKit
import CryptoKit
import Termy
import TermyAppKit
import TermyRemoteKit

/// Font files stay on the Mac. Only the visible non-ASCII glyphs are sent as
/// transparent cell-sized PNGs, so Nerd Font symbols/emoji match the host.
///
/// Choosing the font needs the main actor; drawing and PNG encoding don't,
/// so a frame's new glyphs are drawn off it (`Job`, `render`).
enum GlyphRasterizer {
    // NSCache is thread-safe.
    private static let cache: NSCache<NSString, NSData> = {
        let cache = NSCache<NSString, NSData>()
        cache.totalCostLimit = 8 * 1024 * 1024
        return cache
    }()

    /// One glyph to draw, with everything drawing needs.
    struct Job: @unchecked Sendable {
        let key: String
        let text: String
        let cells: Int
        let color: UInt32
        let config: RemoteConfiguration
        let plan: GlyphPlanEntry?
        /// CTFont is immutable and safe to use from any thread.
        let font: CTFont?
    }

    static func cacheKey(text: String, cells: Int, flags: UInt8, color: UInt32, config: RemoteConfiguration) -> String {
        "\(text)|\(cells)|\(flags)|\(color)|\(config.fontFamily)|\(config.fontSize)|\(config.cellWidth)|\(config.cellHeight)"
    }

    /// A short name for a glyph's content, the same on every run that draws it.
    static func glyphKey(_ cacheKey: String) -> String {
        SHA256.hash(data: Data(cacheKey.utf8)).prefix(16).map { String(format: "%02x", $0) }.joined()
    }

    static func cached(_ key: String) -> Data? { cache.object(forKey: key as NSString) as Data? }

    /// What drawing the glyph takes, or nil when no font has it.
    @MainActor static func job(
        text: String, cells: Int, flags: UInt8, color: UInt32, config: RemoteConfiguration, plan: GlyphPlanEntry?
    ) -> Job? {
        var font: CTFont?
        if plan == nil {
            guard let codepoint = text.unicodeScalars.first?.value,
                let resolved = TerminalGlyphFont.resolve(
                    codepoint: codepoint, family: config.fontFamily,
                    size: Float(config.fontSize), bold: flags & CellRun.bold != 0, italic: flags & CellRun.italic != 0)
            else { return nil }
            font = resolved
        }
        return Job(
            key: cacheKey(text: text, cells: cells, flags: flags, color: color, config: config),
            text: text, cells: cells, color: color, config: config, plan: plan, font: font)
    }

    @MainActor static func png(
        text: String, cells: Int, flags: UInt8, color: UInt32,
        config: RemoteConfiguration, plan: GlyphPlanEntry?
    ) -> Data? {
        let key = cacheKey(text: text, cells: cells, flags: flags, color: color, config: config)
        if let data = cached(key) { return data }
        return job(text: text, cells: cells, flags: flags, color: color, config: config, plan: plan).flatMap(render)
    }

    /// Draws a glyph and keeps its PNG. Runs on any thread.
    static func render(_ job: Job) -> Data? {
        if let data = cached(job.key) { return data }
        let config = job.config
        let scale: CGFloat = 3
        let width = CGFloat(max(1, config.cellWidth)) * CGFloat(job.cells)
        let height = CGFloat(max(1, config.cellHeight))
        guard
            let bitmap = NSBitmapImageRep(
                bitmapDataPlanes: nil, pixelsWide: Int(ceil(width * scale)),
                pixelsHigh: Int(ceil(height * scale)), bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true,
                isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0),
            let graphics = NSGraphicsContext(bitmapImageRep: bitmap)
        else { return nil }
        bitmap.size = NSSize(width: width, height: height)
        NSGraphicsContext.saveGraphicsState()
        defer { NSGraphicsContext.restoreGraphicsState() }
        let context = graphics.cgContext
        context.translateBy(x: 0, y: CGFloat(bitmap.pixelsHigh))
        context.scaleBy(x: scale, y: -scale)
        NSGraphicsContext.current = NSGraphicsContext(cgContext: context, flipped: true)
        let rgba = job.color
        let color = NSColor(
            srgbRed: CGFloat(rgba >> 24 & 255) / 255, green: CGFloat(rgba >> 16 & 255) / 255,
            blue: CGFloat(rgba >> 8 & 255) / 255, alpha: CGFloat(rgba & 255) / 255)
        if let plan = job.plan {
            for rect in plan.rects {
                context.setFillColor(color.withAlphaComponent(CGFloat(rect.alpha)).cgColor)
                context.fill(
                    CGRect(
                        x: CGFloat(rect.left) * width, y: CGFloat(rect.top) * height,
                        width: CGFloat(rect.right - rect.left) * width, height: CGFloat(rect.bottom - rect.top) * height
                    ))
            }
            context.setStrokeColor(color.cgColor)
            for stroke in plan.strokes {
                let points = stroke.points.map { CGPoint(x: CGFloat($0.x) * width, y: CGFloat($0.y) * height) }
                guard let first = points.first else { continue }
                context.beginPath()
                context.move(to: first)
                if stroke.kind == .roundedCorner, points.count == 6 {
                    context.addLine(to: points[1])
                    context.addCurve(to: points[4], control1: points[2], control2: points[3])
                    context.addLine(to: points[5])
                } else {
                    for point in points.dropFirst() { context.addLine(to: point) }
                }
                context.setLineWidth(CGFloat(stroke.width) * width)
                context.strokePath()
            }
        } else if let font = job.font {
            let text = NSAttributedString(string: job.text, attributes: [.font: font as NSFont, .foregroundColor: color])
            let size = text.size()
            text.draw(at: CGPoint(x: max(0, (width - size.width) / 2), y: max(0, (height - size.height) / 2)))
        } else {
            return nil
        }
        guard let data = bitmap.representation(using: .png, properties: [:]) else { return nil }
        cache.setObject(data as NSData, forKey: job.key as NSString, cost: data.count)
        return data
    }
}
