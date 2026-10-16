#if os(macOS)
import CoreGraphics
import Termy

/// Where a Kitty placement draws, in points from the grid's top-left corner
/// (before padding). Mirrors libtermy's `graphics_display_layout`, which the
/// desktop app uses too.
struct KittyPlacementGeometry: Equatable {
    /// Where the whole source crop is drawn. With both `c` and `r` the image
    /// is fitted inside that box with its aspect ratio kept, and centered.
    var image: CGRect
    /// The part of the grid the placement may paint: its box after margin
    /// clipping, or the single cell of a Unicode placeholder.
    var clip: CGRect

    /// The visible part of `image`.
    var visible: CGRect { image.intersection(clip) }

    init(_ placement: KittyGraphicsPlacement, cellWidth: CGFloat, cellHeight: CGFloat) {
        let sourceWidth = CGFloat(placement.sourceWidth > 0 ? placement.sourceWidth : placement.imageWidth)
        let sourceHeight = CGFloat(placement.sourceHeight > 0 ? placement.sourceHeight : placement.imageHeight)
        let isVirtual = placement.virtualCell != nil
        // Virtual placements ignore pixel offsets, as the protocol requires.
        let offsetX = isVirtual ? 0 : CGFloat(placement.xOffset)
        let offsetY = isVirtual ? 0 : CGFloat(placement.yOffset)

        let box: CGSize
        switch (placement.displayCols, placement.displayRows) {
        case let (cols?, rows?):
            box = CGSize(
                width: max(0, CGFloat(cols) * cellWidth - offsetX),
                height: max(0, CGFloat(rows) * cellHeight - offsetY))
        case let (cols?, nil):
            let width = max(0, CGFloat(cols) * cellWidth - offsetX)
            box = CGSize(width: width, height: width * sourceHeight / max(sourceWidth, 1))
        case let (nil, rows?):
            let height = max(0, CGFloat(rows) * cellHeight - offsetY)
            box = CGSize(width: height * sourceWidth / max(sourceHeight, 1), height: height)
        case (nil, nil):
            box = CGSize(width: sourceWidth, height: sourceHeight)
        }
        var fitted = box
        if placement.displayCols != nil, placement.displayRows != nil {
            let scale = min(box.width / max(sourceWidth, 1), box.height / max(sourceHeight, 1))
            fitted = CGSize(width: sourceWidth * scale, height: sourceHeight * scale)
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
    func source(for rect: CGRect, crop: CGRect) -> CGRect {
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
    var sourceCrop: CGRect {
        CGRect(
            x: sourceX, y: sourceY,
            width: sourceWidth > 0 ? sourceWidth : imageWidth,
            height: sourceHeight > 0 ? sourceHeight : imageHeight)
    }
}
#endif
