import SwiftUI
import TermyRemoteKit

struct SettingsView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var removing: RelayDevice?
    @State private var error: String?
    var body: some View {
        NavigationStack {
            Form {
                Section("Paired Computers") {
                    ForEach(model.hosts) { host in
                        VStack(alignment: .leading, spacing: 6) {
                            Text(host.name)
                            Text(model.relayURL(for: host.id)?.host ?? "").font(.caption).foregroundStyle(.secondary)
                            Button("Remove Computer", role: .destructive) { removing = host }
                        }
                    }
                    if model.hosts.isEmpty { Text("No paired computers").foregroundStyle(.secondary) }
                }
                Section("This iPhone’s Key") {
                    Text(model.identity.fingerprint).font(.system(.caption, design: .monospaced)).textSelection(
                        .enabled)
                }
                Section {
                    Text(
                        "Sessions are encrypted between your iPhone and Mac. The relay cannot read them. To use your own relay, set its HTTPS URL in Termy Next → Settings → Mobile before pairing."
                    )
                    .font(.footnote).foregroundStyle(.secondary)
                }
                if let error { Section { Text(error).foregroundStyle(.red) } }
            }
            .navigationTitle("Settings").navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } } }
            .confirmationDialog(
                "Remove this computer?",
                isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } }), presenting: removing
            ) { host in
                Button("Remove Computer", role: .destructive) {
                    Task { do { try await model.removeHost(host) } catch { self.error = error.localizedDescription } }
                }
            } message: { _ in
                Text("You’ll need to scan a new QR code to connect again. Your terminal sessions keep running.")
            }
        }
    }
}
