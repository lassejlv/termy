import AppKit
import Darwin
import TermyNextCore

/// zsh plugins drawn by the app: a completion popup under the prompt and the
/// ⇧⌘E command box. Off by default (Settings → General).
///
/// While on, new panes start zsh with ZDOTDIR pointing at a folder of ours
/// whose .zshenv restores the user's ZDOTDIR and loads `termy-next.zsh`. That
/// plugin connects to a Unix socket here and reports the command line as it
/// changes; the app answers with lines to set. See `ShellProtocol`.
@MainActor
final class ShellPlugins {
    static let shared = ShellPlugins()

    private var listener: Int32 = -1
    private var listenSource: DispatchSourceRead?
    private var connections: [ObjectIdentifier: ShellConnection] = [:]
    /// The latest connection from each pane's shell.
    private var byPane: [String: ShellConnection] = [:]
    private let panes = NSMapTable<NSString, PaneView>.strongToWeakObjects()

    private init() {}

    static var folder: URL {
        if let home = ProcessInfo.processInfo.environment["TERMY_NEXT_HOME"] {
            return URL(fileURLWithPath: home).appendingPathComponent("support/shell", isDirectory: true)
        }
        let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        return support.appendingPathComponent("TermyNext/shell", isDirectory: true)
    }

    /// In the support folder, unless that path is too long for a Unix socket
    /// (104 bytes); then in the temporary folder, named after the support folder.
    static var socketPath: String {
        let path = folder.appendingPathComponent("plugins.sock").path
        guard path.utf8.count >= 104 else { return path }
        let hash = folder.path.utf8.reduce(UInt64(5381)) { ($0 &* 33) &+ UInt64($1) }
        return (NSTemporaryDirectory() as NSString).appendingPathComponent("termy-next-\(String(hash, radix: 36)).sock")
    }
    private static var zshFolder: URL { folder.appendingPathComponent("zsh", isDirectory: true) }
    /// Holds the `ssh` wrapper zsh puts first in PATH when SSH colors are on.
    private static var binFolder: URL { folder.appendingPathComponent("bin", isDirectory: true) }

    var isRunning: Bool { listener >= 0 }

    /// Starts or stops with the preference.
    func start() {
        apply()
        NotificationCenter.default.addObserver(forName: Preferences.didChange, object: nil, queue: .main) { _ in
            MainActor.assumeIsolated { ShellPlugins.shared.apply() }
        }
    }

    private func apply() {
        if Preferences.shared.shellPlugins { listen() } else { stop() }
    }

    /// Variables for a new pane's shell: the zsh integration loads when the
    /// GUI plugins or SSH colors are on. `token` names an in-app pane; hosted
    /// panes go by the host's `TERMY_MUX_PANE` instead.
    func environment(token: String?) -> [String: String] {
        let prefs = Preferences.shared
        guard prefs.shellPlugins || prefs.sshSendsColor, installScripts() else { return [:] }
        var environment = ["ZDOTDIR": Self.zshFolder.path]
        if let zdotdir = ProcessInfo.processInfo.environment["ZDOTDIR"] {
            environment["TERMY_NEXT_ZDOTDIR"] = zdotdir
        }
        if prefs.sshSendsColor {
            environment["TERMY_NEXT_SSH_ENV"] = "1"
            environment["TERMY_NEXT_SSH_BIN"] = Self.binFolder.path
        }
        if isRunning {
            environment["TERMY_NEXT_SOCKET"] = Self.socketPath
            environment["TERMY_NEXT_PANE"] = token ?? ""
        }
        return environment
    }

    private var installed = false

