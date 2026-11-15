import AppKit
import CLua
import TermyNextCore

/// An action a plugin defined with `termy.action{…}` (or `rex.action{…}`),
/// as the command palette and key bindings see it.
struct PluginActionInfo: Sendable, Equatable {
    struct Argument: Sendable, Equatable {
        var name: String
        var type: String
        var optional: Bool
        var description: String?
    }

    var name: String
    var title: String
    var description: String?
    var category: String?
    var keywords: String?
    var arguments: [Argument]
    var hidden: Bool
    /// Where it was defined, `init.lua:12`.
    var source: String = ""
}

/// What a key binding runs: an action by name (a plugin's, or one of Termy
/// Next's own like `pane.zoom`, with its arguments as JSON), or a Lua
/// function of the config or of the client's scripts.
enum PluginBindingTarget: Sendable, Equatable {
    case action(String, args: String?)
    case function(Int, client: Bool)
}

/// One `rex.bind`: its keys (perhaps a sequence, perhaps in a mode) and what they run.
struct PluginBinding: Sendable, Equatable {
    var spec: KeySpec
    var target: PluginBindingTarget
    var source: String
    /// From `client.set_script` rather than the config files.
    var client = false

    var json: [String: Any] {
        var entry: [String: Any] = ["key": spec.keys, "keys": spec.chords.map(\.description), "source": source]
        if let mode = spec.mode { entry["mode"] = mode }
        switch target {
        case .action(let name, let args):
            entry["action"] = name
            if let args, let data = args.data(using: .utf8), let value = try? JSONSerialization.jsonObject(with: data) { entry["args"] = value }
        case .function:
            entry["action"] = "client.key.forward"
        }
        return entry
    }
}

/// A key mode `rex.mode` declares: its bindings apply while a client is in it.
struct PluginMode: Sendable, Equatable {
    var name: String
    /// Keys the mode doesn't bind do nothing while it's on.
    var exclusive: Bool
    var source: String
}

/// The plugins' current actions and bindings, for the main thread.
struct PluginSnapshot: Sendable {
    var actions: [PluginActionInfo] = []
    /// Config bindings in the order they were made; client scripts' after them.
    var bindings: [PluginBinding] = []
    /// Keys `rex.unbind` took from Termy Next's own shortcuts.
    var removals: [KeySpec] = []
    var modes: [PluginMode] = []
    var diagnostics: [String] = []
    var generation = 0
    var files: [String] = []
    var declaredHostsJSON = Data("[]".utf8)

    var declaredHosts: [[String: Any]] {
        (try? JSONSerialization.jsonObject(with: declaredHostsJSON)) as? [[String: Any]] ?? []
    }

    func action(named name: String) -> PluginActionInfo? { actions.first { $0.name == name } }

    /// The keys bound to `action` outside any mode, for the palette.
    func chord(for action: String) -> KeySpec? {
        bindings.filter { $0.spec.mode == nil && { if case .action(action, _) = $0.target { return true } else { return false } }($0) }
            .map(\.spec).min { $0.description < $1.description }
    }

    /// The binding for exactly these keys in `mode`: client scripts first, then the latest made.
    func binding(mode: String?, keys: [KeyChord]) -> PluginBinding? {
        bindings.last { $0.spec.mode == mode && $0.spec.chords == keys && $0.client }
            ?? bindings.last { $0.spec.mode == mode && $0.spec.chords == keys }
    }

    /// Whether a longer sequence in `mode` starts with these keys.
    func startsSequence(mode: String?, keys: [KeyChord]) -> Bool {
        bindings.contains { $0.spec.mode == mode && $0.spec.chords.count > keys.count && Array($0.spec.chords.prefix(keys.count)) == keys }
    }

    func mode(_ name: String) -> PluginMode? { modes.last { $0.name == name } }
}

extension KeySpec {
    /// As menus draw it, ⌃B C for a sequence.
    var symbolLabel: String { (mode.map { "\($0): " } ?? "") + chords.map(\.symbolLabel).joined(separator: " ") }
}

/// Runs Lua plugins: Termy Next's own config (`~/.config/termy-next/init.lua`,
/// written with `termy.*`), then Rex's (`~/.config/rex/init.lua`, where
/// `leaf rex install` and other Rex plugins put themselves). Lua runs on a
/// queue of its own; it reaches the app through `PluginAPI` on the main
/// thread, and the main thread never waits for Lua.
final class PluginHost: @unchecked Sendable {
    static let shared = PluginHost()
    static let didChange = Notification.Name("TermyNextPluginsDidChange")

    let queue = DispatchQueue(label: "sh.termy.next.plugins", qos: .userInitiated)
    private var config: PluginRuntime?
    private var generation = 0
    private var lastLoadWorked = false
    /// Scripts the app client loaded with `client.set_script`.
    private var clientScripts: PluginRuntime?

    @MainActor private(set) var snapshot = PluginSnapshot()

    private let lock = NSLock()
    private var runtimes: [ObjectIdentifier: PluginRuntime] = [:]
    private var waiters: [Waiter] = []
    private var anyHandlers = false

    final class Waiter {
        let type: String
        let filter: [String: Any]
        let semaphore = DispatchSemaphore(value: 0)
        var event: [String: Any]?
        init(type: String, filter: [String: Any]) {
            self.type = type
            self.filter = filter
        }
    }

    /// Whether any plugin listens for events, so panes don't build events nobody hears.
    var hasHandlers: Bool { lock.withLock { anyHandlers || !waiters.isEmpty } }

    /// The config files loaded, in order.
    var configFiles: [URL] {
        var files = [PluginPaths.termyConfig]
        if Self.loadsRexConfig { files.append(PluginPaths.rexConfig) }
        return files
    }

    /// Settings → General → "Load Rex plugins"; on unless turned off.
    static var loadsRexConfig: Bool {
        UserDefaults.standard.object(forKey: "loadsRexPlugins") as? Bool ?? true
    }

    // MARK: Loading

    /// Starts the plugins at launch.
    @MainActor func start() {
        PluginPaths.installShim()
        PluginAPI.shared.start()
        PluginServer.shared.start()
        reload()
    }

