import AppKit
import Testing
import Termy
@testable import TermyAppKit

@Suite("Terminal view (CoreGraphics)")
@MainActor
struct TerminalViewTests {
    private func makeView(feeding output: String = "") throws -> TerminalView {
        let config = try TermyConfig(contents: "padding_x = 4\npadding_y = 4\ncursor_blink = false\n")
        let render = try config.renderConfig(for: .dark)
        let session = try TerminalSession.display(
            size: TerminalSize(cols: 20, rows: 5, cellWidth: render.cellWidth, cellHeight: render.cellHeight))
        let view = TerminalView(session: session, renderConfig: render, backend: .coreGraphics)
        view.frame = NSRect(origin: .zero, size: view.size(cols: 20, rows: 5))
        if !output.isEmpty { try session.feed(Data(output.utf8)) }
        return view
    }

    private func privatePasteboard() -> NSPasteboard {
        NSPasteboard(name: NSPasteboard.Name("dev.termy.tests.\(UUID().uuidString)"))
    }

    @MainActor
    private final class Delegate: TerminalViewDelegate {
        var allowsClipboardWrites = false
        var allowsLinks = true
        var clipboardRequests: [String] = []

        func terminalView(_ view: TerminalView, shouldWriteClipboard text: String) -> Bool {
            clipboardRequests.append(text)
            return allowsClipboardWrites
        }

        func terminalView(_ view: TerminalView, shouldOpen url: URL) -> Bool { allowsLinks }
    }

    private func render(_ view: TerminalView) throws -> NSBitmapImageRep {
        let bitmap = try #require(view.bitmapImageRepForCachingDisplay(in: view.bounds))
        view.cacheDisplay(in: view.bounds, to: bitmap)
        return bitmap
    }

    private func color(in bitmap: NSBitmapImageRep, view: TerminalView, row: Int, col: Int) throws -> NSColor {
        let rect = view.renderer.cellRect(row: row, col: col)
        let scale = CGFloat(bitmap.pixelsWide) / view.bounds.width
        let color = try #require(bitmap.colorAt(x: Int(rect.midX * scale), y: Int(rect.midY * scale)))
        return try #require(color.usingColorSpace(.sRGB))
    }

    @Test func sizesTheGridToTheView() throws {
        let view = try makeView()
        #expect(view.session.size.cols == 20)
        #expect(view.session.size.rows == 5)
        view.setFrameSize(view.size(cols: 30, rows: 8))
        #expect(view.session.size.cols == 30)
        #expect(view.session.size.rows == 8)
        #expect(view.session.frame.cols == 30)
    }

    @Test func rendersCellBackgrounds() throws {
        let view = try makeView(feeding: "\u{1b}[41m  \u{1b}[0m")
        let bitmap = try render(view)
        let red = try color(in: bitmap, view: view, row: 0, col: 0)
        #expect(red.redComponent > 0.5)
        #expect(red.greenComponent < 0.4)

        let background = try color(in: bitmap, view: view, row: 3, col: 10)
        let expected = view.renderConfig.background
        #expect(abs(background.redComponent - CGFloat(expected.r) / 255) < 0.05)
        #expect(abs(background.blueComponent - CGFloat(expected.b) / 255) < 0.05)
    }

    @Test func rendersGlyphsInsideTheirCells() throws {
        let view = try makeView(feeding: "\u{1b}[37mHH")
        let bitmap = try render(view)
        let rect = view.renderer.cellRect(row: 0, col: 0, width: 2)
        let scale = CGFloat(bitmap.pixelsWide) / view.bounds.width
        var brightest: CGFloat = 0
        for y in Int(rect.minY * scale)..<Int(rect.maxY * scale) {
            for x in Int(rect.minX * scale)..<Int(rect.maxX * scale) {
                guard let color = bitmap.colorAt(x: x, y: y)?.usingColorSpace(.sRGB) else { continue }
                brightest = max(brightest, color.greenComponent)
            }
        }
        #expect(brightest > 0.5)
    }

    @Test func selectionCopiesText() throws {
        let view = try makeView(feeding: "hello world\r\nsecond")
        view.selection = TerminalSelection(
            anchor: GridPosition(row: 0, col: 6), head: GridPosition(row: 1, col: 2))
        #expect(view.selection?.text(in: view.session.frame) == "world\nsec")

        view.selection = TerminalSelection(anchor: GridPosition(row: 0, col: 2), granularity: .word)
        #expect(view.selection?.text(in: view.session.frame) == "hello")

        view.selection = TerminalSelection(anchor: GridPosition(row: 1, col: 0), granularity: .line)
        #expect(view.selection?.text(in: view.session.frame) == "second")
    }

