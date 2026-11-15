import AppKit
import Termy
import TermyNextCore

/// What block methods keep between calls: large results to read in parts,
/// what programs copied, file transfers, data tickets and the sizes clients
/// asked for.
@MainActor
final class PluginBlockStore {
    static let shared = PluginBlockStore()

    struct Blob {
        var data: Data
        var expires: Date
    }

    enum Transfer {
        case download(path: String)
        case upload(pane: UUID, batch: String, index: Int, count: Int, path: String, size: Int)
    }

    struct DataTicket {
        var pane: UUID
        var subprotocol: String
        var expires: Date
    }

    private(set) var blobs: [Int: Blob] = [:]
    private(set) var clipboards: [Int: Blob] = [:]
    private var nextID = 1
    var transfers: [String: (transfer: Transfer, expires: Date)] = [:]
    var uploads: [String: [Int: String]] = [:]
    var dataTickets: [String: DataTicket] = [:]
    /// Sizes clients asked a pane for, by client ID.
    var desiredSizes: [UUID: [String: (columns: Int, rows: Int)]] = [:]

    func keep(_ data: Data, minutes: Double = 5) -> Int {
        expire()
        let id = nextID
        nextID += 1
        blobs[id] = Blob(data: data, expires: Date().addingTimeInterval(minutes * 60))
        return id
    }

    func keepClipboard(_ data: Data) -> Int {
        expire()
        let id = nextID
        nextID += 1
        clipboards[id] = Blob(data: data, expires: Date().addingTimeInterval(60))
        return id
    }

    private func expire() {
        let now = Date()
        blobs = blobs.filter { $0.value.expires > now }
        clipboards = clipboards.filter { $0.value.expires > now }
        transfers = transfers.filter { $0.value.expires > now }
        dataTickets = dataTickets.filter { $0.value.expires > now }
    }

    static func ticket() -> String {
        var bytes = [UInt8](repeating: 0, count: 18)
        _ = SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes)
        return Data(bytes).base64EncodedString()
            .replacingOccurrences(of: "+", with: "-").replacingOccurrences(of: "/", with: "_").replacingOccurrences(of: "=", with: "")
    }
}

extension PluginAPI {
    static let blockMethods = [
        "blob_read", "clear", "clipboard_read", "download_begin", "format", "input_mode", "list_dir", "modes",
        "process", "program_status", "pwd", "reset", "resize", "set_theme", "size", "title", "upload_begin", "write",
    ]

    /// Methods on blocks, and the data they hand out.
    func blockCall(_ method: String, _ params: [String: Any], _ context: Context) throws -> Any? {
        switch method {
        case "block.method":
            guard let name = params["method"] as? String else { throw Failure("bad_request", "method is required") }
            guard params["block_id"] != nil || context.blockID != nil || context.callerBlockID != nil || params["session_id"] != nil
                || context.sessionID != nil
            else { throw Failure("bad_request", "block_id is required") }
            let target = try resolveAnyPane(params["block_id"] as? String, context)
            let short = PluginFlavor.terminalMethod(name) ?? name
            return try terminalMethod(short, params["args"] as? [String: Any] ?? [:], pane: target.pane, context: context)
        case "block.send_keys": return try sendKeys(params, context)
        case "block.event": return try blockEvent(params, context)
        case "block.state":
            guard let found = findAnyPane(params["block_id"] as? String) else { return ["exists": false] }
            var state: [String: Any] = ["exists": true, "exited": found.pane.hasExited]
            if let code = found.pane.exitCode { state["exit_code"] = Int(code) }
            return state
        case "data.connect": return try dataConnect(params, context)
        case "data.read": return try dataRead(params)
        case "data.write": return try dataWrite(params)
        default: return nil
        }
    }

    // MARK: Terminal methods

