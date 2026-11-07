import Foundation
import Testing

@testable import TermyRemoteKit

@Suite struct PrivateMetadataTests {
    @Test func onlyOwnerCanDecryptAndNamesArePadded() throws {
        let owner = DeviceIdentity()
        let name = "Lasse’s private Mac"
        let first = try PrivateMetadata.sealName(name, identity: owner)
        let second = try PrivateMetadata.sealName(name, identity: owner)
        #expect(first.count == 285)
        #expect(first != second)
        #expect(try PrivateMetadata.sealName("Mac", identity: owner).count == first.count)
        #expect(try PrivateMetadata.openName(first, identity: owner) == name)
        #expect(throws: (any Error).self) {
            try PrivateMetadata.openName(first, identity: DeviceIdentity())
        }
        var tampered = first
        tampered[40] ^= 1
        #expect(throws: (any Error).self) { try PrivateMetadata.openName(tampered, identity: owner) }
        #expect(throws: (any Error).self) {
            try PrivateMetadata.openName(first.dropLast(), identity: owner)
        }
    }
    @Test func registrationNeverEncodesPlaintextName() throws {
        let registration = try DeviceRegistration(
            name: "Secret Mac", kind: .host, identity: DeviceIdentity())
        let json = try JSONEncoder().encode(registration)
        let object = try #require(JSONSerialization.jsonObject(with: json) as? [String: Any])
        #expect(object["name"] == nil)
        #expect(object["encryptedName"] != nil)
        #expect(!String(decoding: json, as: UTF8.self).contains("Secret Mac"))
        #expect(throws: (any Error).self) {
            try PrivateMetadata.sealName("  ", identity: DeviceIdentity())
        }
    }
    @Test func discoveryCannotSpoofOrOverwriteVerifiedNames() {
        let identity = DeviceIdentity()
        let cached = RelayDevice(id: UUID(), name: "My private Mac", kind: .host,
            publicKey: identity.publicKey.base64EncodedString(), online: false, createdAt: Date(), lastSeenAt: nil)
        var discovered = cached
        discovered.name = "Server-controlled name"
        discovered.online = true
        #expect(discovered.withLocalName(from: cached).name == "My private Mac")
        #expect(discovered.withLocalName(from: cached).online)
        #expect(discovered.withLocalName(from: nil).name == "Mac")
        discovered.publicKey = DeviceIdentity().publicKey.base64EncodedString()
        #expect(discovered.withLocalName(from: cached).name == "Mac")
        discovered = cached
        discovered.id = UUID()
        #expect(discovered.withLocalName(from: cached).name == "Mac")
    }
    @Test func pairingCiphertextIsBoundToInvitationAndHandshake() throws {
        let host = DeviceIdentity()
        let phone = DeviceIdentity()
        let device = RelayDevice(
            id: UUID(), name: "Mac", kind: .host,
            publicKey: host.publicKey.base64EncodedString(), online: false, createdAt: Date(),
            lastSeenAt: nil)
        let status = PairingStatus(
            id: UUID(), expiresAt: Date().addingTimeInterval(120), host: device,
            client: nil, initPacket: nil, proof: nil, responsePacket: nil)
        let code = try PairingCode(
            status: status, relayURL: URL(string: "https://relay.example")!, hostName: "My Mac")
        let handshake = SecureChannel.startClient(identity: phone, hostStaticKey: host.publicKey)
        var pending = status
        pending.client = RelayDevice(
            id: UUID(), name: "Relay-forged name", kind: .client,
            publicKey: phone.publicKey.base64EncodedString(), online: false, createdAt: Date(),
            lastSeenAt: nil)
        pending.initPacket = handshake.initPacket
        pending.encryptedClientName = try code.sealClientName(
            "My iPhone", initPacket: handshake.initPacket)
        pending.proof = code.proof(
            for: handshake.initPacket, encryptedClientName: pending.encryptedClientName!)
        #expect(try code.verify(pending).client.name == "My iPhone")
        // Even a valid QR-secret HMAC cannot transplant ciphertext to another handshake.
        let next = SecureChannel.startClient(identity: phone, hostStaticKey: host.publicKey)
        pending.initPacket = next.initPacket
        pending.proof = code.proof(
            for: next.initPacket, encryptedClientName: pending.encryptedClientName!)
        #expect(throws: (any Error).self) { try code.verify(pending) }
    }
}
