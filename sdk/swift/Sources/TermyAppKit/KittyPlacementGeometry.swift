#if os(macOS)
import CoreGraphics
import Termy

/// Where a Kitty placement draws, in points from the grid's top-left corner
/// (before padding). Mirrors libtermy's `graphics_display_layout`, which the
/// desktop app uses too. `TerminalView` draws with it; use it to draw
/// placements anywhere else.
public struct KittyPlacementGeometry: Equatable, Sendable {
    /// Where the whole source crop is drawn. A direct placement fills its
    /// box, stretching when both `c` and `r` are given, as in kitty. A Unicode
    /// placeholder's image keeps its aspect ratio, centered in its cell box.
    public var image: CGRect
    /// The part of the grid the placement may paint: its box after margin
    /// clipping, or the single cell of a Unicode placeholder.
    public var clip: CGRect

    /// The visible part of `image`.
    public var visible: CGRect { image.intersection(clip) }

    /// `pixelsPerPoint` is the cell size the terminal was given, over the
    /// cell size drawn: image pixels and pixel offsets are in the terminal's
    /// units, which are device pixels for a view on a Retina display.
    public init(
        _ placement: KittyGraphicsPlacement, cellWidth: CGFloat, cellHeight: CGFloat,
        pixelsPerPoint: CGSize = CGSize(width: 1, height: 1)
    ) {
        let scaleX = pixelsPerPoint.width > 0 ? pixelsPerPoint.width : 1
        let scaleY = pixelsPerPoint.height > 0 ? pixelsPerPoint.height : 1
        let crop = placement.sourceCrop
        let natural = CGSize(width: crop.width / scaleX, height: crop.height / scaleY)
        let aspectWidth = max(natural.width, .leastNonzeroMagnitude)
        let aspectHeight = max(natural.height, .leastNonzeroMagnitude)
        let isVirtual = placement.virtualCell != nil
        // Virtual placements ignore pixel offsets, as the protocol requires.
        let offsetX = isVirtual ? 0 : CGFloat(placement.xOffset) / scaleX
        let offsetY = isVirtual ? 0 : CGFloat(placement.yOffset) / scaleY

        var box: CGSize
        switch (placement.displayCols, placement.displayRows) {
        case let (cols?, rows?):
            box = CGSize(
                width: max(0, CGFloat(cols) * cellWidth - offsetX),
                height: max(0, CGFloat(rows) * cellHeight - offsetY))
        case let (cols?, nil):
            let width = max(0, CGFloat(cols) * cellWidth - offsetX)
            box = CGSize(width: width, height: width * natural.height / aspectWidth)
        case let (nil, rows?):
            let height = max(0, CGFloat(rows) * cellHeight - offsetY)
            box = CGSize(width: height * natural.width / aspectHeight, height: height)
        case (nil, nil):
            box = natural
        }
        var fitted = box
        if isVirtual {
            // A placeholder's box is whole cells: the count given, or enough
            // for the image. The image is fitted inside it.
            func cells(_ count: Int?, _ extent: CGFloat, _ cell: CGFloat) -> CGFloat {
                (count.map { CGFloat($0) } ?? max(1, (extent / max(cell, 1) - 1e-3).rounded(.up))) * cell
            }
            box = CGSize(
                width: cells(placement.displayCols, box.width, cellWidth),
                height: cells(placement.displayRows, box.height, cellHeight))
            let scale = min(box.width / aspectWidth, box.height / aspectHeight)
            fitted = CGSize(width: natural.width * scale, height: natural.height * scale)
        }

        let left = CGFloat(placement.col + placement.colOffset) * cellWidth + offsetX
        let top = CGFloat(placement.viewportRow) * cellHeight + offsetY
        // A placeholder cell shows its tile of the image placed at its origin.
        let tile = placement.virtualCell ?? GridPosition(row: 0, col: 0)
        image = CGRect(
            x: left + (box.width - fitted.width) / 2 - CGFloat(tile.col) * cellWidth,
            y: top + (box.height - fitted.height) / 2 - CGFloat(tile.row) * cellHeight,
            width: fitted.width, height: fitted.height)

        let size = isVirtual ? CGSize(width: cellWidth, height: cellHeight) : box
        let clipped = CGFloat(placement.clipTopRows + placement.clipBottomRows) * cellHeight
        clip = CGRect(
            x: left, y: top + CGFloat(placement.clipTopRows) * cellHeight,
            width: size.width, height: max(0, size.height - clipped))
    }

    /// The source rectangle, in image pixels, that lands in `rect` (a part of
    /// `image`).
    public func source(for rect: CGRect, crop: CGRect) -> CGRect {
        guard image.width > 0, image.height > 0 else { return .zero }
        let scaleX = crop.width / image.width
        let scaleY = crop.height / image.height
        return CGRect(
            x: crop.minX + (rect.minX - image.minX) * scaleX,
            y: crop.minY + (rect.minY - image.minY) * scaleY,
            width: rect.width * scaleX, height: rect.height * scaleY)
    }
}

extension KittyGraphicsPlacement {
    /// The source crop in image pixels; zero sizes mean the whole image.
    public var sourceCrop: CGRect {
        CGRect(
            x: sourceX, y: sourceY,
            width: sourceWidth > 0 ? sourceWidth : imageWidth,
            height: sourceHeight > 0 ? sourceHeight : imageHeight)
    }

    /// Where the image draws among text and cell backgrounds. As in kitty, a
    /// negative z-index goes under text, and one below `Int32.min / 2` under
    /// cells with a background color of their own too.
    var layer: KittyImageLayer {
        if zIndex < Int32.min / 2 { return .belowBackgrounds }
        return zIndex < 0 ? .belowText : .aboveText
    }
}

enum KittyImageLayer {
    case belowBackgrounds, belowText, aboveText
}
#endif
