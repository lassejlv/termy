import CryptoKit
import Foundation

public enum PairingError: Error, LocalizedError {
    case invalidCode, expired, invalidProof, storage
    /// A code from a Termy Next that paired through the relay, which Termy no longer has.
    case relayCode
    /// A code from an older Termy Next that pairs another way than this app.
    case outdatedCode
    /// A code from a newer Termy Next than this app understands.
    case newerCode
    public var errorDescription: String? {
        switch self {
        case .invalidCode: "This is not a valid Termy pairing code."
        case .expired: "This pairing code expired. Generate a new one on your Mac."
        case .invalidProof: "The pairing identities could not be verified. Generate a new QR code."
        case .storage: "Could not save device credentials securely."
        case .relayCode:
            "This code is from a Termy Next that pairs through a relay, which Termy no longer uses. Update Termy Next on your Mac, then pair again."
        case .outdatedCode: "This code is from an older Termy Next. Update Termy Next on your Mac, then pair again."
        case .newerCode: "This code is from a newer Termy Next. Update Termy on this iPhone, then pair again."
        }
    }
}

public enum DeviceKind: String, Codable, Sendable {
    /// A computer running a Termy remote host, whose sessions others can open.
    case host
    /// A phone that opens sessions.
    case client
}

/// A device this one paired with, under the ID and name it gave in the pairing.
public struct PairedDevice: Codable, Hashable, Sendable, Identifiable {
    public var id: UUID
    public var name: String
    public var kind: DeviceKind
    /// Base64 X25519 public key.
    public var publicKey: String
    public var online: Bool

    public init(id: UUID, name: String, kind: DeviceKind, publicKey: Data, online: Bool = false) {
        self.id = id
        self.name = name
        self.kind = kind
        self.publicKey = publicKey.base64EncodedString()
        self.online = online
    }

    public var publicKeyData: Data? { Data(base64Encoded: publicKey) }
    public var fingerprint: String { publicKeyData.map(Fingerprint.of) ?? "?" }
}

/// What the QR code on a Mac carries: its Tailscale address, its key, and a one-time
/// secret that never leaves the two devices. The phone claims the invitation on a
/// direct connection (`DirectPairing`), and the secret's HMAC authenticates the claim.
///
/// Version 5. Versions 3 (through a relay) and 4 (from before the relay was removed)
/// are refused with a message that says to update Termy Next.
public struct PairingCode: Codable, Sendable {
    public static let currentVersion = 5

    public let version: Int
    /// Where the Mac listens.
    public let endpoints: [DirectEndpoint]
    public let invitationID: UUID
    public let hostID: UUID
    public let hostName: String
    public let hostKey: Data
    public let secret: Data
    public let expiresAt: Date

    /// An invitation for a phone that can reach one of `endpoints`.
    public init(
        hostID: UUID, hostName: String, hostKey: Data, endpoints: [DirectEndpoint], lifetime: TimeInterval = 120
    ) throws {
        let usable = Array(endpoints.filter(\.isDialable).prefix(DirectEndpoint.maxCount))
        guard hostKey.count == 32, !usable.isEmpty, !hostName.isEmpty else { throw PairingError.invalidCode }
        version = Self.currentVersion
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
        guard let data = Data(base64Encoded: b64) else { throw PairingError.invalidCode }
        // Say what an old code is before insisting on the current layout.
        struct Header: Decodable {
            var version: Int
            var relayURL: String?
        }
        guard let header = try? JSONDecoder().decode(Header.self, from: data) else { throw PairingError.invalidCode }
        if header.relayURL != nil || header.version == 3 { throw PairingError.relayCode }
        if header.version < currentVersion { throw PairingError.outdatedCode }
        if header.version > currentVersion { throw PairingError.newerCode }
        guard let code = try? JSONDecoder().decode(Self.self, from: data),
            code.hostKey.count == 32, code.secret.count == 32, !code.hostName.isEmpty,
            !code.endpoints.isEmpty, code.endpoints.count <= DirectEndpoint.maxCount,
            code.endpoints.allSatisfy(\.isDialable)
        else { throw PairingError.invalidCode }
        guard code.expiresAt > now, code.expiresAt.timeIntervalSince(now) <= 180 else { throw PairingError.expired }
        return code
    }
    private func transcript(_ initPacket: Data) -> Data {
        Data("termy-pair-v\(version)|\(ids)|".utf8) + hostKey + initPacket
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
}

/// What a phone sends on a direct connection to claim an invitation. The Mac shows it
/// for approval only when its HMAC, made with the QR secret, covers all of it.
public struct DirectPairingClaim: Codable, Sendable {
    public var invitationID: UUID
    /// The ID the phone chose for itself; the Mac knows it by this from now on.
    public var clientID: UUID
    public var initPacket: Data
    public var proof: Data
    public var encryptedClientName: Data
}

extension PairingCode {
    func proof(_ initPacket: Data, clientID: UUID, encryptedClientName: Data) -> Data {
        Data(
            HMAC<SHA256>.authenticationCode(
                for: transcript(initPacket) + Data(clientID.uuidString.lowercased().utf8) + encryptedClientName,
                using: SymmetricKey(data: secret)))
    }

    public func claim(clientID: UUID, name: String, initPacket: Data) throws -> DirectPairingClaim {
        let encryptedName = try sealClientName(name, initPacket: initPacket)
        return DirectPairingClaim(
            invitationID: invitationID, clientID: clientID, initPacket: initPacket,
            proof: proof(initPacket, clientID: clientID, encryptedClientName: encryptedName),
            encryptedClientName: encryptedName)
    }

    /// The phone a claim comes from, once the claim is proven to come from whoever read the QR code.
    public func verify(_ claim: DirectPairingClaim, now: Date = Date()) throws -> (client: PairedDevice, initPacket: Data) {
        guard version == Self.currentVersion, expiresAt > now else { throw PairingError.expired }
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
        return (PairedDevice(id: claim.clientID, name: name, kind: .client, publicKey: key), claim.initPacket)
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