    /// A method of a terminal block, by its short name, as Rex's
    /// `com.superlogical.terminal` creator answers it.
    func terminalMethod(_ method: String, _ args: [String: Any], pane: PaneView, context: Context = Context()) throws -> Any {
        let session = pane.terminal.session
        let store = PluginBlockStore.shared
        switch method {
        case "process":
            return processReport(pane)
        case "pwd":
            var result: [String: Any] = [:]
            if let report = session.workingDirectory {
                result["pwd"] = report.hasPrefix("/") ? fileURL(report) : report
            } else if let directory = pane.directory {
                result["pwd"] = fileURL(directory)
            }
            if let owner = frontOwner(pane) { result["owner"] = owner }
            return result
        case "program_status":
            return ["records": Self.statusRecords(of: pane)]
        case "write":
            guard let data = args["data"] as? String else { throw Failure("bad_request", "data is required") }
            do { try session.terminal.write(data) } catch { throw Failure("internal", "the terminal didn't take the text: \(error)") }
            return ["bytes_written": data.utf8.count]
        case "format":
            return try format(args, pane: pane)
        case "blob_read":
            return try read(store.blobs, args, what: "result")
        case "clipboard_read":
            return try read(store.clipboards, args, what: "copy")
        case "modes":
            return try modes(pane)
        case "input_mode":
            guard let pid = pane.foregroundPID ?? pane.childPID, let mode = ShellProcess.inputMode(of: pid) else {
                throw Failure("not_found", "the terminal's input mode can't be read")
            }
            return ["echo": mode.echo, "canonical": mode.canonical, "signals": mode.signals]
        case "title":
            var result: [String: Any] = ["title": session.title ?? ""]
            if let owner = frontOwner(pane) { result["owner"] = owner }
            return result
        case "size":
            return size(pane)
        case "resize":
            guard let columns = (args["columns"] as? NSNumber)?.intValue, let rows = (args["rows"] as? NSNumber)?.intValue,
                  columns > 0, rows > 0, columns <= 10_000, rows <= 10_000
            else { throw Failure("bad_request", "columns and rows must be positive") }
            let caller = PluginClients.callerID(context)
            store.desiredSizes[pane.id, default: [:]][caller] = (columns, rows)
            // It applies when the caller owns the pane; the app otherwise fits it to its view.
            if owner(of: pane) == caller {
                var size = session.size
                size.cols = UInt16(columns)
                size.rows = UInt16(rows)
                try? session.resize(size)
            }
            return [String: Any]()
        case "set_theme":
            try setTheme(args, pane: pane)
            return [String: Any]()
        case "clear":
            _ = try? session.terminal.clearScrollback()
            return [String: Any]()
        case "reset":
            try? session.feed(Data("\u{1b}c".utf8))
            return [String: Any]()
        case "list_dir":
            return listDirectory(args, pane: pane)
        case "download_begin":
            return try downloadBegin(args, pane: pane)
        case "upload_begin":
            return try uploadBegin(args, pane: pane)
        default:
            throw Failure("method_not_found", "the block has no method named \(method)")
        }
    }

    func owner(of pane: PaneView) -> String {
        for session in Workspace.shared.sessions {
            for tab in session.tabs where tab.panes[pane.id] != nil {
                return tab.owners[pane.id] ?? PluginClients.appClientID
            }
        }
        return detachedOwner[pane.id] ?? PluginClients.appClientID
    }

    private func frontOwner(_ pane: PaneView) -> [String: Any]? {
        guard let front = pane.foregroundPID else { return nil }
        return ["pid": Int(front), "name": ShellProcess.name(of: front) ?? ""]
    }

    func fileURL(_ path: String) -> String {
        // gethostname, not ProcessInfo.hostName, which can wait on DNS.
        var buffer = [CChar](repeating: 0, count: 256)
        let host = gethostname(&buffer, buffer.count) == 0 ? String(cString: buffer) : "localhost"
        let encoded = path.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? path
        return "file://\(host)\(encoded)"
    }

