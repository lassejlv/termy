import Foundation
internal import CTermy

/// A libtermy terminal: a VT parser and screen, optionally driving a PTY.
///
/// - `Terminal(size:options:)` spawns a shell (or `startupCommand`) in a PTY.
/// - `Terminal.display(size:)` is parser/screen only. Feed it output from your
///   own transport (SSH, tmux, a recording) and route input with `onInput`.
///
/// A terminal is not thread-safe. Confine each instance to one thread or actor;
/// `TerminalSession` does this on the main actor. The wakeup monitor is the
/// only concurrent piece and is managed for you.
public final class Terminal {
    public enum Mode: Hashable, Sendable {
        /// Drives a program through a PTY: its own, or one a multiplexer host
        /// runs (see `paneID`).
        case pty
        /// No PTY; output is fed by the host.
        case display
    }

    public let mode: Mode
    /// The multiplexer pane this terminal views, when a session host runs its
    /// program. Keep it to attach again after a relaunch.
    public let paneID: String?
    let handle: OpaquePointer
    private var wakeupMonitor: WakeupMonitor?

    /// Receives input bytes (keys, mouse reports, pastes) for display-only
    /// terminals. PTY terminals write input to their child directly.
    public var onInput: ((Data) -> Void)?

    /// True while `drainEvents` runs, including inside its clipboard
    /// callbacks. libtermy holds the clipboard lock there, so draining again
    /// or pasting from a callback would deadlock; those calls throw instead.
    public private(set) var isDraining = false

    private func checkNotDraining(_ operation: @autoclosure () -> String) throws(TermyError) {
        guard !isDraining else {
            throw TermyError(code: .invalidArgument, operation: "\(operation()) (re-entered from a drain callback)")
        }
    }

    /// Starts a shell, or `options.startupCommand`, in a new PTY.
    public init(size: TerminalSize, options: TerminalOptions = TerminalOptions()) throws(TermyError) {
        var handle: OpaquePointer?
        let status = Self.withFFIOptions(options) { termy_terminal_new_with_options(size.ffi, $0, &handle) }
        try check(status, "termy_terminal_new_with_options")
        guard let handle else { throw TermyError(code: .nullPointer, operation: "termy_terminal_new_with_options") }
        self.handle = handle
        self.mode = .pty
        self.paneID = nil
    }

    private init(displayHandle: OpaquePointer) {
        handle = displayHandle
        mode = .display
        paneID = nil
    }

    /// Wraps a terminal attached to a multiplexer pane.
    init(multiplexerHandle: OpaquePointer, paneID: String) {
        handle = multiplexerHandle
        mode = .pty
        self.paneID = paneID
    }

    /// Passes `options` as C options valid for `body`.
    static func withFFIOptions<R>(_ options: TerminalOptions, _ body: (UnsafePointer<TermyFfiTerminalOptions>) -> R) -> R {
        let arena = ByteArena()
        let environment = options.environment.sorted { $0.key < $1.key }.map { key, value in
            let key = arena.slice(key)
            let value = arena.slice(value)
            return TermyFfiEnvVar(key_ptr: key.ptr, key_len: key.len, value_ptr: value.ptr, value_len: value.len)
        }
        let workingDirectory = options.workingDirectory.map { arena.slice($0) }
        let startupCommand = options.startupCommand.map { arena.slice($0) }
        var ffiOptions = TermyFfiTerminalOptions(
            config: options.config?.handle,
            working_directory_ptr: workingDirectory?.ptr,
            working_directory_len: workingDirectory?.len ?? 0,
            startup_command_ptr: startupCommand?.ptr,
            startup_command_len: startupCommand?.len ?? 0,
            env_vars_ptr: arena.array(environment),
            env_vars_len: environment.count
        )
        return withExtendedLifetime((arena, options.config)) { body(&ffiOptions) }
    }

