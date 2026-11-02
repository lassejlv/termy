import Foundation
internal import CTermy

/// One pane in a multiplexer host.
public struct MultiplexerPane: Decodable, Hashable, Sendable {
    public var id: String
    /// The pane's program, as the host started it.
    public var childPID: Int32?
    public var title: String?
    /// Where the pane started, then the latest directory its shell reported.
    public var workingDirectory: String?
    public var exited: Bool

    enum CodingKeys: String, CodingKey {
        case id, title, exited
        case childPID = "child_pid"
        case workingDirectory = "working_directory"
    }
}

/// A connection to a Termy session host, which runs terminal programs outside
/// the app: they keep running when the app quits, and a relaunch attaches to
/// them again. The host is the one `termy mux` and Termy's multiplexer use.
///
/// Terminals it creates or attaches are ordinary `Terminal`s with a `paneID`;
/// wrap them in a `TerminalSession`. Releasing a terminal detaches from the
/// pane and leaves its program running; `close(paneID:)` ends it.
///
/// Every call talks to the host and can block, starting a host for seconds.
/// Make calls off the main thread when that matters. Calls are serialized, so
/// any thread may use the instance.
public final class Multiplexer: @unchecked Sendable {
    let handle: OpaquePointer
    let lock = NSLock()

    private init(handle: OpaquePointer) {
        self.handle = handle
    }

    deinit {
        termy_multiplexer_free(handle)
    }

    /// Connects to the host in `root`, starting `hostExecutable` there when
    /// none runs. A nil `root` uses the directory Termy and `termy mux`
    /// share; give an app its own directory to keep its panes apart. The host
    /// creates `root` private (mode 0700) and refuses an existing directory
    /// that other users can read.
    public static func connect(root: URL? = nil, hostExecutable: URL) throws(TermyError) -> Multiplexer {
        var handle: OpaquePointer?
        let status = withOptionalUTF8(root?.path) { rootPtr, rootLen in
            withUTF8(hostExecutable.path) { hostPtr, hostLen in
                termy_multiplexer_connect(rootPtr, rootLen, hostPtr, hostLen, &handle)
            }
        }
        try check(status, "termy_multiplexer_connect")
        guard let handle else { throw TermyError(code: .nullPointer, operation: "termy_multiplexer_connect") }
        return Multiplexer(handle: handle)
    }

    /// The `termy-session-host` binary: in the app bundle (`Contents/Helpers`,
    /// `Contents/MacOS` or `Contents/Frameworks`), else next to the loaded
    /// libtermy, where `scripts/build-libtermy.sh` stages it.
    public static var bundledHostExecutable: URL? {
        let name = "termy-session-host"
        let fm = FileManager.default
        var candidates: [URL] = []
        if let url = Bundle.main.url(forAuxiliaryExecutable: name) { candidates.append(url) }
        let contents = Bundle.main.bundleURL.appendingPathComponent("Contents")
        candidates += ["Helpers", "MacOS", "Frameworks"].map { contents.appendingPathComponent($0).appendingPathComponent(name) }
        if let library = libtermyDirectory { candidates.append(library.appendingPathComponent(name)) }
        return candidates.first { fm.isExecutableFile(atPath: $0.path) }
    }

    private static var libtermyDirectory: URL? {
        var info = Dl_info()
        let symbol = unsafeBitCast(termy_size_default as @convention(c) () -> TermyFfiSize, to: UnsafeRawPointer.self)
        guard dladdr(symbol, &info) != 0, let path = info.dli_fname else { return nil }
        return URL(fileURLWithPath: String(cString: path)).deletingLastPathComponent()
    }

    /// Starts a pane running the configured shell (or `options.startupCommand`)
    /// and attaches a terminal to it.
    public func createTerminal(size: TerminalSize, options: TerminalOptions = TerminalOptions()) throws(TermyError) -> Terminal {
        lock.lock()
        defer { lock.unlock() }
        var paneID = termy_null_buffer()
        var terminal: OpaquePointer?
        let status = Terminal.withFFIOptions(options) {
            termy_multiplexer_create_terminal(handle, size.ffi, $0, &paneID, &terminal)
        }
        defer { termy_buffer_free(paneID) }
        try check(status, "termy_multiplexer_create_terminal")
        guard let terminal else { throw TermyError(code: .nullPointer, operation: "termy_multiplexer_create_terminal") }
        return Terminal(multiplexerHandle: terminal, paneID: paneID.copyString())
    }

    /// Attaches a terminal to a running pane. Throws `.notFound` when the pane
    /// is gone.
    public func attachTerminal(paneID: String) throws(TermyError) -> Terminal {
        lock.lock()
        defer { lock.unlock() }
        var terminal: OpaquePointer?
        let status = withUTF8(paneID) { termy_multiplexer_attach_terminal(handle, $0, $1, &terminal) }
        try check(status, "termy_multiplexer_attach_terminal")
        guard let terminal else { throw TermyError(code: .nullPointer, operation: "termy_multiplexer_attach_terminal") }
        return Terminal(multiplexerHandle: terminal, paneID: paneID)
    }

    /// Every pane the host runs, including exited ones it still remembers.
    public func panes() throws(TermyError) -> [MultiplexerPane] {
        lock.lock()
        defer { lock.unlock() }
        let json = try takeBytes("termy_multiplexer_list_json") { termy_multiplexer_list_json(handle, $0) }
        do {
            return try JSONDecoder().decode([MultiplexerPane].self, from: json)
        } catch {
            throw TermyError(code: .serializeFailed, operation: "termy_multiplexer_list_json")
        }
    }

    /// Ends a pane's program and removes the pane.
    public func close(paneID: String) throws(TermyError) {
        lock.lock()
        defer { lock.unlock() }
        let status = withUTF8(paneID) { termy_multiplexer_close(handle, $0, $1) }
        try check(status, "termy_multiplexer_close")
    }

    /// Ends every pane and stops the host.
    public func shutdown() throws(TermyError) {
        lock.lock()
        defer { lock.unlock() }
        try check(termy_multiplexer_shutdown(handle), "termy_multiplexer_shutdown")
    }
}
