import CryptoKit
import Foundation

/// A device's long-term key. The relay stores the public half with the device
/// so peers can find it, but the private half never leaves the device, and a
/// peer pins the public half the first time it sees it (see `TrustStore`).
public struct DeviceIdentity: Sendable {
    public let privateKey: Curve25519.KeyAgreement.PrivateKey

    public init(privateKey: Curve25519.KeyAgreement.PrivateKey = .init()) {
        self.privateKey = privateKey
    }

    public init(rawRepresentation: Data) throws {
        privateKey = try .init(rawRepresentation: rawRepresentation)
    }

    public var rawRepresentation: Data { privateKey.rawRepresentation }
    public var publicKey: Data { privateKey.publicKey.rawRepresentation }
    public var fingerprint: String { Fingerprint.of(publicKey) }
}

public enum Fingerprint {
    /// Eight groups of four hex digits from the SHA-256 of a public key, short
    /// enough to compare by eye on two screens.
    public static func of(_ publicKey: Data) -> String {
        let digest = SHA256.hash(data: publicKey).prefix(16)
        let hex = digest.map { String(format: "%02x", $0) }.joined()
        return stride(from: 0, to: hex.count, by: 4).map { offset in
            let start = hex.index(hex.startIndex, offsetBy: offset)
            return String(hex[start ..< hex.index(start, offsetBy: 4)])
        }.joined(separator: " ")
    }
}

public enum ChannelError: Error, Equatable, Sendable {
    case malformed
    case unexpected
    /// The peer's handshake didn't prove it holds the key the relay listed.
    case authenticationFailed
    case replayed
}

/// Packet kinds inside a relay `data` message.
enum PacketKind: UInt8 {
    case handshakeInit = 1
    case handshakeResponse = 2
    case transport = 3
}

/// Directional keys and counters for one authenticated session between a
/// client (the phone) and a host (the computer running `termy-remote`).
///
/// The handshake mixes three X25519 results, as Noise KK does:
/// ephemeral–ephemeral for forward secrecy, client-ephemeral–host-static so
/// only the real host can answer, and host-ephemeral–client-static so only the
/// real client can send. Both static keys and both ephemerals are bound into
/// the HKDF salt. Transport packets are ChaCha20-Poly1305 with a 64-bit
/// counter nonce per direction; a counter that doesn't increase is rejected.
public struct SecureChannel: Sendable {
    public enum Role: Sendable { case client, host }

    public let role: Role
    public let peerStaticKey: Data
    private let sendKey: SymmetricKey
    private let receiveKey: SymmetricKey
    private var sendCounter: UInt64 = 0
    /// The last counter opened; nil until the first packet.
    private var receiveCounter: UInt64?

    static let protocolName = Data("termy-remote-v1".utf8)

    private init(role: Role, peerStaticKey: Data, sendKey: SymmetricKey, receiveKey: SymmetricKey) {
        self.role = role
        self.peerStaticKey = peerStaticKey
        self.sendKey = sendKey
        self.receiveKey = receiveKey
    }

    static func deriveKeys(
        dh: [SharedSecret], clientStatic: Data, hostStatic: Data, clientEphemeral: Data, hostEphemeral: Data
    ) -> (clientToHost: SymmetricKey, hostToClient: SymmetricKey) {
        var ikm = Data()
        for secret in dh { secret.withUnsafeBytes { ikm.append(contentsOf: $0) } }
        var transcript = protocolName
        transcript += clientStatic + hostStatic + clientEphemeral + hostEphemeral
        let salt = Data(SHA256.hash(data: transcript))
        let okm = HKDF<SHA256>.deriveKey(
            inputKeyMaterial: SymmetricKey(data: ikm), salt: salt, info: Data("keys".utf8), outputByteCount: 64
        )
        let bytes = okm.withUnsafeBytes { Data($0) }
        return (SymmetricKey(data: bytes.prefix(32)), SymmetricKey(data: bytes.suffix(32)))
    }

    // MARK: Client

    /// The client's half of a handshake in progress.
    public struct ClientHandshake: Sendable {
        let identity: DeviceIdentity
        let hostStaticKey: Data
        let ephemeral: Curve25519.KeyAgreement.PrivateKey

        /// Sent to the host: our ephemeral and static public keys.
        public let initPacket: Data

        /// Completes the handshake from the host's response, which must prove
        /// the host holds the static key we expected.
        public func finish(_ packet: Data) throws -> SecureChannel {
            guard packet.first == PacketKind.handshakeResponse.rawValue, packet.count > 33 else {
                throw ChannelError.malformed
            }
            let hostEphemeralRaw = Data(packet[packet.startIndex + 1 ..< packet.startIndex + 33])
            let confirmation = Data(packet[(packet.startIndex + 33)...])
            let hostEphemeral = try Curve25519.KeyAgreement.PublicKey(rawRepresentation: hostEphemeralRaw)
            let hostStatic = try Curve25519.KeyAgreement.PublicKey(rawRepresentation: hostStaticKey)
            let dh = [
                try ephemeral.sharedSecretFromKeyAgreement(with: hostEphemeral),
                try ephemeral.sharedSecretFromKeyAgreement(with: hostStatic),
                try identity.privateKey.sharedSecretFromKeyAgreement(with: hostEphemeral),
            ]
            let keys = SecureChannel.deriveKeys(
                dh: dh, clientStatic: identity.publicKey, hostStatic: hostStaticKey,
                clientEphemeral: ephemeral.publicKey.rawRepresentation, hostEphemeral: hostEphemeralRaw
            )
            var channel = SecureChannel(
                role: .client, peerStaticKey: hostStaticKey, sendKey: keys.clientToHost, receiveKey: keys.hostToClient
            )
            do {
                guard try channel.open(confirmation) == Data("ok".utf8) else { throw ChannelError.authenticationFailed }
            } catch {
                throw ChannelError.authenticationFailed
            }
            return channel
        }
    }

