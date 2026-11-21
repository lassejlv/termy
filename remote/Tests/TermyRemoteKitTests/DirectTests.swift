import Foundation
import Testing

@testable import TermyRemoteKit

@Suite struct DirectTests {
    private func interface(_ name: String, _ address: String, mask: UInt32 = 0xFFFF_FF00) -> NetworkInterface {
        NetworkInterface(name: name, address: IPv4(address)!, mask: mask)
    }

    @Test func addressesAreDottedQuadsOnly() {
        #expect(IPv4("192.168.1.20")?.text == "192.168.1.20")
        for bad in ["", "1.2.3", "1.2.3.4.5", "256.1.1.1", "1..2.3", "mac.tailnet.ts.net", "1.2.3.4 ", "0x7f.0.0.1"] {
            #expect(IPv4(bad) == nil, "\(bad)")
        }
    }

    @Test func rangesMatchTailscaleAndPrivateNetworks() {
        #expect(IPv4("100.64.0.1")!.isTailnet)
        #expect(IPv4("100.127.255.254")!.isTailnet)
        #expect(!IPv4("100.63.255.255")!.isTailnet)
        #expect(!IPv4("100.128.0.0")!.isTailnet)
        for address in ["10.0.0.5", "172.16.0.1", "172.31.255.255", "192.168.0.9"] {
            #expect(IPv4(address)!.isPrivate, "\(address)")
        }
        for address in ["172.32.0.1", "192.169.0.1", "8.8.8.8", "100.64.0.1"] {
            #expect(!IPv4(address)!.isPrivate, "\(address)")
        }
    }

