import Termy
import TermyRemoteKit

/// Finds prompts and command output among a pane's prompt marks.
///
/// Lines are numbered like `Terminal.lineBounds`: 0 is the top screen row,
/// negative lines are history. A command's prompt is a `.prompt` mark (OSC
/// 133;A); its output starts at the next `.output` mark (133;C) and runs to
/// the line before the next prompt.
enum PromptNavigation {
    /// The prompt to scroll to from a view whose top row is `viewTop`.
    static func target(_ marks: [PromptMark], from viewTop: Int, forward: Bool) -> Int? {
        let prompts = marks.filter { $0.kinds.contains(.prompt) }.map(\.line)
        return forward ? prompts.first { $0 > viewTop } : prompts.last { $0 < viewTop }
    }

    /// The lines a command printed: the one whose prompt is at `promptLine`,
    /// or, when nil, the newest command with output. `lastLine` ends the
    /// output of a command no prompt followed yet.
    static func output(_ marks: [PromptMark], prompt promptLine: Int?, lastLine: Int) -> ClosedRange<Int>? {
        let prompts = marks.filter { $0.kinds.contains(.prompt) }.map(\.line)
        func range(after prompt: Int) -> ClosedRange<Int>? {
            let next = prompts.first { $0 > prompt }
            guard
                let start = marks.first(where: {
                    $0.kinds.contains(.output) && $0.line >= prompt && $0.line < (next ?? Int.max)
                })?.line
            else { return nil }
            let end = (next ?? lastLine + 1) - 1
            return start <= end ? start...end : nil
        }
        if let promptLine {
            // Output may have scrolled the buffer a little since the phone looked.
            guard let nearest = prompts.min(by: { abs($0 - promptLine) < abs($1 - promptLine) }),
                abs(nearest - promptLine) <= 3
            else { return nil }
            return range(after: nearest)
        }
        for prompt in prompts.reversed() {
            if let found = range(after: prompt) { return found }
        }
        return nil
    }

    /// Output text without the blank lines a command leaves before the next prompt.
    static func trimmed(_ text: String) -> String {
        var lines = text.split(separator: "\n", omittingEmptySubsequences: false)
        while let last = lines.last, last.allSatisfy(\.isWhitespace) { lines.removeLast() }
        return lines.joined(separator: "\n")
    }

    /// The most lines one output reply reads; longer output keeps its end.
    static let maxOutputLines = 20_000

    // MARK: Reading a terminal

    /// Screen rows of the view where a prompt starts; nil when the terminal can't tell.
    static func promptRows(in terminal: Terminal, displayOffset: Int, rows: Int) -> [Int]? {
        guard rows > 0 else { return [] }
        let top = -displayOffset
        guard let marks = (try? terminal.promptMarks(in: top...(top + rows - 1))) ?? nil else { return nil }
        return marks.filter { $0.kinds.contains(.prompt) }.map { $0.line - top }.filter { (0..<rows).contains($0) }
    }

    /// The display offset that puts the previous or next prompt at the top of the view.
    static func jumpOffset(in terminal: Terminal, displayOffset: Int, forward: Bool) -> Int? {
        guard let bounds = try? terminal.lineBounds(), let marks = (try? terminal.promptMarks(in: bounds)) ?? nil
        else { return nil }
        guard let target = Self.target(marks, from: -displayOffset, forward: forward) else {
            // Past the last prompt, forward returns to the live screen.
            return forward ? 0 : nil
        }
        return max(0, -target)
    }

    /// A command's output: the one whose prompt is at `promptLine`, or the newest.
    static func output(in terminal: Terminal, promptLine: Int?) -> RemoteCommandOutput? {
        guard let bounds = try? terminal.lineBounds(), let marks = (try? terminal.promptMarks(in: bounds)) ?? nil,
            let range = output(marks, prompt: promptLine, lastLine: bounds.upperBound)
        else { return nil }
        let capped = max(range.lowerBound, range.upperBound - maxOutputLines + 1)...range.upperBound
        guard let text = try? terminal.text(of: capped) else { return nil }
        var output = RemoteCommandOutput.bounded(command: nil, text: trimmed(text), exitCode: nil)
        if capped != range { output.truncated = true }
        return output
    }
}
