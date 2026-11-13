import Foundation
import TermyNextCore

/// Runs the command completions' generators (`git branch`, `brew list`…) in
/// a pane's directory, off the main thread, and keeps what they print for a
/// while. A generator still running answers with what it had last time, and
/// the popup refreshes when it finishes.
@MainActor
final class GeneratorRunner {
    static let shared = GeneratorRunner()

    private struct Key: Hashable {
        var generator: Generator
        var directory: String
    }

    private var cache: [Key: (items: [SpecCompletion.Item], at: Date)] = [:]
    private var running: Set<Key> = []
    /// Most output a generator may print; `brew formulae` is around 100 KB.
    nonisolated private static let outputLimit = 2_000_000
    nonisolated private static let timeout: TimeInterval = 2
    /// How long past its lifetime a result is kept to show while it refreshes.
    private static let staleGrace: TimeInterval = 300

    func items(_ generator: Generator, word: String, directory: String, searchPath: String,
               onUpdate: @escaping () -> Void) -> [SpecCompletion.Item] {
        switch generator {
        case .files, .folders:
            return Generator.paths(for: word, in: directory, foldersOnly: generator == .folders)
        default:
            break
        }
        let key = Key(generator: generator, directory: directory)
        if let cached = cache[key], Date().timeIntervalSince(cached.at) < generator.lifetime { return cached.items }
        guard !running.contains(key) else { return cache[key]?.items ?? [] }
        let work: (@Sendable () -> [SpecCompletion.Item])?
        if let command = generator.command {
            guard let executable = Self.executable(command[0], searchPath: searchPath) else { return [] }
            let arguments = Array(command.dropFirst())
            work = { generator.items(from: Self.run(executable, arguments, directory: directory, searchPath: searchPath) ?? "") }
        } else {
            work = Self.localWork(generator, directory: directory)
        }
        guard let work else { return [] }
        running.insert(key)
        DispatchQueue.global(qos: .userInitiated).async {
            let items = work()
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    self.running.remove(key)
                    let now = Date()
                    // Results are kept per directory, so every folder completed in
                    // (with `brew formulae`'s thousands of names) stayed forever.
                    // An expired one only stands in while it refreshes; after a
                    // few idle minutes it goes.
                    self.cache = self.cache.filter { entry in
                        self.running.contains(entry.key)
                            || now.timeIntervalSince(entry.value.at) < entry.key.generator.lifetime + Self.staleGrace
                    }
                    self.cache[key] = (items, now)
                    onUpdate()
                }
            }
        }
        return cache[key]?.items ?? []
    }

    /// Sources the app reads itself rather than by running a program.
    private static func localWork(_ generator: Generator, directory: String) -> (@Sendable () -> [SpecCompletion.Item])? {
        switch generator {
        case .packageScripts: { packageJSON(from: directory).map(Generator.packageScripts) ?? [] }
        case .nodeBins: { Generator.nodeBins(from: directory) }
        case .claudeSessions: { Generator.claudeSessions(for: directory) }
        case .codexSessions: { Generator.codexSessions(for: directory) }
        case .sshHosts: {
            let ssh = (NSHomeDirectory() as NSString).appendingPathComponent(".ssh")
            let read = { (name: String) in (try? String(contentsOfFile: (ssh as NSString).appendingPathComponent(name), encoding: .utf8)) ?? "" }
            return Generator.sshHosts(config: read("config"), knownHosts: read("known_hosts"))
        }
        default: nil
        }
    }

    /// Runs a program and returns its output, or nil when it fails or takes
    /// too long.
    nonisolated private static func run(_ executable: URL, _ arguments: [String], directory: String, searchPath: String) -> String? {
        let process = Process()
        process.executableURL = executable
        process.arguments = arguments
        if FileManager.default.fileExists(atPath: directory) { process.currentDirectoryURL = URL(fileURLWithPath: directory) }
        var environment = ProcessInfo.processInfo.environment
        if !searchPath.isEmpty { environment["PATH"] = searchPath }
        environment["NO_COLOR"] = "1"
        environment["HOMEBREW_NO_AUTO_UPDATE"] = "1"
        environment["GIT_TERMINAL_PROMPT"] = "0"
        process.environment = environment
        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = FileHandle.nullDevice
        process.standardInput = FileHandle.nullDevice
        do { try process.run() } catch { return nil }
        let deadline = DispatchWorkItem { if process.isRunning { process.terminate() } }
        DispatchQueue.global().asyncAfter(deadline: .now() + timeout, execute: deadline)
        var data = Data()
        while true {
            let chunk = pipe.fileHandleForReading.availableData
            if chunk.isEmpty { break }
            if data.count < outputLimit { data.append(chunk) }
        }
        process.waitUntilExit()
        deadline.cancel()
        guard process.terminationReason == .exit, process.terminationStatus == 0 else { return nil }
        return String(decoding: data.prefix(outputLimit), as: UTF8.self)
    }

    private static func executable(_ name: String, searchPath: String) -> URL? {
        let folders = searchPath.split(separator: ":").map(String.init)
            + ["/opt/homebrew/bin", "/usr/local/bin", "\(NSHomeDirectory())/.cargo/bin", "\(NSHomeDirectory())/.bun/bin", "/usr/bin"]
        for folder in folders where !folder.isEmpty {
            let path = (folder as NSString).appendingPathComponent(name)
            if FileManager.default.isExecutableFile(atPath: path) { return URL(fileURLWithPath: path) }
        }
        return nil
    }

    /// The nearest package.json at or above `directory`.
    nonisolated private static func packageJSON(from directory: String) -> String? {
        var folder = URL(fileURLWithPath: directory)
        for _ in 0..<12 {
            if let text = try? String(contentsOf: folder.appendingPathComponent("package.json"), encoding: .utf8) { return text }
            let parent = folder.deletingLastPathComponent()
            if parent.path == folder.path { break }
            folder = parent
        }
        return nil
    }
}
