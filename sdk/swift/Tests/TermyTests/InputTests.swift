import Foundation
import Testing
@testable import Termy

@Suite("Input")
struct InputTests {
    private func encoded(_ key: Keystroke, after output: String = "") throws -> String {
        let terminal = try displayTerminal(feeding: output)
        return String(decoding: try terminal.encode(key), as: UTF8.self)
    }

    @Test func namedKeysUseLegacySequences() throws {
        #expect(try encoded(.enter) == "\r")
        #expect(try encoded(.tab) == "\t")
        #expect(try encoded(.backspace) == "\u{7f}")
        #expect(try encoded(.up) == "\u{1b}[A")
        #expect(try encoded(Keystroke(key: "f5")) == "\u{1b}[15~")
    }

    @Test func applicationCursorModeChangesArrows() throws {
        #expect(try encoded(.up, after: "\u{1b}[?1h") == "\u{1b}OA")
    }

    @Test func controlAndText() throws {
        #expect(try encoded(.control("c")) == "\u{03}")
        #expect(try encoded(.text("é")) == "é")
        #expect(try encoded(Keystroke(key: "a", text: "A", modifiers: .shift)) == "A")
    }

    @Test func kittyKeyboardModeDisambiguatesEscape() throws {
        #expect(try encoded(.escape, after: "\u{1b}[>1u") == "\u{1b}[27u")
    }

    @Test func displayTerminalsRouteInputToTheHost() throws {
        let terminal = try displayTerminal()
        var sent = Data()
        terminal.onInput = { sent.append($0) }
        try terminal.send(.enter)
        try terminal.write("ls")
        #expect(String(decoding: sent, as: UTF8.self) == "\rls")
    }

    @Test func mouseIsReportedOnlyWhenTracked() throws {
        let terminal = try displayTerminal()
        let click = MouseInput(.press(.left), at: GridPosition(row: 2, col: 4))
        #expect(try terminal.encode(click).isEmpty)

        try terminal.feed("\u{1b}[?1000h\u{1b}[?1006h")
        #expect(String(decoding: try terminal.encode(click), as: UTF8.self) == "\u{1b}[<0;5;3M")
        let wheel = MouseInput(.wheelUp, at: GridPosition(row: 0, col: 0))
        #expect(String(decoding: try terminal.encode(wheel), as: UTF8.self) == "\u{1b}[<64;1;1M")
    }

    @Test func pasteRespectsBracketedPasteMode() throws {
        let terminal = try displayTerminal()
        var sent = Data()
        terminal.onInput = { sent.append($0) }
        try terminal.paste("plain")
        #expect(String(decoding: sent, as: UTF8.self) == "plain")

        sent.removeAll()
        try terminal.feed("\u{1b}[?2004h")
        #expect(terminal.isBracketedPasteEnabled)
        try terminal.paste("a\u{1b}[201~b")
        #expect(String(decoding: sent, as: UTF8.self) == "\u{1b}[200~a[201~b\u{1b}[201~")
    }

    @Test func kittyPasteEventsReplaceTypedPastes() throws {
        // With mode 5522 the program reads the clipboard itself through
        // OSC 5522, so the text must not also arrive as typed input.
        let terminal = try displayTerminal(feeding: "\u{1b}[?5522h")
        #expect(terminal.isKittyClipboardPasteEventsEnabled)
        var sent = Data()
        terminal.onInput = { sent.append($0) }
        try terminal.paste("secret")
        _ = try drainAll(terminal) { sent.append($0) }
        let output = String(decoding: sent, as: UTF8.self)
        #expect(output.contains("\u{1b}]5522;type=read:status=OK"))
        #expect(!output.contains("secret"))
    }

    @Test func pasteCannotEscapeTheBracket() throws {
        let terminal = try displayTerminal()
        var sent = Data()
        terminal.onInput = { sent.append($0) }
        // A single-pass marker strip would collapse this into a real end marker.
        try terminal.feed("\u{1b}[?2004h")
        try terminal.paste("x\u{1b}[20\u{1b}[201~1~rm -rf ~\n")
        #expect(String(decoding: sent, as: UTF8.self) == "\u{1b}[200~x[20[201~1~rm -rf ~\r\u{1b}[201~")

        // Without bracketed paste, controls are still dropped.
        sent.removeAll()
        try terminal.feed("\u{1b}[?2004l")
        try terminal.paste("ls\u{3}\u{1b}[201~\r\n")
        #expect(String(decoding: sent, as: UTF8.self) == "ls[201~\r")
    }
}
