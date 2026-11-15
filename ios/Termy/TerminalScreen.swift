import CoreGraphics
import Foundation
import ImageIO
import Observation
import TermyRemoteKit

/// A cell on the grid.
struct GridPoint: Hashable, Comparable {
    var row: Int
    var col: Int
    static func < (a: GridPoint, b: GridPoint) -> Bool { (a.row, a.col) < (b.row, b.col) }
}

/// The phone's copy of one pane's screen, kept current from host frames.
@MainActor
@Observable
final class TerminalScreen: Identifiable {
    let stream: Int
    let paneID: String
    var title: String?
    var exited = false
    /// False while the host is unreachable; the screen shows what it last had.
    var live = false
    /// Whether the pane is resized to fit this phone.
    var fitted = true
    var configuration = RemoteConfiguration()
    var searchMatches: [RemoteSearchMatch] = []
    var displayOffset = 0
    var historySize = 0

    @ObservationIgnored private(set) var cols = 0
    @ObservationIgnored private(set) var rows = 0
    @ObservationIgnored private(set) var wrapped: [Bool] = []
    @ObservationIgnored private(set) var lines: [[CellRun]] = []
    @ObservationIgnored private(set) var cursor = RemoteCursor(row: 0, col: 0, visible: false, style: "block")
    @ObservationIgnored private(set) var background: UInt32 = 0x1014_21ff
    /// Kitty graphics on the screen, and the images the host has sent.
    @ObservationIgnored private(set) var images: [RemoteImagePlacement] = []
    /// Screen rows where a shell prompt starts (protocol 5).
    @ObservationIgnored private(set) var promptRows: [Int] = []
    @ObservationIgnored private var pixels: [String: CGImage] = [:]
    /// Called after each frame; the terminal view redraws.
    @ObservationIgnored var changedRows: [Int]?
    @ObservationIgnored var onChange: (() -> Void)?

    /// Local echo for typed characters; see `EchoPredictor`.
    @ObservationIgnored private(set) var predictor = EchoPredictor()
    /// Round-trip time from a key to the frame that answers it, smoothed.
    @ObservationIgnored private(set) var latency: TimeInterval?
    @ObservationIgnored private var keySentAt: Date?
    /// Typed after the screen first goes live, e.g. a Shortcut's command.
    @ObservationIgnored var pendingInput: String?

    /// Where the shell is, from OSC 133 (protocol 5); `.unknown` without shell integration.
    enum ShellPhase { case unknown, prompt, input, running }
    private(set) var shellPhase = ShellPhase.unknown
    /// A command that ran long enough to be worth telling about when it ends.
    struct FinishedCommand: Equatable, Identifiable {
        let id = UUID()
        var exitCode: Int?
        var duration: TimeInterval
        var failed: Bool { (exitCode ?? 0) != 0 }
    }
    private(set) var finishedCommand: FinishedCommand?
    /// Commands shorter than this end without a notice.
    static let noticeAfter: TimeInterval = 5
    @ObservationIgnored private var commandStartedAt: Date?

    nonisolated var id: Int { stream }

    init(stream: Int, paneID: String, title: String?) {
        self.stream = stream
        self.paneID = paneID
        self.title = title
    }

    var visibleText: String { text(from: GridPoint(row: 0, col: 0), to: GridPoint(row: rows - 1, col: cols - 1)) }

    /// The characters of a row, one per column; a wide character's second cell is empty.
    func cells(row: Int) -> [String] {
        guard lines.indices.contains(row) else { return [] }
        var cells = Array(repeating: " ", count: cols)
        for run in lines[row] {
            let scalars = Array(run.text.unicodeScalars)
            if scalars.count == run.cells {
                for (index, scalar) in scalars.enumerated() where run.col + index < cols {
                    cells[run.col + index] = String(scalar)
                }
            } else if run.col < cols {
                cells[run.col] = run.text
                for col in (run.col + 1)..<min(cols, run.col + run.cells) { cells[col] = "" }
            }
        }
        return cells
    }

    func character(row: Int, col: Int) -> Character? {
        let cells = cells(row: row)
        guard cells.indices.contains(col) else { return nil }
        return cells[col].first
    }

