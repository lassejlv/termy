import AppKit
import CoreImage.CIFilterBuiltins
import Observation
import SwiftUI
import TermyRemoteHost
import TermyRemoteKit

/// Next owns the UI/lifecycle; the host and pairing protocol remain reusable.
@MainActor @Observable
final class MobileController {
    static let shared = MobileController()
    var enabled = UserDefaults.standard.bool(forKey: "mobile.enabled") {
        didSet {
            UserDefaults.standard.set(enabled, forKey: "mobile.enabled")
            configure()
        }
    }
    var relayAddress = UserDefaults.standard.string(forKey: "mobile.relay") ?? RelayAPI.defaultURL.absoluteString
    private(set) var status = "Mobile access is off"
    private(set) var devices: [RelayDevice] = []
    private(set) var error: String?
    var showPairing = false
    private(set) var code: PairingCode?
    private(set) var pending: PairingStatus?
    private(set) var pairingBusy = false
    private(set) var pairingError: String?

    @ObservationIgnored private let store = DeviceStore(service: "sh.termy.next.mobile")
    @ObservationIgnored private var credentials: DeviceCredentials?
    @ObservationIgnored private var agent: RemoteHostAgent?
    @ObservationIgnored private var task: Task<Void, Never>?
    @ObservationIgnored private var pairingTask: Task<Void, Never>?
    @ObservationIgnored private var generation = UUID()
    @ObservationIgnored private var pairingGeneration = UUID()
    @ObservationIgnored private var lastUsesSessionHost = Preferences.shared.usesSessionHost
    @ObservationIgnored private var observer: NSObjectProtocol?

