import Darwin
import Foundation
import TermyNextCore

/// One request per connection over a Unix socket: a JSON line
/// `{"method", "params", "context"}` in, a JSON line `{"ok", "result" | "error"}`
/// out. The `rex` command in panes talks to the app this way.
enum PluginWire {
    static func send(_ request: [String: Any], to path: String, timeout: Double?) throws -> [String: Any] {
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { throw PluginCLI.Failure("can't open a socket") }
        defer { close(fd) }
        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        let bytes = Array(path.utf8)
        guard bytes.count < MemoryLayout.size(ofValue: address.sun_path) else { throw PluginCLI.Failure("socket path too long: \(path)") }
        withUnsafeMutableBytes(of: &address.sun_path) { raw in
            raw.copyBytes(from: bytes)
            raw[bytes.count] = 0
        }
        let connected = withUnsafePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) }
        }
        guard connected == 0 else {
            throw PluginCLI.Failure("Termy Next isn't running, or its plugin socket is missing (\(path)). Start Termy Next, or run this inside one of its panes.")
        }
        if let timeout {
            var value = timeval(tv_sec: Int(timeout), tv_usec: Int32((timeout - floor(timeout)) * 1_000_000))
            setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &value, socklen_t(MemoryLayout<timeval>.size))
        }
        var data = try JSONSerialization.data(withJSONObject: request, options: [.withoutEscapingSlashes])
        data.append(0x0A)
        try writeAll(fd, data)
        let reply = try readLine(fd, limit: 256 << 20)
        guard let object = try? JSONSerialization.jsonObject(with: reply) as? [String: Any] else {
            throw PluginCLI.Failure(reply.isEmpty ? "Termy Next didn't answer in time" : "Termy Next sent a reply that isn't JSON")
        }
        return object
    }

    /// Sends `request` and hands each line that comes back to `line` until
    /// the connection closes or `line` returns false.
    static func stream(_ request: [String: Any], to path: String, line: (Data) -> Bool) throws {
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { throw PluginCLI.Failure("can't open a socket") }
        defer { close(fd) }
        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        let bytes = Array(path.utf8)
        withUnsafeMutableBytes(of: &address.sun_path) { raw in
            raw.copyBytes(from: bytes.prefix(raw.count - 1))
            raw[min(bytes.count, raw.count - 1)] = 0
        }
        let connected = withUnsafePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) }
        }
        guard connected == 0 else { throw PluginCLI.Failure("Termy Next isn't running, or its plugin socket is missing (\(path)).") }
        var data = try JSONSerialization.data(withJSONObject: request, options: [.withoutEscapingSlashes])
        data.append(0x0A)
        try writeAll(fd, data)
        var buffer = [UInt8](repeating: 0, count: 65536)
        var pending = Data()
        while true {
            let n = read(fd, &buffer, buffer.count)
            if n < 0, errno == EINTR { continue }
            if n <= 0 { return }
            pending.append(contentsOf: buffer[0..<n])
            while let newline = pending.firstIndex(of: 0x0A) {
                let current = pending[pending.startIndex..<newline]
                pending = pending[(newline + 1)...]
                if !line(Data(current)) { return }
            }
        }
    }

    static func writeAll(_ fd: Int32, _ data: Data) throws {
        try data.withUnsafeBytes { raw in
            var offset = 0
            while offset < raw.count {
                let n = Darwin.write(fd, raw.baseAddress! + offset, raw.count - offset)
                if n < 0 {
                    if errno == EINTR { continue }
                    throw PluginCLI.Failure("the connection broke: \(String(cString: strerror(errno)))")
                }
                offset += n
            }
        }
    }

    static func readLine(_ fd: Int32, limit: Int) throws -> Data {
        var data = Data()
        var buffer = [UInt8](repeating: 0, count: 65536)
        while data.count < limit {
            let n = read(fd, &buffer, buffer.count)
            if n < 0, errno == EINTR { continue }
            if n <= 0 { break }
            if let newline = buffer[0..<n].firstIndex(of: 0x0A) {
                data.append(contentsOf: buffer[0..<newline])
                return data
            }
            data.append(contentsOf: buffer[0..<n])
        }
        return data
    }
}

