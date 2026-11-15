import Foundation
import TermyNextCore

/// `rex …` inside Termy Next: the app's binary run as Rex's command-line
/// tool (`bin/rex` runs it with `--rex-cli`). It sends one request to the
/// running app and prints the answer the way Rex does.
enum PluginCLIRunner {
    static func main(_ arguments: [String]) -> Int32 {
        do {
            let globals = try PluginCLI.globals(arguments)
            timeout = globals.timeout ?? 30
            // Another Mac: its own rex runs the command, over ssh.
            if let server = globals.server ?? home(), !["local", "localhost", "here"].contains(server.lowercased()) {
                return try remote(server, globals.rest)
            }
            switch try PluginCLI.plan(globals.rest) {
            case .help(let text):
                print(PluginCLI.named(text))
                return 0
            case .version:
                let version = Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "dev"
                print("\(PluginCLI.program) (Termy Next \(version))")
                return 0
            case .text(let text):
                print(PluginCLI.named(text))
                return 0
            case .home(let set, let clear):
                return setHome(set, clear: clear)
            case .skillSetup(let directory, let remove):
                return skill(directory, remove: remove)
            case .stream(let method, let params, let json):
                return try stream(method, params.value, json: json)
            case .call(let method, let params, let render, let wait):
                let result = try request(method, params.value)
                let status = try output(result, render: render)
                if wait, let block = ((result as? [String: Any])?["block_ids"] as? [String])?.first {
                    let ended = try request("block.wait", ["block_id": block], timeout: nil) as? [String: Any]
                    return Int32(truncatingIfNeeded: (ended?["exit_code"] as? Int) ?? 0)
                }
                return status
            }
        } catch let failure as PluginCLI.Failure {
            FileHandle.standardError.write(Data("Error: \(PluginCLI.named(failure.description))\n".utf8))
            return 1
        } catch {
            FileHandle.standardError.write(Data("Error: \(error)\n".utf8))
            return 1
        }
    }

    private static let environment = ProcessInfo.processInfo.environment
    nonisolated(unsafe) private static var timeout: Double = 30

    private static var homeFile: URL { PluginPaths.paths.supportDirectory.appendingPathComponent("home") }

    /// The host `rex home` chose, used when no -S names one.
    private static func home() -> String? {
        guard let text = try? String(contentsOf: homeFile, encoding: .utf8) else { return nil }
        let value = text.trimmingCharacters(in: .whitespacesAndNewlines)
        return value.isEmpty ? nil : value
    }

    private static func setHome(_ host: String?, clear: Bool) -> Int32 {
        if clear {
            try? FileManager.default.removeItem(at: homeFile)
            print("Home is this Mac.")
        } else if let host {
            try? FileManager.default.createDirectory(at: homeFile.deletingLastPathComponent(), withIntermediateDirectories: true)
            try? (host + "\n").write(to: homeFile, atomically: true, encoding: .utf8)
            print("Home is \(host): rex sends commands there unless -S names another. rex home --clear goes back to this Mac.")
        } else {
            print(home().map { "Home is \($0)." } ?? "Home is this Mac.")
        }
        return 0
    }

    /// Runs `arguments` with the rex of Termy Next on `server`, a host's
    /// label or an ssh destination.
    private static func remote(_ server: String, _ arguments: [String]) throws -> Int32 {
        var destination = server
        if let hosts = (try? request("hosts.list", [:]) as? [String: Any])?["hosts"] as? [[String: Any]],
           let host = hosts.first(where: { ($0["label"] as? String)?.lowercased() == server.lowercased() || $0["id"] as? String == server }),
           let endpoint = (host["endpoints"] as? [String])?.first {
            destination = endpoint
        }
        let script = #"r="$HOME/Library/Application Support/TermyNext/bin/rex"; [ -x "$r" ] || r=rex; exec "$r" "$@""#
        let command = (["sh", "-c", script, "rex"] + arguments).map(ShellQuote.quote).joined(separator: " ")
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/ssh")
        process.arguments = ["-o", "ConnectTimeout=10", "-q", destination, command]
        try process.run()
        process.waitUntilExit()
        return process.terminationStatus
    }