    private init() {
        observer = NotificationCenter.default.addObserver(forName: Preferences.didChange, object: nil, queue: .main) {
            _ in
            Task { @MainActor in
                guard self.lastUsesSessionHost != Preferences.shared.usesSessionHost else { return }
                self.lastUsesSessionHost = Preferences.shared.usesSessionHost
                self.configure()
            }
        }
    }
    func configure() {
        stop()
        guard enabled else {
            status = "Mobile access is off"
            return
        }
        guard Preferences.shared.usesSessionHost, let mux = SessionHost.shared.multiplexer else {
            status = "Enable the session host in Settings → Sessions to use mobile access."
            return
        }
        let current = generation
        task = Task { [weak self] in
            guard let self else { return }
            do {
                guard let url = URL(string: relayAddress.trimmingCharacters(in: .whitespacesAndNewlines)) else {
                    throw PairingError.invalidCode
                }
                let relay = try RelayAddress.validate(url)
                let identity = try store.identity()
                var saved = try store.load([String: DeviceCredentials].self, key: "registrations") ?? [:]
                var c: DeviceCredentials
                if let existing = saved[relay.absoluteString] {
                    c = existing
                } else {
                    status = "Connecting to relay…"
                    c = try await RelayAPI(baseURL: relay).register(
                        try DeviceRegistration(
                            name: Host.current().localizedName ?? "Mac", kind: .host, identity: identity))
                    saved[relay.absoluteString] = c
                    try store.save(saved, key: "registrations")
                }
                guard !Task.isCancelled, current == generation else { return }
                do {
                    try await c.api.updateName(Host.current().localizedName ?? "Mac", identity: identity)
                    devices = try localNames(try await c.api.devices(), relay: relay)
                } catch let e
                    as RelayAPIError where e.status == 401
                {
                    saved[relay.absoluteString] = nil
                    try store.save(saved, key: "registrations")
                    throw e
                }
                guard !Task.isCancelled, current == generation else { return }
                credentials = c
                let allPins = try store.load([String: [UUID: Data]].self, key: "pins") ?? [:]
                let host = RemoteHostAgent(
                    multiplexer: mux, credentials: c, identity: identity,
                    trustedDevices: allPins[relay.absoluteString] ?? [:])
                host.configPath = AppConfig.configURL.path
                host.launchEnvironment.merge(ShellPlugins.shared.environment(token: nil)) { $1 }
                host.configurationProvider = {
                    RemoteConfiguration(render: AppConfig.shared.renderConfig,
                        contents: (try? String(contentsOf: AppConfig.configURL, encoding: .utf8)) ?? "",
                        reduceMotion: Preferences.shared.reduceMotion, appearance: AppConfig.shared.appearance)
                }
                host.onConnectionState = { [weak self] state in
                    guard let self, current == self.generation else { return }
                    switch state {
                    case .connecting: self.status = "Connecting to relay…"
                    case .connected:
                        self.status = "Mobile access enabled"
                        self.error = nil
                    case .offline: self.status = "Relay disconnected. Reconnecting…"
                    }
                }
                agent = host
                status = "Connecting to relay…"
                error = nil
                let refresh = Task { [weak self] in
                    while !Task.isCancelled {
                        do {
                            try await Task.sleep(for: .seconds(3))
                            try Task.checkCancellation()
                            // Only Settings shows the list: don't ask the relay every
                            // 3 s, day and night, while nobody is looking at it.
                            guard SettingsWindowController.isShowingMobile else { continue }
                            self?.devices = try self?.localNames(try await c.api.devices(), relay: relay) ?? []
                        } catch is CancellationError { return } catch {
                            self?.error = error.localizedDescription
                        }
                    }
                }
                defer { refresh.cancel() }
                try await host.run()
            } catch is CancellationError {} catch {
                guard current == generation else { return }
                status = "Couldn’t connect"
                self.error = error.localizedDescription
            }
        }
    }
    func stop() {
        generation = UUID()
        task?.cancel()
        task = nil
        agent?.stop()
        agent = nil
        cancelPairing()
        showPairing = false
        credentials = nil
        devices = []
    }
    func applyRelay() {
        do {
            guard let url = URL(string: relayAddress.trimmingCharacters(in: .whitespacesAndNewlines)) else {
                throw PairingError.invalidCode
            }
            _ = try RelayAddress.validate(url)
            relayAddress = url.absoluteString
            UserDefaults.standard.set(relayAddress, forKey: "mobile.relay")
            error = nil
            configure()
        } catch { self.error = "Use an HTTPS relay URL, such as https://relay.example.com." }
    }
    func beginPairing() async {
        cancelPairing()
        showPairing = true
        pairingBusy = true
        pairingError = nil
        guard let credentials else {
            pairingBusy = false
            pairingError = "Enable mobile access and connect to the relay first."
            return
        }
        let current = pairingGeneration
        do {
            let status = try await credentials.api.createPairing()
            guard current == pairingGeneration else {
                try? await credentials.api.cancelPairing(status.id)
                return
            }
            guard status.host.id == credentials.device.id,
                status.host.publicKeyData == (try store.identity()).publicKey
            else { throw PairingError.invalidProof }
            code = try PairingCode(
                status: status, relayURL: credentials.relayURL,
                hostName: Host.current().localizedName ?? "Mac")
            pairingBusy = false
            pairingTask = Task { [weak self] in
                guard let self else { return }
                do {
                    while !Task.isCancelled {
                        try await Task.sleep(for: .seconds(1))
                        guard current == pairingGeneration, let code else { return }
                        guard code.expiresAt > Date() else { throw PairingError.expired }
                        let next = try await credentials.api.pairing(code.invitationID)
                        guard current == pairingGeneration else { return }
                        if next.client != nil {
                            let verified = try code.verify(next)
                            var named = next
                            named.client = verified.client
                            pending = named
                            return
                        }
                    }
                } catch is CancellationError {} catch {
                    if current == pairingGeneration {
                        pairingError = error.localizedDescription
                        pending = nil
                        code = nil
                    }
                }
            }
        } catch {
            if current == pairingGeneration {
                pairingBusy = false
                pairingError = error.localizedDescription
            }
        }
    }
    func approve() async {
        guard !pairingBusy, let credentials, let code, let pending else { return }
        pairingBusy = true
        do {
            guard code.expiresAt > Date() else { throw PairingError.expired }
            let verified = try code.verify(pending)
            let (_, response) = try SecureChannel.respond(identity: store.identity(), initPacket: verified.initPacket)
            // Persist the verified pin before authorizing the relay link. A failed write grants no access.
            var pins = try store.load([String: [UUID: Data]].self, key: "pins") ?? [:]
            pins[credentials.relayURL.absoluteString, default: [:]][verified.client.id] = verified.client.publicKeyData!
            try store.save(pins, key: "pins")
            var names = try store.load([String: [UUID: RelayDevice]].self, key: "peerNames") ?? [:]
            names[credentials.relayURL.absoluteString, default: [:]][verified.client.id] = verified.client
            try store.save(names, key: "peerNames")
            try await credentials.api.approve(
                code.invitationID, client: verified.client.id, initPacket: verified.initPacket, response: response)
            showPairing = false
            configure()
        } catch {
            pairingBusy = false
            pairingError = error.localizedDescription
        }
    }
    func cancelPairing() {
        pairingGeneration = UUID()
        pairingTask?.cancel()
        pairingTask = nil
        if let code, let credentials { Task { try? await credentials.api.cancelPairing(code.invitationID) } }
        code = nil
        pending = nil
        pairingBusy = false
    }
    private func localNames(_ peers: [RelayDevice], relay: URL) throws -> [RelayDevice] {
        let saved = try store.load([String: [UUID: RelayDevice]].self, key: "peerNames") ?? [:]
        return peers.map { peer in
            peer.withLocalName(from: saved[relay.absoluteString]?[peer.id])
        }
    }
    func remove(_ device: RelayDevice) async {
        guard let credentials else { return }
        do {
            try await credentials.api.removeDevice(device.id)
            var pins = try store.load([String: [UUID: Data]].self, key: "pins") ?? [:]
            pins[credentials.relayURL.absoluteString]?[device.id] = nil
            try store.save(pins, key: "pins")
            var names = try store.load([String: [UUID: RelayDevice]].self, key: "peerNames") ?? [:]
            names[credentials.relayURL.absoluteString]?[device.id] = nil
            try store.save(names, key: "peerNames")
            configure()
        } catch { self.error = error.localizedDescription }
    }
}