    private func format(_ args: [String: Any], pane: PaneView) throws -> [String: Any] {
        let name = (args["format"] as? String ?? "plain").lowercased()
        let format: TerminalTextFormat = switch name {
        case "plain", "text", "": .plain
        case "vt", "ansi", "escapes": .vt
        case "html": .html
        default: throw Failure("bad_request", "format must be plain, html or vt")
        }
        // A pane off screen isn't drawn; reading catches its terminal up first.
        pane.terminal.session.poll(forceFull: true)
        var content: String
        do {
            content = try pane.terminal.session.terminal.text(format, maxLines: (args["max_lines"] as? NSNumber)?.intValue, unwrap: args["unwrap"] as? Bool ?? false)
        } catch {
            throw Failure("internal", "the terminal can't be read: \(error)")
        }
        if args["trim"] as? Bool == true {
            var lines = content.components(separatedBy: "\n")
            if lines.last == "" { lines.removeLast() }
            lines = lines.map { $0.replacingOccurrences(of: "[ \\t]+$", with: "", options: .regularExpression) }
            while let first = lines.first, first.isEmpty { lines.removeFirst() }
            while let last = lines.last, last.isEmpty { lines.removeLast() }
            content = lines.isEmpty ? "" : lines.joined(separator: "\n") + "\n"
        }
        if args["extra_pwd"] as? Bool == true, let directory = pane.directory {
            content = "\u{1b}]7;\(fileURL(directory))\u{7}" + content
        }
        let data = Data(content.utf8)
        if let limit = (args["max_bytes"] as? NSNumber)?.intValue, limit > 0, data.count > limit {
            let id = PluginBlockStore.shared.keep(data)
            return ["content": String(decoding: data.prefix(limit), as: UTF8.self), "id": id, "total": data.count, "eof": false]
        }
        return ["content": content, "total": data.count, "eof": true]
    }

    private func read(_ blobs: [Int: PluginBlockStore.Blob], _ args: [String: Any], what: String) throws -> [String: Any] {
        guard let id = (args["id"] as? NSNumber)?.intValue else { throw Failure("bad_request", "id is required") }
        guard let blob = blobs[id], blob.expires > Date() else { throw Failure("not_found", "no \(what) \(id); they're kept a few minutes") }
        let offset = max(0, (args["offset"] as? NSNumber)?.intValue ?? 0)
        let limit = (args["max_bytes"] as? NSNumber)?.intValue ?? (1 << 20)
        guard offset <= blob.data.count else { throw Failure("bad_request", "offset is past the end") }
        let end = min(blob.data.count, offset + max(limit, 1))
        return ["id": id, "data": String(decoding: blob.data[offset..<end], as: UTF8.self), "total": blob.data.count,
                "eof": end == blob.data.count]
    }

    private func modes(_ pane: PaneView) throws -> [String: Any] {
        let modes: TerminalModes
        do { modes = try pane.terminal.session.terminal.modes() } catch { throw Failure("internal", "the terminal's modes can't be read") }
        let list: [(String, Int, Bool)] = [
            ("alternate_screen", 1049, modes.alternateScreen),
            ("bracketed_paste", 2004, modes.bracketedPaste),
            ("mouse_click", 1000, modes.mouse.enabled && modes.mouse.reportClick),
            ("mouse_drag", 1002, modes.mouse.enabled && modes.mouse.reportDrag),
            ("mouse_motion", 1003, modes.mouse.enabled && modes.mouse.reportMotion),
            ("mouse_utf8", 1005, modes.mouse.utf8Encoding),
            ("mouse_sgr", 1006, modes.mouse.sgrEncoding),
            ("application_keypad", 66, modes.keyboard.applicationKeypad),
            ("kitty_disambiguate", 0, modes.keyboard.disambiguateEscapeCodes),
            ("kitty_report_events", 0, modes.keyboard.reportEventTypes),
            ("kitty_report_alternates", 0, modes.keyboard.reportAlternateKeys),
            ("kitty_report_all_keys", 0, modes.keyboard.reportAllKeysAsEscapeCodes),
            ("kitty_report_text", 0, modes.keyboard.reportAssociatedText),
        ]
        return [
            "screen": modes.alternateScreen ? "alternate" : "primary",
            "modes": list.map { name, number, on -> [String: Any] in
                var mode: [String: Any] = ["name": name, "on": on]
                if number > 0 { mode["number"] = number }
                return mode
            },
        ]
    }

