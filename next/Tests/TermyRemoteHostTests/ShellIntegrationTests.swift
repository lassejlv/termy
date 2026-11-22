import Foundation
import Termy
import TermyRemoteKit
import Testing

@testable import TermyRemoteHost

/// Protocol 5: phones see each pane's shell commands.
@MainActor struct ShellIntegrationTests {
    @Test func sessionsCarryEachPanesCommandState() throws {
        let json = """
            [{"id":"a","child_pid":1,"title":"zsh","working_directory":null,"exited":false,
              "shell":{"running":true,"commands_started":3,"commands_finished":2,"exit_code":0,"input_command":null}},
             {"id":"b","child_pid":2,"title":"zsh","working_directory":null,"exited":false,
              "shell":{"running":false,"commands_started":1,"commands_finished":1,"exit_code":127,"input_command":null}},
             {"id":"c","child_pid":3,"title":"sh","working_directory":null,"exited":false,"shell":null},
             {"id":"d","child_pid":4,"title":"old host","working_directory":null,"exited":false}]
            """
        let panes = try JSONDecoder().decode([MultiplexerPane].self, from: Data(json.utf8))
        let since = Date(timeIntervalSinceReferenceDate: 100)
        let list = RemoteHostAgent.sessions(panes: panes, layout: nil, runningSince: ["a": since, "b": since])
        let commands = Dictionary(uniqueKeysWithValues: list.panes.map { ($0.id, $0.command) })
        #expect(commands["a"] == RemoteCommandState(running: true, finished: 2, exitCode: 0, runningSince: since))
        // Only a running command has a start time.
        #expect(commands["b"] == RemoteCommandState(running: false, finished: 1, exitCode: 127, runningSince: nil))
        #expect(commands["b"]??.failed == true)
        #expect(commands["c"] == .some(nil))
        #expect(commands["d"] == .some(nil))
    }

    /// The real session host follows OSC 133, and the agent keeps the time a
    /// command started across polls.
    @Test(.timeLimit(.minutes(1))) func hostReportsALiveCommandAndItsExitCode() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("termy-shell-test-\(UUID())")
        let mux = try Multiplexer.connect(root: root, hostExecutable: #require(Multiplexer.bundledHostExecutable))
        defer {
            try? mux.shutdown()
            try? FileManager.default.removeItem(at: root)
        }
        let script =
            #"printf '\033]133;A\007\033]133;C\007'; sleep 2; printf '\033]133;D;3\007\033]133;A\007'; sleep 30"#
        let terminal = try mux.createTerminal(
            size: TerminalSize(cols: 80, rows: 24),
            options: TerminalOptions(startupCommand: "/bin/sh -c \"\(script)\""))
        let pane = try #require(terminal.paneID)
        let agent = RemoteHostAgent(multiplexer: mux, hostID: UUID(), identity: DeviceIdentity(), trustedDevices: [:])

        func command() async -> RemoteCommandState? {
            await agent.currentSessions().panes.first { $0.id == pane }?.command
        }
        var running: RemoteCommandState?
        for _ in 0..<40 where running == nil {
            if let state = await command(), state.running { running = state } else { try await Task.sleep(for: .milliseconds(50)) }
        }
        let started = try #require(running?.runningSince)
        try await Task.sleep(for: .milliseconds(100))
        #expect(await command()?.runningSince == started)

        var finished: RemoteCommandState?
        for _ in 0..<80 where finished == nil {
            if let state = await command(), !state.running, state.finished == 1 { finished = state } else {
                try await Task.sleep(for: .milliseconds(50))
            }
        }
        #expect(finished == RemoteCommandState(running: false, finished: 1, exitCode: 3, runningSince: nil))
        #expect(finished?.failed == true)
    }

