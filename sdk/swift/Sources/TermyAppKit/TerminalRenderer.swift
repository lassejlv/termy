#if os(macOS)
import AppKit
import CoreText
import Termy

/// Everything the renderer reads for one draw pass.
struct RenderState {
    var frame: FrameStore
    var config: RenderConfig
    var glyphPlan: [Int: GlyphPlanEntry]
    var selection: TerminalSelection?
    var images: [KittyImage]
    var isFocused: Bool
    var cursorBlinkVisible: Bool
    var markedText: String?
}

/// A decoded Kitty graphics placement ready to draw.
struct KittyImage {
    var placement: KittyGraphicsPlacement
    var image: CGImage
}

/// Draws a retained terminal frame with CoreGraphics and CoreText.
///
/// The fallback when Metal is unavailable, and the renderer behind
/// `cacheDisplay(in:to:)`. Ordinary text is drawn glyph-by-glyph on the cell
/// grid so columns never drift.
@MainActor
final class TerminalRenderer {
    private var config: RenderConfig?
    private(set) var fonts = TerminalFonts(fontFamily: "", fontSize: 13)
    private var colors: [UInt32: CGColor] = [:]
    private var ascent: CGFloat = 0
    private var descent: CGFloat = 0
    private var underlineOffset: CGFloat = 0

    var cellWidth: CGFloat { CGFloat(config?.cellWidth ?? 8) }
    var cellHeight: CGFloat { CGFloat(config?.cellHeight ?? 16) }
    var paddingX: CGFloat { CGFloat(config?.paddingX ?? 0) }
    var paddingY: CGFloat { CGFloat(config?.paddingY ?? 0) }

    func configure(_ config: RenderConfig) {
        guard config != self.config else { return }
        self.config = config
        fonts = TerminalFonts(fontFamily: config.fontFamily, fontSize: config.fontSize)
        ascent = fonts.ascent
        descent = fonts.descent
        underlineOffset = fonts.underlinePosition
    }

    // MARK: - Geometry

    func cellRect(row: Int, col: Int, width: Int = 1) -> CGRect {
        CGRect(
            x: paddingX + CGFloat(col) * cellWidth,
            y: paddingY + CGFloat(row) * cellHeight,
            width: CGFloat(width) * cellWidth,
            height: cellHeight
        )
    }

    func rowRect(_ row: Int, cols: Int) -> CGRect {
        cellRect(row: row, col: 0, width: cols)
    }

    /// The cell under a point, clamped to the grid.
    func position(at point: CGPoint, cols: Int, rows: Int) -> GridPosition {
        let col = Int(((point.x - paddingX) / cellWidth).rounded(.down))
        let row = Int(((point.y - paddingY) / cellHeight).rounded(.down))
        return GridPosition(row: min(max(row, 0), max(rows - 1, 0)), col: min(max(col, 0), max(cols - 1, 0)))
    }

    private var baselineOffset: CGFloat {
        fonts.baselineOffset(cellHeight: cellHeight)
    }

    // MARK: - Drawing

    func draw(_ state: RenderState, in context: CGContext, dirtyRect: CGRect, bounds: CGRect) {
        // Layer rendering can pass an unbounded dirty rect.
        let dirtyRect = dirtyRect.intersection(bounds)
        guard !dirtyRect.isNull, !dirtyRect.isEmpty else { return }
        configure(state.config)
        let config = state.config
        let frame = state.frame

        context.setFillColor(color(config.background, alpha: CGFloat(config.backgroundOpacity)))
        context.fill(dirtyRect)
        guard !frame.isEmpty else { return }

        let firstRow = max(0, Int(((dirtyRect.minY - paddingY) / cellHeight).rounded(.down)))
        let lastRow = min(frame.rows - 1, Int(((dirtyRect.maxY - paddingY) / cellHeight).rounded(.up)))
        guard firstRow <= lastRow else { return }
        let rows = firstRow...lastRow

        drawBackgrounds(state, rows: rows, in: context)
        drawSelection(state, rows: rows, in: context)
        drawImages(state.images.filter { $0.placement.zIndex < 0 }, in: context)
        let cursor = cursorDrawing(state)
        if let cursor, cursor.style == .block, cursor.filled {
            context.setFillColor(color(config.cursorColor))
            context.fill(cursor.rect)
        }
        drawText(state, rows: rows, invertedCursorCell: cursor?.filled == true ? cursor?.cell : nil, in: context)
        drawGlyphGeometry(state, rows: rows, in: context)
        drawImages(state.images.filter { $0.placement.zIndex >= 0 }, in: context)
        if let cursor { drawCursorOutline(cursor, config: config, in: context) }
        if let marked = state.markedText, !marked.isEmpty { drawMarkedText(marked, state: state, in: context) }
    }

