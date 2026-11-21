import Foundation
import TermyRemoteKit

/// A connection a phone opened to this Mac itself, before and after it proved who it is.
@MainActor
final class DirectClient {
    let connection: DirectConnection
    /// The paired phone it proved to be.
    var device: UUID?
    /// A handshake this Mac answered, waiting for the phone's first sealed message.
    var pending: (device: UUID, channel: SecureChannel)?
    /// It claimed a pairing invitation and waits for the user to decide.
    var pairing = false

    init(connection: DirectConnection) {
        self.connection = connection
    }
}

// Phones on this Mac's local network or tailnet connect to it here. Every packet
// is sealed and only keys pinned on this Mac are let in, so the listener trusts
// the network with nothing.
extension RemoteHostAgent {
    /// Unproven connections included. A handful of phones never needs more.
    static let maxDirectClients = 16
    /// How long a connection may take to prove it is a paired phone.
    static let directHandshakeSeconds = 10

    func serveDirect() async {
        var port = directPort
        // Advertising needs Local Network access on this Mac. Without it the listener
        // must still serve the tailnet and the addresses phones already know.
        var advertise = true
        while !Task.isCancelled, running {
            var ready = false
            var failed = false
            var announcing: Task<Void, Never>?
            do {
                // The name lets a phone find this Mac again when its address on the network changed.
                let listener = try DirectListener(port: port, advertisedAs: advertise ? hostID.uuidString : nil)
                self.listener = listener
                for await event in listener.events {
                    switch event {
                    case .ready(let bound):
                        ready = true
                        log("Listening for phones on port \(bound).")
                        announcing = Task { await self.announce(port: bound) }
                    case .connection(let connection):
                        adopt(connection)
                    case .closed(let error):
                        failed = error != nil
                        if let error { log("Stopped listening for phones: \(error.localizedDescription)") }
                    }
                }
            } catch {
                failed = true
                log("Couldn’t listen for phones: \(error.localizedDescription)")
            }
            announcing?.cancel()
            listener = nil
            setEndpoints([])
            guard running, !Task.isCancelled else { return }
            if failed, advertise {
                advertise = false
                continue
            }
            // Another program has the usual port: any free one works, phones are told which.
            if !ready, port != nil {
                port = nil
                continue
            }
            do { try await Task.sleep(for: .seconds(2)) } catch { return }
        }
    }

    /// Keeps phones told of this Mac's addresses, which change with the network it is on
    /// and with Tailscale starting and stopping.
    private func announce(port: Int) async {
        while !Task.isCancelled {
            setEndpoints(DirectEndpoint.advertised(by: interfaces(), port: port))
            do { try await Task.sleep(for: .seconds(5)) } catch { return }
        }
    }

    private func setEndpoints(_ new: [DirectEndpoint]) {
        guard new != endpoints else { return }
        endpoints = new
        onDirectEndpoints?(new)
        for peer in peers.values { peer.advertise(new) }
    }

    private func adopt(_ connection: DirectConnection) {
        guard running, directClients.count < Self.maxDirectClients else { return connection.close() }
        let client = DirectClient(connection: connection)
        directClients[ObjectIdentifier(connection)] = client
        Task { [weak self] in
            for await event in connection.events {
                guard let self else { break }
                if case .packet(let packet) = event { self.handle(packet, from: client) }
            }
            self?.dropped(client)
        }
        // A connection that never proves itself doesn't keep its place.
        Task { [weak client] in
            try? await Task.sleep(for: .seconds(Self.directHandshakeSeconds))
            if let client, client.device == nil, !client.pairing { client.connection.close() }
        }
    }

    // MARK: Pairing

    /// Makes an invitation for a QR code. It replaces the one before; a phone claims it on
    /// a direct connection, so this Mac needs an address a phone can reach.
    public func beginPairing(hostName: String) throws -> PairingCode {
        cancelPairing()
        let code = try PairingCode(hostID: hostID, hostName: hostName, hostKey: identity.publicKey, endpoints: endpoints)
        invitation = code
        return code
    }