    private func size(_ pane: PaneView) -> [String: Any] {
        let size = pane.terminal.session.size
        // The grid's cells are measured in device pixels already.
        var result: [String: Any] = [
            "columns": Int(size.cols), "rows": Int(size.rows),
            "cell_width_px": Int(size.cellWidth.rounded()),
            "cell_height_px": Int(size.cellHeight.rounded()),
            "owner": owner(of: pane),
        ]
        let desired = PluginBlockStore.shared.desiredSizes[pane.id] ?? [:]
        result["desired"] = desired.sorted { $0.key < $1.key }.map { ["client_id": $0.key, "columns": $0.value.columns, "rows": $0.value.rows] }
        return result
    }

    /// Replaces the terminal's default colors, as a program would with OSC 4, 10, 11 and 12.
    private func setTheme(_ args: [String: Any], pane: PaneView) throws {
        func color(_ raw: Any?) throws -> String? {
            guard let text = raw as? String, !text.isEmpty else { return nil }
            let hex = text.hasPrefix("#") ? String(text.dropFirst()) : text
            guard [3, 6].contains(hex.count), hex.allSatisfy(\.isHexDigit) else { throw Failure("bad_request", "\(text) isn't a #rrggbb color") }
            let full = hex.count == 3 ? hex.map { "\($0)\($0)" }.joined() : hex
            let parts = stride(from: 0, to: 6, by: 2).map { i -> String in
                let start = full.index(full.startIndex, offsetBy: i)
                return String(full[start..<full.index(start, offsetBy: 2)])
            }
            return "rgb:" + parts.joined(separator: "/")
        }
        var sequence = ""
        if let value = try color(args["foreground"]) { sequence += "\u{1b}]10;\(value)\u{1b}\\" }
        if let value = try color(args["background"]) { sequence += "\u{1b}]11;\(value)\u{1b}\\" }
        if let value = try color(args["cursor"]) { sequence += "\u{1b}]12;\(value)\u{1b}\\" }
        for (index, raw) in (args["palette"] as? [Any] ?? []).enumerated() where index < 256 {
            if let value = try color(raw) { sequence += "\u{1b}]4;\(index);\(value)\u{1b}\\" }
        }
        guard !sequence.isEmpty else { return }
        try? pane.terminal.session.feed(Data(sequence.utf8))
    }

