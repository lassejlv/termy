import CryptoKit
import Foundation
import Network
import Termy
import Testing

@testable import TermyRemoteHost
@testable import TermyRemoteKit

/// Phones and a real listener on this Mac.
@Suite("Direct connections", .serialized)
@MainActor struct DirectServerTests {
    /// A phone's end of a direct connection.
    @MainActor final class Phone {
        let connection: DirectConnection
        var channel: SecureChannel?
        var received: [HostMessage] = []
        var closed = false
        var packets = 0
        /// Every packet as it arrived.
        var raw: [Data] = []
        private var handshake: SecureChannel.ClientHandshake?

        /// `firstPacket` replaces the handshake a real phone starts with.
        init(port: Int, identity: DeviceIdentity, hostKey: Data, firstPacket: Data? = nil) {
            connection = DirectConnection(
                NWConnection(
                    host: "127.0.0.1", port: NWEndpoint.Port(rawValue: UInt16(port))!,
                    using: DirectConnection.parameters()),
                remote: nil, timeout: 3)
            let handshake = SecureChannel.startClient(identity: identity, hostStaticKey: hostKey)
            if firstPacket == nil { self.handshake = handshake }
            Task {
                for await event in connection.events {
                    switch event {
                    case .ready: connection.send(firstPacket ?? handshake.initPacket)
                    case .packet(let packet): self.receive(packet)
                    case .closed: self.closed = true
                    }
                }
            }
        }

        private func receive(_ packet: Data) {
            packets += 1
            raw.append(packet)
            if let handshake {
                channel = try? handshake.finish(packet)
                self.handshake = nil
            } else if var channel, let plaintext = try? channel.open(packet: packet),
                let message = try? MessageCodec.decode(HostMessage.self, from: plaintext)
            {
                self.channel = channel
                received.append(message)
            }
        }

        func send(_ message: ClientMessage) throws {
            guard var channel else { return }
            connection.send(try channel.seal(try MessageCodec.encode(message)))
            self.channel = channel
        }

        var sessionLists: Int {
            received.filter { if case .sessions = $0 { true } else { false } }.count
        }
    }

    private func eventually(line: Int = #line, _ predicate: () -> Bool) async throws {
        let deadline = Date().addingTimeInterval(12)
        while Date() < deadline {
            if predicate() { return }
            try await Task.sleep(for: .milliseconds(20))
        }
        Issue.record("Timed out waiting at line \(line).")
        throw CancellationError()
    }

    @Test(.timeLimit(.minutes(1))) func pairedPhonesConnectAndOthersDoNot() async throws {
        let executable = try #require(Multiplexer.bundledHostExecutable)
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(
            "termy-direct-\(UUID().uuidString.prefix(8))")
        let mux = try Multiplexer.connect(root: root, hostExecutable: executable)
        defer {
            try? mux.shutdown()
            try? FileManager.default.removeItem(at: root)
        }
        let identity = DeviceIdentity()
        let phoneIdentity = DeviceIdentity()
        let phoneID = UUID()
        let agent = RemoteHostAgent(
            multiplexer: mux, hostID: UUID(), identity: identity, trustedDevices: [phoneID: phoneIdentity.publicKey])
        agent.directPort = nil
        agent.interfaces = {
            [
                NetworkInterface(name: "en0", address: IPv4("192.168.1.20")!),
                NetworkInterface(name: "utun8", address: IPv4("100.81.122.65")!),
            ]
        }
        var advertised: [DirectEndpoint] = []
        agent.onDirectEndpoints = { advertised = $0 }
        let serving = Task { try await agent.run() }
        defer {
            serving.cancel()
            agent.stop()
        }
        try await eventually { advertised.count == 1 }
        let port = advertised[0].port
        // Only the tailnet address: phones on the Mac's Wi-Fi aren't told of it.
        #expect(advertised.map(\.host) == ["100.81.122.65"])

        // A device that was never paired gets no answer.
        let stranger = Phone(port: port, identity: DeviceIdentity(), hostKey: identity.publicKey)
        try await eventually { stranger.closed }
        #expect(stranger.channel == nil)
        #expect(agent.peers.isEmpty)

        // The paired phone: handshake, then its first sealed message makes it a peer.
        let phone = Phone(port: port, identity: phoneIdentity, hostKey: identity.publicKey)
        try await eventually { phone.channel != nil }
        #expect(agent.peers.isEmpty)
        try phone.send(.clientHello(version: RemoteProtocol.version))
        try await eventually { phone.received.contains(.endpoints(advertised)) }
        #expect(phone.received.first == .hello(hostName: agent.hostName, version: RemoteProtocol.version))
        #expect(agent.peers[phoneID] != nil)
        try phone.send(.listSessions)
        try await eventually { phone.sessionLists == 1 }

        // Someone who only knows the phone's public key starts a handshake with it.
        // They can't seal a message, so the phone's session stays.
        var forged = Data([1])
        forged += Curve25519.KeyAgreement.PrivateKey().publicKey.rawRepresentation
        forged += phoneIdentity.publicKey
        let impostor = Phone(port: port, identity: DeviceIdentity(), hostKey: identity.publicKey, firstPacket: forged)
        try await eventually { impostor.packets == 1 }  // The Mac answered the handshake.
        impostor.connection.send(Data([3]) + Data(repeating: 0, count: 40))
        try await eventually { impostor.closed }
        #expect(agent.peers[phoneID] != nil)
        try phone.send(.listSessions)
        try await eventually { phone.sessionLists == 2 }
        #expect(!phone.closed)

        // The phone reconnects (another network, say): the new connection replaces the old.
        let again = Phone(port: port, identity: phoneIdentity, hostKey: identity.publicKey)
        try await eventually { again.channel != nil }
        try again.send(.listSessions)
        try await eventually { again.sessionLists == 1 }
        try await eventually { phone.closed }
        #expect(agent.peers.count == 1)

        // Hanging up frees the peer.
        again.connection.close()
        try await eventually { agent.peers.isEmpty }
    }

