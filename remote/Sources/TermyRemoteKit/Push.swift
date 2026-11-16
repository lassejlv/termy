import CryptoKit
import Foundation

/// A notification a Mac sends a paired phone through the relay, sealed so
/// that only that phone can read it.
public struct PushPayload: Codable, Hashable, Sendable {
    public var title: String
    public var body: String
    /// The pane it's about, which tapping the notification opens.
    public var pane: String?
    public var sentAt: Date

    public init(title: String, body: String, pane: String?, sentAt: Date = Date()) {
        self.title = String(title.prefix(Self.maxTitle))
        self.body = String(body.prefix(Self.maxBody))
        self.pane = pane.map { String($0.prefix(128)) }
        self.sentAt = sentAt
    }

    static let maxTitle = 80
    static let maxBody = 180
}

/// A command that ran long and just finished, found by comparing two session lists.
public struct FinishedCommand: Hashable, Sendable {
    public var pane: RemotePane
    /// The pane's title while the command ran, which usually names it.
    public var title: String?
    public var state: RemoteCommandState
    public var duration: TimeInterval

    /// Commands that finished between `old` and `new` after running at least `minimum` seconds.
    public static func between(
        _ old: [RemotePane], _ new: [RemotePane], now: Date = Date(), atLeast minimum: TimeInterval
    ) -> [FinishedCommand] {
        let before = Dictionary(old.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        return new.compactMap { pane in
            guard let state = pane.command, !state.running, let was = before[pane.id], let running = was.command,
                running.running, state.finished > running.finished, let since = running.runningSince
            else { return nil }
            let duration = now.timeIntervalSince(since)
            guard duration >= minimum else { return nil }
            let title = [was.title, pane.title].compactMap { $0 }.first { !$0.isEmpty }
            return FinishedCommand(pane: pane, title: title, state: state, duration: duration)
        }
    }

    /// "cargo build finished" / "cargo build failed", and "Done in 3m 12s on Studio" / "Exit 2 after 12s on Studio".
    public func notification(on computer: String) -> (title: String, body: String) {
        let name = title ?? "A command"
        let time = DurationText.short(duration)
        if state.failed {
            return ("\(name) failed", "Exit \(state.exitCode ?? 0) after \(time) on \(computer)")
        }
        return ("\(name) finished", "Done in \(time) on \(computer)")
    }
}

public enum DurationText {
    /// "45s", "3m 12s", "1h 4m".
    public static func short(_ seconds: TimeInterval) -> String {
        let total = max(0, Int(seconds.rounded()))
        if total < 60 { return "\(total)s" }
        if total < 3600 { return "\(total / 60)m \(total % 60)s" }
        return "\(total / 3600)h \((total % 3600) / 60)m"
    }
}

public enum PushEnvironment: String, Codable, Sendable {
    case sandbox, production
}

/// Seals notifications end to end. The phone and the Mac it paired with
/// each hold their own long-term key and the other's pinned public key, so
/// both derive the same key (X25519, then HKDF) without another exchange.
/// The relay only ever handles the sealed bytes.
public enum PushSeal {
    /// The most a sealed payload may be; the relay refuses larger ones.
    public static let maxSealedBytes = 1536
    private static let version: UInt8 = 1

    public static func key(identity: DeviceIdentity, peerPublicKey: Data, host: UUID, client: UUID) throws -> SymmetricKey {
        let peer = try Curve25519.KeyAgreement.PublicKey(rawRepresentation: peerPublicKey)
        let shared = try identity.privateKey.sharedSecretFromKeyAgreement(with: peer)
        return shared.hkdfDerivedSymmetricKey(
            using: SHA256.self, salt: Data("termy push v1".utf8),
            sharedInfo: Data("\(host.uuidString)|\(client.uuidString)".utf8), outputByteCount: 32)
    }

    /// The payload as JSON, padded to a multiple of 64 bytes so lengths say little, then sealed.
    public static func seal(_ payload: PushPayload, key: SymmetricKey) throws -> Data {
        let json = try JSONEncoder.push.encode(payload)
        let padded = (json.count + 2 + 63) / 64 * 64
        var body = Data([UInt8(json.count >> 8), UInt8(json.count & 0xff)]) + json
        body += Data(count: padded - body.count)
        let sealed = Data([version]) + (try ChaChaPoly.seal(body, using: key).combined)
        guard sealed.count <= maxSealedBytes else { throw ChannelError.malformed }
        return sealed
    }

    public static func open(_ sealed: Data, key: SymmetricKey) throws -> PushPayload {
        guard sealed.count <= maxSealedBytes, sealed.first == version else { throw ChannelError.malformed }
        let body = try ChaChaPoly.open(ChaChaPoly.SealedBox(combined: sealed.dropFirst()), using: key)
        guard body.count >= 2 else { throw ChannelError.malformed }
        let length = Int(body[body.startIndex]) << 8 | Int(body[body.startIndex + 1])
        guard length <= body.count - 2 else { throw ChannelError.malformed }
        return try JSONDecoder.push.decode(PushPayload.self, from: body.dropFirst(2).prefix(length))
    }
}

extension RelayAPI {
    /// Lets Macs paired with this phone wake it with notifications.
    public func setPushToken(_ token: Data, environment: PushEnvironment) async throws {
        struct Registration: Encodable {
            var token: String
            var environment: PushEnvironment
        }
        let hex = token.map { String(format: "%02x", $0) }.joined()
        try await send("PUT", "v2/device/push", body: Registration(token: hex, environment: environment))
    }

    public func removePushToken() async throws {
        try await send("DELETE", "v2/device/push", body: Optional<String>.none)
    }

    /// Sends a sealed notification to a paired phone. False when it has notifications off.
    public func push(to device: UUID, sealed: Data, collapseID: String? = nil) async throws -> Bool {
        struct Push: Encodable {
            var sealed: Data
            var collapseId: String?
        }
        struct Sent: Decodable { var sent: Bool }
        let reply: Sent = try await exchange("POST", "v2/push/\(device)", body: Push(sealed: sealed, collapseId: collapseID))
        return reply.sent
    }
}

extension JSONEncoder {
    fileprivate static let push: JSONEncoder = {
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .secondsSince1970
        return encoder
    }()
}

extension JSONDecoder {
    fileprivate static let push: JSONDecoder = {
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .secondsSince1970
        return decoder
    }()
}
