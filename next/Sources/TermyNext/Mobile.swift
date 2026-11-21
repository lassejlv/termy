import AppKit
import CoreImage.CIFilterBuiltins
import Observation
import SwiftUI
import Termy
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
    /// Where the pins and names of paired phones are kept. Entries under other keys are
    /// phones paired through the relay Termy Next used to have.
    private static let pairingsKey = "direct"
    /// Where phones reach this Mac right now.
    private(set) var directEndpoints: [DirectEndpoint] = []
    private(set) var status = "Mobile access is off"
    private(set) var devices: [PairedDevice] = []
    /// iPhones paired through the relay, which no longer exists. They have to pair again.
    private(set) var relayPaired: [PairedDevice] = []
    private(set) var error: String?
    var showPairing = false
    private(set) var code: PairingCode?
    /// The phone waiting for approval.
    private(set) var pendingClient: PairedDevice?
    private(set) var pairingBusy = false
    private(set) var pairingError: String?

    @ObservationIgnored private let store = DeviceStore(service: "sh.termy.next.mobile")
    /// The ID paired phones know this Mac by; nil while Mobile access is off.
    var deviceID: UUID? { agent?.hostID }
    @ObservationIgnored private var agent: RemoteHostAgent?
    @ObservationIgnored private var task: Task<Void, Never>?
    @ObservationIgnored private var pairingTask: Task<Void, Never>?
    @ObservationIgnored private var generation = UUID()
    @ObservationIgnored private var pairingGeneration = UUID()
    @ObservationIgnored private var lastUsesSessionHost = Preferences.shared.usesSessionHost
    @ObservationIgnored private var observer: NSObjectProtocol?

    private init() {
        // Settings of the relay, which is gone.
        for key in ["mobile.relay", "mobile.useRelay", "mobile.direct"] { UserDefaults.standard.removeObject(forKey: key) }
        store.remove(key: "registrations")
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
        loadRelayPaired()
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
            do { try await serve(mux, generation: current) } catch is CancellationError {} catch {
                guard current == generation else { return }
                status = "Couldn’t start mobile access"
                self.error = error.localizedDescription
            }
        }
    }

    private func serve(_ mux: Multiplexer, generation current: UUID) async throws {
        let identity = try store.identity()
        var hostID = try store.load(UUID.self, key: "directHostID")
        if hostID == nil {
            hostID = UUID()
            try store.save(hostID, key: "directHostID")
        }
        let pins = (try store.load([String: [UUID: Data]].self, key: "pins") ?? [:])[Self.pairingsKey] ?? [:]
        guard !Task.isCancelled, current == generation, let hostID else { return }
        let host = RemoteHostAgent(multiplexer: mux, hostID: hostID, identity: identity, trustedDevices: pins)
        host.configPath = AppConfig.configURL.path
        host.launchEnvironment.merge(ShellPlugins.shared.environment(token: nil)) { $1 }
        host.configurationProvider = {
            RemoteConfiguration(render: AppConfig.shared.renderConfig,
                contents: (try? String(contentsOf: AppConfig.configURL, encoding: .utf8)) ?? "",
                reduceMotion: Preferences.shared.reduceMotion, appearance: AppConfig.shared.appearance)
        }
        host.onDirectEndpoints = { [weak self] endpoints in
            guard let self, current == self.generation else { return }
            self.directEndpoints = endpoints
        }
        host.onPeersChanged = { [weak self] in
            guard let self, current == self.generation else { return }
            self.reloadDevices()
        }
        host.onPairingRequest = { [weak self] device in
            guard let self, current == self.generation, self.showPairing else { return }
            self.pendingClient = device
            if device == nil, self.code != nil { self.pairingError = "The iPhone stopped waiting. Scan the code again." }
        }
        agent = host
        status = "Mobile access enabled"
        error = nil
        reloadDevices()
        try await host.run()
    }

    private func reloadDevices() {
        let names = (try? store.load([String: [UUID: PairedDevice]].self, key: "peerNames"))??[Self.pairingsKey] ?? [:]
        let connected = agent?.connectedDevices ?? []
        devices = (agent?.trustedDevices ?? [:]).compactMap { id, key in
            var device = names[id] ?? PairedDevice(id: id, name: "iPhone", kind: .client, publicKey: key)
            device.online = connected.contains(id)
            return device
        }.sorted { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
    }

    private func loadRelayPaired() {
        let pins = (try? store.load([String: [UUID: Data]].self, key: "pins")) ?? [:]
        let names = (try? store.load([String: [UUID: PairedDevice]].self, key: "peerNames")) ?? [:]
        relayPaired = pins.filter { $0.key != Self.pairingsKey }.flatMap { relay, phones in
            phones.map { id, key in names[relay]?[id] ?? PairedDevice(id: id, name: "iPhone", kind: .client, publicKey: key) }
        }.sorted { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
    }

    func beginPairing() {
        cancelPairing()
        showPairing = true
        pairingError = nil
        guard let agent else {
            pairingError = "Enable mobile access first."
            return
        }
        do {
            let code = try agent.beginPairing(hostName: Host.current().localizedName ?? "Mac")
            self.code = code
            let current = pairingGeneration
            pairingTask = Task { [weak self] in
                try? await Task.sleep(for: .seconds(max(0, code.expiresAt.timeIntervalSinceNow)))
                guard let self, !Task.isCancelled, current == self.pairingGeneration else { return }
                self.agent?.cancelPairing()
                self.pendingClient = nil
                self.code = nil
                self.pairingError = PairingError.expired.localizedDescription
            }
        } catch {
            pairingError =
                "This Mac has no address an iPhone could reach. Join a Wi-Fi or Ethernet network, or start Tailscale, then try again."
        }
    }

    func approve() {
        guard !pairingBusy, let agent, let client = agent.pairingClaimant, let key = client.publicKeyData else { return }
        pairingBusy = true
        do {
            // Persist the verified pin before answering the phone. A failed write grants no access.
            var pins = try store.load([String: [UUID: Data]].self, key: "pins") ?? [:]
            let previous = pins[Self.pairingsKey]?[client.id]
            pins[Self.pairingsKey, default: [:]][client.id] = key
            // The same iPhone paired through the relay before: that entry is done with.
            for relay in pins.keys where relay != Self.pairingsKey {
                pins[relay] = pins[relay]?.filter { $0.value != key }
                if pins[relay]?.isEmpty == true { pins[relay] = nil }
            }
            try store.save(pins, key: "pins")
            var names = try store.load([String: [UUID: PairedDevice]].self, key: "peerNames") ?? [:]
            names[Self.pairingsKey, default: [:]][client.id] = client
            try store.save(names, key: "peerNames")
            do { try agent.approvePairing() } catch {
                // The invitation ran out meanwhile: the phone got no answer, so it keeps no access either.
                pins[Self.pairingsKey]?[client.id] = previous
                try? store.save(pins, key: "pins")
                throw error
            }
            showPairing = false
            reloadDevices()
            loadRelayPaired()
        } catch {
            pairingBusy = false
            pendingClient = nil
            code = nil
            pairingError = error.localizedDescription
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
        PaneHandoff.stop()
        devices = []
        directEndpoints = []
    }
    func cancelPairing() {
        pairingGeneration = UUID()
        pairingTask?.cancel()
        pairingTask = nil
        agent?.cancelPairing()
        code = nil
        pendingClient = nil
        pairingBusy = false
    }
    var directStatus: String {
        let local = directEndpoints.contains { $0.kind == .localNetwork }
        let tailnet = directEndpoints.first { $0.kind == .tailscale }
        switch (local, tailnet) {
        case (true, let tailnet?):
            return "Your iPhone connects on this network and over Tailscale (\(tailnet.host))."
        case (true, nil):
            return "Your iPhone connects on this network. Start Tailscale on both devices to connect from anywhere."
        case (false, let tailnet?):
            return "Your iPhone connects over Tailscale (\(tailnet.host))."
        case (false, nil):
            return "This Mac has no local network or Tailscale address right now, so no iPhone can reach it. Join a network or start Tailscale."
        }
    }
    func remove(_ device: PairedDevice) {
        do {
            var pins = try store.load([String: [UUID: Data]].self, key: "pins") ?? [:]
            var names = try store.load([String: [UUID: PairedDevice]].self, key: "peerNames") ?? [:]
            for key in Array(pins.keys) {
                pins[key]?[device.id] = nil
                if key != Self.pairingsKey, pins[key]?.isEmpty == true { pins[key] = nil }
            }
            for key in Array(names.keys) {
                names[key]?[device.id] = nil
                if key != Self.pairingsKey, names[key]?.isEmpty == true { names[key] = nil }
            }
            try store.save(pins, key: "pins")
            try store.save(names, key: "peerNames")
            // Restarting the host drops the phone's connection and forgets its key.
            configure()
        } catch { self.error = error.localizedDescription }
    }
}

struct MobileSettingsView: View {
    @Bindable private var mobile = MobileController.shared
    @State private var removing: PairedDevice?
    var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            Toggle("Allow mobile access", isOn: $mobile.enabled)
            Text(mobile.status).font(.callout).foregroundStyle(.secondary)
            Text(
                "Paired devices can view and control this Mac’s Termy Next sessions. Access works while Termy Next is running."
            )
            .font(.callout).foregroundStyle(.secondary)
            Button("Pair iPhone…", systemImage: "qrcode") { mobile.beginPairing() }
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
            if !mobile.relayPaired.isEmpty {
                Divider()
                Text("Paired Through the Relay").font(.headline)
                Text("Termy no longer uses a relay server. These iPhones can’t connect until you pair them again.")
                    .font(.callout).foregroundStyle(.secondary)
                ForEach(mobile.relayPaired) { device in
                    HStack {
                        Image(systemName: "iphone.slash")
                        VStack(alignment: .leading) {
                            Text(device.name)
                            Text("Pair again to connect").font(.caption).foregroundStyle(.secondary)
                        }
                        Spacer()
                        Button("Remove", role: .destructive) { mobile.remove(device) }
                    }
                }
            }
            if mobile.enabled {
                Divider()
                Text("Connection").font(.headline)
                Text(mobile.directStatus).font(.callout).foregroundStyle(.secondary)
                Text(
                    "No server is involved: your iPhone pairs with this Mac and connects to it directly. Notifications about finished commands arrive while Termy is open on the iPhone."
                )
                .font(.callout).foregroundStyle(.secondary)
            }
            if let error = mobile.error {
                Text(error).foregroundStyle(.red)
                Button("Retry") { mobile.configure() }
            }
        }
        .frame(width: 496, alignment: .leading)
        .sheet(isPresented: $mobile.showPairing) { pairingSheet }
        .confirmationDialog(
            "Remove access for \(removing?.name ?? "this device")?",
            isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } })
        ) {
            Button("Remove Access", role: .destructive) {
                if let removing { mobile.remove(removing) }
                removing = nil
            }
        } message: {
            Text("It will need to scan a new QR code to connect again. Your terminal sessions keep running.")
        }
    }
    private var pairingSheet: some View {
        VStack(spacing: 16) {
            Text(mobile.pendingClient == nil ? "Pair Your iPhone" : "Allow This iPhone?").font(.title2.bold())
            if let client = mobile.pendingClient {
                Text("Allow \(client.name) to view and control this Mac’s terminal sessions?").multilineTextAlignment(
                    .center)
                Text("Only approve a pairing you just started.").foregroundStyle(.secondary)
                Button("Allow Access") { mobile.approve() }.disabled(mobile.pairingBusy)
            } else if let code = mobile.code {
                if let image = qrImage(code) {
                    Image(nsImage: image).interpolation(.none).resizable().frame(width: 250, height: 250)
                        .padding(16).background(.white).accessibilityLabel("Pairing QR code")
                }
                Text("Open Termy on your iPhone and tap Scan QR Code.")
                Text("Keep the iPhone on this Mac’s network or tailnet.")
                    .font(.caption).foregroundStyle(.secondary).multilineTextAlignment(.center)
                TimelineView(.periodic(from: .now, by: 1)) { context in
                    Text("Expires in \(max(0, Int(code.expiresAt.timeIntervalSince(context.date)))) seconds").font(
                        .caption
                    ).foregroundStyle(.secondary)
                }
            }
            if mobile.pairingBusy { ProgressView() }
            if let error = mobile.pairingError {
                Text(error).foregroundStyle(.red).multilineTextAlignment(.center)
                Button("Generate New Code") { mobile.beginPairing() }
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
