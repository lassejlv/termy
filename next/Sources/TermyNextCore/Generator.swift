import Foundation

/// Where an argument's suggestions come from when they depend on the
/// machine or the project: a program the app runs in the pane's directory,
/// or files the app lists itself.
public enum Generator: String, Sendable, CaseIterable {
    case gitBranches = "branch", gitRemoteBranches = "remote branch", gitRemotes = "remote", gitTags = "tag"
    case gitStashes = "stash", gitCommits = "commit", gitChangedFiles = "changed file", gitStagedFiles = "staged file"
    case brewInstalled = "installed formula", brewFormulae = "formula", brewCasks = "cask", brewOutdated = "outdated formula"
    case brewServices = "service", brewTaps = "tap"
    case cargoPackages = "package", cargoBins = "binary", cargoExamples = "example"
    case rustupToolchains = "toolchain", rustupInstalledTargets = "installed target", rustupTargets = "target"
    case rustupComponents = "component"
    case packageScripts = "script"
    case justRecipes = "recipe"
    case nodeBins = "tool"
    case sshHosts = "host"
    case claudeSessions = "session", codexSessions = "codex session"
    case files = "path", folders = "folder"

    /// The program to run, or nil for the ones the app reads itself (files,
    /// folders, package.json scripts).
    public var command: [String]? {
        switch self {
        case .gitBranches: ["git", "--no-optional-locks", "branch", "--no-color", "--sort=-committerdate", "--format=%(refname:short)"]
        case .gitRemoteBranches: ["git", "--no-optional-locks", "branch", "-r", "--no-color", "--format=%(refname:short)"]
        case .gitRemotes: ["git", "--no-optional-locks", "remote"]
        case .gitTags: ["git", "--no-optional-locks", "tag", "--list", "--sort=-creatordate"]
        case .gitStashes: ["git", "--no-optional-locks", "stash", "list"]
        case .gitCommits: ["git", "--no-optional-locks", "log", "--oneline", "--no-color", "-n", "50"]
        case .gitChangedFiles: ["git", "--no-optional-locks", "status", "--porcelain"]
        case .gitStagedFiles: ["git", "--no-optional-locks", "diff", "--cached", "--name-only"]
        case .brewInstalled: ["brew", "list", "-1"]
        case .brewFormulae: ["brew", "formulae"]
        case .brewCasks: ["brew", "casks"]
        case .brewOutdated: ["brew", "outdated", "-q"]
        case .brewServices: ["brew", "services", "list"]
        case .brewTaps: ["brew", "tap"]
        case .cargoPackages, .cargoBins, .cargoExamples: ["cargo", "metadata", "--format-version", "1", "--no-deps", "--offline"]
        case .rustupToolchains: ["rustup", "toolchain", "list"]
        case .rustupInstalledTargets: ["rustup", "target", "list", "--installed"]
        case .rustupTargets: ["rustup", "target", "list"]
        case .rustupComponents: ["rustup", "component", "list"]
        case .justRecipes: ["just", "--list", "--unsorted"]
        case .packageScripts, .nodeBins, .sshHosts, .claudeSessions, .codexSessions, .files, .folders: nil
        }
    }

    /// How long a result stays good: lists that rarely change (every
    /// formula, every target) are kept longer than a project's branches.
    public var lifetime: TimeInterval {
        switch self {
        case .brewFormulae, .brewCasks, .rustupTargets, .rustupComponents, .brewTaps: 600
        case .brewInstalled, .brewOutdated, .rustupToolchains, .rustupInstalledTargets, .sshHosts: 60
        case .claudeSessions, .codexSessions, .justRecipes, .nodeBins: 10
        default: 5
        }
    }