    private static func skill(_ directory: String?, remove: Bool) -> Int32 {
        let folder = URL(fileURLWithPath: (directory ?? "~/.agents/skills/termy-next") .replacingOccurrences(of: "~", with: NSHomeDirectory()))
        let file = folder.appendingPathComponent("SKILL.md")
        if remove {
            try? FileManager.default.removeItem(at: file)
            try? FileManager.default.removeItem(at: folder)
            print("Removed \(file.path).")
            return 0
        }
        let body = "---\nname: termy-next\ndescription: Manage Termy Next sessions, panes and commands with the rex command inside Termy Next panes.\n---\n\n<!-- Installed by \"rex skill setup\". Remove it with \"rex skill remove\". -->\n\n" + PluginCLI.skillGuide
        do {
            try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
            try body.write(to: file, atomically: true, encoding: .utf8)
            print("Wrote \(file.path).")
            return 0
        } catch {
            FileHandle.standardError.write(Data("Error: can't write \(file.path): \(error.localizedDescription)\n".utf8))
            return 1
        }
    }

    /// Prints what a connection that stays open sends, until it closes or ^C.
    private static func stream(_ method: String, _ params: [String: Any], json: Bool) throws -> Int32 {
        let context = requestContext()
        try PluginWire.stream(["method": method, "params": params, "context": context], to: PluginPaths.socket) { line in
            guard let object = try? JSONSerialization.jsonObject(with: line) as? [String: Any] else { return true }
            if object["ok"] as? Bool == false {
                let error = object["error"] as? [String: Any]
                FileHandle.standardError.write(Data("Error: \(error?["message"] as? String ?? "the request failed")\n".utf8))
                return false
            }
            if object["ok"] != nil { return true }
            if json {
                FileHandle.standardOutput.write(line + Data([0x0A]))
            } else {
                let type = object["type"] as? String ?? "event"
                let parts = ["session_id", "window_id", "block_id", "event", "label"].compactMap { key in (object[key] as? String).map { "\(key)=\($0)" } }
                print(([type] + parts).joined(separator: " "))
            }
            fflush(stdout)
            return true
        }
        return 0
    }

    private static func requestContext() -> [String: Any] {
        var context: [String: Any] = ["origin": "cli"]
        if let session = environment["REX_SESSION"] ?? environment["TERMY_SESSION"], !session.isEmpty { context["session_id"] = session }
        if let block = environment["REX_BLOCK"] ?? environment["TERMY_BLOCK"], !block.isEmpty { context["caller_block_id"] = block }
        return context
    }

    private static func request(_ method: String, _ params: [String: Any], timeout: Double? = PluginCLIRunner.timeout) throws -> Any? {
        let context = requestContext()
        let long = ["termy.run_script", "block.wait", "client.action"].contains(method)
        let reply = try PluginWire.send(["method": method, "params": params, "context": context],
                                        to: PluginPaths.socket, timeout: long ? nil : timeout)
        // A script's print output comes first, as it would in Rex.
        if let output = reply["output"] as? String, !output.isEmpty { FileHandle.standardOutput.write(Data(output.utf8)) }
        if reply["ok"] as? Bool == true { return reply["result"] is NSNull ? nil : reply["result"] }
        let error = reply["error"] as? [String: Any] ?? [:]
        let message = error["message"] as? String ?? "the request failed"
        let code = error["code"] as? String
        throw PluginCLI.Failure(code.map { "\(message) (code=\($0))" } ?? message)
    }

    // MARK: Printing

    private static func json(_ value: Any?) -> String {
        guard let value, JSONSerialization.isValidJSONObject(value) || value is String || value is NSNumber else { return "null" }
        let options: JSONSerialization.WritingOptions = [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes, .fragmentsAllowed]
        guard let data = try? JSONSerialization.data(withJSONObject: value, options: options) else { return "null" }
        return String(decoding: data, as: UTF8.self)
    }

    private static func table(_ rows: [[String]]) -> String {
        guard let width = rows.map(\.count).max() else { return "" }
        var widths = [Int](repeating: 0, count: width)
        for row in rows { for (i, cell) in row.enumerated() { widths[i] = max(widths[i], cell.count) } }
        return rows.map { row in
            row.enumerated().map { i, cell in i == row.count - 1 ? cell : cell.padding(toLength: widths[i], withPad: " ", startingAt: 0) }
                .joined(separator: "  ")
        }.joined(separator: "\n")
    }