    /// A parser/screen without a PTY. Drain events and take frames after each
    /// `feed(_:)`. Its wake channel fires when a synchronized update (`?2026`)
    /// times out and commits without further output.
    public static func display(size: TerminalSize) throws(TermyError) -> Terminal {
        var handle: OpaquePointer?
        try check(termy_display_terminal_new(size.ffi, &handle), "termy_display_terminal_new")
        guard let handle else { throw TermyError(code: .nullPointer, operation: "termy_display_terminal_new") }
        return Terminal(displayHandle: handle)
    }

    deinit {
        // The waiter must be joined before the handle is freed.
        wakeupMonitor?.stop()
        termy_terminal_free(handle)
    }

    // MARK: - Output and input

    /// Feeds program output to a display-only terminal.
    public func feed(_ output: some DataProtocol) throws(TermyError) {
        let status = withBytes(output) { termy_terminal_feed_output(handle, $0, $1) }
        try check(status, "termy_terminal_feed_output")
    }

    public func feed(_ output: String) throws(TermyError) {
        try feed(Array(output.utf8))
    }

    /// Sends raw input to the program: the PTY, or `onInput` for display terminals.
    public func write(_ input: some DataProtocol) throws(TermyError) {
        guard !input.isEmpty else { return }
        if mode == .display {
            onInput?(Data(input))
            return
        }
        let status = withBytes(input) { termy_terminal_write(handle, $0, $1) }
        try check(status, "termy_terminal_write")
    }

    public func write(_ input: String) throws(TermyError) {
        try write(Array(input.utf8))
    }

    /// Encodes a key for the program's current keyboard mode. An empty result
    /// is valid and means there is nothing to send.
    public func encode(_ keystroke: Keystroke, optionAsAlt: Bool? = nil) throws(TermyError) -> Data {
        try withUTF8(keystroke.key) { (keyPtr, keyLen) throws(TermyError) -> Data in
            try withOptionalUTF8(keystroke.text) { (textPtr, textLen) throws(TermyError) -> Data in
                var ffi = TermyFfiKeystroke(
                    control: keystroke.modifiers.contains(.control),
                    alt: keystroke.modifiers.contains(.alt),
                    shift: keystroke.modifiers.contains(.shift),
                    platform: keystroke.modifiers.contains(.platform),
                    function: keystroke.modifiers.contains(.function),
                    key_ptr: keyPtr, key_len: keyLen,
                    key_char_ptr: textPtr, key_char_len: textLen,
                    event_kind: keystroke.kind.rawValue
                )
                if let optionAsAlt {
                    return try takeBytes("termy_terminal_encode_key_with_options") {
                        termy_terminal_encode_key_with_options(handle, &ffi, optionAsAlt, $0)
                    }
                }
                return try takeBytes("termy_terminal_encode_key") {
                    termy_terminal_encode_key(handle, &ffi, $0)
                }
            }
        }
    }

    /// Encodes and sends a key.
    public func send(_ keystroke: Keystroke, optionAsAlt: Bool? = nil) throws(TermyError) {
        try write(encode(keystroke, optionAsAlt: optionAsAlt))
    }

    /// Encodes a mouse event for the program's current mouse mode. An empty
    /// result means the program is not tracking this event; handle it locally
    /// (for example, as selection or scrollback).
    public func encode(_ mouse: MouseInput) throws(TermyError) -> Data {
        var ffi = TermyFfiMouseInput(
            kind: mouse.kind.rawKind,
            button: mouse.kind.rawButton,
            col: mouse.position.col,
            row: mouse.position.row,
            control: mouse.modifiers.contains(.control),
            alt: mouse.modifiers.contains(.alt),
            shift: mouse.modifiers.contains(.shift)
        )
        return try takeBytes("termy_terminal_encode_mouse") { termy_terminal_encode_mouse(handle, &ffi, $0) }
    }

