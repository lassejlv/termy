import Foundation
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

    @MainActor static func runs(
        _ cells: [Cell], configuration: RemoteConfiguration = .init(), row: Int = 0, cols: Int = 0,
        glyphs: [Int: GlyphPlanEntry] = [:]
    ) -> [CellRun] {
        var runs: [CellRun] = []
        var col = 0
        while col < cells.count {
            let cell = cells[col]
            if cell.isWideCharacterSpacer {
                col += 1
                continue
            }
            let fg = cell.foreground.rgba
            let bg = cell.background.rgba
            let style = flags(cell)
            let isWide = col + 1 < cells.count && cells[col + 1].isWideCharacterSpacer
            if isWide {
                runs.append(CellRun(col: col, cells: 2, text: cell.text ?? " ", fg: fg, bg: bg, flags: style))
                col += 2
                continue
            }
            // A Kitty graphics placeholder shows an image, not a glyph.
            let text = cell.codepoint == 0x10EEEE ? " " : cell.text ?? " "
            if var last = runs.last, last.fg == fg, last.bg == bg, last.flags == style,
                last.col + last.cells == col, last.cells == last.text.unicodeScalars.count,
                last.glyphPNG == nil, last.text.unicodeScalars.allSatisfy(\.isASCII),
                text.unicodeScalars.allSatisfy(\.isASCII), text.unicodeScalars.count == 1
            {
                last.cells += 1
                last.text += text
                runs[runs.count - 1] = last
            } else {
                runs.append(CellRun(col: col, cells: 1, text: text, fg: fg, bg: bg, flags: style))
            }
            col += 1
        }
        return runs.map { run in
            var run = run
            if run.text.unicodeScalars.contains(where: { !$0.isASCII }) {
                run.glyphPNG = GlyphRasterizer.png(
                    text: run.text, cells: run.cells, flags: run.flags,
                    color: run.fg, config: configuration, plan: glyphs[row * cols + run.col])
            }
            return run
        }
    }

    static func cursor(_ cursor: TerminalCursor) -> RemoteCursor {
        let style =
            switch cursor.style {
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
