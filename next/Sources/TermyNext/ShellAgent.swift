import Foundation
import TermyNextCore

/// A local coding agent the ⇧⌘E command box asks for a command: Claude Code
/// (`claude -p`) or Codex (`codex exec`), using whatever sign-in they already
/// have. Termy Next holds no API keys of its own.
struct ShellAgent: Hashable {
    enum Kind: String { case claude, codex }

    let kind: Kind
    /// Claude Code's model alias; Codex uses the model from its own config.
    let model: String?
    let title: String

    var id: String { model.map { "\(kind.rawValue):\($0)" } ?? kind.rawValue }

    static let all: [ShellAgent] = [
        ShellAgent(kind: .claude, model: "haiku", title: "Claude Code · Haiku"),
        ShellAgent(kind: .claude, model: "sonnet", title: "Claude Code · Sonnet"),
        ShellAgent(kind: .claude, model: "opus", title: "Claude Code · Opus"),
        ShellAgent(kind: .codex, model: nil, title: "Codex"),
    ]

    static func with(id: String) -> ShellAgent? { all.first { $0.id == id } }

    /// The CLI on the shell's PATH, then in the usual install folders.
    func executable(searchPath: String) -> URL? {
        let home = NSHomeDirectory()
        let folders = searchPath.split(separator: ":").map(String.init) + [
            "\(home)/.local/bin", "\(home)/.claude/local", "/opt/homebrew/bin", "/usr/local/bin",
            "\(home)/.bun/bin", "\(home)/.npm-global/bin", "\(home)/.volta/bin",
        ]
        for folder in folders where !folder.isEmpty {
            let path = (folder as NSString).appendingPathComponent(kind.rawValue)
            if FileManager.default.isExecutableFile(atPath: path) { return URL(fileURLWithPath: path) }
        }
        return nil
    }

    /// Asks for one command. Low effort, no tools, no MCP servers: the answer
    /// only has to be a line of shell.
    /// Claude Code takes the rules as its whole system prompt, which also
    /// skips its own long one; Codex gets them at the top of the prompt.
    func arguments(request: String, directory: String?, refining command: String?, output: URL) -> [String] {
        let task = Self.task(request: request, directory: directory, refining: command)
        switch kind {
        case .claude:
            return ["-p", "--model", model ?? "haiku", "--effort", "low", "--tools", "",
                    "--strict-mcp-config", "--setting-sources", "", "--no-session-persistence",
                    "--system-prompt", Self.rules, task]
        case .codex:
            return ["exec", "--skip-git-repo-check", "--ephemeral", "-s", "read-only",
                    "-c", "model_reasoning_effort=\"low\"", "-o", output.path, Self.rules + "\n\n" + task]
        }
    }

    static let rules = """
        You turn a request into one shell command for zsh on macOS. Reply with the command and nothing \
        else: no reasoning, no explanation, no Markdown, no code fences. Prefer a single line.
        """

    static func task(request: String, directory: String?, refining command: String?) -> String {
        var text = ""
        if let directory { text += "Working directory: \(directory)\n" }
        if let command { text += "Change this command: \(command)\n" }
        return text + "Request: \(request)"
    }
}

struct ShellAgentError: Error {
    let message: String
}

/// One run of an agent CLI. Output goes to temporary files, so a chatty
/// agent can't fill a pipe and stall.
@MainActor
final class ShellAgentRun {
    private let process = Process()
    private let folder: URL
    private var timeout: DispatchWorkItem?
    private var completion: ((Result<String, ShellAgentError>) -> Void)?

