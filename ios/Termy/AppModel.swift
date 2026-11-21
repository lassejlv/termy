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

/// Pairing a Mac on a connection of the phone's own, over Tailscale.
enum DirectPairingError: LocalizedError {
    case noTailscale, unreachable(String), refused(String)
    var errorDescription: String? {
        switch self {
        case .noTailscale: "Start Tailscale on this iPhone, then scan again. Termy connects to your Mac over your tailnet."
        case .unreachable(let name):
            "This iPhone can’t reach \(name) over Tailscale. Make sure Tailscale is running on both, on the same tailnet, then scan again."
        case .refused(let name): "\(name) didn’t accept the pairing. Generate a new QR code on it and scan again."
        }
    }
}

/// What the phone keeps about the Macs it paired with.
struct Pairings: Codable, Equatable {
    /// This phone's ID among its Macs, made up the first time it paired.
    var clientID = UUID()
    var pins: [UUID: Data] = [:]
    var hosts: [PairedDevice] = []
    var configurations: [UUID: RemoteConfiguration] = [:]
    /// Where each Mac said it can be reached.
    var endpoints: [UUID: [DirectEndpoint]] = [:]
    /// Macs paired through the relay Termy used to have. They can't be reached and
    /// stay listed until they are paired again or removed.
    var relayPaired: [PairedDevice] = []

    /// What builds that had a relay kept: one profile per relay, and one, "direct",
    /// for the Macs paired without one.
    struct LegacyProfile: Decodable {
        struct Credentials: Decodable { var device: PairedDevice }
        var credentials: Credentials
        var pins: [UUID: Data]
        var hosts: [PairedDevice]
        var configurations: [UUID: RemoteConfiguration]?
        var endpoints: [UUID: [DirectEndpoint]]?
    }

    /// The Macs paired without a relay carry over; those paired through one are kept to be paired again.
    static func migrating(_ profiles: [String: LegacyProfile]) -> Pairings {
        var pairings = Pairings()
        if let direct = profiles["direct"] {
            pairings.clientID = direct.credentials.device.id
            pairings.pins = direct.pins
            pairings.hosts = direct.hosts.filter { direct.pins[$0.id] != nil }
            pairings.configurations = direct.configurations ?? [:]
            pairings.endpoints = direct.endpoints ?? [:]
        }
        var seen = Set(pairings.pins.keys)
        for (key, profile) in profiles.sorted(by: { $0.key < $1.key }) where key != "direct" {
            for host in profile.hosts where profile.pins[host.id] != nil && seen.insert(host.id).inserted {
                var host = host
                host.online = false
                pairings.relayPaired.append(host)
            }
        }
        return pairings
    }
}