struct MobileSettingsView: View {
    @Bindable private var mobile = MobileController.shared
    @State private var removing: RelayDevice?
    var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            Toggle("Allow mobile access", isOn: $mobile.enabled)
            Text(mobile.status).font(.callout).foregroundStyle(.secondary)
            Text(
                "Paired devices can view and control this Mac’s Termy Next sessions. Access works while Termy Next is running."
            )
            .font(.callout).foregroundStyle(.secondary)
            Button("Pair iPhone…", systemImage: "qrcode") { Task { await mobile.beginPairing() } }
                .disabled(!mobile.enabled)
            if !mobile.devices.isEmpty {
                Divider()
                Text("Paired Devices").font(.headline)
                ForEach(mobile.devices) { device in
                    HStack {
                        Image(systemName: "iphone")
                        VStack(alignment: .leading) {
                            Text(device.name)
                            Text(device.online ? "Online" : "Offline").font(.caption).foregroundStyle(.secondary)
                        }
                        Spacer()
                        Button("Remove Access", role: .destructive) { removing = device }
                    }
                }
            }
            Divider()
            Text("Relay Server").font(.headline)
            HStack {
                TextField("https://relay.example.com", text: $mobile.relayAddress).textFieldStyle(.roundedBorder)
                Button("Apply") { mobile.applyRelay() }
            }
            Text(
                "Use Termy’s relay or an HTTPS relay on your own VPS. The QR code configures the phone automatically. Switching relays requires pairing again on that server."
            )
            .font(.callout).foregroundStyle(.secondary)
            if let error = mobile.error {
                Text(error).foregroundStyle(.red)
                Button("Retry Connection") { mobile.configure() }
            }
        }
        .frame(width: 496, alignment: .leading)
        .sheet(isPresented: $mobile.showPairing) { pairingSheet }
        .confirmationDialog(
            "Remove access for \(removing?.name ?? "this device")?",
            isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } })
        ) {
            Button("Remove Access", role: .destructive) {
                if let removing { Task { await mobile.remove(removing) } }
                removing = nil
            }
        } message: {
            Text("It will need to scan a new QR code to connect again. Your terminal sessions keep running.")
        }
    }
    private var pairingSheet: some View {
        VStack(spacing: 16) {
            Text(mobile.pending == nil ? "Pair Your iPhone" : "Allow This iPhone?").font(.title2.bold())
            if let pending = mobile.pending, let client = pending.client {
                Text("Allow \(client.name) to view and control this Mac’s terminal sessions?").multilineTextAlignment(
                    .center)
                Text("Only approve a pairing you just started.").foregroundStyle(.secondary)
                Button("Allow Access") { Task { await mobile.approve() } }.disabled(mobile.pairingBusy)
            } else if let code = mobile.code {
                if let image = qrImage(code) {
                    Image(nsImage: image).interpolation(.none).resizable().frame(width: 250, height: 250)
                        .padding(16).background(.white).accessibilityLabel("Pairing QR code")
                }
                Text("Open Termy on your iPhone and tap Scan QR Code.")
                Text(code.relayURL.host ?? "").font(.caption).foregroundStyle(.secondary)
                TimelineView(.periodic(from: .now, by: 1)) { context in
                    Text("Expires in \(max(0, Int(code.expiresAt.timeIntervalSince(context.date)))) seconds").font(
                        .caption
                    ).foregroundStyle(.secondary)
                }
            }
            if mobile.pairingBusy { ProgressView() }
            if let error = mobile.pairingError {
                Text(error).foregroundStyle(.red).multilineTextAlignment(.center)
                Button("Generate New Code") { Task { await mobile.beginPairing() } }
            }
            Button("Cancel") { mobile.showPairing = false }
        }
        .padding(28).frame(width: 380)
        .onDisappear { mobile.cancelPairing() }
    }
    private func qrImage(_ code: PairingCode) -> NSImage? {
        guard let text = try? code.text else { return nil }
        let filter = CIFilter.qrCodeGenerator()
        filter.message = Data(text.utf8)
        filter.correctionLevel = "M"
        guard let output = filter.outputImage,
            let cg = CIContext().createCGImage(
                output.transformed(by: CGAffineTransform(scaleX: 5, y: 5)),
                from: output.extent.applying(CGAffineTransform(scaleX: 5, y: 5)))
        else { return nil }
        return NSImage(cgImage: cg, size: NSSize(width: cg.width, height: cg.height))
    }
}
