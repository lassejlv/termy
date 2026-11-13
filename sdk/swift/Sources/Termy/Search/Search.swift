internal import CTermy

public struct SearchOptions: Hashable, Sendable {
    public var caseSensitive: Bool
    /// Treat the query as a regular expression. Invalid patterns match nothing.
    public var regex: Bool

    public init(caseSensitive: Bool = false, regex: Bool = false) {
        self.caseSensitive = caseSensitive
        self.regex = regex
    }
}

/// A match in history plus the live screen.
public struct SearchMatch: Hashable, Sendable {
    /// Zero-based buffer row, oldest history row first. Not a viewport row; see
    /// `viewportRow(historySize:displayOffset:)`.
    public var row: Int
    /// Inclusive columns.
    public var startCol: Int
    public var endCol: Int
    public var line: String

    /// The match's viewport row, or nil when it is scrolled out of view.
    public func viewportRow(historySize: Int, displayOffset: Int, rows: Int) -> Int? {
        let viewportRow = row - (historySize - displayOffset)
        return (0..<rows).contains(viewportRow) ? viewportRow : nil
    }

    init(_ match: TermyFfiSearchMatch) {
        row = match.row
        startCol = match.start_col
        endCol = match.end_col
        line = match.line.copyString()
    }
}

/// An OSC 8 hyperlink found at a viewport position.
public struct Hyperlink: Hashable, Sendable {
    /// Inclusive columns of the link on that row.
    public var startCol: Int
    public var endCol: Int
    /// Untrusted terminal content. Validate the scheme before opening it.
    public var uri: String
}

/// A link at a viewport position: an OSC 8 hyperlink, or a URL, domain or
/// existing file path found in the text. File paths are `file://` URLs.
public struct TerminalLink: Hashable, Sendable {
    /// Inclusive viewport cells, clipped to the viewport. A link wrapped
    /// across rows starts and ends on different rows.
    public var start: GridPosition
    public var end: GridPosition
    /// Untrusted terminal content. Validate the scheme before opening it.
    public var uri: String

    public init(start: GridPosition, end: GridPosition, uri: String) {
        self.start = start
        self.end = end
        self.uri = uri
    }

    public func contains(_ position: GridPosition) -> Bool {
        position >= start && position <= end
    }
}

/// Searches a terminal's history and screen from any thread, while the
/// terminal is driven from its own. It holds its own reference to the engine
/// (or, for a session-host pane, its own connection to the host), so it stays
/// usable after the terminal goes away. Make one with `Terminal.searcher()`.
public final class TerminalSearcher: @unchecked Sendable {
    // libtermy allows any number of concurrent searches on one handle.
    private let handle: OpaquePointer

    init(handle: OpaquePointer) {
        self.handle = handle
    }

    deinit {
        termy_searcher_free(handle)
    }

    /// As `Terminal.search(_:options:)`, on the calling thread. A long
    /// history is copied in chunks, so output keeps parsing meanwhile.
    public func search(_ query: String, options: SearchOptions = SearchOptions()) throws(TermyError) -> [SearchMatch] {
        var batch = TermyFfiSearchBatch()
        let ffiOptions = TermyFfiSearchOptions(case_sensitive: options.caseSensitive, regex: options.regex)
        let status = withUTF8(query) { termy_searcher_search(handle, $0, $1, ffiOptions, &batch) }
        defer { termy_search_batch_free(&batch) }
        try check(status, "termy_searcher_search")
        return UnsafeBufferPointer(start: batch.matches_ptr, count: batch.matches_len).map(SearchMatch.init)
    }
}

extension Terminal {
    /// A searcher to run on another thread or task; see `TerminalSearcher`.
    public func searcher() throws(TermyError) -> TerminalSearcher {
        var handle: OpaquePointer?
        try check(termy_terminal_searcher_new(self.handle, &handle), "termy_terminal_searcher_new")
        guard let handle else { throw TermyError(code: .nullPointer, operation: "termy_terminal_searcher_new") }
        return TerminalSearcher(handle: handle)
    }
}