/// What a `rex do` script printed, and what it returned or raised.
struct ScriptReply {
    var output: String
    var value: Any?
    var error: Error?
}

/// Answers the `rex` command (and anything else speaking `PluginWire`).
final class PluginServer: @unchecked Sendable {
    static let shared = PluginServer()
    private var listener: Int32 = -1

    /// A connection that stays open for events (`rex events`, `session.attach`).
    private final class Subscriber {
        let fd: Int32
        let clientID: String
        let session: String?
        let all: Bool
        let queue: DispatchQueue
        var closed = false
        let done = DispatchSemaphore(value: 0)
        init(fd: Int32, clientID: String, session: String?, all: Bool) {
            self.fd = fd
            self.clientID = clientID
            self.session = session
            self.all = all
            queue = DispatchQueue(label: "sh.termy.next.subscriber")
        }
    }

    private let lock = NSLock()
    private var subscribers: [Subscriber] = []

    var hasSubscribers: Bool { lock.withLock { !subscribers.isEmpty } }

    /// Server-wide events every subscriber hears, whatever its session.
    private static let global: Set<String> = [
        "session_created", "session_destroyed", "session_label_changed", "session_moved", "hosts_changed",
        "keymap_changed", "client_joined", "client_left", "client_changed", "tailscale_status_changed",
    ]

    /// Sends `event` to every connection listening for it.
    func broadcast(_ event: [String: Any]) {
        let targets = lock.withLock { subscribers }
        guard !targets.isEmpty, var line = try? JSONSerialization.data(withJSONObject: PluginRuntime.jsonSafe(event) ?? [:], options: [.withoutEscapingSlashes]) else { return }
        line.append(0x0A)
        let type = event["type"] as? String ?? ""
        let session = event["session_id"] as? String
        for subscriber in targets {
            let wanted = subscriber.all || Self.global.contains(type)
                || (subscriber.session != nil && subscriber.session == session)
            guard wanted else { continue }
            subscriber.queue.async { [weak self] in
                guard !subscriber.closed else { return }
                if (try? PluginWire.writeAll(subscriber.fd, line)) == nil { self?.drop(subscriber) }
            }
        }
    }

    private func drop(_ subscriber: Subscriber) {
        guard !subscriber.closed else { return }
        subscriber.closed = true
        lock.withLock { subscribers.removeAll { $0 === subscriber } }
        subscriber.done.signal()
        let id = subscriber.clientID
        DispatchQueue.main.async { MainActor.assumeIsolated { PluginClients.disconnect(id) } }
    }

    /// Keeps the connection open, sending events until the other end closes it.
    private func subscribe(_ client: Int32, params: [String: Any], context: [String: Any]) {
        let session = params["session_id"] as? String
        let all = params["all"] as? Bool ?? false
        let clientID = PluginID.make("client", UUID())
        // The session as the app names it, for matching events.
        let sessionID: String? = session.flatMap { raw in
            onMain { PluginAPI.shared.findSession(raw).map { PluginID.make("session", $0.id) } }
        }
        if session != nil, sessionID == nil {
            if var data = try? JSONSerialization.data(withJSONObject: ["ok": false, "error": ["code": "session_not_found", "message": "no session \(session!)"]]) {
                data.append(0x0A)
                try? PluginWire.writeAll(client, data)
            }
            return
        }
        let subscriber = Subscriber(fd: client, clientID: clientID, session: sessionID, all: all)
        onMain {
            var info = PluginClients.Client(id: clientID, kind: context["client_kind"] as? String ?? "cli", name: "rex")
            if let sessionID { info.sessions.insert(sessionID) }
            info.attachedAll = all
            PluginClients.connect(info)
            if let sessionID { PluginEvents.emit("client_connected", ["client_id": clientID, "session_id": sessionID]) }
        }
        lock.withLock { subscribers.append(subscriber) }
        if var data = try? JSONSerialization.data(withJSONObject: ["ok": true, "result": ["client_id": clientID, "subscribed": true]]) {
            data.append(0x0A)
            try? PluginWire.writeAll(client, data)
        }
        // Wait for the other end to hang up.
        var byte: UInt8 = 0
        while !subscriber.closed {
            let n = read(client, &byte, 1)
            if n <= 0, !(n < 0 && errno == EINTR) { break }
        }
        subscriber.queue.sync { drop(subscriber) }
    }

