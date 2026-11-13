import Compression
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
    /// Names `glyphPNG` by its content, for phones that keep glyphs
    /// (protocol 4): a run whose glyph the phone was sent before carries the
    /// key without the PNG. Older phones never get one.
    public var glyphKey: String? = nil
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
        case glyphKey = "k"
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
        /// Optional, so frames from hosts that never sent it still decode.
        case wrapped = "w"
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

/// One Kitty graphics placement on a pane's screen.
///
/// Rectangles are `[x, y, width, height]`. `frame` and `clip` are in cells
/// from the grid's top-left corner, so they fit whatever cell size the phone
/// draws; `crop` is in fractions of the image, so it fits a downscaled copy.
public struct RemoteImagePlacement: Codable, Hashable, Sendable {
    /// The image to draw, as an earlier `HostMessage.image` named it.
    public var key: String
    /// Where the cropped image is drawn.
    public var frame: [Double]
    /// The part of the grid the placement may paint.
    public var clip: [Double]
    /// The part of the image to draw.
    public var crop: [Double]
    /// Negative values draw below text.
    public var z: Int

    public init(key: String, frame: [Double], clip: [Double], crop: [Double], z: Int) {
        self.key = key
        self.frame = frame
        self.clip = clip
        self.crop = crop
        self.z = z
    }

    enum CodingKeys: String, CodingKey {
        case key = "k"
        case frame = "f"
        case clip = "c"
        case crop = "s"
        case z
    }

    public var isValid: Bool {
        guard key.count <= 64 else { return false }
        return Self.isValidRect(frame) && Self.isValidRect(clip) && Self.isValidRect(crop)
    }

    private static func isValidRect(_ rect: [Double]) -> Bool {
        guard rect.count == 4 else { return false }
        for value in rect where !value.isFinite || abs(value) > 100_000 {
            return false
        }
        return rect[2] >= 0 && rect[3] >= 0
    }
}

/// Every image on a stream's screen. It replaces the list the phone had.
public struct RemoteImages: Codable, Hashable, Sendable {
    public static let maxPlacements = 4096
    /// The largest image the host sends, encoded. The relay carries
    /// messages up to 4 MiB, and JSON takes a third more than the PNG.
    public static let maxImageBytes = 2 * 1024 * 1024

    public var stream: Int
    public var placements: [RemoteImagePlacement]
    /// Images the phone may forget: the host sends them again if needed.
    public var evicted: [String]
    /// True for a stream's first list: forget every image it doesn't place.
    public var full: Bool

    public init(stream: Int, placements: [RemoteImagePlacement], evicted: [String] = [], full: Bool = false) {
        self.stream = stream
        self.placements = placements
        self.evicted = evicted
        self.full = full
    }

    public var isValid: Bool {
        placements.count <= Self.maxPlacements && evicted.count <= Self.maxPlacements
            && placements.allSatisfy(\.isValid)
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
    /// An image's pixels as a PNG, before the first `images` that places it.
    /// Sent to phones that said hello with version 3 or later.
    case image(stream: Int, key: String, png: Data)
    case images(RemoteImages)

}

public enum RemoteProtocol {
    /// 3 adds Kitty graphics: `HostMessage.image` and `.images`.
    /// 4: the phone keeps glyphs by `CellRun.glyphKey` (see `GlyphCache`),
    /// and takes grids up to `RemoteFrame.maxCols` by `maxRows`.
    public static let version = 4
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

    /// The most a deflated message may inflate to. A relay message is at most
    /// 4 MiB; text inflates about tenfold, so a real message stays well
    /// below this, while a message built to inflate without end is refused.
    public static let maxInflatedBytes = 64 * 1024 * 1024

