import Foundation

/// Drives a `Terminal` on the main actor: drains events, keeps a retained
/// `FrameStore` current, and tracks title, directory, progress and status.
///
/// PTY sessions poll automatically when the child produces output. Display
/// sessions poll after each `feed(_:)`.
@MainActor
public final class TerminalSession {
    public let terminal: Terminal
    public let frame = FrameStore()

    public private(set) var title: String?
    public private(set) var workingDirectory: String?
    public private(set) var progress: TerminalProgress = .clear
    public private(set) var programStatus: [ProgramStatusRecord] = []
    public private(set) var hasExited = false
    public private(set) var size: TerminalSize

    /// Called after the retained frame changes visibly.
    public var onFrameChange: ((FrameChange) -> Void)?
    /// Called for every drained event, after session state is updated.
    public var onEvent: ((TerminalEvent) -> Void)?
    /// Services Kitty OSC 5522 clipboard requests. Reads are denied when nil.
    public weak var clipboard: KittyClipboardHandler?
    /// Receives protocol replies for display sessions. Send them to the program.
    public var onProtocolReply: ((Data) -> Void)?

    /// Events drained per poll before yielding back to the run loop.
    public var maxBatchesPerPoll = 8

    private var pollScheduled = false
    private var isPolling = false
    private var pollRequestedWhilePolling = false
    private var forceFullOnNextPoll = true

    /// Spawns a shell in a PTY and starts polling on output.
    public convenience init(size: TerminalSize, options: TerminalOptions = TerminalOptions()) throws(TermyError) {
        self.init(terminal: try Terminal(size: size, options: options), size: size)
    }

    /// A session without a PTY. Feed output with `feed(_:)`; read input from
    /// `terminal.onInput` and replies from `onProtocolReply`.
    public static func display(size: TerminalSize) throws(TermyError) -> TerminalSession {
        TerminalSession(terminal: try Terminal.display(size: size), size: size)
    }

    /// Wraps an existing terminal. `size` must match the size it was created with.
    public init(terminal: Terminal, size: TerminalSize) {
        self.terminal = terminal
        self.size = size
        terminal.startWakeupMonitor { [weak self] in
            Task { @MainActor in self?.schedulePoll() }
        }
    }

    /// Stops the wakeup thread. The session can no longer react to output.
    public func stop() {
        terminal.stopWakeupMonitor()
    }

    // MARK: - Polling

    /// Coalesces poll requests into one run-loop turn.
    public func schedulePoll() {
        guard !pollScheduled else { return }
        pollScheduled = true
        Task { @MainActor [weak self] in
            guard let self else { return }
            self.pollScheduled = false
            self.poll()
        }
    }

    /// Drains pending events and applies the next frame update now.
    public func poll(forceFull: Bool = false) {
        // A clipboard handler that spins a run loop (a modal alert, say) can
        // let a wakeup poll again mid-drain; defer it to after this one.
        guard !isPolling, !terminal.isDraining else {
            pollRequestedWhilePolling = true
            if forceFull { forceFullOnNextPoll = true }
            return
        }
        isPolling = true
        defer {
            isPolling = false
            if pollRequestedWhilePolling {
                pollRequestedWhilePolling = false
                schedulePoll()
            }
        }
        var batches = 0
        var hasMore = true
        while hasMore, batches < maxBatchesPerPoll {
            guard let batch = try? terminal.drainEvents(
                clipboard: clipboard, onProtocolReply: protocolReplyHandler
            ) else { break }
            batch.events.forEach(handle)
            hasMore = batch.hasMore
            batches += 1
        }
        // Under sustained output, finish the rest on a later turn.
        if hasMore { schedulePoll() }

        let force = forceFull || forceFullOnNextPoll
        guard let change = try? terminal.updateFrame(frame, forceFull: force) else { return }
        forceFullOnNextPoll = false
        if change.needsDisplay { onFrameChange?(change) }
    }

    private var protocolReplyHandler: ((Data) -> Void)? {
        guard terminal.mode == .display else { return nil }
        return { [weak self] reply in
            guard let self else { return }
            if let onProtocolReply = self.onProtocolReply {
                onProtocolReply(reply)
            } else {
                self.terminal.onInput?(reply)
            }
        }
    }

    private func handle(_ event: TerminalEvent) {
        switch event {
        case .title(let title): self.title = title
        case .resetTitle: title = nil
        case .workingDirectory(let path): workingDirectory = path
        case .progress(let progress): self.progress = progress
        case .programStatus(let records): programStatus = records
        case .exit:
            hasExited = true
            terminal.stopWakeupMonitor()
        default: break
        }
        onEvent?(event)
    }

    // MARK: - Display sessions

    /// Feeds output to a display session and polls.
    public func feed(_ output: some DataProtocol) throws(TermyError) {
        try terminal.feed(output)
        poll()
    }

    // MARK: - Input and viewport

    public func send(_ keystroke: Keystroke, optionAsAlt: Bool? = nil) throws(TermyError) {
        try terminal.send(keystroke, optionAsAlt: optionAsAlt)
        if terminal.mode == .pty, frame.displayOffset > 0, (try? terminal.scrollToBottom()) == true {
            poll()
        }
    }

    public func paste(_ text: String) throws(TermyError) {
        try terminal.paste(text)
    }

    /// Reports a mouse event if the program tracks it. Returns false when the
    /// host should handle the event itself.
    @discardableResult
    public func send(_ mouse: MouseInput) throws(TermyError) -> Bool {
        try terminal.send(mouse)
    }

    /// Scrolls local history; positive values move toward older rows.
    public func scroll(lines: Int32) {
        if (try? terminal.scroll(lines: lines)) == true { poll() }
    }

    public func scrollToBottom() {
        if (try? terminal.scrollToBottom()) == true { poll() }
    }

    /// Resizes the grid and redraws with a full frame.
    public func resize(_ newSize: TerminalSize) throws(TermyError) {
        guard newSize != size else { return }
        try terminal.resize(newSize)
        size = newSize
        forceFullOnNextPoll = true
        poll()
    }
}