    /// Sends a mouse event if the program tracks it. Returns false when the
    /// event was not reported.
    @discardableResult
    public func send(_ mouse: MouseInput) throws(TermyError) -> Bool {
        let bytes = try encode(mouse)
        guard mode == .pty, !bytes.isEmpty else {
            try write(bytes)
            return !bytes.isEmpty
        }
        switch mouse.kind {
        case .move, .drag:
            // Motion may use only part of the input backlog, so a program
            // that stops reading can't make the pointer starve later keys.
            let status = withBytes(bytes) { termy_terminal_write_mouse_motion(handle, $0, $1) }
            try check(status, "termy_terminal_write_mouse_motion")
        default:
            try write(bytes)
        }
        return true
    }

    /// Whether the program asked for bracketed paste (mode 2004).
    public var isBracketedPasteEnabled: Bool {
        var enabled = false
        return termy_terminal_bracketed_paste_mode(handle, &enabled) == TERMY_FFI_OK && enabled
    }

    /// Encodes pasted text: line endings become Enter, control characters are
    /// dropped so the paste cannot run commands or end the bracket early, and
    /// the text is wrapped in bracketed-paste markers when the program
    /// requested them.
    public func encodePaste(_ text: String) throws(TermyError) -> Data {
        try withUTF8(text) { (textPtr, textLen) throws(TermyError) -> Data in
            try takeBytes("termy_terminal_encode_paste") {
                termy_terminal_encode_paste(handle, textPtr, textLen, $0)
            }
        }
    }

    /// Pastes text encoded by `encodePaste(_:)`.
    ///
    /// When the program enabled Kitty clipboard paste events (mode 5522), it
    /// is told a paste is available and reads the clipboard itself through
    /// OSC 5522, so the text is not also typed.
    public func paste(_ text: String, availableFormats: [String] = ["text/plain"]) throws(TermyError) {
        try checkNotDraining("termy_terminal_paste")
        if try sendKittyClipboardPasteEvent(location: .clipboard, availableFormats: availableFormats) { return }
        try write(encodePaste(text))
    }

    /// The process ID of the program this terminal started in its own PTY.
    /// Nil for display terminals, for session-host panes (the host runs
    /// their program; ask `Multiplexer.panes()`), and once the PTY is gone.
    public var childPID: Int32? {
        var pid: Int32 = 0
        guard termy_terminal_child_pid(handle, &pid) == TERMY_FFI_OK, pid > 0 else { return nil }
        return pid
    }

    // MARK: - Size and scrollback

    public func resize(_ size: TerminalSize) throws(TermyError) {
        try check(termy_terminal_resize(handle, size.ffi), "termy_terminal_resize")
    }

    /// Scrolls the viewport. Positive values move into older history. Returns
    /// true when the viewport moved.
    @discardableResult
    public func scroll(lines: Int32) throws(TermyError) -> Bool {
        var changed = false
        try check(termy_terminal_scroll_display(handle, lines, &changed), "termy_terminal_scroll_display")
        return changed
    }

    @discardableResult
    public func scrollToBottom() throws(TermyError) -> Bool {
        var changed = false
        try check(termy_terminal_scroll_to_bottom(handle, &changed), "termy_terminal_scroll_to_bottom")
        return changed
    }

    @discardableResult
    public func clearScrollback() throws(TermyError) -> Bool {
        var changed = false
        try check(termy_terminal_clear_scrollback(handle, &changed), "termy_terminal_clear_scrollback")
        return changed
    }

    /// Limits retained history, for example to save memory in hidden tabs.
    public func setScrollbackHistory(_ rows: Int) throws(TermyError) {
        try check(termy_terminal_set_scrollback_history(handle, rows), "termy_terminal_set_scrollback_history")
    }

    // MARK: - Frames

    /// A complete copy of the viewport. Prefer `takeFrameUpdate` in render loops.
    public func snapshot() throws(TermyError) -> Frame {
        var frame = TermyFfiFrame()
        let status = termy_terminal_snapshot(handle, &frame)
        defer { termy_frame_free(&frame) }
        try check(status, "termy_terminal_snapshot")
        return Frame(frame)
    }

