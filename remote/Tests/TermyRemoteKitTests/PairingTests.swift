import Foundation
import Testing

@testable import TermyRemoteKit

@Suite struct PairingTests {
    private func device(_ identity: DeviceIdentity, kind: DeviceKind) -> RelayDevice {
        RelayDevice(
            id: UUID(), name: kind == .host ? "Mac" : "iPhone", kind: kind,
            publicKey: identity.publicKey.base64EncodedString(), online: false, createdAt: Date(), lastSeenAt: nil)
    }
    private func fixture() throws -> (PairingCode, PairingStatus, SecureChannel.ClientHandshake, DeviceIdentity) {
        let host = DeviceIdentity()
        let phone = DeviceIdentity()
        let h = device(host, kind: .host)
        let c = device(phone, kind: .client)
        var status = PairingStatus(
            id: UUID(), expiresAt: Date().addingTimeInterval(120), host: h, client: nil, initPacket: nil, proof: nil,
            responsePacket: nil)
        let code = try PairingCode(
            status: status, relayURL: URL(string: "https://my-vps.example/relay")!,
            hostName: "Private Mac")
        let handshake = SecureChannel.startClient(identity: phone, hostStaticKey: host.publicKey)
        status.client = c
        status.initPacket = handshake.initPacket
        status.encryptedClientName = try code.sealClientName(
            "Private iPhone", initPacket: handshake.initPacket)
        status.proof = code.proof(
            for: handshake.initPacket, encryptedClientName: status.encryptedClientName!)
        return (code, status, handshake, host)
    }
    @Test func qrRoundTripAndAuthenticatedApproval() throws {
        let (code, status, handshake, host) = try fixture()
        let scanned = try PairingCode.parse(code.text)
        #expect(scanned.relayURL == code.relayURL)
        #expect(scanned.secret == code.secret)
        let verified = try scanned.verify(status)
        #expect(verified.client.name == "Private iPhone")
        let (_, response) = try SecureChannel.respond(identity: host, initPacket: verified.initPacket)
        #expect(response.count == 59)
        _ = try handshake.finish(response)
    }
    @Test func rejectsRelaySubstitutionAndReplayedInvitations() throws {
        let (code, original, _, _) = try fixture()
        var altered = original
        altered.client = device(DeviceIdentity(), kind: .client)
        #expect(throws: (any Error).self) { try code.verify(altered) }
        altered = original
        altered.id = UUID()
        #expect(throws: (any Error).self) { try code.verify(altered) }
        altered = original
        altered.host = device(DeviceIdentity(), kind: .host)
        #expect(throws: (any Error).self) { try code.verify(altered) }
        altered = original
        altered.initPacket![1] ^= 1
        #expect(throws: (any Error).self) { try code.verify(altered) }
        altered = original
        altered.encryptedClientName![40] ^= 1
        #expect(throws: (any Error).self) { try code.verify(altered) }
        altered = original
        altered.encryptedClientName = nil
        #expect(throws: (any Error).self) { try code.verify(altered) }
        // Another invite has another secret even for the same Mac/device IDs.
        let otherCode = try PairingCode(
            status: original, relayURL: code.relayURL, hostName: "Private Mac")
        #expect(throws: (any Error).self) { try otherCode.verify(original) }
    }
    @Test func rejectsExpiredAndUntrustedTransportURLs() throws {
        let (code, _, _, _) = try fixture()
        #expect(throws: PairingError.expired) {
            try PairingCode.parse(code.text, now: code.expiresAt.addingTimeInterval(1))
        }
        for address in [
            "http://relay.example.com", "https://user:password@relay.example.com", "file:///tmp/relay",
            "https://relay.example.com?token=secret", "https://relay.example.com/#secret",
        ] {
            #expect(throws: (any Error).self) { try RelayAddress.validate(URL(string: address)!) }
        }
        #expect(try RelayAddress.validate(URL(string: "http://127.0.0.1:8787")!).host == "127.0.0.1")
        #expect(throws: (any Error).self) { try PairingCode.parse("termy://pair?data=garbage") }
    }
}