    /// Reads the config files again, as `rex config reload` does. Actions and
    /// bindings are replaced; detached programs keep running.
    func reload(completion: (@Sendable (PluginSnapshot) -> Void)? = nil) {
        queue.async { [self] in
            let files = configFiles
            generation += 1
            let runtime = PluginRuntime(kind: .config, queue: queue, host: self)
            var diagnostics: [String] = []
            for file in files where FileManager.default.fileExists(atPath: file.path) {
                if let problem = runtime.load(file: file) {
                    diagnostics.append(problem)
                    PluginLog.write("config: \(problem)")
                }
            }
            // A file that doesn't load leaves the last load that worked in
            // place, as Rex does; the problem is still reported.
            let kept = !diagnostics.isEmpty && config != nil && lastLoadWorked
            if kept {
                runtime.teardown()
            } else {
                config?.teardown()
                unregister(config)
                config = runtime
                register(runtime)
                lastLoadWorked = diagnostics.isEmpty
            }
            diagnostics += runtime.diagnostics.map { "\($0.source): \($0.message)" }
            var snapshot = config?.snapshot() ?? runtime.snapshot()
            if let client = clientScripts { snapshot.bindings += client.snapshot().bindings.map { var b = $0; b.client = true; return b } }
            snapshot.diagnostics = diagnostics
            snapshot.generation = generation
            snapshot.files = files.map(\.path)
            let published = snapshot
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    let before = PluginHost.shared.keymapHash
                    let hostsBefore = PluginHost.shared.snapshot.declaredHostsJSON
                    PluginHost.shared.snapshot = published
                    NotificationCenter.default.post(name: PluginHost.didChange, object: nil)
                    PluginEvents.emit("keymap_changed", ["generation": published.generation, "hash": PluginHost.shared.keymapHash,
                                                         "changed": before != PluginHost.shared.keymapHash])
                    if hostsBefore != published.declaredHostsJSON { PluginEvents.emit("hosts_changed", ["hash": PluginHosts.hash]) }
                }
                completion?(published)
            }
        }
    }

    /// Loads `path` on its own, without changing what's loaded, and says
    /// what it found: `rex config check`.
    func check(path: String?, completion: @escaping @Sendable ([String: Any]) -> Void) {
        let file = path.map { URL(fileURLWithPath: $0) } ?? PluginPaths.rexConfig
        let checkQueue = DispatchQueue(label: "sh.termy.next.plugins.check")
        checkQueue.async { [self] in
            var report: [String: Any] = ["path": file.path, "exists": FileManager.default.fileExists(atPath: file.path)]
            if report["exists"] as? Bool == true {
                let runtime = PluginRuntime(kind: .check, queue: checkQueue, host: self)
                let problem = runtime.load(file: file)
                let snapshot = runtime.snapshot()
                report["actions"] = snapshot.actions.count
                report["bindings"] = snapshot.bindings.count
                report["removals"] = snapshot.removals.count
                report["modes"] = snapshot.modes.count
                let problems = (problem.map { [$0] } ?? []) + runtime.diagnostics.map { "\($0.source): \($0.message)" }
                if !problems.isEmpty { report["diagnostics"] = problems }
                runtime.teardown()
            }
            completion(report)
        }
    }

    // MARK: Running

    /// Runs the action `name` with `args` in `context`, the way the palette,
    /// a key or `rex do` does. The result arrives on the main thread.
    func perform(action name: String, args: [String: Any], context: [String: Any],
                 completion: (@MainActor (Result<Any?, Error>) -> Void)? = nil) {
        queue.async { [self] in
            guard let config else {
                DispatchQueue.main.async { MainActor.assumeIsolated { completion?(.failure(LuaError("plugins aren't loaded"))) } }
                return
            }
            let result = Result { try config.invoke(name, args: args, context: context) }
            nonisolated(unsafe) let sent = result
            DispatchQueue.main.async { MainActor.assumeIsolated { completion?(sent) } }
        }
    }

    /// Runs what a key is bound to. A function that doesn't take the key
    /// (returns false or nothing) hands it on with `unhandled`.
    func press(_ target: PluginBindingTarget, context: [String: Any], key: [String: Any] = [:],
               unhandled: (@MainActor () -> Void)? = nil) {
        switch target {
        case .action(let name, let args):
            let parsed = args.flatMap { try? JSONSerialization.jsonObject(with: Data($0.utf8)) as? [String: Any] } ?? [:]
            perform(action: name, args: parsed, context: context) { result in
                if case .failure(let error) = result { PluginHost.report(error) }
            }
        case .function(let id, let client):
            nonisolated(unsafe) let sentKey = key
            queue.async { [self] in
                guard let runtime = client ? clientScripts : config else { return }
                do {
                    let taken = try runtime.callBinding(id, context: context, key: sentKey)
                    if !taken, let unhandled { DispatchQueue.main.async { MainActor.assumeIsolated { unhandled() } } }
                } catch {
                    DispatchQueue.main.async { MainActor.assumeIsolated { PluginHost.report(error) } }
                }
            }
        }
    }

    /// `client.key`: runs the function bound to these keys and says, within
    /// 5 seconds, whether it took the key. Called off the main thread.
    func pressAndWait(_ id: Int, client: Bool, context: [String: Any], key: [String: Any]) throws -> Bool {
        let done = DispatchSemaphore(value: 0)
        nonisolated(unsafe) var outcome: Result<Bool, Error> = .success(false)
        nonisolated(unsafe) let sent = (context, key)
        queue.async { [self] in
            outcome = Result {
                guard let runtime = client ? clientScripts : config else { return false }
                return try runtime.callBinding(id, context: sent.0, key: sent.1)
            }
            done.signal()
        }
        guard done.wait(timeout: .now() + 5) == .success else { throw LuaError("the key's function ran longer than 5 seconds") }
        return try outcome.get()
    }

    /// `client.set_script`: loads scripts for the app client, replacing the
    /// ones before. A script that fails leaves the previous ones running.
    func setClientScripts(_ scripts: [(name: String, source: String)], completion: @escaping @Sendable (Result<[String: Any], Error>) -> Void) {
        queue.async { [self] in
            let runtime = PluginRuntime(kind: .config, queue: queue, host: self)
            for script in scripts {
                do {
                    _ = try runtime.run(code: script.source, name: "=" + script.name)
                } catch {
                    runtime.teardown()
                    completion(.failure(PluginAPI.Failure("bad_request", (error as? LuaError)?.description ?? "\(error)")))
                    return
                }
            }
            clientScripts?.teardown()
            unregister(clientScripts)
            clientScripts = runtime
            register(runtime)
            let bindings = runtime.snapshot().bindings
            completion(.success([
                "bindings": bindings.filter { if case .function = $0.target { return true } else { return false } }.map(\.spec.description),
                "all_keys": bindings.map(\.spec.description),
                "diagnostics": runtime.diagnostics.map { ["severity": "error", "source": $0.source, "message": $0.message] },
            ]))
            reload()
        }
    }

    /// Runs a `rex do` script in a runtime of its own; actions it defines are
    /// its own, and others go to the config's.
    func runScript(code: String, name: String, path: String?, args: [String: Any], context: [String: Any],
                   action: String?, completion: @escaping @Sendable (Result<Any?, Error>, String) -> Void) {
        let scriptQueue = DispatchQueue(label: "sh.termy.next.plugins.script")
        nonisolated(unsafe) let sentArgs = args, sentContext = context
        // Lua runs on the script's queue; this thread only waits, so timers
        // and handlers can run while the script listens.
        DispatchQueue.global(qos: .userInitiated).async { [self] in
            let runtime = scriptQueue.sync { () -> PluginRuntime in
                let runtime = PluginRuntime(kind: .script, queue: scriptQueue, host: self, scriptArgs: sentArgs)
                runtime.context = sentContext
                return runtime
            }
            register(runtime)
            var result: Result<Any?, Error> = scriptQueue.sync {
                do {
                    let returned = try runtime.run(code: code, name: path.map { "@" + $0 } ?? "=" + name)
                    if let action { return .success(try runtime.invoke(action, args: sentArgs, context: sentContext)) }
                    return .success(returned)
                } catch let error as LuaError where error.description.contains(PluginRuntime.stopMarker) {
                    let code = error.description.components(separatedBy: PluginRuntime.stopMarker).last ?? ""
                    return .success(["stopped": Int(code.prefix { $0.isNumber || $0 == "-" }) ?? 0])
                } catch {
                    return .failure(error)
                }
            }
            // A script listening for events runs until rex.stop, or --for.
            if case .success = result, scriptQueue.sync(execute: { runtime.keepsRunning && runtime.stopCode == nil }) {
                let seconds = (sentArgs["for"] as? NSNumber)?.doubleValue ?? (sentContext["for"] as? NSNumber)?.doubleValue ?? 86_400
                runtime.waitForStop(seconds: seconds)
                if let code = scriptQueue.sync(execute: { runtime.stopCode }) { result = .success(["stopped": code]) }
            }
            let output = scriptQueue.sync { () -> String in
                let lines = runtime.printed
                runtime.teardown()
                return lines.map { $0 + "\n" }.joined()
            }
            unregister(runtime)
            completion(result, output)
        }
    }

    /// The action list and bindings, for `rex actions` and `rex keymap`.
    @MainActor func keymap() -> [String: Any] {
        var actions: [[String: Any]] = snapshot.actions.map { action in
            var entry: [String: Any] = ["name": action.name, "kind": "lua", "title": action.title, "source": action.source]
            if let description = action.description { entry["description"] = description }
            if let category = action.category { entry["category"] = category }
            if !action.arguments.isEmpty {
                entry["args"] = Dictionary(uniqueKeysWithValues: action.arguments.map { ($0.name, $0.type + ($0.optional ? "?" : "")) })
            }
            return entry
        }
        actions += PluginAPI.blockMethods.map { ["name": PluginFlavor.terminalCreator + "." + $0, "kind": "block"] }
        let bindings = snapshot.bindings.map(\.json)
        let modes: [[String: Any]] = snapshot.modes.map { mode in
            let blocked = snapshot.removals.filter { $0.mode == mode.name }.map(\.keys)
            return ["name": mode.name, "exclusive": mode.exclusive, "source": mode.source, "blocked": blocked]
        }
        let config = PluginPaths.rexConfig
        return [
            "actions": actions, "bindings": bindings, "modes": modes,
            "removals": snapshot.removals.map { removal -> [String: Any] in
                var entry: [String: Any] = ["keys": removal.chords.map(\.description)]
                if let mode = removal.mode { entry["mode"] = mode }
                return entry
            },
            "diagnostics": snapshot.diagnostics.map { ["severity": "error", "message": $0] },
            "config": ["path": config.path, "paths": snapshot.files, "exists": FileManager.default.fileExists(atPath: config.path),
                       "generation": snapshot.generation, "instance_id": PluginAPI.hostID, "hash": keymapHash],
        ]
    }

    @MainActor var keymapHash: String {
        let text = snapshot.bindings.map { $0.spec.description + "\($0.target)" }.joined(separator: "\n")
            + snapshot.modes.map(\.name).joined(separator: ",")
        var value: UInt64 = 0xcbf2_9ce4_8422_2325
        for byte in text.utf8 { value = (value ^ UInt64(byte)) &* 0x100_0000_01b3 }
        return String(format: "%016llx", value)
    }

    /// Shows a failed action's message in the front window.
    @MainActor static func report(_ error: Error) {
        let message = (error as? LuaError)?.description ?? error.localizedDescription
        PluginLog.write("action failed: \(message)")
        if let controller = AppDelegate.shared.keyController ?? AppDelegate.shared.controllersFrontToBack.first {
            controller.pluginShowError(message)
        }
    }

    // MARK: Events

    fileprivate func register(_ runtime: PluginRuntime) {
        lock.withLock { runtimes[ObjectIdentifier(runtime)] = runtime }
    }

    fileprivate func unregister(_ runtime: PluginRuntime?) {
        guard let runtime else { return }
        lock.withLock {
            runtimes[ObjectIdentifier(runtime)] = nil
            anyHandlers = runtimes.values.contains { $0.listens }
        }
    }

    fileprivate func handlersChanged() {
        lock.withLock { anyHandlers = runtimes.values.contains { $0.listens } }
    }

    /// Hands `event` to every `on` handler and `wait` that matches it.
    func dispatch(event: [String: Any]) {
        let (targets, matched): ([PluginRuntime], [Waiter]) = lock.withLock {
            let matched = waiters.filter { PluginRuntime.matches(event, type: $0.type, filter: $0.filter) }
            waiters.removeAll { waiter in matched.contains { $0 === waiter } }
            return (runtimes.values.filter(\.listens), matched)
        }
        for waiter in matched {
            waiter.event = event
            waiter.semaphore.signal()
        }
        nonisolated(unsafe) let sent = event
        for runtime in targets { runtime.queue.async { runtime.deliver(sent) } }
    }

    fileprivate func wait(type: String, filter: [String: Any], timeout: Double) -> [String: Any]? {
        let waiter = Waiter(type: type, filter: filter)
        lock.withLock { waiters.append(waiter) }
        if waiter.semaphore.wait(timeout: .now() + timeout) == .timedOut {
            lock.withLock { waiters.removeAll { $0 === waiter } }
            return nil
        }
        return waiter.event
    }
}

