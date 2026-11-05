import Termy
import TermyRemoteKit

/// Turns libtermy rows into style runs for the phone.
enum FrameEncoder {
    static func flags(_ cell: Cell) -> UInt8 {
        var flags: UInt8 = 0
        if cell.isBold { flags |= CellRun.bold }
        if cell.isItalic { flags |= CellRun.italic }
        if cell.isUnderlined { flags |= CellRun.underline }
        if cell.isStrikethrough { flags |= CellRun.strikethrough }
        if cell.usesDefaultBackground { flags |= CellRun.defaultBackground }
        return flags
    }

    static func runs(_ cells: [Cell]) -> [CellRun] {
        var runs: [CellRun] = []
        var col = 0
        while col < cells.count {
            let cell = cells[col]
            if cell.isWideCharacterSpacer {
                col += 1
                continue
            }
            let fg = cell.foreground.rgba, bg = cell.background.rgba, style = flags(cell)
            let isWide = col + 1 < cells.count && cells[col + 1].isWideCharacterSpacer
            if isWide {
                runs.append(CellRun(col: col, cells: 2, text: cell.text ?? " ", fg: fg, bg: bg, flags: style))
                col += 2
                continue
            }
            let text = cell.text ?? " "
            if var last = runs.last, last.fg == fg, last.bg == bg, last.flags == style,
               last.col + last.cells == col, last.cells == last.text.unicodeScalars.count,
               text.unicodeScalars.count == 1
            {
                last.cells += 1
                last.text += text
                runs[runs.count - 1] = last
            } else {
                runs.append(CellRun(col: col, cells: 1, text: text, fg: fg, bg: bg, flags: style))
            }
            col += 1
        }
        return runs
    }

    static func cursor(_ cursor: TerminalCursor) -> RemoteCursor {
        let style = switch cursor.style {
        case .bar: "bar"
        case .block: "block"
        case .unknown: "block"
        }
        return RemoteCursor(row: cursor.row, col: cursor.col, visible: cursor.isVisible, style: style)
    }

    /// Rows a change touched; nil means all of them.
    static func dirtyRows(_ change: FrameChange) -> Set<Int>? {
        if change.resized || change.viewportChanged { return nil }
        switch change.damage {
        case .full: return nil
        case .none: return []
        case .partial(let spans): return Set(spans.map(\.row))
        }
    }
}