    /// Reads the program's output (or package.json, for `packageScripts`).
    public func items(from output: String) -> [SpecCompletion.Item] {
        let lines = output.split(whereSeparator: \.isNewline).map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
        func item(_ name: some StringProtocol, _ description: String? = nil) -> SpecCompletion.Item {
            .init(name: String(name), description: description, kind: .argument)
        }
        func firstWord(_ line: String) -> String { String(line.split(whereSeparator: \.isWhitespace).first ?? "") }
        var items: [SpecCompletion.Item]
        switch self {
        case .gitRemoteBranches:
            // origin/HEAD shows as just "origin".
            items = lines.filter { $0.contains("/") }.map { item($0) }
        case .gitStashes:
            items = lines.map { line in
                let parts = line.split(separator: ":", maxSplits: 1)
                return item(parts[0], parts.count > 1 ? parts[1].trimmingCharacters(in: .whitespaces) : nil)
            }
        case .gitCommits:
            items = lines.map { line in
                let parts = line.split(separator: " ", maxSplits: 1)
                return item(parts[0], parts.count > 1 ? String(parts[1]) : nil)
            }
        case .gitChangedFiles:
            // "XY path", or "R  old -> new".
            items = output.split(whereSeparator: \.isNewline).compactMap { raw in
                guard raw.count > 3 else { return nil }
                let status = raw.prefix(2).trimmingCharacters(in: .whitespaces)
                let path = String(raw.dropFirst(3)).components(separatedBy: " -> ").last ?? ""
                return path.isEmpty ? nil : item(path, Self.statusName(status))
            }
        case .brewInstalled, .brewFormulae, .brewCasks, .brewOutdated, .brewTaps, .gitBranches, .gitRemotes, .gitTags, .gitStagedFiles:
            items = lines.filter { !$0.hasPrefix("==>") }.map { item(firstWord($0)) }
        case .brewServices:
            items = lines.dropFirst().map { line in
                let fields = line.split(whereSeparator: \.isWhitespace)
                return item(fields[0], fields.count > 1 ? String(fields[1]) : nil)
            }
        case .rustupToolchains:
            items = lines.map { item(firstWord($0), $0.contains("(default)") ? "default" : nil) }
        case .rustupTargets, .rustupComponents:
            items = lines.map { item(firstWord($0), $0.contains("(installed)") ? "installed" : nil) }
        case .rustupInstalledTargets:
            items = lines.map { item(firstWord($0)) }
        case .cargoPackages, .cargoBins, .cargoExamples:
            items = cargoItems(output)
        case .packageScripts:
            items = Self.packageScripts(output)
        case .justRecipes:
            // "Available recipes:" then "    build target # Build it".
            items = output.split(whereSeparator: \.isNewline).dropFirst().compactMap { raw in
                let line = raw.trimmingCharacters(in: .whitespaces)
                guard !line.isEmpty, !line.hasSuffix(":") else { return nil }
                let parts = line.components(separatedBy: " # ")
                return item(firstWord(parts[0]), parts.count > 1 ? parts[1].trimmingCharacters(in: .whitespaces) : nil)
            }
        case .nodeBins, .sshHosts, .claudeSessions, .codexSessions, .files, .folders:
            items = []
        }
        var seen: Set<String> = []
        return items.filter { !$0.name.isEmpty && seen.insert($0.name).inserted }
    }

    /// Items from this generator that complete `word`.
    public static func matching(_ items: [SpecCompletion.Item], word: String) -> [SpecCompletion.Item] {
        items.filter { $0.name.hasPrefix(word) && $0.name != word }
    }

    private func cargoItems(_ output: String) -> [SpecCompletion.Item] {
        guard let data = output.data(using: .utf8),
              let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let packages = json["packages"] as? [[String: Any]] else { return [] }
        if self == .cargoPackages {
            return packages.compactMap { package in
                (package["name"] as? String).map { .init(name: $0, description: package["version"] as? String, kind: .argument) }
            }
        }
        let wanted = self == .cargoBins ? "bin" : "example"
        return packages.flatMap { package in
            ((package["targets"] as? [[String: Any]]) ?? []).compactMap { target -> SpecCompletion.Item? in
                guard let name = target["name"] as? String, (target["kind"] as? [String])?.contains(wanted) == true else { return nil }
                return .init(name: name, description: package["name"] as? String, kind: .argument)
            }
        }
    }

    /// `scripts` in a package.json, with the command each runs.
    public static func packageScripts(_ json: String) -> [SpecCompletion.Item] {
        guard let data = json.data(using: .utf8),
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let scripts = object["scripts"] as? [String: Any] else { return [] }
        return scripts.keys.sorted().map { .init(name: $0, description: scripts[$0] as? String, kind: .argument) }
    }