/// Calls the app on the main thread from Lua's queue.
func onMain<T>(_ body: @MainActor () throws -> T) rethrows -> T {
    if Thread.isMainThread { return try MainActor.assumeIsolated(body) }
    return try DispatchQueue.main.sync { try MainActor.assumeIsolated(body) }
}

// MARK: - Runtime

/// One Lua state with the plugin API: the loaded config, a `rex do` script,
/// or a `rex config check`.
final class PluginRuntime: @unchecked Sendable {
    enum Kind { case config, script, check }

    static let stopMarker = "\u{1}termy-stop:"

    let kind: Kind
    let queue: DispatchQueue
    let vm: LuaVM
    private unowned let host: PluginHost

    struct Action {
        var info: PluginActionInfo
        var run: LuaRef
    }

    private var actions: [String: Action] = [:]
    private var actionOrder: [String] = []
    private var bindings: [PluginBinding] = []
    private var bindingFunctions: [Int: LuaRef] = [:]
    private var removals: [KeySpec] = []
    private var modes: [PluginMode] = []
    private var declaredHosts: [[String: Any]] = []
    /// Problems that didn't stop the load, such as a bad key.
    private(set) var diagnostics: [(source: String, message: String)] = []
    private var handlers: [(id: Int, type: String, filter: [String: Any], function: LuaRef)] = []
    private var timers: [Int: DispatchSourceTimer] = [:]
    private var processes: [Int: ManagedProcess] = [:]
    private var nextID = 1
    private var tornDown = false
    private let stopped = DispatchSemaphore(value: 0)
    private(set) var stopCode: Int?

    /// The action's `ctx` while it runs; session calls default to its session.
    var context: [String: Any] = [:]
    var scriptArgs: [String: Any] = [:]
    /// What a `rex do` script printed, for the command to show.
    private(set) var printed: [String] = []
    /// Client actions an action queued with `rex.client.queue`, run after it.
    private var queued: [(String, [String: Any])] = []

    var listens: Bool { !handlers.isEmpty }
    var keepsRunning: Bool { !handlers.isEmpty || !timers.isEmpty }

    init(kind: Kind, queue: DispatchQueue, host: PluginHost, scriptArgs: [String: Any] = [:]) {
        self.kind = kind
        self.queue = queue
        self.host = host
        self.scriptArgs = scriptArgs
        vm = LuaVM(queue: queue)
        install()
    }

    func teardown() {
        guard !tornDown else { return }
        tornDown = true
        timers.values.forEach { $0.cancel() }
        timers.removeAll()
        for process in processes.values where !process.detached { process.terminate() }
        processes.removeAll()
        handlers.removeAll()
        actions.removeAll()
        bindingFunctions.removeAll()
        stopped.signal()
        vm.close()
    }

    func snapshot() -> PluginSnapshot {
        var snapshot = PluginSnapshot(actions: actionOrder.compactMap { actions[$0]?.info }, bindings: bindings,
                                      removals: removals, modes: modes)
        snapshot.declaredHostsJSON = (try? JSONSerialization.data(withJSONObject: declaredHosts)) ?? Data("[]".utf8)
        return snapshot
    }

    /// `init.lua:12`: where the Lua that called a Swift function is.
    private func source(_ lua: Lua) -> String {
        var raw: UnsafePointer<CChar>?
        var line: Int32 = 0
        guard clua_caller(lua.state, 1, &raw, &line) != 0, let raw else { return "" }
        var name = String(cString: raw)
        // "@/path/init.lua" is a file, "=-e" a name as given.
        if name.hasPrefix("@") || name.hasPrefix("=") { name.removeFirst() }
        if name.hasPrefix("/") { name = (name as NSString).lastPathComponent }
        return line > 0 ? "\(name):\(line)" : name
    }

    // MARK: Loading

    /// Runs `file` with `require` finding modules beside it; returns the
    /// problem when it fails.
    func load(file: URL) -> String? {
        guard let code = try? String(contentsOf: file, encoding: .utf8) else { return "\(file.path): can't read the file" }
        let directory = file.deletingLastPathComponent().path
        setPackagePath(directory)
        do {
            try vm.run(code, name: "@" + file.path)
            return nil
        } catch {
            return (error as? LuaError)?.description ?? "\(error)"
        }
    }

    func run(code: String, name: String) throws -> Any? {
        if name.hasPrefix("@") { setPackagePath((String(name.dropFirst()) as NSString).deletingLastPathComponent) }
        return try vm.run(code, name: name).first ?? nil
    }

    private func setPackagePath(_ directory: String) {
        let lua = vm.main
        lua.getGlobal("package")
        guard lua.type(-1) == LUA_TTABLE else { return lua.pop() }
        let current = lua.field(-1, "path") as? String ?? ""
        let mine = "\(directory)/?.lua;\(directory)/?/init.lua"
        if !current.hasPrefix(mine) { lua.setField(-1, "path", mine + ";" + current) }
        lua.pop()
    }

    // MARK: Actions

    /// Runs the action `name`: one this runtime defines, else the config's,
    /// else one of Termy Next's own.
    func invoke(_ name: String, args: [String: Any], context ctx: [String: Any]) throws -> Any? {
        if let action = actions[name] {
            let converted = try convert(args, for: action.info)
            let saved = context
            context = ctx
            defer { context = saved }
            let result = try vm.call(action.run, [ctx, converted]).first ?? nil
            flushQueued()
            return result
        }
        if kind == .script, let config = host.configRuntime, config !== self {
            return try host.queue.sync { try config.invoke(name, args: args, context: ctx) }
        }
        try onMain { try PluginClientActions.run(name, args: args, context: ctx) }
        return nil
    }

    /// Runs a key's function; true when it took the key (returned true).
    func callBinding(_ id: Int, context ctx: [String: Any], key: [String: Any] = [:]) throws -> Bool {
        guard let function = bindingFunctions[id] else { return false }
        let saved = context
        context = ctx
        defer { context = saved }
        var event = key
        event["origin"] = "key"
        let result = try vm.call(function, [ctx, event]).first ?? nil
        flushQueued()
        return (result as? Bool) == true
    }

    private func flushQueued() {
        guard !queued.isEmpty else { return }
        let actions = queued
        queued.removeAll()
        let ctx = context
        nonisolated(unsafe) let sent = (actions, ctx)
        DispatchQueue.main.async {
            MainActor.assumeIsolated {
                for (name, args) in sent.0 {
                    do { try PluginClientActions.run(name, args: args, context: sent.1) } catch { PluginHost.report(error) }
                }
            }
        }
    }

    /// Checks `args` against the action's schema, turning text into numbers
    /// and booleans where the schema asks.
    private func convert(_ args: [String: Any], for action: PluginActionInfo) throws -> [String: Any] {
        var out = args
        for argument in action.arguments {
            guard let value = args[argument.name] else {
                if !argument.optional { throw LuaError("\(action.name): missing argument \(argument.name) (\(argument.type))") }
                continue
            }
            switch argument.type {
            case "number", "integer", "int":
                if let text = value as? String {
                    guard let number = Double(text) else { throw LuaError("\(action.name): \(argument.name) must be a number") }
                    out[argument.name] = argument.type == "number" ? number : Int(number)
                }
            case "boolean", "bool":
                if let text = value as? String { out[argument.name] = ["true", "1", "yes", "on"].contains(text.lowercased()) }
            case "string":
                if let number = value as? NSNumber { out[argument.name] = number.stringValue }
            default:
                break
            }
        }
        return out
    }

    // MARK: Events