    /// Streams a block's screen for a `data.connect` ticket: a JSON line with
    /// the content whenever it changes, until either end closes.
    private func stream(_ client: Int32, params: [String: Any]) {
        let ticket = params["ticket"] as? String ?? ""
        let found: (UUID, String)? = onMain {
            guard let entry = PluginBlockStore.shared.dataTickets.removeValue(forKey: ticket), entry.expires > Date() else { return nil }
            return (entry.pane, entry.subprotocol)
        }
        guard let (paneID, subprotocol) = found else {
            if var data = try? JSONSerialization.data(withJSONObject: ["ok": false, "error": ["code": "not_found", "message": "unknown or used ticket"]]) {
                data.append(0x0A)
                try? PluginWire.writeAll(client, data)
            }
            return
        }
        let format = params["format"] as? String ?? "vt"
        if var data = try? JSONSerialization.data(withJSONObject: ["ok": true, "result": ["subprotocol": subprotocol]]) {
            data.append(0x0A)
            guard (try? PluginWire.writeAll(client, data)) != nil else { return }
        }
        var last: String?
        var flags = fcntl(client, F_GETFL)
        flags |= O_NONBLOCK
        _ = fcntl(client, F_SETFL, flags)
        while true {
            var byte: UInt8 = 0
            let n = read(client, &byte, 1)
            if n == 0 || (n < 0 && errno != EAGAIN && errno != EINTR) { return }
            let content: String?? = onMain {
                guard let pane = PluginAPI.shared.findAnyPane(PluginID.make("block", paneID))?.pane else { return .some(nil) }
                let screen = try? PluginAPI.shared.terminalMethod("format", ["format": format], pane: pane) as? [String: Any]
                return .some(screen?["content"] as? String)
            }
            guard let current = content ?? nil else {
                if var data = try? JSONSerialization.data(withJSONObject: ["type": "closed"]) { data.append(0x0A); try? PluginWire.writeAll(client, data) }
                return
            }
            if current != last {
                last = current
                guard var data = try? JSONSerialization.data(withJSONObject: ["type": "screen", "content": current]) else { return }
                data.append(0x0A)
                if (try? PluginWire.writeAll(client, data)) == nil { return }
            }
            Thread.sleep(forTimeInterval: 0.1)
        }
    }

