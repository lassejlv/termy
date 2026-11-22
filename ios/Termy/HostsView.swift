import SwiftUI
import TermyRemoteKit

struct HostsView: View {
    @Environment(AppModel.self) private var model
    @State private var showSettings = false
    @State private var showPairing = false
    @State private var path: [UUID] = []
    /// The computer open when the app was last used, restored on launch.
    @SceneStorage("openHost") private var openHost = ""
    var body: some View {
        NavigationStack(path: $path) {
            List {
                if let error = model.lastError {
                    Section { Label(error, systemImage: "exclamationmark.triangle").foregroundStyle(.orange) }
                }
                if model.hosts.isEmpty, model.relayPaired.isEmpty {
                    Section {
                        VStack(alignment: .leading, spacing: 14) {
                            Image(systemName: "macbook.and.iphone").font(.largeTitle)
                            Text("Connect Your Mac").font(.title2.bold())
                            Text(
                                "Termy connects to your Mac over Tailscale: run it on this iPhone and the Mac, on the same tailnet. Then, in Termy Next, open Settings → Mobile, choose Pair iPhone… and scan the QR code here."
                            ).foregroundStyle(.secondary)
                            Button("Scan QR Code", systemImage: "qrcode.viewfinder") { showPairing = true }
                        }.padding(.vertical, 12)
                    }
                } else {
                    Section {
                        ForEach(model.hosts) { host in
                            NavigationLink(value: host.id) {
                                HostRow(host: host, connected: model.connectedHosts.contains(host.id))
                            }
                        }
                    }
                }
                if !model.relayPaired.isEmpty {
                    Section {
                        ForEach(model.relayPaired) { host in
                            RelayPairedRow(host: host)
                        }
                    } header: {
                        Text("Pair Again")
                    } footer: {
                        Text("These were paired through the relay, which Termy no longer uses. Update Termy Next on the Mac, then pair it again.")
                    }
                }
            }
            .navigationTitle("Computers")
            .navigationDestination(for: UUID.self) { id in
                if let host = model.hosts.first(where: { $0.id == id }) { SessionsView(link: model.link(for: host)) }
            }
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("Settings", systemImage: "gearshape") { showSettings = true }
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Pair Computer", systemImage: "qrcode.viewfinder") { showPairing = true }
                }
            }
            .sheet(isPresented: $showSettings) { SettingsView() }
            .sheet(isPresented: $showPairing) { PairComputerView() }
        }
        .onAppear {
            if let route = model.route {
                follow(route)
            } else if path.isEmpty, let id = UUID(uuidString: openHost), model.hosts.contains(where: { $0.id == id }) {
                path = [id]
            }
            if model.pairingRequested { openPairing() }
        }
        .onChange(of: model.route) { _, route in if let route { follow(route) } }
        .onChange(of: model.pairingRequested) { _, requested in if requested { openPairing() } }
        .onChange(of: path) { _, path in openHost = path.last?.uuidString ?? "" }
    }

    private func follow(_ route: Route) {
        showSettings = false
        showPairing = false
        if path.last != route.host { path = [route.host] }
    }

    private func openPairing() {
        model.pairingRequested = false
        path = []
        showSettings = false
        showPairing = true
    }
}
struct HostRow: View {
    let host: PairedDevice
    /// Set while the phone has a connection to the computer.
    var connected = false
    private var reachable: Bool { connected }
    private var status: String { connected ? "Connected · Tailscale" : "Over Tailscale" }
    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: "desktopcomputer").font(.title2).foregroundStyle(
                reachable ? Color.accentColor : .secondary
            ).frame(width: 34)
            VStack(alignment: .leading, spacing: 2) {
                Text(host.name).font(.body.weight(.medium))
                Text(status).font(.caption).foregroundStyle(reachable ? .green : .secondary)
            }
        }.padding(.vertical, 4)
    }
}

/// A computer paired through the relay: it can't be reached until it is paired again.
struct RelayPairedRow: View {
    @Environment(AppModel.self) private var model
    let host: PairedDevice
    @State private var error: String?
    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: "desktopcomputer.trianglebadge.exclamationmark").font(.title2).foregroundStyle(.orange)
                .frame(width: 34)
            VStack(alignment: .leading, spacing: 2) {
                Text(host.name).font(.body.weight(.medium))
                Text(error ?? "Paired through the relay · Pair again").font(.caption).foregroundStyle(.secondary)
            }
            Spacer()
            Button("Pair Again") {
                do { try model.repair(host) } catch { self.error = error.localizedDescription }
            }
            .buttonStyle(.bordered)
        }.padding(.vertical, 4)
    }
}
