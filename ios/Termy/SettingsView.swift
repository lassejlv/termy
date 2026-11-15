import SwiftUI
import TermyRemoteKit

struct SettingsView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var removing: RelayDevice?
    @State private var error: String?
    @AppStorage("predictiveEcho") private var predictiveEcho = true
    @State private var notifier = CommandNotifier.shared
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
                Section {
                    NavigationLink("Snippets") { SnippetsSettingsView() }
                    NavigationLink("Key Bar") { KeyBarSettingsView() }
                    Toggle("Predictive Typing", isOn: $predictiveEcho)
                } header: {
                    Text("Terminal")
                } footer: {
                    Text(
                        "On slow connections, shows what you type right away and underlines it until your Mac confirms it. Never used for passwords or programs that don’t echo."
                    )
                }
                Section {
                    // Turning it on is what asks iOS for permission, not launching the app.
                    Toggle(
                        "Notify When Commands Finish",
                        isOn: Binding(
                            get: { notifier.enabled },
                            set: { on in Task { await notifier.setEnabled(on) } }))
                    if notifier.denied {
                        Button("Allow Notifications in Settings") {
                            if let url = URL(string: UIApplication.openNotificationSettingsURLString) {
                                UIApplication.shared.open(url)
                            }
                        }
                    }
                } header: {
                    Text("Notifications")
                } footer: {
                    Text(
                        "Tells you when a command that ran \(Int(CommandNotifier.minimumDuration)) seconds or more finishes in a pane you aren’t looking at, while Termy is open. Needs shell integration on your Mac."
                    )
                }
                Section("Shortcuts") {
                    Text(
                        "Use “Run Command on Computer” in the Shortcuts app to run a command on your Mac from Siri, the Action button or an automation."
                    )
                    .font(.footnote).foregroundStyle(.secondary)
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