    static func matches(_ event: [String: Any], type: String, filter: [String: Any]) -> Bool {
        let eventType = event["type"] as? String ?? ""
        let blockEvent = eventType == "block_event" ? event["event"] as? String : nil
        guard type == "*" || type == eventType || type == blockEvent else { return false }
        for (key, expected) in filter {
            let actual = event[key] ?? (event["data"] as? [String: Any])?[key]
            if "\(actual ?? "")" != "\(expected)" { return false }
        }
        return true
    }

    func deliver(_ event: [String: Any]) {
        guard !tornDown else { return }
        for handler in handlers where Self.matches(event, type: handler.type, filter: handler.filter) {
            do {
                _ = try vm.call(handler.function, [event])
            } catch let error as LuaError where error.description.contains(Self.stopMarker) {
                return
            } catch {
                PluginLog.write("rex.on \(handler.type): \(error)")
            }
        }
    }

    /// Waits, off the runtime's queue, until `rex.stop` or `seconds` pass.
    func waitForStop(seconds: Double) {
        _ = stopped.wait(timeout: .now() + seconds)
    }

    // MARK: The API

    /// The API `termy` and `rex` both name.
    private func install() {
        let lua = vm.main
        lua_createtable(lua.state, 0, 32)
        let api = lua.top

        lua.setFunction(api, "action") { [unowned self] lua in try self.defineAction(lua) }
        lua.setFunction(api, "bind") { [unowned self] lua in try self.bind(lua) }
        lua.setFunction(api, "unbind") { [unowned self] lua in
            guard let raw = lua.string(1), let spec = KeySpec(raw) else {
                self.warn(self.source(lua), "rex.unbind: unknown key \(lua.string(1).map { "\"\($0)\"" } ?? "(none)")")
                lua_pushinteger(lua.state, 0)
                return 1
            }
            let action = lua.string(2)
            let before = self.bindings.count
            self.bindings.removeAll { binding in
                guard binding.spec == spec else { return false }
                if let action, case .action(let name, _) = binding.target { return name == action }
                return action == nil
            }
            let removed = before - self.bindings.count
            // Without an action, the key is also taken from Termy Next's own shortcuts.
            if action == nil, !self.removals.contains(spec) { self.removals.append(spec) }
            lua_pushinteger(lua.state, lua_Integer(removed))
            return 1
        }
        lua.setFunction(api, "bindings") { [unowned self] lua in
            lua.push(self.bindings.map { binding -> [String: Any] in
                var entry: [String: Any] = ["key": binding.spec.keys, "source": binding.source]
                if let mode = binding.spec.mode { entry["mode"] = mode }
                switch binding.target {
                case .action(let name, let args):
                    entry["action"] = name
                    if let args, let value = try? JSONSerialization.jsonObject(with: Data(args.utf8)) { entry["args"] = value }
                case .function(let id, _):
                    entry["fn"] = self.bindingFunctions[id]
                }
                return entry
            })
            return 1
        }
        lua.setFunction(api, "invoke") { [unowned self] lua in
            guard let name = lua.string(1) else { throw LuaError("rex.invoke: expected an action name") }
            let args = lua.value(2) as? [String: Any] ?? [:]
            lua.push(try self.invoke(name, args: args, context: self.context))
            return 1
        }
        lua.setFunction(api, "call") { [unowned self] lua in
            guard let method = lua.string(1) else { throw LuaError("rex.call: expected a method name") }
            return self.callAPI(lua, method: method, params: lua.value(2) as? [String: Any] ?? [:])
        }
        lua.setFunction(api, "log") { lua in
            let parts = (1...max(lua.top, 1)).compactMap { i -> String? in
                lua.top >= i ? (lua.string(i) ?? (lua.value(i).map { "\($0)" } ?? "nil")) : nil
            }
            PluginLog.write("[lua] " + parts.joined(separator: " "))
            return 0
        }
        lua.setFunction(api, "sleep") { lua in
            let seconds = min(max(lua.number(1) ?? 0, 0), 3600)
            Thread.sleep(forTimeInterval: seconds)
            return 0
        }
        lua.setFunction(api, "after") { [unowned self] lua in try self.timer(lua, repeating: false) }
        lua.setFunction(api, "every") { [unowned self] lua in try self.timer(lua, repeating: true) }
        lua.setFunction(api, "on") { [unowned self] lua in try self.on(lua) }
        lua.setFunction(api, "wait") { [unowned self] lua in
            guard let type = lua.string(1) else { throw LuaError("rex.wait: expected an event type") }
            let filter = lua.value(2) as? [String: Any] ?? [:]
            let timeout = lua.number(3) ?? lua.number(2) ?? 30
            let event = self.host.wait(type: type, filter: filter, timeout: timeout)
            lua.push(event)
            return 1
        }
        lua.setFunction(api, "stop") { [unowned self] lua in
            self.stopCode = Int(lua.number(1) ?? 0)
            self.stopped.signal()
            throw LuaError(Self.stopMarker + "\(self.stopCode ?? 0)")
        }
        lua.setFunction(api, "mode") { [unowned self] lua in
            let where_ = self.source(lua)
            guard lua.type(1) == LUA_TSTRING, let name = lua.string(1) else {
                // As in Rex: reported, and the rest of the file goes on.
                self.warn(where_, "rex.mode: expected a string, got \(lua.typeName(1))")
                return 0
            }
            guard KeySpec.isValidMode(name) else {
                self.warn(where_, "rex.mode: \(name) isn't a mode name: lowercase letters, digits, _ and - only, and not root")
                return 0
            }
            let options = lua.value(2) as? [String: Any] ?? [:]
            self.modes.removeAll { $0.name == name }
            self.modes.append(PluginMode(name: name, exclusive: options["exclusive"] as? Bool ?? false, source: where_))
            return 0
        }
        lua.setFunction(api, "modes") { [unowned self] lua in
            lua.push(self.modes.map { ["name": $0.name, "exclusive": $0.exclusive, "source": $0.source] })
            return 1
        }
        lua.setFunction(api, "servers") { lua in
            let labels = onMain { PluginHosts.list().compactMap { $0["label"] as? String } }
            lua.push(labels)
            return 1
        }
        lua.setFunction(api, "server") { [unowned self] lua in
            guard let label = lua.string(1) else { throw LuaError("rex.server: expected a host label") }
            guard let host = onMain({ PluginHosts.find(label) }) ?? self.declaredHosts.first(where: { ($0["label"] as? String)?.lowercased() == label.lowercased() }),
                  let endpoint = (host["endpoints"] as? [String])?.first
            else { throw LuaError("rex.server: no server named \"\(label)\"") }
            self.pushRemoteAPI(lua, label: label, endpoint: endpoint)
            return 1
        }
        lua.setFunction(api, "host") { [unowned self] lua in
            let where_ = self.source(lua)
            guard var host = lua.value(1) as? [String: Any] else {
                self.warn(where_, "rex.host: expected a table, got \(lua.typeName(1)), so no host was declared")
                return 0
            }
            if let ssh = host["ssh"] as? String, host["endpoints"] == nil { host["endpoints"] = [ssh] }
            if let endpoint = host["endpoint"] as? String, host["endpoints"] == nil { host["endpoints"] = [endpoint] }
            guard let label = host["label"] as? String, !label.isEmpty, host["endpoints"] as? [String] != nil else {
                self.warn(where_, "rex.host: a host needs a label and endpoints")
                return 0
            }
            if host["id"] == nil { host["id"] = "host:" + label.lowercased() }
            host["source"] = where_
            self.declaredHosts.removeAll { ($0["label"] as? String)?.lowercased() == label.lowercased() }
            self.declaredHosts.append(host)
            return 0
        }
        lua.setField(api, "args", kind == .script ? scriptArgs : [String: Any]())
        lua.setField(api, "version", Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "dev")
        lua.setField(api, "app", "Termy Next")

        // termy.session.<method>{…}: Rex's session methods.
        lua_createtable(lua.state, 0, 0)
        let session = lua.top
        lua_createtable(lua.state, 0, 1)
        lua.setFunction(-1, "__index") { [unowned self] lua in
            guard let name = lua.string(2) else { return 0 }
            let method = "session." + name
            lua.pushFunction { [unowned self] lua in
                self.callAPI(lua, method: method, params: lua.value(1) as? [String: Any] ?? [:])
            }
            // Kept, so each method is made once.
            lua_pushvalue(lua.state, 2)
            lua_pushvalue(lua.state, -2)
            lua_rawset(lua.state, 1)
            return 1
        }
        lua_setmetatable(lua.state, session)
        lua.pushString("session")
        lua_pushvalue(lua.state, session)
        lua_rawset(lua.state, api)
        lua.pop()