    /// Changed cells since the last update. Pass `forceFull` for the first
    /// frame, after a resize, or to recover a lost cache.
    public func takeFrameUpdate(forceFull: Bool = false) throws(TermyError) -> FrameUpdate {
        var update = TermyFfiFrameUpdate()
        let status = termy_terminal_take_frame_update(handle, forceFull, &update)
        defer { termy_frame_update_free(&update) }
        try check(status, "termy_terminal_take_frame_update")
        return FrameUpdate(update)
    }

    /// Takes a frame update and applies it to `store` without copying cells
    /// through Swift values. Recovers with a forced full frame when the store
    /// cannot apply a partial update.
    ///
    /// With `replayingScrolls`, output that scrolled is replayed on the store
    /// and reported in `FrameChange.scrolls`, and `damage` names only the
    /// cells patched after it: rows the scroll moved are not in it. Pass it
    /// when you keep per-row work, such as drawn rows, that you can move too.
    @discardableResult
    public func updateFrame(
        _ store: FrameStore, forceFull: Bool = false, replayingScrolls: Bool = false
    ) throws(TermyError) -> FrameChange {
        let force = forceFull || store.isEmpty
        if let change = try applyUpdate(to: store, forceFull: force, replayingScrolls: replayingScrolls) {
            return change
        }
        guard let change = try applyUpdate(to: store, forceFull: true, replayingScrolls: false) else {
            throw TermyError(code: .invalidArgument, operation: "termy_terminal_take_frame_update")
        }
        return change
    }

    private func applyUpdate(
        to store: FrameStore, forceFull: Bool, replayingScrolls: Bool
    ) throws(TermyError) -> FrameChange? {
        var update = TermyFfiFrameUpdate()
        guard replayingScrolls else {
            let status = termy_terminal_take_frame_update(handle, forceFull, &update)
            defer { termy_frame_update_free(&update) }
            try check(status, "termy_terminal_take_frame_update")
            return try? store.apply(update)
        }
        var batch = TermyFfiViewportScrollBatch()
        let status = termy_terminal_take_frame_update_with_scrolls(handle, forceFull, &update, &batch)
        defer {
            termy_frame_update_free(&update)
            termy_viewport_scrolls_free(&batch)
        }
        try check(status, "termy_terminal_take_frame_update_with_scrolls")
        let raw = UnsafeBufferPointer(start: batch.scrolls_ptr, count: batch.scrolls_len)
        let scrolls = raw.compactMap(ViewportScroll.init)
        // A scroll this SDK can't read can't be replayed: start over.
        guard scrolls.count == raw.count else { return nil }
        return try? store.apply(update, scrolls: scrolls)
    }

    // MARK: - Events

    /// Drains one batch of events. Repeat while `hasMore` is true.
    ///
    /// Display-only terminals must pass `onProtocolReply` and send those bytes
    /// to their program: they answer queries such as cursor position and
    /// device attributes. Provide `clipboard` to service Kitty OSC 5522
    /// requests; without it, reads are denied.
    public func drainEvents(
        clipboard: KittyClipboardHandler? = nil,
        onProtocolReply: ((Data) -> Void)? = nil
    ) throws(TermyError) -> TerminalEventBatch {
        try checkNotDraining("termy_terminal_drain_events")
        isDraining = true
        defer { isDraining = false }
        var batch = TermyFfiEventBatch()
        defer { termy_event_batch_free(&batch) }
        if clipboard == nil, onProtocolReply == nil {
            try check(termy_terminal_drain_events(handle, &batch), "termy_terminal_drain_events")
            return TerminalEventBatch(batch)
        }
        let context = DrainContext(clipboard: clipboard, onProtocolReply: onProtocolReply)
        let status = withExtendedLifetime(context) {
            termy_terminal_drain_events_with_clipboard(
                handle,
                Unmanaged.passUnretained(context).toOpaque(),
                DrainContext.readCallback,
                DrainContext.writeCallback,
                DrainContext.protocolReplyCallback,
                &batch
            )
        }
        try check(status, "termy_terminal_drain_events_with_clipboard")
        return TerminalEventBatch(batch)
    }

