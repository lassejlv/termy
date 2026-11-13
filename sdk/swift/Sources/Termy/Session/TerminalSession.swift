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
    /// Goes up each time a poll applies a frame update that changed the
    /// terminal's content: cells, the scrollback position or history, or
    /// the grid size. Cursor moves and blinks leave it alone, so a host can
    /// skip work, such as searching again, while nothing changed.
    public private(set) var contentGeneration: UInt64 = 0

    /// Called after the retained frame changes visibly.
    public var onFrameChange: ((FrameChange) -> Void)?
    /// Called when Kitty graphics change. An image placed without moving the
    /// cursor, or deleted, changes no cell, so `onFrameChange` stays silent.
    public var onGraphicsChange: (() -> Void)?
    /// Called for every drained event, after session state is updated.
    public var onEvent: ((TerminalEvent) -> Void)?
    /// Services Kitty OSC 5522 clipboard requests. Reads are denied when nil.
    public weak var clipboard: KittyClipboardHandler?
    /// Receives protocol replies for display sessions. Send them to the program.
    public var onProtocolReply: ((Data) -> Void)?

    /// Replay scrolled output on `frame` and report it in
    /// `FrameChange.scrolls`, leaving the moved rows out of the damage. Set it
    /// when `onFrameChange` keeps per-row work it can move along.
    public var replaysScrolls = false

    /// Events drained per poll before yielding back to the run loop.
    public var maxBatchesPerPoll = 8
    /// The shortest time between polls that output wakes, a 120 Hz refresh.
    /// A local PTY wakes on every read; output arriving faster than this is
    /// taken by the next poll, so a flood costs a frame update per refresh,
    /// not one per read, and leaves the engine lock to the parser.
    public var minimumPollInterval: TimeInterval = 1.0 / 120

    private var pollScheduled = false
    private var lastPoll: TimeInterval = 0
    private var isPolling = false
    private var pollRequestedWhilePolling = false
    private var forceFullOnNextPoll = true
    private var graphicsRevision: UInt64?

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

    /// Coalesces poll requests into one run-loop turn, at most one per
    /// `minimumPollInterval`.
    public func schedulePoll() {
        guard !pollScheduled else { return }
        pollScheduled = true
        let wait = lastPoll + minimumPollInterval - ProcessInfo.processInfo.systemUptime
        guard wait > 0 else {
            Task { @MainActor [weak self] in self?.runScheduledPoll() }
            return
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + wait) { [weak self] in
            MainActor.assumeIsolated { self?.runScheduledPoll() }
        }
    }

    private func runScheduledPoll() {
        pollScheduled = false
        poll()
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
        lastPoll = ProcessInfo.processInfo.systemUptime
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
        if let change = try? terminal.updateFrame(frame, forceFull: force, replayingScrolls: replaysScrolls) {
            forceFullOnNextPoll = false
            if change.contentChanged { contentGeneration &+= 1 }
            if change.needsDisplay { onFrameChange?(change) }
        }
        if let revision = try? terminal.kittyGraphicsRevision(), revision != graphicsRevision {
            graphicsRevision = revision
            onGraphicsChange?()
        }
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

    // MARK: - Search

    /// `terminal.search(_:options:)` off the main actor: the scan of a long
    /// history, or a session host's round trip, doesn't hold up the main
    /// thread, and output keeps arriving meanwhile. Matches are numbered as
    /// the history was when the search began.
    public func search(_ query: String, options: SearchOptions = SearchOptions()) async throws(TermyError) -> [SearchMatch] {
        // One searcher per session: a session-host pane's searcher has its
        // own connection to the host, which every search would open again.
        let searcher: TerminalSearcher
        if let cached = cachedSearcher {
            searcher = cached
        } else {
            searcher = try terminal.searcher()
            cachedSearcher = searcher
        }
        let result = await Task.detached { () -> Result<[SearchMatch], TermyError> in
            do throws(TermyError) {
                return .success(try searcher.search(query, options: options))
            } catch {
                return .failure(error)
            }
        }.value
        // A searcher that failed (its connection may be gone) is made anew.
        if case .failure = result, cachedSearcher === searcher { cachedSearcher = nil }
        return try result.get()
    }

    private var cachedSearcher: TerminalSearcher?

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