    @Test func boxDrawingUsesGlyphGeometry() throws {
        let view = try makeView(feeding: "┌─┐")
        #expect(Set(view.glyphPlan.keys) == [0, 1, 2])
        try view.session.feed(Data("\r\u{1b}[Kabc".utf8))
        #expect(view.glyphPlan.isEmpty)
    }

    @Test func programsCannotWriteTheClipboardUnlessTheHostAllows() throws {
        let view = try makeView()
        let pasteboard = privatePasteboard()
        defer { pasteboard.releaseGlobally() }
        view.pasteboard = pasteboard
        pasteboard.clearContents()
        pasteboard.setString("mine", forType: .string)
        let store = Data("\u{1b}]52;c;c3RvcmVkIHRleHQ=\u{7}".utf8)

        try view.session.feed(store)
        #expect(pasteboard.string(forType: .string) == "mine")

        let delegate = Delegate()
        view.delegate = delegate
        try view.session.feed(store)
        #expect(delegate.clipboardRequests == ["stored text"])
        #expect(pasteboard.string(forType: .string) == "mine")

        delegate.allowsClipboardWrites = true
        try view.session.feed(store)
        #expect(pasteboard.string(forType: .string) == "stored text")
    }

    @Test func linksOpenOnlySafeTargets() throws {
        func action(for uri: String, schemes: Set<String>? = nil, delegate: Delegate? = nil) throws -> TerminalView.LinkAction? {
            let view = try makeView(feeding: "\u{1b}]8;;\(uri)\u{1b}\\L\u{1b}]8;;\u{1b}\\")
            if let schemes { view.allowedLinkSchemes = schemes }
            view.delegate = delegate
            return view.linkAction(at: GridPosition(row: 0, col: 0))
        }
        let web = try #require(URL(string: "https://example.com/docs"))
        #expect(try action(for: web.absoluteString) == .open(web))
        #expect(try action(for: "HTTPS://example.com") != nil)
        // Link text is chosen by the program, so file and ftp targets are opt-in.
        #expect(try action(for: "file:///tmp/notes.txt") == nil)
        #expect(try action(for: "ftp://example.com/x.app") == nil)

        let files: Set<String> = ["FILE"]
        let notes = try #require(URL(string: "file:///tmp/notes.txt"))
        #expect(try action(for: notes.absoluteString, schemes: files) == .open(notes))
        let app = try #require(URL(string: "file:///Applications/Calculator.app"))
        #expect(try action(for: app.absoluteString, schemes: files) == .reveal(app))

        let veto = Delegate()
        veto.allowsLinks = false
        #expect(try action(for: web.absoluteString, delegate: veto) == nil)
    }

    @Test func launchableFilesAreRecognized() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("termy-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let tool = directory.appendingPathComponent("tool")
        let notes = directory.appendingPathComponent("notes.txt")
        FileManager.default.createFile(atPath: tool.path, contents: Data("#!/bin/sh\n".utf8), attributes: [.posixPermissions: 0o755])
        FileManager.default.createFile(atPath: notes.path, contents: Data(), attributes: [.posixPermissions: 0o644])

        #expect(TerminalView.isLaunchable(tool))
        #expect(!TerminalView.isLaunchable(notes))
        #expect(!TerminalView.isLaunchable(directory))
        #expect(TerminalView.isLaunchable(directory.appendingPathComponent("Run.command")))
        #expect(TerminalView.isLaunchable(directory.appendingPathComponent("Thing.APP")))
    }

    @Test func pastedFilesBecomeQuotedPaths() throws {
        let view = try makeView()
        let pasteboard = privatePasteboard()
        defer { pasteboard.releaseGlobally() }
        view.pasteboard = pasteboard
        var sent = Data()
        view.session.terminal.onInput = { sent.append($0) }

        pasteboard.clearContents()
        pasteboard.writeObjects([
            URL(fileURLWithPath: "/tmp/my dir/it's $(id)") as NSURL,
            URL(fileURLWithPath: "/tmp/plain-1.txt") as NSURL,
        ])
        view.paste(nil)
        #expect(String(decoding: sent, as: UTF8.self) == "'/tmp/my dir/it'\\''s $(id)' /tmp/plain-1.txt")

        sent.removeAll()
        pasteboard.clearContents()
        pasteboard.setString("echo hi", forType: .string)
        view.paste(nil)
        #expect(String(decoding: sent, as: UTF8.self) == "echo hi")

        #expect(TerminalView.shellQuoted("") == "''")
        #expect(TerminalView.shellQuoted("a/b_c.d") == "a/b_c.d")
    }

    @Test func pasteWritesThroughTheSession() throws {
        let view = try makeView()
        var sent = Data()
        view.session.terminal.onInput = { sent.append($0) }
        try view.session.paste("echo hi")
        #expect(String(decoding: sent, as: UTF8.self) == "echo hi")
    }
}
