internal import CTermy

/// What changed when an update was applied to a `FrameStore`.
public struct FrameChange: Hashable, Sendable {
    public var damage: FrameDamage
    /// The grid dimensions changed; redraw everything.
    public var resized: Bool
    public var previousCursor: TerminalCursor
    public var cursor: TerminalCursor
    /// The scroll position or history length changed.
    public var viewportChanged: Bool
    /// Some cell now holds something else. Damage alone doesn't say so: the
    /// engine also marks the cells a cursor left or entered.
    public var cellsChanged = true
    /// Scrolls replayed before `damage` patched the frame. Rows they moved
    /// hold what the rows they came from held, and are not in `damage`.
    /// Only set when the update was taken replaying scrolls.
    public var scrolls: [ViewportScroll] = []

    public var cursorChanged: Bool { previousCursor != cursor }

    /// Cells, the scroll position or history, or the grid size changed.
    public var contentChanged: Bool { cellsChanged || resized || viewportChanged }

    /// True when anything visible may have changed.
    public var needsDisplay: Bool {
        damage != .none || resized || cursorChanged || viewportChanged
    }
}

public enum FrameStoreError: Error, Hashable, Sendable {
    /// A partial update arrived before a full frame, or after a resize.
    case needsFullFrame
    case cellCountMismatch(expected: Int, actual: Int)
    case spanOutOfBounds(DirtySpan)
}

/// A retained viewport that applies full and partial frame updates.
///
/// Renderers keep one store per terminal, patch it with each update, and draw
/// from it. On `FrameStoreError`, request a forced full frame to recover.
public final class FrameStore {
    public private(set) var cols = 0
    public private(set) var rows = 0
    public private(set) var cursor = TerminalCursor(isVisible: false, row: 0, col: 0, style: .block)
    public private(set) var displayOffset = 0
    public private(set) var historySize = 0
    /// Increments on every applied update.
    public private(set) var generation: UInt64 = 0

    /// Row-major cells in the C layout, so glyph plans can borrow them directly.
    private(set) var rawCells: [TermyFfiCell] = []

    public init() {}

    /// True until the first full frame is applied.
    public var isEmpty: Bool { rawCells.isEmpty }

    public subscript(row: Int, col: Int) -> Cell {
        Cell(rawCells[row * cols + col])
    }

    public func cells(row: Int) -> [Cell] {
        rawCells[row * cols ..< (row + 1) * cols].map(Cell.init)
    }

    /// The visible text of one row, without trailing blanks.
    public func text(row: Int) -> String {
        rowText(rawCells[row * cols ..< (row + 1) * cols].lazy.map(Cell.init))
    }

    /// Text between two viewport positions (inclusive, reading order). Wrapped
    /// rows join without a newline; other rows end with one.
    public func text(from start: GridPosition, to end: GridPosition) -> String {
        guard !isEmpty else { return "" }
        let (first, last) = start <= end ? (start, end) : (end, start)
        // A selection can lie outside the grid after a shrinking resize; a
        // closed range with its bounds inverted would trap.
        let firstRow = max(0, first.row)
        let lastRow = min(rows - 1, last.row)
        guard firstRow <= lastRow else { return "" }
        var lines: [String] = []
        var current = ""
        for row in firstRow...lastRow {
            let from = row == first.row ? max(0, first.col) : 0
            let to = row == last.row ? min(cols - 1, last.col) : cols - 1
            guard from <= to else { continue }
            let slice = rawCells[row * cols + from ... row * cols + to].lazy.map(Cell.init)
            current += rowText(slice)
            let wraps = rawCells[row * cols + cols - 1].line_wrapped
            if !(wraps && row != last.row) {
                lines.append(current)
                current = ""
            }
        }
        if !current.isEmpty { lines.append(current) }
        return lines.joined(separator: "\n")
    }

    /// A value copy of the retained viewport.
    public var frame: Frame {
        Frame(
            cols: cols, rows: rows, cells: rawCells.map(Cell.init), cursor: cursor,
            displayOffset: displayOffset, historySize: historySize
        )
    }

    /// Applies a Swift frame update.
    @discardableResult
    public func apply(_ update: FrameUpdate) throws(FrameStoreError) -> FrameChange {
        try apply(
            cols: update.cols, rows: update.rows, damage: update.damage,
            cellCount: update.cells.count, cursor: update.cursor,
            displayOffset: update.displayOffset, historySize: update.historySize
        ) { index in update.cells[index].ffi }
    }

    /// Applies a C frame update without converting its cells, replaying
    /// `scrolls` first.
    func apply(_ update: TermyFfiFrameUpdate, scrolls: [ViewportScroll] = []) throws(FrameStoreError) -> FrameChange {
        let cells = UnsafeBufferPointer(start: update.cells_ptr, count: update.cells_len)
        return try apply(
            cols: Int(update.cols), rows: Int(update.rows), damage: FrameDamage(update),
            cellCount: cells.count, cursor: TerminalCursor(update.cursor),
            displayOffset: update.display_offset, historySize: update.history_size, scrolls: scrolls
        ) { index in cells[index] }
    }

    /// Resets to empty so the next update must be full.
    public func reset() {
        rawCells.removeAll()
        cols = 0
        rows = 0
        generation &+= 1
    }