    private func listDirectory(_ args: [String: Any], pane: PaneView) -> [String: Any] {
        let home = NSHomeDirectory()
        var raw = (args["path"] as? String).flatMap { $0.isEmpty ? nil : $0 } ?? "."
        if raw == "~" || raw.hasPrefix("~/") { raw = (raw as NSString).expandingTildeInPath }
        let base = pane.foregroundPID.flatMap(ShellProcess.workingDirectory(of:)) ?? pane.directory ?? home
        let path = ((raw.hasPrefix("/") ? raw : (base as NSString).appendingPathComponent(raw)) as NSString).standardizingPath
        var result: [String: Any] = ["path": path, "home": home]
        var isDirectory: ObjCBool = false
        guard FileManager.default.fileExists(atPath: path, isDirectory: &isDirectory) else {
            result["status"] = "not_found"
            result["entries"] = [Any]()
            result["truncated"] = false
            return result
        }
        guard isDirectory.boolValue else {
            result["status"] = "not_a_directory"
            result["entries"] = [Any]()
            result["truncated"] = false
            return result
        }
        guard let names = try? FileManager.default.contentsOfDirectory(atPath: path) else {
            result["status"] = "permission_denied"
            result["entries"] = [Any]()
            result["truncated"] = false
            return result
        }
        let prefix = (args["prefix"] as? String ?? "").lowercased()
        let files = args["files"] as? Bool ?? false
        let limit = (args["limit"] as? NSNumber)?.intValue ?? 1000
        var entries: [[String: Any]] = []
        for name in names.sorted(by: { $0.localizedStandardCompare($1) == .orderedAscending })
        where prefix.isEmpty || name.lowercased().hasPrefix(prefix) {
            var dir: ObjCBool = false
            let full = (path as NSString).appendingPathComponent(name)
            FileManager.default.fileExists(atPath: full, isDirectory: &dir)
            if !dir.boolValue, !files { continue }
            var entry: [String: Any] = ["name": name, "kind": dir.boolValue ? "directory" : "file"]
            if name.hasPrefix(".") { entry["hidden"] = true }
            entries.append(entry)
        }
        result["status"] = "ok"
        result["truncated"] = entries.count > limit
        result["entries"] = Array(entries.prefix(max(limit, 0)))
        return result
    }

    // MARK: Files

    private func downloadBegin(_ args: [String: Any], pane: PaneView) throws -> [String: Any] {
        guard let raw = args["path"] as? String, !raw.isEmpty else { throw Failure("bad_request", "path is required") }
        let base = pane.foregroundPID.flatMap(ShellProcess.workingDirectory(of:)) ?? pane.directory ?? NSHomeDirectory()
        let expanded = (raw as NSString).expandingTildeInPath
        let path = ((expanded.hasPrefix("/") ? expanded : (base as NSString).appendingPathComponent(expanded)) as NSString).standardizingPath
        guard let attributes = try? FileManager.default.attributesOfItem(atPath: path) else { throw Failure("not_found", "no file \(path)") }
        guard attributes[.type] as? FileAttributeType == .typeRegular else { throw Failure("bad_request", "\(path) isn't a regular file") }
        let size = (attributes[.size] as? NSNumber)?.intValue ?? 0
        let modified = attributes[.modificationDate] as? Date ?? Date()
        let version = "\(size)-\(Int(modified.timeIntervalSince1970 * 1000))"
        var result: [String: Any] = [
            "path": path, "name": (path as NSString).lastPathComponent, "size": size,
            "modified_at": ISO8601DateFormatter().string(from: modified), "version": version,
        ]
        if args["if_none_match"] as? String == version {
            result["not_modified"] = true
            return result
        }
        let ticket = PluginBlockStore.ticket()
        PluginBlockStore.shared.transfers[ticket] = (.download(path: path), Date().addingTimeInterval(60))
        result["ticket"] = ticket
        result["expires_in_ms"] = 60_000
        return result
    }

    private func uploadBegin(_ args: [String: Any], pane: PaneView) throws -> [String: Any] {
        guard let name = (args["name"] as? String).map({ ($0 as NSString).lastPathComponent }), !name.isEmpty, name != "..", name != "." else {
            throw Failure("bad_request", "name is required")
        }
        let size = (args["size"] as? NSNumber)?.intValue ?? 0
        let batch = (args["batch"] as? String).flatMap { $0.isEmpty ? nil : $0 } ?? PluginBlockStore.ticket()
        let index = (args["index"] as? NSNumber)?.intValue ?? 0
        let count = max((args["count"] as? NSNumber)?.intValue ?? 1, 1)
        let safeBatch = batch.filter { $0.isLetter || $0.isNumber || $0 == "-" || $0 == "_" }
        let folder = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("TermyNext/uploads/\(safeBatch)", isDirectory: true)
        try? FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let path = folder.appendingPathComponent(name).path
        FileManager.default.createFile(atPath: path, contents: Data())
        let ticket = PluginBlockStore.ticket()
        PluginBlockStore.shared.transfers[ticket] = (.upload(pane: pane.id, batch: batch, index: index, count: count, path: path, size: size),
                                                     Date().addingTimeInterval(600))
        return ["ticket": ticket, "expires_in_ms": 600_000]
    }