        // termy.block.call(creator, method, args)
        lua.push([String: Any]())
        lua.setFunction(-1, "call") { [unowned self] lua in
            guard let creator = lua.string(1), let method = lua.string(2) else {
                throw LuaError("rex.block.call: expected a creator and a method")
            }
            var args = lua.value(3) as? [String: Any] ?? [:]
            guard PluginFlavor.isTerminalCreator(creator) else {
                lua_pushnil(lua.state)
                lua.push("no block creator named \(creator)")
                return 2
            }
            var params: [String: Any] = ["method": method]
            params["block_id"] = args.removeValue(forKey: "block_id") ?? self.context["block_id"]
            params["session_id"] = args.removeValue(forKey: "session_id") ?? self.context["session_id"]
            params["args"] = args
            return self.callAPI(lua, method: "block.method", params: params)
        }
        lua.setFunction(-1, "list") { [unowned self] lua in
            self.callAPI(lua, method: "session.list_blocks", params: lua.value(1) as? [String: Any] ?? [:])
        }
        lua.setFunction(-1, "close") { [unowned self] lua in
            self.callAPI(lua, method: "block.close", params: lua.value(1) as? [String: Any] ?? [:])
        }
        lua.pushString("block")
        clua_insert(lua.state, -2)
        lua_rawset(lua.state, api)

        // termy.client.queue(action, args)
        lua.push([String: Any]())
        lua.setFunction(-1, "queue") { [unowned self] lua in
            guard let name = lua.string(1) else { throw LuaError("rex.client.queue: expected an action name") }
            self.queued.append((name, lua.value(2) as? [String: Any] ?? [:]))
            if self.context.isEmpty || self.kind == .script { self.flushQueued() }
            return 0
        }
        lua.pushString("client")
        clua_insert(lua.state, -2)
        lua_rawset(lua.state, api)

        // termy.exec(argv, opts) and termy.exec.start(argv, opts)
        lua.push([String: Any]())
        let exec = lua.top
        lua.setFunction(exec, "start") { [unowned self] lua in try self.start(lua) }
        lua_createtable(lua.state, 0, 1)
        lua.setFunction(-1, "__call") { [unowned self] lua in try self.exec(lua, argvIndex: 2) }
        lua_setmetatable(lua.state, exec)
        lua.pushString("exec")
        clua_insert(lua.state, -2)
        lua_rawset(lua.state, api)

        // termy.json
        lua.push([String: Any]())
        lua.setFunction(-1, "encode") { lua in
            let value = Self.jsonSafe(lua.value(1))
            let options: JSONSerialization.WritingOptions = [.sortedKeys, .fragmentsAllowed, .withoutEscapingSlashes]
            guard let data = try? JSONSerialization.data(withJSONObject: value ?? NSNull(), options: options) else {
                throw LuaError("rex.json.encode: the value can't be written as JSON")
            }
            lua.push(String(decoding: data, as: UTF8.self))
            return 1
        }
        lua.setFunction(-1, "decode") { lua in
            guard let text = lua.string(1), let value = try? JSONSerialization.jsonObject(with: Data(text.utf8), options: [.fragmentsAllowed]) else {
                lua_pushnil(lua.state)
                lua.push("rex.json.decode: not valid JSON")
                return 2
            }
            lua.push(value)
            return 1
        }
        lua.pushString("json")
        clua_insert(lua.state, -2)
        lua_rawset(lua.state, api)

        // termy.layout
        lua.push([String: Any]())
        lua.setFunction(-1, "block") { lua in
            guard lua.type(1) == LUA_TTABLE else { throw LuaError("bad argument #1 to block (table expected, got \(lua.typeName(1)))") }
            var spec = lua.value(1) as? [String: Any] ?? [:]
            if spec["flavor"] == nil { spec["flavor"] = PluginFlavor.shell }
            lua.push(["block": spec])
            return 1
        }
        for direction in ["horizontal", "vertical"] {
            lua.setFunction(-1, direction) { lua in
                var split: [String: Any] = ["direction": direction]
                split["before"] = lua.value(1)
                split["after"] = lua.value(2)
                if let ratio = lua.number(3) { split["ratio"] = ratio }
                lua.push(["split": split])
                return 1
            }
        }
        lua.pushString("layout")
        clua_insert(lua.state, -2)
        lua_rawset(lua.state, api)

        // Termy Next's own: its commands by name.
        lua.setFunction(api, "command") { [unowned self] lua in
            guard let name = lua.string(1) else { throw LuaError("termy.command: expected a command name") }
            let ctx = self.context
            try onMain { try PluginClientActions.run(name, args: lua.value(2) as? [String: Any] ?? [:], context: ctx) }
            return 0
        }
        lua.setFunction(api, "commands") { lua in
            lua.push(onMain { PluginClientActions.catalog() })
            return 1
        }

        lua_pushvalue(lua.state, api)
        lua.setGlobal("termy")
        lua_pushvalue(lua.state, api)
        lua.setGlobal("rex")
        lua.pop()