    private func apply(
        cols newCols: Int,
        rows newRows: Int,
        damage: FrameDamage,
        cellCount: Int,
        cursor newCursor: TerminalCursor,
        displayOffset newOffset: Int,
        historySize newHistory: Int,
        scrolls: [ViewportScroll] = [],
        cell: (Int) -> TermyFfiCell
    ) throws(FrameStoreError) -> FrameChange {
        let resized = newCols != cols || newRows != rows
        var cellsChanged = false
        var replayed: [ViewportScroll] = []
        switch damage {
        case .full:
            let expected = newCols * newRows
            guard cellCount == expected else {
                throw .cellCountMismatch(expected: expected, actual: cellCount)
            }
            let cells = (0..<cellCount).map(cell)
            cellsChanged = resized || !zip(cells, rawCells).allSatisfy(TermyFfiCell.same)
            rawCells = cells
        case .partial(let spans):
            guard !isEmpty, !resized else { throw .needsFullFrame }
            let expected = spans.reduce(0) { $0 + $1.width }
            guard cellCount == expected else {
                throw .cellCountMismatch(expected: expected, actual: cellCount)
            }
            // Check every span before moving anything, so a bad update
            // leaves the frame as it was for the full frame that recovers it.
            for span in spans {
                guard span.row >= 0, span.row < newRows, span.leftCol >= 0,
                      span.leftCol <= span.rightCol, span.rightCol < newCols else {
                    throw .spanOutOfBounds(span)
                }
            }
            guard scrolls.allSatisfy({ $0.fits(rows: newRows) }) else { throw .needsFullFrame }
            for scroll in scrolls {
                rawCells.replay(scroll, width: newCols)
            }
            replayed = scrolls
            if !scrolls.isEmpty { cellsChanged = true }
            var source = 0
            for span in spans {
                guard span.row >= 0, span.row < newRows, span.leftCol >= 0,
                      span.leftCol <= span.rightCol, span.rightCol < newCols else {
                    throw .spanOutOfBounds(span)
                }
                let destination = span.row * newCols + span.leftCol
                for offset in 0..<span.width {
                    let new = cell(source + offset)
                    if !cellsChanged, !TermyFfiCell.same(rawCells[destination + offset], new) { cellsChanged = true }
                    rawCells[destination + offset] = new
                }
                source += span.width
            }
        case .none:
            guard !isEmpty, !resized else { throw .needsFullFrame }
            // Scrolled rows that need no patch, such as blank ones.
            guard scrolls.allSatisfy({ $0.fits(rows: newRows) }) else { throw .needsFullFrame }
            for scroll in scrolls {
                rawCells.replay(scroll, width: newCols)
            }
            replayed = scrolls
            if !scrolls.isEmpty { cellsChanged = true }
        }

        let change = FrameChange(
            damage: damage,
            resized: resized,
            previousCursor: cursor,
            cursor: newCursor,
            viewportChanged: newOffset != displayOffset || newHistory != historySize,
            cellsChanged: cellsChanged,
            scrolls: replayed
        )
        cols = newCols
        rows = newRows
        cursor = newCursor
        displayOffset = newOffset
        historySize = newHistory
        generation &+= 1
        return change
    }
}

/// A zero-based viewport cell position.
public struct GridPosition: Hashable, Comparable, Sendable {
    public var row: Int
    public var col: Int

    public init(row: Int, col: Int) {
        self.row = row
        self.col = col
    }

    public static func < (lhs: GridPosition, rhs: GridPosition) -> Bool {
        (lhs.row, lhs.col) < (rhs.row, rhs.col)
    }
}

extension TermyFfiCell {
    /// Field by field: the C struct has padding, so its bytes can't be compared.
    static func same(_ a: TermyFfiCell, _ b: TermyFfiCell) -> Bool {
        a.codepoint == b.codepoint
            && a.fg.r == b.fg.r && a.fg.g == b.fg.g && a.fg.b == b.fg.b && a.fg.a == b.fg.a
            && a.bg.r == b.bg.r && a.bg.g == b.bg.g && a.bg.b == b.bg.b && a.bg.a == b.bg.a
            && a.uses_terminal_default_bg == b.uses_terminal_default_bg && a.bold == b.bold
            && a.render_text == b.render_text && a.wide_character_spacer == b.wide_character_spacer
            && a.line_wrapped == b.line_wrapped && a.italic == b.italic && a.underline == b.underline
            && a.strikethrough == b.strikethrough
    }
}

extension Cell {
    var ffi: TermyFfiCell {
        TermyFfiCell(
            codepoint: codepoint,
            fg: TermyFfiColor(r: foreground.r, g: foreground.g, b: foreground.b, a: foreground.a),
            bg: TermyFfiColor(r: background.r, g: background.g, b: background.b, a: background.a),
            uses_terminal_default_bg: usesDefaultBackground,
            bold: isBold,
            render_text: rendersText,
            wide_character_spacer: isWideCharacterSpacer,
            line_wrapped: isLineWrapped,
            italic: isItalic,
            underline: isUnderlined,
            strikethrough: isStrikethrough
        )
    }
}
