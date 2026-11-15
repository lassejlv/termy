import TermyRemoteKit
import XCTest

@testable import Termy

final class EchoPredictorTests: XCTestCase {
    private func cursor(_ col: Int, row: Int = 0) -> RemoteCursor {
        RemoteCursor(row: row, col: col, visible: true, style: "block")
    }

    func testShowsNothingUntilTheHostEchoesOnce() {
        var predictor = EchoPredictor()
        predictor.noteKey(.text("l"), cursor: cursor(2), cols: 80)
        XCTAssertTrue(predictor.visible.isEmpty)
        predictor.reconcile(cursor: cursor(3)) { _, col in col == 2 ? "l" : nil }
        XCTAssertTrue(predictor.confirmed)
        XCTAssertTrue(predictor.pending.isEmpty)

        predictor.noteKey(.text("s"), cursor: cursor(3), cols: 80)
        XCTAssertEqual(predictor.visible.map(\.character), ["s"])
        XCTAssertEqual(predictor.visible.first?.col, 3)
    }

    func testPredictionsStackAndBackspaceTakesOneBack() {
        var predictor = EchoPredictor()
        predictor.noteKey(.text("a"), cursor: cursor(0), cols: 80)
        predictor.reconcile(cursor: cursor(1)) { _, _ in "a" }
        predictor.noteKey(.text("b"), cursor: cursor(1), cols: 80)
        predictor.noteKey(.text("c"), cursor: cursor(1), cols: 80)
        XCTAssertEqual(predictor.visible.map(\.col), [1, 2])
        predictor.noteKey(RemoteKey(key: "backspace"), cursor: cursor(1), cols: 80)
        XCTAssertEqual(predictor.visible.map(\.character), ["b"])
    }

    func testWrongEchoEndsTheEpoch() {
        var predictor = EchoPredictor()
        predictor.noteKey(.text("a"), cursor: cursor(0), cols: 80)
        predictor.reconcile(cursor: cursor(1)) { _, _ in "a" }
        predictor.noteKey(.text("b"), cursor: cursor(1), cols: 80)
        // The program drew something else and moved on.
        predictor.reconcile(cursor: cursor(2)) { _, _ in "*" }
        XCTAssertFalse(predictor.confirmed)
        XCTAssertTrue(predictor.pending.isEmpty)
    }

    func testEnterAndControlKeysEndTheEpoch() {
        var predictor = EchoPredictor()
        predictor.noteKey(.text("a"), cursor: cursor(0), cols: 80)
        predictor.reconcile(cursor: cursor(1)) { _, _ in "a" }
        predictor.noteKey(RemoteKey(key: "enter"), cursor: cursor(1), cols: 80)
        XCTAssertFalse(predictor.confirmed)
        predictor.noteKey(.text("p"), cursor: cursor(0, row: 1), cols: 80)
        XCTAssertTrue(predictor.visible.isEmpty, "A password prompt after Enter never shows predictions")

        predictor.reconcile(cursor: cursor(1, row: 1)) { _, _ in "p" }
        predictor.noteKey(RemoteKey(key: "c", control: true), cursor: cursor(1, row: 1), cols: 80)
        XCTAssertFalse(predictor.confirmed)
    }

    func testUnansweredPredictionsExpire() {
        var predictor = EchoPredictor()
        let start = Date()
        predictor.noteKey(.text("a"), cursor: cursor(0), cols: 80, now: start)
        predictor.reconcile(cursor: cursor(1)) { _, _ in "a" }
        predictor.noteKey(.text("b"), cursor: cursor(1), cols: 80, now: start)
        XCTAssertFalse(predictor.expire(now: start.addingTimeInterval(1)))
        XCTAssertTrue(predictor.expire(now: start.addingTimeInterval(2)))
        XCTAssertFalse(predictor.confirmed)
    }

    func testNeverGuessesALineWrapOrWideCharacter() {
        var predictor = EchoPredictor()
        predictor.noteKey(.text("a"), cursor: cursor(79), cols: 80)
        XCTAssertTrue(predictor.pending.isEmpty)
        predictor.noteKey(.text("界"), cursor: cursor(0), cols: 80)
        XCTAssertTrue(predictor.pending.isEmpty)
    }
}