        // print goes to `rex do`'s output for a script, else to the log;
        // os.exit would end Termy Next.
        lua.pushFunction { [unowned self] lua in
            let parts = lua.top > 0 ? (1...lua.top).map { lua.string($0) ?? (lua.value($0).map { "\($0)" } ?? "nil") } : []
            let line = parts.joined(separator: "\t")
            if self.kind == .script { self.printed.append(line) } else { PluginLog.write("[print] " + line) }
            return 0
        }
        lua.setGlobal("print")
        lua.getGlobal("os")
        lua.setFunction(-1, "exit") { _ in throw LuaError("os.exit isn't available to plugins") }
        lua.pop()
        // Rex runs Lua 5.1; its globals keep working.
        _ = try? vm.run(Self.lua51Compatibility, name: "=compat")
    }

    /// Calls the API on the main thread: the result, or nil and the error.
    private func callAPI(_ lua: Lua, method: String, params: [String: Any]) -> Int32 {
        var params = Self.jsonSafe(params) as? [String: Any] ?? [:]
        if kind == .check, !["session.list", "session.view", "session.list_blocks", "block.method"].contains(method) {
            lua_pushnil(lua.state)
            lua.push("\(method) isn't available while checking the config")
            return 2
        }
        if method.hasPrefix("session."), params["session_id"] == nil, !["session.list", "session.create"].contains(method),
           let session = context["session_id"] {
            params["session_id"] = session
        }
        let ctx = PluginAPI.Context(json: context)
        nonisolated(unsafe) let sent = params
        do {
            let result = try onMain { try PluginAPI.shared.call(method, sent, context: ctx) }
            lua.push(result)
            return 1
        } catch {
            lua_pushnil(lua.state)
            lua.push((error as? PluginAPI.Failure)?.description ?? "\(error)")
            return 2
        }
    }

    /// `rex.server(label)`: the same helpers as `rex`, with every call going
    /// to Termy Next on that host, through its `rex` command over ssh.
    private func pushRemoteAPI(_ lua: Lua, label: String, endpoint: String) {
        let call: (String, [String: Any]) -> (Any?, String?) = { method, params in
            let script = #"r="$HOME/Library/Application Support/TermyNext/bin/rex"; [ -x "$r" ] || r=rex; exec "$r" api call "$0" -"#
            let remote = "sh -c " + ShellQuote.quote(script) + " " + ShellQuote.quote(method)
            let process = Process()
            process.executableURL = URL(fileURLWithPath: "/usr/bin/ssh")
            process.arguments = ["-o", "BatchMode=yes", "-o", "ConnectTimeout=10", endpoint, remote]
            let input = Pipe(), output = Pipe(), errors = Pipe()
            process.standardInput = input
            process.standardOutput = output
            process.standardError = errors
            do { try process.run() } catch { return (nil, "rex.server(\(label)): can't run ssh: \(error)") }
            let body = (try? JSONSerialization.data(withJSONObject: Self.jsonSafe(params) ?? [:])) ?? Data("{}".utf8)
            try? input.fileHandleForWriting.write(contentsOf: body)
            try? input.fileHandleForWriting.close()
            let out = output.fileHandleForReading.readDataToEndOfFile()
            let err = errors.fileHandleForReading.readDataToEndOfFile()
            process.waitUntilExit()
            guard process.terminationStatus == 0 else {
                let message = String(decoding: err, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
                return (nil, message.replacingOccurrences(of: "Error: ", with: "").isEmpty ? "ssh to \(endpoint) failed" : message.replacingOccurrences(of: "Error: ", with: ""))
            }
            return ((try? JSONSerialization.jsonObject(with: out, options: [.fragmentsAllowed])) ?? NSNull(), nil)
        }
        func push(_ result: (Any?, String?), _ lua: Lua) -> Int32 {
            if let error = result.1 {
                lua_pushnil(lua.state)
                lua.push(error)
                return 2
            }
            lua.push(result.0 is NSNull ? nil : result.0)
            return 1
        }
        lua.push(["label": label, "endpoint": endpoint])
        lua.setFunction(-1, "call") { lua in
            guard let method = lua.string(1) else { throw LuaError("rex.server(\(label)).call: expected a method name") }
            return push(call(method, lua.value(2) as? [String: Any] ?? [:]), lua)
        }
        lua.push([String: Any]())
        lua.setFunction(-1, "call") { lua in
            guard let creator = lua.string(1), let method = lua.string(2) else { throw LuaError("block.call: expected a creator and a method") }
            var args = lua.value(3) as? [String: Any] ?? [:]
            var params: [String: Any] = ["method": PluginFlavor.isTerminalCreator(creator) ? method : creator + "." + method]
            params["block_id"] = args.removeValue(forKey: "block_id")
            params["session_id"] = args.removeValue(forKey: "session_id")
            params["args"] = args
            return push(call("block.method", params), lua)
        }
        lua.pushString("block")
        clua_insert(lua.state, -2)
        lua_rawset(lua.state, -3)
        lua_createtable(lua.state, 0, 0)
        lua_createtable(lua.state, 0, 1)
        lua.setFunction(-1, "__index") { lua in
            guard let name = lua.string(2) else { return 0 }
            lua.pushFunction { lua in push(call("session." + name, lua.value(1) as? [String: Any] ?? [:]), lua) }
            return 1
        }
        lua_setmetatable(lua.state, -2)
        lua.pushString("session")
        clua_insert(lua.state, -2)
        lua_rawset(lua.state, -3)
    }

    /// Drops what JSON can't carry, such as functions.
    static func jsonSafe(_ value: Any?) -> Any? {
        switch value {
        case let dict as [String: Any]: return dict.compactMapValues { jsonSafe($0) }
        case let array as [Any]: return array.compactMap { jsonSafe($0) }
        case is LuaRef: return nil
        default: return value
        }
    }

    private func defineAction(_ lua: Lua) throws -> Int32 {
        let where_ = source(lua)
        var spec: [String: Any]
        if lua.type(1) == LUA_TSTRING {
            spec = lua.value(3) as? [String: Any] ?? [:]
            spec["name"] = lua.string(1)
            spec["run"] = lua.type(2) == LUA_TFUNCTION ? lua.ref(2) : nil
        } else if lua.type(1) == LUA_TTABLE {
            spec = lua.value(1) as? [String: Any] ?? [:]
        } else {
            throw LuaError("rex.action: expected a table, got \(lua.typeName(1))")
        }
        guard let name = spec["name"] as? String, !name.isEmpty else { throw LuaError("rex.action: name is required") }
        guard name.range(of: #"^[A-Za-z0-9_.:-]+$"#, options: .regularExpression) != nil else {
            throw LuaError("rex.action: \(name) isn't a valid action name")
        }
        guard let run = spec["run"] as? LuaRef else { throw LuaError("rex.action: \(name) needs a run function") }
        var arguments: [PluginActionInfo.Argument] = []
        for (argName, raw) in (spec["args"] as? [String: Any] ?? [:]).sorted(by: { $0.key < $1.key }) {
            var type = "string", optional = false, description: String?
            if let text = raw as? String {
                optional = text.hasSuffix("?")
                type = optional ? String(text.dropLast()) : text
            } else if let table = raw as? [String: Any] {
                let text = table["type"] as? String ?? "string"
                optional = (table["optional"] as? Bool) ?? text.hasSuffix("?") || table["default"] != nil
                type = text.hasSuffix("?") ? String(text.dropLast()) : text
                description = table["description"] as? String
            }
            arguments.append(.init(name: argName, type: type, optional: optional, description: description))
        }
        let keywords: String? = (spec["keywords"] as? String) ?? (spec["keywords"] as? [Any])?.map { "\($0)" }.joined(separator: " ")
        let info = PluginActionInfo(
            name: name, title: spec["title"] as? String ?? name, description: spec["description"] as? String,
            category: spec["category"] as? String, keywords: keywords, arguments: arguments,
            hidden: (spec["hidden"] as? Bool) ?? (spec["palette"] as? Bool).map { !$0 } ?? false, source: where_)
        if actions[name] == nil { actionOrder.append(name) }
        actions[name] = Action(info: info, run: run)
        return 0
    }

    private func bind(_ lua: Lua) throws -> Int32 {
        let where_ = source(lua)
        guard let raw = lua.string(1), let spec = KeySpec(raw) else {
            // As Rex does: reported, the binding skipped, the rest of the file loaded.
            warn(where_, "rex.bind: unknown key \(lua.string(1).map { "\"\($0)\"" } ?? "(none)")")
            return 0
        }
        let target: PluginBindingTarget
        switch lua.type(2) {
        case LUA_TSTRING:
            var args: String?
            if lua.type(3) == LUA_TTABLE, let value = Self.jsonSafe(lua.value(3)),
               let data = try? JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]) {
                args = String(decoding: data, as: UTF8.self)
            }
            target = .action(lua.string(2)!, args: args)
        case LUA_TFUNCTION:
            let id = nextID
            nextID += 1
            bindingFunctions[id] = lua.ref(2)
            target = .function(id, client: false)
        default:
            throw LuaError("rex.bind: \(raw) needs an action name or a function")
        }
        bindings.removeAll { $0.spec == spec }
        removals.removeAll { $0 == spec }
        bindings.append(PluginBinding(spec: spec, target: target, source: where_))
        return 0
    }

    private func warn(_ source: String, _ message: String) {
        diagnostics.append((source, message))
        PluginLog.write("\(source.isEmpty ? "" : source + ": ")\(message)")
    }

    /// Rex runs Lua 5.1; these bring back what 5.4 dropped.
    static let lua51Compatibility = """
    unpack = unpack or table.unpack
    loadstring = loadstring or load
    table.getn = table.getn or function(t) return #t end
    table.maxn = table.maxn or function(t) local n = 0 for k in pairs(t) do if type(k) == "number" and k > n then n = k end end return n end
    math.pow = math.pow or function(a, b) return a ^ b end
    math.mod = math.mod or math.fmod
    math.ldexp = math.ldexp or function(m, e) return m * 2.0 ^ e end
    string.gfind = string.gfind or string.gmatch
    function setfenv(f, env)
      if type(f) == "number" then f = debug.getinfo(f + 1, "f").func end
      local i = 1
      while true do
        local name = debug.getupvalue(f, i)
        if name == "_ENV" then debug.upvaluejoin(f, i, function() return env end, 1) break
        elseif not name then break end
        i = i + 1
      end
      return f
    end
    function getfenv(f)
      if f == nil or f == 0 then return _G end
      if type(f) == "number" then f = debug.getinfo(f + 1, "f").func end
      local i = 1
      while true do
        local name, value = debug.getupvalue(f, i)
        if name == "_ENV" then return value elseif not name then break end
        i = i + 1
      end
      return _G
    end
    function module(name, ...)
      local m = package.loaded[name] or _G[name] or {}
      package.loaded[name] = m
      m._NAME, m._M, m._PACKAGE = name, m, name:gsub("[^.]*$", "")
      _G[name] = m
      for _, f in ipairs({...}) do f(m) end
      setfenv(2, m)
      return m
    end
    function package.seeall(m) setmetatable(m, { __index = _G }) end
    """

    private func timer(_ lua: Lua, repeating: Bool) throws -> Int32 {
        guard let seconds = lua.number(1), lua.type(2) == LUA_TFUNCTION else {
            throw LuaError("rex.\(repeating ? "every" : "after"): expected seconds and a function")
        }
        let function = lua.ref(2)
        let id = nextID
        nextID += 1
        let source = DispatchSource.makeTimerSource(queue: queue)
        let interval = max(seconds, repeating ? 0.05 : 0)
        source.schedule(deadline: .now() + interval, repeating: repeating ? .milliseconds(Int(interval * 1000)) : .never)
        source.setEventHandler { [weak self] in
            guard let self, !self.tornDown else { return }
            if !repeating { self.timers[id] = nil }
            do { _ = try self.vm.call(function, []) } catch where !"\(error)".contains(Self.stopMarker) { PluginLog.write("timer: \(error)") } catch {}
        }
        timers[id] = source
        source.resume()
        lua.push(["id": id, "cancel": LuaFunction { [weak self] _ in
            self?.timers.removeValue(forKey: id)?.cancel()
            return 0
        }])
        return 1
    }

    private func on(_ lua: Lua) throws -> Int32 {
        guard let type = lua.string(1) else { throw LuaError("rex.on: expected an event type") }
        let (filter, functionIndex): ([String: Any], Int32) = lua.type(2) == LUA_TTABLE ? (lua.value(2) as? [String: Any] ?? [:], 3) : ([:], 2)
        guard lua.type(functionIndex) == LUA_TFUNCTION else { throw LuaError("rex.on: \(type) needs a function") }
        let id = nextID
        nextID += 1
        handlers.append((id, type, filter, lua.ref(functionIndex)))
        host.handlersChanged()
        lua.push(["id": id, "off": LuaFunction { [weak self] _ in
            self?.handlers.removeAll { $0.id == id }
            self?.host.handlersChanged()
            return 0
        }])
        return 1
    }

    // MARK: exec

    private static let loginEnvironment: [String: String] = {
        var environment = ProcessInfo.processInfo.environment
        let shell = environment["SHELL"].flatMap { $0.isEmpty ? nil : $0 } ?? "/bin/zsh"
        let process = Process()
        process.executableURL = URL(fileURLWithPath: shell)
        process.arguments = ["-l", "-c", "/usr/bin/env -0"]
        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = FileHandle.nullDevice
        process.standardInput = FileHandle.nullDevice
        let done = DispatchSemaphore(value: 0)
        process.terminationHandler = { _ in done.signal() }
        if (try? process.run()) != nil {
            var data = Data()
            let reader = DispatchQueue(label: "sh.termy.next.login-env")
            let read = DispatchSemaphore(value: 0)
            reader.async {
                data = pipe.fileHandleForReading.readDataToEndOfFile()
                read.signal()
            }
            if done.wait(timeout: .now() + 4) == .timedOut { process.terminate() }
            if read.wait(timeout: .now() + 1) == .success {
                for entry in data.split(separator: 0) {
                    let text = String(decoding: entry, as: UTF8.self)
                    guard let eq = text.firstIndex(of: "=") else { continue }
                    environment[String(text[..<eq])] = String(text[text.index(after: eq)...])
                }
            }
        }
        for key in PluginEnvironment.foreign { environment[key] = nil }
        for key in ["SHLVL", "_", "PWD", "OLDPWD", "TERM_SESSION_ID"] { environment[key] = nil }
        return environment
    }()

    private func environment(_ extra: [String: Any]?) -> [String: String] {
        var environment = Self.loginEnvironment
        environment.merge(PluginEnvironment.common()) { $1 }
        if let login = Self.loginEnvironment["PATH"] {
            environment["PATH"] = PluginPaths.binDirectory.path + ":" + login
        }
        for (key, value) in extra ?? [:] {
            if let text = value as? String { environment[key] = text } else if let number = value as? NSNumber { environment[key] = number.stringValue }
        }
        return environment
    }

    private func argv(_ lua: Lua, _ index: Int32) throws -> [String] {
        if let line = lua.string(index) { return ["/bin/sh", "-c", line] }
        guard let parts = lua.value(index) as? [Any], !parts.isEmpty else {
            throw LuaError("rex.exec: expected a list of strings, the program and its arguments")
        }
        return parts.map { ($0 as? String) ?? "\($0)" }
    }

    private func spawn(_ argv: [String], options: [String: Any], detached: Bool, capture: Bool, pipeStdin: Bool) throws -> ManagedProcess {
        let argv = PluginPaths.translate(argv)
        let environment = environment(options["env"] as? [String: Any])
        guard let executable = ManagedProcess.resolve(argv[0], path: environment["PATH"] ?? "") else {
            throw LuaError("\(argv[0]): posix_spawn \(argv[0]): no such file or directory")
        }
        let cwd = (options["cwd"] as? String).map { ($0 as NSString).expandingTildeInPath } ?? NSHomeDirectory()
        let process = ManagedProcess(id: nextID, detached: detached)
        nextID += 1
        try process.launch(executable: executable, arguments: Array(argv.dropFirst()), environment: environment, cwd: cwd,
                           capture: capture, pipeStdin: pipeStdin)
        return process
    }

    private func exec(_ lua: Lua, argvIndex: Int32) throws -> Int32 {
        let argv = try argv(lua, argvIndex)
        let options = lua.value(argvIndex + 1) as? [String: Any] ?? [:]
        if options["detach"] as? Bool == true { throw LuaError("detach = true needs rex.exec.start, because rex.exec waits for the command") }
        if options["stdin"] as? Bool == true { throw LuaError("stdin = true needs rex.exec.start, which can write to it") }
        for key in ["stdout", "stderr", "on_exit"] where options[key] is LuaRef {
            throw LuaError("\(key) = function needs rex.exec.start, because rex.exec returns the output only when the command exits")
        }
        let process: ManagedProcess
        do {
            process = try spawn(argv, options: options, detached: false, capture: true, pipeStdin: options["stdin"] is String)
        } catch {
            lua_pushnil(lua.state)
            lua.push((error as? LuaError)?.description ?? "\(error)")
            return 2
        }
        if let input = options["stdin"] as? String { process.write(input); process.closeInput() }
        let timeout = (options["timeout"] as? NSNumber)?.doubleValue
        lua.push(process.wait(timeout: timeout))
        return 1
    }

    private func start(_ lua: Lua) throws -> Int32 {
        let argv = try argv(lua, 1)
        let options = lua.value(2) as? [String: Any] ?? [:]
        let detached = options["detach"] as? Bool ?? false
        let process: ManagedProcess
        do {
            process = try spawn(argv, options: options, detached: detached, capture: !detached,
                                pipeStdin: options["stdin"] as? Bool == true || options["stdin"] is String)
        } catch {
            lua_pushnil(lua.state)
            lua.push((error as? LuaError)?.description ?? "\(error)")
            return 2
        }
        if let input = options["stdin"] as? String { process.write(input); process.closeInput() }
        if !detached { processes[process.id] = process }
        for (key, stream) in [("stdout", 1), ("stderr", 2)] {
            guard let function = options[key] as? LuaRef else { continue }
            process.onChunk(stream: stream) { [weak self] chunk in
                self?.queue.async {
                    guard let self, !self.tornDown else { return }
                    do { _ = try self.vm.call(function, [chunk]) } catch { PluginLog.write("exec \(key): \(error)") }
                }
            }
        }
        if let function = options["on_exit"] as? LuaRef {
            process.onExit { [weak self] result in
                nonisolated(unsafe) let sent = result
                self?.queue.async {
                    guard let self, !self.tornDown else { return }
                    self.processes[process.id] = nil
                    do { _ = try self.vm.call(function, [sent]) } catch { PluginLog.write("exec on_exit: \(error)") }
                }
            }
        }
        // A userdata, as Rex's handles are, whose methods are on its metatable.
        _ = clua_newuserdata(lua.state, 1)
        lua_createtable(lua.state, 0, 3)
        lua.setField(-1, "__name", "rex.process")
        lua.setFunction(-1, "__tostring") { lua in
            lua.push("rex.process (pid \(process.pid))")
            return 1
        }
        lua.pushString("__index")
        lua.push([
            "pid": Int(process.pid),
            "wait": LuaFunction { lua in
                let timeout = lua.number(2) ?? lua.number(1)
                lua.push(process.wait(timeout: timeout))
                return 1
            },
            "kill": LuaFunction { lua in
                process.signal(Int32(lua.number(2) ?? lua.number(1) ?? Double(SIGTERM)))
                return 0
            },
            "write": LuaFunction { lua in
                if let text = lua.string(2) ?? lua.string(1) { process.write(text) }
                return 0
            },
            "close": LuaFunction { _ in
                process.closeInput()
                return 0
            },
            "running": LuaFunction { lua in
                lua_pushboolean(lua.state, process.isRunning ? 1 : 0)
                return 1
            },
        ])
        lua_rawset(lua.state, -3)
        lua_setmetatable(lua.state, -2)
        return 1
    }
}