    /// Whether the program enabled Kitty clipboard paste events (mode 5522).
    public var isKittyClipboardPasteEventsEnabled: Bool {
        var enabled = false
        return termy_terminal_kitty_clipboard_paste_events_enabled(handle, &enabled) == TERMY_FFI_OK && enabled
    }

    /// Tells the program which MIME types a paste offers, if it asked to know.
    @discardableResult
    public func sendKittyClipboardPasteEvent(
        location: ClipboardLocation, availableFormats: [String]
    ) throws(TermyError) -> Bool {
        try checkNotDraining("termy_terminal_send_kitty_clipboard_paste_event")
        let arena = ByteArena()
        let formats = availableFormats.map { arena.slice($0) }
        var sent = false
        let status = withExtendedLifetime(arena) {
            termy_terminal_send_kitty_clipboard_paste_event(
                handle, location.rawValue, arena.array(formats), formats.count, &sent)
        }
        try check(status, "termy_terminal_send_kitty_clipboard_paste_event")
        return sent
    }

    // MARK: - Wakeups

    /// Calls `onWake` from a background thread whenever the terminal has new
    /// work: PTY output, or for display terminals a synchronized update that
    /// timed out. Hop to the terminal's thread before touching it.
    public func startWakeupMonitor(onWake: @escaping @Sendable () -> Void) {
        wakeupMonitor?.stop()
        let monitor = WakeupMonitor(handle: handle, onWake: onWake)
        wakeupMonitor = monitor
        monitor.start()
    }

    /// Stops and joins the wakeup thread.
    public func stopWakeupMonitor() {
        wakeupMonitor?.stop()
        wakeupMonitor = nil
    }

    /// Enables or suppresses wake signals, for example while a tab is hidden.
    public func setWakeupEnabled(_ enabled: Bool) throws(TermyError) {
        try check(termy_terminal_set_wakeup_enabled(handle, enabled), "termy_terminal_set_wakeup_enabled")
    }

    // MARK: - Search, links, graphics, colors

    /// Searches history and the live screen. Case-insensitive literal by default.
    public func search(_ query: String, options: SearchOptions = SearchOptions()) throws(TermyError) -> [SearchMatch] {
        var batch = TermyFfiSearchBatch()
        let ffiOptions = TermyFfiSearchOptions(case_sensitive: options.caseSensitive, regex: options.regex)
        let status = withUTF8(query) { termy_terminal_search_with_options(handle, $0, $1, ffiOptions, &batch) }
        defer { termy_search_batch_free(&batch) }
        try check(status, "termy_terminal_search_with_options")
        return UnsafeBufferPointer(start: batch.matches_ptr, count: batch.matches_len).map(SearchMatch.init)
    }

    /// The OSC 8 hyperlink at a viewport cell, if any.
    public func hyperlink(at position: GridPosition) throws(TermyError) -> Hyperlink? {
        var found = false
        var link = TermyFfiHyperlink()
        let status = termy_terminal_hyperlink_at(handle, position.row, position.col, &found, &link)
        defer { termy_hyperlink_free(&link) }
        try check(status, "termy_terminal_hyperlink_at")
        guard found else { return nil }
        return Hyperlink(startCol: link.start_col, endCol: link.end_col, uri: link.uri.copyString())
    }

    /// The link at a viewport cell: an OSC 8 hyperlink, or a URL, domain or
    /// existing file path in the text, which may wrap across rows. Relative
    /// paths resolve against the shell's working directory. Path lookups touch
    /// the filesystem and a session host terminal asks its host, so call this
    /// on demand (a modifier hover, a click), not on every mouse move.
    public func link(at position: GridPosition) throws(TermyError) -> TerminalLink? {
        var found = false
        var link = TermyFfiLink()
        let status = termy_terminal_link_at(handle, position.row, position.col, &found, &link)
        defer { termy_link_free(&link) }
        try check(status, "termy_terminal_link_at")
        guard found else { return nil }
        return TerminalLink(
            start: GridPosition(row: link.start_row, col: link.start_col),
            end: GridPosition(row: link.end_row, col: link.end_col),
            uri: link.uri.copyString())
    }

