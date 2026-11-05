import SwiftUI
import TermyRemoteKit

struct HostsView: View {
    @Environment(AppModel.self) private var model
    @State private var showSettings = false

    var body: some View {
        NavigationStack {
            List {
                if model.hosts.isEmpty {
                    Section {
                        SetupHint()
                    }
                } else {
                    Section {
                        ForEach(model.hosts) { host in
                            NavigationLink(value: host.id) {
                                HostRow(host: host)
                            }
                        }
                    } footer: {
                        if model.relayState != .connected {
                            Label(model.relayState == .connecting ? "Connecting to the relay…" : "Offline", systemImage: "wifi.exclamationmark")
                        }
                    }
                }
            }
            .navigationTitle("Computers")
            .navigationDestination(for: UUID.self) { id in
                if let host = model.hosts.first(where: { $0.id == id }) {
                    SessionsView(link: model.link(for: host))
                }
            }
            .refreshable { await model.refreshDevices() }
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Settings", systemImage: "gearshape") { showSettings = true }
                }
            }
            .sheet(isPresented: $showSettings) { SettingsView() }
            .task { await model.refreshDevices() }
        }
    }
}

struct HostRow: View {
    let host: RelayDevice

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: "desktopcomputer")
                .font(.title2)
                .foregroundStyle(host.online ? Color.accentColor : .secondary)
                .frame(width: 34)
            VStack(alignment: .leading, spacing: 2) {
                Text(host.name).font(.body.weight(.medium))
                Text(host.online ? "Online" : lastSeen)
                    .font(.caption)
                    .foregroundStyle(host.online ? .green : .secondary)
            }
        }
        .padding(.vertical, 4)
    }

    private var lastSeen: String {
        guard let date = host.lastSeenAt else { return "Offline" }
        return "Last seen \(date.formatted(.relative(presentation: .named)))"
    }
}

struct SetupHint: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Label("Connect your Mac", systemImage: "desktopcomputer").font(.headline)
            Text("On your Mac, sign in with the same account and start serving your Termy Next sessions:")
                .font(.subheadline)
                .foregroundStyle(.secondary)
            VStack(alignment: .leading, spacing: 6) {
                Text("termy remote login")
                Text("termy remote serve")
            }
            .font(.system(.callout, design: .monospaced))
            .padding(10)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(.quaternary, in: RoundedRectangle(cornerRadius: 8))
            Text("Pull down to refresh once it's running.")
                .font(.footnote)
                .foregroundStyle(.secondary)
        }
        .padding(.vertical, 6)
    }
}
