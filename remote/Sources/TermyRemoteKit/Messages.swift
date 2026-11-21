import Compression
import Foundation

/// What travels inside a `SecureChannel`. The network in between never sees any of it.

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
    /// Screen rows where a shell prompt starts (OSC 133;A), for protocol 5
    /// phones. Nil when the pane has no shell integration.
    public var promptRows: [Int]? = nil

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
    /// The largest image the host sends, encoded. A packet is at most
    /// 4 MiB (`DirectWire.maxPacketBytes`), and JSON takes a third more than the PNG.
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

/// A pane's shell commands, from OSC 133 shell integration (protocol 5).
///
/// Only panes whose shell reported a lifecycle event have one. Older phones
/// ignore it, and older hosts never send it.
public struct RemoteCommandState: Codable, Hashable, Sendable {
    /// A command was submitted and hasn't finished.
    public var running: Bool
    /// Commands that finished so far.
    public var finished: Int
    /// What the last finished command exited with.
    public var exitCode: Int?
    /// When the host first saw the running command, to within its poll.
    public var runningSince: Date?

    public init(running: Bool, finished: Int, exitCode: Int?, runningSince: Date? = nil) {
        self.running = running
        self.finished = finished
        self.exitCode = exitCode
        self.runningSince = runningSince
    }

    /// The last command failed and nothing runs now.
    public var failed: Bool { !running && finished > 0 && (exitCode ?? 0) != 0 }
}

/// One step of a shell command, as an attached pane reports it (protocol 5).
public enum RemoteShellEvent: Codable, Hashable, Sendable {
    /// The shell shows its prompt (OSC 133;A).
    case prompt
    /// The user is typing a command (OSC 133;B).
    case input
    /// The command was submitted and runs (OSC 133;C).
    case executing
    /// The command finished (OSC 133;D).
    case finished(exitCode: Int?)
}

/// A pane as the phone lists it.
public struct RemotePane: Codable, Hashable, Sendable, Identifiable {
    public var id: String
    public var title: String?
    public var workingDirectory: String?
    public var exited: Bool
    public var command: RemoteCommandState? = nil

    public init(
        id: String, title: String?, workingDirectory: String?, exited: Bool, command: RemoteCommandState? = nil
    ) {
        self.id = id
        self.title = title
        self.workingDirectory = workingDirectory
        self.exited = exited
        self.command = command
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
    /// Scrolls to the previous or next shell prompt (protocol 5).
    case jumpToPrompt(stream: Int, forward: Bool)
    /// Asks for a command's output: the one whose prompt starts at
    /// `promptBufferRow`, or the last finished one when nil (protocol 5).
    case commandOutput(stream: Int, promptBufferRow: Int?)
    /// A file from the phone, such as a photo, saved on the Mac; its path is
    /// pasted into the pane (protocol 5). See `RemoteUpload`.
    case upload(stream: Int, name: String, data: Data)

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
    /// A shell lifecycle step on an attached pane. Sent to phones of version 5 or later.
    case shell(stream: Int, event: RemoteShellEvent)
    /// The text a command printed, answering `ClientMessage.commandOutput`.
    /// Nil when there is no such command; `truncated` when it was cut to
    /// `RemoteCommandOutput.maxBytes` (its end is kept).
    case commandOutput(stream: Int, output: RemoteCommandOutput?)
    /// An upload was saved at `path` and the path pasted into the pane.
    case uploaded(stream: Int, path: String)
    /// Where this Mac can be reached, empty when nowhere. Sent to
    /// phones of version 6 or later, after their hello and whenever it changes.
    case endpoints([DirectEndpoint])

}

public enum RemoteProtocol {
    /// 3 adds Kitty graphics: `HostMessage.image` and `.images`.
    /// 4: the phone keeps glyphs by `CellRun.glyphKey` (see `GlyphCache`),
    /// and takes grids up to `RemoteFrame.maxCols` by `maxRows`.
    /// 5: shell integration: `HostMessage.shell`, `RemotePane.command`,
    /// `RemoteFrame.promptRows`, prompt jumps, command output and uploads.
    /// 6: the direct path: `HostMessage.endpoints` (see `DirectEndpoint`).
    public static let version = 6
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

