import Foundation

/// What travels inside a `SecureChannel`. The relay never sees any of it.

/// A key the phone sends; the host encodes it for the program's current
/// keyboard mode, so the phone needs no terminal state.
public struct RemoteKey: Codable, Hashable, Sendable {
    /// libtermy logical key name: a character, or `enter`, `tab`, `escape`,
    /// `backspace`, `up`, `down`, `left`, `right`, `home`, `end`, `pageup`,
    /// `pagedown`, `delete`, `f1`…
    public var key: String
    public var text: String?
    public var control: Bool
    public var alt: Bool
    public var shift: Bool

    public init(key: String, text: String? = nil, control: Bool = false, alt: Bool = false, shift: Bool = false) {
        self.key = key
        self.text = text
        self.control = control
        self.alt = alt
        self.shift = shift
    }

    public static func text(_ text: String) -> RemoteKey { RemoteKey(key: text.lowercased(), text: text) }
}

/// One run of cells on a row that share a style.
///
/// When `cells` equals the number of scalars in `text`, each scalar fills one
/// cell; otherwise the run is a single wide character spanning `cells`.
public struct CellRun: Codable, Hashable, Sendable {
    public var col: Int
    public var cells: Int
    public var text: String
    /// 0xRRGGBBAA.
    public var fg: UInt32
    public var bg: UInt32
    public var glyphPNG: Data? = nil
    public var flags: UInt8

    public static let bold: UInt8 = 1
    public static let italic: UInt8 = 2
    public static let underline: UInt8 = 4
    public static let strikethrough: UInt8 = 8
    /// The cell uses the terminal's default background.
    public static let defaultBackground: UInt8 = 16

    public init(col: Int, cells: Int, text: String, fg: UInt32, bg: UInt32, flags: UInt8) {
        self.col = col
        self.cells = cells
        self.text = text
        self.fg = fg
        self.bg = bg
        self.flags = flags
    }

    enum CodingKeys: String, CodingKey {
        case col = "c"
        case cells = "n"
        case text = "s"
        case fg = "f"
        case bg = "b"
        case flags = "a"
        case glyphPNG = "g"
    }
}

public struct RowUpdate: Codable, Hashable, Sendable {
    public var row: Int
    public var runs: [CellRun]
    public var wrapped: Bool? = nil

    public init(row: Int, runs: [CellRun]) {
        self.row = row
        self.runs = runs
    }

    enum CodingKeys: String, CodingKey {
        case row = "r"
        case runs = "u"
    }
}

public struct RemoteCursor: Codable, Hashable, Sendable {
    public var row: Int
    public var col: Int
    public var visible: Bool
    /// "block", "bar" or "underline".
    public var style: String

    public init(row: Int, col: Int, visible: Bool, style: String) {
        self.row = row
        self.col = col
        self.visible = visible
        self.style = style
    }
}

public struct RemoteFrame: Codable, Hashable, Sendable {
    public var stream: Int
    public var cols: Int
    public var rows: Int
    /// True when `updates` covers every row; the phone drops what it had.
    public var full: Bool
    public var updates: [RowUpdate]
    public var cursor: RemoteCursor
    /// 0xRRGGBBAA of the default background, for padding around the grid.
    public var background: UInt32
    public var displayOffset: Int? = nil
    public var historySize: Int? = nil

    public init(
        stream: Int, cols: Int, rows: Int, full: Bool, updates: [RowUpdate], cursor: RemoteCursor, background: UInt32
    ) {
        self.stream = stream
        self.cols = cols
        self.rows = rows
        self.full = full
        self.updates = updates
        self.cursor = cursor
        self.background = background
    }
}

/// A pane as the phone lists it.
public struct RemotePane: Codable, Hashable, Sendable, Identifiable {
    public var id: String
    public var title: String?
    public var workingDirectory: String?
    public var exited: Bool

    public init(id: String, title: String?, workingDirectory: String?, exited: Bool) {
        self.id = id
        self.title = title
        self.workingDirectory = workingDirectory
        self.exited = exited
    }
}

/// A Termy Next session (a host workspace) and its tabs.
public struct RemoteSessionGroup: Codable, Hashable, Sendable, Identifiable {
    public struct Tab: Codable, Hashable, Sendable {
        public var title: String?
        public var activePaneID: String? = nil
        public var paneIDs: [String]