    init(agent: ShellAgent, request: String, refining command: String?, directory: String?, searchPath: String,
         completion: @escaping (Result<String, ShellAgentError>) -> Void) {
        folder = FileManager.default.temporaryDirectory.appendingPathComponent("termy-next-ask-\(UUID().uuidString)")
        self.completion = completion
        guard let executable = agent.executable(searchPath: searchPath) else {
            let name = agent.kind == .claude ? "Claude Code (claude)" : "Codex (codex)"
            finish(.failure(ShellAgentError(message: "\(name) isn't installed")))
            return
        }
        do {
            try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        } catch {
            finish(.failure(ShellAgentError(message: error.localizedDescription)))
            return
        }
        let stdout = folder.appendingPathComponent("stdout")
        let stderr = folder.appendingPathComponent("stderr")
        let reply = folder.appendingPathComponent("reply")
        FileManager.default.createFile(atPath: stdout.path, contents: nil)
        FileManager.default.createFile(atPath: stderr.path, contents: nil)

        var environment = ProcessInfo.processInfo.environment
        if !searchPath.isEmpty { environment["PATH"] = searchPath }
        // Started from inside a Claude Code session (say, `swift run` in one),
        // claude would think it's nested.
        environment["CLAUDECODE"] = nil
        environment["CLAUDE_CODE_ENTRYPOINT"] = nil
        environment["NO_COLOR"] = "1"

        process.executableURL = executable
        process.arguments = agent.arguments(request: request, directory: directory, refining: command, output: reply)
        process.environment = environment
        if let directory, FileManager.default.fileExists(atPath: directory) {
            process.currentDirectoryURL = URL(fileURLWithPath: directory)
        }
        process.standardInput = FileHandle.nullDevice
        let outHandle = try? FileHandle(forWritingTo: stdout)
        let errHandle = try? FileHandle(forWritingTo: stderr)
        process.standardOutput = outHandle
        process.standardError = errHandle
        // The agent has its own copies once started; ours would stay open as
        // long as the run object lives.
        defer {
            try? outHandle?.close()
            try? errHandle?.close()
        }
        process.terminationHandler = { [weak self] process in
            let status = process.terminationStatus
            let reason = process.terminationReason
            DispatchQueue.main.async {
                MainActor.assumeIsolated { self?.exited(status: status, reason: reason, agent: agent, stdout: stdout, stderr: stderr, reply: reply) }
            }
        }
        do {
            try process.run()
        } catch {
            finish(.failure(ShellAgentError(message: "Can't start \(executable.lastPathComponent): \(error.localizedDescription)")))
            return
        }
        let timeout = DispatchWorkItem { [weak self] in
            MainActor.assumeIsolated {
                guard let self, self.process.isRunning else { return }
                Self.end(self.process)
                self.finish(.failure(ShellAgentError(message: "\(agent.title) took too long")))
            }
        }
        self.timeout = timeout
        DispatchQueue.main.asyncAfter(deadline: .now() + 120, execute: timeout)
    }

    /// Stops the agent; the completion isn't called.
    func cancel() {
        completion = nil
        if process.isRunning { Self.end(process) }
        cleanUp()
    }

    /// Agents told to stop that haven't exited yet. Holding them until they
    /// do keeps Foundation reaping them after the run that started them is gone.
    private static var ending: Set<Process> = []

    /// Asks the agent to quit, kills it if it's still there a few seconds
    /// later, and keeps it until it has exited and been reaped.
    private static func end(_ process: Process) {
        ending.insert(process)
        process.terminationHandler = { process in
            DispatchQueue.main.async { MainActor.assumeIsolated { _ = ShellAgentRun.ending.remove(process) } }
        }
        process.terminate()
        let pid = process.processIdentifier
        DispatchQueue.main.asyncAfter(deadline: .now() + 3) {
            MainActor.assumeIsolated {
                if ShellAgentRun.ending.contains(process), process.isRunning { kill(pid, SIGKILL) }
            }
        }
        // It may have exited before the handler was replaced.
        if !process.isRunning { ending.remove(process) }
    }

    private func exited(status: Int32, reason: Process.TerminationReason, agent: ShellAgent, stdout: URL, stderr: URL, reply: URL) {
        guard completion != nil else { return }
        let read = { (url: URL) in (try? String(contentsOf: url, encoding: .utf8)) ?? "" }
        let output = agent.kind == .codex ? read(reply) : read(stdout)
        if status == 0, reason == .exit, let command = ShellProtocol.command(fromAgentReply: output) {
            finish(.success(command))
        } else {
            let detail = [read(stderr), read(stdout)]
                .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
                .first { !$0.isEmpty }?
                .components(separatedBy: "\n").last ?? "exited with status \(status)"
            finish(.failure(ShellAgentError(message: "\(agent.title): \(detail)")))
        }
    }

    private func finish(_ result: Result<String, ShellAgentError>) {
        let completion = self.completion
        self.completion = nil
        cleanUp()
        // Always later, so callers can keep the run before it completes.
        DispatchQueue.main.async { completion?(result) }
    }

    private func cleanUp() {
        timeout?.cancel()
        timeout = nil
        try? FileManager.default.removeItem(at: folder)
    }
}
