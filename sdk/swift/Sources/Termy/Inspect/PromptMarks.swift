internal import CTermy

/// A buffer line OSC 133 shell integration marked: the shell's prompt (133;A)
/// or a command's output (133;C) started on it while the cursor was there.
/// Marks follow their rows into history and through reflow; restoring a
/// terminal from a snapshot drops them.
public struct PromptMark: Hashable, Sendable {
    public enum Kind: UInt8, Sendable {
        /// OSC 133;A: the shell's prompt starts on the line.
        case prompt = 1
        /// OSC 133;C: the command's output starts on the line.
        case output = 2
    }

    /// Numbered like `lineBounds`: 0 is the top screen row, negative lines are history (-1 the newest).
    public var line: Int
    public var kinds: Set<Kind>   // a row can carry both

    public init(line: Int, kinds: Set<Kind>) {
        self.line = line
        self.kinds = kinds
    }

    init(_ mark: TermyFfiPromptMark) {
        line = Int(mark.line)
        kinds = Set([Kind.prompt, .output].filter { mark.kinds & $0.rawValue != 0 })
    }
}

extension Terminal {
    /// The first and last line of the buffer (history included).
    public func lineBounds() throws(TermyError) -> ClosedRange<Int> {
        var first: Int32 = 0
        var last: Int32 = 0
        try check(termy_terminal_line_bounds(handle, &first, &last), "termy_terminal_line_bounds")
        return Int(first)...Int(max(first, last))
    }

    /// Prompt marks on lines `lines`, oldest first. Nil when this terminal can't report them
    /// (a session host of another build).
    public func promptMarks(in lines: ClosedRange<Int>) throws(TermyError) -> [PromptMark]? {
        var supported = false
        var batch = TermyFfiPromptMarkBatch()
        let status = termy_terminal_prompt_marks(
            handle, Int32(clamping: lines.lowerBound), Int32(clamping: lines.upperBound), &supported, &batch)
        defer { termy_prompt_mark_batch_free(&batch) }
        try check(status, "termy_terminal_prompt_marks")
        guard supported else { return nil }
        return UnsafeBufferPointer(start: batch.marks_ptr, count: batch.marks_len).map(PromptMark.init)
    }

    /// The text of `lines`: soft-wrapped lines joined without a newline, other lines ended by "\n"
    /// (none after the last), trailing blanks of each line dropped.
    public func text(of lines: ClosedRange<Int>) throws(TermyError) -> String {
        try takeString("termy_terminal_line_range_text") {
            termy_terminal_line_range_text(
                handle, Int32(clamping: lines.lowerBound), Int32(clamping: lines.upperBound), $0)
        }
    }
}
