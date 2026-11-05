import Foundation
import Termy
import TermyRemoteKit
import Testing

@testable import TermyRemoteHost

/// Opt-in test against a local relay backed by an isolated database.
@Suite("Remote terminal", .serialized)
@MainActor struct RemoteHostTests {
    @Test(.enabled(if: ProcessInfo.processInfo.environment["RELAY_TEST_URL"] != nil), .timeLimit(.minutes(1)))
    func pairsStreamsResizesAndRevokes() async throws {
        let url = try #require(ProcessInfo.processInfo.environment["RELAY_TEST_URL"].flatMap(URL.init(string:)))
        let hostIdentity = DeviceIdentity()
        let phoneIdentity = DeviceIdentity()
        let host = try await RelayAPI(baseURL: url).register(
            DeviceRegistration(name: "Test Mac", kind: .host, publicKey: hostIdentity.publicKey))
        let phone = try await RelayAPI(baseURL: url).register(
            DeviceRegistration(name: "Test iPhone", kind: .client, publicKey: phoneIdentity.publicKey))
        let invitation = try await host.api.createPairing()
        let qr = try PairingCode.parse(PairingCode(status: invitation, relayURL: url).text)
        let initHandshake = SecureChannel.startClient(identity: phoneIdentity, hostStaticKey: qr.hostKey)
        try await phone.api.claim(
            qr.invitationID, initPacket: initHandshake.initPacket, proof: qr.proof(for: initHandshake.initPacket))
        let pending = try await host.api.pairing(qr.invitationID)
        let verified = try qr.verify(pending)
        #expect(try await host.api.devices().isEmpty)
        let (_, response) = try SecureChannel.respond(identity: hostIdentity, initPacket: verified.initPacket)
        try await host.api.approve(
            qr.invitationID, client: verified.client.id, initPacket: verified.initPacket, response: response)
        _ = try initHandshake.finish(#require(try await phone.api.pairing(qr.invitationID).responsePacket))

        let root = FileManager.default.temporaryDirectory.appendingPathComponent("termy-remote-test-\(UUID())")
        let mux = try Multiplexer.connect(root: root, hostExecutable: #require(Multiplexer.bundledHostExecutable))
        defer {
            try? mux.shutdown()
            try? FileManager.default.removeItem(at: root)
        }
        let agent = RemoteHostAgent(
            multiplexer: mux, credentials: host, identity: hostIdentity,
            trustedDevices: [phone.device.id: phoneIdentity.publicKey])
        let serving = Task { try await agent.run() }
        defer {
            serving.cancel()
            agent.stop()
        }
        let client = TestPhone(credentials: phone, host: host.device, identity: phoneIdentity)
        client.start()
        defer { client.stop() }
        try await eventually { client.channel != nil }
        try client.send(.createPane)
        try await eventually { client.createdPane != nil }
        let paneID = try #require(client.createdPane)
        try client.send(.attach(stream: 1, paneID: paneID))
        try await eventually { !client.frames.isEmpty }
        // Octal output ensures this marker comes from the program, not shell input echo.
        try client.send(
            .paste(stream: 1, text: "printf '\\124\\105\\122\\115\\131\\137\\120\\101\\111\\122\\105\\104\\n'\n"))
        try await eventually {
            client.frames.contains { $0.updates.contains { $0.runs.map(\.text).joined().contains("TERMY_PAIRED") } }
        }
        try client.send(.resize(stream: 1, cols: 42, rows: 12))
        try await eventually { client.frames.contains { $0.cols == 42 && $0.rows == 12 && $0.full } }
        try client.send(.detach(stream: 1))
        try await eventually { (try? mux.attachTerminal(paneID: paneID).snapshot().cols) == 80 }
        try await host.api.removeDevice(phone.device.id)
        try await eventually { client.revoked }
        #expect(try await phone.api.devices().isEmpty)
        try await phone.api.deleteRegistration()
        try await host.api.deleteRegistration()
    }
    private func eventually(_ predicate: () throws -> Bool) async throws {
        let deadline = Date().addingTimeInterval(12)
        while Date() < deadline {
            if try predicate() { return }
            try await Task.sleep(for: .milliseconds(30))
        }
        throw WaitError.timedOut
    }
    private enum WaitError: Error { case timedOut }
}
@MainActor private final class TestPhone {
    let credentials: DeviceCredentials
    let host: RelayDevice
    let identity: DeviceIdentity
    var channel: SecureChannel?
    var frames: [RemoteFrame] = []
    var createdPane: String?
    var revoked = false
    private var handshake: SecureChannel.ClientHandshake?
    private var connection: RelayConnection?
    private var task: Task<Void, Never>?
    init(credentials: DeviceCredentials, host: RelayDevice, identity: DeviceIdentity) {
        self.credentials = credentials
        self.host = host
        self.identity = identity
    }
    func start() {
        let connection = RelayConnection(api: credentials.api)
        self.connection = connection
        task = Task {
            for await event in connection.events {
                do {
                    switch event {
                    case .ready: connect()
                    case .presence(let id, let online) where id == host.id:
                        if online {
                            connect()
                        } else {
                            revoked = channel != nil
                            channel = nil
                            handshake = nil
                        }
                    case .undeliverable: handshake = nil
                    case .packet(let id, let packet) where id == host.id:
                        if let handshake {
                            channel = try handshake.finish(packet)
                            self.handshake = nil
                        } else if var c = channel {
                            let message = try MessageCodec.decode(HostMessage.self, from: c.open(packet: packet))
                            channel = c
                            switch message {
                            case .created(let id): createdPane = id
                            case .frame(let f): frames.append(f)
                            default: break
                            }
                        }
                    default: break
                    }
                } catch { Issue.record(error) }
            }
        }
    }
    private func connect() {
        guard channel == nil, handshake == nil, let key = host.publicKeyData else { return }
        let h = SecureChannel.startClient(identity: identity, hostStaticKey: key)
        handshake = h
        connection?.send(h.initPacket, to: host.id)
    }
    func send(_ message: ClientMessage) throws {
        guard var c = channel else { throw ChannelError.unexpected }
        let packet = try c.seal(MessageCodec.encode(message))
        channel = c
        connection?.send(packet, to: host.id)
    }
    func stop() {
        task?.cancel()
        connection?.close()
    }
}