    /// Entries of the folder `word` points into, relative to `directory`:
    /// "src/ma" lists src/ and keeps names starting with "ma". Folders end
    /// in "/" and take no space after.
    public static func paths(for word: String, in directory: String, foldersOnly: Bool) -> [SpecCompletion.Item] {
        let expanded = (word as NSString).expandingTildeInPath
        let slash = word.lastIndex(of: "/")
        let typedFolder = slash.map { String(word[...$0]) } ?? ""
        let prefix = slash.map { String(word[word.index(after: $0)...]) } ?? word
        let folderPath: String
        if typedFolder.isEmpty {
            folderPath = directory
        } else {
            let base = (expanded as NSString).deletingLastPathComponent
            let absolute = expanded.hasPrefix("/") ? (expanded.hasSuffix("/") ? expanded : base) : (directory as NSString).appendingPathComponent(expanded.hasSuffix("/") ? expanded : base)
            folderPath = absolute
        }
        guard let names = try? FileManager.default.contentsOfDirectory(atPath: folderPath) else { return [] }
        return names.sorted { $0.localizedStandardCompare($1) == .orderedAscending }.compactMap { name in
            guard name.hasPrefix(prefix), prefix.hasPrefix(".") || !name.hasPrefix(".") else { return nil }
            var isFolder: ObjCBool = false
            FileManager.default.fileExists(atPath: (folderPath as NSString).appendingPathComponent(name), isDirectory: &isFolder)
            if foldersOnly, !isFolder.boolValue { return nil }
            let full = typedFolder + name + (isFolder.boolValue ? "/" : "")
            guard full != word else { return nil }
            return .init(name: full, kind: .argument, appendsSpace: !isFolder.boolValue)
        }
    }

    /// Hosts from an ssh config (`Host` entries without wildcards, with their
    /// HostName) and then known_hosts (unhashed names).
    public static func sshHosts(config: String, knownHosts: String) -> [SpecCompletion.Item] {
        var items: [SpecCompletion.Item] = []
        var seen: Set<String> = []
        var current: [Int] = []
        for raw in config.split(whereSeparator: \.isNewline) {
            let fields = raw.trimmingCharacters(in: .whitespaces).split(whereSeparator: { $0 == " " || $0 == "\t" || $0 == "=" })
            guard let key = fields.first?.lowercased(), fields.count > 1 else { continue }
            if key == "host" {
                current = []
                for host in fields.dropFirst().map(String.init) where !host.contains(where: { "*?!".contains($0) }) {
                    if seen.insert(host).inserted {
                        current.append(items.count)
                        items.append(.init(name: host, kind: .argument))
                    }
                }
            } else if key == "hostname" {
                for index in current { items[index].description = String(fields[1]) }
            } else if key == "match" {
                current = []
            }
        }
        for raw in knownHosts.split(whereSeparator: \.isNewline) {
            guard let first = raw.split(separator: " ").first, !first.hasPrefix("|"), !first.hasPrefix("#"), !first.hasPrefix("@") else { continue }
            for name in first.split(separator: ",") {
                var host = String(name)
                // "[host]:2222"
                if host.hasPrefix("["), let close = host.firstIndex(of: "]") { host = String(host[host.index(after: host.startIndex)..<close]) }
                if !host.isEmpty, seen.insert(host).inserted { items.append(.init(name: host, description: "known host", kind: .argument)) }
            }
        }
        return items
    }

    /// The tools in the nearest node_modules/.bin, for `npx` and `bunx`.
    public static func nodeBins(from directory: String) -> [SpecCompletion.Item] {
        var folder = URL(fileURLWithPath: directory)
        for _ in 0..<12 {
            let bin = folder.appendingPathComponent("node_modules/.bin").path
            if let names = try? FileManager.default.contentsOfDirectory(atPath: bin) {
                return names.filter { !$0.hasPrefix(".") }.sorted().map { .init(name: $0, kind: .argument) }
            }
            let parent = folder.deletingLastPathComponent()
            if parent.path == folder.path { break }
            folder = parent
        }
        return []
    }

