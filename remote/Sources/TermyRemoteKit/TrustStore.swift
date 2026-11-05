import Foundation

/// Public keys this device has pinned, by device ID. The relay lists every
/// device's key, so a relay that lied could put its own key there; pinning
/// the first key seen per device (and refusing a changed one) means a
/// compromised relay can't silently step into a session that already worked.
public struct TrustStore: Sendable {
    public struct Entry: Codable, Hashable, Sendable {
        public var name: String
        public var publicKey: String
        public var addedAt: Date
    }

    public enum Verdict: Equatable, Sendable {
        case trusted
        case unknown
        /// The device presents a different key than the one pinned.
        case changed
    }

    public let url: URL
    public private(set) var entries: [String: Entry]

    public init(url: URL) {
        self.url = url
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        entries = (try? Data(contentsOf: url)).flatMap { try? decoder.decode([String: Entry].self, from: $0) } ?? [:]
    }

    public func verdict(deviceID: UUID, publicKey: Data) -> Verdict {
        guard let entry = entries[deviceID.uuidString] else { return .unknown }
        return entry.publicKey == publicKey.base64EncodedString() ? .trusted : .changed
    }

    /// Whether any device is pinned with this key (the host knows phones by key).
    public func isTrusted(publicKey: Data) -> Bool {
        entries.values.contains { $0.publicKey == publicKey.base64EncodedString() }
    }

    public mutating func trust(deviceID: UUID, name: String, publicKey: Data) throws {
        entries[deviceID.uuidString] = Entry(name: name, publicKey: publicKey.base64EncodedString(), addedAt: Date())
        try save()
    }

    public mutating func remove(deviceID: UUID) throws {
        entries[deviceID.uuidString] = nil
        try save()
    }

    private func save() throws {
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try encoder.encode(entries).write(to: url, options: [.atomic, .completeFileProtection])
        try? FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: url.path)
    }
}
