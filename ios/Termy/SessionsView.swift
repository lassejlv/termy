import SwiftUI
import TermyRemoteKit

struct SessionsView: View {
    @Environment(AppModel.self) private var model
    let link: HostLink
    @State private var openScreen: TerminalScreen?
    @State private var closing: RemotePane?

    var body: some View {
        List {
            switch link.state {
            case .needsTrust:
                TrustSection(link: link, changed: false)
            case .keyChanged:
                TrustSection(link: link, changed: true)
            case .offline:
                StatusRow(symbol: "moon.zzz", text: "\(link.device.name) is offline. Open Termy Next with Mobile access enabled.")
            case .failed(let message):
                StatusRow(symbol: "exclamationmark.triangle", text: message)
                Button("Try Again") { link.connect() }
            case .connecting, .idle:
                HStack { ProgressView(); Text("Connecting securely…").foregroundStyle(.secondary) }
            case .connected:
                sessionSections
            }
            if let error = link.lastError {
                Text(error).font(.footnote).foregroundStyle(.red)
            }
        }
        .navigationTitle(link.hostName ?? link.device.name)
        .toolbar {
            if link.state == .connected {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("New Session", systemImage: "plus") { link.send(.createPane) }
                }
            }
        }
        .onAppear { link.connect() }
        .onChange(of: link.createdPaneID) { _, id in
            guard let id else { return }
            link.createdPaneID = nil
            openScreen = link.open(RemotePane(id: id, title: nil, workingDirectory: nil, exited: false))
        }
        .fullScreenCover(item: $openScreen) { screen in
            TerminalScreenView(link: link, screen: screen)
        }
        .confirmationDialog(
            "End this session?", isPresented: Binding(get: { closing != nil }, set: { if !$0 { closing = nil } }),
            titleVisibility: .visible, presenting: closing
        ) { pane in
            Button("End Session", role: .destructive) { link.send(.closePane(paneID: pane.id)) }
        } message: { _ in
            Text("The program running in it stops.")
        }
    }

    @ViewBuilder
    private var sessionSections: some View {
        let grouped = Set(link.sessions.groups.flatMap { $0.tabs.flatMap(\.paneIDs) })
        ForEach(link.sessions.groups) { group in
            Section(group.name) {
                ForEach(Array(group.tabs.enumerated()), id: \.offset) { index, tab in
                    ForEach(tab.paneIDs, id: \.self) { id in
                        if let pane = link.pane(id) {
                            paneRow(pane, fallback: tab.title ?? "Tab \(index + 1)")
                        }
                    }
                }
            }
        }
        let loose = link.sessions.panes.filter { !grouped.contains($0.id) && !$0.exited }
        if !loose.isEmpty {
            Section(link.sessions.groups.isEmpty ? "Sessions" : "Other") {
                ForEach(loose) { paneRow($0, fallback: "Shell") }
            }
        }
        if link.sessions.panes.isEmpty {
            Text("No sessions yet. Tap + to start one.").foregroundStyle(.secondary)
        }
    }

    private func paneRow(_ pane: RemotePane, fallback: String) -> some View {
        Button {
            openScreen = link.open(pane)
        } label: {
            VStack(alignment: .leading, spacing: 2) {
                Text(pane.title?.isEmpty == false ? pane.title! : fallback)
                    .foregroundStyle(pane.exited ? .secondary : .primary)
                    .lineLimit(1)
                if let directory = pane.workingDirectory {
                    Text(Self.abbreviate(directory))
                        .font(.caption.monospaced())
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.head)
                }
            }
        }
        .swipeActions {
            Button("End", role: .destructive) { closing = pane }
        }
    }

    static func abbreviate(_ path: String) -> String {
        let path = path.hasPrefix("file://") ? (URL(string: path)?.path ?? path) : path
        if let range = path.range(of: #"^/Users/[^/]+"#, options: .regularExpression) {
            return "~" + path[range.upperBound...]
        }
        return path
    }
}

struct StatusRow: View {
    let symbol: String
    let text: String

    var body: some View {
        Label(text, systemImage: symbol).foregroundStyle(.secondary)
    }
}

/// Pairing pins the computer key; a changed key always requires a fresh QR exchange.
struct TrustSection: View {
    let link: HostLink
    let changed: Bool
    var body: some View {
        Section {
            Label(changed ? "This computer’s key changed" : "Pair this computer again", systemImage: "exclamationmark.shield")
                .foregroundStyle(.orange)
            Text("Remove this computer in Settings, then scan a new QR code from Termy Next → Settings → Mobile.")
                .foregroundStyle(.secondary)
        }
    }
}
