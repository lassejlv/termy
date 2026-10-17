import Foundation
internal import CTermy

public enum TmuxNotification: Hashable, Sendable {
    /// Output for a pane. Feed it to that pane's display terminal.
    case output(paneID: String, data: Data)
    /// Pane layout or contents changed; re-query state.
    case needsRefresh
    case warning(String)
    /// The control session ended.
    case exit(reason: String?)
    case unknown(kind: UInt32)

    init(_ notification: TermyFfiTmuxNotification) {
        switch notification.kind {
        case 0: self = .output(paneID: notification.pane_id.copyString(), data: notification.data.copyData())
        case 1: self = .needsRefresh
        case 2: self = .warning(notification.data.copyString())
        case 3:
            let reason = notification.data.copyString()
            self = .exit(reason: reason.isEmpty ? nil : reason)
        default: self = .unknown(kind: notification.kind)
        }
    }
}

/// A `tmux -CC` control-mode connection.
///
/// Render each pane with a `Terminal.display(size:)` and route its
/// `.output` notifications to it. Send pane input as tmux commands.
public final class TmuxControl {
    private var handle: OpaquePointer?

    /// Starts `tmux -L socket -CC new-session -s session`. `socket` is a tmux
    /// socket name; tmux's own default is `"default"`.
    public init(binary: String = "tmux", socket: String = "default", session: String) throws(TermyError) {
        var handle: OpaquePointer?
        let status = withUTF8(binary) { binaryPtr, binaryLen in
            withUTF8(socket) { socketPtr, socketLen in
                withUTF8(session) { sessionPtr, sessionLen in
                    termy_tmux_control_open(
                        binaryPtr, binaryLen, socketPtr, socketLen, sessionPtr, sessionLen, &handle)
                }
            }
        }
        try check(status, "termy_tmux_control_open")
        self.handle = handle
    }

    deinit {
        close()
    }

    /// Returns notifications received since the last poll without blocking.
    public func poll() throws(TermyError) -> [TmuxNotification] {
        guard let handle else { return [] }
        var batch = TermyFfiTmuxNotificationBatch()
        let status = termy_tmux_control_poll(handle, &batch)
        defer { termy_tmux_control_notifications_free(&batch) }
        try check(status, "termy_tmux_control_poll")
        return UnsafeBufferPointer(start: batch.notifications_ptr, count: batch.notifications_len)
            .map(TmuxNotification.init)
    }

    /// Runs a tmux command and returns its output.
    ///
    /// The command must be a single line; one containing a newline, carriage
    /// return or NUL throws `.invalidArgument`. Use `send(arguments:)` to pass
    /// untrusted text, such as typed input for `send-keys -l`. This blocks
    /// until tmux answers (up to 5 seconds), so keep it off the main thread
    /// for anything latency-sensitive.
    @discardableResult
    public func send(_ command: String) throws(TermyError) -> String {
        guard let handle else { throw TermyError(code: .nullPointer, operation: "termy_tmux_control_send") }
        return try withUTF8(command) { (pointer, length) throws(TermyError) -> String in
            try takeString("termy_tmux_control_send") { termy_tmux_control_send(handle, pointer, length, $0) }
        }
    }

    /// Runs a tmux command built from arguments, quoting each one the way
    /// tmux parses them, and returns its output.
    @discardableResult
    public func send(arguments: [String]) throws(TermyError) -> String {
        try send(arguments.map(Self.quoted).joined(separator: " "))
    }

    /// Quotes one argument for tmux's command parser.
    static func quoted(_ argument: String) -> String {
        let plain = Set("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_./:@%+,=".unicodeScalars)
        if !argument.isEmpty, argument.unicodeScalars.allSatisfy({ plain.contains($0) }) { return argument }
        return "'" + argument.replacingOccurrences(of: "'", with: "'\\''") + "'"
    }

    public func close() {
        guard let handle else { return }
        termy_tmux_control_close(handle)
        self.handle = nil
    }
}