    public func cancelPairing() {
        invitation = nil
        claimant?.client.connection.close()
        claimant = nil
    }

    /// The phone waiting for approval.
    public var pairingClaimant: PairedDevice? { claimant?.device }

    /// Pins the waiting phone and answers it with the proof of this Mac's key. The caller
    /// saves the pin first: a pin that couldn't be saved must not grant access.
    public func approvePairing() throws {
        guard let claimant, let invitation, invitation.expiresAt > Date(), let key = claimant.device.publicKeyData
        else {
            cancelPairing()
            throw PairingError.expired
        }
        let (_, response) = try SecureChannel.respond(identity: identity, initPacket: claimant.initPacket)
        trustedDevices[claimant.device.id] = key
        claimant.client.connection.send(DirectPairing.answer(response))
        // The phone hangs up and dials again as a paired one; this connection has done its part.
        claimant.client.pairing = false
        let connection = claimant.client.connection
        Task {
            try? await Task.sleep(for: .seconds(Self.directHandshakeSeconds))
            connection.close()
        }
        self.claimant = nil
        self.invitation = nil
    }

    private func claim(_ claim: DirectPairingClaim, from client: DirectClient) {
        // One phone per invitation, and never a pinned phone's ID under another key.
        guard let invitation, claimant == nil, let (device, initPacket) = try? invitation.verify(claim),
            let key = device.publicKeyData, trustedDevices[device.id].map({ $0 == key }) ?? true
        else {
            log("Refused a pairing request that didn’t come from the QR code on screen.")
            return client.connection.close()
        }
        client.pairing = true
        claimant = (client, device, initPacket)
        onPairingRequest?(device)
    }

    private func handle(_ packet: Data, from client: DirectClient) {
        if let device = client.device {
            guard let peer = peers[device], peer.connection === client.connection else { return }
            do {
                peer.handle(try MessageCodec.decode(ClientMessage.self, from: try peer.channel.open(packet: packet)))
            } catch {
                log("Dropped a packet from \(peer.name): \(error)")
            }
            return
        }
        do {
            if let pending = client.pending {
                // Anyone can send a paired phone's public key. Only that phone can seal a
                // message with the keys the handshake made, so its first one is the proof,
                // and only then does it replace the session that phone had.
                var channel = pending.channel
                let message = try MessageCodec.decode(ClientMessage.self, from: try channel.open(packet: packet))
                client.pending = nil
                client.device = pending.device
                peers.removeValue(forKey: pending.device)?.close()
                let peer = Peer(
                    id: pending.device, name: "iPhone", channel: channel, agent: self, connection: client.connection)
                peers[pending.device] = peer
                peer.send(.hello(hostName: hostName, version: RemoteProtocol.version))
                peer.handle(message)
                onPeersChanged?()
                log("\(peer.name) connected\(client.connection.remote.map { " from \($0.text)" } ?? "").")
                return
            }
            if let claim = DirectPairing.claim(in: packet) { return self.claim(claim, from: client) }
            guard !client.pairing else { return }
            let key = try SecureChannel.clientStaticKey(inInit: packet)
            guard let device = trustedDevices.first(where: { $0.value == key })?.key else {
                log("Refused a direct connection from a device that isn’t paired with this Mac.")
                return client.connection.close()
            }
            let (channel, response) = try SecureChannel.respond(identity: identity, initPacket: packet)
            client.pending = (device, channel)
            client.connection.send(response)
        } catch {
            client.connection.close()
        }
    }

    private func dropped(_ client: DirectClient) {
        directClients[ObjectIdentifier(client.connection)] = nil
        if claimant?.client === client {
            // The phone gave up before the user decided.
            claimant = nil
            onPairingRequest?(nil)
        }
        guard let device = client.device, let peer = peers[device], peer.connection === client.connection else { return }
        peers[device] = nil
        peer.close()
        onPeersChanged?()
        log("\(peer.name) disconnected.")
    }
}