    /// The text between two cells in reading order, both included. Wrapped rows join without a newline.
    func text(from start: GridPoint, to end: GridPoint) -> String {
        guard rows > 0, cols > 0 else { return "" }
        let (a, b) = start <= end ? (start, end) : (end, start)
        var result = ""
        for row in max(0, a.row)...min(rows - 1, b.row) {
            let cells = cells(row: row)
            let from = row == a.row ? max(0, a.col) : 0
            let to = row == b.row ? min(cols - 1, b.col) : cols - 1
            guard from <= to, !cells.isEmpty else { continue }
            let text = cells[from...to].joined()
            let soft = wrapped.indices.contains(row) && wrapped[row] && to == cols - 1
            result += soft ? text : text.replacingOccurrences(of: #" +$"#, with: "", options: .regularExpression)
            if row < b.row, !soft { result += "\n" }
        }
        return result
    }

    /// The word around a cell: the run of non-blank characters it is in.
    func word(at point: GridPoint) -> (GridPoint, GridPoint) {
        let cells = cells(row: point.row)
        guard cells.indices.contains(point.col) else { return (point, point) }
        guard cells[point.col] != " ", !cells[point.col].isEmpty else { return (point, point) }
        var start = point.col
        var end = point.col
        while start > 0, cells[start - 1] != " " { start -= 1 }
        while end < cells.count - 1, cells[end + 1] != " " { end += 1 }
        return (GridPoint(row: point.row, col: start), GridPoint(row: point.row, col: end))
    }

    func image(_ key: String) -> CGImage? { pixels[key] }

    /// The buffer row of a screen row, as the host numbers them for prompts and search: 0 is the oldest kept line.
    func bufferRow(ofScreenRow row: Int) -> Int { historySize - displayOffset + row }

    /// Keeps an image the host sent; a later `apply(_:)` places it.
    func store(image png: Data, key: String) {
        // Untrusted: bound what is decoded and how much is kept.
        guard key.count <= 64, png.count <= RemoteImages.maxImageBytes, pixels.count < RemoteImages.maxPlacements,
            let source = CGImageSourceCreateWithData(png as CFData, nil),
            let image = CGImageSourceCreateImageAtIndex(source, 0, nil),
            image.width <= 4096, image.height <= 4096
        else { return }
        pixels[key] = image
    }

    func apply(_ list: RemoteImages) {
        guard list.isValid else { return }
        for key in list.evicted { pixels[key] = nil }
        if list.full {
            let placed = Set(list.placements.map(\.key))
            pixels = pixels.filter { placed.contains($0.key) }
        }
        images = list.placements
        changedRows = nil
        onChange?()
    }

    func apply(_ frame: RemoteFrame, now: Date = Date()) {
        guard frame.isValid else { return }
        let previousPredictions = Set(predictor.visible.map(\.row))
        changedRows =
            frame.full || frame.cols != cols || frame.rows != rows
            ? nil : Array(Set(frame.updates.map(\.row) + [cursor.row, frame.cursor.row]).union(previousPredictions))
        if frame.full || frame.cols != cols || frame.rows != rows {
            cols = frame.cols
            rows = frame.rows
            lines = Array(repeating: [], count: frame.rows)
            wrapped = Array(repeating: false, count: frame.rows)
        }
        for update in frame.updates where update.row < lines.count {
            lines[update.row] = update.runs
            wrapped[update.row] = update.wrapped ?? false
        }
        let prompts = frame.promptRows ?? []
        if prompts != promptRows {
            changedRows = changedRows.map { Array(Set($0 + promptRows + prompts)) }
            promptRows = prompts
        }
        displayOffset = max(0, frame.displayOffset ?? 0)
        historySize = max(0, frame.historySize ?? 0)
        cursor = frame.cursor
        background = frame.background
        live = true
        if let sent = keySentAt {
            let sample = now.timeIntervalSince(sent)
            latency = latency.map { $0 * 0.8 + sample * 0.2 } ?? sample
            keySentAt = nil
        }
        if displayOffset > 0 {
            predictor.reset()
        } else {
            predictor.reconcile(cursor: cursor, now: now) { [unowned self] row, col in character(row: row, col: col) }
        }
        onChange?()
    }

    /// Records a key on its way to the host, for latency and local echo.
    func noteSent(_ key: RemoteKey, now: Date = Date()) {
        if keySentAt == nil { keySentAt = now }
        guard displayOffset == 0, shellPhase != .running else { return }
        predictor.noteKey(key, cursor: cursor, cols: cols, now: now)
    }

    func notePasted() { predictor.reset() }

    func apply(shell event: RemoteShellEvent, now: Date = Date()) {
        switch event {
        case .prompt: shellPhase = .prompt
        case .input: shellPhase = .input
        case .executing:
            shellPhase = .running
            commandStartedAt = now
            // Whatever runs now decides what it echoes.
            predictor.reset()
        case .finished(let code):
            if let start = commandStartedAt, now.timeIntervalSince(start) >= Self.noticeAfter {
                finishedCommand = FinishedCommand(exitCode: code, duration: now.timeIntervalSince(start))
            }
            commandStartedAt = nil
            shellPhase = .prompt
        }
    }

    func dismissFinishedCommand() { finishedCommand = nil }

    /// Drops predictions the host never answered. True when some were shown.
    func expirePredictions(now: Date = Date()) -> Bool {
        let shown = !predictor.visible.isEmpty
        return predictor.expire(now: now) && shown
    }

    /// Predictions worth drawing: only when the link is slow enough to notice.
    func predictions(enabled: Bool) -> [EchoPredictor.Prediction] {
        // A running program (an editor, a REPL) may not echo as typed; only the shell's prompt is safe.
        guard enabled, shellPhase != .running, let latency, latency > 0.04 else { return [] }
        return predictor.visible
    }
}
