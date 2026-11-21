import Foundation
import Network
import Observation
import TermyRemoteKit
import UIKit
import WidgetKit

/// Somewhere the app was asked to go: a computer, a pane, or a command to run.
struct Route: Equatable, Identifiable {
    let id = UUID()
    var host: UUID
    var pane: String?
    /// Run in a new tab of `sessionName` (made if missing). Only Shortcuts set it.
    var command: String?
    var sessionName = "Shortcuts"
}

/// Pairing a Mac that uses no relay, on a connection of the phone's own.
enum DirectPairingError: LocalizedError {
    case unreachable(String), refused(String)
    var errorDescription: String? {
        switch self {
        case .unreachable(let name):
            "This iPhone can’t reach \(name). It uses no relay: join the same Wi-Fi network, or turn on Tailscale on both devices, then scan again."
        case .refused(let name): "\(name) didn’t accept the pairing. Generate a new QR code on it and scan again."
        }
    }
}

/// One device identity, with separate credentials and pairings for each relay, and one
/// more set for the Macs paired without any.
@MainActor @Observable
final class AppModel: HostTransport {
    /// One model per process, so App Intents and the widget's links reach the UI's.
    static let shared = AppModel()
    struct Profile: Codable {
        var credentials: DeviceCredentials
        var pins: [UUID: Data] = [:]
        var hosts: [RelayDevice] = []
        var configurations: [UUID: RemoteConfiguration]?
        /// Where each Mac said it can be reached without the relay.
        var endpoints: [UUID: [DirectEndpoint]]?
    }
    /// The phone's own connection to one Mac, open or being opened.
    private struct Direct {
        var connection: DirectConnection?
        var task: Task<Void, Never>
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
    @ObservationIgnored private var directs: [UUID: Direct] = [:]
    /// The Macs reached directly, and over what.
    private(set) var directKinds: [UUID: DirectEndpoint.Kind] = [:]
    @ObservationIgnored private let pathMonitor = NWPathMonitor()
    /// Connections kept for a while after the app left the screen (see `background`).
    @ObservationIgnored private var lingering: Task<Void, Never>?
    @ObservationIgnored private var backgroundTask = UIBackgroundTaskIdentifier.invalid
    @ObservationIgnored private var backgroundedAt: Date?
    static let lingerSeconds: TimeInterval = 20
    @ObservationIgnored private var active = true
    @ObservationIgnored private var identityError: Error?
    /// Where the UI should go next; the views clear it once they got there.
    var route: Route?
    /// The pairing sheet should open, e.g. after removing a computer to pair it again.
    var pairingRequested = false
    @ObservationIgnored private var publishedSnapshot: HostSnapshot?

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
        // Joining the Mac's Wi-Fi, or Tailscale connecting, opens a direct path.
        pathMonitor.pathUpdateHandler = { [weak self] _ in Task { @MainActor in self?.dialAll() } }
        pathMonitor.start(queue: .main)
    }
    var hosts: [RelayDevice] { devices }
    var relayState: RelayState {
        if states.values.contains(where: { $0 == .connecting }) { return .connecting }
        return states.values.contains(where: { $0 == .connected }) ? .connected : .offline
    }
    /// The profile of the Macs paired without a relay.
    private static let directKey = "direct"
    private func key(for host: UUID) -> String? { profiles.first(where: { $0.value.pins[host] != nil })?.key }
    /// Nil for a Mac paired without a relay.
    func relayURL(for host: UUID) -> URL? {
        key(for: host).flatMap { profiles[$0]?.credentials }.flatMap { $0.isRelayless ? nil : $0.relayURL }
    }
    func isRelayless(_ host: UUID) -> Bool { key(for: host) == Self.directKey }
    /// Whether any paired Mac is reached through a relay.
    var usesRelay: Bool { devices.contains { !isRelayless($0.id) } }
    private func relays(_ k: String) -> Bool { profiles[k].map { !$0.credentials.isRelayless } ?? false }
    func path(to host: UUID) -> HostPath? {
        if directKinds[host] != nil { return .direct }
        return key(for: host).map { states[$0] == .connected } == true ? .relay : nil
    }
    private func persist() throws { try store.save(profiles, key: "profiles") }
    private func rebuildDevices() {
        devices = profiles.values.flatMap(\.hosts).sorted {
            $0.name.localizedStandardCompare($1.name) == .orderedAscending
        }
        publishDevices()
    }

    // MARK: Routes, widget and quick actions