    private func drawBackgrounds(_ state: RenderState, rows: ClosedRange<Int>, in context: CGContext) {
        let frame = state.frame
        let cellAlpha = state.config.backgroundOpacityAffectsCells ? CGFloat(state.config.backgroundOpacity) : 1
        for row in rows {
            var col = 0
            while col < frame.cols {
                let cell = frame[row, col]
                guard !cell.usesDefaultBackground else { col += 1; continue }
                var end = col + 1
                while end < frame.cols {
                    let next = frame[row, end]
                    guard !next.usesDefaultBackground, next.background == cell.background else { break }
                    end += 1
                }
                context.setFillColor(color(cell.background, alpha: cellAlpha))
                context.fill(cellRect(row: row, col: col, width: end - col))
                col = end
            }
        }
    }

    private func drawSelection(_ state: RenderState, rows: ClosedRange<Int>, in context: CGContext) {
        guard let selection = state.selection?.expanded(in: state.frame) else { return }
        let frame = state.frame
        context.setFillColor(NSColor.selectedTextBackgroundColor.withAlphaComponent(0.55).cgColor)
        for row in rows where selection.rows.contains(row) {
            let from = row == selection.start.row ? selection.start.col : 0
            let to = row == selection.end.row ? selection.end.col : frame.cols - 1
            guard from <= to else { continue }
            context.fill(cellRect(row: row, col: from, width: to - from + 1))
        }
    }

    private func drawText(
        _ state: RenderState, rows: ClosedRange<Int>, invertedCursorCell: GridPosition?, in context: CGContext
    ) {
        let frame = state.frame
        context.saveGState()
        defer { context.restoreGState() }
        // The view is flipped; flip glyphs back upright.
        context.textMatrix = CGAffineTransform(scaleX: 1, y: -1)

        struct Run {
            var glyphs: [CGGlyph] = []
            var positions: [CGPoint] = []
        }
        for row in rows {
            var runs: [Int: [UInt32: (TermyColor, Run)]] = [:]
            let baseline = paddingY + CGFloat(row) * cellHeight + baselineOffset
            for col in 0..<frame.cols {
                let index = row * frame.cols + col
                guard state.glyphPlan[index] == nil else { continue }
                let cell = frame[row, col]
                guard cell.text != nil,
                      let glyph = fonts.glyph(for: cell.codepoint, style: .init(bold: cell.isBold, italic: cell.isItalic))
                else { continue }
                let inverted = invertedCursorCell == GridPosition(row: row, col: col)
                let foreground = inverted ? cursorTextColor(cell, config: state.config) : cell.foreground
                let point = CGPoint(x: paddingX + CGFloat(col) * cellWidth, y: baseline)

                var entry = runs[glyph.fontIndex, default: [:]][foreground.rgba] ?? (foreground, Run())
                entry.1.glyphs.append(glyph.glyph)
                // Glyph positions are in text space, which the flipped text
                // matrix mirrors back into the view.
                entry.1.positions.append(CGPoint(x: point.x, y: -point.y))
                runs[glyph.fontIndex, default: [:]][foreground.rgba] = entry
            }
            context.textPosition = .zero
            for (fontIndex, byColor) in runs {
                let font = fonts.font(at: fontIndex)
                for (foreground, run) in byColor.values {
                    context.setFillColor(color(foreground))
                    CTFontDrawGlyphs(font, run.glyphs, run.positions, run.glyphs.count, context)
                }
            }
            drawDecorations(state, row: row, baseline: baseline, in: context)
        }
    }