    public static func decode<T: Decodable>(_ type: T.Type, from data: Data) throws -> T {
        guard let flag = data.first else { throw ChannelError.malformed }
        var body = Data(data.dropFirst())
        if flag == 1 {
            body = try inflate(body, limit: maxInflatedBytes)
        } else if flag != 0 {
            throw ChannelError.malformed
        }
        return try JSONDecoder().decode(type, from: body)
    }
}

extension MessageCodec {
    /// Inflates what `NSData.compressed(using: .zlib)` deflated, giving up as
    /// soon as the output passes `limit` bytes.
    static func inflate(_ data: Data, limit: Int) throws -> Data {
        guard !data.isEmpty else { throw ChannelError.malformed }
        let stream = UnsafeMutablePointer<compression_stream>.allocate(capacity: 1)
        defer { stream.deallocate() }
        guard compression_stream_init(stream, COMPRESSION_STREAM_DECODE, COMPRESSION_ZLIB) == COMPRESSION_STATUS_OK
        else { throw ChannelError.malformed }
        defer { compression_stream_destroy(stream) }
        let chunk = 64 * 1024
        let buffer = UnsafeMutablePointer<UInt8>.allocate(capacity: chunk)
        defer { buffer.deallocate() }
        var output = Data()
        var status = COMPRESSION_STATUS_OK
        try data.withUnsafeBytes { (source: UnsafeRawBufferPointer) in
            stream.pointee.src_ptr = source.bindMemory(to: UInt8.self).baseAddress!
            stream.pointee.src_size = source.count
            repeat {
                stream.pointee.dst_ptr = buffer
                stream.pointee.dst_size = chunk
                status = compression_stream_process(stream, Int32(COMPRESSION_STREAM_FINALIZE.rawValue))
                guard status != COMPRESSION_STATUS_ERROR else { throw ChannelError.malformed }
                let produced = chunk - stream.pointee.dst_size
                guard output.count + produced <= limit else { throw ChannelError.malformed }
                output.append(buffer, count: produced)
            } while status == COMPRESSION_STATUS_OK
        }
        guard status == COMPRESSION_STATUS_END else { throw ChannelError.malformed }
        return output
    }
}

extension RemoteFrame {
    /// The frame cut to its top-left `cols` by `rows` cells, for a phone that
    /// drops larger ones: it then shows the part that fits instead of nothing.
    /// Row and column numbers stay those of the pane.
    public func clipped(cols maxCols: Int, rows maxRows: Int) -> RemoteFrame {
        guard cols > maxCols || rows > maxRows else { return self }
        var frame = self
        frame.cols = min(cols, maxCols)
        frame.rows = min(rows, maxRows)
        frame.updates = updates.filter { $0.row < frame.rows }.map { update in
            var update = update
            update.runs = update.runs.compactMap { run in
                guard run.col < frame.cols else { return nil }
                guard run.col + run.cells > frame.cols else { return run }
                // A wide glyph or an image run doesn't split; text does.
                let fits = frame.cols - run.col
                guard run.glyphPNG == nil, run.glyphKey == nil, run.text.unicodeScalars.count == run.cells
                else { return nil }
                var cut = run
                cut.cells = fits
                cut.text = String(String.UnicodeScalarView(run.text.unicodeScalars.prefix(fits)))
                return cut
            }
            return update
        }
        if cursor.row >= frame.rows || cursor.col >= frame.cols { frame.cursor.visible = false }
        frame.cursor.row = min(cursor.row, frame.rows - 1)
        frame.cursor.col = min(cursor.col, frame.cols - 1)
        return frame
    }
}

/// The host's record of the glyphs a phone keeps, kept in step with the
/// phone's `GlyphCache`: every run's key is seen by both in the same order,
/// and the phone keeps twice as many, so it holds every glyph this says it
/// does. A run whose glyph the phone holds goes without its PNG.
public struct GlyphLedger: Sendable {
    public let capacity: Int
    private var tick: UInt64 = 0
    private var held: [String: UInt64] = [:]

    public init(capacity: Int = GlyphCache.defaultCapacity / 2) {
        self.capacity = capacity
    }

    public var count: Int { held.count }

    /// Leaves out the PNGs the phone holds and records the ones sent.
    public mutating func prepare(_ frame: inout RemoteFrame) {
        for update in frame.updates.indices {
            for index in frame.updates[update].runs.indices {
                guard let key = frame.updates[update].runs[index].glyphKey else { continue }
                tick += 1
                if held[key] != nil {
                    held[key] = tick
                    frame.updates[update].runs[index].glyphPNG = nil
                } else if let png = frame.updates[update].runs[index].glyphPNG {
                    // One the phone won't keep goes inline every time.
                    if png.count <= GlyphCache.maxGlyphBytes {
                        held[key] = tick
                    } else {
                        frame.updates[update].runs[index].glyphKey = nil
                    }
                } else {
                    // Nothing to send and the phone hasn't got it.
                    frame.updates[update].runs[index].glyphKey = nil
                }
            }
        }
        if held.count > capacity {
            let keep = held.sorted { $0.value > $1.value }.prefix(capacity)
            held = Dictionary(uniqueKeysWithValues: keep.map { ($0.key, $0.value) })
        }
    }

    /// Forgets everything, so the next frames send each glyph again.
    public mutating func reset() { held.removeAll() }
}

/// The glyphs a phone was sent, by key. It fills in the PNG of each run that
/// names a glyph it holds, so drawing needs nothing else.
public final class GlyphCache {
    public static let defaultCapacity = 2048
    /// Larger PNGs are not kept: a glyph is one or two cells.
    public static let maxGlyphBytes = 256 * 1024

    public let capacity: Int
    private var tick: UInt64 = 0
    private var glyphs: [String: (png: Data, used: UInt64)] = [:]

    public init(capacity: Int = GlyphCache.defaultCapacity) {
        self.capacity = capacity
    }

    public var count: Int { glyphs.count }

    /// Call for every frame from the host, in order, whatever it is for.
    public func resolve(_ frame: inout RemoteFrame) {
        for update in frame.updates.indices {
            for index in frame.updates[update].runs.indices {
                let run = frame.updates[update].runs[index]
                guard let key = run.glyphKey, key.utf8.count <= 64 else { continue }
                tick += 1
                if let png = run.glyphPNG {
                    if png.count <= Self.maxGlyphBytes { glyphs[key] = (png, tick) }
                } else if let held = glyphs[key] {
                    glyphs[key] = (held.png, tick)
                    frame.updates[update].runs[index].glyphPNG = held.png
                }
            }
        }
        if glyphs.count > capacity {
            let keep = glyphs.sorted { $0.value.used > $1.value.used }.prefix(capacity)
            glyphs = Dictionary(uniqueKeysWithValues: keep.map { ($0.key, $0.value) })
        }
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
    /// The largest grid a phone of protocol 4 or later takes.
    public static let maxCols = 1000
    public static let maxRows = 500
    /// What phones before protocol 4 take: they drop larger frames.
    public static let legacyMaxCols = 500
    public static let legacyMaxRows = 300

    /// Reject invalid dimensions/indices before allocating or drawing untrusted frames.
    public var isValid: Bool {
        (1...Self.maxCols).contains(cols) && (1...Self.maxRows).contains(rows) && updates.count <= rows
            && updates.allSatisfy { update in
                (0..<rows).contains(update.row) && update.runs.count <= cols
                    && update.runs.allSatisfy {
                        $0.col >= 0 && $0.cells > 0 && $0.col < cols && $0.cells <= cols - $0.col
                    }
            }
    }
}
