internal import CTermy
import Foundation

/// The modes programs in a terminal turned on.
public struct TerminalModes: Decodable, Hashable, Sendable {
    public struct Mouse: Decodable, Hashable, Sendable {
        public var enabled: Bool
        public var reportClick: Bool
        public var reportDrag: Bool
        public var reportMotion: Bool
        public var sgrEncoding: Bool
        public var utf8Encoding: Bool

        enum CodingKeys: String, CodingKey {
            case enabled
            case reportClick = "report_click"
            case reportDrag = "report_drag"
            case reportMotion = "report_motion"
            case sgrEncoding = "sgr_encoding"
            case utf8Encoding = "utf8_encoding"
        }
    }

    public struct Keyboard: Decodable, Hashable, Sendable {
        public var applicationKeypad: Bool
        public var disambiguateEscapeCodes: Bool
        public var reportEventTypes: Bool
        public var reportAlternateKeys: Bool
        public var reportAllKeysAsEscapeCodes: Bool
        public var reportAssociatedText: Bool

        enum CodingKeys: String, CodingKey {
            case applicationKeypad = "application_keypad"
            case disambiguateEscapeCodes = "disambiguate_escape_codes"
            case reportEventTypes = "report_event_types"
            case reportAlternateKeys = "report_alternate_keys"
            case reportAllKeysAsEscapeCodes = "report_all_keys_as_esc"
            case reportAssociatedText = "report_associated_text"
        }
    }

    /// A full-screen program, such as an editor, is showing.
    public var alternateScreen: Bool
    public var bracketedPaste: Bool
    public var mouse: Mouse
    public var keyboard: Keyboard

    enum CodingKeys: String, CodingKey {
        case alternateScreen = "alternate_screen"
        case bracketedPaste = "bracketed_paste"
        case mouse, keyboard
    }
}

/// How `Terminal.text(_:maxLines:unwrap:)` writes the screen.
public enum TerminalTextFormat: UInt32, Sendable {
    case plain = 0
    /// With SGR escape sequences for colors and styles.
    case vt = 1
    /// An HTML `<pre>` with inline styles.
    case html = 2
}

extension Terminal {
    /// The modes programs turned on: the alternate screen, mouse reporting,
    /// keyboard protocols and bracketed paste.
    public func modes() throws(TermyError) -> TerminalModes {
        let data = try takeBytes("termy_terminal_modes_json") { termy_terminal_modes_json(handle, $0) }
        do {
            return try JSONDecoder().decode(TerminalModes.self, from: data)
        } catch {
            throw TermyError(code: .serializeFailed, operation: "termy_terminal_modes_json")
        }
    }

    /// The screen and scrollback as text, the last `maxLines` lines (all when
    /// nil), oldest first. With `unwrap`, lines the terminal wrapped join.
    public func text(_ format: TerminalTextFormat = .plain, maxLines: Int? = nil, unwrap: Bool = false) throws(TermyError) -> String {
        let flags = format.rawValue | (unwrap ? UInt32(TERMY_TEXT_UNWRAP) : 0)
        let limit = UInt32(clamping: max(maxLines ?? 0, 0))
        return try takeString("termy_terminal_text") { termy_terminal_text(handle, flags, limit, $0) }
    }
}
