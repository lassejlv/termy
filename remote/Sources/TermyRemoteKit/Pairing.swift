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
///
/// Version 3 pairs through a relay. Version 4 names no relay: it carries the Mac's
/// addresses, and the phone claims the invitation on a direct connection (`DirectPairing`).
public struct PairingCode: Codable, Sendable {
    public let version: Int
    /// Nil in a code that pairs without a relay.
    public let relayURL: URL?
    /// Where the Mac listens, in a code that pairs without a relay.
    public let endpoints: [DirectEndpoint]?
    public let invitationID: UUID
    public let hostID: UUID
    public let hostName: String
    public let hostKey: Data
    public let secret: Data
    public let expiresAt: Date

    public init(status: PairingStatus, relayURL: URL, hostName: String) throws {
        self.version = 3
        self.relayURL = try RelayAddress.validate(relayURL)
        endpoints = nil
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
    /// An invitation a Mac makes by itself, for a phone that can reach one of `endpoints`.
    public init(
        hostID: UUID, hostName: String, hostKey: Data, endpoints: [DirectEndpoint], lifetime: TimeInterval = 120
    ) throws {
        let usable = Array(endpoints.filter { $0.kind != nil }.prefix(DirectEndpoint.maxCount))
        guard hostKey.count == 32, !usable.isEmpty, !hostName.isEmpty else { throw PairingError.invalidCode }
        version = 4
        relayURL = nil
        self.endpoints = usable
        invitationID = UUID()
        self.hostID = hostID
        self.hostName = hostName
        self.hostKey = hostKey
        expiresAt = Date().addingTimeInterval(lifetime)
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
            code.hostKey.count == 32, code.secret.count == 32, !code.hostName.isEmpty
        else { throw PairingError.invalidCode }
        switch (code.version, code.relayURL, code.endpoints) {
        case (3, let relay?, nil): _ = try RelayAddress.validate(relay)
        case (4, nil, let endpoints?):
            guard !endpoints.isEmpty, endpoints.count <= DirectEndpoint.maxCount,
                endpoints.allSatisfy({ $0.kind != nil })
            else { throw PairingError.invalidCode }
        default: throw PairingError.invalidCode
        }
        guard code.expiresAt > now, code.expiresAt.timeIntervalSince(now) <= 180 else { throw PairingError.expired }
        return code
    }
    private func transcript(_ initPacket: Data) -> Data {
        let where_ = relayURL.map { "termy-pair-v3|\(ids)|\($0.absoluteString)|" } ?? "termy-pair-v4|\(ids)|direct|"
        return Data(where_.utf8) + hostKey + initPacket
    }
    private var ids: String { "\(invitationID.uuidString.lowercased())|\(hostID.uuidString.lowercased())" }
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
// MARK: - Pairing without a relay

/// What a phone sends on a direct connection to claim a version-4 invitation. The Mac
/// shows it for approval only when its HMAC, made with the QR secret, covers all of it.
public struct DirectPairingClaim: Codable, Sendable {
    public var invitationID: UUID
    /// The ID the phone chose for itself; the Mac knows it by this from now on.
    public var clientID: UUID
    public var initPacket: Data
    public var proof: Data
    public var encryptedClientName: Data
}

extension PairingCode {
    private func directProof(_ initPacket: Data, clientID: UUID, encryptedClientName: Data) -> Data {
        Data(
            HMAC<SHA256>.authenticationCode(
                for: transcript(initPacket) + Data(clientID.uuidString.lowercased().utf8) + encryptedClientName,
                using: SymmetricKey(data: secret)))
    }

    public func claim(clientID: UUID, name: String, initPacket: Data) throws -> DirectPairingClaim {
        let encryptedName = try sealClientName(name, initPacket: initPacket)
        return DirectPairingClaim(
            invitationID: invitationID, clientID: clientID, initPacket: initPacket,
            proof: directProof(initPacket, clientID: clientID, encryptedClientName: encryptedName),
            encryptedClientName: encryptedName)
    }

    /// The phone a claim comes from, once the claim is proven to come from whoever read the QR code.
    public func verify(_ claim: DirectPairingClaim, now: Date = Date()) throws -> (client: RelayDevice, initPacket: Data) {
        guard version == 4, expiresAt > now else { throw PairingError.expired }
        guard claim.invitationID == invitationID,
            let key = try? SecureChannel.clientStaticKey(inInit: claim.initPacket),
            HMAC<SHA256>.isValidAuthenticationCode(
                claim.proof,
                authenticating: transcript(claim.initPacket) + Data(claim.clientID.uuidString.lowercased().utf8)
                    + claim.encryptedClientName,
                using: SymmetricKey(data: secret))
        else { throw PairingError.invalidProof }
        let name = try PrivateMetadata.open(
            claim.encryptedClientName, key: nameKey, context: transcript(claim.initPacket))
        return (RelayDevice(id: claim.clientID, name: name, kind: .client, publicKey: key), claim.initPacket)
    }
}

/// The two pairing packets on a direct connection: the phone's claim, and the Mac's
/// answer once its user approved, which is the handshake response proving the Mac's key.
public enum DirectPairing {
    static let maxClaimBytes = 4096

    public static func packet(_ claim: DirectPairingClaim) throws -> Data {
        Data([PacketKind.pairingClaim.rawValue]) + (try JSONEncoder().encode(claim))
    }

    public static func claim(in packet: Data) -> DirectPairingClaim? {
        guard packet.first == PacketKind.pairingClaim.rawValue, packet.count <= maxClaimBytes else { return nil }
        return try? JSONDecoder().decode(DirectPairingClaim.self, from: packet.dropFirst())
    }

    public static func answer(_ handshakeResponse: Data) -> Data {
        Data([PacketKind.pairingAnswer.rawValue]) + handshakeResponse
    }

    public static func handshakeResponse(in packet: Data) -> Data? {
        guard packet.first == PacketKind.pairingAnswer.rawValue, packet.count > 1 else { return nil }
        return Data(packet.dropFirst())
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