        public init(title: String?, paneIDs: [String]) {
            self.title = title
            self.paneIDs = paneIDs
        }
    }

    public var name: String
    public var windowID: String? = nil
    public var tabs: [Tab]
    public var id: String { (windowID ?? "") + ":" + name }

    public init(name: String, tabs: [Tab]) {
        self.name = name
        self.tabs = tabs
    }
}

public struct RemoteSessionList: Codable, Hashable, Sendable {
    public var groups: [RemoteSessionGroup]
    public var panes: [RemotePane]

    public init(groups: [RemoteSessionGroup], panes: [RemotePane]) {
        self.groups = groups
        self.panes = panes
    }
}

/// Phone → host.
public enum ClientMessage: Codable, Hashable, Sendable {
    case clientHello(version: Int)
    case listSessions
    /// Starts streaming a pane's screen as `stream`.
    case attach(stream: Int, paneID: String)
    case detach(stream: Int)
    case key(stream: Int, key: RemoteKey)
    case paste(stream: Int, text: String)
    /// Resizes the pane to fit the phone; `nil` restores the size it had.
    case resize(stream: Int, cols: Int?, rows: Int?)
    case scroll(stream: Int, lines: Int)
    /// Starts a new pane running the default shell.
    case createPane
    case closePane(paneID: String)
    case createSession(name: String)
    /// A pane anchors a stable session even if its name changes during the request.
    case createTab(inSessionContaining: String)
    case splitPane(paneID: String, horizontal: Bool)
    case renameSession(containing: String, name: String)
    case renameTab(containing: String, title: String)
    case mouse(stream: Int, col: Int, row: Int, action: String)
    case scrollToBottom(stream: Int)
    case search(stream: Int, query: String)
    case revealSearch(stream: Int, bufferRow: Int)

}

/// Host → phone.
public enum HostMessage: Codable, Hashable, Sendable {
    case hello(hostName: String, version: Int)
    case sessions(RemoteSessionList)
    case frame(RemoteFrame)
    case title(stream: Int, title: String)
    case exited(stream: Int)
    case bell(stream: Int)
    case created(paneID: String)
    case error(String)
    case configuration(RemoteConfiguration)
    case searchResults(stream: Int, matches: [RemoteSearchMatch])

}

public enum RemoteProtocol {
    public static let version = 2
}

/// JSON, deflated when that saves space. The first byte says which.
public enum MessageCodec {
    private static let encoder: JSONEncoder = {
        let encoder = JSONEncoder()
        encoder.outputFormatting = []
        return encoder
    }()

    public static func encode<T: Encodable>(_ value: T) throws -> Data {
        let json = try encoder.encode(value)
        if json.count > 512, let compressed = try? (json as NSData).compressed(using: .zlib) as Data,
            compressed.count < json.count
        {
            return Data([1]) + compressed
        }
        return Data([0]) + json
    }

    public static func decode<T: Decodable>(_ type: T.Type, from data: Data) throws -> T {
        guard let flag = data.first else { throw ChannelError.malformed }
        var body = Data(data.dropFirst())
        if flag == 1 {
            body = try (body as NSData).decompressed(using: .zlib) as Data
        } else if flag != 0 {
            throw ChannelError.malformed
        }
        return try JSONDecoder().decode(type, from: body)
    }
}

public struct RemoteSearchMatch: Codable, Hashable, Sendable {
    public var row: Int
    public var startCol: Int
    public var endCol: Int
    public var line: String
    public init(row: Int, startCol: Int, endCol: Int, line: String) {
        self.row = row
        self.startCol = startCol
        self.endCol = endCol
        self.line = line
    }
}

extension RemoteFrame {
    /// Reject invalid dimensions/indices before allocating or drawing untrusted frames.
    public var isValid: Bool {
        (1...500).contains(cols) && (1...300).contains(rows) && updates.count <= rows
            && updates.allSatisfy { update in
                (0..<rows).contains(update.row) && update.runs.count <= cols
                    && update.runs.allSatisfy {
                        $0.col >= 0 && $0.cells > 0 && $0.col < cols && $0.cells <= cols - $0.col
                    }
            }
    }
}