extension PluginHost {
    fileprivate var configRuntime: PluginRuntime? { queue.sync { config } }
}

// MARK: - Processes

/// A program `rex.exec` started.
final class ManagedProcess: @unchecked Sendable {
    let id: Int
    let detached: Bool
    private let process = Process()
    private var stdoutPipe: Pipe?
    private var stderrPipe: Pipe?
    private var stdinPipe: Pipe?
    private let lock = NSLock()
    private var output = Data()
    private var errors = Data()
    private var chunkHandlers: [Int: (String) -> Void] = [:]
    private var exitHandlers: [([String: Any]) -> Void] = []
    private let exited = DispatchSemaphore(value: 0)
    private let drained = DispatchGroup()
    private var finished = false
    private var timedOut = false

    init(id: Int, detached: Bool) {
        self.id = id
        self.detached = detached
    }

    var pid: Int32 { process.processIdentifier }
    var isRunning: Bool { process.isRunning }

    static func resolve(_ program: String, path: String) -> String? {
        if program.contains("/") {
            let expanded = (program as NSString).expandingTildeInPath
            return FileManager.default.isExecutableFile(atPath: expanded) ? expanded : nil
        }
        let directories = path.split(separator: ":").map(String.init) + ["/usr/bin", "/bin", "/usr/sbin", "/sbin", "/opt/homebrew/bin", "/usr/local/bin"]
        for directory in directories {
            let candidate = (directory as NSString).appendingPathComponent(program)
            if FileManager.default.isExecutableFile(atPath: candidate) { return candidate }
        }
        return nil
    }

