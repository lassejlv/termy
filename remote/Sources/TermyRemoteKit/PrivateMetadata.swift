import CryptoKit
import Foundation

/// Private names are padded before sealing so their lengths do not identify devices.
/// The relay receives only versioned, authenticated ciphertext, never these keys.
public enum PrivateMetadata {
    private static let size = 256
    public static func sealName(_ name: String, identity: DeviceIdentity) throws -> Data {
        try seal(
            name, key: key(identity), context: Data("termy-device-name-v1".utf8) + identity.publicKey)
    }
    public static func openName(_ ciphertext: Data, identity: DeviceIdentity) throws -> String {
        try open(
            ciphertext, key: key(identity),
            context: Data("termy-device-name-v1".utf8) + identity.publicKey)
    }
    private static func key(_ identity: DeviceIdentity) -> SymmetricKey {
        HKDF<SHA256>.deriveKey(
            inputKeyMaterial: SymmetricKey(data: identity.rawRepresentation),
            salt: identity.publicKey, info: Data("termy-private-metadata-v1".utf8), outputByteCount: 32)
    }
    static func seal(_ name: String, key: SymmetricKey, context: Data) throws -> Data {
        let bytes = Data(name.trimmingCharacters(in: .whitespacesAndNewlines).prefix(80).utf8)
        guard !bytes.isEmpty, bytes.count <= size - 2 else { throw PairingError.invalidProof }
        var padded = Data([UInt8(bytes.count >> 8), UInt8(bytes.count & 255)]) + bytes
        padded.append(Data(repeating: 0, count: size - padded.count))
        return Data([1]) + (try ChaChaPoly.seal(padded, using: key, authenticating: context)).combined
    }
    static func open(_ ciphertext: Data, key: SymmetricKey, context: Data) throws -> String {
        guard ciphertext.count == 285, ciphertext.first == 1 else { throw PairingError.invalidProof }
        let plaintext = try ChaChaPoly.open(
            ChaChaPoly.SealedBox(combined: ciphertext.dropFirst()), using: key, authenticating: context)
        let bytes = [UInt8](plaintext)
        let length = Int(bytes[0]) << 8 | Int(bytes[1])
        guard (1...size - 2).contains(length),
            let name = String(data: plaintext.dropFirst(2).prefix(length), encoding: .utf8),
            name.count <= 80, !name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        else { throw PairingError.invalidProof }
        return name
    }
}