    /// Reads a downloaded file in parts, base64: `{ticket, offset, max_bytes}`.
    private func dataRead(_ params: [String: Any]) throws -> [String: Any] {
        guard let ticket = params["ticket"] as? String, let entry = PluginBlockStore.shared.transfers[ticket],
              case .download(let path) = entry.transfer
        else { throw Failure("not_found", "no download for that ticket") }
        guard let handle = FileHandle(forReadingAtPath: path) else { throw Failure("not_found", "can't read \(path)") }
        defer { try? handle.close() }
        let offset = UInt64(max(0, (params["offset"] as? NSNumber)?.intValue ?? 0))
        let limit = min(max((params["max_bytes"] as? NSNumber)?.intValue ?? (4 << 20), 1), 16 << 20)
        try? handle.seek(toOffset: offset)
        let data = (try? handle.read(upToCount: limit)) ?? Data()
        let total = (try? FileManager.default.attributesOfItem(atPath: path)[.size] as? NSNumber)?.intValue ?? 0
        let eof = Int(offset) + data.count >= total
        if eof { PluginBlockStore.shared.transfers[ticket] = nil }
        return ["data": data.base64EncodedString(), "offset": Int(offset), "total": total, "eof": eof]
    }

    /// Writes part of an upload, base64: `{ticket, data, offset}`. When every
    /// file of its batch is in, the paths are pasted into the terminal.
    private func dataWrite(_ params: [String: Any]) throws -> [String: Any] {
        let store = PluginBlockStore.shared
        guard let ticket = params["ticket"] as? String, let entry = store.transfers[ticket],
              case .upload(let paneID, let batch, let index, let count, let path, let size) = entry.transfer
        else { throw Failure("not_found", "no upload for that ticket") }
        guard let data = Data(base64Encoded: params["data"] as? String ?? "") else { throw Failure("bad_request", "data must be base64") }
        guard let handle = FileHandle(forWritingAtPath: path) else { throw Failure("internal", "can't write \(path)") }
        let offset = UInt64(max(0, (params["offset"] as? NSNumber)?.intValue ?? 0))
        try? handle.seek(toOffset: offset)
        try? handle.write(contentsOf: data)
        let written = Int((try? handle.offsetInFile) ?? 0)
        try? handle.close()
        let done = params["eof"] as? Bool ?? (written >= size)
        guard done else { return ["written": written, "complete": false] }
        store.transfers[ticket] = nil
        store.uploads[batch, default: [:]][index] = path
        if store.uploads[batch]?.count ?? 0 >= count, let paths = store.uploads.removeValue(forKey: batch) {
            let text = paths.sorted { $0.key < $1.key }.map { ShellQuote.quote($0.value) }.joined(separator: " ") + " "
            if let pane = findAnyPane(PluginID.make("block", paneID))?.pane { try? pane.terminal.session.paste(text) }
        }
        return ["written": written, "complete": true]
    }

    private func dataConnect(_ params: [String: Any], _ context: Context) throws -> [String: Any] {
        let target = try resolveAnyPane(params["block_id"] as? String, context)
        let reference = params["data_ref_id"] as? String ?? "terminal"
        guard reference == "terminal" else { throw Failure("not_found", "the block has no data named \(reference)") }
        let subprotocol = params["subprotocol"] as? String ?? "termy.data.terminal.v1"
        guard ["termy.data.terminal.v1", "rex.data.terminal.v1"].contains(subprotocol) else {
            throw Failure("bad_request", "subprotocol must be termy.data.terminal.v1")
        }
        let ticket = PluginBlockStore.ticket()
        PluginBlockStore.shared.dataTickets[ticket] = .init(pane: target.pane.id, subprotocol: subprotocol, expires: Date().addingTimeInterval(60))
        return ["ticket": ticket, "expires_in_ms": 60_000]
    }