    func start() {
        guard listener < 0 else { return }
        let path = PluginPaths.socket
        try? FileManager.default.createDirectory(atPath: (path as NSString).deletingLastPathComponent, withIntermediateDirectories: true)
        unlink(path)
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { return PluginLog.write("server: can't open a socket") }
        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        let bytes = Array(path.utf8)
        guard bytes.count < MemoryLayout.size(ofValue: address.sun_path) else { close(fd); return }
        withUnsafeMutableBytes(of: &address.sun_path) { raw in
            raw.copyBytes(from: bytes)
            raw[bytes.count] = 0
        }
        let bound = withUnsafePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { bind(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) }
        }
        guard bound == 0, listen(fd, 32) == 0 else {
            PluginLog.write("server: can't listen on \(path): \(String(cString: strerror(errno)))")
            close(fd)
            return
        }
        chmod(path, 0o600)
        listener = fd
        let thread = Thread { [weak self] in self?.acceptLoop(fd) }
        thread.name = "sh.termy.next.plugin-server"
        thread.start()
    }

    private func acceptLoop(_ fd: Int32) {
        while true {
            let client = accept(fd, nil, nil)
            if client < 0 {
                if errno == EINTR { continue }
                return
            }
            // Only programs of this user may drive the app.
            var uid: uid_t = 0, gid: gid_t = 0
            guard getpeereid(client, &uid, &gid) == 0, uid == getuid() else {
                close(client)
                continue
            }
            DispatchQueue.global(qos: .userInitiated).async { [weak self] in
                self?.serve(client)
                close(client)
            }
        }
    }

    private func serve(_ client: Int32) {
        let reply: [String: Any]
        do {
            let line = try PluginWire.readLine(client, limit: 64 << 20)
            guard let request = try JSONSerialization.jsonObject(with: line) as? [String: Any],
                  let method = request["method"] as? String
            else { throw PluginAPI.Failure("bad_request", "a request is a JSON object with a method") }
            let params = request["params"] as? [String: Any] ?? [:]
            let context = request["context"] as? [String: Any] ?? [:]
            switch method {
            case "events.subscribe": return subscribe(client, params: params, context: context)
            case "data.open": return stream(client, params: params)
            default: break
            }
            let answer = try handle(method, params, context)
            if let script = answer as? ScriptReply {
                if let error = script.error {
                    let message = (error as? LuaError)?.description ?? (error as? PluginAPI.Failure)?.message ?? "\(error)"
                    reply = ["ok": false, "output": script.output, "error": ["code": "internal", "message": message]]
                } else {
                    reply = ["ok": true, "output": script.output, "result": script.value ?? NSNull()]
                }
            } else {
                reply = ["ok": true, "result": answer ?? NSNull()]
            }
        } catch let failure as PluginAPI.Failure {
            reply = ["ok": false, "error": ["code": failure.code, "message": failure.message]]
        } catch {
            reply = ["ok": false, "error": ["code": "internal", "message": (error as? LuaError)?.description ?? "\(error)"]]
        }
        let safe = JSONSerialization.isValidJSONObject(reply) ? reply : ["ok": false, "error": ["code": "internal", "message": "the result can't be sent as JSON"]]
        if var data = try? JSONSerialization.data(withJSONObject: safe, options: [.withoutEscapingSlashes]) {
            data.append(0x0A)
            try? PluginWire.writeAll(client, data)
        }
    }

    /// Waits on this connection's thread for work done elsewhere.
    private func waitFor<T>(_ start: (@escaping @Sendable (T) -> Void) -> Void) -> T {
        let done = DispatchSemaphore(value: 0)
        nonisolated(unsafe) var value: T?
        start { result in
            value = result
            done.signal()
        }
        done.wait()
        return value!
    }

    private func handle(_ method: String, _ params: [String: Any], _ raw: [String: Any]) throws -> Any? {
        let context = PluginAPI.Context(json: raw)
        let host = PluginHost.shared
        nonisolated(unsafe) let params = params
        switch method {
        case "config.check":
            return waitFor { done in host.check(path: params["path"] as? String, completion: done) }
        case "server.config_reload":
            let snapshot: PluginSnapshot = waitFor { done in host.reload(completion: done) }
            return ["generation": snapshot.generation, "diagnostics": snapshot.diagnostics,
                    "actions": snapshot.actions.count, "bindings": snapshot.bindings.count]
        case "client.keymap":
            return onMain { host.keymap() }
        case "client.set_script":
            guard params["session_id"] == nil, params["client_id"] == nil else {
                throw PluginAPI.Failure("bad_request", "client.set_script takes no session_id or client_id")
            }
            let scripts = (params["scripts"] as? [[String: Any]] ?? []).map { ($0["name"] as? String ?? "script", $0["source"] as? String ?? "") }
            let result: Result<[String: Any], Error> = waitFor { done in host.setClientScripts(scripts) { r in nonisolated(unsafe) let s = r; done(s) } }
            return try result.get()
        case "client.key":
            return try key(params, context)
        case "client.action", "client.perform":
            guard let action = params["action"] as? String else { throw PluginAPI.Failure("bad_request", "action is required") }
            let ctx = try actionContext(params, context)
            let args = params["args"] as? [String: Any] ?? [:]
            nonisolated(unsafe) let sentCtx = ctx, sentArgs = args
            let result: Result<Any?, Error> = waitFor { done in
                DispatchQueue.main.async {
                    MainActor.assumeIsolated {
                        host.perform(action: action, args: sentArgs, context: sentCtx) { result in
                            nonisolated(unsafe) let sent = result
                            done(sent)
                        }
                    }
                }
            }
            return try PluginRuntime.jsonSafe(result.get())
        case "termy.run_script":
            guard let code = params["code"] as? String else { throw PluginAPI.Failure("bad_request", "code is required") }
            let ctx = try actionContext(params, context)
            nonisolated(unsafe) let sentCtx = ctx
            let (result, output): (Result<Any?, Error>, String) = waitFor { done in
                host.runScript(code: code, name: params["name"] as? String ?? "script", path: params["path"] as? String,
                               args: params["args"] as? [String: Any] ?? [:], context: sentCtx,
                               action: params["action"] as? String) { result, output in
                    nonisolated(unsafe) let sent = (result, output)
                    done(sent)
                }
            }
            // The command prints what the script printed, then its result or error.
            switch result {
            case .success(let value): return ScriptReply(output: output, value: PluginRuntime.jsonSafe(value), error: nil)
            case .failure(let error): return ScriptReply(output: output, value: nil, error: error)
            }
        case "block.wait":
            let deadline = Date().addingTimeInterval((params["timeout"] as? NSNumber)?.doubleValue ?? 86_400)
            while Date() < deadline {
                let state = try onMain { try PluginAPI.shared.call("block.state", params, context: context) } as? [String: Any] ?? [:]
                if state["exists"] as? Bool != true || state["exited"] as? Bool == true {
                    var result: [String: Any] = ["ended": true, "block_id": params["block_id"] ?? NSNull()]
                    if let code = state["exit_code"] { result["exit_code"] = code }
                    return result
                }
                Thread.sleep(forTimeInterval: 0.25)
            }
            return ["ended": false, "timed_out": true]
        default:
            return try onMain { () throws -> Any? in try PluginAPI.shared.call(method, params, context: context) }
        }
    }

    /// `client.key`: the functions bound to the key get it first, then the block.
    private func key(_ params: [String: Any], _ context: PluginAPI.Context) throws -> [String: Any] {
        nonisolated(unsafe) let params = params
        let resolved: (PluginBindingTarget?, [String: Any], KeyChord, PaneView?) = try onMain {
            let api = PluginAPI.shared
            let session = try api.session(params, context)
            let chord: KeyChord? = (params["key"] as? String).flatMap(KeyChord.init)
                ?? (params["key"] as? [String: Any]).flatMap(PluginClients.chord(fromRexKey:))
            guard let chord else { throw PluginAPI.Failure("bad_request", "key names no key") }
            var prefix: [KeyChord] = []
            for raw in params["prefix"] as? [String] ?? [] {
                guard let chord = KeyChord(raw) else { throw PluginAPI.Failure("bad_request", "prefix element \(raw) isn't a key") }
                prefix.append(chord)
            }
            let target = try? api.resolveAnyPane(params["block_id"] as? String, PluginAPI.Context(sessionID: PluginID.make("session", session.id)))
            var ctx: [String: Any] = ["origin": "key", "client_id": context.clientID ?? "client:cli", "session_id": PluginID.make("session", session.id)]
            if let target { ctx["block_id"] = PluginID.make("block", target.pane.id) }
            let binding = PluginHost.shared.snapshot.binding(mode: params["mode"] as? String, keys: prefix + [chord])
            if case .function = binding?.target {
                return (binding?.target, ctx, chord, target?.pane)
            }
            guard prefix.isEmpty || binding == nil else { return (nil, ctx, chord, nil) }
            return (nil, ctx, chord, target?.pane)
        }
        if case .function(let id, let client)? = resolved.0 {
            let taken = try PluginHost.shared.pressAndWait(id, client: client, context: resolved.1, key: ["key": resolved.2.description])
            if taken { return ["handled": true] }
        }
        nonisolated(unsafe) let pane = resolved.3
        let chord = resolved.2
        guard let pane else { return ["handled": false] }
        onMain { PluginAPI.shared.send(chord, to: pane) }
        return ["handled": true]
    }

    /// `ctx` for an action run by `rex do`: the session named, else the
    /// caller's; the block named, else that session's focused one.
    private func actionContext(_ params: [String: Any], _ context: PluginAPI.Context) throws -> [String: Any] {
        nonisolated(unsafe) let params = params
        return try onMain {
            let api = PluginAPI.shared
            let session = try api.session(params, context)
            var ctx: [String: Any] = ["origin": params["origin"] as? String ?? context.origin, "client_id": "client:termy-next",
                                      "session_id": PluginID.make("session", session.id)]
            if let located = try? api.resolvePane(params["block_id"] as? String, context, session: session) {
                ctx["block_id"] = PluginID.make("block", located.pane.id)
                ctx["window_id"] = PluginID.make("window", located.tab.id)
            }
            return ctx
        }
    }
}
