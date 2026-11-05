import Foundation
import Observation
import TermyRemoteKit
import UIKit

/// The signed-in account, this phone's key, and the one relay socket every
/// host link shares.
@MainActor
@Observable
final class AppModel {
    struct Account: Codable, Equatable {
        var email: String
        var token: String
        var deviceID: UUID
        var relayURL: URL
    }

    enum RelayState: Equatable {
        case offline
        case connecting
        case connected
    }

    private(set) var account: Account?
    private(set) var devices: [RelayDevice] = []
    private(set) var relayState: RelayState = .offline
    private(set) var lastError: String?
    var relayURL: URL {
        didSet { UserDefaults.standard.set(relayURL.absoluteString, forKey: "relayURL") }
    }

    let identity: DeviceIdentity
    @ObservationIgnored private(set) var trust: TrustStore
    @ObservationIgnored private var links: [UUID: HostLink] = [:]
    @ObservationIgnored private var connection: RelayConnection?
    @ObservationIgnored private var loop: Task<Void, Never>?
    @ObservationIgnored private var active = true

    /// Point a development build at a local relay (`just relay`) under Relay server on the sign-in screen.
    static var defaultRelayURL: URL { RelayAPI.defaultURL }

    init() {
        if let raw = Keychain.data("identity"), let identity = try? DeviceIdentity(rawRepresentation: raw) {
            self.identity = identity
        } else {
            identity = DeviceIdentity()
            Keychain.set(identity.rawRepresentation, for: "identity")
        }
        let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        trust = TrustStore(url: support.appendingPathComponent("trusted-hosts.json"))
        relayURL = UserDefaults.standard.string(forKey: "relayURL").flatMap(URL.init(string:)) ?? Self.defaultRelayURL
        // Keychain items outlive the app; a fresh install starts signed out.
        if !UserDefaults.standard.bool(forKey: "installed") {
            Keychain.remove("account")
            UserDefaults.standard.set(true, forKey: "installed")
        }
        if let data = Keychain.data("account"), let account = try? JSONDecoder().decode(Account.self, from: data) {
            self.account = account
            start()
        }
    }

    var api: RelayAPI { RelayAPI(baseURL: account?.relayURL ?? relayURL, token: account?.token) }

    var hosts: [RelayDevice] { devices.filter { $0.kind == .host } }

    // MARK: Account

    func signIn(email: String, password: String, creating: Bool) async throws {
        let api = RelayAPI(baseURL: relayURL)
        let device = DeviceRegistration(name: UIDevice.current.name, kind: .client, publicKey: identity.publicKey)
        let response = creating
            ? try await api.signUp(email: email, password: password, device: device)
            : try await api.logIn(email: email, password: password, device: device)
        let account = Account(email: response.user.email, token: response.token, deviceID: response.device.id, relayURL: relayURL)
        if let data = try? JSONEncoder().encode(account) { Keychain.set(data, for: "account") }
        self.account = account
        start()
    }

    func signOut() async {
        try? await api.logOut()
        endSession()
    }

    func deleteAccount() async throws {
        try await api.deleteAccount()
        endSession()
    }

    private func endSession() {
        stop()
        Keychain.remove("account")
        account = nil
        devices = []
        links.removeAll()
    }

    func refreshDevices() async {
        guard account != nil else { return }
        do {
            devices = try await api.devices()
            lastError = nil
        } catch let error as RelayAPIError where error.status == 401 {
            endSession()
        } catch {
            lastError = error.localizedDescription
        }
    }

    // MARK: Relay socket

    func link(for host: RelayDevice) -> HostLink {
        if let link = links[host.id] {
            link.device = host
            return link
        }
        let link = HostLink(device: host, model: self)
        links[host.id] = link
        return link
    }

    func send(_ packet: Data, to device: UUID) {
        connection?.send(packet, to: device)
    }

    /// The app came to the front: reconnect.
    func resume() {
        active = true
        if account != nil, loop == nil { start() }
    }

    /// The app went to the background: drop the socket; links reconnect on resume.
    func suspend() {
        active = false
        stop()
    }

    private func start() {
        guard loop == nil else { return }
        loop = Task { [weak self] in await self?.runRelay() }
    }

    private func stop() {
        loop?.cancel()
        loop = nil
        connection?.close()
        connection = nil
        relayState = .offline
        for link in links.values { link.relayWentAway() }
    }

    private func runRelay() async {
        var delay: UInt64 = 1
        while !Task.isCancelled, account != nil {
            relayState = .connecting
            await refreshDevices()
            guard account != nil else { break }
            let connection = RelayConnection(api: api)
            self.connection = connection
            for await event in connection.events {
                switch event {
                case .ready:
                    relayState = .connected
                    delay = 1
                    for link in links.values { link.relayReady() }
                case .packet(let from, let packet):
                    links[from]?.receive(packet)
                case .presence(let device, let online):
                    if let index = devices.firstIndex(where: { $0.id == device }) {
                        devices[index].online = online
                    } else if online {
                        await refreshDevices()
                    }
                    links[device]?.presenceChanged(online: online)
                case .undeliverable(let device):
                    links[device]?.presenceChanged(online: false)
                case .closed:
                    break
                }
            }
            if self.connection === connection { self.connection = nil }
            relayState = .offline
            for link in links.values { link.relayWentAway() }
            guard !Task.isCancelled, active else { break }
            try? await Task.sleep(nanoseconds: delay * 1_000_000_000)
            delay = min(delay * 2, 30)
        }
    }

    // MARK: Trust

    func verdict(for host: RelayDevice) -> TrustStore.Verdict {
        guard let key = host.publicKeyData else { return .changed }
        return trust.verdict(deviceID: host.id, publicKey: key)
    }

    func trustHost(_ host: RelayDevice) {
        guard let key = host.publicKeyData else { return }
        try? trust.trust(deviceID: host.id, name: host.name, publicKey: key)
    }
}