    func open(_ destination: TermyLink.Destination) {
        switch destination {
        case .pair:
            route = nil
            pairingRequested = true
        case .host(let id, let pane):
            guard devices.contains(where: { $0.id == id }) else { return }
            route = Route(host: id, pane: pane)
        }
    }

    /// A pane Termy Next on a Mac handed off: open it, when that Mac is paired.
    func `continue`(_ activity: NSUserActivity) {
        guard let target = RemoteHandoff.parse(activity.userInfo) else { return }
        open(.host(target.host, pane: target.pane))
    }

    func open(_ url: URL) {
        if let destination = TermyLink.parse(url) { open(destination) }
    }

    /// Removes a computer whose key no longer matches, then opens pairing.
    func repair(_ host: RelayDevice) async throws {
        try await removeHost(host)
        route = nil
        pairingRequested = true
    }

    func sessionsChanged(for host: UUID) { publishDevices() }

    /// Shares the computer list with the widget, the Home Screen quick actions and Shortcuts.
    private func publishDevices() {
        let hosts = devices.map { device in
            let link = links[device.id]
            let connected = link?.state == .connected
            let panes = connected ? link?.orderedPanes : nil
            return HostSnapshot.Host(
                id: device.id, name: device.name, online: device.online || directKinds[device.id] != nil,
                panes: panes?.count,
                running: panes?.filter { $0.command?.running == true }.count)
        }
        let previous = publishedSnapshot ?? HostSnapshotStore.load()
        let stale = Date().timeIntervalSince(previous.updatedAt) > 300
        guard previous.hosts != hosts || stale else { return }
        let snapshot = HostSnapshot(hosts: hosts, updatedAt: Date())
        publishedSnapshot = snapshot
        if HostSnapshotStore.save(snapshot) { WidgetCenter.shared.reloadAllTimelines() }
        if previous.hosts.map(\.id) != hosts.map(\.id) || previous.hosts.map(\.name) != hosts.map(\.name) {
            TermyShortcuts.updateAppShortcutParameters()
        }
        var items = devices.prefix(3).map { device in
            UIApplicationShortcutItem(
                type: "sh.termy.open-host", localizedTitle: device.name,
                localizedSubtitle: nil, icon: UIApplicationShortcutIcon(systemImageName: "desktopcomputer"),
                userInfo: ["url": TermyLink.open(host: device.id).absoluteString as NSString])
        }
        items.append(
            UIApplicationShortcutItem(
                type: "sh.termy.pair", localizedTitle: "Pair Computer", localizedSubtitle: nil,
                icon: UIApplicationShortcutIcon(systemImageName: "qrcode.viewfinder"),
                userInfo: ["url": TermyLink.pair.absoluteString as NSString]))
        UIApplication.shared.shortcutItems = items
    }
    func pair(_ code: PairingCode) async throws {
        guard !pairingBusy else { return }
        if let identityError { throw identityError }
        _ = try PairingCode.parse(code.text)
        pairingBusy = true
        defer {
            pairingBusy = false
            pairingMessage = nil
        }
        guard let relayURL = code.relayURL else { return try await pairDirect(code) }
        pairingMessage = "Connecting to \(relayURL.host ?? "relay")…"
        let relayKey = relayURL.absoluteString
        var profile: Profile
        if let existing = profiles[relayKey] {
            profile = existing
        } else {
            let credentials = try await RelayAPI(baseURL: relayURL).register(
                try DeviceRegistration(name: UIDevice.current.name, kind: .client, identity: identity))
            profile = Profile(credentials: credentials)
            profiles[relayKey] = profile
            try persist()
        }
        let handshake = SecureChannel.startClient(identity: identity, hostStaticKey: code.hostKey)
        let encryptedName = try code.sealClientName(
            UIDevice.current.name, initPacket: handshake.initPacket)
        try Task.checkCancellation()
        do {
            try await profile.credentials.api.updateName(UIDevice.current.name, identity: identity)
            try await profile.credentials.api.claim(
                code.invitationID, initPacket: handshake.initPacket,
                proof: code.proof(for: handshake.initPacket, encryptedClientName: encryptedName),
                encryptedClientName: encryptedName)
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
                host.name = code.hostName
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
    /// Pairs a Mac that uses no relay: claims its invitation on a connection to one of the
    /// addresses in the QR code and waits there for the Mac's user to approve.
    private func pairDirect(_ code: PairingCode) async throws {
        pairingMessage = "Looking for \(code.hostName)…"
        // This phone's ID among the Macs it pairs directly, where a relay would have assigned one.
        let own =
            profiles[Self.directKey]?.credentials
            ?? .relayless(
                device: RelayDevice(
                    id: UUID(), name: UIDevice.current.name, kind: .client, publicKey: identity.publicKey))
        let handshake = SecureChannel.startClient(identity: identity, hostStaticKey: code.hostKey)
        let claim = try DirectPairing.packet(
            try code.claim(clientID: own.device.id, name: UIDevice.current.name, initPacket: handshake.initPacket))
        for endpoint in DirectEndpoint.reachable(code.endpoints ?? [], from: NetworkInterface.current()) {
            try Task.checkCancellation()
            guard let connection = DirectConnection(to: endpoint) else { continue }
            defer { connection.close() }
            var reached = false
            for await event in connection.events {
                switch event {
                case .ready:
                    reached = true
                    connection.send(claim)
                    pairingMessage = "Approve this iPhone in Termy Next on \(code.hostName)."
                case .packet(let packet):
                    // The answer proves the Mac holds the key the QR code showed.
                    guard let response = DirectPairing.handshakeResponse(in: packet) else {
                        throw PairingError.invalidProof
                    }
                    _ = try handshake.finish(response)
                    var profile = profiles[Self.directKey] ?? Profile(credentials: own)
                    profile.pins[code.hostID] = code.hostKey
                    profile.hosts.removeAll { $0.id == code.hostID }
                    profile.hosts.append(
                        RelayDevice(id: code.hostID, name: code.hostName, kind: .host, publicKey: code.hostKey))
                    if profile.endpoints == nil { profile.endpoints = [:] }
                    profile.endpoints?[code.hostID] = code.endpoints
                    profiles[Self.directKey] = profile
                    try persist()
                    rebuildDevices()
                    return
                case .closed: break
                }
            }
            try Task.checkCancellation()
            // It answered and hung up: declined, expired, or not the code on its screen.
            if reached { throw DirectPairingError.refused(code.hostName) }
        }
        throw DirectPairingError.unreachable(code.hostName)
    }
    func removeHost(_ host: RelayDevice) async throws {
        guard let k = key(for: host.id), var profile = profiles[k] else { return }
        // A Mac paired without a relay keeps this phone's key until it is removed there too.
        if relays(k) {
            do { try await profile.credentials.api.removeDevice(host.id) } catch let e as RelayAPIError
                where e.status == 401
            { /* Registration already revoked. */  }
        }
        guard let latest = profiles[k], latest.credentials.token == profile.credentials.token else { return }
        profile = latest
        profile.pins[host.id] = nil
        profile.configurations?[host.id] = nil
        profile.endpoints?[host.id] = nil
        profile.hosts.removeAll { $0.id == host.id }
        hangUp(host.id)
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
        guard relays(k), let original = profiles[k] else { return }
        let revision = revisions[k]
        do {
            try await original.credentials.api.updateName(UIDevice.current.name, identity: identity)
            let peers = try await original.credentials.api.devices()
            guard !Task.isCancelled, revisions[k] == revision, var profile = profiles[k],
                profile.credentials.token == original.credentials.token
            else { return }
            // A removed link is never restored just because its old key is still pinned.
            let allowed = Set(peers.map(\.id))
            for id in profile.pins.keys where !allowed.contains(id) {
                profile.pins[id] = nil
                profile.endpoints?[id] = nil
                hangUp(id)
                links[id]?.presenceChanged(online: false)
            }
            let names = Dictionary(uniqueKeysWithValues: profile.hosts.map { ($0.id, $0) })
            profile.hosts = peers.filter { $0.kind == .host && profile.pins[$0.id] != nil }.map { peer in
                peer.withLocalName(from: names[peer.id])
            }
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
    func configuration(for host: UUID) -> RemoteConfiguration? {
        key(for: host).flatMap { profiles[$0]?.configurations?[host] }
    }
    func saveConfiguration(_ configuration: RemoteConfiguration, for host: UUID) {
        guard let key = key(for: host) else { return }
        if profiles[key]?.configurations == nil { profiles[key]?.configurations = [:] }
        profiles[key]?.configurations?[host] = configuration
        do { try persist() } catch { lastError = "Couldn’t securely save the Mac’s config." }
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
    func send(_ packet: Data, to host: UUID, via path: HostPath) {
        switch path {
        case .direct: directs[host]?.connection?.send(packet)
        case .relay:
            guard let k = key(for: host) else { return }
            connections[k]?.send(packet, to: host)
        }
    }

    // MARK: Direct connections

    /// A Mac said where it listens. Nothing but the Mac itself, inside the encrypted
    /// channel, can say so; the list is kept with its pin.
    func endpointsChanged(_ endpoints: [DirectEndpoint], for host: UUID) {
        guard let k = key(for: host) else { return }
        let kept = Array(endpoints.filter { $0.kind != nil }.prefix(DirectEndpoint.maxCount))
        if profiles[k]?.endpoints?[host] ?? [] != kept {
            if profiles[k]?.endpoints == nil { profiles[k]?.endpoints = [:] }
            profiles[k]?.endpoints?[host] = kept.isEmpty ? nil : kept
            try? persist()
        }
        dial(host)
    }

    func directFailed(for host: UUID) {
        hangUp(host)
        links[host]?.directWentAway()
    }

    private func dialAll() {
        for profile in profiles.values { for host in profile.pins.keys { dial(host) } }
    }

    /// Opens a connection of the phone's own to a Mac a view has open: every address this
    /// phone's network reaches is dialed at once and the first to answer is used, so one the
    /// Mac no longer has costs nothing. Without one, the relay carries the link as before.
    /// A failed try is repeated soon: a reopened app dials before its network is back.
    func dial(_ host: UUID) {
        guard active, directs[host] == nil, links[host]?.wanted == true, let k = key(for: host),
            profiles[k]?.pins[host] != nil
        else { return }
        let relayless = !relays(k)
        let task = Task { [weak self] in
            guard let self else { return }
            var failures = 0
            while !Task.isCancelled, self.active {
                let started = Date()
                let candidates = self.candidates(for: host)
                let winner = await Self.firstReady(candidates.map(\.connection))
                guard !Task.isCancelled else {
                    winner?.close()
                    return
                }
                if let winner, let candidate = candidates.first(where: { $0.connection === winner }) {
                    self.directs[host]?.connection = winner
                    if candidate.advertised, let found = winner.reached { self.found(found, for: host) }
                    self.directKinds[host] = candidate.kind
                    self.links[host]?.directReady()
                    for await event in winner.events {
                        if case .packet(let packet) = event { self.links[host]?.receive(packet, via: .direct) }
                    }
                    winner.close()
                    guard !Task.isCancelled else { return }
                    self.directs[host]?.connection = nil
                    self.directKinds[host] = nil
                    self.links[host]?.directWentAway()
                    // One that lasted was a working path, and the Mac may be right back (it
                    // restarts its listener with its settings). One that didn't counts as a miss.
                    failures = Date().timeIntervalSince(started) > 10 ? 0 : failures + 1
                    if failures == 0 { continue }
                } else {
                    failures += 1
                }
                // No relay says when such a Mac is back: asking again is the only way to
                // know. Through a relay, a few tries cover the network coming back.
                if relayless, failures >= 2 { self.links[host]?.unreachable() }
                guard relayless || failures <= 4 else { break }
                let pause = min(500 << min(failures - 1, 5), relayless ? 8000 : 4000)
                try? await Task.sleep(for: .milliseconds(pause))
            }
            if !Task.isCancelled { self.directs[host] = nil }
        }
        directs[host] = Direct(connection: nil, task: task)
    }

    /// A connection to each address of a Mac this phone's network reaches right now.
    private func candidates(for host: UUID) -> [(connection: DirectConnection, kind: DirectEndpoint.Kind, advertised: Bool)] {
        guard let k = key(for: host) else { return [] }
        let interfaces = NetworkInterface.current()
        var result = DirectEndpoint.reachable(profiles[k]?.endpoints?[host] ?? [], from: interfaces).compactMap {
            endpoint in
            DirectConnection(to: endpoint).flatMap { connection in
                endpoint.kind.map { (connection: connection, kind: $0, advertised: false) }
            }
        }
        // And by the name the Mac advertises on the local network, in case its address
        // there changed. Only when no relay is there to say so: the question names the Mac
        // to whatever network the phone is on.
        if interfaces.contains(where: \.isLocalNetwork), !relays(k) || states[k] != .connected {
            result.append(
                (connection: DirectConnection(advertised: host.uuidString), kind: .localNetwork, advertised: true))
        }
        return result
    }

    /// The first of `connections` to open; the others are closed.
    private nonisolated static func firstReady(_ connections: [DirectConnection]) async -> DirectConnection? {
        await withTaskGroup(of: DirectConnection?.self) { group in
            for connection in connections {
                group.addTask {
                    for await event in connection.events {
                        if case .ready = event { return connection }
                        if case .closed = event { return nil }
                    }
                    return nil
                }
            }
            var winner: DirectConnection?
            for await opened in group {
                if let opened {
                    winner = opened
                    break
                }
            }
            group.cancelAll()
            for connection in connections where connection !== winner { connection.close() }
            return winner
        }
    }

    /// The Mac answered to its name at an address the phone didn't have: that replaces
    /// the local ones it had, and its tailnet address stays.
    private func found(_ endpoint: DirectEndpoint, for host: UUID) {
        guard endpoint.kind == .localNetwork, let k = key(for: host) else { return }
        let kept = (profiles[k]?.endpoints?[host] ?? []).filter { $0.kind == .tailscale }
        if profiles[k]?.endpoints == nil { profiles[k]?.endpoints = [:] }
        profiles[k]?.endpoints?[host] = [endpoint] + kept
        try? persist()
    }

    /// Closes the direct connection to a Mac without telling its link.
    private func hangUp(_ host: UUID) {
        guard let direct = directs.removeValue(forKey: host) else { return }
        direct.task.cancel()
        direct.connection?.close()
        directKinds[host] = nil
    }
    func verdict(for host: RelayDevice) -> TrustStore.Verdict {
        guard let k = key(for: host.id), let pin = profiles[k]?.pins[host.id] else { return .unknown }
        return host.publicKeyData == pin ? .trusted : .changed
    }
    /// The app left the screen. A short trip to another app and back shouldn't cost a
    /// reconnect, so the connections stay for a while, as long as iOS lets the app run.
    func background() {
        guard active, lingering == nil else { return }
        backgroundedAt = Date()
        backgroundTask = UIApplication.shared.beginBackgroundTask(withName: "Keep sessions for a quick return") {
            [weak self] in
            // iOS is about to stop the app: hang up, so nothing looks connected when it isn't.
            MainActor.assumeIsolated { self?.stopLingering(suspending: true) }
        }
        guard backgroundTask != .invalid else { return suspend() }
        lingering = Task { [weak self] in
            try? await Task.sleep(for: .seconds(Self.lingerSeconds))
            guard !Task.isCancelled else { return }
            self?.stopLingering(suspending: true)
        }
    }

    private func stopLingering(suspending: Bool) {
        lingering?.cancel()
        lingering = nil
        if suspending { suspend() }
        if backgroundTask != .invalid {
            UIApplication.shared.endBackgroundTask(backgroundTask)
            backgroundTask = .invalid
        }
    }

    func resume() {
        if lingering != nil {
            // Stopped by iOS before the time was up and back much later: what still
            // looks connected has long been dropped at the other end.
            let stale = backgroundedAt.map { Date().timeIntervalSince($0) > Self.lingerSeconds + 5 } ?? true
            stopLingering(suspending: stale)
        }
        active = true
        for k in profiles.keys where !profiles[k]!.pins.isEmpty && relays(k) && loops[k] == nil { start(k) }
        dialAll()
    }
    func suspend() {
        active = false
        for k in Array(loops.keys) { stop(k) }
        for host in Array(directs.keys) {
            hangUp(host)
            links[host]?.directWentAway()
        }
    }
    private func restart(_ k: String) {
        stop(k)
        if active, relays(k), let profile = profiles[k], !profile.pins.isEmpty { start(k) }
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
            guard let profile = profiles[k], !profile.pins.isEmpty else { break }
            let connection = RelayConnection(api: profile.credentials.api)
            connections[k] = connection
            // The paired-computer list is brought up to date meanwhile: waiting for it first
            // put two requests in front of every reconnect. A removed Mac is dropped when it
            // answers, and the relay routes nothing to one before that.
            let refreshing = Task { [weak self] in await self?.refresh(k) }
            defer { refreshing.cancel() }
            for await event in connection.events {
                guard !Task.isCancelled else { break }
                switch event {
                case .ready:
                    states[k] = .connected
                    delay = 1
                    for id in (profiles[k]?.pins ?? [:]).keys { links[id]?.relayReady() }
                case .packet(let from, let packet):
                    if profiles[k]?.pins[from] != nil { links[from]?.receive(packet, via: .relay) }
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
