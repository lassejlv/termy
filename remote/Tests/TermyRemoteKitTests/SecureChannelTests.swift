import Foundation
import Testing
@testable import TermyRemoteKit

@Suite struct SecureChannelTests {
    let client = DeviceIdentity()
    let host = DeviceIdentity()

    func pair() throws -> (SecureChannel, SecureChannel) {
        let handshake = SecureChannel.startClient(identity: client, hostStaticKey: host.publicKey)
        #expect(try SecureChannel.clientStaticKey(inInit: handshake.initPacket) == client.publicKey)
        let (hostChannel, response) = try SecureChannel.respond(identity: host, initPacket: handshake.initPacket)
        return (try handshake.finish(response), hostChannel)
    }

    @Test func roundTripsBothWays() throws {
        var (c, h) = try pair()
        for i in 0 ..< 5 {
            let up = try c.seal(Data("key \(i)".utf8))
            #expect(try h.open(packet: up) == Data("key \(i)".utf8))
            let down = try h.seal(Data("frame \(i)".utf8))
            #expect(try c.open(packet: down) == Data("frame \(i)".utf8))
        }
    }

    @Test func rejectsReplay() throws {
        var (c, h) = try pair()
        let packet = try c.seal(Data("ls\n".utf8))
        _ = try h.open(packet: packet)
        #expect(throws: ChannelError.replayed) { try h.open(packet: packet) }
    }

    @Test func rejectsImpostorHost() throws {
        let impostor = DeviceIdentity()
        let handshake = SecureChannel.startClient(identity: client, hostStaticKey: host.publicKey)
        let (_, response) = try SecureChannel.respond(identity: impostor, initPacket: handshake.initPacket)
        #expect(throws: ChannelError.authenticationFailed) { try handshake.finish(response) }
    }

    @Test func impostorClientCannotFinish() throws {
        // Someone who knows the client's public key but not its private key
        // sends an init claiming to be it: without the client's private key
        // it can't derive the session keys from the host's response.
        let impostor = DeviceIdentity()
        let handshake = SecureChannel.startClient(identity: impostor, hostStaticKey: host.publicKey)
        var forged = handshake.initPacket
        forged.replaceSubrange(33 ..< 65, with: client.publicKey)
        let (_, response) = try SecureChannel.respond(identity: host, initPacket: forged)
        #expect(throws: ChannelError.authenticationFailed) { try handshake.finish(response) }
    }

    @Test func tamperedPacketFails() throws {
        var (c, h) = try pair()
        var packet = try c.seal(Data("hello".utf8))
        packet[packet.count - 1] ^= 1
        #expect(throws: (any Error).self) { try h.open(packet: packet) }
    }

    @Test func codecRoundTripsAndCompresses() throws {
        let runs = (0 ..< 80).map { CellRun(col: $0, cells: 1, text: "x", fg: 0xffffffff, bg: 0x000000ff, flags: 0) }
        let frame = RemoteFrame(
            stream: 1, cols: 80, rows: 24, full: true,
            updates: (0 ..< 24).map { RowUpdate(row: $0, runs: runs) },
            cursor: RemoteCursor(row: 0, col: 0, visible: true, style: "block"), background: 0
        )
        let data = try MessageCodec.encode(HostMessage.frame(frame))
        #expect(data.first == 1)
        #expect(try MessageCodec.decode(HostMessage.self, from: data) == .frame(frame))
    }

    @Test func wireRoundTrips() {
        let peer = UUID()
        let frame = RelayWire.encode(peer: peer, packet: Data([3, 4, 5]))
        let decoded = RelayWire.decode(frame)
        #expect(decoded?.peer == peer)
        #expect(decoded?.packet == Data([3, 4, 5]))
    }
}