    @Test func aMacAdvertisesItsLocalAndTailnetAddresses() {
        let interfaces = [
            interface("lo0", "127.0.0.1", mask: 0xFF00_0000),
            interface("en0", "192.168.1.20"),
            interface("bridge100", "192.168.64.1"),
            interface("en5", "203.0.113.7"),
            interface("utun8", "100.81.122.65", mask: 0xFFFF_FFFF),
            interface("utun3", "10.8.0.2", mask: 0xFFFF_FFFF),
        ]
        #expect(
            DirectEndpoint.advertised(by: interfaces, port: 47126) == [
                DirectEndpoint(host: "192.168.1.20", port: 47126), DirectEndpoint(host: "100.81.122.65", port: 47126),
            ])
        #expect(DirectEndpoint.advertised(by: [interface("lo0", "127.0.0.1")], port: 1).isEmpty)
    }

    @Test func aPhoneDialsOnlyWhatItsNetworkReaches() {
        let local = DirectEndpoint(host: "192.168.1.20", port: 47126)
        let tailnet = DirectEndpoint(host: "100.81.122.65", port: 47126)
        let all = [tailnet, local]
        let wifi = interface("en0", "192.168.1.77")
        let tailscale = interface("utun4", "100.90.1.2", mask: 0xFFFF_FFFF)
        // The same network is nearer than the tailnet.
        #expect(DirectEndpoint.reachable(all, from: [wifi, tailscale]) == [local, tailnet])
        #expect(DirectEndpoint.reachable(all, from: [wifi]) == [local])
        // Another network with the same kind of addresses, and mobile data.
        #expect(DirectEndpoint.reachable(all, from: [interface("en0", "192.168.2.77")]).isEmpty)
        #expect(DirectEndpoint.reachable(all, from: [interface("pdp_ip0", "10.20.30.40"), tailscale]) == [tailnet])
    }

    @Test func aPhoneNeverDialsNamesPublicAddressesOrBadPorts() {
        let everywhere = [
            NetworkInterface(name: "en0", address: IPv4("192.168.1.77")!, mask: 0),
            NetworkInterface(name: "en1", address: IPv4("8.8.4.4")!, mask: 0xFF00_0000),
            interface("utun4", "100.90.1.2"),
        ]
        let bad = [
            DirectEndpoint(host: "evil.example.com", port: 443), DirectEndpoint(host: "8.8.8.8", port: 53),
            DirectEndpoint(host: "192.168.1.20", port: 0), DirectEndpoint(host: "100.64.0.1", port: 70000),
        ]
        #expect(DirectEndpoint.reachable(bad, from: everywhere).isEmpty)
        #expect(bad.allSatisfy { DirectConnection(to: $0) == nil })
        // No more than a Mac advertises, however many arrive.
        let many = (1...20).map { DirectEndpoint(host: "100.64.0.\($0)", port: 47126) }
        #expect(DirectEndpoint.reachable(many, from: everywhere).count == DirectEndpoint.maxCount)
    }

    @Test func readerCutsTheStreamIntoPackets() throws {
        let packets = [Data("a".utf8), Data(repeating: 7, count: 70_000), Data("end".utf8)]
        let stream = packets.map(DirectWire.frame).reduce(Data(), +)
        // Whole, byte by byte at the start, and in uneven pieces.
        for step in [stream.count, 1, 3, 4096] {
            var reader = DirectWire.Reader()
            var read: [Data] = []
            var offset = 0
            while offset < stream.count {
                let size = offset < 16 ? min(step, 1) : step
                read += try reader.append(stream[offset..<min(stream.count, offset + max(size, 1))])
                offset += max(size, 1)
            }
            #expect(read == packets, "step \(step)")
        }
    }

    @Test func readerRefusesLengthsNoSenderUses() {
        var empty = DirectWire.Reader()
        #expect(throws: ChannelError.malformed) { try empty.append(Data([0, 0, 0, 0])) }
        var huge = DirectWire.Reader()
        #expect(throws: ChannelError.malformed) { try huge.append(Data([0xFF, 0xFF, 0xFF, 0xFF, 1])) }
    }

    @Test func endpointsTravelInsideTheChannel() throws {
        let message = HostMessage.endpoints([DirectEndpoint(host: "100.81.122.65", port: 47126)])
        #expect(try MessageCodec.decode(HostMessage.self, from: try MessageCodec.encode(message)) == message)
    }

    @Test(.timeLimit(.minutes(1))) func packetsCrossARealConnectionBothWays() async throws {
        let listener = try DirectListener(port: nil)
        defer { listener.close() }
        var events = listener.events.makeAsyncIterator()
        guard case .ready(let port) = await events.next() else {
            Issue.record("The listener didn't start.")
            return
        }
        #expect(port > 0)

        // Loopback isn't an address a phone dials, so the test builds its connection itself.
        let client = DirectConnection(
            .init(host: "127.0.0.1", port: .init(rawValue: UInt16(port))!, using: DirectConnection.parameters()),
            remote: IPv4("127.0.0.1"), timeout: 3)
        defer { client.close() }
        var fromServer = client.events.makeAsyncIterator()
        guard case .ready = await fromServer.next() else {
            Issue.record("The client didn't connect.")
            return
        }
        guard case .connection(let server) = await events.next() else {
            Issue.record("The listener didn't hand over the connection.")
            return
        }
        #expect(server.remote?.isLoopback == true)
        var fromClient = server.events.makeAsyncIterator()
        guard case .ready = await fromClient.next() else {
            Issue.record("The accepted connection didn't start.")
            return
        }

        let large = Data((0..<300_000).map { UInt8(truncatingIfNeeded: $0) })
        client.send(Data("hello".utf8))
        client.send(large)
        guard case .packet(let first) = await fromClient.next(), case .packet(let second) = await fromClient.next()
        else {
            Issue.record("The packets didn't arrive.")
            return
        }
        #expect(first == Data("hello".utf8))
        #expect(second == large)

        server.send(Data("frame".utf8))
        guard case .packet(let reply) = await fromServer.next() else {
            Issue.record("The reply didn't arrive.")
            return
        }
        #expect(reply == Data("frame".utf8))

        server.close()
        guard case .closed = await fromServer.next() else {
            Issue.record("The client didn't see the close.")
            return
        }
    }

    @Test(.timeLimit(.minutes(1))) func aClosedPortFailsInsteadOfWaiting() async throws {
        // Nothing listens on the discard port.
        let client = DirectConnection(
            .init(host: "127.0.0.1", port: 9, using: DirectConnection.parameters()), remote: nil, timeout: 2)
        var seen = client.events.makeAsyncIterator()
        guard case .closed(let error) = await seen.next() else {
            Issue.record("A connection to a closed port must fail.")
            return
        }
        #expect(error != nil)
    }
}

@Suite struct DirectPairingTests {
    let host = DeviceIdentity()
    let phone = DeviceIdentity()
    let endpoints = [DirectEndpoint(host: "192.168.1.20", port: 47126), DirectEndpoint(host: "100.81.122.65", port: 47126)]

