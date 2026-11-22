import CryptoKit
import Foundation
import Testing

@testable import TermyRemoteKit

@Suite struct PrivateMetadataTests {
    @Test func namesArePaddedAndOnlyTheirKeyOpensThem() throws {
        let key = SymmetricKey(size: .bits256)
        let context = Data("context".utf8)
        let name = "Lasse’s private iPhone"
        let first = try PrivateMetadata.seal(name, key: key, context: context)
        let second = try PrivateMetadata.seal(name, key: key, context: context)
        #expect(first.count == 285)
        #expect(first != second)
        #expect(try PrivateMetadata.seal("iPhone", key: key, context: context).count == first.count)
        #expect(try PrivateMetadata.open(first, key: key, context: context) == name)
        #expect(throws: (any Error).self) {
            try PrivateMetadata.open(first, key: SymmetricKey(size: .bits256), context: context)
        }
        #expect(throws: (any Error).self) { try PrivateMetadata.open(first, key: key, context: Data("other".utf8)) }
        var tampered = first
        tampered[40] ^= 1
        #expect(throws: (any Error).self) { try PrivateMetadata.open(tampered, key: key, context: context) }
        #expect(throws: (any Error).self) { try PrivateMetadata.open(first.dropLast(), key: key, context: context) }
        #expect(throws: (any Error).self) { try PrivateMetadata.seal("  ", key: key, context: context) }
    }

    @Test func aClaimedNameIsBoundToItsHandshake() throws {
        let host = DeviceIdentity()
        let phone = DeviceIdentity()
        let code = try PairingCode(
            hostID: UUID(), hostName: "My Mac", hostKey: host.publicKey,
            endpoints: [DirectEndpoint(host: "100.81.122.65", port: 47126)])
        let handshake = SecureChannel.startClient(identity: phone, hostStaticKey: host.publicKey)
        var claim = try code.claim(clientID: UUID(), name: "My iPhone", initPacket: handshake.initPacket)
        #expect(try code.verify(claim).client.name == "My iPhone")
        // Even a valid QR-secret HMAC cannot transplant the sealed name to another handshake.
        let next = SecureChannel.startClient(identity: phone, hostStaticKey: host.publicKey)
        claim.initPacket = next.initPacket
        claim.proof = code.proof(next.initPacket, clientID: claim.clientID, encryptedClientName: claim.encryptedClientName)
        #expect(throws: (any Error).self) { try code.verify(claim) }
    }
}