    /// The most a deflated message may inflate to. A packet is at most
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
        frame.promptRows = promptRows?.filter { $0 < frame.rows }
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

/// A command's output, as the host read it from the pane's history.
public struct RemoteCommandOutput: Codable, Hashable, Sendable {
    /// What fits a packet with room to spare after JSON escaping.
    public static let maxBytes = 1024 * 1024

    public var command: String?
    public var text: String
    public var exitCode: Int?
    public var truncated: Bool

    public init(command: String?, text: String, exitCode: Int?, truncated: Bool) {
        self.command = command
        self.text = text
        self.exitCode = exitCode
        self.truncated = truncated
    }

    /// Keeps the last `maxBytes` of `text`, on a character boundary.
    public static func bounded(command: String?, text: String, exitCode: Int?) -> RemoteCommandOutput {
        guard text.utf8.count > maxBytes else {
            return RemoteCommandOutput(command: command, text: text, exitCode: exitCode, truncated: false)
        }
        var tail = Substring(text)
        while tail.utf8.count > maxBytes { tail = tail.dropFirst(max(1, (tail.utf8.count - maxBytes) / 4)) }
        return RemoteCommandOutput(command: command, text: String(tail), exitCode: exitCode, truncated: true)
    }
}

/// Files a phone sends into a pane.
public enum RemoteUpload {
    /// The largest file: base64 in JSON grows it by a third, and a packet
    /// is at most 4 MiB.
    public static let maxBytes = 2_500_000

    /// A safe file name: letters, digits, dots, dashes and underscores, at
    /// most 80 characters, never hidden, never empty.
    public static func fileName(_ name: String, fallbackExtension: String = "bin") -> String {
        let allowed = Set("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._-")
        var cleaned = String(name.map { allowed.contains($0) ? $0 : "-" }.prefix(80))
        while cleaned.hasPrefix(".") || cleaned.hasPrefix("-") { cleaned.removeFirst() }
        if cleaned.isEmpty || !cleaned.contains(".") { cleaned = (cleaned.isEmpty ? "upload" : cleaned) + "." + fallbackExtension }
        return cleaned
    }

    /// The extension the bytes call for, from their signature.
    public static func sniffedExtension(_ data: Data) -> String? {
        let bytes = [UInt8](data.prefix(12))
        if bytes.starts(with: [0x89, 0x50, 0x4e, 0x47]) { return "png" }
        if bytes.starts(with: [0xff, 0xd8, 0xff]) { return "jpg" }
        if bytes.starts(with: Array("GIF8".utf8)) { return "gif" }
        if bytes.count >= 12, bytes[0..<4] == [0x52, 0x49, 0x46, 0x46], bytes[8..<12] == [0x57, 0x45, 0x42, 0x50] {
            return "webp"
        }
        if bytes.starts(with: Array("%PDF".utf8)) { return "pdf" }
        return nil
    }

    /// A path as one shell word, quoted only when it needs to be.
    public static func shellQuoted(_ path: String) -> String {
        let plain = Set("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._-/+@%:,")
        if !path.isEmpty, path.allSatisfy(plain.contains) { return path }
        return "'" + path.replacingOccurrences(of: "'", with: "'\\''") + "'"
    }
}

/// Handoff between the iPhone app and Termy Next: the pane on screen, and
/// the computer it runs on (the host ID it paired under, which both apps know).
public enum RemoteHandoff {
    public static let activityType = "sh.termy.pane"

    public static func userInfo(host: UUID, pane: String) -> [String: String] {
        ["host": host.uuidString, "pane": pane]
    }

    public static func parse(_ userInfo: [AnyHashable: Any]?) -> (host: UUID, pane: String)? {
        guard let raw = userInfo?["host"] as? String, let host = UUID(uuidString: raw),
            let pane = userInfo?["pane"] as? String, !pane.isEmpty, pane.count <= 128
        else { return nil }
        return (host, pane)
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
            && (promptRows ?? []).count <= rows && (promptRows ?? []).allSatisfy { (0..<rows).contains($0) }
            && updates.allSatisfy { update in
                (0..<rows).contains(update.row) && update.runs.count <= cols
                    && update.runs.allSatisfy {
                        $0.col >= 0 && $0.cells > 0 && $0.col < cols && $0.cells <= cols - $0.col
                    }
            }
    }
}