    func code() throws -> PairingCode {
        try PairingCode.parse(
            try PairingCode(hostID: UUID(), hostName: "Studio", hostKey: host.publicKey, endpoints: endpoints).text)
    }

    @Test func aCodeCarriesTheMacsAddresses() throws {
        let code = try code()
        #expect(code.version == 5)
        #expect(code.endpoints == endpoints)
        #expect(try code.text.utf8.count < 900, "It has to fit a QR code a phone reads off a screen.")
        // Nowhere to reach the Mac, or only somewhere a phone must not dial: no code.
        #expect(throws: PairingError.self) {
            try PairingCode(hostID: UUID(), hostName: "Studio", hostKey: host.publicKey, endpoints: [])
        }
        #expect(throws: PairingError.self) {
            try PairingCode(
                hostID: UUID(), hostName: "Studio", hostKey: host.publicKey,
                endpoints: [DirectEndpoint(host: "evil.example.com", port: 443)])
        }
    }

    @Test func thePhoneClaimsAndBothEndsPinEachOther() throws {
        let code = try code()
        let clientID = UUID()
        let handshake = SecureChannel.startClient(identity: phone, hostStaticKey: code.hostKey)
        let packet = try DirectPairing.packet(
            try code.claim(clientID: clientID, name: "Lasse’s iPhone", initPacket: handshake.initPacket))
        // On the Mac.
        let claim = try #require(DirectPairing.claim(in: packet))
        let verified = try code.verify(claim)
        #expect(verified.client.id == clientID)
        #expect(verified.client.name == "Lasse’s iPhone")
        #expect(verified.client.publicKeyData == phone.publicKey)
        let (_, response) = try SecureChannel.respond(identity: host, initPacket: verified.initPacket)
        // On the phone: only the Mac whose key the QR code showed can have answered.
        let answer = try #require(DirectPairing.handshakeResponse(in: DirectPairing.answer(response)))
        _ = try handshake.finish(answer)
    }

    @Test func aClaimWithoutTheQRSecretOrChangedOnTheWayIsRefused() throws {
        let code = try code()
        let handshake = SecureChannel.startClient(identity: phone, hostStaticKey: code.hostKey)
        let claim = try code.claim(clientID: UUID(), name: "iPhone", initPacket: handshake.initPacket)

        // Someone on the network who saw the claim swaps in their own key, ID or name.
        var otherKey = claim
        otherKey.initPacket = SecureChannel.startClient(identity: DeviceIdentity(), hostStaticKey: code.hostKey).initPacket
        var otherID = claim
        otherID.clientID = UUID()
        var otherName = claim
        otherName.encryptedClientName = try code.sealClientName("Mallory", initPacket: handshake.initPacket)
        for forged in [otherKey, otherID, otherName] {
            #expect(throws: PairingError.self) { try code.verify(forged) }
        }
        // A claim made for another invitation of the same Mac.
        let other = try self.code()
        #expect(throws: PairingError.self) {
            try code.verify(try other.claim(clientID: UUID(), name: "iPhone", initPacket: handshake.initPacket))
        }
        #expect(throws: PairingError.self) { try code.verify(claim, now: Date().addingTimeInterval(121)) }
        // Other packets aren't claims.
        #expect(DirectPairing.claim(in: handshake.initPacket) == nil)
        #expect(DirectPairing.claim(in: Data([4]) + Data(repeating: 0x7B, count: 5000)) == nil)
    }

    @Test(.timeLimit(.minutes(1))) func aMacIsFoundByTheNameItAdvertises() async throws {
        let name = UUID().uuidString
        let listener = try DirectListener(port: nil, advertisedAs: name)
        defer { listener.close() }
        var events = listener.events.makeAsyncIterator()
        guard case .ready = await events.next() else {
            Issue.record("The listener didn't start.")
            return
        }
        // Bonjour answers on this Mac alone when it is on no network; give it a few tries.
        for _ in 0..<5 {
            let client = DirectConnection(advertised: name, timeout: 3)
            var seen = client.events.makeAsyncIterator()
            if case .ready = await seen.next() {
                #expect(client.reached?.port != nil)
                client.close()
                return
            }
        }
        Issue.record("The advertised name didn't lead to the listener.")
    }
}