    private func drawDecorations(_ state: RenderState, row: Int, baseline: CGFloat, in context: CGContext) {
        let frame = state.frame
        for col in 0..<frame.cols {
            let cell = frame[row, col]
            guard cell.isUnderlined || cell.isStrikethrough, !cell.isWideCharacterSpacer else { continue }
            let rect = cellRect(row: row, col: col)
            context.setFillColor(color(cell.foreground))
            if cell.isUnderlined {
                context.fill(CGRect(x: rect.minX, y: (baseline + max(underlineOffset, 1)).rounded(), width: rect.width, height: 1))
            }
            if cell.isStrikethrough {
                context.fill(CGRect(x: rect.minX, y: (baseline - ascent * 0.35).rounded(), width: rect.width, height: 1))
            }
        }
    }

    /// Box drawing, blocks, Braille and friends, from libtermy's geometry plans.
    private func drawGlyphGeometry(_ state: RenderState, rows: ClosedRange<Int>, in context: CGContext) {
        guard !state.glyphPlan.isEmpty else { return }
        let frame = state.frame
        let scale = context.userSpaceToDeviceSpaceTransform.a
        for (index, entry) in state.glyphPlan {
            let row = index / frame.cols
            guard rows.contains(row) else { continue }
            let col = index % frame.cols
            let cell = frame[row, col]
            let origin = cellRect(row: row, col: col)
            for rect in entry.rects {
                context.setFillColor(color(cell.foreground, alpha: CGFloat(rect.alpha)))
                context.fill(snap(rect, in: origin, scale: scale))
            }
            guard !entry.strokes.isEmpty else { continue }
            context.setStrokeColor(color(cell.foreground))
            context.setLineCap(.butt)
            for stroke in entry.strokes {
                let points = stroke.points.map {
                    CGPoint(x: origin.minX + CGFloat($0.x) * cellWidth, y: origin.minY + CGFloat($0.y) * cellHeight)
                }
                context.setLineWidth(max(1 / scale, CGFloat(stroke.width) * cellWidth))
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
    }

    private func snap(_ rect: GlyphRect, in cell: CGRect, scale: CGFloat) -> CGRect {
        func round(_ value: CGFloat, outward: Bool, up: Bool) -> CGFloat {
            let device = value * scale
            let rounded = outward ? (up ? device.rounded(.up) : device.rounded(.down)) : device.rounded()
            return rounded / scale
        }
        let outward = rect.snap == .outward
        let left = round(cell.minX + CGFloat(rect.left) * cellWidth, outward: outward, up: false)
        let top = round(cell.minY + CGFloat(rect.top) * cellHeight, outward: outward, up: false)
        let right = round(cell.minX + CGFloat(rect.right) * cellWidth, outward: outward, up: true)
        let bottom = round(cell.minY + CGFloat(rect.bottom) * cellHeight, outward: outward, up: true)
        return CGRect(x: left, y: top, width: max(right - left, 1 / scale), height: max(bottom - top, 1 / scale))
    }

    private func drawImages(_ images: [KittyImage], in context: CGContext) {
        let origin = cellRect(row: 0, col: 0).origin
        for image in images.sorted(by: { $0.placement.zIndex < $1.placement.zIndex }) {
            let placement = image.placement
            let geometry = KittyPlacementGeometry(placement, cellWidth: cellWidth, cellHeight: cellHeight)
            let clip = geometry.clip.offsetBy(dx: origin.x, dy: origin.y)
            let target = geometry.image.offsetBy(dx: origin.x, dy: origin.y)
            guard !geometry.visible.isNull, let cropped = image.image.cropping(to: placement.sourceCrop) else { continue }

            context.saveGState()
            context.clip(to: clip)
            // CGImage draws bottom-up; flip into the view's top-down space.
            context.translateBy(x: 0, y: target.maxY + target.minY)
            context.scaleBy(x: 1, y: -1)
            context.draw(cropped, in: target)
            context.restoreGState()
        }
    }

    // MARK: - Cursor and IME

    private struct CursorDrawing {
        var cell: GridPosition
        var rect: CGRect
        var style: CursorStyle
        /// A focused block cursor fills its cell; an unfocused one is hollow.
        var filled: Bool
    }

    private func cursorDrawing(_ state: RenderState) -> CursorDrawing? {
        let frame = state.frame
        let cursor = frame.cursor
        guard cursor.isVisible, state.cursorBlinkVisible || !state.isFocused,
              cursor.row < frame.rows, cursor.col < frame.cols else { return nil }
        let wide = cursor.col + 1 < frame.cols && frame[cursor.row, cursor.col + 1].isWideCharacterSpacer
        let rect = cellRect(row: cursor.row, col: cursor.col, width: wide ? 2 : 1)
        let style: CursorStyle = cursor.style == .bar ? .bar : .block
        return CursorDrawing(
            cell: GridPosition(row: cursor.row, col: cursor.col), rect: rect, style: style,
            filled: style == .block && state.isFocused)
    }

    private func drawCursorOutline(_ cursor: CursorDrawing, config: RenderConfig, in context: CGContext) {
        context.setFillColor(color(config.cursorColor))
        context.setStrokeColor(color(config.cursorColor))
        switch cursor.style {
        case .bar:
            context.fill(CGRect(x: cursor.rect.minX, y: cursor.rect.minY, width: 2, height: cursor.rect.height))
        default:
            guard !cursor.filled else { return }
            context.setLineWidth(1)
            context.stroke(cursor.rect.insetBy(dx: 0.5, dy: 0.5))
        }
    }

    private func cursorTextColor(_ cell: Cell, config: RenderConfig) -> TermyColor {
        cell.usesDefaultBackground ? config.background : cell.background
    }

    private func drawMarkedText(_ text: String, state: RenderState, in context: CGContext) {
        let cursor = state.frame.cursor
        let origin = cellRect(row: cursor.row, col: cursor.col)
        let font = fonts.font(.regular)
        let attributes: [NSAttributedString.Key: Any] = [
            .init(kCTFontAttributeName as String): font,
            .init(kCTForegroundColorAttributeName as String): color(state.config.foreground),
        ]
        let line = CTLineCreateWithAttributedString(NSAttributedString(string: text, attributes: attributes))
        let width = CGFloat(CTLineGetTypographicBounds(line, nil, nil, nil))
        context.setFillColor(color(state.config.background))
        context.fill(CGRect(x: origin.minX, y: origin.minY, width: width, height: cellHeight))
        context.saveGState()
        context.textMatrix = CGAffineTransform(scaleX: 1, y: -1)
        context.textPosition = CGPoint(x: origin.minX, y: origin.minY + baselineOffset)
        CTLineDraw(line, context)
        context.restoreGState()
        context.setFillColor(color(state.config.foreground))
        context.fill(CGRect(x: origin.minX, y: origin.maxY - 2, width: width, height: 1))
    }

    // MARK: - Caches

    func color(_ color: TermyColor, alpha: CGFloat = 1) -> CGColor {
        let effectiveAlpha = CGFloat(color.a) / 255 * alpha
        let key = TermyColor(r: color.r, g: color.g, b: color.b, a: UInt8((effectiveAlpha * 255).rounded())).rgba
        if let cached = colors[key] { return cached }
        let created = CGColor(
            srgbRed: CGFloat(color.r) / 255, green: CGFloat(color.g) / 255, blue: CGFloat(color.b) / 255,
            alpha: effectiveAlpha)
        colors[key] = created
        return created
    }
}
#endif