    func launch(executable: String, arguments: [String], environment: [String: String], cwd: String, capture: Bool, pipeStdin: Bool) throws {
        process.executableURL = URL(fileURLWithPath: executable)
        process.arguments = arguments
        process.environment = environment
        if FileManager.default.fileExists(atPath: cwd) { process.currentDirectoryURL = URL(fileURLWithPath: cwd) }
        if capture {
            let out = Pipe(), err = Pipe()
            stdoutPipe = out
            stderrPipe = err
            process.standardOutput = out
            process.standardError = err
        } else {
            process.standardOutput = FileHandle.nullDevice
            process.standardError = FileHandle.nullDevice
        }
        if pipeStdin {
            let input = Pipe()
            stdinPipe = input
            process.standardInput = input
        } else {
            process.standardInput = FileHandle.nullDevice
        }
        process.terminationHandler = { [weak self] _ in self?.didExit() }
        try process.run()
        if detached { return }
        for (stream, pipe) in [(1, stdoutPipe), (2, stderrPipe)] {
            guard let pipe else { continue }
            drained.enter()
            DispatchQueue.global(qos: .utility).async { [weak self] in
                let handle = pipe.fileHandleForReading
                while true {
                    let data = handle.availableData
                    if data.isEmpty { break }
                    self?.received(data, stream: stream)
                }
                self?.drained.leave()
            }
        }
    }

    private func received(_ data: Data, stream: Int) {
        let handler: ((String) -> Void)? = lock.withLock {
            if stream == 1 { output.append(data) } else { errors.append(data) }
            return chunkHandlers[stream]
        }
        handler?(String(decoding: data, as: UTF8.self))
    }

    func onChunk(stream: Int, _ handler: @escaping (String) -> Void) {
        lock.withLock { chunkHandlers[stream] = handler }
    }

    func onExit(_ handler: @escaping ([String: Any]) -> Void) {
        let done = lock.withLock { () -> Bool in
            if !finished { exitHandlers.append(handler) }
            return finished
        }
        if done { handler(result) }
    }

    private func didExit() {
        // Output can trail the exit a little; a program that left a child
        // holding the pipe doesn't hold the result up for long.
        _ = drained.wait(timeout: .now() + 1)
        let handlers = lock.withLock { () -> [([String: Any]) -> Void] in
            finished = true
            return exitHandlers
        }
        exited.signal()
        let result = self.result
        handlers.forEach { $0(result) }
    }

    var result: [String: Any] {
        lock.withLock {
            let code: Int = process.isRunning ? -1 : (process.terminationReason == .uncaughtSignal ? -1 : Int(process.terminationStatus))
            var result: [String: Any] = [
                "ok": !timedOut && code == 0 && !process.isRunning,
                "code": code,
                "stdout": String(decoding: output, as: UTF8.self),
                "stderr": String(decoding: errors, as: UTF8.self),
                "pid": Int(process.processIdentifier),
            ]
            if timedOut { result["timed_out"] = true; result["error"] = "timed out" }
            if process.terminationReason == .uncaughtSignal, !process.isRunning { result["signal"] = Int(process.terminationStatus) }
            return result
        }
    }

    /// Waits for the program to end, at most `timeout` seconds, then stops it.
    func wait(timeout: Double?) -> [String: Any] {
        if let timeout, timeout > 0 {
            if exited.wait(timeout: .now() + timeout) == .timedOut {
                lock.withLock { timedOut = true }
                terminate()
                if exited.wait(timeout: .now() + 2) == .success { exited.signal() }
            } else {
                exited.signal()
            }
        } else {
            exited.wait()
            exited.signal()
        }
        return result
    }

    func write(_ text: String) {
        try? stdinPipe?.fileHandleForWriting.write(contentsOf: Data(text.utf8))
    }

    func closeInput() {
        try? stdinPipe?.fileHandleForWriting.close()
    }

    func signal(_ signal: Int32) {
        guard process.isRunning else { return }
        kill(process.processIdentifier, signal)
    }

    func terminate() {
        guard process.isRunning else { return }
        process.terminate()
        let pid = process.processIdentifier
        DispatchQueue.global().asyncAfter(deadline: .now() + 1) { [weak self] in
            if self?.process.isRunning == true { kill(pid, SIGKILL) }
        }
    }
}

// MARK: - Termy Next's own actions

/// Actions Termy Next performs itself, by Rex's names (`pane.split`,
/// `pane.zoom`, `client.tab.goto`) or Termy's (`termy.splitRight`, or a
/// command's title), for `rex.bind`, `rex.invoke` and `rex do`.
@MainActor
enum PluginClientActions {
    static func run(_ name: String, args: [String: Any], context: [String: Any]) throws {
        let controller = controller(for: context)
        if let command = command(for: name, args: args) {
            guard let controller else { throw LuaError("\(name) needs a Termy Next window") }
            controller.run(command)
            return
        }
        switch name {
        case "client.mode.enter", "mode.enter":
            guard let controller, let mode = args["name"] as? String ?? args["mode"] as? String else { throw LuaError("\(name) needs name") }
            PluginKeys.shared.enter(mode, in: controller)
        case "client.mode.exit", "mode.exit":
            if let controller { PluginKeys.shared.exit(in: controller) }
        case "client.mode.exit_all", "mode.exit_all":
            if let controller { PluginKeys.shared.exitAll(in: controller) }
        case "client.key.forward":
            break
        case "server.config_reload", "config.reload", "termy.reload_plugins":
            PluginHost.shared.reload()
        case "client.session.goto", "session.select":
            if let raw = args["session_id"] as? String ?? args["session"] as? String, let session = PluginAPI.shared.findSession(raw) {
                if let shown = PluginAPI.shared.controller(for: session) {
                    shown.window?.makeKeyAndOrderFront(nil)
                } else if let controller {
                    controller.pluginShow(session: session)
                } else {
                    AppDelegate.shared.open(session)
                }
            }
        default:
            if let short = PluginFlavor.terminalMethod(name) {
                let located = try PluginAPI.shared.resolvePane(context["block_id"] as? String, PluginAPI.Context(json: context))
                _ = try PluginAPI.shared.terminalMethod(short, args, pane: located.pane)
                return
            }
            throw LuaError("unknown action \(name)")
        }
    }

    private static func controller(for context: [String: Any]) -> WindowController? {
        if let raw = context["session_id"] as? String, let session = PluginAPI.shared.findSession(raw),
           let controller = PluginAPI.shared.controller(for: session) {
            return controller
        }
        return AppDelegate.shared.keyController ?? AppDelegate.shared.controllersFrontToBack.first
    }

    private static func direction(_ args: [String: Any]) -> String {
        (args["direction"] as? String ?? args["side"] as? String ?? "right").lowercased()
    }

    static func command(for name: String, args: [String: Any]) -> Command? {
        if let command = Command(rawValue: name) { return command }
        if name.hasPrefix("termy."), let command = Command(rawValue: String(name.dropFirst(6))) { return command }
        if let command = Command.allCases.first(where: { $0.title == name || $0.title == name + "…" }) { return command }
        let bare = name.hasPrefix("client.") ? String(name.dropFirst(7)) : name
        switch bare {
        case "pane.split", "block.split":
            return switch direction(args) {
            case "left": .splitLeft
            case "up", "above", "top": .splitUp
            case "down", "below", "bottom", "vertical": .splitDown
            case "auto", "best": .splitBestFit
            default: .splitRight
            }
        case "pane.zoom", "block.zoom": return .zoomPane
        case "pane.float", "block.float": return .floatPane
        case "pane.close", "block.close": return .closePane
        case "pane.focus", "block.focus":
            return switch direction(args) {
            case "left": .focusLeft
            case "up": .focusUp
            case "down": .focusDown
            case "next": .focusNextPane
            case "previous", "prev": .focusPreviousPane
            default: .focusRight
            }
        case "pane.next", "focus.next": return .focusNextPane
        case "pane.previous", "focus.previous": return .focusPreviousPane
        case "pane.swap", "pane.move":
            return switch direction(args) {
            case "left": .swapLeft
            case "up": .swapUp
            case "down": .swapDown
            default: .swapRight
            }
        case "pane.equalize", "pane.balance": return .equalizePanes
        case "pane.clear", "terminal.clear": return .clearScrollback
        case "pane.find", "find": return .find
        case "tab.new", "window.new": return .newTab
        case "tab.close", "window.close": return .closeTab
        case "tab.next", "window.next": return .nextTab
        case "tab.previous", "window.previous": return .previousTab
        case "tab.rename", "window.rename": return .renameTab
        case "tab.goto", "window.goto":
            let index = (args["index"] as? Int) ?? Int(args["index"] as? String ?? "") ?? 1
            return index >= 9 ? .selectLastTab : Command.allCases.first { $0.tabNumber == index }
        case "tab.last": return .selectLastTab
        case "session.new": return .newSession
        case "session.picker", "session.switch", "session.change": return .changeSession
        case "session.rename": return .renameSession
        case "session.close": return .closeSession
        case "palette.open", "palette", "command_palette": return .commandPalette
        case "sidebar.toggle": return .toggleSidebar
        case "activity.toggle": return .toggleActivity
        case "font.increase", "font.bigger": return .increaseFontSize
        case "font.decrease", "font.smaller": return .decreaseFontSize
        case "font.reset": return .resetFontSize
        case "app.settings", "settings": return .settings
        case "app.new_window", "app.window.new": return .newWindow
        case "directory.goto", "goto_directory": return .goToDirectory
        default: return nil
        }
    }

    static func catalog() -> [Any] {
        Command.allCases.map { ["name": "termy." + $0.rawValue, "title": $0.title] }
    }
}
