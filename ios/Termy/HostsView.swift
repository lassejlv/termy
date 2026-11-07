import SwiftUI
import TermyRemoteKit

struct HostsView: View {
    @Environment(AppModel.self) private var model
    @State private var showSettings = false
    @State private var showPairing = false
    var body: some View {
        NavigationStack {
            List {
                if let error = model.lastError {
                    Section { Label(error, systemImage: "exclamationmark.triangle").foregroundStyle(.orange) }
                }
                if model.hosts.isEmpty {
                    Section {
                        VStack(alignment: .leading, spacing: 14) {
                            Image(systemName: "macbook.and.iphone").font(.largeTitle)
                            Text("Connect Your Mac").font(.title2.bold())
                            Text(
                                "In Termy Next, open Settings → Mobile and choose Pair iPhone. Then scan the QR code here."
                            ).foregroundStyle(.secondary)
                            Button("Scan QR Code", systemImage: "qrcode.viewfinder") { showPairing = true }
                        }.padding(.vertical, 12)
                    }
                } else {
                    Section {
                        ForEach(model.hosts) { host in NavigationLink(value: host.id) { HostRow(host: host) } }
                    } footer: {
                        if model.relayState != .connected {
                            Text(model.relayState == .connecting ? "Connecting…" : "Offline")
                        }
                    }
                }
            }
            .navigationTitle("Computers")
            .navigationDestination(for: UUID.self) { id in
                if let host = model.hosts.first(where: { $0.id == id }) { SessionsView(link: model.link(for: host)) }
            }
            .refreshable { await model.refreshDevices() }
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
            .task { await model.refreshDevices() }
        }
    }
}
struct HostRow: View {
    let host: RelayDevice
    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: "desktopcomputer").font(.title2).foregroundStyle(
                host.online ? Color.accentColor : .secondary
            ).frame(width: 34)
            VStack(alignment: .leading, spacing: 2) {
                Text(host.name).font(.body.weight(.medium))
                Text(host.online ? "Online" : "Offline").font(.caption).foregroundStyle(
                    host.online ? .green : .secondary)
            }
        }.padding(.vertical, 4)
    }
}
