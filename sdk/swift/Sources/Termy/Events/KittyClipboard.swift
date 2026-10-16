import Foundation
internal import CTermy

/// Where a Kitty OSC 5522 clipboard request points.
public enum ClipboardLocation: Hashable, Sendable {
    case clipboard
    case primarySelection

    init(rawValue: UInt32) {
        self = rawValue == TERMY_FFI_CLIPBOARD_LOCATION_PRIMARY.rawValue ? .primarySelection : .clipboard
    }

    var rawValue: UInt32 {
        switch self {
        case .clipboard: TERMY_FFI_CLIPBOARD_LOCATION_CLIPBOARD.rawValue
        case .primarySelection: TERMY_FFI_CLIPBOARD_LOCATION_PRIMARY.rawValue
        }
    }
}

/// The outcome of a clipboard request. Distinct from `TermyError`.
public enum ClipboardResult: Hashable, Sendable {
    case success, denied, unsupported, busy, invalidData, ioError

    var rawValue: UInt32 {
        switch self {
        case .success: TERMY_FFI_CLIPBOARD_RESULT_SUCCESS.rawValue
        case .denied: TERMY_FFI_CLIPBOARD_RESULT_DENIED.rawValue
        case .unsupported: TERMY_FFI_CLIPBOARD_RESULT_UNSUPPORTED.rawValue
        case .busy: TERMY_FFI_CLIPBOARD_RESULT_BUSY.rawValue
        case .invalidData: TERMY_FFI_CLIPBOARD_RESULT_INVALID_DATA.rawValue
        case .ioError: TERMY_FFI_CLIPBOARD_RESULT_IO_ERROR.rawValue
        }
    }
}

public struct ClipboardContent: Hashable, Sendable {
    public var mimeType: String
    public var data: Data

    public init(mimeType: String, data: Data) {
        self.mimeType = mimeType
        self.data = data
    }
}

public struct ClipboardReadRequest: Hashable, Sendable {
    public var location: ClipboardLocation
    public var mimeTypes: [String]
    /// The program only wants the list of available formats.
    public var listAvailable: Bool
    /// The program's self-reported name, when given.
    public var name: String?
    public var permissionGranted: Bool
    public var canRememberPermission: Bool

    init(_ request: TermyFfiClipboardReadRequest) {
        location = ClipboardLocation(rawValue: request.location)
        mimeTypes = UnsafeBufferPointer(start: request.mime_types_ptr, count: request.mime_types_len)
            .map { $0.copyString() }
        listAvailable = request.list_available
        name = request.has_name ? request.name.copyString() : nil
        permissionGranted = request.permission_granted
        canRememberPermission = request.can_remember_permission
    }
}

public struct ClipboardReadResponse: Hashable, Sendable {
    public var result: ClipboardResult
    public var availableFormats: [String]
    public var contents: [ClipboardContent]
    public var rememberPermission: Bool

    public init(
        result: ClipboardResult, availableFormats: [String] = [],
        contents: [ClipboardContent] = [], rememberPermission: Bool = false
    ) {
        self.result = result
        self.availableFormats = availableFormats
        self.contents = contents
        self.rememberPermission = rememberPermission
    }

    public static let denied = ClipboardReadResponse(result: .denied)
}

public struct ClipboardWriteRequest: Hashable, Sendable {
    public var location: ClipboardLocation
    public var contents: [ClipboardContent]
    public var name: String?
    public var permissionGranted: Bool
    public var canRememberPermission: Bool

    init(_ request: TermyFfiClipboardWriteRequest) {
        location = ClipboardLocation(rawValue: request.location)
        contents = UnsafeBufferPointer(start: request.contents_ptr, count: request.contents_len)
            .map { ClipboardContent(mimeType: $0.mime_type.copyString(), data: $0.data.copyData()) }
        name = request.has_name ? request.name.copyString() : nil
        permissionGranted = request.permission_granted
        canRememberPermission = request.can_remember_permission
    }
}

public struct ClipboardWriteResponse: Hashable, Sendable {
    public var result: ClipboardResult
    public var rememberPermission: Bool

    public init(result: ClipboardResult, rememberPermission: Bool = false) {
        self.result = result
        self.rememberPermission = rememberPermission
    }
}

/// Services Kitty OSC 5522 clipboard requests while events drain.
///
/// Calls are synchronous and happen inside `Terminal.drainEvents`. Enforce your
/// own permission policy, and never call back into the same terminal: answer
/// from state you already have, and don't run a modal or nested run loop to
/// ask the user (deny, and ask asynchronously instead). Draining or pasting
/// from a callback throws.
public protocol KittyClipboardHandler: AnyObject {
    func read(_ request: ClipboardReadRequest) -> ClipboardReadResponse
    func write(_ request: ClipboardWriteRequest) -> ClipboardWriteResponse
}

/// Callback context for `termy_terminal_drain_events_with_clipboard`.
final class DrainContext {
    let clipboard: KittyClipboardHandler?
    let onProtocolReply: ((Data) -> Void)?

    init(clipboard: KittyClipboardHandler?, onProtocolReply: ((Data) -> Void)?) {
        self.clipboard = clipboard
        self.onProtocolReply = onProtocolReply
    }

    static func from(_ userData: UnsafeMutableRawPointer?) -> DrainContext? {
        guard let userData else { return nil }
        return Unmanaged<DrainContext>.fromOpaque(userData).takeUnretainedValue()
    }

    static let readCallback: TermyFfiClipboardReadCallback = { userData, request, replyUserData, reply in
        guard let request, let reply else { return }
        let response = from(userData)?.clipboard?.read(ClipboardReadRequest(request.pointee))
            ?? .denied
        let arena = ByteArena()
        let formats = response.availableFormats.map { arena.slice($0) }
        let contents = response.contents.map {
            TermyFfiClipboardContent(mime_type: arena.slice($0.mimeType), data: arena.slice($0.data))
        }
        var ffi = TermyFfiClipboardReadResponse(
            status: response.result.rawValue,
            available_formats_ptr: arena.array(formats),
            available_formats_len: formats.count,
            contents_ptr: arena.array(contents),
            contents_len: contents.count,
            remember_permission: response.rememberPermission
        )
        // libtermy copies the response during this call; the arena may then go.
        withExtendedLifetime(arena) { reply(replyUserData, &ffi) }
    }

    static let writeCallback: TermyFfiClipboardWriteCallback = { userData, request in
        guard let request, let handler = from(userData)?.clipboard else {
            return TermyFfiClipboardWriteResponse(
                status: ClipboardResult.unsupported.rawValue, remember_permission: false)
        }
        let response = handler.write(ClipboardWriteRequest(request.pointee))
        return TermyFfiClipboardWriteResponse(
            status: response.result.rawValue, remember_permission: response.rememberPermission)
    }

    static let protocolReplyCallback: TermyFfiProtocolReplyCallback = { userData, reply in
        from(userData)?.onProtocolReply?(reply.copyData())
    }
}
