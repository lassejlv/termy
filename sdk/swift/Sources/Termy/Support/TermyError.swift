import Foundation
internal import CTermy

/// A libtermy call returned a non-OK status.
public struct TermyError: LocalizedError, Equatable, Sendable, CustomStringConvertible {
    public enum Code: Equatable, Sendable {
        case nullPointer
        case invalidUTF8
        case spawnFailed
        case configLoadFailed
        case unknownKey
        case writeFailed
        case serializeFailed
        /// libtermy caught a Rust panic before it crossed the C ABI.
        case panicked
        case invalidArgument
        /// No multiplexer host runs there, or it cannot be reached or started.
        case hostUnavailable
        /// A multiplexer host of another Termy version runs there.
        case versionMismatch
        /// The multiplexer pane is gone or its program exited.
        case notFound
        case unknown(UInt32)
    }

    public let code: Code
    /// The C function that failed, for diagnostics.
    public let operation: String

    public init(code: Code, operation: String) {
        self.code = code
        self.operation = operation
    }

    public var description: String { "\(operation) failed: \(code)" }

    /// A sentence for people, such as in an alert.
    public var errorDescription: String? {
        switch code {
        case .hostUnavailable: "The session host isn't running and couldn't be started (\(operation))."
        case .versionMismatch: "A session host from another Termy version is running (\(operation))."
        case .notFound: "That pane is gone (\(operation))."
        default: description
        }
    }

    init(status: TermyFfiStatus, operation: String) {
        let code: Code = switch status {
        case TERMY_FFI_NULL: .nullPointer
        case TERMY_FFI_INVALID_UTF8: .invalidUTF8
        case TERMY_FFI_SPAWN_FAILED: .spawnFailed
        case TERMY_FFI_CONFIG_LOAD_FAILED: .configLoadFailed
        case TERMY_FFI_UNKNOWN_KEY: .unknownKey
        case TERMY_FFI_WRITE_FAILED: .writeFailed
        case TERMY_FFI_SERIALIZE_FAILED: .serializeFailed
        case TERMY_FFI_PANICKED: .panicked
        case TERMY_FFI_INVALID_ARGUMENT: .invalidArgument
        case TERMY_FFI_HOST_UNAVAILABLE: .hostUnavailable
        case TERMY_FFI_VERSION_MISMATCH: .versionMismatch
        case TERMY_FFI_NOT_FOUND: .notFound
        default: .unknown(status.rawValue)
        }
        self.init(code: code, operation: operation)
    }
}

/// Throws unless `status` is `TERMY_FFI_OK`.
@inline(__always)
func check(_ status: TermyFfiStatus, _ operation: @autoclosure () -> String) throws(TermyError) {
    guard status == TERMY_FFI_OK else {
        throw TermyError(status: status, operation: operation())
    }
}
