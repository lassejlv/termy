internal import CTermy

/// A complete viewport, cells in row-major order.
public struct Frame: Hashable, Sendable {
    public var cols: Int
    public var rows: Int
    public var cells: [Cell]
    public var cursor: TerminalCursor
    /// Rows above the live bottom; zero means the viewport shows live output.
    public var displayOffset: Int
    /// Rows of retained scrollback history.
    public var historySize: Int

    public subscript(row: Int, col: Int) -> Cell {
        cells[row * cols + col]
    }

    /// The visible text of one row, without trailing blanks.
    public func text(row: Int) -> String {
        rowText(cells[row * cols ..< (row + 1) * cols])
    }

    public init(
        cols: Int, rows: Int, cells: [Cell], cursor: TerminalCursor, displayOffset: Int, historySize: Int
    ) {
        self.cols = cols
        self.rows = rows
        self.cells = cells
        self.cursor = cursor
        self.displayOffset = displayOffset
        self.historySize = historySize
    }

    init(_ frame: TermyFfiFrame) {
        self.init(
            cols: Int(frame.cols),
            rows: Int(frame.rows),
            cells: UnsafeBufferPointer(start: frame.cells_ptr, count: frame.cells_len).map(Cell.init),
            cursor: TerminalCursor(frame.cursor),
            displayOffset: frame.display_offset,
            historySize: frame.history_size
        )
    }
}

/// Which cells a frame update carries.
public enum FrameDamage: Hashable, Sendable {
    /// No cells changed; metadata such as the cursor may still have.
    case none
    /// Every cell, row-major.
    case full
    /// Cells for each span, concatenated in span order.
    case partial([DirtySpan])
}

/// Changed cells plus viewport metadata from `Terminal.takeFrameUpdate`.
public struct FrameUpdate: Hashable, Sendable {
    public var cols: Int
    public var rows: Int
    /// Full damage: `cols * rows` cells. Partial: cells for each span in order.
    public var cells: [Cell]
    public var damage: FrameDamage
    public var cursor: TerminalCursor
    public var displayOffset: Int
    public var historySize: Int

    public init(
        cols: Int, rows: Int, cells: [Cell], damage: FrameDamage, cursor: TerminalCursor,
        displayOffset: Int, historySize: Int
    ) {
        self.cols = cols
        self.rows = rows
        self.cells = cells
        self.damage = damage
        self.cursor = cursor
        self.displayOffset = displayOffset
        self.historySize = historySize
    }

    init(_ update: TermyFfiFrameUpdate) {
        self.init(
            cols: Int(update.cols),
            rows: Int(update.rows),
            cells: UnsafeBufferPointer(start: update.cells_ptr, count: update.cells_len).map(Cell.init),
            damage: FrameDamage(update),
            cursor: TerminalCursor(update.cursor),
            displayOffset: update.display_offset,
            historySize: update.history_size
        )
    }
}

extension FrameDamage {
    init(_ update: TermyFfiFrameUpdate) {
        switch update.damage_kind {
        case 1:
            self = .full
        case 2:
            let spans = UnsafeBufferPointer(start: update.spans_ptr, count: update.spans_len)
            self = .partial(spans.map(DirtySpan.init))
        default:
            self = .none
        }
    }
}

/// Output scrolled viewport rows `top...bottom` by `count` rows. A partial
/// update that carries scrolls is applied by replaying them on the frame
/// shown, in order, and then patching its spans.
public struct ViewportScroll: Hashable, Sendable {
    public enum Direction: Hashable, Sendable {
        /// Content moved toward row 0, as when output scrolls into history.
        case up
        case down
    }

    public var top: Int
    /// Inclusive.
    public var bottom: Int
    public var count: Int
    public var direction: Direction

    public init(top: Int, bottom: Int, count: Int, direction: Direction) {
        self.top = top
        self.bottom = bottom
        self.count = count
        self.direction = direction
    }

    /// Where the content of viewport row `row` is after the scroll, or nil
    /// when it scrolled out of the region.
    public func destination(ofRow row: Int) -> Int? {
        guard (top...bottom).contains(row) else { return row }
        let moved = direction == .up ? row - count : row + count
        return (top...bottom).contains(moved) ? moved : nil
    }

    /// Whether it fits a viewport of `rows` rows.
    func fits(rows: Int) -> Bool {
        top >= 0 && top <= bottom && bottom < rows && count > 0 && count <= bottom - top + 1
    }

    init?(_ scroll: TermyFfiViewportScroll) {
        switch scroll.direction {
        case 1: direction = .up
        case 2: direction = .down
        default: return nil
        }
        top = scroll.top
        bottom = scroll.bottom
        count = scroll.count
    }
}

extension Array {
    /// Replays `scroll` on row-major rows of `width` elements, moving each
    /// row's elements where its content went; rows the scroll exposes keep
    /// what scrolled out, for the caller to replace.
    mutating func replay(_ scroll: ViewportScroll, width: Int) {
        let start = scroll.top * width
        let end = (scroll.bottom + 1) * width
        let shift = scroll.count * width
        let region = self[start..<end]
        let rotated = scroll.direction == .up
            ? region[(start + shift)...] + region[..<(start + shift)]
            : region[(end - shift)...] + region[..<(end - shift)]
        replaceSubrange(start..<end, with: rotated)
    }
}

func rowText(_ cells: some Collection<Cell>) -> String {
    var text = ""
    var pendingSpaces = 0
    for cell in cells where !cell.isWideCharacterSpacer {
        if let scalar = cell.scalar, cell.rendersText, cell.codepoint > 0x20 {
            text.append(String(repeating: " ", count: pendingSpaces))
            pendingSpaces = 0
            text.unicodeScalars.append(scalar)
        } else {
            pendingSpaces += 1
        }
    }
    return text
}