    private static func output(_ result: Any?, render: PluginCLI.Render) throws -> Int32 {
        let object = result as? [String: Any] ?? [:]
        switch render {
        case .json:
            print(json(result))
        case .value:
            switch result {
            case nil: break
            case let text as String: print(text)
            case let number as NSNumber: print(number)
            default: print(json(result))
            }
        case .none:
            break
        case .sessions:
            let sessions = object["sessions"] as? [[String: Any]] ?? []
            print(table([["SESSION", "LABEL", "WINDOWS", "ATTACHED"]] + sessions.map {
                [$0["session_id"] as? String ?? "", $0["label"] as? String ?? "", "\($0["window_count"] ?? 0)",
                 ($0["attached"] as? Bool) == true ? "yes" : "no"]
            }))
        case .blocks:
            let blocks = object["blocks"] as? [[String: Any]] ?? []
            print(table([["BLOCK", "LABEL", "WINDOW"]] + blocks.map {
                [$0["block_id"] as? String ?? "", $0["label"] as? String ?? "", $0["window_id"] as? String ?? "-"]
            }))
        case .windows:
            let windows = object["windows"] as? [[String: Any]] ?? []
            let active = object["active_window_id"] as? String
            print(table([["WINDOW", "LABEL", "BLOCKS", "ACTIVE"]] + windows.map { window in
                let count = (window["layers"] as? [[String: Any]] ?? []).reduce(0) { $0 + (($1["blocks"] as? [Any])?.count ?? 0) }
                let id = window["window_id"] as? String ?? ""
                return [id, window["label"] as? String ?? "", "\(count)", id == active ? "*" : ""]
            }))
        case .blockIDs:
            for id in object["block_ids"] as? [String] ?? [] { print(id) }
        case .sessionID:
            print(object["session_id"] as? String ?? "")
        case .capture(let trim):
            var text = object["content"] as? String ?? object["text"] as? String ?? ""
            if trim {
                var lines = text.components(separatedBy: "\n").map { $0.replacingOccurrences(of: "\\s+$", with: "", options: .regularExpression) }
                while lines.last?.isEmpty == true { lines.removeLast() }
                text = lines.joined(separator: "\n") + (lines.isEmpty ? "" : "\n")
            }
            FileHandle.standardOutput.write(Data(text.utf8))
        case .configCheck:
            print("path: \(object["path"] as? String ?? "")")
            let exists = object["exists"] as? Bool == true
            print("exists: \(exists ? "yes" : "no")")
            if exists {
                for key in ["bindings", "removals", "modes", "actions"] { print("\(key): \(object[key] ?? 0)") }
            }
            let problems = object["diagnostics"] as? [String] ?? []
            for problem in problems { FileHandle.standardError.write(Data("error: \(problem)\n".utf8)) }
            return problems.isEmpty ? 0 : 1
        case .actions:
            let actions = object["actions"] as? [[String: Any]] ?? []
            print(table([["ACTION", "TITLE"]] + actions.map { [$0["name"] as? String ?? "", $0["title"] as? String ?? ""] }))
        case .keymap:
            let bindings = object["bindings"] as? [[String: Any]] ?? []
            print(table([["KEY", "ACTION", "SOURCE"]] + bindings.map {
                [($0["mode"] as? String).map { "\($0)/" } ?? "" + "", $0["key"] as? String ?? "", $0["action"] as? String ?? "", $0["source"] as? String ?? ""]
            }.map { [$0[0] + $0[1], $0[2], $0[3]] }))
        case .clients:
            let clients = object["clients"] as? [[String: Any]] ?? []
            print(table([["CLIENT", "KIND", "NAME", "LABEL"]] + clients.map { client in
                let info = client["info"] as? [String: Any] ?? [:]
                return [client["client_id"] as? String ?? "", info["kind"] as? String ?? "", info["name"] as? String ?? "", client["label"] as? String ?? ""]
            }))
        case .hosts:
            let hosts = object["hosts"] as? [[String: Any]] ?? []
            print(table([["LABEL", "ENDPOINTS", "ID"]] + hosts.map {
                [$0["label"] as? String ?? "", ($0["endpoints"] as? [String] ?? []).joined(separator: ","), $0["id"] as? String ?? ""]
            }))
        }
        return 0
    }
}