    // MARK: Processes

    private func processInfo(_ pid: pid_t) -> [String: Any] {
        var info: [String: Any] = ["pid": Int(pid), "name": ShellProcess.name(of: pid) ?? "?", "user_id": 0]
        let path = ShellProcess.executablePath(of: pid)
        if let path { info["path"] = path }
        let arguments = ShellProcess.arguments(of: pid)
        if let argv0 = arguments.first {
            info["argv0"] = String(argv0.prefix(256))
            // The path it was started through, a symlink say, when it leads to the same file.
            if argv0.hasPrefix("/"), let path, argv0 != path,
               (try? FileManager.default.destinationOfSymbolicLink(atPath: argv0)) != nil || (argv0 as NSString).resolvingSymlinksInPath == path {
                info["invoked_path"] = argv0
            }
        }
        if let cwd = ShellProcess.workingDirectory(of: pid) { info["cwd"] = cwd }
        if let (_, uid) = ShellProcess.parentAndUser(of: pid) {
            info["user_id"] = Int(uid)
            if let entry = getpwuid(uid) { info["user"] = String(cString: entry.pointee.pw_name) }
        }
        if ShellProcess.scriptName(arguments: arguments) != nil,
           let script = arguments.dropFirst().first(where: { !$0.hasPrefix("-") && FileManager.default.fileExists(atPath: $0) }) {
            info["script"] = script.hasPrefix("/") ? script : ((info["cwd"] as? String ?? "") as NSString).appendingPathComponent(script)
        }
        return info
    }

    private func processReport(_ pane: PaneView) -> [String: Any] {
        var report: [String: Any] = ["home": NSHomeDirectory(), "last_exit": NSNull()]
        if pane.hasExited {
            let code = pane.exitCode.map { Int($0) } ?? -1
            var exit: [String: Any] = ["exit_code": code > 128 ? -1 : code]
            if code != 0 { exit["error"] = code > 128 ? "signal: \(code - 128)" : "exit status \(code)" }
            report["last_exit"] = exit
        }
        guard let child = pane.childPID, ShellProcess.isAlive(child) else { return report }
        report["child"] = processInfo(child)
        let front = pane.foregroundPID ?? child
        report["foreground"] = processInfo(front)
        if front != child {
            var ancestors: [[String: Any]] = []
            var current = front
            while ancestors.count <= 16, let (parent, _) = ShellProcess.parentAndUser(of: current), parent > 1 {
                ancestors.append(processInfo(parent))
                if parent == child { break }
                current = parent
            }
            if ancestors.count <= 16, ancestors.last?["pid"] as? Int == Int(child) { report["ancestors"] = ancestors }
        }
        return report
    }

    static func statusRecords(of pane: PaneView) -> [[String: Any]] {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return pane.terminal.session.programStatus.sorted { $0.id < $1.id }.map { record in
            var entry: [String: Any] = ["state": record.state.rawValue]
            if !record.id.isEmpty { entry["id"] = record.id }
            if let kind = record.kind { entry["kind"] = kind.rawValue }
            if let progress = record.progress { entry["progress"] = Int(progress) }
            if let app = record.app { entry["app"] = app; entry["name"] = app }
            if let title = record.title { entry["title"] = title }
            if let msg = record.msg { entry["msg"] = msg }
            if let owner = pane.statusOwners[record.id] {
                if owner.pid > 0 { entry["owner"] = ["pid": Int(owner.pid), "name": owner.name] }
                entry["updated_at"] = formatter.string(from: owner.updated)
            }
            return entry
        }
    }

    // MARK: Input

