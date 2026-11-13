import Foundation

/// The line protocol between Termy Next and its zsh plugin, over a Unix
/// socket. One message per line. Fields are separated by US (0x1F), list
/// items by RS (0x1E), and a field escapes its backslashes and newlines as
/// `\\` and `\n`, which zsh undoes with `${(g::)…}`.
///
/// From zsh:
///
///     hello US <pane> US <pid> US <PATH>
///     line  US <cursor> US <buffer> US <history RS …> US <commands RS …>
///     done
///
/// To zsh: `set US <text>` replaces the command line, and zsh reports the
/// new line back. To run it, Termy Next types Return after that report.
public enum ShellMessage: Equatable, Sendable {
    /// A shell connected. `pane` is its `TERMY_NEXT_PANE` or `TERMY_MUX_PANE`.
    case hello(pane: String, pid: Int32, path: String)
    /// The command line changed while zsh edits it.
    case line(ShellLine)
    /// The line was run or abandoned: zsh is no longer editing it.
    case done
}

/// The command line zsh is editing, with what it suggests for it.
public struct ShellLine: Equatable, Sendable {
    public var buffer: String
    /// Cursor position in characters.
    public var cursor: Int
    /// History entries starting with the buffer, newest first, unique.
    public var history: [String]
    /// Commands, aliases, functions and builtins starting with the buffer, sorted.
    public var commands: [String]

    public init(buffer: String, cursor: Int, history: [String] = [], commands: [String] = []) {
        self.buffer = buffer
        self.cursor = cursor
        self.history = history
        self.commands = commands
    }

    /// The popup's two sections: history, then commands, without the line
    /// itself. Empty while the cursor isn't at the end or the line is blank.
    public var suggestions: (history: [String], commands: [String]) {
        guard cursor >= buffer.count, buffer.contains(where: { !$0.isWhitespace }) else { return ([], []) }
        return (history.filter { $0 != buffer }, commands.filter { $0 != buffer })
    }
}

public enum ShellProtocol {
    static let fieldSeparator: Character = "\u{1F}"
    static let itemSeparator: Character = "\u{1E}"

    public static func parse(_ message: String) -> ShellMessage? {
        let fields = message.split(separator: fieldSeparator, omittingEmptySubsequences: false).map(String.init)
        switch fields.first {
        case "hello" where fields.count >= 4:
            return .hello(pane: unescape(fields[1]), pid: Int32(fields[2]) ?? 0, path: unescape(fields[3]))
        case "line" where fields.count >= 5:
            let buffer = unescape(fields[2])
            return .line(ShellLine(buffer: buffer, cursor: characterOffset(ofScalarOffset: Int(fields[1]) ?? 0, in: buffer),
                                   history: list(fields[3]), commands: list(fields[4])))
        case "done":
            return .done
        default:
            return nil
        }
    }

    /// A message to zsh, such as `set`, newline included.
    public static func encode(_ verb: String, _ text: String) -> String {
        "\(verb)\(fieldSeparator)\(escape(text))\n"
    }

    public static func escape(_ text: String) -> String {
        var out = ""
        for character in text {
            switch character {
            case "\\": out += "\\\\"
            case "\n": out += "\\n"
            case fieldSeparator, itemSeparator: continue
            default: out.append(character)
            }
        }
        return out
    }

    public static func unescape(_ text: String) -> String {
        var out = ""
        var escaping = false
        for character in text {
            if escaping {
                switch character {
                case "n": out.append("\n")
                case "\\": out.append("\\")
                default: out.append("\\"); out.append(character)
                }
                escaping = false
            } else if character == "\\" {
                escaping = true
            } else {
                out.append(character)
            }
        }
        if escaping { out.append("\\") }
        return out
    }

    /// zsh's `$CURSOR` counts code points; Swift strings count characters
    /// (an accented letter or a flag can be several code points). The number
    /// of whole characters before the cursor, so a cursor inside one isn't
    /// taken for the end of the line.
    public static func characterOffset(ofScalarOffset offset: Int, in text: String) -> Int {
        guard offset > 0 else { return 0 }
        var scalars = 0
        var characters = 0
        for character in text {
            scalars += character.unicodeScalars.count
            guard scalars <= offset else { break }
            characters += 1
        }
        return characters
    }

    private static func list(_ field: String) -> [String] {
        field.split(separator: itemSeparator).map { unescape(String($0)) }.filter { !$0.isEmpty }
    }

    /// The command in an agent's reply: the first fenced block if there is
    /// one, else the last paragraph (models sometimes think out loud first),
    /// without a leading `$ ` or wrapping backticks. Nil when nothing is left.
    public static func command(fromAgentReply reply: String) -> String? {
        let lines = reply.replacingOccurrences(of: "\r\n", with: "\n").components(separatedBy: "\n")
        var text: String
        if let fence = lines.firstIndex(where: { $0.trimmingCharacters(in: .whitespaces).hasPrefix("```") }) {
            let rest = lines[(fence + 1)...]
            let close = rest.firstIndex { $0.trimmingCharacters(in: .whitespaces).hasPrefix("```") } ?? rest.endIndex
            text = rest[..<close].joined(separator: "\n")
        } else {
            let paragraphs = lines.split { $0.trimmingCharacters(in: .whitespaces).isEmpty }
            text = paragraphs.last?.joined(separator: "\n") ?? ""
        }
        text = text.trimmingCharacters(in: .whitespacesAndNewlines)
        if text.hasPrefix("$ ") { text.removeFirst(2) }
        if text.count > 2, text.hasPrefix("`"), text.hasSuffix("`"), !text.dropFirst().dropLast().contains("`") {
            text = String(text.dropFirst().dropLast())
        }
        return text.isEmpty ? nil : text
    }
}