/// One device identity and the Macs it paired with.
@MainActor @Observable
final class AppModel: HostTransport {
    /// One model per process, so App Intents and the widget's links reach the UI's.
    static let shared = AppModel()
    /// The phone's own connection to one Mac, open or being opened.
    private struct Direct {
        var connection: DirectConnection?
        var task: Task<Void, Never>
    }
    let identity: DeviceIdentity
    private(set) var devices: [PairedDevice] = []
    /// Macs paired through the relay, which no longer exists. They have to pair again.
    private(set) var relayPaired: [PairedDevice] = []
    private(set) var lastError: String?
    private(set) var pairingMessage: String?
    private(set) var pairingBusy = false
    @ObservationIgnored private let store = DeviceStore(service: "sh.termy.ios.pairing")
    @ObservationIgnored private var pairings = Pairings()
    @ObservationIgnored private var links: [UUID: HostLink] = [:]
    @ObservationIgnored private var directs: [UUID: Direct] = [:]
    /// The Macs the phone has a connection to right now.
    private(set) var connectedHosts: Set<UUID> = []
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
            do { try load() } catch {
                identityError = error
                lastError = error.localizedDescription
            }
        }
        rebuildDevices()
        resume()
        // Tailscale connecting opens a way to the Macs.
        pathMonitor.pathUpdateHandler = { [weak self] _ in Task { @MainActor in self?.dialAll() } }
        pathMonitor.start(queue: .main)
    }

    private func load() throws {
        if let saved = try store.load(Pairings.self, key: "pairings") {
            pairings = saved
            return
        }
        // A build that had a relay saved its pairings as profiles, its relay credentials among them.
        guard let old = try? store.load([String: Pairings.LegacyProfile].self, key: "profiles") else { return }
        pairings = Pairings.migrating(old)
        try persist()
        store.remove(key: "profiles")
    }

    var hosts: [PairedDevice] { devices }
    private func persist() throws { try store.save(pairings, key: "pairings") }
    private func rebuildDevices() {
        devices = pairings.hosts.sorted { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
        relayPaired = pairings.relayPaired.sorted { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
        publishDevices()
    }
    func isConnected(to host: UUID) -> Bool { directs[host]?.connection != nil }

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

    /// Removes a computer, whose key no longer matches or that was paired through the relay, then opens pairing.
    func repair(_ host: PairedDevice) throws {
        try removeHost(host)
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
                id: device.id, name: device.name, online: connectedHosts.contains(device.id),
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

    // MARK: Pairing

    /// Claims the Mac's invitation on a connection to one of the addresses in the QR code
    /// and waits there for the Mac's user to approve.
    func pair(_ code: PairingCode) async throws {
        guard !pairingBusy else { return }
        if let identityError { throw identityError }
        _ = try PairingCode.parse(code.text)
        pairingBusy = true
        defer {
            pairingBusy = false
            pairingMessage = nil
        }
        guard NetworkInterface.onTailnet else { throw DirectPairingError.noTailscale }
        pairingMessage = "Looking for \(code.hostName)…"
        let handshake = SecureChannel.startClient(identity: identity, hostStaticKey: code.hostKey)
        let claim = try DirectPairing.packet(
            try code.claim(clientID: pairings.clientID, name: UIDevice.current.name, initPacket: handshake.initPacket))
        for endpoint in DirectEndpoint.reachable(code.endpoints, from: NetworkInterface.current()) {
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
                    pairings.pins[code.hostID] = code.hostKey
                    pairings.hosts.removeAll { $0.id == code.hostID }
                    pairings.hosts.append(PairedDevice(id: code.hostID, name: code.hostName, kind: .host, publicKey: code.hostKey))
                    pairings.endpoints[code.hostID] = code.endpoints
                    // Paired again: the relay-era entry for the same Mac, known by its key, is done with.
                    pairings.relayPaired.removeAll { $0.publicKeyData == code.hostKey }
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

    /// Forgets a computer here. It keeps this phone's key until it is removed on the Mac too.
    func removeHost(_ host: PairedDevice) throws {
        var updated = pairings
        updated.pins[host.id] = nil
        updated.configurations[host.id] = nil
        updated.endpoints[host.id] = nil
        updated.hosts.removeAll { $0.id == host.id }
        updated.relayPaired.removeAll { $0.id == host.id }
        try store.save(updated, key: "pairings")
        pairings = updated
        hangUp(host.id)
        links.removeValue(forKey: host.id)?.disconnect()
        rebuildDevices()
    }
    func configuration(for host: UUID) -> RemoteConfiguration? { pairings.configurations[host] }
    func saveConfiguration(_ configuration: RemoteConfiguration, for host: UUID) {
        guard pairings.pins[host] != nil else { return }
        pairings.configurations[host] = configuration
        do { try persist() } catch { lastError = "Couldn’t securely save the Mac’s config." }
    }
    func link(for host: PairedDevice) -> HostLink {
        if let link = links[host.id] {
            link.device = host
            return link
        }
        let link = HostLink(device: host, model: self)
        links[host.id] = link
        return link
    }
    func send(_ packet: Data, to host: UUID) {
        directs[host]?.connection?.send(packet)
    }

    // MARK: Direct connections

    /// A Mac said where it listens. Nothing but the Mac itself, inside the encrypted
    /// channel, can say so; the list is kept with its pin.
    func endpointsChanged(_ endpoints: [DirectEndpoint], for host: UUID) {
        guard pairings.pins[host] != nil else { return }
        let kept = Array(endpoints.filter(\.isDialable).prefix(DirectEndpoint.maxCount))
        if pairings.endpoints[host] ?? [] != kept {
            pairings.endpoints[host] = kept.isEmpty ? nil : kept
            try? persist()
        }
        dial(host)
    }

    func directFailed(for host: UUID) {
        hangUp(host)
        links[host]?.directWentAway()
    }

    private func dialAll() {
        for host in pairings.pins.keys { dial(host) }
    }

    /// Opens a connection of the phone's own to a Mac a view has open, over the tailnet:
    /// every address it knows is dialed at once and the first to answer is used, so one the
    /// Mac no longer has costs nothing. A failed try is repeated: a reopened app dials
    /// before its network is back, and nothing else says when the Mac is.
    func dial(_ host: UUID) {
        guard active, directs[host] == nil, links[host]?.wanted == true, pairings.pins[host] != nil else { return }
        let task = Task { [weak self] in
            guard let self else { return }
            var failures = 0
            while !Task.isCancelled, self.active {
                let started = Date()
                let candidates = DirectEndpoint.reachable(self.pairings.endpoints[host] ?? [], from: NetworkInterface.current())
                    .compactMap { DirectConnection(to: $0) }
                let winner = await Self.firstReady(candidates)
                guard !Task.isCancelled else {
                    winner?.close()
                    return
                }
                if let winner {
                    self.directs[host]?.connection = winner
                    self.connectedHosts.insert(host)
                    self.links[host]?.directReady()
                    for await event in winner.events {
                        if case .packet(let packet) = event { self.links[host]?.receive(packet) }
                    }
                    winner.close()
                    guard !Task.isCancelled else { return }
                    self.directs[host]?.connection = nil
                    self.connectedHosts.remove(host)
                    self.links[host]?.directWentAway()
                    // One that lasted was a working path, and the Mac may be right back (it
                    // restarts its listener with its settings). One that didn't counts as a miss.
                    failures = Date().timeIntervalSince(started) > 10 ? 0 : failures + 1
                    if failures == 0 { continue }
                } else {
                    failures += 1
                }
                if failures >= 2 { self.links[host]?.unreachable(self.unreachableReason(host)) }
                try? await Task.sleep(for: .milliseconds(min(500 << min(failures - 1, 5), 8000)))
            }
            if !Task.isCancelled { self.directs[host] = nil }
        }
        directs[host] = Direct(connection: nil, task: task)
    }

    /// Why a Mac doesn't answer, as far as the phone can tell.
    private func unreachableReason(_ host: UUID) -> HostLink.Unreachable {
        guard NetworkInterface.onTailnet else { return .phoneOffTailnet }
        return (pairings.endpoints[host] ?? []).contains(where: \.isDialable) ? .noAnswer : .noTailnetAddress
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

    /// Closes the direct connection to a Mac without telling its link.
    private func hangUp(_ host: UUID) {
        guard let direct = directs.removeValue(forKey: host) else { return }
        direct.task.cancel()
        direct.connection?.close()
        connectedHosts.remove(host)
    }
    func verdict(for host: PairedDevice) -> TrustStore.Verdict {
        guard let pin = pairings.pins[host.id] else { return .unknown }
        return host.publicKeyData == pin ? .trusted : .changed
    }

    // MARK: Lifecycle

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
        dialAll()
    }
    func suspend() {
        active = false
        for host in Array(directs.keys) {
            hangUp(host)
            links[host]?.directWentAway()
        }
    }
}