    /// The Mac makes an invitation, the phone claims it on a direct connection,
    /// the user approves, and the phone then connects as a paired one.
    @Test(.timeLimit(.minutes(1))) func aPhonePairsAndConnects() async throws {
        let executable = try #require(Multiplexer.bundledHostExecutable)
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(
            "termy-pairing-\(UUID().uuidString.prefix(8))")
        let mux = try Multiplexer.connect(root: root, hostExecutable: executable)
        defer {
            try? mux.shutdown()
            try? FileManager.default.removeItem(at: root)
        }
        let identity = DeviceIdentity()
        let hostID = UUID()
        let agent = RemoteHostAgent(multiplexer: mux, hostID: hostID, identity: identity, trustedDevices: [:])
        agent.directPort = nil
        agent.interfaces = { [NetworkInterface(name: "utun8", address: IPv4("100.81.122.65")!)] }
        var advertised: [DirectEndpoint] = []
        agent.onDirectEndpoints = { advertised = $0 }
        var requests: [PairedDevice?] = []
        agent.onPairingRequest = { requests.append($0) }
        // Before it listens there is nowhere for a phone to go, so no code.
        #expect(throws: PairingError.self) { try agent.beginPairing(hostName: "Studio") }
        let serving = Task { try await agent.run() }
        defer {
            serving.cancel()
            agent.stop()
        }
        try await eventually { advertised.count == 1 }
        let port = advertised[0].port

        // What the phone reads off the Mac's screen.
        let code = try PairingCode.parse(try agent.beginPairing(hostName: "Studio").text)
        #expect(code.hostID == hostID)
        #expect(code.endpoints == advertised)
        let phoneIdentity = DeviceIdentity()
        let phoneID = UUID()

        // Someone on the network who didn't see the code asks first.
        let stranger = DeviceIdentity()
        let other = try PairingCode(hostID: hostID, hostName: "Studio", hostKey: identity.publicKey, endpoints: advertised)
        let forged = try DirectPairing.packet(
            try other.claim(
                clientID: UUID(), name: "Mallory",
                initPacket: SecureChannel.startClient(identity: stranger, hostStaticKey: identity.publicKey).initPacket))
        let mallory = Phone(port: port, identity: stranger, hostKey: identity.publicKey, firstPacket: forged)
        try await eventually { mallory.closed }
        #expect(requests.isEmpty)

        let handshake = SecureChannel.startClient(identity: phoneIdentity, hostStaticKey: code.hostKey)
        let claim = try DirectPairing.packet(
            try code.claim(clientID: phoneID, name: "Test iPhone", initPacket: handshake.initPacket))
        let pairing = Phone(port: port, identity: phoneIdentity, hostKey: identity.publicKey, firstPacket: claim)
        try await eventually { requests.count == 1 }
        #expect(requests[0]?.name == "Test iPhone")
        #expect(agent.pairingClaimant?.id == phoneID)
        // Nothing is granted, and the phone isn't hung up on, while the user decides.
        #expect(agent.trustedDevices.isEmpty)
        #expect(pairing.raw.isEmpty)

        try agent.approvePairing()
        try await eventually { pairing.raw.count == 1 }
        let response = try #require(DirectPairing.handshakeResponse(in: pairing.raw[0]))
        _ = try handshake.finish(response)  // The Mac proved the key from the QR code.
        #expect(agent.trustedDevices == [phoneID: phoneIdentity.publicKey])
        pairing.connection.close()
        // The invitation is spent: the same claim again pairs nobody.
        let replay = Phone(port: port, identity: phoneIdentity, hostKey: identity.publicKey, firstPacket: claim)
        try await eventually { replay.closed }
        #expect(requests.count == 1)

        // Paired: a session over the direct connection.
        let phone = Phone(port: port, identity: phoneIdentity, hostKey: identity.publicKey)
        try await eventually { phone.channel != nil }
        try phone.send(.clientHello(version: RemoteProtocol.version))
        try phone.send(.listSessions)
        try await eventually { phone.sessionLists == 1 }
        #expect(agent.connectedDevices == [phoneID])
        try phone.send(.createSession(name: "Paired"))
        try await eventually { phone.received.contains { if case .created = $0 { true } else { false } } }
    }
}
