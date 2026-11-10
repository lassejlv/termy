import Foundation
internal import CTermy

/// An image's pixels: they never change for one id and generation.
public struct KittyImageKey: Hashable, Sendable {
    public var imageID: UInt32
    public var generation: UInt64

    public init(imageID: UInt32, generation: UInt64) {
        self.imageID = imageID
        self.generation = generation
    }
}

/// One visible Kitty graphics placement.
///
/// Positions are viewport cells and may be negative when an image is partly
/// scrolled out. Keep them signed when clipping.
public struct KittyGraphicsPlacement: Hashable, Sendable {
    public var placementSerial: UInt64
    public var imageID: UInt32
    public var placementID: UInt32
    /// The full decoded image, PNG-encoded. Empty from
    /// `Terminal.kittyGraphics(holding:)` unless the image has no `rgba`.
    public var png: Data
    /// The full image as `imageWidth * imageHeight` RGBA8 pixels with straight
    /// alpha. Only from `Terminal.kittyGraphics(holding:)`, and nil for an
    /// image the caller holds already.
    public var rgba: Data?
    public var imageWidth: Int
    public var imageHeight: Int
    /// Changes when the image's pixels change; use with `imageID` as a cache key.
    public var imageGeneration: UInt64
    public var viewportRow: Int
    public var col: Int
    public var colOffset: Int
    /// The source crop, in image pixels.
    public var sourceX: Int
    public var sourceY: Int
    public var sourceWidth: Int
    public var sourceHeight: Int
    /// The requested display size in cells, when given.
    public var displayCols: Int?
    public var displayRows: Int?
    /// The cells the placement occupies.
    public var occupiedCols: Int
    public var occupiedRows: Int
    /// Pixel offsets within the first cell.
    public var xOffset: Int
    public var yOffset: Int
    /// Negative values draw below text.
    public var zIndex: Int32
    /// Unicode placeholder placements report their virtual cell.
    public var virtualCell: GridPosition?
    /// Rows clipped from the top and bottom by margins.
    public var clipTopRows: Int
    public var clipBottomRows: Int
    /// Milliseconds until the next animation frame; nil for static images.
    public var nextFrameDelay: UInt64?

    /// The cache key for the image's pixels.
    public var imageKey: KittyImageKey {
        KittyImageKey(imageID: imageID, generation: imageGeneration)
    }

    init(_ placement: TermyFfiKittyGraphicsPlacement, png: Data, rgba: Data? = nil) {
        placementSerial = placement.placement_serial
        imageID = placement.image_id
        placementID = placement.placement_id
        self.png = png
        self.rgba = rgba
        imageWidth = Int(placement.image_width)
        imageHeight = Int(placement.image_height)
        imageGeneration = placement.image_generation
        viewportRow = Int(placement.viewport_row)
        col = placement.col
        colOffset = Int(placement.col_offset)
        sourceX = Int(placement.source_x)
        sourceY = Int(placement.source_y)
        sourceWidth = Int(placement.source_width)
        sourceHeight = Int(placement.source_height)
        displayCols = placement.has_display_cols ? Int(placement.display_cols) : nil
        displayRows = placement.has_display_rows ? Int(placement.display_rows) : nil
        occupiedCols = Int(placement.occupied_cols)
        occupiedRows = Int(placement.occupied_rows)
        xOffset = Int(placement.x_offset)
        yOffset = Int(placement.y_offset)
        zIndex = placement.z_index
        virtualCell = placement.is_virtual_cell
            ? GridPosition(row: Int(placement.virtual_cell_row), col: Int(placement.virtual_cell_col))
            : nil
        clipTopRows = Int(placement.clip_top_rows)
        clipBottomRows = Int(placement.clip_bottom_rows)
        nextFrameDelay = placement.next_frame_delay_ms == 0 ? nil : placement.next_frame_delay_ms
    }
}

public struct KittyGraphicsSnapshot: Hashable, Sendable {
    public var revision: UInt64
    public var placements: [KittyGraphicsPlacement]
    /// The terminal's cell size, in the pixels that image sizes and pixel
    /// offsets are measured in: the size it was last resized with, by this
    /// session or another view of the same pane.
    public var cellWidth: Float = 0
    public var cellHeight: Float = 0

    /// The soonest animation deadline, if any placement is animated.
    public var nextFrameDelay: UInt64? {
        placements.compactMap(\.nextFrameDelay).min()
    }
}
