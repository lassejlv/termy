import SwiftUI
import TermyRemoteKit

struct SettingsView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var confirmDelete = false
    @State private var error: String?

    var body: some View {
        NavigationStack {
            Form {
                Section("Account") {
                    LabeledContent("Email", value: model.account?.email ?? "")
                    LabeledContent("Relay", value: model.account?.relayURL.host ?? "")
                }
                Section {
                    Text(model.identity.fingerprint)
                        .font(.system(.callout, design: .monospaced))
                        .textSelection(.enabled)
                } header: {
                    Text("This iPhone's key")
                } footer: {
                    Text("Your Mac asks to trust this key the first time this iPhone connects. Sessions are encrypted between the two; the relay can't read them.")
                }
                Section("Devices") {
                    ForEach(model.devices) { device in
                        HStack {
                            Image(systemName: device.kind == .host ? "desktopcomputer" : "iphone")
                                .foregroundStyle(.secondary)
                            VStack(alignment: .leading) {
                                Text(device.name + (device.id == model.account?.deviceID ? " (this iPhone)" : ""))
                                Text(device.fingerprint).font(.caption2.monospaced()).foregroundStyle(.secondary)
                            }
                        }
                    }
                }
                if let error {
                    Section { Text(error).foregroundStyle(.red) }
                }
                Section {
                    Button("Sign Out") {
                        Task {
                            await model.signOut()
                            dismiss()
                        }
                    }
                    Button("Delete Account", role: .destructive) { confirmDelete = true }
                }
            }
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } }
            }
            .confirmationDialog("Delete your Termy account?", isPresented: $confirmDelete, titleVisibility: .visible) {
                Button("Delete Account", role: .destructive) {
                    Task {
                        do {
                            try await model.deleteAccount()
                            dismiss()
                        } catch {
                            self.error = error.localizedDescription
                        }
                    }
                }
            } message: {
                Text("This removes the account and signs out every device. Your sessions keep running on your computers.")
            }
        }
    }
}
