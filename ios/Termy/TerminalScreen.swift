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
    var fitted = false

    @ObservationIgnored private(set) var cols = 0
    @ObservationIgnored private(set) var rows = 0
    @ObservationIgnored private(set) var lines: [[CellRun]] = []
    @ObservationIgnored private(set) var cursor = RemoteCursor(row: 0, col: 0, visible: false, style: "block")
    @ObservationIgnored private(set) var background: UInt32 = 0x101421ff
    /// Called after each frame; the terminal view redraws.
    @ObservationIgnored var onChange: (() -> Void)?

    nonisolated var id: Int { stream }

    init(stream: Int, paneID: String, title: String?) {
        self.stream = stream
        self.paneID = paneID
        self.title = title
    }

    func apply(_ frame: RemoteFrame) {
        if frame.full || frame.cols != cols || frame.rows != rows {
            cols = frame.cols
            rows = frame.rows
            lines = Array(repeating: [], count: frame.rows)
        }
        for update in frame.updates where update.row < lines.count {
            lines[update.row] = update.runs
        }
        cursor = frame.cursor
        background = frame.background
        live = true
        onChange?()
    }
}
