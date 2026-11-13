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
        glyphs: [Int: GlyphPlanEntry] = [:], rasterize: Bool = true
    ) -> [CellRun] {
        var runs: [CellRun] = []
        // Whether the last run holds only one-cell ASCII characters, so the
        // next one can join it. Tracked here rather than rescanning the run's
        // text per cell, which made every row quadratic in its width.
        var lastIsASCII = false
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
                lastIsASCII = false
                col += 2
                continue
            }
            // A Kitty graphics placeholder shows an image, not a glyph.
            let text = cell.codepoint == 0x10EEEE ? " " : cell.text ?? " "
            let isASCII = text.utf8.count == 1
            let last = runs.count - 1
            if lastIsASCII, isASCII, runs[last].fg == fg, runs[last].bg == bg, runs[last].flags == style,
                runs[last].col + runs[last].cells == col
            {
                // In place, so the run's text buffer is appended to, not copied.
                runs[last].cells += 1
                runs[last].text += text
            } else {
                runs.append(CellRun(col: col, cells: 1, text: text, fg: fg, bg: bg, flags: style))
                lastIsASCII = isASCII
            }
            col += 1
        }
        guard rasterize else { return runs }
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

    /// `message` encoded as payloads of at most `limit` bytes. The relay
    /// closes a socket that sends a larger message, so a frame too large for
    /// it (a wide screen of glyph images) goes as several: the first keeps
    /// `full`, and the rest patch their rows into it.
    static func payloads(for message: HostMessage, limit: Int) throws -> [Data] {
        var stripped = false
        return try payloads(for: message, limit: limit, strippedGlyphs: &stripped)
    }

    /// `strippedGlyphs` is set when a row had to go without its glyph PNGs.
    static func payloads(for message: HostMessage, limit: Int, strippedGlyphs: inout Bool) throws -> [Data] {
        let payload = try MessageCodec.encode(message)
        guard payload.count > limit, case .frame(var frame) = message else { return [payload] }
        if frame.updates.count > 1 {
            let half = frame.updates.count / 2
            var rest = frame
            frame.updates = Array(frame.updates[..<half])
            rest.updates = Array(rest.updates[half...])
            rest.full = false
            return try payloads(for: .frame(frame), limit: limit, strippedGlyphs: &strippedGlyphs)
                + payloads(for: .frame(rest), limit: limit, strippedGlyphs: &strippedGlyphs)
        }
        // A single row is still too large: send its text without the host's glyphs.
        guard frame.updates.contains(where: { $0.runs.contains { $0.glyphPNG != nil } }) else { return [payload] }
        for update in frame.updates.indices {
            for run in frame.updates[update].runs.indices {
                frame.updates[update].runs[run].glyphPNG = nil
                frame.updates[update].runs[run].glyphKey = nil
            }
        }
        strippedGlyphs = true
        return try payloads(for: .frame(frame), limit: limit, strippedGlyphs: &strippedGlyphs)
    }

    /// Where a frame's runs need a glyph drawn by the host.
    struct GlyphSlot {
        var update: Int
        var run: Int
        var job: GlyphRasterizer.Job
    }

    /// Names each run that needs a host glyph, fills in the PNGs drawn
    /// before, and returns the ones to draw (`GlyphRasterizer.render`, on any
    /// thread). Runs with no font for their glyph keep plain text.
    @MainActor static func attachGlyphs(
        to frame: inout RemoteFrame, configuration: RemoteConfiguration, glyphs: [Int: GlyphPlanEntry]
    ) -> [GlyphSlot] {
        var slots: [GlyphSlot] = []
        for update in frame.updates.indices {
            let row = frame.updates[update].row
            for index in frame.updates[update].runs.indices {
                let run = frame.updates[update].runs[index]
                guard run.text.unicodeScalars.contains(where: { !$0.isASCII }) else { continue }
                let key = GlyphRasterizer.cacheKey(
                    text: run.text, cells: run.cells, flags: run.flags, color: run.fg, config: configuration)
                if let png = GlyphRasterizer.cached(key) {
                    frame.updates[update].runs[index].glyphPNG = png
                    frame.updates[update].runs[index].glyphKey = GlyphRasterizer.glyphKey(key)
                } else if let job = GlyphRasterizer.job(
                    text: run.text, cells: run.cells, flags: run.flags, color: run.fg,
                    config: configuration, plan: glyphs[row * frame.cols + run.col])
                {
                    slots.append(GlyphSlot(update: update, run: index, job: job))
                }
            }
        }
        return slots
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