    /// Claude Code's sessions for `directory`, newest first, each with its
    /// first message: ~/.claude/projects/<folder>/<id>.jsonl, where <folder>
    /// is the directory with every other character than a letter or digit
    /// turned into "-".
    public static func claudeSessions(for directory: String, home: String = NSHomeDirectory(), limit: Int = 12) -> [SpecCompletion.Item] {
        let mangled = String(directory.map { $0.isLetter || $0.isNumber ? $0 : "-" })
        let folder = URL(fileURLWithPath: home).appendingPathComponent(".claude/projects/\(mangled)")
        return newestFiles(in: [folder], limit: limit) { $0.pathExtension == "jsonl" }.map { file in
            let id = file.deletingPathExtension().lastPathComponent
            let text = prefix(of: file)
            var message: String?
            for line in text.split(whereSeparator: \.isNewline) {
                guard line.contains("\"type\":\"user\""), let data = line.data(using: .utf8),
                      let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                      let content = (object["message"] as? [String: Any])?["content"] else { continue }
                if let string = content as? String { message = string }
                if let parts = content as? [[String: Any]] { message = parts.compactMap { $0["text"] as? String }.first }
                if let found = message, !found.hasPrefix("<") { break } else { message = nil }
            }
            return .init(name: id, description: message.map(summary), kind: .argument)
        }
    }

    /// Codex's sessions, newest first, those started in `directory` before
    /// the rest: ~/.codex/sessions/YYYY/MM/DD/rollout-…-<id>.jsonl.
    public static func codexSessions(for directory: String, home: String = NSHomeDirectory(), limit: Int = 12) -> [SpecCompletion.Item] {
        let root = URL(fileURLWithPath: home).appendingPathComponent(".codex/sessions")
        // The newest days only.
        let days = subfolders(of: root, depth: 3).sorted { $0.path > $1.path }.prefix(14)
        let files = newestFiles(in: Array(days), limit: 60) { $0.lastPathComponent.hasPrefix("rollout-") && $0.pathExtension == "jsonl" }
        var here: [SpecCompletion.Item] = []
        var elsewhere: [SpecCompletion.Item] = []
        for file in files {
            let text = prefix(of: file)
            var id: String?
            var cwd: String?
            var message: String?
            for line in text.split(whereSeparator: \.isNewline) {
                guard let data = line.data(using: .utf8),
                      let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                      let payload = object["payload"] as? [String: Any] else { continue }
                if object["type"] as? String == "session_meta" {
                    id = (payload["id"] ?? payload["session_id"]) as? String
                    cwd = payload["cwd"] as? String
                } else if payload["type"] as? String == "user_message", let text = payload["message"] as? String {
                    message = text
                    break
                }
            }
            guard let id else { continue }
            let item = SpecCompletion.Item(name: id, description: message.map(summary) ?? cwd.map { ($0 as NSString).lastPathComponent },
                                           kind: .argument)
            if cwd == directory { here.append(item) } else { elsewhere.append(item) }
        }
        return Array((here + elsewhere).prefix(limit))
    }

    private static func summary(_ text: String) -> String {
        let line = text.split(whereSeparator: \.isNewline).first.map(String.init) ?? text
        return line.count > 70 ? String(line.prefix(69)) + "…" : line
    }

    /// The first 256 KB of a file, enough for a session's opening lines.
    private static func prefix(of file: URL) -> String {
        guard let handle = try? FileHandle(forReadingFrom: file) else { return "" }
        defer { try? handle.close() }
        return String(decoding: (try? handle.read(upToCount: 256 * 1024)) ?? Data(), as: UTF8.self)
    }

    private static func newestFiles(in folders: [URL], limit: Int, where keep: (URL) -> Bool) -> [URL] {
        let fm = FileManager.default
        let files = folders.flatMap { (try? fm.contentsOfDirectory(at: $0, includingPropertiesForKeys: [.contentModificationDateKey])) ?? [] }
            .filter(keep)
        func date(_ url: URL) -> Date { (try? url.resourceValues(forKeys: [.contentModificationDateKey]).contentModificationDate) ?? .distantPast }
        return Array(files.map { ($0, date($0)) }.sorted { $0.1 > $1.1 }.prefix(limit).map(\.0))
    }

    private static func subfolders(of root: URL, depth: Int) -> [URL] {
        guard depth > 0 else { return [root] }
        let children = ((try? FileManager.default.contentsOfDirectory(at: root, includingPropertiesForKeys: nil)) ?? [])
            .filter { $0.hasDirectoryPath }
        return children.flatMap { subfolders(of: $0, depth: depth - 1) }
    }

    private static func statusName(_ code: String) -> String? {
        switch code.first {
        case "M": "modified"
        case "A": "added"
        case "D": "deleted"
        case "R": "renamed"
        case "?": "untracked"
        case "U": "conflict"
        default: nil
        }
    }
}
