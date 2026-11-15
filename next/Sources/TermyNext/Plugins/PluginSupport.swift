import Foundation
import TermyNextCore

/// Where plugin files live: the config files Termy Next loads, the `rex`
/// command it puts in its panes, and the socket that command talks to.
enum PluginPaths {
    static var paths: NextPaths { NextPaths.current }

    /// Termy Next's own config, loaded first: `termy.*` (or `rex.*`).
    static var termyConfig: URL {
        if let path = ProcessInfo.processInfo.environment["TERMY_NEXT_LUA_CONFIG"], !path.isEmpty {
            return URL(fileURLWithPath: (path as NSString).expandingTildeInPath)
        }
        return paths.configurationDirectory.appendingPathComponent("init.lua")
    }

    /// Rex's config, where `leaf rex install` and other Rex plugins put
    /// themselves: REX_CONFIG, else rex/init.lua in the config directory.
    static var rexConfig: URL {
        let environment = ProcessInfo.processInfo.environment
        if let path = environment["REX_CONFIG"], !path.isEmpty {
            return URL(fileURLWithPath: (path as NSString).expandingTildeInPath)
        }
        if let xdg = environment["XDG_CONFIG_HOME"], !xdg.isEmpty {
            return URL(fileURLWithPath: xdg).appendingPathComponent("rex/init.lua")
        }
        return FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".config/rex/init.lua")
    }

    static var binDirectory: URL { paths.supportDirectory.appendingPathComponent("bin", isDirectory: true) }
    static var logFile: URL { paths.supportDirectory.appendingPathComponent("plugins.log") }

    /// The socket the `rex` command reaches the app through. A Unix socket
    /// path is limited to 104 bytes, so a long support path falls back to /tmp.
    static var socket: String {
        if let path = ProcessInfo.processInfo.environment["TERMY_PLUGIN_SOCKET"], !path.isEmpty { return path }
        let path = paths.supportDirectory.appendingPathComponent("plugins.sock").path
        if path.utf8.count < 100 { return path }
        // FNV-1a: the app and the `rex` command must agree on it.
        var hash: UInt32 = 2_166_136_261
        for byte in path.utf8 { hash = (hash ^ UInt32(byte)) &* 16_777_619 }
        return "/tmp/termy-next-\(getuid())-\(String(hash, radix: 16)).sock"
    }

    /// The executable the `rex` shim runs: this app's own binary.
    static var executable: String {
        Bundle.main.executablePath ?? CommandLine.arguments[0]
    }

    /// Runs a block's program: in its own process group, so it is the
    /// program in front, then reports its exit status with OSC 133;D.
    static var runner: String { binDirectory.appendingPathComponent("termy-run").path }

    static let runnerScript = #"""
    #!/bin/sh
    # Runs a pane's program for Termy Next. Job control (set -m) gives the
    # program its own process group in front of the terminal; its exit status
    # goes to Termy Next as OSC 133;D, then becomes this script's.
    set -m
    "$@"
    code=$?
    printf '\033]133;D;%d\007' "$code"
    exit "$code"

    """#

    /// Whether `program` is Rex's own `rex` (from Rex.app or its server),
    /// which a plugin written for Rex may name by its full path.
    static func isRexBinary(_ program: String) -> Bool {
        guard (program as NSString).lastPathComponent == "rex", program.contains("/") else { return false }
        let path = ((program as NSString).expandingTildeInPath as NSString).resolvingSymlinksInPath
        guard !path.hasPrefix(binDirectory.resolvingSymlinksInPath().path) else { return false }
        return ["com.superlogical.rex", "/Rex Server.app/", "/Rex Server.rex/", "/Rex.app/", "/Rex Beta.app/"].contains { path.contains($0) }
    }

    /// `argv` with Rex's own `rex` swapped for Termy Next's, so a plugin
    /// that runs Rex's binary drives Termy Next instead.
    static func translate(_ argv: [String]) -> [String] {
        guard let first = argv.first, isRexBinary(first) else { return argv }
        return [binDirectory.appendingPathComponent("rex").path] + argv.dropFirst()
    }

    /// Writes `bin/rex`, which runs this binary as Rex's command-line tool.
    static func installShim() {
        let fm = FileManager.default
        try? fm.createDirectory(at: binDirectory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o755])
        let script = """
        #!/bin/sh
        # Rex's command-line tool, as Termy Next provides it. Written by Termy Next
        # on every launch; `rex --help` lists the commands.
        exec \(ShellQuote.quote(executable)) --rex-cli "$@"

        """
        let runnerURL = URL(fileURLWithPath: runner)
        if (try? String(contentsOf: runnerURL, encoding: .utf8)) != runnerScript {
            try? runnerScript.write(to: runnerURL, atomically: true, encoding: .utf8)
        }
        try? fm.setAttributes([.posixPermissions: 0o755], ofItemAtPath: runner)
        // The same tool as `termy`, the name Install CLI gives it.
        let termy = script.replacingOccurrences(of: "--rex-cli", with: "--termy-cli")
            .replacingOccurrences(of: "Rex's command-line tool", with: "Termy Next's command-line tool")
            .replacingOccurrences(of: "`rex --help`", with: "`termy --help`")
        for (name, text) in [("rex", script), ("termy-plugin", script), ("termy", termy)] {
            let url = binDirectory.appendingPathComponent(name)
            if (try? String(contentsOf: url, encoding: .utf8)) != text {
                try? text.write(to: url, atomically: true, encoding: .utf8)
            }
            try? fm.setAttributes([.posixPermissions: 0o755], ofItemAtPath: url.path)
        }
    }
}

/// IDs that outlive a relaunch: a session keeps its ID by name, and a pane in
/// the session host keeps its ID by the host's pane ID, so REX_SESSION and
/// REX_BLOCK in a running shell still name it after Termy Next restarts.
@MainActor
enum PluginIdentity {
    private static let defaults = UserDefaults.standard
    private static let sessionsKey = "pluginSessionIDs"
    private static let blocksKey = "pluginBlockIDs"

    static func sessionID(named name: String, taken: Set<UUID>) -> UUID {
        var map = defaults.dictionary(forKey: sessionsKey) as? [String: String] ?? [:]
        if let raw = map[name], let id = UUID(uuidString: raw), !taken.contains(id) { return id }
        let id = UUID()
        map[name] = id.uuidString
        defaults.set(map, forKey: sessionsKey)
        return id
    }

    static func sessionRenamed(_ id: UUID, from old: String, to new: String) {
        var map = defaults.dictionary(forKey: sessionsKey) as? [String: String] ?? [:]
        if map[old] == id.uuidString { map[old] = nil }
        map[new] = id.uuidString
        defaults.set(map, forKey: sessionsKey)
    }

    static func blockID(forHostPane paneID: String) -> UUID? {
        let map = defaults.dictionary(forKey: blocksKey) as? [String: String] ?? [:]
        return map[paneID].flatMap(UUID.init(uuidString:))
    }

    static func remember(block: UUID, hostPane paneID: String) {
        var map = defaults.dictionary(forKey: blocksKey) as? [String: String] ?? [:]
        guard map[paneID] != block.uuidString else { return }
        map[paneID] = block.uuidString
        defaults.set(map, forKey: blocksKey)
    }

    private static let labelsKey = "pluginBlockLabels"
    private static let tabsKey = "pluginTabIDs"
    private static var tabCache: [String: String]?

    /// The ID a tab of these panes had: the one most of its hosted panes were in.
    static func tabID(for panes: [PaneView]) -> UUID? {
        let map = tabCache ?? (defaults.dictionary(forKey: tabsKey) as? [String: String] ?? [:])
        tabCache = map
        let ids = panes.compactMap(\.paneID).compactMap { map[$0] }
        guard let best = Dictionary(grouping: ids, by: { $0 }).max(by: { $0.value.count < $1.value.count })?.key,
              let id = UUID(uuidString: best),
              !Workspace.shared.sessions.contains(where: { $0.tabs.contains { $0.id == id } })
        else { return nil }
        return id
    }

    /// Notes which tab each hosted pane is in, for `tabID(for:)` after a relaunch.
    static func rememberTabs() {
        var map: [String: String] = [:]
        for session in Workspace.shared.sessions {
            for tab in session.tabs { for pane in tab.allPanes { if let paneID = pane.paneID { map[paneID] = tab.id.uuidString } } }
        }
        guard map != tabCache, !map.isEmpty else { return }
        tabCache = map
        defaults.set(map, forKey: tabsKey)
    }

    /// A label a plugin gave a hosted pane, kept like its ID.
    static func label(forHostPane paneID: String) -> String? {
        (defaults.dictionary(forKey: labelsKey) as? [String: String])?[paneID]
    }

    static func remember(label: String?, hostPane paneID: String) {
        var map = defaults.dictionary(forKey: labelsKey) as? [String: String] ?? [:]
        guard map[paneID] != label else { return }
        map[paneID] = label
        defaults.set(map, forKey: labelsKey)
    }

    /// Forgets panes the host no longer runs.
    static func prune(livePanes: Set<String>) {
        for key in [blocksKey, labelsKey] {
            let map = defaults.dictionary(forKey: key) as? [String: String] ?? [:]
            let kept = map.filter { livePanes.contains($0.key) }
            if kept.count != map.count { defaults.set(kept, forKey: key) }
        }
    }
}

/// The environment Termy Next gives programs so Rex plugins and tools (leaf,
/// agents) find their pane and the `rex` command.
enum PluginEnvironment {
    /// Variables that name a pane or session of another multiplexer, which a
    /// program started from here must not inherit.
    static let foreign = ["REX_SESSION", "REX_BLOCK", "REX_CLIENT", "TERMY_SESSION", "TERMY_BLOCK"]

    static func common() -> [String: String] {
        let bin = PluginPaths.binDirectory.path
        var path = ProcessInfo.processInfo.environment["PATH"] ?? "/usr/bin:/bin:/usr/sbin:/sbin"
        if !path.split(separator: ":").contains(Substring(bin)) { path = bin + ":" + path }
        return [
            "REX_BIN_DIR": bin,
            "TERMY_BIN_DIR": bin,
            "TERMY_PLUGIN_SOCKET": PluginPaths.socket,
            "PATH": path,
        ]
    }

    /// For a pane: its session and block, Rex's way and Termy's.
    static func pane(block: UUID, session: UUID?) -> [String: String] {
        var environment = common()
        let blockID = PluginID.make("block", block)
        let sessionID = session.map { PluginID.make("session", $0) } ?? ""
        environment["REX_BLOCK"] = blockID
        environment["TERMY_BLOCK"] = blockID
        environment["REX_SESSION"] = sessionID
        environment["TERMY_SESSION"] = sessionID
        return environment
    }
}

/// Appends to plugins.log, for `termy.log` and load errors.
enum PluginLog {
    private static let queue = DispatchQueue(label: "sh.termy.next.plugin-log")

    static func write(_ message: String) {
        let line = "\(ISO8601DateFormatter().string(from: Date())) \(message)\n"
        queue.async {
            let url = PluginPaths.logFile
            try? FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
            if let handle = try? FileHandle(forWritingTo: url) {
                handle.seekToEndOfFile()
                handle.write(Data(line.utf8))
                try? handle.close()
            } else {
                try? Data(line.utf8).write(to: url)
            }
        }
    }
}

extension PluginPaths {
    static let starterConfig = """
    -- Termy Next plugins. `termy` is the plugin API; Rex plugins written
    -- against `rex` run too, since `rex` is the same table. Termy Next also
    -- loads Rex's config (~/.config/rex/init.lua) after this file.
    -- Run "Reload Plugins" from the command palette after editing.

    -- An action shows in the command palette and can be bound to a key.
    termy.action{
      name = "say_hello",
      title = "Say Hello",
      description = "Type a greeting into the focused pane.",
      category = "Examples",
      run = function(ctx)
        termy.block.call("terminal", "write", { block_id = ctx.block_id, data = "echo hello from Termy Next\\r" })
      end,
    }

    -- termy.bind("ctrl+shift+h", "say_hello")

    -- Open a split running a program beside the focused pane:
    -- termy.action{
    --   name = "htop_beside",
    --   title = "htop Beside",
    --   run = function(ctx)
    --     termy.session.new_split{
    --       anchor_id = ctx.block_id, direction = "horizontal", side = "after", focus = true,
    --       layout = termy.layout.block{ label = "htop", options = { command = { "htop" } } },
    --     }
    --   end,
    -- }

    """
}