    /// Writes the zsh scripts once per launch, so they match this build.
    private func installScripts() -> Bool {
        guard !installed else { return true }
        do {
            try FileManager.default.createDirectory(at: Self.zshFolder, withIntermediateDirectories: true,
                                                    attributes: [.posixPermissions: 0o700])
            try ShellScripts.zshenv.write(to: Self.zshFolder.appendingPathComponent(".zshenv"), atomically: true, encoding: .utf8)
            try ShellScripts.plugin.write(to: Self.zshFolder.appendingPathComponent("termy-next.zsh"), atomically: true, encoding: .utf8)
            try FileManager.default.createDirectory(at: Self.binFolder, withIntermediateDirectories: true,
                                                    attributes: [.posixPermissions: 0o700])
            let ssh = Self.binFolder.appendingPathComponent("ssh")
            try ShellScripts.ssh.write(to: ssh, atomically: true, encoding: .utf8)
            try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: ssh.path)
            installed = true
        } catch {
            NSLog("Termy Next: can't install the zsh integration: \(error)")
        }
        return installed
    }

    func register(_ pane: PaneView) {
        guard let token = pane.shellToken else { return }
        panes.setObject(pane, forKey: token as NSString)
    }

    func connection(for pane: PaneView) -> ShellConnection? {
        pane.shellToken.flatMap { byPane[$0] }
    }

    // MARK: Socket

    private func listen() {
        guard listener < 0 else { return }
        let path = Self.socketPath
        guard installScripts() else { return }
        var address = sockaddr_un()
        let bytes = Array(path.utf8)
        guard bytes.count < MemoryLayout.size(ofValue: address.sun_path) else {
            NSLog("Termy Next: the plugin socket path is too long: \(path)")
            return
        }
        address.sun_family = sa_family_t(AF_UNIX)
        address.sun_len = UInt8(MemoryLayout<sockaddr_un>.size)
        withUnsafeMutableBytes(of: &address.sun_path) { raw in
            raw.copyBytes(from: bytes)
            raw[bytes.count] = 0
        }
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { return }
        unlink(path)
        let bound = withUnsafePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { bind(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) }
        }
        guard bound == 0, chmod(path, 0o600) == 0, Darwin.listen(fd, 16) == 0 else {
            NSLog("Termy Next: can't listen on \(path): \(String(cString: strerror(errno)))")
            close(fd)
            return
        }
        _ = fcntl(fd, F_SETFL, fcntl(fd, F_GETFL) | O_NONBLOCK)
        listener = fd
        let source = DispatchSource.makeReadSource(fileDescriptor: fd, queue: .main)
        source.setEventHandler { MainActor.assumeIsolated { ShellPlugins.shared.acceptConnections() } }
        source.setCancelHandler { close(fd) }
        source.resume()
        listenSource = source
    }

    private func stop() {
        guard listener >= 0 else { return }
        listenSource?.cancel()
        listenSource = nil
        listener = -1
        unlink(Self.socketPath)
        for connection in Array(connections.values) { connection.close() }
    }

    private func acceptConnections() {
        while true {
            let fd = accept(listener, nil, nil)
            guard fd >= 0 else { return }
            // Only shells of this user.
            var uid: uid_t = 0, gid: gid_t = 0
            guard getpeereid(fd, &uid, &gid) == 0, uid == getuid() else {
                close(fd)
                continue
            }
            let connection = ShellConnection(fd: fd)
            connections[ObjectIdentifier(connection)] = connection
            connection.onMessage = { [weak self] connection, message in self?.handle(message, from: connection) }
            connection.onClose = { [weak self] connection in self?.closed(connection) }
            connection.start()
        }
    }

    private func handle(_ message: ShellMessage, from connection: ShellConnection) {
        switch message {
        case .hello(let pane, let pid, let path):
            guard !pane.isEmpty else { return connection.close() }
            connection.pane = pane
            connection.pid = pid
            connection.path = path
            if let old = byPane[pane], old !== connection { old.close() }
            byPane[pane] = connection
        case .line(let line):
            connection.line = line
        case .done:
            connection.line = nil
        }
        if let pane = connection.pane, byPane[pane] === connection {
            panes.object(forKey: pane as NSString)?.shellLineChanged()
        }
    }

    private func closed(_ connection: ShellConnection) {
        connections[ObjectIdentifier(connection)] = nil
        guard let pane = connection.pane, byPane[pane] === connection else { return }
        byPane[pane] = nil
        panes.object(forKey: pane as NSString)?.shellLineChanged()
    }
}

/// One zsh connected over the plugin socket.
@MainActor
final class ShellConnection {
    private let fd: Int32
    private var source: DispatchSourceRead?
    private var pending = Data()
    private(set) var isOpen = true

    /// `TERMY_NEXT_PANE` or `TERMY_MUX_PANE`, once the shell said hello.
    var pane: String?
    var pid: Int32 = 0
    /// The shell's PATH, where the command box looks for `claude` and `codex`.
    var path = ""
    /// The line zsh is editing, or nil while it runs a command.
    var line: ShellLine?

    var onMessage: ((ShellConnection, ShellMessage) -> Void)?
    var onClose: ((ShellConnection) -> Void)?

    init(fd: Int32) {
        self.fd = fd
        var on: Int32 = 1
        setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &on, socklen_t(MemoryLayout<Int32>.size))
        _ = fcntl(fd, F_SETFL, fcntl(fd, F_GETFL) | O_NONBLOCK)
    }

    func start() {
        let source = DispatchSource.makeReadSource(fileDescriptor: fd, queue: .main)
        source.setEventHandler { [weak self] in MainActor.assumeIsolated { self?.readAvailable() } }
        let fd = self.fd
        source.setCancelHandler { Darwin.close(fd) }
        source.resume()
        self.source = source
    }

    private func readAvailable() {
        var buffer = [UInt8](repeating: 0, count: 65536)
        while isOpen {
            let count = read(fd, &buffer, buffer.count)
            if count > 0 {
                pending.append(contentsOf: buffer[0..<count])
            } else if count < 0, errno == EAGAIN || errno == EINTR {
                break
            } else {
                close()
                return
            }
        }
        while let newline = pending.firstIndex(of: 0x0A) {
            let message = String(decoding: pending[pending.startIndex..<newline], as: UTF8.self)
            pending.removeSubrange(pending.startIndex...newline)
            if let parsed = ShellProtocol.parse(message) { onMessage?(self, parsed) }
        }
    }

    /// Replaces the command line zsh is editing.
    func setLine(_ text: String) {
        send(ShellProtocol.encode("set", text))
    }

    private func send(_ text: String) {
        guard isOpen else { return }
        let data = Array(text.utf8)
        var offset = 0
        while offset < data.count {
            let written = data[offset...].withUnsafeBytes { write(fd, $0.baseAddress, $0.count) }
            if written > 0 {
                offset += written
            } else if written < 0, errno == EINTR {
                continue
            } else {
                // A full buffer means the shell stopped reading; it isn't at a prompt.
                return
            }
        }
    }

    func close() {
        guard isOpen else { return }
        isOpen = false
        line = nil
        source?.cancel()
        source = nil
        onClose?(self)
    }
}