    /// Changes whenever Kitty graphics placements change.
    public func kittyGraphicsRevision() throws(TermyError) -> UInt64 {
        var revision: UInt64 = 0
        try check(termy_terminal_kitty_graphics_revision(handle, &revision), "termy_terminal_kitty_graphics_revision")
        return revision
    }

    public func kittyGraphics() throws(TermyError) -> KittyGraphicsSnapshot {
        var batch = TermyFfiKittyGraphicsBatch()
        let status = termy_terminal_kitty_graphics_placements(handle, &batch)
        defer { termy_kitty_graphics_batch_free(&batch) }
        try check(status, "termy_terminal_kitty_graphics_placements")
        // Placements of one image share a PNG buffer; copy each buffer once so
        // placeholder images spanning many cells share one `Data`.
        var pngs: [UnsafeMutablePointer<UInt8>: Data] = [:]
        let placements = UnsafeBufferPointer(start: batch.placements_ptr, count: batch.placements_len)
            .map { placement -> KittyGraphicsPlacement in
                guard let ptr = placement.png.ptr else {
                    return KittyGraphicsPlacement(placement, png: Data())
                }
                if let png = pngs[ptr] {
                    return KittyGraphicsPlacement(placement, png: png)
                }
                let png = placement.png.copyData()
                pngs[ptr] = png
                return KittyGraphicsPlacement(placement, png: png)
            }
        return KittyGraphicsSnapshot(
            revision: batch.revision, placements: placements,
            cellWidth: batch.cell_width, cellHeight: batch.cell_height)
    }

    /// The placements, with pixels as raw RGBA and none of the PNG encoding
    /// `kittyGraphics()` does. Images in `held` come without pixels: pass the
    /// keys of the images you keep, so each one is copied once.
    public func kittyGraphics(holding held: Set<KittyImageKey>) throws(TermyError) -> KittyGraphicsSnapshot {
        var batch = TermyFfiKittyGraphicsBatch()
        let known = held.map { TermyFfiKittyImageKey(image_id: $0.imageID, image_generation: $0.generation) }
        let status = known.withUnsafeBufferPointer { known in
            termy_terminal_kitty_graphics_placements_rgba(handle, known.baseAddress, known.count, &batch)
        }
        defer { termy_kitty_graphics_batch_free(&batch) }
        try check(status, "termy_terminal_kitty_graphics_placements_rgba")
        // Placements of one image share a buffer; copy each buffer once.
        var buffers: [UnsafeMutablePointer<UInt8>: Data] = [:]
        func copy(_ bytes: TermyFfiBytes) -> Data? {
            guard let ptr = bytes.ptr else { return nil }
            if let data = buffers[ptr] { return data }
            let data = bytes.copyData()
            buffers[ptr] = data
            return data
        }
        let placements = UnsafeBufferPointer(start: batch.placements_ptr, count: batch.placements_len)
            .map { KittyGraphicsPlacement($0, png: copy($0.png) ?? Data(), rgba: copy($0.rgba)) }
        return KittyGraphicsSnapshot(
            revision: batch.revision, placements: placements,
            cellWidth: batch.cell_width, cellHeight: batch.cell_height)
    }

    /// Applies a config's theme colors, resolved for `appearance`.
    public func applyColors(from config: TermyConfig, appearance: Appearance) throws(TermyError) {
        let status = termy_terminal_apply_config_colors_for_appearance(handle, config.handle, appearance.rawValue)
        try check(status, "termy_terminal_apply_config_colors_for_appearance")
    }

    /// Reloads colors from the user's config file.
    public func reloadDefaultConfigColors() throws(TermyError) {
        try check(termy_terminal_reload_default_config_colors(handle), "termy_terminal_reload_default_config_colors")
    }
}