    @Test func promptNavigationFindsPromptsAndOutput() {
        func mark(_ line: Int, _ kinds: PromptMark.Kind...) -> PromptMark { PromptMark(line: line, kinds: Set(kinds)) }
        // Prompt at -20 with output from -19; prompt at -5 with output from -4; the live prompt at 3.
        let marks = [mark(-20, .prompt), mark(-19, .output), mark(-5, .prompt), mark(-4, .output), mark(3, .prompt)]
        #expect(PromptNavigation.target(marks, from: 0, forward: false) == -5)
        #expect(PromptNavigation.target(marks, from: -5, forward: false) == -20)
        #expect(PromptNavigation.target(marks, from: -20, forward: false) == nil)
        #expect(PromptNavigation.target(marks, from: -20, forward: true) == -5)
        // The newest command with output is the one before the live prompt.
        #expect(PromptNavigation.output(marks, prompt: nil, lastLine: 10) == -4...2)
        #expect(PromptNavigation.output(marks, prompt: -20, lastLine: 10) == -19...(-6))
        // A prompt the phone saw a line or two away still counts; a far one doesn't.
        #expect(PromptNavigation.output(marks, prompt: -21, lastLine: 10) == -19...(-6))
        #expect(PromptNavigation.output(marks, prompt: -12, lastLine: 10) == nil)
        // The live prompt has no output yet.
        #expect(PromptNavigation.output(marks, prompt: 3, lastLine: 10) == nil)
        // Still running: its output runs to the last line.
        #expect(PromptNavigation.output([mark(0, .prompt), mark(1, .output)], prompt: nil, lastLine: 9) == 1...9)
        #expect(PromptNavigation.trimmed("one\ntwo\n  \n\n") == "one\ntwo")
    }

    /// Marks travel from the session host to an attached pane, as the phone bridge reads them.
    @Test(.timeLimit(.minutes(1))) func attachedPaneReportsPromptRowsAndOutput() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("termy-prompt-test-\(UUID())")
        let mux = try Multiplexer.connect(root: root, hostExecutable: #require(Multiplexer.bundledHostExecutable))
        defer {
            try? mux.shutdown()
            try? FileManager.default.removeItem(at: root)
        }
        let script = #"printf '\033]133;A\007$ ls\n\033]133;C\007one\ntwo\n\033]133;D;0\007\033]133;A\007$ '; sleep 30"#
        let created = try mux.createTerminal(
            size: TerminalSize(cols: 40, rows: 10), options: TerminalOptions(startupCommand: "/bin/sh -c \"\(script)\""))
        let pane = try #require(created.paneID)
        let terminal = try mux.attachTerminal(paneID: pane)
        var rows: [Int]?
        for _ in 0..<60 {
            rows = PromptNavigation.promptRows(in: terminal, displayOffset: 0, rows: 10)
            if rows == [0, 3] { break }
            try await Task.sleep(for: .milliseconds(50))
        }
        #expect(rows == [0, 3])
        let output = PromptNavigation.output(in: terminal, promptLine: nil)
        #expect(output?.text == "one\ntwo")
        #expect(output?.truncated == false)
        #expect(PromptNavigation.output(in: terminal, promptLine: 0)?.text == "one\ntwo")
        #expect(PromptNavigation.jumpOffset(in: terminal, displayOffset: 0, forward: true) == 0)
    }

    @Test func uploadsAreSavedPrivatelyWithSafeNames() throws {
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("termy-upload-test-\(UUID())")
        defer { try? FileManager.default.removeItem(at: folder) }
        let jpeg = Data([0xff, 0xd8, 0xff, 0xe0, 1, 2, 3])
        let now = Date()
        let path = try Uploads.save(jpeg, name: "../IMG 1.HEIC", in: folder, now: now)
        #expect(URL(fileURLWithPath: path).deletingLastPathComponent().standardizedFileURL == folder.standardizedFileURL)
        #expect(path.hasSuffix("-IMG-1.jpg"))
        #expect(FileManager.default.contents(atPath: path) == jpeg)
        let mode = try FileManager.default.attributesOfItem(atPath: path)[.posixPermissions] as? Int
        #expect(mode == 0o600)
        let second = try Uploads.save(jpeg, name: "../IMG 1.HEIC", in: folder, now: now)
        #expect(second != path)
        // A week later, both are gone when the next one arrives.
        let later = try Uploads.save(jpeg, name: "new.jpg", in: folder, now: now.addingTimeInterval(8 * 24 * 3600))
        #expect(!FileManager.default.fileExists(atPath: path) && !FileManager.default.fileExists(atPath: second))
        #expect(FileManager.default.fileExists(atPath: later))
        #expect(throws: (any Error).self) { try Uploads.save(Data(), name: "empty", in: folder) }
    }
}
