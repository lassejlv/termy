import Foundation
import Testing
@testable import Termy

@Suite("Sessions")
@MainActor
struct SessionTests {
    @Test func displaySessionTracksStateAndFrames() throws {
        let session = try TerminalSession.display(size: TerminalSize(cols: 40, rows: 10))
        var changes: [FrameChange] = []
        session.onFrameChange = { changes.append($0) }

        try session.feed(Data("\u{1b}]2;Logs\u{07}\u{1b}]9;4;1;10\u{07}tail -f\r\n".utf8))
        #expect(session.title == "Logs")
        #expect(session.progress == .inProgress(10))
        #expect(session.frame.text(row: 0) == "tail -f")
        #expect(changes.count == 1)

        var replies = Data()
        session.onProtocolReply = { replies.append($0) }
        try session.feed(Data("\u{1b}[6n".utf8))
        #expect(String(decoding: replies, as: UTF8.self) == "\u{1b}[2;1R")
    }

    @Test func resizeForcesAFullFrame() throws {
        let session = try TerminalSession.display(size: TerminalSize(cols: 40, rows: 10))
        try session.feed(Data("x".utf8))
        var lastChange: FrameChange?
        session.onFrameChange = { lastChange = $0 }
        try session.resize(TerminalSize(cols: 60, rows: 12))
        #expect(lastChange?.damage == .full)
        #expect(session.frame.cols == 60)
    }

    @Test func contentGenerationFollowsContentChangesOnly() throws {
        let session = try TerminalSession.display(size: TerminalSize(cols: 40, rows: 10))
        try session.feed(Data("hello".utf8))
        let afterText = session.contentGeneration
        #expect(afterText > 0)
        // Nothing new: polling again changes nothing.
        session.poll()
        #expect(session.contentGeneration == afterText)
        // A cursor move alone is not content.
        try session.feed(Data("\u{1b}[3;3H".utf8))
        #expect(session.frame.cursor.row == 2)
        #expect(session.contentGeneration == afterText)
        try session.feed(Data("x".utf8))
        #expect(session.contentGeneration == afterText + 1)
        try session.resize(TerminalSize(cols: 50, rows: 10))
        #expect(session.contentGeneration == afterText + 2)
    }

    @Test func wakeupsRightAfterAPollWaitForTheMinimumInterval() async throws {
        let session = try TerminalSession.display(size: TerminalSize(cols: 40, rows: 10))
        defer { session.stop() }
        session.minimumPollInterval = 0.3
        session.poll()
        // Output that arrives at once, as from a PTY read, waits for the interval.
        try session.terminal.feed(Data("later".utf8))
        session.schedulePoll()
        try await Task.sleep(for: .milliseconds(100))
        #expect(session.frame.text(row: 0) == "")
        let deadline = Date().addingTimeInterval(5)
        while session.frame.text(row: 0) != "later", Date() < deadline {
            try await Task.sleep(for: .milliseconds(20))
        }
        #expect(session.frame.text(row: 0) == "later")
    }

    @Test func childPIDIsTheProgramOfTheTerminalsOwnPTY() throws {
        let display = try Terminal.display(size: TerminalSize(cols: 10, rows: 2))
        #expect(display.childPID == nil)
        let terminal = try Terminal(
            size: TerminalSize(cols: 20, rows: 4), options: TerminalOptions(startupCommand: "sleep 30"))
        let pid = try #require(terminal.childPID)
        #expect(getpgid(pid) >= 0)
        // The app's own child, not some other process.
        var info = kinfo_proc()
        var size = MemoryLayout<kinfo_proc>.stride
        var mib: [Int32] = [CTL_KERN, KERN_PROC, KERN_PROC_PID, pid]
        #expect(sysctl(&mib, 4, &info, &size, nil, 0) == 0)
        #expect(info.kp_eproc.e_ppid == getpid())
    }

    @Test(.timeLimit(.minutes(1)))
    func searchesRunOffTheMainActorWhileTheSessionIsFedAndPolled() async throws {
        let session = try TerminalSession.display(size: TerminalSize(cols: 40, rows: 10))
        defer { session.stop() }
        try session.terminal.setScrollbackHistory(5_000)
        try session.feed(Data((0..<3000).map { "needle \($0)\r\n" }.joined().utf8))
        let searches = Task { () -> [Int] in
            var counts: [Int] = []
            for _ in 0..<10 { counts.append(try await session.search("needle").count) }
            return counts
        }
        // The main actor keeps feeding and polling while searches run.
        for line in 0..<500 {
            try session.feed(Data("other \(line)\r\n".utf8))
            if line % 50 == 0 { await Task.yield() }
        }
        let counts = try await searches.value
        #expect(counts.allSatisfy { $0 > 0 && $0 <= 3000 })
        let last = try await session.search("other 499")
        #expect(last.count == 1)
        #expect(last.first?.line == "other 499")
    }

    /// End to end: a real PTY child process, the wakeup thread, and main-actor polling.
    @Test(.timeLimit(.minutes(1)))
    func ptySessionRunsACommand() async throws {
        let session = try TerminalSession(
            size: TerminalSize(cols: 60, rows: 10),
            options: TerminalOptions(
                startupCommand: "/bin/sh -c 'printf \"pty:%s\" \"$TERMY_SDK_TEST\"; sleep 0.2'",
                environment: ["TERMY_SDK_TEST": "ok"]
            )
        )
        defer { session.stop() }

        let deadline = Date().addingTimeInterval(10)
        while !session.frame.text(row: 0).contains("pty:ok"), Date() < deadline {
            try await Task.sleep(for: .milliseconds(20))
        }
        #expect(session.frame.text(row: 0).contains("pty:ok"))
    }
}