@MainActor final class ScreenTextTests: XCTestCase {
    private func screen(_ rows: [String], cols: Int = 10, wrapped: Set<Int> = []) -> TerminalScreen {
        let screen = TerminalScreen(stream: 1, paneID: "p", title: nil)
        let updates = rows.enumerated().map { index, text in
            var update = RowUpdate(
                row: index, runs: text.isEmpty ? [] : [CellRun(col: 0, cells: text.count, text: text, fg: 0, bg: 0, flags: 0)])
            update.wrapped = wrapped.contains(index)
            return update
        }
        screen.apply(
            RemoteFrame(
                stream: 1, cols: cols, rows: rows.count, full: true, updates: updates,
                cursor: RemoteCursor(row: 0, col: 0, visible: true, style: "block"), background: 0))
        return screen
    }

    func testSelectionTextSpansRowsInReadingOrder() {
        let screen = screen(["hello all", "second", "third row"])
        XCTAssertEqual(screen.text(from: GridPoint(row: 0, col: 6), to: GridPoint(row: 1, col: 2)), "all\nsec")
        // Backwards selections read the same.
        XCTAssertEqual(screen.text(from: GridPoint(row: 1, col: 2), to: GridPoint(row: 0, col: 6)), "all\nsec")
    }

    func testWordSelectionStopsAtBlanks() {
        let screen = screen(["git push origin"], cols: 20)
        let word = screen.word(at: GridPoint(row: 0, col: 5))
        XCTAssertEqual(screen.text(from: word.0, to: word.1), "push")
        let blank = screen.word(at: GridPoint(row: 0, col: 3))
        XCTAssertEqual(blank.0, blank.1)
    }

    func testWideCharacterOwnsBothCells() {
        let screen = TerminalScreen(stream: 1, paneID: "p", title: nil)
        screen.apply(
            RemoteFrame(
                stream: 1, cols: 4, rows: 1, full: true,
                updates: [RowUpdate(row: 0, runs: [CellRun(col: 1, cells: 2, text: "界", fg: 0, bg: 0, flags: 0)])],
                cursor: RemoteCursor(row: 0, col: 0, visible: true, style: "block"), background: 0))
        XCTAssertEqual(screen.cells(row: 0), [" ", "界", "", " "])
        XCTAssertEqual(screen.visibleText, " 界")
    }

    func testPredictionsShowOnlyOnSlowLinks() {
        let screen = TerminalScreen(stream: 1, paneID: "p", title: nil)
        let start = Date()
        screen.apply(HostLinkTests.frame(stream: 1, text: "$ "), now: start)
        screen.noteSent(.text("l"), now: start)
        screen.apply(HostLinkTests.frame(stream: 1, text: "$ l"), now: start.addingTimeInterval(0.2))
        screen.noteSent(.text("s"), now: start.addingTimeInterval(0.3))
        XCTAssertEqual(screen.predictions(enabled: true).map(\.character), ["s"])
        XCTAssertTrue(screen.predictions(enabled: false).isEmpty)
    }
}

final class KeyBarAndLinkTests: XCTestCase {
    func testKeyBarLayoutDropsUnknownAndRepeatedKeys() throws {
        let defaults = try XCTUnwrap(UserDefaults(suiteName: "KeyBarAndLinkTests"))
        defer { defaults.removePersistentDomain(forName: "KeyBarAndLinkTests") }
        XCTAssertEqual(KeyBarLayout.load(defaults), KeyBarKey.defaultLayout)
        defaults.set(["escape", "warp-drive", "escape", "up"], forKey: KeyBarLayout.defaultsKey)
        XCTAssertEqual(KeyBarLayout.load(defaults), [.escape, .up])
        defaults.set(["nope"], forKey: KeyBarLayout.defaultsKey)
        XCTAssertEqual(KeyBarLayout.load(defaults), KeyBarKey.defaultLayout)
    }

    func testEverySwipeKeyHasACornerLabel() {
        for key in KeyBarKey.allCases {
            if let secondary = key.secondary { XCTAssertFalse(secondary.title.isEmpty, key.rawValue) }
        }
    }

    func testLinksRoundTripAndOnlyNavigate() {
        let host = UUID()
        XCTAssertEqual(TermyLink.parse(TermyLink.open(host: host)), .host(host, pane: nil))
        XCTAssertEqual(TermyLink.parse(TermyLink.open(host: host, pane: "p 1")), .host(host, pane: "p 1"))
        XCTAssertEqual(TermyLink.parse(TermyLink.pair), .pair)
        XCTAssertNil(TermyLink.parse(URL(string: "termy://run?host=\(host)&command=rm")!))
        XCTAssertNil(TermyLink.parse(URL(string: "https://open?host=\(host)")!))
        XCTAssertNil(TermyLink.parse(URL(string: "termy://open?host=nope")!))
    }
}