    public static func startClient(identity: DeviceIdentity, hostStaticKey: Data) -> ClientHandshake {
        let ephemeral = Curve25519.KeyAgreement.PrivateKey()
        var packet = Data([PacketKind.handshakeInit.rawValue])
        packet += ephemeral.publicKey.rawRepresentation
        packet += identity.publicKey
        return ClientHandshake(identity: identity, hostStaticKey: hostStaticKey, ephemeral: ephemeral, initPacket: packet)
    }

    // MARK: Host

    /// The client's static key from an init packet, so the host can decide
    /// whether it trusts the device before answering.
    public static func clientStaticKey(inInit packet: Data) throws -> Data {
        guard packet.first == PacketKind.handshakeInit.rawValue, packet.count == 65 else { throw ChannelError.malformed }
        return Data(packet[(packet.startIndex + 33)...])
    }

    /// Answers a client's init packet. Returns the channel and the response to
    /// send back. The client is authenticated by its first transport packet:
    /// only the holder of its static key can derive the receive key.
    public static func respond(identity: DeviceIdentity, initPacket packet: Data) throws -> (SecureChannel, Data) {
        let clientStaticRaw = try clientStaticKey(inInit: packet)
        let clientEphemeralRaw = Data(packet[packet.startIndex + 1 ..< packet.startIndex + 33])
        let clientEphemeral = try Curve25519.KeyAgreement.PublicKey(rawRepresentation: clientEphemeralRaw)
        let clientStatic = try Curve25519.KeyAgreement.PublicKey(rawRepresentation: clientStaticRaw)
        let ephemeral = Curve25519.KeyAgreement.PrivateKey()
        let dh = [
            try ephemeral.sharedSecretFromKeyAgreement(with: clientEphemeral),
            try identity.privateKey.sharedSecretFromKeyAgreement(with: clientEphemeral),
            try ephemeral.sharedSecretFromKeyAgreement(with: clientStatic),
        ]
        let keys = deriveKeys(
            dh: dh, clientStatic: clientStaticRaw, hostStatic: identity.publicKey,
            clientEphemeral: clientEphemeralRaw, hostEphemeral: ephemeral.publicKey.rawRepresentation
        )
        var channel = SecureChannel(
            role: .host, peerStaticKey: clientStaticRaw, sendKey: keys.hostToClient, receiveKey: keys.clientToHost
        )
        var response = Data([PacketKind.handshakeResponse.rawValue])
        response += ephemeral.publicKey.rawRepresentation
        response += try channel.sealBody(Data("ok".utf8))
        return (channel, response)
    }

    // MARK: Transport

    private static func nonce(_ counter: UInt64) throws -> ChaChaPoly.Nonce {
        var bytes = Data(count: 4)
        withUnsafeBytes(of: counter.bigEndian) { bytes.append(contentsOf: $0) }
        return try ChaChaPoly.Nonce(data: bytes)
    }

    /// Counter (8 bytes, big endian) then ciphertext and tag.
    private mutating func sealBody(_ plaintext: Data) throws -> Data {
        let counter = sendCounter
        sendCounter += 1
        let box = try ChaChaPoly.seal(plaintext, using: sendKey, nonce: Self.nonce(counter))
        var out = Data()
        withUnsafeBytes(of: counter.bigEndian) { out.append(contentsOf: $0) }
        out += box.ciphertext + box.tag
        return out
    }

    private mutating func open(_ body: Data) throws -> Data {
        guard body.count >= 8 + 16 else { throw ChannelError.malformed }
        let counter = body.prefix(8).reduce(UInt64(0)) { $0 << 8 | UInt64($1) }
        if let last = receiveCounter, counter <= last {
            throw ChannelError.replayed
        }
        let rest = body.dropFirst(8)
        let box = try ChaChaPoly.SealedBox(
            nonce: Self.nonce(counter), ciphertext: rest.dropLast(16), tag: rest.suffix(16)
        )
        let plaintext = try ChaChaPoly.open(box, using: receiveKey)
        receiveCounter = counter
        return plaintext
    }

    public mutating func seal(_ plaintext: Data) throws -> Data {
        Data([PacketKind.transport.rawValue]) + (try sealBody(plaintext))
    }

    public mutating func open(packet: Data) throws -> Data {
        guard packet.first == PacketKind.transport.rawValue else { throw ChannelError.unexpected }
        return try open(Data(packet.dropFirst()))
    }

    /// Whether a packet starts a new handshake (the peer reconnected).
    public static func isHandshakeInit(_ packet: Data) -> Bool {
        packet.first == PacketKind.handshakeInit.rawValue
    }
}
