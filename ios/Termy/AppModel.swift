import Foundation
import Observation
import TermyRemoteKit
import UIKit

/// One device identity, with separate credentials and pairings for each relay.
@MainActor @Observable
final class AppModel {
    struct Profile: Codable {
        var credentials: DeviceCredentials
        var pins: [UUID: Data] = [:]
        var hosts: [RelayDevice] = []
    }
    enum RelayState { case offline, connecting, connected }
    let identity: DeviceIdentity
    private(set) var devices: [RelayDevice] = []
    private(set) var lastError: String?
    private(set) var pairingMessage: String?
    private(set) var pairingBusy = false
    @ObservationIgnored private let store = DeviceStore(service: "sh.termy.ios.pairing")
    @ObservationIgnored private var profiles: [String: Profile] = [:]
    @ObservationIgnored private var revisions: [String: UUID] = [:]
    @ObservationIgnored private var links: [UUID: HostLink] = [:]
    @ObservationIgnored private var connections: [String: RelayConnection] = [:]
    @ObservationIgnored private var loops: [String: Task<Void, Never>] = [:]
    private(set) var states: [String: RelayState] = [:]
    @ObservationIgnored private var active = true
    @ObservationIgnored private var identityError: Error?

    init() {
        do {
            identity = try store.identity()
        } catch {
            identity = DeviceIdentity()
            identityError = error
            lastError = error.localizedDescription
        }
        if identityError == nil {
            do { profiles = try store.load([String: Profile].self, key: "profiles") ?? [:] } catch {
                identityError = error
                lastError = error.localizedDescription
            }
        }
        rebuildDevices()
        resume()
    }
    var hosts: [RelayDevice] { devices }
    var relayState: RelayState {
        if states.values.contains(where: { $0 == .connecting }) { return .connecting }
        return states.values.contains(where: { $0 == .connected }) ? .connected : .offline
    }
    private func key(for host: UUID) -> String? { profiles.first(where: { $0.value.pins[host] != nil })?.key }
    func relayURL(for host: UUID) -> URL? { key(for: host).flatMap { profiles[$0]?.credentials.relayURL } }
    func isConnected(to host: UUID) -> Bool { key(for: host).map { states[$0] == .connected } ?? false }
    private func persist() throws { try store.save(profiles, key: "profiles") }
    private func rebuildDevices() {
        devices = profiles.values.flatMap(\.hosts).sorted {
            $0.name.localizedStandardCompare($1.name) == .orderedAscending
        }
    }
    func pair(_ code: PairingCode) async throws {
        guard !pairingBusy else { return }
        if let identityError { throw identityError }
        _ = try PairingCode.parse(code.text)
        pairingBusy = true
        pairingMessage = "Connecting to \(code.relayURL.host ?? "relay")…"
        defer {
            pairingBusy = false
            pairingMessage = nil
        }
        let relayKey = code.relayURL.absoluteString
        var profile: Profile
        if let existing = profiles[relayKey] {
            profile = existing
        } else {
            let credentials = try await RelayAPI(baseURL: code.relayURL).register(
                DeviceRegistration(name: UIDevice.current.name, kind: .client, publicKey: identity.publicKey))
            profile = Profile(credentials: credentials)
            profiles[relayKey] = profile
            try persist()
        }
        let handshake = SecureChannel.startClient(identity: identity, hostStaticKey: code.hostKey)
        try Task.checkCancellation()
        do {
            try await profile.credentials.api.claim(
                code.invitationID, initPacket: handshake.initPacket, proof: code.proof(for: handshake.initPacket))
        } catch let e as RelayAPIError where e.status == 401 {
            // An explicitly removed registration cannot be recovered; register a new device and re-pair.
            profiles[relayKey] = nil
            try persist()
            rebuildDevices()
            throw e
        }
        pairingMessage = "Approve this iPhone in Termy Next on \(code.hostName)."
        // Approval can occur at the expiry boundary; allow a brief retrieval window.
        while Date() < code.expiresAt.addingTimeInterval(20) {
            try Task.checkCancellation()
            let status = try await profile.credentials.api.pairing(code.invitationID)
            guard status.host.id == code.hostID, status.host.publicKeyData == code.hostKey,
                status.client?.id == profile.credentials.device.id, status.initPacket == handshake.initPacket
            else { throw PairingError.invalidProof }
            if let response = status.responsePacket {
                _ = try handshake.finish(response)
                try Task.checkCancellation()
                guard let latest = profiles[relayKey], latest.credentials.token == profile.credentials.token else {
                    throw PairingError.invalidProof
                }
                profile = latest
                profile.pins[code.hostID] = code.hostKey
                profile.hosts.removeAll { $0.id == code.hostID }
                var host = status.host
                host.online = false
                profile.hosts.append(host)
                profiles[relayKey] = profile
                revisions[relayKey] = UUID()
                try persist()
                rebuildDevices()
                restart(relayKey)
                return
            }
            guard Date() < code.expiresAt else { throw PairingError.expired }
            try await Task.sleep(for: .seconds(1))
        }
        throw PairingError.expired
    }
    func removeHost(_ host: RelayDevice) async throws {
        guard let k = key(for: host.id), var profile = profiles[k] else { return }
        do { try await profile.credentials.api.removeDevice(host.id) } catch let e as RelayAPIError
            where e.status == 401
        { /* Registration already revoked. */  }
        guard let latest = profiles[k], latest.credentials.token == profile.credentials.token else { return }
        profile = latest
        profile.pins[host.id] = nil
        profile.hosts.removeAll { $0.id == host.id }
        links.removeValue(forKey: host.id)?.disconnect()
        profiles[k] = profile
        revisions[k] = UUID()
        try persist()
        rebuildDevices()
        restart(k)
    }
    func refreshDevices() async {
        for k in Array(profiles.keys) { await refresh(k) }
    }
    private func refresh(_ k: String) async {
        guard let original = profiles[k] else { return }
        let revision = revisions[k]
        do {
            let peers = try await original.credentials.api.devices()
            guard !Task.isCancelled, revisions[k] == revision, var profile = profiles[k],
                profile.credentials.token == original.credentials.token
            else { return }
            // A removed link is never restored just because its old key is still pinned.
            let allowed = Set(peers.map(\.id))
            for id in profile.pins.keys where !allowed.contains(id) {
                profile.pins[id] = nil
                links[id]?.presenceChanged(online: false)
            }
            profile.hosts = peers.filter { $0.kind == .host && profile.pins[$0.id] != nil }
            profiles[k] = profile
            revisions[k] = UUID()
            try persist()
            rebuildDevices()
            lastError = nil
        } catch let e as RelayAPIError where e.status == 401 {
            guard revisions[k] == revision else { return }
            connections[k]?.close()
            profiles[k] = nil
            try? persist()
            rebuildDevices()
            lastError = "This device registration ended. Pair your Mac again."
        } catch { lastError = error.localizedDescription }
    }
    func link(for host: RelayDevice) -> HostLink {
        if let link = links[host.id] {
            link.device = host
            return link
        }
        let link = HostLink(device: host, model: self)
        links[host.id] = link
        return link
    }
    func send(_ packet: Data, to host: UUID) {
        guard let k = key(for: host) else { return }
        connections[k]?.send(packet, to: host)
    }
    func verdict(for host: RelayDevice) -> TrustStore.Verdict {
        guard let k = key(for: host.id), let pin = profiles[k]?.pins[host.id] else { return .unknown }
        return host.publicKeyData == pin ? .trusted : .changed
    }
    func resume() {
        active = true
        for k in profiles.keys where !profiles[k]!.pins.isEmpty && loops[k] == nil { start(k) }
    }
    func suspend() {
        active = false
        for k in Array(loops.keys) { stop(k) }
    }
    private func restart(_ k: String) {
        stop(k)
        if active, let profile = profiles[k], !profile.pins.isEmpty { start(k) }
    }
    private func start(_ k: String) { loops[k] = Task { [weak self] in await self?.runRelay(k) } }
    private func stop(_ k: String) {
        loops[k]?.cancel()
        loops[k] = nil
        connections.removeValue(forKey: k)?.close()
        states[k] = .offline
        for (id, link) in links where profiles[k]?.pins[id] != nil { link.relayWentAway() }
    }
    private func runRelay(_ k: String) async {
        var delay = 1
        while !Task.isCancelled, active {
            states[k] = .connecting
            await refresh(k)
            guard !Task.isCancelled, let profile = profiles[k], !profile.pins.isEmpty else { break }
            let connection = RelayConnection(api: profile.credentials.api)
            connections[k] = connection
            for await event in connection.events {
                guard !Task.isCancelled else { break }
                switch event {
                case .ready:
                    states[k] = .connected
                    delay = 1
                    for id in profile.pins.keys { links[id]?.relayReady() }
                case .packet(let from, let packet):
                    if profiles[k]?.pins[from] != nil { links[from]?.receive(packet) }
                case .presence(let id, let online):
                    if let i = profiles[k]?.hosts.firstIndex(where: { $0.id == id }) {
                        profiles[k]?.hosts[i].online = online
                        rebuildDevices()
                    }
                    links[id]?.presenceChanged(online: online)
                    if !online { await refresh(k) }
                case .undeliverable(let id): links[id]?.presenceChanged(online: false)
                case .closed: break
                }
            }
            // A superseded loop must not clobber its successor's connection state.
            guard !Task.isCancelled else { return }
            connections[k] = nil
            states[k] = .offline
            for id in profile.pins.keys { links[id]?.relayWentAway() }
            do { try await Task.sleep(for: .seconds(delay)) } catch { return }
            delay = min(delay * 2, 30)
        }
        if !Task.isCancelled {
            states[k] = .offline
            loops[k] = nil
        }
    }
}
