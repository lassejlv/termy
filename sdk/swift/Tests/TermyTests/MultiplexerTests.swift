import Foundation
import Testing
@testable import Termy

@Suite("Multiplexer", .serialized)
@MainActor
struct MultiplexerTests {
    /// A host in a private directory, so tests never touch the user's sessions.
    private func connect() throws -> (Multiplexer, URL) {
        let host = try #require(Multiplexer.bundledHostExecutable, "run scripts/build-libtermy.sh to stage termy-session-host")
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("termy-mux-\(UUID().uuidString.prefix(8))")
        return (try Multiplexer.connect(root: root, hostExecutable: host), root)
    }

    private func waitFor(_ text: String, in session: TerminalSession) async throws -> Bool {
        let deadline = Date().addingTimeInterval(10)
        while Date() < deadline {
            if (0..<session.frame.rows).contains(where: { session.frame.text(row: $0).contains(text) }) { return true }
            try await Task.sleep(for: .milliseconds(20))
        }
        return false
    }

    private func waitFor(_ condition: () -> Bool) async throws -> Bool {
        let deadline = Date().addingTimeInterval(10)
        while Date() < deadline {
            if condition() { return true }
            try await Task.sleep(for: .milliseconds(20))
        }
        return false
    }

    /// Two views of one pane, such as the Mac and a phone: the one the user
    /// works in takes the grid back after the other resized it, and asking
    /// for the size it already has does nothing.
    @Test(.timeLimit(.minutes(1)))
    func viewsTakeTheGridBackFromEachOther() async throws {
        let (mux, root) = try connect()
        defer {
            try? mux.shutdown()
            try? FileManager.default.removeItem(at: root)
        }
        let wide = TerminalSize(cols: 120, rows: 30)
        let narrow = TerminalSize(cols: 50, rows: 40)
        let terminal = try mux.createTerminal(size: wide, options: TerminalOptions(startupCommand: "/bin/sh -c 'sleep 30'"))
        let mac = TerminalSession(terminal: terminal, size: wide)
        let phone = TerminalSession(terminal: try mux.attachTerminal(paneID: try #require(terminal.paneID)), size: wide)
        #expect(try await waitFor { mac.frame.cols == 120 && phone.frame.cols == 120 })
        #expect(!mac.isGridSizeTakenOver)

        try phone.resize(narrow)
        #expect(try await waitFor { mac.frame.cols == 50 && mac.frame.rows == 40 })
        #expect(mac.isGridSizeTakenOver)
        // Plain `resize` keeps the size it already asked for.
        try mac.resize(wide)
        try await Task.sleep(for: .milliseconds(200))
        #expect(mac.frame.cols == 50)

        try mac.reclaim(wide)
        #expect(try await waitFor { mac.frame.cols == 120 && mac.frame.rows == 30 && phone.frame.cols == 120 })
        #expect(!mac.isGridSizeTakenOver)
        #expect(phone.isGridSizeTakenOver)
        // Nothing to take back: no new request, no resize.
        try mac.reclaim(wide)
        #expect(!mac.isGridSizeTakenOver)

        try phone.reclaim(narrow)
        #expect(try await waitFor { mac.frame.cols == 50 })
        mac.stop()
        phone.stop()
    }

    /// Shutting the host down ends its panes, and a new host starts on the
    /// same root straight away, as a restart from the app does.
    @Test(.timeLimit(.minutes(1)))
    func restartsOnTheSameRoot() async throws {
        let (mux, root) = try connect()
        defer { try? FileManager.default.removeItem(at: root) }
        let host = try #require(Multiplexer.bundledHostExecutable)
        let size = TerminalSize(cols: 40, rows: 5)
        let first = try mux.createTerminal(size: size, options: TerminalOptions(startupCommand: "/bin/sh -c 'sleep 30'"))
        let firstID = try #require(first.paneID)
        try mux.shutdown()

        var restarted: Multiplexer?
        for _ in 0..<20 where restarted == nil {
            restarted = try? Multiplexer.connect(root: root, hostExecutable: host)
            if restarted == nil { try await Task.sleep(for: .milliseconds(100)) }
        }
        let fresh = try #require(restarted)
        defer { try? fresh.shutdown() }
        #expect(try !fresh.panes().contains { $0.id == firstID && !$0.exited })
        let second = try fresh.createTerminal(size: size, options: TerminalOptions(startupCommand: "/bin/sh -c 'sleep 30'"))
        #expect(try fresh.panes().contains { $0.id == second.paneID && !$0.exited })
    }

    /// The shared layout: a write lands at the next revision, a stale write
    /// is refused, and a watch reports each revision.
    @Test(.timeLimit(.minutes(1)))
    func sharesTheLayout() async throws {
        let (mux, root) = try connect()
        defer {
            try? mux.shutdown()
            try? FileManager.default.removeItem(at: root)
        }
        let size = TerminalSize(cols: 40, rows: 5)
        let a = try #require(try mux.createTerminal(size: size, options: TerminalOptions(startupCommand: "/bin/sh -c 'sleep 30'")).paneID)
        let b = try #require(try mux.createTerminal(size: size, options: TerminalOptions(startupCommand: "/bin/sh -c 'sleep 30'")).paneID)

        let revisions = RevisionLog()
        let watch = try mux.watchLayout { revisions.append($0) }
        var layout = try mux.layout()
        layout.windows = [.init(id: "w1", workspaces: [.init(name: "main", tabs: [
            .init(title: "build", activePane: b, zoom: [b: 2], cols: 100, rows: 30,
                  tree: .split(axis: .horizontal, ratio: 0.5, first: .leaf(a), second: .leaf(b))),
        ])])]
        let first = try mux.setLayout(layout)
        #expect(first.updated)
        #expect(first.revision > layout.revision)

        let read = try mux.layout()
        #expect(read.revision == first.revision)
        let tab = try #require(read.windows.first?.workspaces.first?.tabs.first)
        #expect(tab.tree.paneIDs == [a, b])
        #expect(tab.title == "build")
        #expect(tab.activePane == b)
        #expect(tab.zoom[b] == 2)

        // Written against an old revision: refused, and told the new one.
        let stale = try mux.setLayout(layout)
        #expect(!stale.updated)
        #expect(stale.revision == first.revision)

        let deadline = Date().addingTimeInterval(5)
        while !revisions.values.contains(first.revision), Date() < deadline { try await Task.sleep(for: .milliseconds(20)) }
        #expect(revisions.values.contains(first.revision))
        _ = watch
    }

    /// End to end: the host runs the program, a second view attaches to it,
    /// detaching leaves it running and closing ends it.
    @Test(.timeLimit(.minutes(1)))
    func panesOutliveTheirViews() async throws {
        let (mux, root) = try connect()
        defer {
            try? mux.shutdown()
            try? FileManager.default.removeItem(at: root)
        }
        let size = TerminalSize(cols: 60, rows: 10)

        let terminal = try mux.createTerminal(
            size: size,
            options: TerminalOptions(startupCommand: "/bin/sh -c 'printf \"mux:%s\" \"$TERMY_SDK_TEST\"; sleep 30'",
                                     environment: ["TERMY_SDK_TEST": "ok"]))
        let paneID = try #require(terminal.paneID)
        var session: TerminalSession? = TerminalSession(terminal: terminal, size: size)
        #expect(try await waitFor("mux:ok", in: session!))
        #expect(try mux.panes().contains { $0.id == paneID && !$0.exited && $0.childPID != nil })

        // Releasing the view detaches; the program keeps running.
        session?.stop()
        session = nil
        #expect(try mux.panes().contains { $0.id == paneID && !$0.exited })

        let reattached = TerminalSession(terminal: try mux.attachTerminal(paneID: paneID), size: size)
        defer { reattached.stop() }
        #expect(try await waitFor("mux:ok", in: reattached))

        try mux.close(paneID: paneID)
        #expect(try !mux.panes().contains { $0.id == paneID && !$0.exited })
        #expect(throws: TermyError.self) { try mux.attachTerminal(paneID: "no-such-pane") }
    }
}

final class RevisionLog: @unchecked Sendable {
    private let lock = NSLock()
    private var stored: [UInt64?] = []
    func append(_ revision: UInt64?) { lock.withLock { stored.append(revision) } }
    var values: [UInt64?] { lock.withLock { stored } }
}
