import Foundation
import Observation
import TermyRemoteKit

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
    /// Called after each frame; the terminal view redraws.
    @ObservationIgnored var changedRows: [Int]?
    @ObservationIgnored var onChange: (() -> Void)?

    nonisolated var id: Int { stream }

    init(stream: Int, paneID: String, title: String?) {
        self.stream = stream
        self.paneID = paneID
        self.title = title
    }

    var visibleText: String {
        var result = ""
        for (index, row) in lines.enumerated() {
            let text = row.map(\.text).joined()
            result +=
                wrapped.indices.contains(index) && wrapped[index]
                ? text : text.replacingOccurrences(of: #" +$"#, with: "", options: .regularExpression)
            if index < lines.count - 1, !(wrapped.indices.contains(index) && wrapped[index]) { result += "\n" }
        }
        return result
    }
    func apply(_ frame: RemoteFrame) {
        guard frame.isValid else { return }
        changedRows =
            frame.full || frame.cols != cols || frame.rows != rows
            ? nil : Array(Set(frame.updates.map(\.row) + [cursor.row, frame.cursor.row]))
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
        displayOffset = max(0, frame.displayOffset ?? 0)
        historySize = max(0, frame.historySize ?? 0)
        cursor = frame.cursor
        background = frame.background
        live = true
        onChange?()
    }
}
