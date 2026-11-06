import AppKit
import Termy
import TermyAppKit
import TermyRemoteKit

/// Font files stay on the Mac. Only the visible non-ASCII glyphs are sent as
/// transparent cell-sized PNGs, so Nerd Font symbols/emoji match the host.
@MainActor enum GlyphRasterizer {
    private static let cache = NSCache<NSString, NSData>()
    static func png(
        text: String, cells: Int, flags: UInt8, color: UInt32,
        config: RemoteConfiguration, plan: GlyphPlanEntry?
    ) -> Data? {
        let key =
            "\(text)|\(cells)|\(flags)|\(color)|\(config.fontFamily)|\(config.fontSize)|\(config.cellWidth)|\(config.cellHeight)"
            as NSString
        if let data = cache.object(forKey: key) { return data as Data }
        let scale: CGFloat = 3
        let width = CGFloat(max(1, config.cellWidth)) * CGFloat(cells)
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
        let color = NSColor(
            srgbRed: CGFloat(color >> 24 & 255) / 255, green: CGFloat(color >> 16 & 255) / 255,
            blue: CGFloat(color >> 8 & 255) / 255, alpha: CGFloat(color & 255) / 255)
        if let plan {
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
        } else {
            guard let codepoint = text.unicodeScalars.first?.value,
                let resolved = TerminalGlyphFont.resolve(
                    codepoint: codepoint, family: config.fontFamily,
                    size: Float(config.fontSize), bold: flags & CellRun.bold != 0, italic: flags & CellRun.italic != 0)
            else { return nil }
            let font = resolved as NSFont
            let text = NSAttributedString(string: text, attributes: [.font: font, .foregroundColor: color])
            let size = text.size()
            text.draw(at: CGPoint(x: max(0, (width - size.width) / 2), y: max(0, (height - size.height) / 2)))
        }
        guard let data = bitmap.representation(using: .png, properties: [:]) else { return nil }
        cache.totalCostLimit = 8 * 1024 * 1024
        cache.setObject(data as NSData, forKey: key, cost: data.count)
        return data
    }
}