    func keystroke(_ chord: KeyChord) -> Keystroke {
        var modifiers: KeyModifiers = []
        if chord.modifiers.contains(.control) { modifiers.insert(.control) }
        if chord.modifiers.contains(.option) { modifiers.insert(.alt) }
        if chord.modifiers.contains(.shift) { modifiers.insert(.shift) }
        if chord.modifiers.contains(.command) { modifiers.insert(.platform) }
        var key = chord.key
        if key.hasPrefix("["), let code = KeyChord.physicalKeys[String(key.dropFirst().dropLast())] {
            key = Self.physicalKeyNames[code] ?? key
        }
        if key.count == 1 {
            let text = chord.modifiers.contains(.shift) ? key.uppercased() : key
            let types = chord.modifiers.isDisjoint(with: [.control, .command, .option])
            return Keystroke(key: key, text: types ? text : nil, modifiers: modifiers)
        }
        return Keystroke(key: key, text: key == "space" ? " " : nil, modifiers: modifiers)
    }

    /// What a physical key types on a US layout, for sending it.
    static let physicalKeyNames: [UInt16: String] = {
        var names: [UInt16: String] = [36: "enter", 48: "tab", 49: "space", 51: "backspace", 53: "escape",
                                       123: "left", 124: "right", 125: "down", 126: "up"]
        for (name, code) in KeyChord.physicalKeys {
            if name.hasPrefix("Key") { names[code] = name.dropFirst(3).lowercased() }
            if name.hasPrefix("Digit") { names[code] = String(name.dropFirst(5)) }
        }
        let symbols: [String: String] = ["Equal": "=", "Minus": "-", "BracketRight": "]", "BracketLeft": "[", "Quote": "'",
                                         "Semicolon": ";", "Backslash": "\\", "Comma": ",", "Slash": "/", "Period": ".", "Backquote": "`"]
        for (name, symbol) in symbols { if let code = KeyChord.physicalKeys[name] { names[code] = symbol } }
        return names
    }()

    func send(_ chord: KeyChord, to pane: PaneView) {
        try? pane.terminal.session.send(keystroke(chord))
    }

    private func sendKeys(_ params: [String: Any], _ context: Context) throws -> [String: Any] {
        let target = try resolveAnyPane(params["block_id"] as? String, context)
        for raw in params["keys"] as? [String] ?? [] {
            guard let chord = KeyChord(raw) else { throw Failure("bad_request", "unknown key \(raw)") }
            do { try target.pane.terminal.session.send(keystroke(chord)) } catch {
                throw Failure("internal", "the terminal didn't take \(raw): \(error)")
            }
        }
        return [:]
    }

    /// `block.event`: a key press or a paste, as a client sends input.
    private func blockEvent(_ params: [String: Any], _ context: Context) throws -> [String: Any] {
        let target = try resolveAnyPane(params["block_id"] as? String, context)
        let event = params["event"] as? [String: Any] ?? params
        switch event["type"] as? String ?? "" {
        case "key":
            let chord = (event["key"] as? String).flatMap(KeyChord.init) ?? PluginClients.chord(fromRexKey: event)
            guard let chord else { throw Failure("bad_request", "event.key names no key") }
            if event["action"] as? String == "release" { return [:] }
            send(chord, to: target.pane)
        case "paste":
            try? target.pane.terminal.session.paste(event["text"] as? String ?? event["data"] as? String ?? "")
        case "text":
            try? target.pane.terminal.session.terminal.write(event["text"] as? String ?? "")
        default:
            throw Failure("bad_request", "event.type must be key, paste or text")
        }
        return [:]
    }

    /// Keeps a program's copy for `clipboard_read`, and tells plugins.
    func recordClipboard(_ text: String, from pane: PaneView) {
        let id = PluginBlockStore.shared.keepClipboard(Data(text.utf8))
        PluginEvents.blockEvent(pane, "selection_copied", ["id": id, "total": text.utf8.count])
    }
}
