import TermyRemoteKit
import XCTest

@testable import Termy

@MainActor final class TerminalTests: XCTestCase {
    func testRetainedRowsKeepPartialUpdatesAndRejectInvalidFrames() {
        let screen = TerminalScreen(stream: 1, paneID: "test", title: "Test")
        let cursor = RemoteCursor(row: 0, col: 0, visible: false, style: "block")
        let row = RowUpdate(row: 0, runs: [.init(col: 0, cells: 5, text: "hello", fg: 0xffff_ffff, bg: 0, flags: 0)])
        screen.apply(
            .init(stream: 1, cols: 40, rows: 12, full: true, updates: [row], cursor: cursor, background: 0x1014_21ff))
        screen.apply(
            .init(
                stream: 1, cols: 40, rows: 12, full: false, updates: [.init(row: 1, runs: row.runs)], cursor: cursor,
                background: 0x1014_21ff))
        XCTAssertEqual(screen.lines[0].first?.text, "hello")
        XCTAssertEqual(screen.lines[1].first?.text, "hello")
        screen.apply(.init(stream: 1, cols: 40, rows: -1, full: true, updates: [], cursor: cursor, background: 0))
        XCTAssertEqual(screen.rows, 12)
    }
    func testRealClaudeFrameWhenProvided() throws {
        let url = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0].appendingPathComponent(
            "claude-render-frame.json")
        guard FileManager.default.fileExists(atPath: url.path) else {
            throw XCTSkip("Optional real Claude Code frame not provided")
        }
        let frame = try JSONDecoder().decode(RemoteFrame.self, from: Data(contentsOf: url))
        XCTAssertTrue(frame.isValid)
        let screen = TerminalScreen(stream: frame.stream, paneID: "claude-test", title: "Claude Code")
        screen.fitted = false
        screen.apply(frame)
        let view = TerminalUIView(frame: CGRect(x: 0, y: 0, width: 390, height: 700))
        view.screen = screen
        let image = UIGraphicsImageRenderer(size: view.bounds.size).image { view.layer.render(in: $0.cgContext) }
        let attachment = XCTAttachment(image: image)
        attachment.name = "Real Claude Code through encrypted relay"
        attachment.lifetime = .keepAlways
        add(attachment)
        XCTAssertTrue(
            screen.visibleText.localizedCaseInsensitiveContains("Claude")
                || screen.visibleText.localizedCaseInsensitiveContains("trust"))
    }
    func testCopyJoinsWrappedRows() {
        let screen = TerminalScreen(stream: 1, paneID: "test", title: nil)
        var first = RowUpdate(row: 0, runs: [.init(col: 0, cells: 5, text: "hello", fg: 0, bg: 0, flags: 0)])
        first.wrapped = true
        screen.apply(
            .init(
                stream: 1, cols: 5, rows: 2, full: true,
                updates: [
                    first, .init(row: 1, runs: [.init(col: 0, cells: 5, text: "world", fg: 0, bg: 0, flags: 0)]),
                ], cursor: .init(row: 0, col: 0, visible: false, style: "block"), background: 0))
        XCTAssertEqual(screen.visibleText, "helloworld")
    }
    func testSyncedMetricsDetermineKeyboardAndLandscapeGrid() {
        let screen = TerminalScreen(stream: 1, paneID: "test", title: "Test")
        screen.configuration.fontSize = 14
        screen.configuration.cellWidth = 8
        screen.configuration.cellHeight = 18
        screen.configuration.paddingX = 8
        screen.configuration.paddingY = 8
        let view = TerminalUIView(frame: CGRect(x: 0, y: 0, width: 390, height: 700))
        view.screen = screen
        XCTAssertEqual(view.readableGrid.cols, 46)
        XCTAssertEqual(view.readableGrid.rows, 38)
        view.bounds.size.height = 350
        XCTAssertEqual(view.readableGrid.rows, 18)
        view.bounds.size = CGSize(width: 780, height: 320)
        XCTAssertEqual(view.readableGrid.cols, 95)
        XCTAssertEqual(view.readableGrid.rows, 16)
    }
    func testRendererDrawsStyledUnicodeAndWideCells() throws {
        let screen = TerminalScreen(stream: 1, paneID: "test", title: "Claude Code rendering check")
        screen.fitted = false
        screen.configuration.cellWidth = 8
        screen.configuration.cellHeight = 18
        let rows = [
            RowUpdate(
                row: 0,
                runs: [
                    .init(
                        col: 0, cells: 22, text: "Claude Code · Terminal", fg: 0xe6b4_82ff, bg: 0, flags: CellRun.bold)
                ]),
            RowUpdate(
                row: 2, runs: [.init(col: 0, cells: 20, text: "╭──────────────────╮", fg: 0x63c0_ffff, bg: 0, flags: 0)]
            ),
            RowUpdate(
                row: 3, runs: [.init(col: 0, cells: 20, text: "│ Reading project  │", fg: 0xffff_ffff, bg: 0, flags: 0)]
            ),
            RowUpdate(
                row: 4, runs: [.init(col: 0, cells: 20, text: "╰──────────────────╯", fg: 0x63c0_ffff, bg: 0, flags: 0)]
            ),
            RowUpdate(
                row: 6,
                runs: [
                    .init(col: 0, cells: 1, text: "✓", fg: 0x58ce_8fff, bg: 0, flags: 0),
                    .init(col: 2, cells: 2, text: "界", fg: 0xffff_ffff, bg: 0, flags: CellRun.underline),
                ]),
        ]
        screen.apply(
            .init(
                stream: 1, cols: 40, rows: 14, full: true, updates: rows,
                cursor: .init(row: 8, col: 0, visible: true, style: "bar"), background: 0x1014_21ff))
        let view = TerminalUIView(frame: CGRect(x: 0, y: 0, width: 390, height: 700))
        view.screen = screen
        let image = UIGraphicsImageRenderer(size: view.bounds.size).image { view.layer.render(in: $0.cgContext) }
        let data = try XCTUnwrap(image.pngData())
        XCTAssertGreaterThan(data.count, 1000)
        let attachment = XCTAttachment(image: image)
        attachment.name = "Mobile terminal renderer"
        attachment.lifetime = .keepAlways
        add(attachment)
    }
}
