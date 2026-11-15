import Observation
import SwiftUI
import TermyRemoteKit

/// A saved command, typed into the terminal from the key bar or the terminal menu.
struct Snippet: Codable, Hashable, Identifiable {
    var id = UUID()
    var name: String
    var command: String
    /// Presses Return after typing it.
    var runs = true

    var title: String { name.isEmpty ? command : name }
}

/// Saved commands, kept in Keychain: they may hold hosts, paths or tokens.
@MainActor @Observable
final class SnippetStore {
    static let shared = SnippetStore()

    private(set) var snippets: [Snippet] = []
    private(set) var lastError: String?
    @ObservationIgnored private let store = DeviceStore(service: "sh.termy.ios.snippets")
    @ObservationIgnored private var readFailed = false

    init() {
        do {
            snippets =
                try store.load([Snippet].self, key: "snippets") ?? [
                    Snippet(name: "Git status", command: "git status"),
                    Snippet(name: "Disk usage", command: "df -h"),
                ]
        } catch {
            // Don't overwrite snippets we failed to read.
            lastError = "Couldn’t read snippets from Keychain."
            readFailed = true
        }
    }

    func save(_ snippet: Snippet) {
        if let index = snippets.firstIndex(where: { $0.id == snippet.id }) {
            snippets[index] = snippet
        } else {
            snippets.append(snippet)
        }
        persist()
    }

    func delete(at offsets: IndexSet) {
        snippets.remove(atOffsets: offsets)
        persist()
    }

    func move(from source: IndexSet, to destination: Int) {
        snippets.move(fromOffsets: source, toOffset: destination)
        persist()
    }

    private func persist() {
        guard !readFailed else { return }
        do {
            try store.save(snippets, key: "snippets")
            lastError = nil
        } catch {
            lastError = "Couldn’t save snippets securely."
        }
    }
}

struct SnippetsSettingsView: View {
    @State private var store = SnippetStore.shared
    @State private var editing: Snippet?

    var body: some View {
        List {
            Section {
                ForEach(store.snippets) { snippet in
                    Button {
                        editing = snippet
                    } label: {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(snippet.title).foregroundStyle(.primary)
                            Text(snippet.command).font(.caption.monospaced()).foregroundStyle(.secondary)
                                .lineLimit(1)
                        }
                    }
                }
                .onDelete { store.delete(at: $0) }
                .onMove { store.move(from: $0, to: $1) }
            } footer: {
                Text("Run snippets from the key bar or the terminal menu. They’re stored in this iPhone’s Keychain.")
            }
            if let error = store.lastError { Text(error).foregroundStyle(.red) }
        }
        .navigationTitle("Snippets")
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button("Add Snippet", systemImage: "plus") { editing = Snippet(name: "", command: "") }
            }
            ToolbarItem(placement: .topBarTrailing) { EditButton() }
        }
        .sheet(item: $editing) { snippet in SnippetEditor(snippet: snippet) { store.save($0) } }
    }
}

private struct SnippetEditor: View {
    @State var snippet: Snippet
    let onSave: (Snippet) -> Void
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            Form {
                TextField("Name", text: $snippet.name)
                Section("Command") {
                    TextField("Command", text: $snippet.command, axis: .vertical)
                        .font(.body.monospaced()).textInputAutocapitalization(.never).autocorrectionDisabled()
                        .lineLimit(1...8)
                }
                Toggle("Press Return After Typing", isOn: $snippet.runs)
            }
            .navigationTitle(snippet.name.isEmpty ? "Snippet" : snippet.name).navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Save") {
                        onSave(snippet)
                        dismiss()
                    }
                    .disabled(snippet.command.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                }
            }
        }
    }
}
