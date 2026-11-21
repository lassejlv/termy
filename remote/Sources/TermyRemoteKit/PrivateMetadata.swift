import CryptoKit
import Foundation

/// Private names are padded before sealing so their lengths do not identify devices.
/// The phone's name travels this way in its pairing claim, under a key from the QR secret.
public enum PrivateMetadata {
    private static let size = 256
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
