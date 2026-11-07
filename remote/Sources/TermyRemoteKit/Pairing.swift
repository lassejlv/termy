import CryptoKit
import Foundation

public enum PairingError: Error, LocalizedError {
    case invalidCode, expired, invalidProof, storage
    public var errorDescription: String? {
        switch self {
        case .invalidCode: "This is not a valid Termy pairing code."
        case .expired: "This pairing code expired. Generate a new one on your Mac."
        case .invalidProof: "The pairing identities could not be verified. Generate a new QR code."
        case .storage: "Could not save device credentials securely."
        }
    }
}
/// The secret is carried only by the QR code, never sent to the relay.
/// Its HMAC authenticates the entire phone handshake, Mac identity and invitation.
public struct PairingCode: Codable, Sendable {
    public let version: Int
    public let relayURL: URL
    public let invitationID: UUID
    public let hostID: UUID
    public let hostName: String
    public let hostKey: Data
    public let secret: Data
    public let expiresAt: Date

    public init(status: PairingStatus, relayURL: URL, hostName: String) throws {
        self.version = 3
        self.relayURL = try RelayAddress.validate(relayURL)
        invitationID = status.id
        hostID = status.host.id
        self.hostName = hostName
        guard let key = status.host.publicKeyData, key.count == 32 else {
            throw PairingError.invalidCode
        }
        hostKey = key
        expiresAt = status.expiresAt
        secret = SymmetricKey(size: .bits256).withUnsafeBytes { Data($0) }
    }
    public var text: String {
        get throws {
            let data = try JSONEncoder().encode(self)
            return "termy://pair?data="
                + data.base64EncodedString().replacingOccurrences(of: "+", with: "-").replacingOccurrences(
                    of: "/", with: "_"
                ).replacingOccurrences(of: "=", with: "")
        }
    }
    public static func parse(_ text: String, now: Date = Date()) throws -> Self {
        guard text.utf8.count < 4096, let parts = URLComponents(string: text), parts.scheme == "termy",
            parts.host == "pair",
            let raw = parts.queryItems?.first(where: { $0.name == "data" })?.value
        else { throw PairingError.invalidCode }
        var b64 = raw.replacingOccurrences(of: "-", with: "+").replacingOccurrences(of: "_", with: "/")
        b64 += String(repeating: "=", count: (4 - b64.count % 4) % 4)
        guard let data = Data(base64Encoded: b64),
            let code = try? JSONDecoder().decode(Self.self, from: data),
            code.version == 3, code.hostKey.count == 32, code.secret.count == 32, !code.hostName.isEmpty
        else { throw PairingError.invalidCode }
        _ = try RelayAddress.validate(code.relayURL)
        guard code.expiresAt > now, code.expiresAt.timeIntervalSince(now) <= 180 else { throw PairingError.expired }
        return code
    }
    private func transcript(_ initPacket: Data) -> Data {
        Data(
            "termy-pair-v3|\(invitationID.uuidString.lowercased())|\(hostID.uuidString.lowercased())|\(relayURL.absoluteString)|"
                .utf8) + hostKey + initPacket
    }
    private var nameKey: SymmetricKey {
        HKDF<SHA256>.deriveKey(
            inputKeyMaterial: SymmetricKey(data: secret), salt: hostKey,
            info: Data("termy-pairing-name-v3".utf8), outputByteCount: 32)
    }
    public func sealClientName(_ name: String, initPacket: Data) throws -> Data {
        try PrivateMetadata.seal(name, key: nameKey, context: transcript(initPacket))
    }
    public func proof(for initPacket: Data, encryptedClientName: Data) -> Data {
        Data(
            HMAC<SHA256>.authenticationCode(
                for: transcript(initPacket) + encryptedClientName, using: SymmetricKey(data: secret)))
    }
    public func verify(_ status: PairingStatus) throws -> (client: RelayDevice, initPacket: Data) {
        guard status.id == invitationID, status.host.id == hostID, status.host.publicKeyData == hostKey,
            var client = status.client, client.kind == .client, let key = client.publicKeyData,
            let packet = status.initPacket, let proof = status.proof,
            let encryptedName = status.encryptedClientName,
            try SecureChannel.clientStaticKey(inInit: packet) == key,
            HMAC<SHA256>.isValidAuthenticationCode(
                proof, authenticating: transcript(packet) + encryptedName, using: SymmetricKey(data: secret)
            )
        else { throw PairingError.invalidProof }
        client.name = try PrivateMetadata.open(encryptedName, key: nameKey, context: transcript(packet))
        return (client, packet)
    }
}
public enum RelayAddress {
    /// HTTPS for public relays; HTTP only for loopback development servers.
    public static func validate(_ url: URL) throws -> URL {
        guard let p = URLComponents(url: url, resolvingAgainstBaseURL: false), let host = p.host, !host.isEmpty,
            p.user == nil, p.password == nil, p.query == nil, p.fragment == nil,
            p.scheme == "https" || (p.scheme == "http" && ["localhost", "127.0.0.1", "[::1]", "::1"].contains(host))
        else { throw PairingError.invalidCode }
        return url
    }
}
