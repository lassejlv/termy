import Foundation
import Testing
@testable import Termy

@Suite("Events")
struct EventTests {
    @Test func titleBellAndDirectory() throws {
        let terminal = try displayTerminal(
            feeding: "\u{1b}]2;Build\u{07}\u{07}\u{1b}]7;file://host/tmp/project%20one\u{1b}\\")
        let events = try drainAll(terminal)
        #expect(events.contains(.title("Build")))
        #expect(events.contains(.bell))
        #expect(events.contains(.workingDirectory("/tmp/project one")))
    }

    @Test func progressReports() throws {
        let terminal = try displayTerminal(feeding: "\u{1b}]9;4;1;42\u{07}")
        #expect(try drainAll(terminal).contains(.progress(.inProgress(42))))
        try terminal.feed("\u{1b}]9;4;3;0\u{07}")
        #expect(try drainAll(terminal).contains(.progress(.indeterminate)))
    }

    @Test func shellIntegrationLifecycle() throws {
        let terminal = try displayTerminal(
            feeding: "\u{1b}]133;A\u{07}\u{1b}]133;B\u{07}\u{1b}]133;C\u{07}\u{1b}]133;D;2\u{07}\u{1b}]133;D\u{07}")
        let events = try drainAll(terminal).filter { $0 != .wakeup }
        #expect(events == [
            .shellPromptStart, .shellCommandStart, .shellCommandExecuting,
            .shellCommandFinished(exitCode: 2), .shellCommandFinished(exitCode: nil),
        ])
    }

    @Test func programStatusSnapshots() throws {
        // "Approve?" in base64.
        let terminal = try displayTerminal(
            feeding: "\u{1b}]7501;state=working:app=build:progress=40\u{1b}\\"
                + "\u{1b}]7501;state=blocked:id=deploy:kind=permission:msg=QXBwcm92ZT8=\u{1b}\\")
        let snapshots = try drainAll(terminal).compactMap { event -> [ProgramStatusRecord]? in
            if case .programStatus(let records) = event { return records }
            return nil
        }
        let records = try #require(snapshots.last)
        let root = try #require(records.first { $0.id.isEmpty })
        #expect(root.state == .working)
        #expect(root.progress == 40)
        #expect(root.app == "build")
        let deploy = try #require(records.first { $0.id == "deploy" })
        #expect(deploy.state == .blocked)
        #expect(deploy.kind == .permission)
        #expect(deploy.msg == "Approve?")
        #expect(deploy.app == "build")
        #expect(records.mostUrgent?.id == "deploy")

        try terminal.feed("\u{1b}]7501;state=clear\u{1b}\\")
        #expect(try drainAll(terminal).contains(.programStatus([])))
    }

    @Test func displayTerminalsForwardProtocolReplies() throws {
        let terminal = try displayTerminal(feeding: "\u{1b}[c\u{1b}[6n")
        var replies = Data()
        _ = try drainAll(terminal) { replies.append($0) }
        #expect(String(decoding: replies, as: UTF8.self) == "\u{1b}[?62;22c\u{1b}[1;1R")
    }

    @Test func kittyClipboardReadsGoThroughTheHandler() throws {
        final class Clipboard: KittyClipboardHandler {
            var requests: [ClipboardReadRequest] = []
            func read(_ request: ClipboardReadRequest) -> ClipboardReadResponse {
                requests.append(request)
                return ClipboardReadResponse(
                    result: .success,
                    contents: [ClipboardContent(mimeType: "text/plain", data: Data("hi".utf8))])
            }
            func write(_ request: ClipboardWriteRequest) -> ClipboardWriteResponse {
                ClipboardWriteResponse(result: .denied)
            }
        }
        let clipboard = Clipboard()
        // OSC 5522 read request for text/plain (MIME type base64-encoded).
        let terminal = try displayTerminal(feeding: "\u{1b}]5522;type=read;dGV4dC9wbGFpbg==\u{1b}\\")
        var replies = Data()
        _ = try drainAll(terminal, clipboard: clipboard) { replies.append($0) }
        #expect(clipboard.requests.count == 1)
        #expect(clipboard.requests.first?.mimeTypes == ["text/plain"])
        #expect(!replies.isEmpty)
    }

    @Test func clipboardHandlersCannotReenterTheTerminal() throws {
        // libtermy holds the clipboard lock while a handler runs, so a nested
        // drain or paste (say from a modal run loop) must throw, not deadlock.
        final class Reentrant: KittyClipboardHandler {
            weak var terminal: Terminal?
            var errors: [TermyError] = []
            var sawDraining = false
            func read(_ request: ClipboardReadRequest) -> ClipboardReadResponse {
                sawDraining = terminal?.isDraining == true
                do { try terminal?.paste("x") } catch { errors.append(error) }
                do { _ = try terminal?.drainEvents() } catch { errors.append(error) }
                return .denied
            }
            func write(_ request: ClipboardWriteRequest) -> ClipboardWriteResponse {
                ClipboardWriteResponse(result: .denied)
            }
        }
        let terminal = try displayTerminal(feeding: "\u{1b}]5522;type=read;dGV4dC9wbGFpbg==\u{1b}\\")
        let handler = Reentrant()
        handler.terminal = terminal
        _ = try drainAll(terminal, clipboard: handler) { _ in }
        #expect(handler.sawDraining)
        #expect(handler.errors.map(\.code) == [.invalidArgument, .invalidArgument])
        #expect(!terminal.isDraining)

        var sent = Data()
        terminal.onInput = { sent.append($0) }
        try terminal.paste("ok")
        #expect(String(decoding: sent, as: UTF8.self) == "ok")
    }
}
